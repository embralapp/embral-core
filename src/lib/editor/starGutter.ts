import { Extension, type Editor } from '@tiptap/core';
import { Plugin } from '@tiptap/pm/state';
import type { Node as PmNode } from '@tiptap/pm/model';
import type { EditorView } from '@tiptap/pm/view';

/** Star anchors live as a `starId` attribute on textblocks (paragraphs and
 * headings, which includes each bullet's own paragraph), rendered as a
 * gutter ★ via CSS `::before`:
 *
 * - Undo-safe: the anchor is document state, so deleting a starred
 *   chunk and pressing Ctrl+Z brings the stars back with their lines.
 * - Per-line: every textblock (each bullet included) anchors its own
 *   star.
 * - Selection-proof: a pseudo-element is never part of the DOM
 *   selection, so Ctrl+A copies only the notes text.
 *
 * Starring/unstarring transactions carry `addToHistory: false` so the
 * star action itself is not an undo step (undo is for the text).
 */
export function starAnchors(onStarClick?: (id: number) => void) {
  return Extension.create({
    name: 'starAnchors',
    addGlobalAttributes() {
      return [
        {
          types: ['paragraph', 'heading'],
          attributes: {
            starId: {
              default: null,
              keepOnSplit: false,
              parseHTML: (el: HTMLElement) => {
                const raw = el.getAttribute('data-star-id');
                return raw === null ? null : Number(raw);
              },
              renderHTML: (attrs: Record<string, unknown>) =>
                attrs.starId === null || attrs.starId === undefined
                  ? {}
                  : { 'data-star-id': String(attrs.starId) }
            }
          }
        }
      ];
    },
    addProseMirrorPlugins() {
      if (!onStarClick) return [];
      return [
        new Plugin({
          props: {
            handleDOMEvents: {
              // Clicks in the left padding (the gutter) on a starred
              // line's vertical range hit the star.
              mousedown: (view: EditorView, event: MouseEvent) => {
                const prose = view.dom as HTMLElement;
                const rect = prose.getBoundingClientRect();
                const padLeft = parseFloat(getComputedStyle(prose).paddingLeft) || 0;
                if (event.clientX > rect.left + padLeft) return false;
                for (const el of prose.querySelectorAll('[data-star-id]')) {
                  const r = (el as HTMLElement).getBoundingClientRect();
                  if (event.clientY >= r.top && event.clientY <= r.bottom) {
                    event.preventDefault();
                    onStarClick(Number((el as HTMLElement).dataset.starId));
                    return true;
                  }
                }
                return false;
              }
            }
          }
        })
      ];
    }
  });
}

/** Every textblock in document order, with the position of the node
 * itself (usable with `setNodeMarkup`). */
function textblocks(doc: PmNode): { pos: number; node: PmNode }[] {
  const out: { pos: number; node: PmNode }[] = [];
  doc.descendants((node, pos) => {
    if (node.isTextblock) {
      out.push({ pos, node });
      return false;
    }
    return true;
  });
  return out;
}

function setStarAttr(editor: Editor, pos: number, node: PmNode, starId: number | null) {
  const tr = editor.state.tr
    .setNodeMarkup(pos, undefined, { ...node.attrs, starId })
    .setMeta('addToHistory', false);
  editor.view.dispatch(tr);
}

/** The caret's textblock (the last one when the editor isn't focused). */
function cursorTextblock(editor: Editor): { pos: number; node: PmNode } | null {
  if (editor.isFocused) {
    const $from = editor.state.selection.$from;
    for (let d = $from.depth; d >= 1; d--) {
      if ($from.node(d).isTextblock) {
        return { pos: $from.before(d), node: $from.node(d) };
      }
    }
    return null;
  }
  const blocks = textblocks(editor.state.doc);
  return blocks[blocks.length - 1] ?? null;
}

/** The star already on the caret's line, if any (Ctrl+S toggles it). */
export function starAtCursor(editor: Editor): number | null {
  const block = cursorTextblock(editor);
  return (block?.node.attrs.starId as number | null) ?? null;
}

/** Anchor a star on the caret's line (replacing any star already there;
 * one star per line). */
export function anchorStarAtCursor(editor: Editor, id: number) {
  const block = cursorTextblock(editor);
  if (block) setStarAttr(editor, block.pos, block.node, id);
}

/** Drop a star's anchor wherever it sits. */
export function clearStarAnchor(editor: Editor, id: number) {
  for (const { pos, node } of textblocks(editor.state.doc)) {
    if (node.attrs.starId === id) {
      setStarAttr(editor, pos, node, null);
      return;
    }
  }
}

/** star id → textblock index, for persisting anchors at stop. */
export function starBlockIndexes(editor: Editor): Map<number, number> {
  const out = new Map<number, number>();
  textblocks(editor.state.doc).forEach(({ node }, index) => {
    if (node.attrs.starId !== null) {
      out.set(node.attrs.starId as number, index);
    }
  });
  return out;
}

/** Re-anchor a star at a saved textblock index (the saved-notes view). */
export function anchorStarAtBlock(editor: Editor, id: number, blockIndex: number) {
  const blocks = textblocks(editor.state.doc);
  const block = blocks[Math.min(blockIndex, blocks.length - 1)];
  if (block) setStarAttr(editor, block.pos, block.node, id);
}
