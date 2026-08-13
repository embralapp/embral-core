//! Meeting library commands: list, detail, palette search, title/notes/
//! transcript edits, and deletes.

use embral_db::{MeetingCursor, MeetingRow};
use embral_types::{AppError, MeetingRecord, MeetingSummary};
use tauri::{AppHandle, State};

use crate::AppState;

use super::support::*;

#[tauri::command]
pub async fn get_meetings(
    state: State<'_, AppState>,
    limit: Option<u32>,
    since: Option<String>,
) -> Result<Vec<MeetingSummary>, AppError> {
    let db = state.db().await?;
    let since = parse_since(since)?;
    let rows = db.list_meetings(limit, since).map_err(|e| e.to_string())?;
    Ok(rows
        .iter()
        .map(|r| MeetingSummary::from(&r.to_record()))
        .collect())
}

/// The last row of the page the list already has, so the next call can
/// continue from it. The fields are a `MeetingRecord`'s own `date` and `id`
/// handed straight back, which is why this is a date rather than a row
/// number: see [`MeetingCursor`].
#[derive(serde::Deserialize)]
pub struct MeetingPageCursor {
    pub date: String,
    pub id: String,
}

#[tauri::command]
pub async fn get_meeting_records(
    state: State<'_, AppState>,
    limit: Option<u32>,
    since: Option<String>,
    before: Option<MeetingPageCursor>,
) -> Result<Vec<MeetingRecord>, AppError> {
    let db = state.db().await?;
    let since = parse_since(since)?;
    let before = before
        .map(|c| {
            Ok::<_, AppError>(MeetingCursor {
                started_at: parse_rfc3339(&c.date)?,
                id: c.id,
            })
        })
        .transpose()?;
    let rows = db
        .list_meetings_page(limit, since, before.as_ref())
        .map_err(|e| e.to_string())?;
    Ok(rows.iter().map(MeetingRow::to_record).collect())
}

fn parse_rfc3339(value: &str) -> Result<chrono::DateTime<chrono::Utc>, AppError> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .map_err(AppError::internal)
}

fn parse_since(since: Option<String>) -> Result<Option<chrono::DateTime<chrono::Utc>>, AppError> {
    since.as_deref().map(parse_rfc3339).transpose()
}

#[tauri::command]
pub async fn get_meeting_detail(
    state: State<'_, AppState>,
    id: String,
) -> Result<MeetingDetail, AppError> {
    let config = state.config.lock().await.clone();
    let base = crate::storage::storage_base(&config.storage_dir);
    let db = state.db().await?;
    meeting_detail(&db, &base, require_row(&db, &id)?)
}

/// One meeting the search matched, plus enough to land on the passage that
/// matched it. Without the locator the palette can only open the meeting at
/// the top of whichever tab the config prefers, leaving the user to find by
/// eye the sentence the search already knew.
#[derive(serde::Serialize)]
pub struct LibraryMeetingHit {
    pub id: String,
    pub title: String,
    pub started_at: String,
    pub snippet: String,
    /// Which document the passage came from; it names the tab to open.
    pub source: String,
    /// Where in the recording the passage runs, for transcript passages.
    /// The pair bounds the search for the words the user typed: a common
    /// one ("north") occurs all over a long transcript, and only the
    /// occurrence inside this passage is the one the result showed.
    pub start_secs: Option<f64>,
    pub end_secs: Option<f64>,
    /// The passage's opening line, for finding it in a document. Not the
    /// whole passage: a line is enough to locate it, and twelve 400-word
    /// passages is a lot of payload for drawing a list.
    pub lead: String,
    /// The image an `image_text` passage was read out of. Its text is
    /// inside a PNG, so there is nothing in the document to search for.
    pub image: Option<String>,
}

#[derive(serde::Serialize)]
pub struct LibraryDictationHit {
    pub id: i64,
    pub snippet: String,
    pub created_at: String,
}

#[derive(serde::Serialize)]
pub struct LibrarySearchResults {
    pub meetings: Vec<LibraryMeetingHit>,
    pub dictations: Vec<LibraryDictationHit>,
}

