// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest';
import { Editor } from '@tiptap/core';
import { markdownExtensions } from './extensions';

/** Parse markdown into a real editor and serialize it straight back — the
 * exact path a document takes when a tab mounts and the user types once. */
function roundTrip(markdown: string): string {
  const element = document.createElement('div');
  const editor = new Editor({ element, extensions: markdownExtensions(), content: markdown });
  const out = editor.storage.markdown.getMarkdown();
  editor.destroy();
  return out;
}

/** Everything in the supported contract, one construct per case. Formatting
 * is allowed to normalize (`*` vs `-` bullets, spacing); *content* is not
 * allowed to disappear, which is what each `keeps` string checks for. */
const CORPUS: { name: string; markdown: string; keeps: string[] }[] = [
  { name: 'headings', markdown: '# One\n\n## Two\n\n### Three', keeps: ['# One', '## Two', '### Three'] },
  { name: 'emphasis', markdown: 'plain **bold** and *italic* and ~~struck~~', keeps: ['**bold**', 'italic', '~~struck~~'] },
  { name: 'inline code', markdown: 'run `cargo test` now', keeps: ['`cargo test`'] },
  { name: 'code blocks', markdown: '```rust\nfn main() {}\n```', keeps: ['```', 'fn main() {}'] },
  { name: 'blockquotes', markdown: '> someone said this', keeps: ['> someone said this'] },
  { name: 'horizontal rules', markdown: 'above\n\n---\n\nbelow', keeps: ['---'] },
  { name: 'bullet lists', markdown: '- one\n- two\n  - nested', keeps: ['one', 'two', 'nested'] },
  { name: 'ordered lists', markdown: '1. first\n2. second', keeps: ['1.', 'first', 'second'] },
  // The four that StarterKit alone silently deleted.
  { name: 'links', markdown: 'see [the spec](https://embral.app/spec) for more', keeps: ['[the spec](https://embral.app/spec)'] },
  { name: 'images', markdown: 'before ![shot](assets/260326T143000_a3f9b2/img-01.png) after', keeps: ['![shot](assets/260326T143000_a3f9b2/img-01.png)'] },
  { name: 'tables', markdown: '| a | b |\n| --- | --- |\n| 1 | 2 |', keeps: ['| a', '| b', '| 1', '| 2'] },
  { name: 'task lists', markdown: '- [ ] open\n- [x] done', keeps: ['[ ] open', '[x] done'] },
  // A pasted screenshot can BE a bullet (imagePaste.ts): list items admit
  // an image as their opening child, and that shape must survive a save
  // without growing an empty text line above the image. (Task items are
  // deliberately not in this corpus: markdown cannot write an image-only
  // task, so their schema still opens with a paragraph.)
  {
    name: 'an image as a whole bullet',
    markdown: '- ![shot](assets/260326T143000_a3f9b2/img-01.png)\n- after',
    keeps: ['- ![shot](assets/260326T143000_a3f9b2/img-01.png)', '- after']
  }
];

describe('the markdown contract', () => {
  for (const { name, markdown, keeps } of CORPUS) {
    it(`keeps ${name} through a round trip`, () => {
      const out = roundTrip(markdown);
      for (const fragment of keeps) {
        expect(out, `${name}: lost ${JSON.stringify(fragment)} — got ${JSON.stringify(out)}`).toContain(
          fragment
        );
      }
    });
  }

  it('is stable: a second round trip changes nothing', () => {
    // Normalization on the first pass is fine; drift on every save is not.
    const all = CORPUS.map((c) => c.markdown).join('\n\n');
    const once = roundTrip(all);
    expect(roundTrip(once)).toBe(once);
  });

  it('keeps an image link that a summary references', () => {
    // The specific loss that would land the moment the LLM places an image:
    // no image node meant markdown-it produced an <img> with no parse rule,
    // and the first transaction in the Summary tab saved it away.
    const summary = [
      '# Q3 Review',
      '',
      '## Key Takeaways',
      '',
      '- The pipeline chart is the headline:',
      '',
      '![pipeline](assets/260326T143000_a3f9b2/img-02.png)'
    ].join('\n');
    expect(roundTrip(summary)).toContain('assets/260326T143000_a3f9b2/img-02.png');
  });

  // Images are block nodes ([shell.md] §Writing surface). The stock
  // serializer never closed the image's block — the next paragraph came
  // out glued onto the image's line — so these pin the corrected spec.
  it('a block image keeps its distance from the next paragraph', () => {
    const doc = 'one\n\n![shot](assets/a/img-01.png)\n\ntwo';
    expect(roundTrip(doc)).toBe(doc);
  });

  it('a mid-sentence image becomes blocks without artifact growth', () => {
    const once = roundTrip('before ![shot](assets/a/img-01.png) after');
    expect(once).toBe('before\n\n![shot](assets/a/img-01.png)\n\nafter');
    expect(roundTrip(once)).toBe(once);
  });

  it('an image nested in a list item stays in its item', () => {
    const doc = '- point one\n\n  ![shot](assets/a/img-01.png)\n\n- point two';
    expect(roundTrip(doc)).toBe(doc);
  });

  it('an image alone on a tight bullet normalizes once and holds', () => {
    // listItem content is `paragraph block*`, so a marker-line image gets
    // hoisted out of the (now empty) item — content preserved, and the
    // shape is stable from the first pass on.
    const once = roundTrip('- ![shot](assets/a/img-01.png)');
    expect(once).toContain('![shot](assets/a/img-01.png)');
    expect(roundTrip(once)).toBe(once);
  });
});
