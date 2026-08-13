//! Storage roots, the database handle, and generated exports.
//!
//! Since R1 the SQLite database (`{storage_dir}/embral.db`) is the source of
//! truth. `index.json` is an *export* regenerated from it after every
//! mutation. The summary and transcript documents used to be exported as
//! markdown files too; since v11 they are columns only, and putting a
//! meeting on disk in readable form is the markdown export's job
//! ([integrations.md](../../docs/integrations.md)).

use anyhow::Result;
use chrono::Utc;
use embral_db::{Db, MeetingRow};
use embral_types::resolve_storage_path;
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub fn storage_base(storage_dir: &str) -> PathBuf {
    resolve_storage_path(storage_dir)
}

pub fn init_storage_dirs(base: &Path) -> Result<()> {
    std::fs::create_dir_all(base.join("audio"))?;
    // Images pasted into a meeting's documents, one directory per meeting
    // (`embral_notes::assets`).
    std::fs::create_dir_all(base.join("assets"))?;
    Ok(())
}

/// Let the webview load files out of the storage dir over `asset:`.
///
/// The static scope in `tauri.conf.json` allows `$HOME`, `$DOCUMENT` and
/// `$AUDIO`, but the storage dir is a free-form directory picker
/// (Settings → General), so anywhere else — a second drive, most obviously —
/// was outside it and every `convertFileSrc` URL 403'd. That is why audio
/// playback silently died for a library on `D:\`. Called at startup and again
/// whenever `storage_dir` changes; the scope is additive, so a moved library
/// leaves the old directory allowed until the next launch, which is
/// harmless — it is a read permission on the user's own former library.
pub fn allow_asset_access(app: &tauri::AppHandle, base: &Path) {
    use tauri::Manager;
    if let Err(e) = app.asset_protocol_scope().allow_directory(base, true) {
        tracing::warn!(
            "could not allow asset access to {}: {e} — audio and images may not load",
            base.display()
        );
    }
}

pub fn generate_meeting_id() -> String {
    let ts = Utc::now().format("%y%m%dT%H%M%S");
    let uid = &Uuid::new_v4().to_string()[..6];
    format!("{}_{}", ts, uid)
}

/// Open (or create) the database under `base`, importing a legacy
/// `index.json` library on first run against this directory.
pub fn open_db(base: &Path) -> Result<Db> {
    let db = Db::open(&base.join("embral.db"))?;
    if db.meeting_count()? == 0 {
        let imported = import_legacy_index(&db, base)?;
        if imported > 0 {
            tracing::info!(
                imported,
                "imported legacy index.json library into the database"
            );
        }
    }
    Ok(db)
}

/// One entry of a pre-R1 `index.json`, which is the only place the markdown
/// file paths still exist — the current export dropped them with v11, and
/// this shape is what makes the one-time migration able to find the files.
#[derive(serde::Deserialize)]
struct LegacyRecord {
    id: String,
    title: String,
    date: chrono::DateTime<Utc>,
    duration_seconds: u64,
    #[serde(default)]
    audio_path: String,
    #[serde(default)]
    notes_path: String,
    #[serde(default, alias = "raw_path")]
    transcript_path: String,
}

/// Pre-R1 index reader, kept for the one-time migration.
fn read_legacy_index(base: &Path) -> Result<Vec<LegacyRecord>> {
    let path = base.join("index.json");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(&path)?;
    Ok(serde_json::from_str(&text)?)
}

