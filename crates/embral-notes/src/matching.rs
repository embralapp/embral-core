//! Naming diarized speakers from the user's typed notes: the pure half.
//!
//! The user often writes lines like "John: we need a new plan" while the
//! transcript reads "Speaker 1: We need to come up with a new plan for
//! XYZ". This module turns that overlap into evidence: split the notes
//! into lines, score each line against the transcript's paragraphs
//! (keyword overlap, plus embedding cosine when vectors are supplied),
//! and hand the best pairs to one LLM call whose prompt is built and
//! parsed here. Everything is deterministic and unit-tested; the async
//! embed/LLM orchestration lives in the app crate.

use crate::transcript::Paragraph;

/// Evidence pairs kept per note line.
const EVIDENCE_PER_LINE: usize = 3;
/// Combined-score floor below which a pair is noise, not evidence. The
/// keyword leg is an overlap coefficient in [0,1]; the semantic leg is a
/// cosine of ~0.7-0.9 for related e5 sentence pairs, so 0.3 keeps
/// paraphrases while dropping lines that merely share a stopword-ish term.
const EVIDENCE_FLOOR: f32 = 0.3;
/// Total evidence pairs across the whole meeting (prompt-size bound).
const EVIDENCE_CAP: usize = 20;
/// Transcript excerpts are clipped to this many characters in the prompt.
const QUOTE_CHARS: usize = 280;
/// Sanity bound on an assigned name.
const MAX_NAME_CHARS: usize = 60;

/// One transcript excerpt a note line resembles.
#[derive(Debug, Clone, PartialEq)]
pub struct EvidencePair {
    pub note_line: String,
    /// The generic label ("Speaker N") of the excerpt's paragraph.
    pub label: String,
    pub quote: String,
    pub score: f32,
}

/// One candidate transcript excerpt: a generically-labeled paragraph.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub label: String,
    pub text: String,
}

/// A session-generated numbered label ("Speaker 3"): the only labels the
/// naming pass may rename. Lives in embral-types so the registry layer can
/// apply the same rule.
pub fn is_generic_label(label: &str) -> bool {
    embral_types::is_generic_speaker_label(label)
}

/// The matchable side of the transcript: paragraphs still carrying a
/// generic label, capped (prompt/embedding bound) in transcript order.
pub fn candidates(paragraphs: &[Paragraph], cap: usize) -> Vec<Candidate> {
    paragraphs
        .iter()
        .filter_map(|p| {
            let label = p.speaker.as_deref()?;
            if !is_generic_label(label) || p.text.trim().is_empty() {
                return None;
            }
            Some(Candidate {
                label: label.to_string(),
                text: p.text.clone(),
            })
        })
        .take(cap)
        .collect()
}

/// The notes as matchable lines: markdown syntax stripped, blanks and
/// one-word fragments dropped.
pub fn note_lines(summary: &str) -> Vec<String> {
    summary
        .lines()
        .map(strip_markdown_line)
        .filter(|l| tokens(l).len() >= 2)
        .collect()
}

fn strip_markdown_line(line: &str) -> String {
    let mut s = line.trim();
    // Leading block syntax: headers, quotes, bullets, numbered lists.
    s = s.trim_start_matches(['#', '>', ' ']);
    s = s.trim_start_matches(['-', '*', '+']).trim_start();
    if let Some(rest) = s.split_once('.').and_then(|(n, rest)| {
        (!n.is_empty() && n.chars().all(|c| c.is_ascii_digit())).then_some(rest)
    }) {
        s = rest.trim_start();
    }
    // Inline emphasis markers add nothing to matching.
    s.replace(['*', '_', '`'], "").trim().to_string()
}

/// Lowercase alphanumeric tokens.
fn tokens(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

/// Overlap coefficient over token sets: |A∩B| / min(|A|,|B|), 0 when
/// either side is empty.
fn overlap(a: &[String], b: &[String]) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let set: std::collections::HashSet<&str> = a.iter().map(String::as_str).collect();
    let b_set: std::collections::HashSet<&str> = b.iter().map(String::as_str).collect();
    let common = b_set.iter().filter(|t| set.contains(**t)).count();
    common as f32 / set.len().min(b_set.len()) as f32
}

/// Cosine similarity (inlined to keep this crate dependency-light).
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na * nb)
}

