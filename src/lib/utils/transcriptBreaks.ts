// Paragraph segmentation for transcript surfaces: one frontend
// definition, used by the live view and the post-meeting editor alike.
// The reference is the tested Rust `starts_new_paragraph`
// (crates/embral-notes/src/transcript.rs), which renders the stored
// markdown; keep the constants and rules in step with it by hand
// ([transcription.md] §Transcript formatting).

import type { TranscriptionSegment } from '$lib/types';

export const STRONG_GAP = 4.0;
export const SOFT_GAP = 2.0;
export const MAX_PARAGRAPH_CHARS = 800;

const SENTENCE_END = /[.!?]$/;

/** Paragraph length in characters, not UTF-16 code units. The Rust side
 * counts the same way, so the live view and the stored markdown break in
 * the same places; `.length` counts a CJK or emoji character as two and
 * broke a non-Latin transcript early. */
export function charLen(text: string): number {
  return [...text].length;
}

/** Whether `curr` should start a new paragraph after `prev`. */
export function startsNewParagraph(
  prev: TranscriptionSegment,
  curr: TranscriptionSegment,
  runningLen: number
): boolean {
  if ((prev.speaker ?? null) !== (curr.speaker ?? null)) return true;
  const gap = curr.start - prev.end;
  if (gap >= STRONG_GAP) return true;
  if (gap >= SOFT_GAP && SENTENCE_END.test(prev.text.trimEnd())) return true;
  return runningLen + charLen(curr.text) + 1 > MAX_PARAGRAPH_CHARS;
}
