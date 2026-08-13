// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest';
import { Editor } from '@tiptap/core';
import { markdownExtensions } from './extensions';
import { findImagePos, findMatchIndex, findTextPos } from './locate';

/** A real document through the real schema: the same path a tab takes when
 * it mounts, so what the locator walks is what the user is looking at. */
function docFrom(markdown: string) {
  const element = document.createElement('div');
  const editor = new Editor({ element, extensions: markdownExtensions(), content: markdown });
  const { doc } = editor.state;
  editor.destroy();
  return doc;
}

describe('findTextPos', () => {
  const markdown = [
    '# Planning Sync',
    '',
    'We opened with the budget and nobody objected.',
    '',
    'Then hiring: two engineers, starting in October.',
    '',
    '- a bullet that mentions the vendor',
    ''
  ].join('\n');

  it('finds the paragraph a passage opens with', () => {
    const doc = docFrom(markdown);
    const pos = findTextPos(doc, 'Then hiring: two engineers, starting in October.');
    expect(pos).not.toBeNull();
    expect(doc.nodeAt(pos!)?.textContent).toContain('Then hiring');
  });

  it('ignores case and whitespace, which the indexer normalizes anyway', () => {
    const doc = docFrom(markdown);
    expect(findTextPos(doc, '  THEN   HIRING: two engineers, starting in October.  ')).not.toBeNull();
  });

  it('finds text inside a list item like any other block', () => {
    const doc = docFrom(markdown);
    const pos = findTextPos(doc, 'a bullet that mentions the vendor');
    expect(pos).not.toBeNull();
    expect(doc.nodeAt(pos!)?.textContent).toContain('vendor');
  });

  /** The indexer strips an image link to its alt text, which the editor
   * holds as a node rather than characters, so the two sides genuinely
   * disagree about this paragraph, and an exact match would fail. */
  it('still lands when the passage and the document disagree about an image', () => {
    const doc = docFrom('the numbers are in ![the chart](assets/m1/img-01.png) as promised');
    const pos = findTextPos(doc, 'the numbers are in the chart as promised');
    expect(pos).not.toBeNull();
    expect(doc.nodeAt(pos!)?.textContent).toContain('the numbers are in');
  });

  /** The lead is a line of markdown source; the document holds its rendered
   * text. A bullet's `- `, a heading's `## `, and `**bold**` asterisks are
   * all syntax the editor turns into structure, so they exist on one side
   * of the match and not the other. A real summary is almost entirely
   * these, which is exactly where this first failed. */
  it('matches a bullet whose lead still carries its markdown marker', () => {
    const doc = docFrom('## Key Takeaways\n\n- Matt reviewed the deck and endorsed it.\n- Doug scored it 85.');
    const pos = findTextPos(doc, '- Matt reviewed the deck and endorsed it.');
    expect(pos).not.toBeNull();
    expect(doc.nodeAt(pos!)?.textContent).toContain('Matt reviewed');
  });

  it('matches a heading whose lead still carries its hashes', () => {
    const doc = docFrom('## Next Steps\n\n- something');
    expect(findTextPos(doc, '## Next Steps')).not.toBeNull();
  });

  it('matches through bold, which the document renders as a mark', () => {
    const doc = docFrom('- **Malina** Squeeze the workforce specialist bullet.');
    expect(findTextPos(doc, '- **Malina** Squeeze the workforce specialist bullet.')).not.toBeNull();
  });

  it('matches through a link, whose URL is an attribute and not text', () => {
    const doc = docFrom('see [the spec](https://embral.app/spec) for more');
    expect(findTextPos(doc, 'see [the spec](https://embral.app/spec) for more')).not.toBeNull();
  });

  it('returns null when the text is gone, so the caller simply does not scroll', () => {
    const doc = docFrom(markdown);
    expect(findTextPos(doc, 'a sentence nobody ever wrote here')).toBeNull();
    expect(findTextPos(doc, '   ')).toBeNull();
  });

  /** A common word occurs all over a long document; only the one inside
   * the passage the palette showed is the right one. `from` is what bounds
   * it; searching "north" without it stops at "North Carolina". */
  it('starts looking where it is told to', () => {
    const doc = docFrom(
      'Throughout North Carolina, right?\n\nsomething else\n\nKathy said Northstar, Northstar.'
    );
    const first = findTextPos(doc, 'north');
    expect(doc.nodeAt(first!)?.textContent).toContain('North Carolina');

    const passage = findTextPos(doc, 'Kathy said Northstar');
    const scoped = findTextPos(doc, 'north', passage!);
    expect(doc.nodeAt(scoped!)?.textContent).toContain('Northstar');
  });

  it('takes the first of two identical openings rather than throwing', () => {
    const doc = docFrom('the same line\n\nsomething else\n\nthe same line');
    const first = findTextPos(doc, 'the same line');
    expect(first).toBe(findTextPos(docFrom('the same line\n\nsomething else\n\nthe same line'), 'the same line'));
    expect(first).not.toBeNull();
  });
});

/** A transcript is segments, not a document: the same match against a
 * list. It exists because a passage's `start_secs` is where the passage
 * begins, which for packed paragraphs is routinely minutes before the line
 * the user searched for. */
describe('findMatchIndex', () => {
  const segments = [
    'Okay, we are off to the races.',
    'Matt reviewed the deck and endorsed it.',
    'Doug agent scored the deck 85, a strong signal.',
    'What did you just say?'
  ];

  it('finds the entry the words are in, not the one the passage starts at', () => {
    expect(findMatchIndex(segments, 'Doug agent scored the deck')).toBe(2);
    expect(findMatchIndex(segments, 'What did you just')).toBe(3);
  });

  it('ignores case and punctuation, like the document match', () => {
    expect(findMatchIndex(segments, 'WHAT DID YOU JUST SAY')).toBe(3);
  });

  it('falls back to a shorter opening before giving up', () => {
    // Only the first words survive; the tail is the user's own phrasing.
    expect(findMatchIndex(segments, 'Matt reviewed the deck at some length')).toBe(1);
  });

  it('returns null when the words are absent, so the caller can fall back', () => {
    expect(findMatchIndex(segments, 'nothing like this was ever said')).toBeNull();
    expect(findMatchIndex(segments, '   ')).toBeNull();
    expect(findMatchIndex([], 'anything')).toBeNull();
  });
});

describe('findImagePos', () => {
  const markdown = [
    'first ![one](assets/260326T143000_a3f9b2/img-01.png)',
    '',
    'second ![two](assets/260326T143000_a3f9b2/img-02.png)'
  ].join('\n');

  it('finds an image by the filename a hit names', () => {
    const doc = docFrom(markdown);
    const pos = findImagePos(doc, 'img-02.png');
    expect(pos).not.toBeNull();
    expect(doc.nodeAt(pos!)?.type.name).toBe('image');
    expect(String(doc.nodeAt(pos!)?.attrs.src)).toContain('img-02.png');
  });

  /** Images outlive their links by design: the file is kept so the summary
   * cannot break when the notes drop it. So a hit can name an image this
   * document no longer shows, and that has to be quiet, not an error. */
  it('returns null for an image the document no longer shows', () => {
    expect(findImagePos(docFrom(markdown), 'img-09.png')).toBeNull();
    expect(findImagePos(docFrom(markdown), '')).toBeNull();
  });
});