/// Score every note line against every candidate and keep the best pairs.
/// `note_vecs`/`candidate_vecs`, when given, must align 1:1 with the input
/// slices; without them (embedder unavailable) scoring is keyword-only.
pub fn evidence(
    lines: &[String],
    candidates: &[Candidate],
    note_vecs: Option<&[Vec<f32>]>,
    candidate_vecs: Option<&[Vec<f32>]>,
) -> Vec<EvidencePair> {
    let line_tokens: Vec<Vec<String>> = lines.iter().map(|l| tokens(l)).collect();
    let cand_tokens: Vec<Vec<String>> = candidates.iter().map(|c| tokens(&c.text)).collect();

    let mut pairs: Vec<EvidencePair> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let mut scored: Vec<(usize, f32)> = candidates
            .iter()
            .enumerate()
            .map(|(j, _)| {
                let kw = overlap(&line_tokens[i], &cand_tokens[j]);
                let score = match (
                    note_vecs.and_then(|v| v.get(i)),
                    candidate_vecs.and_then(|v| v.get(j)),
                ) {
                    (Some(nv), Some(cv)) => (kw + cosine(nv, cv)) / 2.0,
                    _ => kw,
                };
                (j, score)
            })
            .filter(|(_, s)| *s >= EVIDENCE_FLOOR)
            .collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        for (j, score) in scored.into_iter().take(EVIDENCE_PER_LINE) {
            let mut quote = candidates[j].text.clone();
            if quote.len() > QUOTE_CHARS {
                let cut = (0..=QUOTE_CHARS).rev().find(|&i| quote.is_char_boundary(i));
                quote.truncate(cut.unwrap_or(0));
                quote.push('…');
            }
            pairs.push(EvidencePair {
                note_line: line.clone(),
                label: candidates[j].label.clone(),
                quote,
                score,
            });
        }
    }
    pairs.sort_by(|a, b| b.score.total_cmp(&a.score));
    pairs.truncate(EVIDENCE_CAP);
    pairs
}

/// System prompt for the naming call. The whole job is restraint: the
/// notes usually describe content, not speakers, and an empty mapping is
/// the expected answer.
pub const NAMING_SYSTEM_PROMPT: &str = r#"You match speakers in a meeting transcript to real names using the notes the user typed during the meeting. The transcript labels speakers generically ("Speaker 1", "Speaker 2"). The user's notes sometimes reveal who a speaker is — most often a line like "John: we need a new plan" paraphrasing something that speaker said.

You are given the user's notes, the generic labels in play, and evidence pairs: a note line beside a transcript excerpt that resembles it.

Rules:
- Only name a speaker when the notes clearly identify them as the one speaking. A note that merely mentions a person ("ask John about the budget") does NOT make that person the speaker of a similar excerpt.
- Notes are usually about content, not speakers. An empty mapping is a common and correct answer. Never guess.
- Map only the generic labels listed. Never invent a label, and never use a generic label as a name.
- One name per label, and one label per name.

Reply with ONLY this JSON — no prose, no code fences:
{"assignments":[{"label":"Speaker 1","name":"John"}]}
Use {"assignments":[]} when the notes don't identify anyone."#;

/// Build the naming call's user message: the labels, the full notes for
/// context, and the evidence pairs.
pub fn build_naming_message(
    summary: &str,
    labels: &[String],
    evidence: &[EvidencePair],
) -> String {
    let mut out = format!("GENERIC SPEAKER LABELS: {}\n\n", labels.join(", "));
    out.push_str("USER NOTES (typed live during the meeting):\n");
    out.push_str(summary.trim());
    out.push_str("\n\nEVIDENCE (note line beside a transcript excerpt that resembles it):\n");
    if evidence.is_empty() {
        out.push_str("(none — no note line resembles any excerpt)\n");
    }
    for (i, pair) in evidence.iter().enumerate() {
        out.push_str(&format!(
            "{}. NOTE: \"{}\"\n   {} SAID: \"{}\"\n",
            i + 1,
            pair.note_line,
            pair.label,
            pair.quote
        ));
    }
    out
}

