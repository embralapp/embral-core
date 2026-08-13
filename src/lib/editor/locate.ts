/**
 * Finding the place in a document that a search result is pointing at.
 *
 * The locator arrives as text (the passage's opening line) rather than a
 * position, because positions do not survive an edit. Text does, mostly:
 * the index is rebuilt on every save, so the line the search matched is the
 * line that is there. "Mostly" is why the match is forgiving.
 */

import type { Node as PMNode } from '@tiptap/pm/model';

/**
 * Reduce a line to the words in it, so a passage of markdown source can be
 * matched against a document's rendered text.
 *
 * The lead arrives as source: `- ` bullet markers, `## ` hashes, `**bold**`
 * asterisks, `[text](url)` link syntax. The editor turns all of that into
 * structure and marks, so none of it appears in `textContent`: a summary,
 * which is almost entirely headings and bullets, would never match on a
 * literal comparison. Dropping everything that is not a letter or digit
 * leaves the one thing both sides agree on.
 */
function normalize(text: string): string {
  return text
    // A link's URL is an attribute, not text; keep only the label.
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/[^\p{L}\p{N}\s]+/gu, ' ')
    .replace(/\s+/g, ' ')
    .trim()
    .toLowerCase();
}

function firstWords(text: string, count: number): string {
  return text.split(' ').slice(0, count).join(' ');
}

/**
 * The needles to try, longest first.
 *
 * The chunk's text and the document's text are close but not identical: the
 * indexer strips image links down to their alt text, which the editor holds
 * as a node rather than as characters, so a paragraph with a picture in it
 * reads differently on each side. Rather than trying to reverse that, the
 * match falls back to shorter and shorter openings: six words is still a
 * distinctive phrase, three is enough to find the right paragraph, and
 * the alternative to a near-match is dumping the user at the top.
 */
function needles(text: string): string[] {
  const full = normalize(text);
  if (!full) return [];
  return [full, firstWords(full, 6), firstWords(full, 3)].filter(Boolean);
}

/**
 * The position of the first textblock at or after `from` carrying `text`,
 * or `null`.
 *
 * `from` is what sends a common word to the right place: searching for
 * "north" in a long transcript finds "North Carolina" minutes before the
 * "Northstar" passage the result was actually about. The passage's own
 * start bounds the search, so the match is the one the palette showed.
 */
export function findTextPos(doc: PMNode, text: string, from = 0): number | null {
  for (const needle of needles(text)) {
    let found: number | null = null;
    doc.descendants((node, pos) => {
      if (found !== null) return false;
      if (!node.isTextblock) return true;
      if (pos >= from && normalize(node.textContent).includes(needle)) {
        found = pos;
        return false;
      }
      return true;
    });
    if (found !== null) return found;
  }
  return null;
}

/**
 * The index of the first entry carrying `text`, or `null`: the same match
 * against a plain list, which is what a transcript is once it is segments
 * rather than a document.
 */
export function findMatchIndex(texts: string[], text: string): number | null {
  for (const needle of needles(text)) {
    const found = texts.findIndex((entry) => normalize(entry).includes(needle));
    if (found >= 0) return found;
  }
  return null;
}

/**
 * The position of the image node stored under `filename`, or `null` when the
 * user has since deleted it from this document; the file outlives the link
 * by design, so a hit can name an image the document no longer shows.
 */
export function findImagePos(doc: PMNode, filename: string): number | null {
  if (!filename) return null;
  let found: number | null = null;
  doc.descendants((node, pos) => {
    if (found !== null) return false;
    if (node.type.name !== 'image') return true;
    const src = String(node.attrs.src ?? '');
    if (src === filename || src.endsWith(`/${filename}`)) {
      found = pos;
      return false;
    }
    return true;
  });
  return found;
}
