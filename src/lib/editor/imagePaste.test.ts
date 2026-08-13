// @vitest-environment happy-dom
//
// The paste placement contract ([shell.md] §Writing surface): a pasted
// image goes where typed text would. An empty block is replaced
// outright (an empty bullet becomes the image's bullet), a non-empty one
// takes the image right after it, still nested under its bullet, and the
// caret then moves below the last image onto a fresh bullet or line, so
// writing continues under the screenshot. Driven through the extension's
// real handlePaste with a synthetic clipboard; the backend save is
// mocked at the tauri bridge.

import { beforeEach, describe, expect, it, vi } from 'vitest';
import { Editor } from '@tiptap/core';
import { TextSelection } from '@tiptap/pm/state';
import { markdownExtensions } from './extensions';
import { imagePaste } from './imagePaste';

const saved: string[] = [];
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => {
    saved.push('asset');
    return `assets/m1/img-${String(saved.length).padStart(2, '0')}.png`;
  })
}));

function makeEditor(content: string): Editor {
  const element = document.createElement('div');
  document.body.appendChild(element);
  return new Editor({
    element,
    extensions: [...markdownExtensions(), imagePaste({ meetingId: () => 'm1' })],
    content
  });
}

function pasteFiles(editor: Editor, count: number): boolean {
  const dt = new DataTransfer();
  for (let i = 0; i < count; i++) {
    dt.items.add(new File([new Uint8Array([137, 80])], `p${i}.png`, { type: 'image/png' }));
  }
  const event = new ClipboardEvent('paste', { clipboardData: dt });
  return editor.view.someProp('handlePaste', (f) => f(editor.view, event, null as never)) ?? false;
}

/** Caret to the very end of the text (the position math the tests need is
 * the doc's, not a hand-counted offset). */
function caretToEnd(editor: Editor) {
  const end = TextSelection.atEnd(editor.state.doc);
  editor.view.dispatch(editor.state.tr.setSelection(end));
}