/// Parse the model's reply into validated `(label, name)` assignments.
/// Lenient about wrapping (reasoning blocks, prose, code fences) and
/// strict about content: unknown labels, generic or oversized names, and
/// duplicates are dropped. Anything unparseable is an empty list: a
/// confused model must never rename a transcript.
pub fn parse_assignments(reply: &str, valid_labels: &[String]) -> Vec<(String, String)> {
    let cleaned = crate::providers::strip_reasoning(reply);
    let Some(start) = cleaned.find('{') else {
        return Vec::new();
    };
    let Some(end) = cleaned.rfind('}') else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&cleaned[start..=end]) else {
        return Vec::new();
    };
    let Some(items) = value.get("assignments").and_then(|a| a.as_array()) else {
        return Vec::new();
    };

    let mut out: Vec<(String, String)> = Vec::new();
    for item in items {
        let (Some(label), Some(name)) = (
            item.get("label").and_then(|v| v.as_str()),
            item.get("name").and_then(|v| v.as_str()),
        ) else {
            continue;
        };
        let name = name.trim();
        if !valid_labels.iter().any(|l| l == label)
            || name.is_empty()
            || name.chars().count() > MAX_NAME_CHARS
            || is_generic_label(name)
            || name.eq_ignore_ascii_case(label)
        {
            continue;
        }
        // One name per label, one label per name; first wins.
        if out.iter().any(|(l, n)| l == label || n.eq_ignore_ascii_case(name)) {
            continue;
        }
        out.push((label.to_string(), name.to_string()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(label: &str, text: &str) -> Candidate {
        Candidate {
            label: label.to_string(),
            text: text.to_string(),
        }
    }

    #[test]
    fn note_lines_strip_markdown_and_drop_fragments() {
        let notes = "# Standup\n\n- **John**: we need a new plan\n> quoted thought here\n2. second item text\nok\n\n";
        let lines = note_lines(notes);
        assert_eq!(
            lines,
            vec![
                "John: we need a new plan",
                "quoted thought here",
                "second item text",
            ]
        );
        // "# Standup" survives stripping but "Standup" alone is one token →
        // dropped; "ok" likewise.
        assert!(!lines.iter().any(|l| l == "Standup" || l == "ok"));
    }

    #[test]
    fn overlap_is_symmetric_and_bounded() {
        let a = tokens("we need a new plan");
        let b = tokens("We need to come up with a new plan for XYZ");
        let o = overlap(&a, &b);
        assert!(o > 0.5, "paraphrase overlap got {o}");
        assert_eq!(overlap(&a, &tokens("")), 0.0);
        assert_eq!(overlap(&tokens("alpha beta"), &tokens("alpha beta")), 1.0);
    }

    #[test]
    fn candidates_keep_only_generic_labels() {
        let paras = vec![
            crate::transcript::Paragraph {
                speaker: Some("Speaker 1".into()),
                speaker_id: None,
                start: 0.0,
                end: 5.0,
                text: "We need a new plan".into(),
            },
            crate::transcript::Paragraph {
                speaker: Some("Alice".into()),
                speaker_id: None,
                start: 5.0,
                end: 8.0,
                text: "Agreed".into(),
            },
            crate::transcript::Paragraph {
                speaker: None,
                speaker_id: None,
                start: 8.0,
                end: 9.0,
                text: "Unattributed".into(),
            },
        ];
        let c = candidates(&paras, 10);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].label, "Speaker 1");
        assert!(candidates(&paras, 0).is_empty(), "cap respected");
    }

    #[test]
    fn evidence_pairs_note_lines_with_similar_excerpts() {
        let lines = vec![
            "John: we need a new plan".to_string(),
            "unrelated grocery reminder".to_string(),
        ];
        let cands = vec![
            cand("Speaker 1", "We need to come up with a new plan for XYZ"),
            cand("Speaker 2", "The weather has been quite something lately"),
        ];
        let pairs = evidence(&lines, &cands, None, None);
        assert_eq!(pairs.len(), 1, "only the paraphrase clears the floor");
        assert_eq!(pairs[0].label, "Speaker 1");
        assert_eq!(pairs[0].note_line, "John: we need a new plan");
    }

    #[test]
    fn evidence_uses_vectors_when_given() {
        let lines = vec!["budget follow up".to_string()];
        let cands = vec![
            cand("Speaker 1", "totally different words entirely"),
            cand("Speaker 2", "also nothing shared here"),
        ];
        // No keyword overlap at all: keyword-only finds nothing…
        assert!(evidence(&lines, &cands, None, None).is_empty());
        // …but near-identical embeddings on candidate 2 clear the floor
        // ((0 + ~1.0) / 2 ≥ 0.3).
        let note_vecs = vec![vec![1.0, 0.0]];
        let cand_vecs = vec![vec![0.0, 1.0], vec![1.0, 0.05]];
        let pairs = evidence(&lines, &cands, Some(&note_vecs), Some(&cand_vecs));
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].label, "Speaker 2");
    }

    #[test]
    fn evidence_caps_and_truncates_quotes() {
        let lines: Vec<String> = (0..30).map(|i| format!("shared phrase {i}")).collect();
        let long_text = format!("shared phrase {}", "x".repeat(500));
        let cands = vec![cand("Speaker 1", &long_text)];
        let pairs = evidence(&lines, &cands, None, None);
        assert!(pairs.len() <= EVIDENCE_CAP);
        assert!(pairs[0].quote.chars().count() <= QUOTE_CHARS + 1);
        assert!(pairs[0].quote.ends_with('…'));
    }

    #[test]
    fn naming_message_carries_labels_notes_and_evidence() {
        let labels = vec!["Speaker 1".to_string(), "Speaker 2".to_string()];
        let evidence = vec![EvidencePair {
            note_line: "John: new plan".into(),
            label: "Speaker 1".into(),
            quote: "We need a new plan".into(),
            score: 0.8,
        }];
        let msg = build_naming_message("John: new plan", &labels, &evidence);
        assert!(msg.contains("Speaker 1, Speaker 2"));
        assert!(msg.contains("USER NOTES"));
        assert!(msg.contains("Speaker 1 SAID: \"We need a new plan\""));
    }

    #[test]
    fn parse_accepts_clean_and_wrapped_json() {
        let labels = vec!["Speaker 1".to_string(), "Speaker 2".to_string()];
        let clean = r#"{"assignments":[{"label":"Speaker 1","name":"John"}]}"#;
        assert_eq!(
            parse_assignments(clean, &labels),
            vec![("Speaker 1".to_string(), "John".to_string())]
        );
        // Reasoning block + prose + code fences around the same JSON.
        let wrapped = format!(
            "<think>hmm who is who</think>\nHere you go:\n```json\n{clean}\n```"
        );
        assert_eq!(
            parse_assignments(&wrapped, &labels),
            vec![("Speaker 1".to_string(), "John".to_string())]
        );
        // The empty mapping is a valid answer.
        assert!(parse_assignments(r#"{"assignments":[]}"#, &labels).is_empty());
    }

    #[test]
    fn parse_rejects_invalid_and_duplicate_assignments() {
        let labels = vec!["Speaker 1".to_string(), "Speaker 2".to_string()];
        let reply = r#"{"assignments":[
            {"label":"Speaker 9","name":"Ghost"},
            {"label":"Speaker 1","name":"Speaker 3"},
            {"label":"Speaker 1","name":"  "},
            {"label":"Speaker 1","name":"John"},
            {"label":"Speaker 1","name":"Paul"},
            {"label":"Speaker 2","name":"john"}
        ]}"#;
        // Unknown label, generic name, blank name, second name for the same
        // label, and a second label for the same name all drop.
        assert_eq!(
            parse_assignments(reply, &labels),
            vec![("Speaker 1".to_string(), "John".to_string())]
        );
    }

    #[test]
    fn parse_survives_garbage() {
        let labels = vec!["Speaker 1".to_string()];
        assert!(parse_assignments("", &labels).is_empty());
        assert!(parse_assignments("no json here", &labels).is_empty());
        assert!(parse_assignments("{broken", &labels).is_empty());
        assert!(parse_assignments(r#"{"other":"shape"}"#, &labels).is_empty());
        let oversized = format!(
            r#"{{"assignments":[{{"label":"Speaker 1","name":"{}"}}]}}"#,
            "n".repeat(100)
        );
        assert!(parse_assignments(&oversized, &labels).is_empty());
    }

    #[test]
    fn generic_label_detection() {
        assert!(is_generic_label("Speaker 12"));
        assert!(!is_generic_label("Speaker"));
        assert!(!is_generic_label("Speaker Twelve"));
        assert!(!is_generic_label("Sam"));
    }
}
