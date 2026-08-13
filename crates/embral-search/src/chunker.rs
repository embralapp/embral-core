//! Building passages ("chunks") out of a meeting's four documents and out
//! of dictations. The transcript chunker reuses the tested paragraph rules
//! in `embral-notes::transcript`: a passage is packed paragraphs, never a
//! new segmentation theory.

use chrono::{DateTime, Utc};
use embral_notes::transcript::paragraphs;
use embral_types::TranscriptionSegment;
use sha2::{Digest, Sha256};

/// Where a chunk's text came from. A user-written note is a stronger signal
/// than a generated summary, and both differ from verbatim speech; search
/// keeps them distinct rather than blending everything into one soup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Transcript,
    UserNotes,
    Summary,
    Dictation,
    /// Text an OCR engine read out of an image the user pasted. Nobody
    /// wrote it and nobody said it, which is why it is neither notes nor
    /// transcript: it is evidence the user chose to keep.
    ImageText,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Transcript => "transcript",
            Source::UserNotes => "user_notes",
            Source::Summary => "summary",
            Source::Dictation => "dictation",
            Source::ImageText => "image_text",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BuiltChunk {
    pub source: Source,
    pub chunk_index: u32,
    /// The verbatim passage: what results quote.
    pub text: String,
    /// Context header + text: what gets embedded and hashed.
    pub embedding_text: String,
    pub start_secs: Option<f64>,
    pub end_secs: Option<f64>,
    pub speakers: Vec<String>,
    pub speaker_ids: Vec<String>,
    pub content_hash: String,
    /// Which image this passage was read out of; `Some` only for
    /// [`Source::ImageText`]. A search hit has nothing in the document to
    /// scroll to for an image, so this is how it points at one.
    pub image_filename: Option<String>,
}

pub struct MeetingDocs<'a> {
    pub title: &'a str,
    pub started_at: DateTime<Utc>,
    pub segments: &'a [TranscriptionSegment],
    pub user_notes: &'a str,
    pub summary: &'a str,
    pub transcript: &'a str,
    /// What OCR read out of the images the documents above link to, as
    /// `(filename, text)` in paste order, already filtered by
    /// [`referenced_image_text`]. Not a document, so not a fifth string:
    /// each entry is one image, and the filename is what lets a hit point
    /// back at it.
    pub image_text: &'a [(String, String)],
}

/// Passage word budget: pack whole paragraphs up to the cap; a paragraph
/// that alone exceeds it stays one oversized chunk (paragraphs are already
/// length-capped for transcripts; prose blocks are rarely this long).
const MAX_WORDS: usize = 400;
/// Overlap: each chunk re-opens with its predecessor's final unit so a
/// thought split across the boundary is findable from either side;
/// skipped when that unit alone is most of a budget.
const MAX_OVERLAP_WORDS: usize = 120;

fn words(text: &str) -> usize {
    text.split_whitespace().count()
}

fn content_hash(embedding_text: &str) -> String {
    let digest = Sha256::digest(embedding_text.as_bytes());
    digest[..16].iter().map(|b| format!("{b:02x}")).collect()
}

/// One packable unit of text: a transcript paragraph, a prose block, or a
/// slice of what one image said.
#[derive(Default)]
struct Unit {
    text: String,
    start: Option<f64>,
    end: Option<f64>,
    speaker: Option<String>,
    speaker_id: Option<String>,
    /// The image this unit was read out of, for image units only.
    image: Option<String>,
}

/// Pack consecutive units into chunk-sized groups (indices into `units`).
fn pack(units: &[Unit]) -> Vec<Vec<usize>> {
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    let mut count = 0usize;

    for (i, unit) in units.iter().enumerate() {
        let w = words(&unit.text);
        if !current.is_empty() && count + w > MAX_WORDS {
            let overlap = *current.last().expect("non-empty group");
            groups.push(std::mem::take(&mut current));
            count = 0;
            if words(&units[overlap].text) <= MAX_OVERLAP_WORDS {
                current.push(overlap);
                count = words(&units[overlap].text);
            }
        }
        current.push(i);
        count += w;
    }
    // Every flush is immediately followed by a push, so a non-empty tail is
    // never the bare overlap seed.
    if !current.is_empty() {
        groups.push(current);
    }
    groups
}

fn header(title: &str, date: DateTime<Utc>, speakers: &[String]) -> String {
    let day = date.format("%Y-%m-%d");
    if speakers.is_empty() {
        format!("{title} — {day}.")
    } else {
        format!("{title} — {day}. {}", speakers.join(", "))
    }
}

