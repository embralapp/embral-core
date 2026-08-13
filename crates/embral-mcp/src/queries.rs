//! The tool bodies, as plain functions over an open database — testable
//! without a transport. Search runs through embral-search's hybrid engine;
//! the caller supplies the query vector (or `None` — keyword-only, the
//! degrade path when the embedding model is absent).

use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use embral_db::rusqlite::OptionalExtension;
use embral_db::{Db, MeetingRow};
use embral_search::{Hit, Mode, OwnerKind, SearchArgs, Source};
use serde_json::{json, Value};

use crate::store::{Store, ToolError};

const LIST_LIMIT_MAX: u32 = 100;
const LIST_LIMIT_DEFAULT: u32 = 20;
const SEARCH_LIMIT_MAX: u32 = 25;
const SEARCH_LIMIT_DEFAULT: u32 = 8;

fn rfc3339(dt: &DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn summary(m: &MeetingRow) -> Value {
    json!({
        "id": m.id,
        "title": m.title,
        "started_at": rfc3339(&m.started_at),
        "duration_seconds": m.duration_seconds,
        "attendees": m.attendees,
    })
}

/// RFC3339 date-time, or a plain date (`before` dates read as end-of-day so
/// "before 2026-07-01" includes that whole day's meetings).
fn parse_time(s: &str, end_of_day: bool) -> Result<DateTime<Utc>, ToolError> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map(|d| {
            let (h, m, sec) = if end_of_day { (23, 59, 59) } else { (0, 0, 0) };
            d.and_hms_opt(h, m, sec).expect("valid time").and_utc()
        })
        .map_err(|_| ToolError::InvalidArgument {
            message: format!("'{s}' is not an RFC3339 date-time or a YYYY-MM-DD date"),
        })
}

/// Shared shape of the two search tools after schema-level parsing.
pub struct SearchParams {
    pub query: String,
    pub mode: Mode,
    pub sources: Option<Vec<Source>>,
    pub participants: Option<Vec<String>>,
    pub speakers: Option<Vec<String>>,
    pub after: Option<String>,
    pub before: Option<String>,
    pub limit: Option<u32>,
}

fn hit_json(h: &Hit) -> Value {
    let mut body = serde_json::Map::new();
    body.insert("passage_id".into(), json!(h.chunk_id));
    if let Some(id) = &h.meeting_id {
        body.insert("meeting_id".into(), json!(id));
        body.insert("title".into(), json!(h.title));
        body.insert("started_at".into(), json!(rfc3339(&h.date)));
    }
    if let Some(id) = h.dictation_id {
        body.insert("dictation_id".into(), json!(id));
        body.insert("created_at".into(), json!(rfc3339(&h.date)));
    }
    body.insert("source".into(), json!(h.source));
    // Which image an `image_text` passage was read out of, so a caller can
    // say *where* it saw something rather than only that it did.
    if let Some(image) = &h.image_filename {
        body.insert("image".into(), json!(image));
    }
    if !h.speakers.is_empty() {
        body.insert("speakers".into(), json!(h.speakers));
    }
    if let (Some(s), Some(e)) = (h.start_secs, h.end_secs) {
        body.insert("start_secs".into(), json!(s));
        body.insert("end_secs".into(), json!(e));
    }
    body.insert("text".into(), json!(h.text));
    let mut matched = serde_json::Map::new();
    if let Some(r) = h.fts_rank {
        matched.insert("keyword_rank".into(), json!(r));
    }
    if let Some(r) = h.semantic_rank {
        matched.insert("semantic_rank".into(), json!(r));
    }
    matched.insert("exact_phrase".into(), json!(h.exact_phrase));
    body.insert("match".into(), Value::Object(matched));
    Value::Object(body)
}

/// Does a requested name cover a profile's display name? True when its
/// words are a contiguous run of the profile's words, case-insensitively —
/// "john" and "john smith" both cover "John Smith Jr", "smith john" covers
/// nothing.
fn name_covers(requested: &str, profile_name: &str) -> bool {
    let req: Vec<String> = requested.split_whitespace().map(str::to_lowercase).collect();
    let prof: Vec<String> = profile_name.split_whitespace().map(str::to_lowercase).collect();
    !req.is_empty() && prof.windows(req.len()).any(|w| w == req.as_slice())
}

