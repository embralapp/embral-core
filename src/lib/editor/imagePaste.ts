import { Extension } from '@tiptap/core';
import { Plugin, PluginKey } from '@tiptap/pm/state';
import { invoke } from '@tauri-apps/api/core';

/** Pasting an image into a document.
 *
 * The bytes go to the backend first and the node gets its real `src` when
 * that returns, so what the editor serializes is always a link to a file
 * that exists. While the save is in flight a placeholder node holds the
 * spot, keyed by a token so an undo (or a second paste landing first) can
 * never make us overwrite the wrong node.
 *
 * Mirrors `starGutter.ts`: the machinery lives here, not in the component. */

const key = new PluginKey('embral-image-paste');

/** Where the bytes go. `meetingId` absent = the recording happening now,
 * which the backend resolves from the recovery scratch. */
async function saveAsset(bytes: ArrayBuffer, meetingId?: string): Promise<string> {
  return invoke<string>('save_note_asset', bytes, {
    headers: meetingId ? { 'x-meeting-id': meetingId } : {}
  });
}

let nextToken = 1;

export interface ImagePasteOptions {
  /** The meeting this document belongs to; omit while recording. */
  meetingId?: () => string | undefined;
  /** Told when a paste fails, so the surface can say so in its own voice. */
  onError?: (message: string) => void;
}

export const imagePaste = (options: ImagePasteOptions = {}) =>
  Extension.create({
    name: 'embralImagePaste',
    addProseMirrorPlugins() {
      const editor = this.editor;
      return [
        new Plugin({
          key,
          props: {
            handlePaste(view, event) {
              const files = imageFilesFrom(event.clipboardData);
              if (files.length === 0) return false;
              // Take it: a screenshot paste usually carries text/plain
              // alongside, and letting that through would drop a file name
              // into the document next to the image.
              event.preventDefault();
              // All placeholders are inserted synchronously: each insert
              // leaves a NodeSelection on its image, which is what stacks
              // multiple files in paste order. The caret then moves below
              // the last image onto a fresh bullet or line, so pasting
              // reads like typing: the image goes where you were, and
              // writing continues underneath it.
              const placed = files.map((file) => ({ file, token: placePlaceholder() }));
              caretBelow();
              for (const { file, token } of placed) void save(token, file);
              return true;
            }
          }
        })
      ];

      /** Insert the pending node where typed text would go. An empty
       * textblock is replaced outright: pasted onto an empty bullet, the
       * image becomes the bullet (the schema admits it: extensions.ts).
       * A non-empty one takes the image right after it: a mid-sentence
       * paste never splits its sentence, and inside a list item that
       * position is still inside the item, so the image stays nested
       * under its bullet. At depth 0 (a NodeSelection or gap cursor),
       * right after the selected node, which is what stacks a multi-file
       * paste in order. */
      function placePlaceholder(): string {
        const token = `pending-${nextToken++}`;
        const { $to } = editor.state.selection;
        const node = { type: 'image', attrs: { src: '', title: token } };
        // Whether the container would take an image in the textblock's
        // place. List items do (their schema admits an image-first child,
        // extensions.ts); task items refuse, because markdown cannot
        // write an image-only task, and fall back to nesting below.
        const replaceable =
          $to.parent.isTextblock &&
          $to.parent.content.size === 0 &&
          $to.node(-1).canReplaceWith($to.index(-1), $to.index(-1) + 1, editor.schema.nodes.image);
        const chain = editor.chain().focus();
        if (replaceable) {
          chain.insertContentAt({ from: $to.before(), to: $to.after() }, node);
        } else {
          chain.insertContentAt($to.parent.isTextblock ? $to.after() : $to.pos, node);
        }
        // A placeholder with no src renders as a broken image, which is
        // honest: something is arriving and it is not here yet. The
        // explicit scroll matters because the placeholder has no height
        // for the focus scroll to work with.
        chain.scrollIntoView().run();
        return token;
      }

      /** Put the caret on a fresh empty block below the last pasted
       * image (a new bullet when the image sits in a list or task item,
       * a new line otherwise) so writing continues under the screenshot
       * instead of stranded above it. Reads the NodeSelection the last
       * insert left behind. */
      function caretBelow() {
        const { $to } = editor.state.selection;
        let itemDepth = 0;
        for (let depth = $to.depth; depth > 0; depth--) {
          const name = $to.node(depth).type.name;
          if (name === 'listItem' || name === 'taskItem') {
            itemDepth = depth;
            break;
          }
        }
        if (itemDepth > 0) {
          const item = $to.node(itemDepth).type.name;
          const after = $to.after(itemDepth);
          editor
            .chain()
            .insertContentAt(after, {
              type: item,
              ...(item === 'taskItem' ? { attrs: { checked: false } } : {}),
              content: [{ type: 'paragraph' }]
            })
            // Past the item's opening and its paragraph's: the text spot.
            .setTextSelection(after + 2)
            .scrollIntoView()
            .run();
        } else {
          const after = $to.pos;
          editor
            .chain()
            .insertContentAt(after, { type: 'paragraph' })
            .setTextSelection(after + 1)
            .scrollIntoView()
            .run();
        }
      }

      async function save(token: string, file: File) {
        try {
          const link = await saveAsset(await file.arrayBuffer(), options.meetingId?.());
          replacePlaceholder(token, link);
        } catch (e) {
          replacePlaceholder(token, null);
          options.onError?.(e instanceof Error ? e.message : String(e));
        }
      }

      /** Swap the placeholder for the real link, or remove it on failure.
       * Finding it by token rather than by position is what makes an undo
       * mid-flight harmless: the node is not there and nothing happens. */
      function replacePlaceholder(token: string, link: string | null) {
        const { state } = editor.view;
        let found: number | null = null;
        state.doc.descendants((node, pos) => {
          if (found === null && node.type.name === 'image' && node.attrs.title === token) {
            found = pos;
          }
        });
        if (found === null) return;
        const tr = state.tr;
        if (link === null) {
          // The placeholder may be all its list item holds (it replaced
          // the empty paragraph the paste landed on), and deleting just
          // the node would leave a child-less item the schema refuses.
          // Take every ancestor it is the only child of along with it:
          // the item, and the list too when that item was its last.
          const $pos = state.doc.resolve(found);
          let from: number = found;
          let to: number = found + 1;
          for (let depth = $pos.depth; depth > 0; depth--) {
            if ($pos.node(depth).childCount > 1) break;
            from = $pos.before(depth);
            to = $pos.after(depth);
          }
          if (from === 0 && to === state.doc.content.size) {
            // Nothing else in the document: a doc must keep one block.
            tr.replaceWith(from, to, state.schema.nodes.paragraph.create());
          } else {
            tr.delete(from, to);
          }
        } else {
          tr.setNodeMarkup(found, undefined, { src: link, title: null, alt: null });
        }
        editor.view.dispatch(tr);
        if (link === null) return;
        // Keep the arriving image in view: once when the real link is set,
        // and once more when the bytes render and the node takes its
        // height (nothing else listens for that growth). `nearest` so a
        // viewport already showing the image does not move.
        const dom = editor.view.nodeDOM(found);
        if (dom instanceof HTMLImageElement) {
          dom.scrollIntoView({ block: 'nearest' });
          dom.addEventListener(
            'load',
            () => dom.scrollIntoView({ block: 'nearest' }),
            { once: true }
          );
        }
      }
    }
  });

function imageFilesFrom(data: DataTransfer | null): File[] {
  if (!data) return [];
  return Array.from(data.files).filter((f) => f.type.startsWith('image/'));
}
