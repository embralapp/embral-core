// The frontend mirror of the Rust `starts_new_paragraph` reference —
// these cases pin the same shapes its tests do, so a drifted mirror
// fails here rather than rendering differently from the stored document.

import { describe, expect, it } from 'vitest';
import { startsNewParagraph, MAX_PARAGRAPH_CHARS } from './transcriptBreaks';
import type { TranscriptionSegment } from '$lib/types';

function seg(
  text: string,
  start: number,
  end: number,
  speaker: string | null = null
): TranscriptionSegment {
  return { speaker, speaker_id: null, text, start, end };
}

describe('paragraph breaks', () => {
  it('a speaker change always breaks', () => {
    expect(startsNewParagraph(seg('hi', 0, 1, 'Alice'), seg('yo', 1, 2, 'Bob'), 2)).toBe(true);
  });

  it('null speakers on both sides do not break by themselves', () => {
    // The speakerless meeting (labeling off, the runaway guard): gaps and
    // length carry the structure instead of one endless turn.
    expect(startsNewParagraph(seg('one', 0, 1), seg('two', 1.2, 2), 3)).toBe(false);
  });

  it('a strong gap breaks regardless of punctuation', () => {
    expect(startsNewParagraph(seg('no punct', 0, 1), seg('next', 5.0, 6), 8)).toBe(true);
  });

  it('a soft gap breaks only after sentence-final punctuation', () => {
    expect(startsNewParagraph(seg('Done.', 0, 1), seg('next', 3.2, 4), 5)).toBe(true);
    expect(startsNewParagraph(seg('trailing comma,', 0, 1), seg('next', 3.2, 4), 15)).toBe(false);
  });

  it('running length past the cap breaks', () => {
    const prev = seg('x', 0, 1);
    const curr = seg('y'.repeat(10), 1.1, 2);
    expect(startsNewParagraph(prev, curr, MAX_PARAGRAPH_CHARS - 5)).toBe(true);
    expect(startsNewParagraph(prev, curr, 100)).toBe(false);
  });
});