fn build(source: Source, units: &[Unit], title: &str, date: DateTime<Utc>) -> Vec<BuiltChunk> {
    let mut out = Vec::new();
    for group in pack(units) {
        let text = group
            .iter()
            .map(|&i| units[i].text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        let mut speakers: Vec<String> = Vec::new();
        let mut speaker_ids: Vec<String> = Vec::new();
        for &i in &group {
            if let Some(s) = &units[i].speaker {
                if !speakers.iter().any(|e| e == s) {
                    speakers.push(s.clone());
                }
            }
            if let Some(id) = &units[i].speaker_id {
                if !speaker_ids.iter().any(|e| e == id) {
                    speaker_ids.push(id.clone());
                }
            }
        }
        let embedding_text = format!("{}\n{}", header(title, date, &speakers), text);
        out.push(BuiltChunk {
            source,
            chunk_index: out.len() as u32,
            content_hash: content_hash(&embedding_text),
            start_secs: group.iter().filter_map(|&i| units[i].start).next(),
            end_secs: group.iter().rev().filter_map(|&i| units[i].end).next(),
            // Two short images can pack into one chunk; the passage opens
            // with the first, so that is the one a hit should point at.
            image_filename: group.iter().find_map(|&i| units[i].image.clone()),
            text,
            embedding_text,
            speakers,
            speaker_ids,
        })
    }
    out
}

/// Drop image links, keeping their alt text.
///
/// A chunk's `text` is what search matches, what the embedder reads, and
/// what the palette shows as a snippet. An image link is a file path: as
/// tokens it is noise, as embedded content it drags the passage's meaning
/// toward nothing, and as a snippet it reads like a bug. The alt text is
/// real prose about the image and stays.
fn strip_image_links(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    let mut rest = md;
    while let Some(start) = rest.find("![") {
        let (before, from_bang) = rest.split_at(start);
        out.push_str(before);
        let Some(close) = from_bang[2..].find(']') else {
            out.push_str(from_bang);
            return out;
        };
        let after_label = 2 + close + 1;
        let alt = &from_bang[2..2 + close];
        if !from_bang[after_label..].starts_with('(') {
            out.push_str(alt);
            rest = &from_bang[after_label..];
            continue;
        }
        let Some(end) = from_bang[after_label + 1..].find(')') else {
            out.push_str(from_bang);
            return out;
        };
        out.push_str(alt);
        rest = &from_bang[after_label + 1 + end + 1..];
    }
    out.push_str(rest);
    out
}

/// Strip YAML frontmatter and a leading `# ` title line: document
/// scaffolding, not content.
fn strip_scaffolding(md: &str) -> &str {
    let mut rest = md.trim_start();
    if let Some(after) = rest.strip_prefix("---") {
        if let Some(end) = after.find("\n---") {
            rest = after[end + 4..].trim_start();
        }
    }
    if rest.starts_with("# ") {
        rest = rest.split_once('\n').map(|(_, r)| r).unwrap_or("");
    }
    rest.trim()
}

/// Blank-line blocks of prose (headings stay in their block position).
fn prose_units(text: &str) -> Vec<Unit> {
    text.split("\n\n")
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .map(|b| Unit {
            text: b.to_string(),
            ..Default::default()
        })
        .collect()
}

/// Best-effort `Name: text` speaker extraction for transcript documents
/// that no longer have segments (legacy imports).
fn labeled_prose_units(text: &str) -> Vec<Unit> {
    prose_units(text)
        .into_iter()
        .map(|mut u| {
            if let Some((name, _)) = u.text.split_once(": ") {
                if !name.is_empty() && name.len() <= 40 && !name.contains('\n') {
                    u.speaker = Some(name.to_string());
                }
            }
            u
        })
        .collect()
}

pub fn chunk_meeting(docs: &MeetingDocs) -> Vec<BuiltChunk> {
    let mut out = Vec::new();

    // Transcript: paragraphs from segments when we have them; the rendered
    // document is the fallback for meetings that predate segment storage.
    if !docs.segments.is_empty() {
        let units: Vec<Unit> = paragraphs(docs.segments)
            .into_iter()
            .map(|p| Unit {
                text: p.text,
                start: Some(p.start),
                end: Some(p.end),
                speaker: p.speaker,
                speaker_id: p.speaker_id,
                image: None,
            })
            .collect();
        out.extend(build(Source::Transcript, &units, docs.title, docs.started_at));
    } else {
        let body = strip_scaffolding(docs.transcript);
        // The transcript-less placeholder is document scaffolding, not
        // content; indexed, it wins semantic queries it has no answer to.
        if !body.is_empty() && body != "_No transcript segments were captured._" {
            let units = labeled_prose_units(body);
            out.extend(build(Source::Transcript, &units, docs.title, docs.started_at));
        }
    }

    let notes = strip_image_links(docs.user_notes);
    let notes = notes.trim();
    if !notes.is_empty() {
        out.extend(build(
            Source::UserNotes,
            &prose_units(notes),
            docs.title,
            docs.started_at,
        ));
    }

    let summary = strip_image_links(&strip_scaffolding(docs.summary));
    let summary = summary.trim();
    if !summary.is_empty() {
        out.extend(build(
            Source::Summary,
            &prose_units(summary),
            docs.title,
            docs.started_at,
        ));
    }

    // One unit per image, so two slides never blend into one passage. A
    // full-page screenshot is the exception the `blocks` split exists for.
    let image_units: Vec<Unit> = docs
        .image_text
        .iter()
        .flat_map(|(filename, text)| {
            embral_notes::ocr::blocks(text, MAX_WORDS)
                .into_iter()
                .map(move |text| Unit {
                    text,
                    image: Some(filename.clone()),
                    ..Default::default()
                })
        })
        .collect();
    if !image_units.is_empty() {
        out.extend(build(
            Source::ImageText,
            &image_units,
            docs.title,
            docs.started_at,
        ));
    }

    out
}

/// The OCR text of the images a meeting's documents currently link, in
/// paste order.
///
/// An image's bytes are never collected when the user deletes it from their
/// notes: the summary may still be showing the same file. Its text is a
/// different matter: search quoting a screenshot the user removed from
/// their writing, and cannot see anywhere, reads as a bug. The row
/// stays cached, so putting the image back costs nothing.
///
/// Unusable readings are dropped here too, in one place, so the index and
/// the summary prompt agree on what an image is worth.
pub fn referenced_image_text(
    meeting_id: &str,
    documents: &[&str],
    stored: &[(String, String)],
) -> Vec<(String, String)> {
    let linked: std::collections::HashSet<String> = documents
        .iter()
        .flat_map(|doc| embral_notes::assets::image_links(doc))
        .collect();
    stored
        .iter()
        .filter(|(filename, text)| {
            linked.contains(&embral_notes::assets::link_rel(meeting_id, filename))
                && embral_notes::ocr::is_usable(text)
        })
        .cloned()
        .collect()
}

/// Dictations are usually one thought; chunked only when long.
pub fn chunk_dictation(created_at: DateTime<Utc>, text: &str) -> Vec<BuiltChunk> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }
    let units = if words(text) > MAX_WORDS {
        prose_units(text)
    } else {
        vec![Unit {
            text: text.to_string(),
            ..Default::default()
        }]
    };
    build(Source::Dictation, &units, "Dictation", created_at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn seg(speaker: Option<&str>, text: &str, start: f64, end: f64) -> TranscriptionSegment {
        TranscriptionSegment {
            speaker: speaker.map(String::from),
            speaker_id: speaker.map(|s| format!("id-{s}")),
            text: text.to_string(),
            start,
            end,
        }
    }

    fn date() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 1, 10, 0, 0).unwrap()
    }

    fn docs<'a>(segments: &'a [TranscriptionSegment]) -> MeetingDocs<'a> {
        MeetingDocs {
            title: "Planning Sync",
            started_at: date(),
            segments,
            user_notes: "",
            summary: "",
            transcript: "",
            image_text: &[],
        }
    }

    #[test]
    fn passages_respect_the_word_budget_and_overlap() {
        // 40 paragraphs of ~50 words each (alternating speakers force breaks).
        let sentence = "these are exactly ten words of filler for the test.";
        let long: String = std::iter::repeat(sentence).take(5).collect::<Vec<_>>().join(" ");
        let segments: Vec<_> = (0..40)
            .map(|i| {
                seg(
                    Some(if i % 2 == 0 { "A" } else { "B" }),
                    &long,
                    i as f64 * 10.0,
                    i as f64 * 10.0 + 9.0,
                )
            })
            .collect();

        let chunks = chunk_meeting(&docs(&segments));
        assert!(chunks.len() > 1);
        for c in &chunks {
            assert!(words(&c.text) <= MAX_WORDS + MAX_OVERLAP_WORDS, "chunk too big");
        }
        // Overlap: each later chunk begins with its predecessor's last paragraph.
        for pair in chunks.windows(2) {
            let last_para = pair[0].text.split("\n\n").last().unwrap();
            assert!(pair[1].text.starts_with(last_para));
        }
        // Speakers and timing carried.
        assert!(chunks[0].speakers.contains(&"A".to_string()));
        assert!(chunks[0].speaker_ids.contains(&"id-A".to_string()));
        assert_eq!(chunks[0].start_secs, Some(0.0));
    }

    #[test]
    fn embedding_text_carries_the_context_header() {
        let segments = [seg(Some("Alice"), "We should ship the beta.", 0.0, 2.0)];
        let chunks = chunk_meeting(&docs(&segments));
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0]
            .embedding_text
            .starts_with("Planning Sync — 2026-07-01. Alice\n"));
        assert_eq!(chunks[0].text, "We should ship the beta.");
    }

    #[test]
    fn hash_is_stable_and_content_sensitive() {
        let segments = [seg(Some("Alice"), "We should ship the beta.", 0.0, 2.0)];
        let a = chunk_meeting(&docs(&segments));
        let b = chunk_meeting(&docs(&segments));
        assert_eq!(a[0].content_hash, b[0].content_hash);

        let mut d = docs(&segments);
        d.title = "Renamed Sync"; // header feeds the hash; a rename re-embeds
        let c = chunk_meeting(&d);
        assert_ne!(a[0].content_hash, c[0].content_hash);
    }

    #[test]
    fn every_meeting_source_chunks_distinctly() {
        let segments = [seg(Some("Alice"), "Spoken words here.", 0.0, 2.0)];
        let image_text = vec![("img-01.png".to_string(), "what the slide said".to_string())];
        let mut d = docs(&segments);
        d.user_notes = "my own shorthand note";
        d.summary = "---\nmeeting_id: x\n---\n# Planning Sync\n\n## Key Takeaways\n\nShip it.";
        d.image_text = &image_text;
        let chunks = chunk_meeting(&d);

        let sources: Vec<Source> = chunks.iter().map(|c| c.source).collect();
        assert!(sources.contains(&Source::Transcript));
        assert!(sources.contains(&Source::UserNotes));
        assert!(sources.contains(&Source::Summary));
        assert!(sources.contains(&Source::ImageText));
        // Frontmatter and the title line never become content.
        let summary = chunks.iter().find(|c| c.source == Source::Summary).unwrap();
        assert!(!summary.text.contains("meeting_id"));
        assert!(!summary.text.contains("# Planning Sync"));
        assert!(summary.text.contains("Ship it."));
    }

    #[test]
    fn segmentless_meetings_fall_back_to_the_rendered_transcript() {
        let mut d = docs(&[]);
        d.transcript =
            "---\nmeeting_id: x\n---\n# Old Import Transcript\n\nDana: We agreed on the vendor.\n\nUnattributed closing remarks.";
        let chunks = chunk_meeting(&d);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].source, Source::Transcript);
        assert!(chunks[0].speakers.contains(&"Dana".to_string()));
        assert!(chunks[0].start_secs.is_none());
    }

    #[test]
    fn the_no_transcript_placeholder_is_not_content() {
        let mut d = docs(&[]);
        d.transcript =
            "---\nmeeting_id: x\n---\n# Quiet Meeting Transcript\n\n_No transcript segments were captured._";
        assert!(chunk_meeting(&d).is_empty());
    }

    #[test]
    fn dictations_stay_whole_until_long() {
        let short = chunk_dictation(date(), "send the follow-up email tomorrow");
        assert_eq!(short.len(), 1);
        assert_eq!(short[0].source, Source::Dictation);
        assert!(short[0].embedding_text.starts_with("Dictation — 2026-07-01.\n"));

        let para = "word ".repeat(300);
        let long_text = format!("{para}\n\n{para}\n\n{para}");
        let long = chunk_dictation(date(), &long_text);
        assert!(long.len() > 1);

        assert!(chunk_dictation(date(), "   ").is_empty());
    }
    /// A file path is not searchable prose: as tokens it is noise, as
    /// embedded content it drags a passage toward nothing, and as a palette
    /// snippet it reads like a bug. The alt text survives, because that part
    /// is real writing about the image.
    #[test]
    fn image_links_leave_the_index_but_their_alt_text_stays() {
        assert_eq!(
            strip_image_links("before ![the pipeline chart](assets/m1/img-01.png) after"),
            "before the pipeline chart after"
        );
        // No alt text: nothing of the link remains.
        assert_eq!(strip_image_links("a ![](assets/m1/img-02.png) b"), "a  b");
        // Ordinary links are not images and are left alone.
        let link = "see [the spec](https://embral.app) for more";
        assert_eq!(strip_image_links(link), link);
        // Prose with no images is returned untouched.
        let plain = "# Title

Just words.
";
        assert_eq!(strip_image_links(plain), plain);
    }

    /// The path leaves the index and the OCR text takes its place: the
    /// two halves of the same decision.
    #[test]
    fn what_an_image_says_is_indexed_under_its_own_source() {
        let image_text = vec![(
            "img-01.png".to_string(),
            "Q3 revenue 4.2M\nQ4 forecast 5.1M".to_string(),
        )];
        let mut docs = docs(&[]);
        docs.user_notes = "the numbers are in ![the chart](assets/m1/img-01.png)";
        docs.image_text = &image_text;
        let built = chunk_meeting(&docs);

        let notes: Vec<&BuiltChunk> =
            built.iter().filter(|c| c.source == Source::UserNotes).collect();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].text, "the numbers are in the chart");
        assert!(!notes[0].text.contains("img-01"));

        let images: Vec<&BuiltChunk> =
            built.iter().filter(|c| c.source == Source::ImageText).collect();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].text, "Q3 revenue 4.2M\nQ4 forecast 5.1M");
        // The context header is included, like every other source.
        assert!(images[0].embedding_text.contains("Planning Sync"));
        // And the passage knows which image it came out of, which is the
        // only way a search hit can point at one.
        assert_eq!(images[0].image_filename.as_deref(), Some("img-01.png"));
        assert!(notes[0].image_filename.is_none(), "only image passages name one");
    }

    /// Two short images pack into one chunk, as any two short units would,
    /// but they stay separate units, so one slide's last line never runs
    /// into the next slide's first.
    #[test]
    fn two_images_stay_two_units() {
        let image_text = vec![
            ("img-01.png".to_string(), "Q3 revenue 4.2M".to_string()),
            ("img-02.png".to_string(), "the hiring plan for October".to_string()),
        ];
        let mut docs = docs(&[]);
        docs.image_text = &image_text;
        let built = chunk_meeting(&docs);
        let images: Vec<&BuiltChunk> =
            built.iter().filter(|c| c.source == Source::ImageText).collect();
        // Short enough to pack together; they must not merge into one unit.
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].text, "Q3 revenue 4.2M\n\nthe hiring plan for October");
        // The passage opens with the first image, so that is the one a hit
        // points at.
        assert_eq!(images[0].image_filename.as_deref(), Some("img-01.png"));
    }

    #[test]
    fn a_meeting_with_no_images_gains_no_chunks() {
        assert!(chunk_meeting(&docs(&[])).is_empty());
    }

    /// Only images a document still links get indexed. Deleting an image
    /// from the notes must take its text out of search, even though the
    /// file itself stays (the summary may still be showing it).
    #[test]
    fn only_the_images_a_document_still_links_are_indexed() {
        let stored = vec![
            ("img-01.png".to_string(), "Q3 revenue 4.2M".to_string()),
            ("img-02.png".to_string(), "the hiring plan for October".to_string()),
        ];
        let notes = "kept ![a](assets/m1/img-01.png)";
        let summary = "no images here";
        assert_eq!(
            referenced_image_text("m1", &[notes, summary], &stored),
            vec![("img-01.png".to_string(), "Q3 revenue 4.2M".to_string())]
        );

        // The summary counts too: an image the user removed from the notes
        // is still live while the summary places it.
        let summary = "as shown ![b](assets/m1/img-02.png)";
        assert_eq!(
            referenced_image_text("m1", &["", summary], &stored),
            vec![(
                "img-02.png".to_string(),
                "the hiring plan for October".to_string()
            )]
        );

        // Another meeting's path never matches, even with the same filename.
        assert!(referenced_image_text("m2", &[notes, ""], &stored).is_empty());
    }

    #[test]
    fn a_reading_that_is_not_worth_indexing_is_dropped() {
        let stored = vec![
            ("img-01.png".to_string(), "|| ~ ^^".to_string()),
            ("img-02.png".to_string(), String::new()),
            ("img-03.png".to_string(), "Q3 revenue 4.2M".to_string()),
        ];
        let notes = "![a](assets/m1/img-01.png) ![b](assets/m1/img-02.png) \
                     ![c](assets/m1/img-03.png)";
        assert_eq!(
            referenced_image_text("m1", &[notes], &stored),
            vec![("img-03.png".to_string(), "Q3 revenue 4.2M".to_string())]
        );
    }
}
