//! Shared helpers behind the command modules: path safety, frontmatter
//! read/write, the `MeetingDetail` payload, and transcript formatting.
//! Also reached from `storage.rs` and `speaker_commands.rs` via
//! `crate::commands::` (re-exported by `mod.rs`).

use embral_db::MeetingRow;
use embral_types::{AppError, MeetingRecord};
use std::path::{Component, Path, PathBuf};

#[derive(serde::Serialize)]
pub struct MeetingDetail {
    pub record: MeetingRecord,
    pub summary: String,
    pub transcript: String,
    pub audio_path: Option<String>,
    pub audio_exists: bool,
    pub attendees: Vec<String>,
    /// Structured transcript; empty for legacy meetings that only have
    /// markdown (the UI falls back to the raw editor then).
    pub segments: Vec<embral_types::TranscriptionSegment>,
    /// Pending "Speaker N looks like X" suggestions from the user's notes.
    pub name_suggestions: Vec<crate::speaker_commands::NameSuggestionView>,
    /// User-starred moments (empty when none).
    pub stars: Vec<Star>,
    /// The user's raw live notes, verbatim (the Notes tab).
    pub notes: String,
}

/// One starred moment: when it happened, and (when the notes editor was
/// mounted at stop) which top-level block of the user's notes it sits on.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Star {
    pub seconds: f64,
    pub note_block: Option<u32>,
}