/// Build DB rows from a legacy index + its markdown files. Legacy meetings
/// have no structured segments; their transcript text still lands in
/// `transcript`, which the FTS index covers.
fn import_legacy_index(db: &Db, base: &Path) -> Result<usize> {
    let records = read_legacy_index(base)?;
    if records.is_empty() {
        return Ok(0);
    }
    let read_md = |rel: &str| -> String {
        if rel.trim().is_empty() {
            return String::new();
        }
        std::fs::read_to_string(base.join(rel)).unwrap_or_default()
    };
    let rows: Vec<MeetingRow> = records
        .iter()
        .map(|r| {
            let summary = read_md(&r.notes_path);
            let transcript = read_md(&r.transcript_path);
            let attendees = {
                let from_notes = crate::commands::parse_attendees(&summary);
                if from_notes.is_empty() {
                    crate::commands::parse_attendees(&transcript)
                } else {
                    from_notes
                }
            };
            MeetingRow {
                id: r.id.clone(),
                title: r.title.clone(),
                started_at: r.date,
                duration_seconds: r.duration_seconds,
                summary,
                transcript,
                attendees,
                audio_path: r.audio_path.clone(),
            }
        })
        .collect();
    db.import_legacy(&rows)
}

/// Delete audio files older than `days` (0 = disabled): the file is removed,
/// the row's `audio_path` cleared, and the index re-exported. Transcripts and
/// notes are never touched. Returns how many meetings were pruned.
pub fn prune_old_audio(db: &Db, base: &Path, days: u32) -> Result<usize> {
    if days == 0 {
        return Ok(0);
    }
    let cutoff = Utc::now() - chrono::Duration::days(i64::from(days));
    let mut pruned = 0usize;
    for mut row in db.list_meetings(None, None)? {
        if row.audio_path.trim().is_empty() || row.started_at >= cutoff {
            continue;
        }
        match crate::commands::resolve_indexed_path(base, &row.audio_path) {
            Ok(path) => {
                if path.is_file() {
                    if let Err(e) = std::fs::remove_file(&path) {
                        tracing::warn!("janitor: failed to delete {}: {e}", path.display());
                        continue;
                    }
                }
                row.audio_path = String::new();
                db.upsert_meeting(&row)?;
                pruned += 1;
            }
            Err(e) => tracing::warn!("janitor: skipping {}: {e}", row.id),
        }
    }
    if pruned > 0 {
        export_index(db, base)?;
    }
    Ok(pruned)
}

/// Delete whole meetings older than `days` (0 = disabled): the audio file
/// and the database row, which carries both documents (segments cascade).
/// Returns how many meetings were removed.
pub fn prune_old_meetings(db: &Db, base: &Path, days: u32) -> Result<usize> {
    if days == 0 {
        return Ok(0);
    }
    let cutoff = Utc::now() - chrono::Duration::days(i64::from(days));
    let mut pruned = 0usize;
    for row in db.list_meetings(None, None)? {
        if row.started_at >= cutoff {
            continue;
        }
        if !row.audio_path.trim().is_empty() {
            match crate::commands::resolve_indexed_path(base, &row.audio_path) {
                Ok(path) => {
                    if path.is_file() {
                        if let Err(e) = std::fs::remove_file(&path) {
                            tracing::warn!("janitor: failed to delete {}: {e}", path.display());
                        }
                    }
                }
                Err(e) => tracing::warn!("janitor: skipping file of {}: {e}", row.id),
            }
        }
        crate::commands::remove_meeting_assets(base, &row.id);
        db.delete_meeting(&row.id)?;
        pruned += 1;
    }
    if pruned > 0 {
        export_index(db, base)?;
    }
    Ok(pruned)
}

/// Delete asset directories whose meeting no longer exists — the residue of
/// a recording abandoned between the first paste and the row being written,
/// or of a save that failed after the images landed.
///
/// **The live recording's directory is skipped**, and that guard is the
/// whole subtlety: a recording in flight has images on disk and no row yet,
/// so a sweep that only asked the database would delete the user's
/// screenshots mid-meeting. A meeting with a recovery scratch still
/// pending is skipped for the same reason — its rescue has not run yet
/// (or is between attempts), and its images belong to the meeting the
/// rescue will commit.
pub fn prune_orphan_assets(db: &Db, base: &Path) -> Result<usize> {
    let dir = base.join("assets");
    if !dir.is_dir() {
        return Ok(0);
    }
    let live = crate::recovery::active_meeting_id(base);
    let waiting = crate::recovery::pending(base);
    let mut pruned = 0usize;
    for entry in std::fs::read_dir(&dir)?.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if live.as_deref() == Some(name.as_str()) || waiting.iter().any(|w| *w == name) {
            continue;
        }
        if db.get_meeting(&name)?.is_some() {
            continue;
        }
        match std::fs::remove_dir_all(entry.path()) {
            Ok(()) => {
                tracing::info!("janitor: removed orphaned assets for {name}");
                pruned += 1;
            }
            Err(e) => tracing::warn!("janitor: could not remove assets for {name}: {e}"),
        }
    }
    Ok(pruned)
}