fn push_name(names: &mut Vec<String>, name: &str) {
    if !names.iter().any(|n| n.eq_ignore_ascii_case(name)) {
        names.push(name.to_string());
    }
}

/// Expand each requested speaker name through the registry so "what has
/// John said" finds what the labels call "John Smith". The requested name
/// itself always stays in the list — a label that never got a profile
/// keeps matching as typed — and an ambiguous first name includes every
/// profile it covers: search is recall-oriented and each hit carries the
/// full name.
fn resolve_speakers(db: &Db, requested: &[String]) -> Result<Vec<String>, ToolError> {
    let profiles = db.list_speakers().map_err(ToolError::Db)?;
    let mut names = Vec::new();
    for name in requested {
        push_name(&mut names, name);
        for p in profiles.iter().filter(|p| name_covers(name, &p.name)) {
            push_name(&mut names, &p.name);
        }
    }
    Ok(names)
}

fn run_search(
    db: &Db,
    owner: OwnerKind,
    params: &SearchParams,
    vector: Option<&[f32]>,
) -> Result<Vec<Hit>, ToolError> {
    let mut args = SearchArgs::new(&params.query, owner);
    args.mode = params.mode;
    args.limit = params
        .limit
        .unwrap_or(SEARCH_LIMIT_DEFAULT)
        .clamp(1, SEARCH_LIMIT_MAX) as usize;
    args.sources = params.sources.clone();
    args.participants = params.participants.clone();
    args.speakers = params
        .speakers
        .as_deref()
        .map(|names| resolve_speakers(db, names))
        .transpose()?;
    args.after = params.after.as_deref().map(|s| parse_time(s, false)).transpose()?;
    args.before = params.before.as_deref().map(|s| parse_time(s, true)).transpose()?;
    embral_search::search(db, &args, vector).map_err(ToolError::Db)
}

pub fn search_meetings(
    db: &Db,
    params: &SearchParams,
    vector: Option<&[f32]>,
) -> Result<Value, ToolError> {
    let hits = run_search(db, OwnerKind::Meetings, params, vector)?;
    Ok(json!({
        "query": params.query,
        "semantic": vector.is_some(),
        "count": hits.len(),
        "hits": hits.iter().map(hit_json).collect::<Vec<_>>(),
    }))
}

pub fn search_dictations(
    db: &Db,
    params: &SearchParams,
    vector: Option<&[f32]>,
) -> Result<Value, ToolError> {
    let hits = run_search(db, OwnerKind::Dictations, params, vector)?;
    Ok(json!({
        "query": params.query,
        "semantic": vector.is_some(),
        "count": hits.len(),
        "hits": hits.iter().map(hit_json).collect::<Vec<_>>(),
    }))
}

struct ChunkRow {
    meeting_id: Option<String>,
    dictation_id: Option<i64>,
    source: String,
    chunk_index: i64,
    start_secs: Option<f64>,
    end_secs: Option<f64>,
    text: String,
    image_filename: Option<String>,
}

fn load_chunk(db: &Db, passage_id: i64) -> Result<ChunkRow, ToolError> {
    db.with_conn(|conn| {
        Ok(conn
            .query_row(
                "SELECT meeting_id, dictation_id, source, chunk_index,
                        start_secs, end_secs, text, image_filename
                 FROM chunks WHERE id = ?1",
                [passage_id],
                |r| {
                    Ok(ChunkRow {
                        meeting_id: r.get(0)?,
                        dictation_id: r.get(1)?,
                        source: r.get(2)?,
                        chunk_index: r.get(3)?,
                        start_secs: r.get(4)?,
                        end_secs: r.get(5)?,
                        text: r.get(6)?,
                        image_filename: r.get(7)?,
                    })
                },
            )
            .optional()?)
    })
    .map_err(ToolError::Db)?
    .ok_or(ToolError::PassageNotFound { id: passage_id })
}