/// Also used by the audio janitor in `storage.rs`.
pub(crate) fn resolve_indexed_path(base: &Path, indexed_path: &str) -> Result<PathBuf, AppError> {
    let path = Path::new(indexed_path);
    if path.is_absolute() {
        return Err(AppError::internal("Indexed meeting path must be relative"));
    }
    if path.components().any(|c| {
        matches!(
            c,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(AppError::internal(
            "Indexed meeting path escapes the storage directory",
        ));
    }
    Ok(base.join(path))
}

pub(crate) fn strip_frontmatter(markdown: &str) -> &str {
    if !markdown.starts_with("---") {
        return markdown;
    }
    let Some(end) = markdown.find("\n---") else {
        return markdown;
    };
    let closing_end = markdown[end + 4..]
        .find('\n')
        .map(|offset| end + 4 + offset + 1)
        .unwrap_or(markdown.len());
    markdown[closing_end..].trim_start()
}

pub(crate) fn frontmatter_value(markdown: &str, key: &str) -> Option<String> {
    if !markdown.starts_with("---") {
        return None;
    }
    let end = markdown.find("\n---")?;
    let block = &markdown[3..end];
    for line in block.lines() {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim() == key {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

pub(crate) fn parse_attendees_value(value: &str) -> Vec<String> {
    if let Ok(names) = serde_json::from_str::<Vec<String>>(value) {
        return normalize_attendees(names);
    }

    let trimmed = value.trim().trim_start_matches('[').trim_end_matches(']');
    if trimmed.is_empty() {
        return Vec::new();
    }
    normalize_attendees(trimmed.split(',').map(ToString::to_string).collect())
}

/// Also used by the legacy index.json import in `storage.rs`.
pub(crate) fn parse_attendees(markdown: &str) -> Vec<String> {
    frontmatter_value(markdown, "attendees")
        .map(|value| parse_attendees_value(&value))
        .unwrap_or_default()
}

pub(crate) fn normalize_attendees(attendees: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for attendee in attendees {
        let attendee = attendee.trim().trim_matches('"').to_string();
        if !attendee.is_empty() && !out.iter().any(|existing| existing == &attendee) {
            out.push(attendee);
        }
    }
    out
}

pub(crate) fn canonical_frontmatter(
    start_time: &str,
    duration_minutes: u32,
    meeting_id: &str,
    attendees: &[String],
) -> String {
    let attendees = serde_json::to_string(attendees).unwrap_or_else(|_| "[]".to_string());
    format!(
        "---\nstart_time: {}\nduration_minutes: {}\nmeeting_id: {}\nattendees: {}\n---\n",
        start_time, duration_minutes, meeting_id, attendees
    )
}

pub(crate) fn prepend_frontmatter(markdown: &str, frontmatter: &str) -> String {
    format!(
        "{}\n{}",
        frontmatter.trim_end(),
        strip_frontmatter(markdown)
    )
}

pub(crate) fn attendees_from_segments(segments: &[embral_types::TranscriptionSegment]) -> Vec<String> {
    let mut speakers = Vec::new();
    for segment in segments {
        if let Some(speaker) = segment.speaker.as_deref() {
            let speaker = speaker.trim();
            if !speaker.is_empty() && !speakers.iter().any(|s| s == speaker) {
                speakers.push(speaker.to_string());
            }
        }
    }
    speakers
}

pub(crate) fn fallback_duration_minutes(record: &MeetingRecord) -> u32 {
    ((record.duration_seconds as f64 / 60.0).ceil() as u32).max(1)
}

pub(crate) fn canonicalize_start_time(value: &str) -> Option<String> {
    chrono::DateTime::parse_from_rfc3339(value).ok().map(|dt| {
        dt.with_timezone(&chrono::Utc)
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    })
}

pub(crate) fn canonicalize_frontmatter(
    markdown: &str,
    record: &MeetingRecord,
    attendees: &[String],
) -> String {
    let start_time = frontmatter_value(markdown, "start_time")
        .and_then(|value| canonicalize_start_time(&value))
        .unwrap_or_else(|| {
            record
                .date
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        });
    let duration_minutes = frontmatter_value(markdown, "duration_minutes")
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or_else(|| fallback_duration_minutes(record));
    let meeting_id = frontmatter_value(markdown, "meeting_id").unwrap_or_else(|| record.id.clone());
    let frontmatter = canonical_frontmatter(&start_time, duration_minutes, &meeting_id, attendees);
    prepend_frontmatter(markdown, &frontmatter)
}

pub(crate) fn remove_indexed_file(base: &Path, indexed_path: &str) -> Result<(), AppError> {
    if indexed_path.trim().is_empty() {
        return Ok(());
    }
    let path = resolve_indexed_path(base, indexed_path)?;
    if path.is_file() {
        std::fs::remove_file(path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Remove a meeting's asset directory and everything in it. Missing is not
/// an error; most meetings never had one.
pub(crate) fn remove_meeting_assets(base: &Path, meeting_id: &str) {
    let rel = embral_notes::assets::asset_dir_rel(meeting_id);
    let Ok(dir) = resolve_indexed_path(base, &rel) else {
        return;
    };
    if dir.is_dir() {
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            tracing::warn!("could not remove {}: {e}", dir.display());
        }
    }
}

pub(crate) fn rename_indexed_file(base: &Path, old_path: &str, new_path: &str) -> Result<(), AppError> {
    if old_path.trim().is_empty() || old_path == new_path {
        return Ok(());
    }
    let old = resolve_indexed_path(base, old_path)?;
    let new = resolve_indexed_path(base, new_path)?;
    if let Some(parent) = new.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    if old.is_file() {
        std::fs::rename(old, new).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Fetch a meeting row or produce the standard not-found message.
pub(crate) fn require_row(db: &embral_db::Db, id: &str) -> Result<MeetingRow, AppError> {
    db.get_meeting(id)
        .map_err(AppError::internal)?
        .ok_or_else(|| AppError::MeetingNotFound { id: id.to_string() })
}

/// Build the frontend detail payload from a DB row. Markdown comes straight
/// from the database; only the audio file is checked on disk.
pub(crate) fn meeting_detail(
    db: &embral_db::Db,
    base: &Path,
    row: MeetingRow,
) -> Result<MeetingDetail, AppError> {
    let audio_path_value = row.audio_path.trim();
    let audio_file_path = if audio_path_value.is_empty() {
        None
    } else {
        Some(resolve_indexed_path(base, audio_path_value)?)
    };
    let audio_exists = audio_file_path.as_ref().is_some_and(|path| path.is_file());
    let audio_path = if audio_exists {
        audio_file_path.map(|path| path.to_string_lossy().to_string())
    } else {
        None
    };

    let segments = db.get_segments(&row.id).map_err(|e| e.to_string())?;
    let name_suggestions =
        crate::speaker_commands::name_suggestion_views(db, &row.id).unwrap_or_default();
    let stars = db
        .get_stars(&row.id)
        .ok()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default();
    let notes = db.get_notes(&row.id).unwrap_or_default();

    Ok(MeetingDetail {
        record: row.to_record(),
        summary: row.summary,
        transcript: row.transcript,
        audio_path,
        audio_exists,
        attendees: row.attendees,
        segments,
        name_suggestions,
        stars,
        notes,
    })
}

pub(crate) fn meeting_timestamp_prefix(record: &MeetingRecord) -> String {
    record
        .id
        .get(..13.min(record.id.len()))
        .unwrap_or(&record.id)
        .to_string()
}

pub(crate) fn meeting_start_time(meeting_id: &str) -> chrono::DateTime<chrono::Utc> {
    let prefix = meeting_id
        .split_once('_')
        .map(|(prefix, _)| prefix)
        .unwrap_or(meeting_id);
    chrono::NaiveDateTime::parse_from_str(prefix, "%y%m%dT%H%M%S")
        .map(|dt| chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(dt, chrono::Utc))
        .unwrap_or_else(|_| chrono::Utc::now())
}

pub(crate) fn normalize_title(title: String) -> Result<String, AppError> {
    let title = title.trim().to_string();
    if title.is_empty() {
        Err(AppError::TitleEmpty)
    } else {
        Ok(title)
    }
}

pub(crate) fn transcript_frontmatter(
    meeting_id: &str,
    start_time: &str,
    duration_minutes: u32,
    attendees: &[String],
) -> String {
    canonical_frontmatter(start_time, duration_minutes, meeting_id, attendees)
}

pub(crate) fn format_transcript_document(
    title: &str,
    meeting_id: &str,
    start_time: &str,
    duration_minutes: u32,
    attendees: &[String],
    transcript_text: &str,
) -> String {
    let heading = if title.trim().is_empty() {
        "Transcript".to_string()
    } else {
        format!("{} Transcript", title.trim())
    };
    let transcript_body = if transcript_text.trim().is_empty() {
        "_No transcript segments were captured._"
    } else {
        transcript_text.trim()
    };

    format!(
        "{}\n# {}\n\n{}",
        transcript_frontmatter(meeting_id, start_time, duration_minutes, attendees),
        heading,
        transcript_body
    )
}