/// The passage's opening line, trimmed: what the frontend looks for in the
/// document to scroll to. A whole line is distinctive enough to find the
/// right paragraph and short enough to send twelve of.
fn hit_lead(hit: &embral_search::Hit) -> String {
    hit.text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .chars()
        .take(160)
        .collect()
}

/// A semantic-only hit carries no FTS excerpt; lead with the passage.
fn hit_snippet(hit: &embral_search::Hit) -> String {
    hit.snippet.clone().unwrap_or_else(|| {
        let mut text: String = hit.text.chars().take(140).collect();
        if text.len() < hit.text.len() {
            text.push('…');
        }
        text
    })
}

/// The palette's search: the hybrid engine over meetings (best passage per
/// meeting) and dictations in one call. The vector leg joins only when the
/// embed worker is already warm, so a keystroke never waits on a model load;
/// a cold worker gets a background warm-up and the next keystroke benefits.
#[tauri::command]
pub async fn search_library(
    app: AppHandle,
    state: State<'_, AppState>,
    query: String,
    limit: Option<u32>,
) -> Result<LibrarySearchResults, AppError> {
    // Timed end to end. The legs are benched separately (embral-search's
    // bench harness); this line says where keystroke time actually goes.
    let started = std::time::Instant::now();
    crate::telemetry::track(&state, "search_used", serde_json::json!({}));
    let db = state.db().await?;
    let acquire = started.elapsed();

    let q = query.trim().to_string();
    let mut vector: Option<Vec<f32>> = None;
    let mut embed = std::time::Duration::ZERO;
    if q.chars().count() >= 4 && embral_search::model::present() {
        if state.search.is_warm().await {
            let t = std::time::Instant::now();
            match state.search.embed_query(&q).await {
                Ok(v) => vector = Some(v),
                Err(e) => tracing::debug!("query embed failed, keyword-only: {e}"),
            }
            embed = t.elapsed();
        } else {
            crate::search_index::SearchRuntime::warm_up(app.clone());
        }
    }

    let limit = limit.unwrap_or(12) as usize;
    let mut args = embral_search::SearchArgs::new(&q, embral_search::OwnerKind::Meetings);
    // Chunk-level hits collapse to meetings below; fetch extra so dense
    // meetings don't crowd out the rest.
    args.limit = limit * 3;
    args.prefix_last_token = true;
    let chunk_hits =
        embral_search::search(&db, &args, vector.as_deref()).map_err(|e| e.to_string())?;

    let mut seen = std::collections::HashSet::new();
    let mut meetings = Vec::new();
    for hit in &chunk_hits {
        let Some(meeting_id) = hit.meeting_id.clone() else { continue };
        if !seen.insert(meeting_id.clone()) {
            continue;
        }
        meetings.push(LibraryMeetingHit {
            id: meeting_id,
            title: hit.title.clone().unwrap_or_default(),
            started_at: hit.date.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            snippet: hit_snippet(hit),
            source: hit.source.clone(),
            start_secs: hit.start_secs,
            end_secs: hit.end_secs,
            lead: hit_lead(hit),
            image: hit.image_filename.clone(),
        });
        if meetings.len() >= limit {
            break;
        }
    }

    let mut args = embral_search::SearchArgs::new(&q, embral_search::OwnerKind::Dictations);
    args.limit = 5;
    args.prefix_last_token = true;
    let dictations = embral_search::search(&db, &args, vector.as_deref())
        .map_err(|e| e.to_string())?
        .iter()
        .filter_map(|hit| {
            Some(LibraryDictationHit {
                id: hit.dictation_id?,
                snippet: hit_snippet(hit),
                created_at: hit.date.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            })
        })
        .collect();

    // Debug, not info: this fires on every debounced keystroke, and a shipped
    // log should not be a keylogger of what the user searched for.
    tracing::debug!(
        meetings = meetings.len(),
        semantic = vector.is_some(),
        acquire_ms = acquire.as_secs_f64() * 1000.0,
        embed_ms = embed.as_secs_f64() * 1000.0,
        total_ms = started.elapsed().as_secs_f64() * 1000.0,
        "search"
    );
    Ok(LibrarySearchResults { meetings, dictations })
}