/// Grow a search hit: transcript passages expand into the surrounding
/// minutes rendered from segments; other sources return their neighboring
/// passages in the same document.
pub fn passage_context(
    db: &Db,
    passage_id: i64,
    before_secs: f64,
    after_secs: f64,
) -> Result<Value, ToolError> {
    let chunk = load_chunk(db, passage_id)?;

    if let (Some(meeting_id), Some(start), Some(end)) =
        (&chunk.meeting_id, chunk.start_secs, chunk.end_secs)
    {
        let segments = db.get_segments(meeting_id).map_err(ToolError::Db)?;
        if !segments.is_empty() {
            let lo = (start - before_secs.max(0.0)).max(0.0);
            let hi = end + after_secs.max(0.0);
            let window: Vec<_> = segments
                .into_iter()
                .filter(|s| s.end >= lo && s.start <= hi)
                .collect();
            return Ok(json!({
                "passage_id": passage_id,
                "meeting_id": meeting_id,
                "from_secs": lo,
                "to_secs": hi,
                "context": embral_notes::transcript::format_transcript(&window),
            }));
        }
    }

    // Prose documents (and segment-less transcripts): the neighbors are the
    // context.
    let (owner_sql, owner_param): (&str, embral_db::rusqlite::types::Value) =
        match (&chunk.meeting_id, chunk.dictation_id) {
            (Some(id), _) => ("meeting_id = ?1", id.clone().into()),
            (_, Some(id)) => ("dictation_id = ?1", id.into()),
            _ => return Err(ToolError::PassageNotFound { id: passage_id }),
        };
    let neighbor = |offset: i64| -> Result<Option<String>, ToolError> {
        db.with_conn(|conn| {
            Ok(conn
                .query_row(
                    &format!(
                        "SELECT text FROM chunks
                         WHERE {owner_sql} AND source = ?2 AND chunk_index = ?3"
                    ),
                    embral_db::rusqlite::params![
                        owner_param,
                        chunk.source,
                        chunk.chunk_index + offset
                    ],
                    |r| r.get::<_, String>(0),
                )
                .optional()?)
        })
        .map_err(ToolError::Db)
    };
    let mut out = json!({
        "passage_id": passage_id,
        "meeting_id": chunk.meeting_id,
        "dictation_id": chunk.dictation_id,
        "before": neighbor(-1)?,
        "passage": chunk.text,
        "after": neighbor(1)?,
    });
    // Which image an image_text passage was read out of — the handle
    // `get_meeting_image` takes, kept through expansion like the search
    // hit that led here.
    if let Some(image) = &chunk.image_filename {
        out["image"] = json!(image);
    }
    Ok(out)
}

fn require_meeting(db: &Db, id: &str) -> Result<MeetingRow, ToolError> {
    db.get_meeting(id)
        .map_err(ToolError::Db)?
        .ok_or_else(|| ToolError::MeetingNotFound { id: id.to_string() })
}

/// The full picture of one meeting: metadata, who attended vs who spoke,
/// the summary document, the user's own notes, and which pasted images
/// exist (fetchable through `get_meeting_image`).
pub fn get_meeting(db: &Db, storage_dir: &std::path::Path, id: &str) -> Result<Value, ToolError> {
    let m = require_meeting(db, id)?;
    let spoke: Vec<String> = db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT DISTINCT speaker FROM segments
                 WHERE meeting_id = ?1 AND speaker IS NOT NULL
                 ORDER BY speaker",
            )?;
            let names = stmt
                .query_map([id], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(names)
        })
        .map_err(ToolError::Db)?;
    let user_notes = db.get_notes(id).map_err(ToolError::Db)?;
    // The inventory is what's on disk; `has_text` says whether a usable
    // OCR reading is already indexed for it.
    let readings = db.image_text(id).map_err(ToolError::Db)?;
    let images: Vec<Value> = crate::images::list(storage_dir, id)
        .into_iter()
        .map(|filename| {
            let has_text = readings
                .iter()
                .any(|(name, text)| *name == filename && embral_notes::ocr::is_usable(text));
            json!({ "filename": filename, "has_text": has_text })
        })
        .collect();
    Ok(json!({
        "meeting": summary(&m),
        "speakers": spoke,
        "summary": m.summary,
        "user_notes": user_notes,
        "has_transcript": !m.transcript.trim().is_empty(),
        "images": images,
    }))
}