/** Wait until the mocked saves have all swapped their placeholders in. */
async function settled(editor: Editor, images: number) {
  for (let i = 0; i < 50; i++) {
    const md = editor.storage.markdown.getMarkdown();
    if ((md.match(/!\[[^\]]*\]\(assets\//g) ?? []).length >= images) return;
    await new Promise((r) => setTimeout(r, 10));
  }
}

beforeEach(() => {
  saved.length = 0;
  document.body.innerHTML = '';
});

describe('image paste placement', () => {
  it('a mid-sentence paste keeps the sentence whole and the caret moves below', async () => {
    const editor = makeEditor('alpha beta gamma');
    // Caret between beta and gamma.
    const pos = editor.state.doc.textContent.indexOf(' gamma') + 1;
    editor.commands.setTextSelection(pos);

    expect(pasteFiles(editor, 1)).toBe(true);
    await settled(editor, 1);

    // The sentence never splits around a screenshot.
    expect(editor.storage.markdown.getMarkdown()).toContain(
      'alpha beta gamma\n\n![](assets/m1/img-01.png)'
    );
    // The caret sits on the fresh line below the image: typing continues
    // under the screenshot, not back inside the sentence.
    expect(editor.state.selection).toBeInstanceOf(TextSelection);
    editor.commands.insertContent('below');
    expect(editor.storage.markdown.getMarkdown()).toBe(
      'alpha beta gamma\n\n![](assets/m1/img-01.png)\n\nbelow'
    );
    editor.destroy();
  });

  it('a paste inside a bullet stays nested and the caret takes a new bullet', async () => {
    const editor = makeEditor('- point one\n- point two');
    // Caret inside the first item's text.
    editor.commands.setTextSelection(4);

    expect(pasteFiles(editor, 1)).toBe(true);
    await settled(editor, 1);

    const md = editor.storage.markdown.getMarkdown();
    // Indented under its bullet (the 2-space continuation), with the
    // second item intact after it. Exact blank-line shape depends on the
    // list's tightness, which is not this test's business.
    expect(md).toContain('- point one\n\n  ![](assets/m1/img-01.png)');
    expect(md).toContain('- point two');
    // The caret took a fresh bullet between the image's item and "point
    // two"; typing writes that bullet.
    editor.commands.insertContent('here');
    const after = editor.storage.markdown.getMarkdown();
    expect(after).toContain('- here');
    expect(after.indexOf('img-01.png')).toBeLessThan(after.indexOf('here'));
    expect(after.indexOf('here')).toBeLessThan(after.indexOf('point two'));
    editor.destroy();
  });

  it('a paste on an empty bullet becomes the bullet, caret on a fresh one', async () => {
    const editor = makeEditor('- point one');
    caretToEnd(editor);
    // The empty bullet a user just opened with Enter.
    editor.commands.splitListItem('listItem');

    expect(pasteFiles(editor, 1)).toBe(true);
    await settled(editor, 1);

    // The image is itself the second bullet, with no empty text line above it.
    expect(editor.storage.markdown.getMarkdown()).toContain('- ![](assets/m1/img-01.png)');
    editor.commands.insertContent('next');
    expect(editor.storage.markdown.getMarkdown()).toContain('- next');
    editor.destroy();
  });

  it('a paste on an empty task item nests below instead (no image-only task)', async () => {
    const editor = makeEditor('- [ ] open');
    caretToEnd(editor);
    editor.commands.splitListItem('taskItem');

    expect(pasteFiles(editor, 1)).toBe(true);
    await settled(editor, 1);

    // Markdown cannot write an image-only task, so the empty paragraph
    // stays and the image nests under it, and nothing trips the schema.
    const md = editor.storage.markdown.getMarkdown();
    expect(md).toContain('[ ] open');
    expect(md).toContain('img-01.png');
    editor.destroy();
  });

  it('a paste on an empty line takes the line, caret on a fresh one below', async () => {
    const editor = makeEditor('intro');
    caretToEnd(editor);
    // The empty line a user just opened with Enter.
    editor.commands.splitBlock();

    expect(pasteFiles(editor, 1)).toBe(true);
    await settled(editor, 1);

    // No empty paragraph above the image (it was replaced), and the caret
    // is on the fresh line under it.
    editor.commands.insertContent('tail');
    expect(editor.storage.markdown.getMarkdown()).toBe(
      'intro\n\n![](assets/m1/img-01.png)\n\ntail'
    );
    editor.destroy();
  });

  it('two files in one paste land in order, caret below the last', async () => {
    const editor = makeEditor('intro');
    editor.commands.setTextSelection(3);

    expect(pasteFiles(editor, 2)).toBe(true);
    await settled(editor, 2);

    editor.commands.insertContent('zed');
    const md = editor.storage.markdown.getMarkdown();
    const first = md.indexOf('img-01.png');
    const second = md.indexOf('img-02.png');
    expect(first).toBeGreaterThan(-1);
    expect(second).toBeGreaterThan(first);
    expect(md.indexOf('zed')).toBeGreaterThan(second);
    editor.destroy();
  });

  it('a failed save removes the placeholder and reports', async () => {
    const { invoke } = await import('@tauri-apps/api/core');
    (invoke as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new Error('disk full'));
    const errors: string[] = [];
    const element = document.createElement('div');
    document.body.appendChild(element);
    const editor = new Editor({
      element,
      extensions: [
        ...markdownExtensions(),
        imagePaste({ meetingId: () => 'm1', onError: (m) => errors.push(m) })
      ],
      content: 'text'
    });
    editor.commands.setTextSelection(3);

    expect(pasteFiles(editor, 1)).toBe(true);
    await new Promise((r) => setTimeout(r, 100));

    expect(editor.storage.markdown.getMarkdown()).toBe('text');
    expect(errors).toEqual(['disk full']);
    editor.destroy();
  });

  it('a failed save into an empty bullet takes the whole item with it', async () => {
    const { invoke } = await import('@tauri-apps/api/core');
    (invoke as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new Error('disk full'));
    const editor = makeEditor('- one');
    caretToEnd(editor);
    editor.commands.splitListItem('listItem');

    expect(pasteFiles(editor, 1)).toBe(true);
    await new Promise((r) => setTimeout(r, 100));

    // The placeholder was that item's only child; deleting just the node
    // would leave a child-less item the schema refuses. The item goes
    // with it, and the caret's fresh bullet remains writable.
    const md = editor.storage.markdown.getMarkdown();
    expect(md).not.toContain('![');
    expect(md).toContain('- one');
    editor.commands.insertContent('still here');
    expect(editor.storage.markdown.getMarkdown()).toContain('still here');
    editor.destroy();
  });
});