#[tauri::command]
pub async fn update_meeting_title(
    state: State<'_, AppState>,
    id: String,
    title: String,
) -> Result<MeetingDetail, AppError> {
    let title = normalize_title(title)?;
    let config = state.config.lock().await.clone();
    let base = crate::storage::storage_base(&config.storage_dir);
    let db = state.db().await?;
    let mut row = require_row(&db, &id)?;
    let old_record = row.to_record();

    // Audio is the one file a meeting owns, so it is the only rename left:
    // both documents live in the database.
    let new_audio_path = if row.audio_path.trim().is_empty() {
        String::new()
    } else {
        let safe_title = crate::refinement::sanitize_filename(&title);
        let stem = format!("{} - {}", meeting_timestamp_prefix(&old_record), safe_title);
        let path = format!("audio/{}.mp3", stem);
        rename_indexed_file(&base, &row.audio_path, &path)?;
        path
    };

    // A meeting recorded with summaries off has no summary document; leave it
    // that way rather than writing a heading over nothing.
    if !row.summary.trim().is_empty() {
        let titled = crate::refinement::apply_title(&row.summary, &title);
        row.summary = canonicalize_frontmatter(&titled, &old_record, &row.attendees);
    }
    if !row.transcript.trim().is_empty() {
        let titled =
            crate::refinement::apply_title(&row.transcript, &format!("{} Transcript", title));
        row.transcript = canonicalize_frontmatter(&titled, &old_record, &row.attendees);
    }

    row.title = title;
    row.audio_path = new_audio_path;
    db.upsert_meeting(&row).map_err(|e| e.to_string())?;
    crate::storage::export_index(&db, &base).map_err(|e| e.to_string())?;
    crate::search_index::sync_meeting(&db, &state.search, &row.id);

    meeting_detail(&db, &base, row)
}

#[tauri::command]
pub async fn update_meeting_summary(
    state: State<'_, AppState>,
    id: String,
    markdown: String,
) -> Result<MeetingDetail, AppError> {
    let config = state.config.lock().await.clone();
    let base = crate::storage::storage_base(&config.storage_dir);
    let db = state.db().await?;
    let mut row = require_row(&db, &id)?;
    let record = row.to_record();

    let mut attendees = parse_attendees(&markdown);
    if attendees.is_empty() {
        attendees = row.attendees.clone();
    }
    row.summary = canonicalize_frontmatter(&markdown, &record, &attendees);
    row.attendees = attendees;
    db.upsert_meeting(&row).map_err(|e| e.to_string())?;
    crate::storage::export_index(&db, &base).map_err(|e| e.to_string())?;
    crate::search_index::sync_meeting(&db, &state.search, &id);
    meeting_detail(&db, &base, row)
}

/// Save the user's own notes, plus where their stars now sit.
///
/// Unlike the summary and the transcript this document has no frontmatter
/// and no file; it is one column, kept verbatim. The stars come with it
/// because they anchor into it by textblock ordinal: editing the notes
/// moves the blocks under them, so a save that left `stars_json` alone
/// would leave every star pointing at whatever now occupies its old index,
/// and the drift would compound with each edit. The frontend re-derives the
/// ordinals from the live document and sends them here.
#[tauri::command]
pub async fn update_meeting_notes(
    state: State<'_, AppState>,
    id: String,
    markdown: String,
    stars: Vec<Star>,
) -> Result<MeetingDetail, AppError> {
    let config = state.config.lock().await.clone();
    let base = crate::storage::storage_base(&config.storage_dir);
    let db = state.db().await?;
    let row = require_row(&db, &id)?;

    db.set_notes(&id, &markdown).map_err(|e| e.to_string())?;
    let stars_json = serde_json::to_string(&stars).map_err(AppError::internal)?;
    db.set_stars(&id, &stars_json).map_err(|e| e.to_string())?;
    crate::search_index::sync_meeting(&db, &state.search, &id);
    meeting_detail(&db, &base, row)
}