/// The full transcript document, or a segment-rendered window of it.
pub fn get_transcript(
    db: &Db,
    id: &str,
    from_secs: Option<f64>,
    to_secs: Option<f64>,
) -> Result<Value, ToolError> {
    let m = require_meeting(db, id)?;
    if from_secs.is_none() && to_secs.is_none() {
        return Ok(json!({
            "meeting": summary(&m),
            "transcript": m.transcript,
        }));
    }
    let lo = from_secs.unwrap_or(0.0).max(0.0);
    let hi = to_secs.unwrap_or(f64::MAX);
    if hi < lo {
        return Err(ToolError::InvalidArgument {
            message: "to_secs must be at or after from_secs".into(),
        });
    }
    let window: Vec<_> = db
        .get_segments(id)
        .map_err(ToolError::Db)?
        .into_iter()
        .filter(|s| s.end >= lo && s.start <= hi)
        .collect();
    Ok(json!({
        "meeting": summary(&m),
        "from_secs": lo,
        "to_secs": if hi == f64::MAX { Value::Null } else { json!(hi) },
        "transcript": embral_notes::transcript::format_transcript(&window),
    }))
}

pub fn list_meetings(
    db: &Db,
    limit: Option<u32>,
    since: Option<&str>,
    participant: Option<&str>,
) -> Result<Value, ToolError> {
    let limit = limit.unwrap_or(LIST_LIMIT_DEFAULT).clamp(1, LIST_LIMIT_MAX);
    let since = since.map(|s| parse_time(s, false)).transpose()?;
    let mut meetings = db.list_meetings(Some(limit), since).map_err(ToolError::Db)?;
    if let Some(person) = participant {
        let needle = person.to_lowercase();
        meetings.retain(|m| m.attendees.iter().any(|a| a.to_lowercase() == needle));
    }
    Ok(json!({
        "count": meetings.len(),
        "meetings": meetings.iter().map(summary).collect::<Vec<_>>(),
    }))
}