/// Regenerate `index.json` from the database (newest first, same shape the
/// pre-R1 app wrote, so MCP servers keep working unchanged).
pub fn export_index(db: &Db, base: &Path) -> Result<()> {
    let records = db.export_records()?;
    std::fs::write(
        embral_types::index_path(base),
        serde_json::to_string_pretty(&records)?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use embral_types::MeetingRecord;

    /// Boot-path migration against the real demo library fixture: a pre-R1
    /// storage dir (index.json + markdown files) imports on first open, and
    /// the re-exported index round-trips.
    #[test]
    fn legacy_library_imports_on_first_open() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("docs")
            .join("prepop")
            .join("embral-demo");
        if !fixture.join("index.json").is_file() {
            // The demo library lives in docs/, which the public-repo filter
            // drops — the open-core tree skips this test (decode.rs idiom).
            eprintln!("demo fixture missing; skipping");
            return;
        }

        // Copy the fixture into a temp dir so the test never mutates it.
        let tmp = std::env::temp_dir().join(format!("embral-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        for sub in ["notes", "transcripts"] {
            std::fs::create_dir_all(tmp.join(sub)).unwrap();
            for entry in std::fs::read_dir(fixture.join(sub)).unwrap() {
                let entry = entry.unwrap();
                std::fs::copy(entry.path(), tmp.join(sub).join(entry.file_name())).unwrap();
            }
        }
        std::fs::copy(fixture.join("index.json"), tmp.join("index.json")).unwrap();

        let db = open_db(&tmp).expect("open + migrate");
        let n = db.meeting_count().unwrap();
        assert_eq!(n, 10, "all demo meetings imported");

        // Imported content carries the markdown bodies (search happens at
        // chunk level now — embral-search's own tests cover it).
        let rows = db.list_meetings(None, None).unwrap();
        assert!(rows.iter().all(|r| !r.summary.is_empty()));

        // The documents came across into the database; the index is now just
        // the meeting list, with audio the only path left on it.
        export_index(&db, &tmp).unwrap();
        let reread: Vec<MeetingRecord> =
            serde_json::from_str(&std::fs::read_to_string(tmp.join("index.json")).unwrap())
                .unwrap();
        assert_eq!(reread.len(), 10);
        assert!(reread.iter().all(|r| r.audio_path.starts_with("audio/")));

        // Second open must not double-import.
        drop(db);
        let db2 = open_db(&tmp).expect("re-open");
        assert_eq!(db2.meeting_count().unwrap(), 10);

        drop(db2);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn janitor_prunes_only_old_audio() {
        let tmp = std::env::temp_dir().join(format!("embral-janitor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("audio")).unwrap();

        let db = Db::open(&tmp.join("embral.db")).unwrap();
        let mk = |id: &str, days_ago: i64| {
            let audio_rel = format!("audio/{id}.mp3");
            std::fs::write(tmp.join(&audio_rel), b"mp3").unwrap();
            db.upsert_meeting(&MeetingRow {
                id: id.into(),
                title: id.into(),
                started_at: Utc::now() - chrono::Duration::days(days_ago),
                duration_seconds: 60,
                summary: String::new(),
                transcript: "t".into(),
                attendees: vec![],
                audio_path: audio_rel,
            })
            .unwrap();
        };
        mk("old", 40);
        mk("recent", 3);

        // Disabled (0 days) prunes nothing.
        assert_eq!(prune_old_audio(&db, &tmp, 0).unwrap(), 0);

        assert_eq!(prune_old_audio(&db, &tmp, 30).unwrap(), 1);
        assert!(!tmp.join("audio/old.mp3").exists());
        assert!(tmp.join("audio/recent.mp3").exists());
        assert_eq!(db.get_meeting("old").unwrap().unwrap().audio_path, "");
        assert_eq!(
            db.get_meeting("recent").unwrap().unwrap().audio_path,
            "audio/recent.mp3"
        );
        // Transcript markdown untouched.
        assert_eq!(db.get_meeting("old").unwrap().unwrap().transcript, "t");

        // Re-run is a no-op.
        assert_eq!(prune_old_audio(&db, &tmp, 30).unwrap(), 0);

        drop(db);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The orphan sweep deletes asset directories with no meeting behind
    /// them — except the recording happening right now, which has images on
    /// disk and no row yet. Getting that guard wrong deletes the user's
    /// screenshots mid-meeting.
    #[test]
    fn the_asset_sweep_spares_the_recording_in_flight() {
        let tmp = std::env::temp_dir().join(format!("embral-assets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        // The sweep asks the database one question — "is there a meeting
        // with this id" — and does its real work on the filesystem, so an
        // in-memory library keeps the test about the part that matters.
        let db = Db::open_in_memory().unwrap();

        let asset = |id: &str| {
            let dir = tmp.join("assets").join(id);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("img-01.png"), b"x").unwrap();
        };
        // One saved meeting, one abandoned, one recording right now.
        db.upsert_meeting(&MeetingRow {
            id: "saved".into(),
            title: "Saved".into(),
            started_at: Utc::now(),
            duration_seconds: 60,
            summary: String::new(),
            transcript: String::new(),
            attendees: vec![],
            audio_path: String::new(),
        })
        .unwrap();
        asset("saved");
        asset("abandoned");
        asset("recording-now");
        crate::recovery::begin(&tmp, "recording-now");

        assert_eq!(prune_orphan_assets(&db, &tmp).unwrap(), 1);
        assert!(tmp.join("assets/saved/img-01.png").exists());
        assert!(
            tmp.join("assets/recording-now/img-01.png").exists(),
            "the live recording's images must survive"
        );
        assert!(!tmp.join("assets/abandoned").exists());

        drop(db);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn janitor_prunes_whole_meetings() {
        let tmp = std::env::temp_dir().join(format!("embral-mjanitor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("audio")).unwrap();

        let db = Db::open(&tmp.join("embral.db")).unwrap();
        let mk = |id: &str, days_ago: i64| {
            std::fs::write(tmp.join(format!("audio/{id}.mp3")), b"x").unwrap();
            db.upsert_meeting(&MeetingRow {
                id: id.into(),
                title: id.into(),
                started_at: Utc::now() - chrono::Duration::days(days_ago),
                duration_seconds: 60,
                summary: "n".into(),
                transcript: "t".into(),
                attendees: vec![],
                audio_path: format!("audio/{id}.mp3"),
            })
            .unwrap();
        };
        mk("ancient", 400);
        mk("recent", 3);

        // Disabled (0 days) prunes nothing.
        assert_eq!(prune_old_meetings(&db, &tmp, 0).unwrap(), 0);

        assert_eq!(prune_old_meetings(&db, &tmp, 365).unwrap(), 1);
        // The row carries both documents, so deleting it takes them with it;
        // audio is the one file that has to be removed by hand.
        assert!(db.get_meeting("ancient").unwrap().is_none());
        assert!(!tmp.join("audio/ancient.mp3").exists());
        // The recent meeting is fully intact.
        assert!(db.get_meeting("recent").unwrap().is_some());
        assert!(tmp.join("audio/recent.mp3").exists());

        // Re-run is a no-op.
        assert_eq!(prune_old_meetings(&db, &tmp, 365).unwrap(), 0);

        drop(db);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