#[tauri::command]
pub async fn update_meeting_transcript(
    state: State<'_, AppState>,
    id: String,
    markdown: String,
) -> Result<MeetingDetail, AppError> {
    let config = state.config.lock().await.clone();
    let base = crate::storage::storage_base(&config.storage_dir);
    let db = state.db().await?;
    let mut row = require_row(&db, &id)?;
    let record = row.to_record();

    let mut attendees = parse_attendees(&markdown);
    if attendees.is_empty() {
        attendees = row.attendees.clone();
    }
    row.transcript = canonicalize_frontmatter(&markdown, &record, &attendees);
    row.attendees = attendees;
    db.upsert_meeting(&row).map_err(|e| e.to_string())?;
    crate::storage::export_index(&db, &base).map_err(|e| e.to_string())?;
    crate::search_index::sync_meeting(&db, &state.search, &id);
    meeting_detail(&db, &base, row)
}

/// Delete several meetings at once (the list's multi-select). The index is
/// exported once at the end rather than per row, and a missing meeting is
/// not an error; it is already in the state the caller wanted.
#[tauri::command]
pub async fn delete_meetings(state: State<'_, AppState>, ids: Vec<String>) -> Result<(), AppError> {
    let config = state.config.lock().await.clone();
    let base = crate::storage::storage_base(&config.storage_dir);
    let db = state.db().await?;

    let mut linked: Vec<String> = Vec::new();
    for id in &ids {
        let Some(row) = db.get_meeting(id).map_err(|e| e.to_string())? else {
            continue;
        };
        collect_linked_speakers(&db, id, &mut linked)?;
        remove_indexed_file(&base, &row.audio_path)?;
        remove_meeting_assets(&base, id);
        db.delete_meeting(id).map_err(|e| e.to_string())?;
    }
    prune_released_speakers(&db, linked)?;
    crate::storage::export_index(&db, &base).map_err(|e| e.to_string())?;
    crate::search_index::after_delete(&db);
    Ok(())
}

/// The registry ids a meeting's segments link to, remembered before the
/// delete so profiles the meeting was the last home of can be pruned.
fn collect_linked_speakers(
    db: &embral_db::Db,
    meeting_id: &str,
    into: &mut Vec<String>,
) -> Result<(), AppError> {
    for seg in db.get_segments(meeting_id).map_err(|e| e.to_string())? {
        if let Some(id) = seg.speaker_id {
            if !into.contains(&id) {
                into.push(id);
            }
        }
    }
    Ok(())
}

/// Deleting a meeting orphans the people who only ever spoke in it; a
/// profile with no history left and no notes goes with the meeting.
fn prune_released_speakers(db: &embral_db::Db, linked: Vec<String>) -> Result<(), AppError> {
    let pruned = db
        .prune_orphaned_speakers(&linked)
        .map_err(|e| e.to_string())?;
    if !pruned.is_empty() {
        tracing::info!(count = pruned.len(), "pruned orphaned speaker profiles");
    }
    Ok(())
}

#[tauri::command]
pub async fn delete_meeting(state: State<'_, AppState>, id: String) -> Result<(), AppError> {
    let config = state.config.lock().await.clone();
    let base = crate::storage::storage_base(&config.storage_dir);
    let db = state.db().await?;
    let row = require_row(&db, &id)?;

    let mut linked: Vec<String> = Vec::new();
    collect_linked_speakers(&db, &id, &mut linked)?;
    remove_indexed_file(&base, &row.audio_path)?;
    remove_meeting_assets(&base, &id);
    db.delete_meeting(&id).map_err(|e| e.to_string())?;
    prune_released_speakers(&db, linked)?;
    crate::storage::export_index(&db, &base).map_err(|e| e.to_string())?;
    crate::search_index::after_delete(&db);
    Ok(())
}
