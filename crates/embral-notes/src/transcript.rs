//! Transcript rendering and segment-level editing.
//!
//! `format_transcript` was previously inlined in `src-tauri/src/commands.rs`
//! (untested, and mirrored by hand in `LiveTranscript.svelte`). It now lives
//! here so the paragraph-grouping rules have one tested definition, and so
//! segment-editing operations (split / delete / reassign speaker) can be
//! built on the same segment model.

use embral_types::TranscriptionSegment;

// Paragraph segmentation thresholds. Keep in sync with the TypeScript
// counterpart in `src/lib/components/LiveTranscript.svelte`.
pub const STRONG_GAP: f64 = 4.0;
pub const SOFT_GAP: f64 = 2.0;
pub const MAX_PARAGRAPH_CHARS: usize = 800;

fn ends_sentence(text: &str) -> bool {
    text.trim_end()
        .ends_with(|c: char| matches!(c, '.' | '!' | '?'))
}

/// Whether `curr` should start a new paragraph after `prev`.
pub fn starts_new_paragraph(
    prev: &TranscriptionSegment,
    curr: &TranscriptionSegment,
    running_len: usize,
) -> bool {
    if prev.speaker != curr.speaker {
        return true;
    }
    let gap = curr.start - prev.end;
    if gap >= STRONG_GAP {
        return true;
    }
    if gap >= SOFT_GAP && ends_sentence(&prev.text) {
        return true;
    }
    running_len + curr.text.len() + 1 > MAX_PARAGRAPH_CHARS
}

/// One paragraph's worth of consecutive segments: the same grouping
/// `format_transcript` renders, kept structured (speaker, timing, joined
/// text) so consumers like the search chunker can build passages without
/// re-deriving the break rules.
#[derive(Debug, Clone, PartialEq)]
pub struct Paragraph {
    pub speaker: Option<String>,
    /// Registry link of the first segment in the paragraph, when known.
    pub speaker_id: Option<String>,
    pub start: f64,
    pub end: f64,
    /// Segment texts joined with single spaces.
    pub text: String,
}

/// Group segments into paragraphs by the break rules above. The one
/// definition — `format_transcript` renders from this.
pub fn paragraphs(segments: &[TranscriptionSegment]) -> Vec<Paragraph> {
    let mut out: Vec<Paragraph> = Vec::new();
    let mut current: Option<Paragraph> = None;
    let mut running_len: usize = 0;

    for (i, seg) in segments.iter().enumerate() {
        let breaks = match current.as_ref() {
            None => true,
            Some(_) => starts_new_paragraph(&segments[i - 1], seg, running_len),
        };
        if breaks {
            if let Some(p) = current.take() {
                if !p.text.trim().is_empty() {
                    out.push(p);
                }
            }
            current = Some(Paragraph {
                speaker: seg.speaker.clone(),
                speaker_id: seg.speaker_id.clone(),
                start: seg.start,
                end: seg.end,
                text: seg.text.clone(),
            });
            running_len = seg.text.len();
        } else if let Some(p) = current.as_mut() {
            p.text.push(' ');
            p.text.push_str(&seg.text);
            p.end = seg.end;
            running_len += seg.text.len() + 1;
        }
    }
    if let Some(p) = current {
        if !p.text.trim().is_empty() {
            out.push(p);
        }
    }
    out
}

