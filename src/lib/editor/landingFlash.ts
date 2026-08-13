import { Extension } from '@tiptap/core';
import { Plugin, PluginKey } from '@tiptap/pm/state';
import { Decoration, DecorationSet, type EditorView } from '@tiptap/pm/view';
import { FLASH_CLASS, FLASH_MS } from '$lib/utils/flash';

/**
 * Marking the passage a search result landed on, inside the editor.
 *
 * It has to be a decoration, not a class on the node's element:
 * ProseMirror owns that DOM and rebuilds it from document state, so a
 * `classList.add` survives only until the next redraw, which for a
 * document the user is about to type in is immediate. A decoration is
 * part of the state ProseMirror redraws from, so it stays put, and it
 * leaves the document itself untouched (unlike a star, which is a node
 * attribute, because a star is content and this is not).
 */
export const landingFlashKey = new PluginKey<DecorationSet>('landingFlash');

/** Set the flash to a node range, or clear it with `null`. */
type FlashAction = { from: number; to: number } | null;

export function landingFlash() {
  return Extension.create({
    name: 'landingFlash',
    addProseMirrorPlugins() {
      return [
        new Plugin<DecorationSet>({
          key: landingFlashKey,
          state: {
            init: () => DecorationSet.empty,
            apply(tr, current) {
              const action = tr.getMeta(landingFlashKey) as FlashAction | undefined;
              if (action === null) return DecorationSet.empty;
              if (action) {
                return DecorationSet.create(tr.doc, [
                  Decoration.node(action.from, action.to, { class: FLASH_CLASS })
                ]);
              }
              // No action: carry the mark through edits elsewhere in the
              // document rather than dropping it on the first keystroke.
              return current.map(tr.mapping, tr.doc);
            }
          },
          props: {
            decorations(state) {
              return landingFlashKey.getState(state);
            }
          }
        })
      ];
    }
  });
}

/** Mark the node at `pos` and clear the mark on its own, so nothing is left
 * for the user to dismiss. */
export function flashNodeAt(view: EditorView, pos: number, durationMs = FLASH_MS): boolean {
  const node = view.state.doc.nodeAt(pos);
  if (!node) return false;
  // `addToHistory: false` keeps the mark out of the undo stack: undo is
  // for the user's text, not for where a search sent them. The transaction
  // changes no content, so the editor's save guard ignores it either way.
  const set = (action: FlashAction) =>
    view.dispatch(view.state.tr.setMeta(landingFlashKey, action).setMeta('addToHistory', false));
  set({ from: pos, to: pos + node.nodeSize });
  setTimeout(() => set(null), durationMs);
  return true;
}