/// Never errors on a missing library — reporting that state is its job.
pub fn storage_status(store: &Store, embedder_loaded: bool) -> Result<Value, ToolError> {
    let db_path = store.db_path();
    if !db_path.is_file() {
        return Ok(json!({
            "storage_dir": store.storage_dir.display().to_string(),
            "db_path": db_path.display().to_string(),
            "db_exists": false,
            "meeting_count": 0,
        }));
    }
    let db = Db::open_read_only(&db_path).map_err(ToolError::Db)?;
    let (chunk_count, pending_count): (i64, i64) = db
        .with_conn(|conn| {
            Ok((
                conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))?,
                conn.query_row(
                    "SELECT COUNT(*) FROM chunks WHERE embedded_with IS NULL",
                    [],
                    |r| r.get(0),
                )?,
            ))
        })
        .map_err(ToolError::Db)?;
    Ok(json!({
        "storage_dir": store.storage_dir.display().to_string(),
        "db_path": db_path.display().to_string(),
        "db_exists": true,
        "schema_version": db.schema_version().map_err(ToolError::Db)?,
        "server_schema_version": embral_db::latest_schema_version(),
        "meeting_count": db.meeting_count().map_err(ToolError::Db)?,
        "index": {
            "chunk_count": chunk_count,
            "pending_count": pending_count,
            "embedding_model": embral_search::model::MODEL_ID,
            "embedding_model_present": embral_search::model::present(),
            "embedder_loaded": embedder_loaded,
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use embral_types::TranscriptionSegment;

    fn seg(speaker: &str, text: &str, start: f64, end: f64) -> TranscriptionSegment {
        TranscriptionSegment {
            speaker: Some(speaker.to_string()),
            speaker_id: Some(format!("id-{speaker}")),
            text: text.to_string(),
            start,
            end,
        }
    }

    fn mk(id: &str, title: &str, day: u32, attendees: &[&str]) -> MeetingRow {
        MeetingRow {
            id: id.to_string(),
            title: title.to_string(),
            started_at: Utc.with_ymd_and_hms(2026, 6, day, 10, 0, 0).unwrap(),
            duration_seconds: 600,
            summary: String::new(),
            transcript: String::new(),
            attendees: attendees.iter().map(|s| s.to_string()).collect(),
            audio_path: String::new(),
        }
    }

    /// A seeded library on disk plus a read-only handle — the production
    /// arrangement in miniature.
    fn fixture(dir: &std::path::Path) -> Db {
        let path = dir.join("embral.db");
        {
            let writer = Db::open(&path).unwrap();
            let mut budget = mk("m-budget", "Budget Review", 1, &["Alice", "Bob"]);
            budget.summary = "# Budget Review\n\n## Key Takeaways\n\nSpending freeze until Q4.".into();
            budget.transcript = "# Budget Review Transcript\n\nbody".into();
            writer.upsert_meeting(&budget).unwrap();
            writer
                .replace_segments(
                    "m-budget",
                    &[
                        seg("Alice", "The quarterly budget needs a spending freeze.", 0.0, 10.0),
                        seg("Bob", "Marketing takes the largest cut this quarter.", 30.0, 40.0),
                        seg("Alice", "Agreed, we will revisit in October.", 120.0, 130.0),
                    ],
                )
                .unwrap();
            writer.set_notes("m-budget", "freeze confirmed by finance").unwrap();

            writer
                .upsert_meeting(&mk("m-hiring", "Hiring Sync", 20, &["Alice", "Dana"]))
                .unwrap();
            writer
                .replace_segments(
                    "m-hiring",
                    &[seg("Dana Smith", "Two engineering offers go out this week.", 0.0, 8.0)],
                )
                .unwrap();
            // Dana has a registry profile; Alice and Bob are plain labels.
            writer
                .upsert_speaker(&embral_db::SpeakerRow {
                    id: "sp_dana".into(),
                    name: "Dana Smith".into(),
                    notes: String::new(),
                })
                .unwrap();

            writer
                .add_dictation("remind me to send the budget spreadsheet", None, None)
                .unwrap();

            embral_search::sync_meeting(&writer, "m-budget").unwrap();
            embral_search::sync_meeting(&writer, "m-hiring").unwrap();
            embral_search::sync_dictation(&writer, 1).unwrap();
        }
        Db::open_read_only(&path).unwrap()
    }

    fn params(query: &str) -> SearchParams {
        SearchParams {
            query: query.to_string(),
            mode: Mode::Auto,
            sources: None,
            participants: None,
            speakers: None,
            after: None,
            before: None,
            limit: None,
        }
    }

    #[test]
    fn the_two_search_tools_stay_in_their_corpora() {
        let dir = tempfile::tempdir().unwrap();
        let db = fixture(dir.path());

        let meetings = search_meetings(&db, &params("budget"), None).unwrap();
        assert!(meetings["count"].as_u64().unwrap() >= 1);
        assert!(meetings["hits"][0]["meeting_id"].is_string());
        assert_eq!(meetings["semantic"], false);
        for hit in meetings["hits"].as_array().unwrap() {
            assert!(hit["dictation_id"].is_null());
            assert!(hit["passage_id"].is_i64());
        }

        let dictations = search_dictations(&db, &params("budget"), None).unwrap();
        assert_eq!(dictations["count"], 1);
        assert!(dictations["hits"][0]["dictation_id"].is_i64());
    }

    #[test]
    fn participant_and_speaker_filters_differ() {
        let dir = tempfile::tempdir().unwrap();
        let db = fixture(dir.path());

        // Alice attended the hiring sync but never spoke in it.
        let mut by_participant = params("offers");
        by_participant.participants = Some(vec!["alice".into()]);
        assert_eq!(search_meetings(&db, &by_participant, None).unwrap()["count"], 1);

        let mut by_speaker = params("offers");
        by_speaker.speakers = Some(vec!["Alice".into()]);
        assert_eq!(search_meetings(&db, &by_speaker, None).unwrap()["count"], 0);
    }

    #[test]
    fn speaker_filter_resolves_names_through_the_registry() {
        let dir = tempfile::tempdir().unwrap();
        let db = fixture(dir.path());

        // "what has Dana said": the labels say "Dana Smith", the caller
        // says "dana" — the registry bridges them.
        let mut by_first = params("offers");
        by_first.speakers = Some(vec!["dana".into()]);
        assert_eq!(search_meetings(&db, &by_first, None).unwrap()["count"], 1);

        // A label with no profile still matches as typed.
        let mut unregistered = params("freeze");
        unregistered.speakers = Some(vec!["alice".into()]);
        let found = search_meetings(&db, &unregistered, None).unwrap();
        assert!(found["count"].as_u64().unwrap() >= 1);
    }

    #[test]
    fn name_covers_matches_contiguous_words() {
        assert!(name_covers("john", "John Smith"));
        assert!(name_covers("JOHN SMITH", "John Smith"));
        assert!(name_covers("smith jr", "John Smith Jr"));
        assert!(!name_covers("smith john", "John Smith"));
        assert!(!name_covers("jo", "John Smith"));
        assert!(!name_covers("", "John Smith"));
    }

    #[test]
    fn date_arguments_parse_or_reject() {
        let dir = tempfile::tempdir().unwrap();
        let db = fixture(dir.path());

        let mut args = params("offers");
        args.after = Some("2026-06-10".into());
        assert_eq!(search_meetings(&db, &args, None).unwrap()["count"], 1);

        // `before` a date includes that day (end-of-day semantics).
        let mut args = params("budget");
        args.before = Some("2026-06-01".into());
        assert!(search_meetings(&db, &args, None).unwrap()["count"].as_u64().unwrap() >= 1);

        let mut args = params("budget");
        args.after = Some("last tuesday".into());
        assert!(matches!(
            search_meetings(&db, &args, None),
            Err(ToolError::InvalidArgument { .. })
        ));
    }

    #[test]
    fn passage_context_expands_transcript_hits_by_time() {
        let dir = tempfile::tempdir().unwrap();
        let db = fixture(dir.path());

        // Find the chunk holding the middle utterance.
        let hits = search_meetings(&db, &params("marketing"), None).unwrap();
        let passage_id = hits["hits"][0]["passage_id"].as_i64().unwrap();

        let ctx = passage_context(&db, passage_id, 60.0, 0.0).unwrap();
        let text = ctx["context"].as_str().unwrap();
        assert!(text.contains("spending freeze"), "context was: {text}");

        assert!(matches!(
            passage_context(&db, 999_999, 60.0, 60.0),
            Err(ToolError::PassageNotFound { .. })
        ));
    }

    #[test]
    fn prose_passages_expand_to_neighbors() {
        let dir = tempfile::tempdir().unwrap();
        let db = fixture(dir.path());

        let mut args = params("freeze confirmed");
        args.sources = Some(vec![Source::UserNotes]);
        let hits = search_meetings(&db, &args, None).unwrap();
        let passage_id = hits["hits"][0]["passage_id"].as_i64().unwrap();

        let ctx = passage_context(&db, passage_id, 60.0, 60.0).unwrap();
        assert_eq!(ctx["passage"], "freeze confirmed by finance");
        assert!(ctx["before"].is_null()); // a one-chunk document has no neighbors
        assert!(ctx["after"].is_null());
    }

    #[test]
    fn get_meeting_separates_attended_from_spoke() {
        let dir = tempfile::tempdir().unwrap();
        let db = fixture(dir.path());

        let detail = get_meeting(&db, dir.path(), "m-hiring").unwrap();
        assert_eq!(detail["meeting"]["attendees"], json!(["Alice", "Dana"]));
        assert_eq!(detail["speakers"], json!(["Dana Smith"]));
        assert_eq!(detail["user_notes"], "");
        assert_eq!(detail["images"], json!([]));

        assert!(matches!(
            get_meeting(&db, dir.path(), "nope"),
            Err(ToolError::MeetingNotFound { .. })
        ));
    }

    #[test]
    fn get_meeting_inventories_images_with_their_text_state() {
        let dir = tempfile::tempdir().unwrap();
        let db = fixture(dir.path());
        std::fs::create_dir_all(dir.path().join("assets/m-budget")).unwrap();
        std::fs::write(dir.path().join("assets/m-budget/img-01.png"), b"x").unwrap();
        std::fs::write(dir.path().join("assets/m-budget/img-02.png"), b"x").unwrap();
        // One usable reading, one unread — written by a writer handle; the
        // fixture's is read-only, like production's.
        {
            let writer = Db::open(&dir.path().join("embral.db")).unwrap();
            writer
                .set_image_text("m-budget", "img-01.png", "Q3 revenue up twelve percent", "windows")
                .unwrap();
        }

        let detail = get_meeting(&db, dir.path(), "m-budget").unwrap();
        assert_eq!(
            detail["images"],
            json!([
                { "filename": "img-01.png", "has_text": true },
                { "filename": "img-02.png", "has_text": false }
            ])
        );
    }

    #[test]
    fn image_passages_keep_their_image_through_expansion() {
        let dir = tempfile::tempdir().unwrap();
        let db = fixture(dir.path());
        {
            let writer = Db::open(&dir.path().join("embral.db")).unwrap();
            // Image text is indexed for images the documents reference.
            writer
                .set_notes(
                    "m-budget",
                    "freeze confirmed by finance\n\n![shot](assets/m-budget/img-01.png)",
                )
                .unwrap();
            writer
                .set_image_text(
                    "m-budget",
                    "img-01.png",
                    "Spending freeze until Q4 on one slide",
                    "windows",
                )
                .unwrap();
            embral_search::sync_meeting(&writer, "m-budget").unwrap();
        }

        let (id, image): (i64, Option<String>) = db
            .with_conn(|conn| {
                Ok(conn.query_row(
                    "SELECT id, image_filename FROM chunks WHERE source = 'image_text' LIMIT 1",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?)
            })
            .unwrap();
        assert_eq!(image.as_deref(), Some("img-01.png"));

        let ctx = passage_context(&db, id, 0.0, 0.0).unwrap();
        assert_eq!(ctx["image"], "img-01.png");
        // A notes passage carries none.
        let (notes_id,): (i64,) = db
            .with_conn(|conn| {
                Ok(conn.query_row(
                    "SELECT id FROM chunks WHERE source = 'user_notes' LIMIT 1",
                    [],
                    |r| Ok((r.get(0)?,)),
                )?)
            })
            .unwrap();
        let notes_ctx = passage_context(&db, notes_id, 0.0, 0.0).unwrap();
        assert!(notes_ctx.get("image").is_none());
    }

    #[test]
    fn transcript_windows_render_from_segments() {
        let dir = tempfile::tempdir().unwrap();
        let db = fixture(dir.path());

        let whole = get_transcript(&db, "m-budget", None, None).unwrap();
        assert!(whole["transcript"].as_str().unwrap().contains("body"));

        let window = get_transcript(&db, "m-budget", Some(25.0), Some(50.0)).unwrap();
        let text = window["transcript"].as_str().unwrap();
        assert!(text.contains("Marketing"));
        assert!(!text.contains("October"));

        assert!(matches!(
            get_transcript(&db, "m-budget", Some(50.0), Some(10.0)),
            Err(ToolError::InvalidArgument { .. })
        ));
    }

    #[test]
    fn listing_filters_by_participant() {
        let dir = tempfile::tempdir().unwrap();
        let db = fixture(dir.path());

        let all = list_meetings(&db, None, None, None).unwrap();
        assert_eq!(all["count"], 2);
        let danas = list_meetings(&db, None, None, Some("dana")).unwrap();
        assert_eq!(danas["count"], 1);
        assert_eq!(danas["meetings"][0]["id"], "m-hiring");
    }

    #[test]
    fn storage_status_reports_the_index() {
        let dir = tempfile::tempdir().unwrap();
        let db = fixture(dir.path());
        drop(db);

        let store = Store { storage_dir: dir.path().to_path_buf() };
        let status = storage_status(&store, false).unwrap();
        assert_eq!(status["db_exists"], true);
        assert_eq!(status["meeting_count"], 2);
        assert!(status["index"]["chunk_count"].as_i64().unwrap() >= 3);
        assert!(status["index"]["pending_count"].as_i64().unwrap() >= 3);
        assert_eq!(status["index"]["embedder_loaded"], false);
    }
}