fn format_paragraphs(segments: &[TranscriptionSegment]) -> String {
    paragraphs(segments)
        .into_iter()
        .map(|p| match p.speaker {
            Some(spk) => format!("{}: {}", spk, p.text),
            None => p.text,
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Group finalized segments into Markdown paragraphs (blank-line separated).
///
/// Segments are assumed to arrive in monotonic finalization order and are **not**
/// re-sorted (a non-load-bearing sort would mask ordering bugs). Consecutive
/// same-speaker segments join with a single space until a paragraph break fires
/// (speaker change, long pause, sentence-end + moderate pause, runaway length).
pub fn format_transcript(segments: &[TranscriptionSegment]) -> String {
    format_paragraphs(segments)
}

/// Deduplicated speaker labels in first-seen order — the attendee seed.
pub fn speakers(segments: &[TranscriptionSegment]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in segments {
        if let Some(spk) = s.speaker.as_deref() {
            let spk = spk.trim();
            if !spk.is_empty() && !out.iter().any(|e| e == spk) {
                out.push(spk.to_string());
            }
        }
    }
    out
}

// --- Segment-level editing (edit / delete / split segments) ---

/// Remove the segment at `index`. Out-of-range indexes are ignored.
pub fn delete_segment(segments: &mut Vec<TranscriptionSegment>, index: usize) {
    if index < segments.len() {
        segments.remove(index);
    }
}

/// Set the speaker label of the segment at `index` (e.g. "Speaker 2" → "Dana").
/// An empty/whitespace name clears the label (`None`). Any registry link is
/// dropped — the caller re-links when the new label is a known person.
pub fn reassign_speaker(segments: &mut [TranscriptionSegment], index: usize, speaker: &str) {
    if let Some(seg) = segments.get_mut(index) {
        let s = speaker.trim();
        seg.speaker = if s.is_empty() {
            None
        } else {
            Some(s.to_string())
        };
        seg.speaker_id = None;
    }
}

/// Reassign a whole inclusive range — a turn renamed in one edit, however
/// long. Bounds are clamped; an inverted range does nothing.
pub fn reassign_speaker_range(
    segments: &mut [TranscriptionSegment],
    from: usize,
    to: usize,
    speaker: &str,
) {
    if from > to {
        return;
    }
    let end = to.min(segments.len().saturating_sub(1));
    for index in from..=end {
        reassign_speaker(segments, index, speaker);
    }
}

/// Split the segment at `index` into two at a UTF-8 character boundary
/// `char_offset`. Both halves keep the original speaker; timing is interpolated
/// by character ratio so ordering stays monotonic. No-op if the split point is
/// at either end (nothing to split) or the index is out of range.
pub fn split_segment(segments: &mut Vec<TranscriptionSegment>, index: usize, char_offset: usize) {
    let Some(seg) = segments.get(index).cloned() else {
        return;
    };
    let chars: Vec<char> = seg.text.chars().collect();
    if char_offset == 0 || char_offset >= chars.len() {
        return;
    }
    let left: String = chars[..char_offset].iter().collect();
    let right: String = chars[char_offset..].iter().collect();
    let (left, right) = (left.trim().to_string(), right.trim().to_string());
    if left.is_empty() || right.is_empty() {
        return;
    }

    let ratio = char_offset as f64 / chars.len() as f64;
    let mid = seg.start + (seg.end - seg.start) * ratio;

    let first = TranscriptionSegment {
        speaker: seg.speaker.clone(),
        speaker_id: seg.speaker_id.clone(),
        text: left,
        start: seg.start,
        end: mid,
    };
    let second = TranscriptionSegment {
        speaker: seg.speaker,
        speaker_id: seg.speaker_id,
        text: right,
        start: mid,
        end: seg.end,
    };
    segments.splice(index..=index, [first, second]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(speaker: Option<&str>, text: &str, start: f64, end: f64) -> TranscriptionSegment {
        TranscriptionSegment {
            speaker: speaker.map(str::to_string),
            speaker_id: None,
            text: text.to_string(),
            start,
            end,
        }
    }

    #[test]
    fn empty_is_empty() {
        assert_eq!(format_transcript(&[]), "");
        assert!(paragraphs(&[]).is_empty());
    }

    #[test]
    fn paragraphs_carry_timing_and_speaker_id() {
        let mut a = seg(Some("A"), "Hello there.", 0.0, 1.0);
        a.speaker_id = Some("sp-a".into());
        let mut b = seg(Some("A"), "How are you?", 1.1, 2.0);
        b.speaker_id = Some("sp-a".into());
        let c = seg(Some("B"), "Fine.", 2.2, 3.0);

        let ps = paragraphs(&[a, b, c]);
        assert_eq!(ps.len(), 2);
        assert_eq!(ps[0].speaker.as_deref(), Some("A"));
        assert_eq!(ps[0].speaker_id.as_deref(), Some("sp-a"));
        assert_eq!(ps[0].start, 0.0);
        assert_eq!(ps[0].end, 2.0); // spans first→last segment of the group
        assert_eq!(ps[0].text, "Hello there. How are you?");
        assert_eq!(ps[1].speaker.as_deref(), Some("B"));
        assert!(ps[1].speaker_id.is_none());
    }

    #[test]
    fn paragraphs_and_format_transcript_agree_on_breaks() {
        let s = vec![
            seg(Some("A"), "One.", 0.0, 1.0),
            seg(Some("A"), "Two.", 6.0, 7.0), // strong gap
            seg(Some("B"), "Three.", 7.1, 8.0),
            seg(None, "Four.", 8.1, 9.0),
        ];
        let rendered = format_transcript(&s);
        let structured = paragraphs(&s);
        assert_eq!(rendered.split("\n\n").count(), structured.len());
    }

    #[test]
    fn same_speaker_joins_with_space() {
        let s = vec![
            seg(Some("A"), "Hello there.", 0.0, 1.0),
            seg(Some("A"), "How are you?", 1.1, 2.0),
        ];
        assert_eq!(format_transcript(&s), "A: Hello there. How are you?");
    }

    #[test]
    fn speaker_change_breaks_paragraph() {
        let s = vec![
            seg(Some("A"), "Hi.", 0.0, 1.0),
            seg(Some("B"), "Hey.", 1.1, 2.0),
        ];
        assert_eq!(format_transcript(&s), "A: Hi.\n\nB: Hey.");
    }

    #[test]
    fn strong_gap_breaks_even_same_speaker() {
        let s = vec![
            seg(Some("A"), "First", 0.0, 1.0),
            seg(Some("A"), "Second", 6.0, 7.0), // 5s gap ≥ STRONG_GAP
        ];
        assert_eq!(format_transcript(&s), "A: First\n\nA: Second");
    }

    #[test]
    fn no_speaker_renders_plain() {
        let s = vec![seg(None, "just text", 0.0, 1.0)];
        assert_eq!(format_transcript(&s), "just text");
    }

    #[test]
    fn speakers_dedup_in_order() {
        let s = vec![
            seg(Some("A"), "x", 0.0, 1.0),
            seg(Some("B"), "y", 1.0, 2.0),
            seg(Some("A"), "z", 2.0, 3.0),
        ];
        assert_eq!(speakers(&s), vec!["A", "B"]);
    }

    #[test]
    fn delete_removes_indexed_segment() {
        let mut s = vec![seg(None, "a", 0.0, 1.0), seg(None, "b", 1.0, 2.0)];
        delete_segment(&mut s, 0);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].text, "b");
        delete_segment(&mut s, 99); // out of range, no panic
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn reassign_sets_and_clears_speaker() {
        let mut s = vec![seg(Some("Speaker 2"), "hi", 0.0, 1.0)];
        reassign_speaker(&mut s, 0, "Dana");
        assert_eq!(s[0].speaker.as_deref(), Some("Dana"));
        reassign_speaker(&mut s, 0, "  ");
        assert_eq!(s[0].speaker, None);
    }

    #[test]
    fn a_range_reassigns_only_its_rows() {
        let mut s = vec![
            seg(None, "a", 0.0, 1.0),
            seg(None, "b", 1.0, 2.0),
            seg(None, "c", 2.0, 3.0),
            seg(None, "d", 3.0, 4.0),
        ];
        reassign_speaker_range(&mut s, 1, 2, "Dana");
        assert_eq!(s[0].speaker, None);
        assert_eq!(s[1].speaker.as_deref(), Some("Dana"));
        assert_eq!(s[2].speaker.as_deref(), Some("Dana"));
        assert_eq!(s[3].speaker, None);
    }

    #[test]
    fn a_range_clamps_and_an_inverted_one_does_nothing() {
        let mut s = vec![seg(None, "a", 0.0, 1.0), seg(None, "b", 1.0, 2.0)];
        reassign_speaker_range(&mut s, 1, 99, "Dana");
        assert_eq!(s[1].speaker.as_deref(), Some("Dana"));
        reassign_speaker_range(&mut s, 1, 0, "Nobody");
        assert_eq!(s[0].speaker, None);
        assert_eq!(s[1].speaker.as_deref(), Some("Dana"));
        // An empty slice is a no-op, not a panic.
        reassign_speaker_range(&mut [], 0, 0, "Dana");
    }

    #[test]
    fn split_divides_text_and_timing() {
        let mut s = vec![seg(Some("A"), "hello world", 0.0, 10.0)];
        split_segment(&mut s, 0, 5); // "hello" | " world"
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].text, "hello");
        assert_eq!(s[1].text, "world");
        assert_eq!(s[0].start, 0.0);
        assert_eq!(s[1].end, 10.0);
        assert!(s[0].end > 0.0 && s[0].end < 10.0);
        assert_eq!(s[0].end, s[1].start);
        assert_eq!(s[0].speaker.as_deref(), Some("A"));
    }

    #[test]
    fn split_noop_at_boundaries() {
        let mut s = vec![seg(None, "abc", 0.0, 1.0)];
        split_segment(&mut s, 0, 0);
        split_segment(&mut s, 0, 3);
        assert_eq!(s.len(), 1);
    }
}
