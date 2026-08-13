//! SQLite storage layer for Embral.
//!
//! The database (`{storage_dir}/embral.db`) is the **source of truth** for
//! meetings, their two documents, and their structured transcript segments.
//! `index.json` remains on disk as a generated export written after each
//! mutation, and the markdown export writes meetings into a vault one-way
//! ([integrations.md](../../../docs/integrations.md)); every read inside the
//! app comes from here.
//!
//! A meeting has two documents and they are easy to confuse: `summary` is
//! what the LLM wrote, `notes` is what the user typed. Before v12 the first
//! was called `notes_md` and the second `user_notes`, so "notes" meant
//! opposite things on either side of the command layer.
//!
//! Concurrency: a single `Connection` behind a `Mutex`. All operations are
//! short (row-level CRUD and FTS queries); callers on the async runtime treat
//! them as cheap blocking calls, matching how config/file IO is already
//! handled in the Tauri commands.

use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use embral_types::{MeetingRecord, TranscriptionSegment};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};

mod schema;

/// The schema version this build of the crate writes; readers compare it
/// against [`Db::schema_version`] to detect a library from a different
/// embral version.
pub use schema::latest_version as latest_schema_version;

/// Re-exported so sibling crates (embral-search) run SQL through
/// [`Db::with_conn`] against the same rusqlite: one version, one
/// `Connection` type across the workspace.
pub use rusqlite;

/// Register sqlite-vec as an auto-extension, once per process: every
/// connection opened afterwards (the app's writer, the MCP server's
/// per-call read-only opens, in-memory test DBs) has the `vec0` module.
/// This must run before any `Connection::open`, which is why every open
/// path below calls it first.
fn register_vec_extension() {
    static VEC_INIT: std::sync::Once = std::sync::Once::new();
    VEC_INIT.call_once(|| unsafe {
        // sqlite-vec declares the init fn against its own libsqlite3-sys
        // types; the transmute re-types it for rusqlite's bundled ffi (same
        // C ABI either way).
        let init: unsafe extern "C" fn(
            *mut rusqlite::ffi::sqlite3,
            *mut *mut std::os::raw::c_char,
            *const rusqlite::ffi::sqlite3_api_routines,
        ) -> std::os::raw::c_int =
            std::mem::transmute(sqlite_vec::sqlite3_vec_init as *const ());
        rusqlite::ffi::sqlite3_auto_extension(Some(init));
    });
}

/// One meeting, as stored. Field set mirrors what the commands layer needs
/// to render `MeetingDetail` and regenerate the index export. The user's own
/// notes are not here: they are read and written on their own
/// (`get_notes` / `set_notes`), so an `upsert_meeting` can never clobber
/// them with a stale copy.
#[derive(Debug, Clone, PartialEq)]
pub struct MeetingRow {
    pub id: String,
    pub title: String,
    pub started_at: DateTime<Utc>,
    pub duration_seconds: u64,
    pub summary: String,
    pub transcript: String,
    pub attendees: Vec<String>,
    /// Storage-relative path to the meeting's audio. The one file a meeting
    /// still owns: the summary and transcript documents are columns, not
    /// files (v11).
    pub audio_path: String,
}

/// Where a page of the meeting list stopped: the last row it handed back.
/// The next page asks for the rows strictly older than this one.
///
/// A row offset would be the other way to page, and it is the wrong one
/// here: the list is newest-first and rows come and go under it (a delete, a
/// finished recording, the janitor), and any of those shifts every later row
/// by one, so an offset silently skips a meeting or shows the same one
/// twice. A cursor names a place in the order instead of counting from the
/// top, so it survives all of that.
#[derive(Debug, Clone, PartialEq)]
pub struct MeetingCursor {
    pub started_at: DateTime<Utc>,
    pub id: String,
}

impl MeetingRow {
    /// Where a page ending on this row continues from.
    pub fn cursor(&self) -> MeetingCursor {
        MeetingCursor {
            started_at: self.started_at,
            id: self.id.clone(),
        }
    }

    /// The index.json record for this row.
    pub fn to_record(&self) -> MeetingRecord {
        MeetingRecord {
            id: self.id.clone(),
            title: self.title.clone(),
            date: self.started_at,
            duration_seconds: self.duration_seconds,
            chunks: 1,
            audio_path: self.audio_path.clone(),
        }
    }
}

/// One person in the speaker registry. This is the write shape; the profiles
/// list reads [`SpeakerListRow`], which adds the activity the list orders by.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SpeakerRow {
    pub id: String,
    pub name: String,
    pub notes: String,
}

/// A registry person plus when they were last in a meeting: what the profiles
/// list sorts and groups by, so its date headers mean "people you met with
/// today" rather than "people you happened to add today".
#[derive(Debug, Clone, PartialEq)]
pub struct SpeakerListRow {
    pub speaker: SpeakerRow,
    pub created_at: DateTime<Utc>,
    /// The newest meeting this person is linked to; `None` if they have never
    /// been seen in one (a profile created by hand).
    pub last_seen: Option<DateTime<Utc>>,
}

/// One meeting a person spoke in: a row of the profile pane's record.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeakerMeetingRow {
    pub meeting_id: String,
    pub title: String,
    pub started_at: DateTime<Utc>,
    /// How many of the meeting's segments are theirs.
    pub segment_count: i64,
}

/// One saved dictation.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct DictationRow {
    pub id: i64,
    pub raw_text: String,
    pub cleaned_text: Option<String>,
    /// Process name of the app that had focus when dictation finished.
    pub app: Option<String>,
    /// RFC3339 UTC.
    pub created_at: String,
}

/// f32 slice → little-endian bytes for BLOB storage: the wire format
/// sqlite-vec's `vec0` tables take (embral-search's vector store).
pub fn embedding_to_blob(embedding: &[f32]) -> Vec<u8> {
    embedding.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// BLOB bytes → f32 vec (inverse of [`embedding_to_blob`]).
pub fn blob_to_embedding(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}

pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    /// Open (creating and migrating as needed) the database at `path`.
    pub fn open(path: &Path) -> Result<Db> {
        register_vec_extension();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create db dir {}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("open sqlite db at {}", path.display()))?;
        Self::init(conn)
    }

    /// In-memory database (tests only).
    pub fn open_in_memory() -> Result<Db> {
        register_vec_extension();
        Self::init(Connection::open_in_memory()?)
    }

    /// Open an existing database read-only, for sibling processes (the MCP
    /// server) reading while the app owns all writes. Never creates a file,
    /// never migrates, never changes the journal mode; a missing database is
    /// an error the caller renders friendly.
    pub fn open_read_only(path: &Path) -> Result<Db> {
        register_vec_extension();
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .with_context(|| format!("open sqlite db read-only at {}", path.display()))?;
        // WAL readers only wait during checkpoints; don't fail on the first
        // SQLITE_BUSY, don't hang a tool call forever either.
        conn.busy_timeout(std::time::Duration::from_millis(2_000))?;
        // Redundant with the open flag, but the intent survives refactors.
        conn.pragma_update(None, "query_only", "ON")?;
        Ok(Db {
            conn: Mutex::new(conn),
        })
    }

    /// The applied `meta.schema_version`; 0 when the meta table is absent
    /// (a pre-schema or foreign file).
    pub fn schema_version(&self) -> Result<i64> {
        let version = self
            .lock()
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .unwrap_or(None)
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        Ok(version)
    }

    fn init(conn: Connection) -> Result<Db> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        schema::migrate(&conn)?;
        Ok(Db {
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        // A poisoned mutex means a prior panic mid-statement; propagating the
        // panic is the correct response for a storage layer.
        self.conn.lock().expect("db mutex poisoned")
    }

    /// Run `f` against the raw connection, holding the lock for the
    /// closure's duration. This is embral-search's entry point: the retrieval
    /// engine owns its own SQL (chunks, FTS, vectors) rather than mirroring
    /// CRUD into this crate, but the connection and its lock stay here.
    pub fn with_conn<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        f(&self.lock())
    }

    // --- Meetings ---

    /// Insert or fully update a meeting row. `created_at` is set on first
    /// insert; `updated_at` on every call.
    pub fn upsert_meeting(&self, m: &MeetingRow) -> Result<()> {
        let now = rfc3339(&Utc::now());
        self.lock().execute(
            "INSERT INTO meetings (id, title, started_at, duration_seconds, summary,
                                   transcript, attendees, audio_path,
                                   created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)
             ON CONFLICT(id) DO UPDATE SET
               title = excluded.title,
               started_at = excluded.started_at,
               duration_seconds = excluded.duration_seconds,
               summary = excluded.summary,
               transcript = excluded.transcript,
               attendees = excluded.attendees,
               audio_path = excluded.audio_path,
               updated_at = excluded.updated_at",
            params![
                m.id,
                m.title,
                rfc3339(&m.started_at),
                m.duration_seconds as i64,
                m.summary,
                m.transcript,
                serde_json::to_string(&m.attendees)?,
                m.audio_path,
                now,
            ],
        )?;
        Ok(())
    }

    pub fn get_meeting(&self, id: &str) -> Result<Option<MeetingRow>> {
        self.lock()
            .query_row(
                &format!("SELECT {MEETING_COLS} FROM meetings WHERE id = ?1"),
                params![id],
                row_to_meeting,
            )
            .optional()
            .map_err(Into::into)
    }

    /// Meetings ordered newest-first, optionally filtered/limited.
    pub fn list_meetings(
        &self,
        limit: Option<u32>,
        since: Option<DateTime<Utc>>,
    ) -> Result<Vec<MeetingRow>> {
        self.list_meetings_page(limit, since, None)
    }

    /// One page of [`list_meetings`], picking up where `before` left off.
    /// The whole list is this call with no cursor, so both read the same
    /// order and a page boundary lands where the previous page ended.
    pub fn list_meetings_page(
        &self,
        limit: Option<u32>,
        since: Option<DateTime<Utc>>,
        before: Option<&MeetingCursor>,
    ) -> Result<Vec<MeetingRow>> {
        let conn = self.lock();
        let since_str = since.map(|dt| rfc3339(&dt));
        let before_at = before.map(|c| rfc3339(&c.started_at));
        let before_id = before.map(|c| c.id.as_str());
        // RFC3339 UTC strings sort lexicographically in time order.
        //
        // The id breaks ties on the second, which a bulk import produces
        // easily. Without it the order is not total, so two meetings sharing
        // a start time could fall either side of a page boundary and be
        // dropped or repeated; with it the cursor names exactly one row.
        let mut stmt = conn.prepare(&format!(
            "SELECT {MEETING_COLS} FROM meetings
             WHERE (?1 IS NULL OR started_at >= ?1)
               AND (?2 IS NULL
                    OR started_at < ?2
                    OR (started_at = ?2 AND id < ?3))
             ORDER BY started_at DESC, id DESC
             LIMIT ?4"
        ))?;
        let rows = stmt.query_map(
            params![
                since_str,
                before_at,
                before_id,
                limit.map(i64::from).unwrap_or(-1)
            ],
            row_to_meeting,
        )?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    pub fn delete_meeting(&self, id: &str) -> Result<bool> {
        // segments cascade via FK; FTS rows via trigger.
        let n = self
            .lock()
            .execute("DELETE FROM meetings WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    pub fn meeting_count(&self) -> Result<u64> {
        let n: i64 = self
            .lock()
            .query_row("SELECT COUNT(*) FROM meetings", [], |r| r.get(0))?;
        Ok(n as u64)
    }

    /// All meetings as index.json-compatible records, newest first.
    pub fn export_records(&self) -> Result<Vec<MeetingRecord>> {
        Ok(self
            .list_meetings(None, None)?
            .iter()
            .map(MeetingRow::to_record)
            .collect())
    }

    /// Bulk-import legacy rows (from index.json + markdown files) in one
    /// transaction. Existing ids are overwritten.
    pub fn import_legacy(&self, rows: &[MeetingRow]) -> Result<usize> {
        {
            let conn = self.lock();
            conn.execute_batch("BEGIN")?;
        }
        let result: Result<()> = rows.iter().try_for_each(|m| self.upsert_meeting(m));
        let conn = self.lock();
        match result {
            Ok(()) => {
                conn.execute_batch("COMMIT")?;
                Ok(rows.len())
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }

    // --- Segments ---

    /// Replace the structured transcript for a meeting.
    pub fn replace_segments(&self, meeting_id: &str, segments: &[TranscriptionSegment]) -> Result<()> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM segments WHERE meeting_id = ?1",
            params![meeting_id],
        )?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO segments (meeting_id, idx, speaker, speaker_id, text, start_secs, end_secs)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for (idx, s) in segments.iter().enumerate() {
                stmt.execute(params![
                    meeting_id,
                    idx as i64,
                    s.speaker,
                    s.speaker_id,
                    s.text,
                    s.start,
                    s.end
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn get_segments(&self, meeting_id: &str) -> Result<Vec<TranscriptionSegment>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT speaker, speaker_id, text, start_secs, end_secs FROM segments
             WHERE meeting_id = ?1 ORDER BY idx",
        )?;
        let rows = stmt.query_map(params![meeting_id], |row| {
            Ok(TranscriptionSegment {
                speaker: row.get(0)?,
                speaker_id: row.get(1)?,
                text: row.get(2)?,
                start: row.get(3)?,
                end: row.get(4)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    // --- Speakers ---

    /// Insert or update a registry speaker.
    pub fn upsert_speaker(&self, s: &SpeakerRow) -> Result<()> {
        let now = rfc3339(&Utc::now());
        self.lock().execute(
            "INSERT INTO speakers (id, name, notes, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?4)
             ON CONFLICT(id) DO UPDATE SET
               name = excluded.name,
               notes = excluded.notes,
               updated_at = excluded.updated_at",
            params![s.id, s.name, s.notes, now],
        )?;
        Ok(())
    }

    pub fn get_speaker(&self, id: &str) -> Result<Option<SpeakerRow>> {
        self.lock()
            .query_row(
                "SELECT id, name, notes FROM speakers WHERE id = ?1",
                params![id],
                row_to_speaker,
            )
            .optional()
            .map_err(Into::into)
    }

    /// All registry speakers, by name.
    pub fn list_speakers(&self) -> Result<Vec<SpeakerRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, name, notes FROM speakers
             ORDER BY name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], row_to_speaker)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    /// Registry speakers with their last meeting, newest first: the profiles
    /// list's order. Someone never seen in a meeting sorts by when they were
    /// created, so a just-added profile still appears at the top where the
    /// user is looking.
    pub fn list_speakers_by_activity(&self) -> Result<Vec<SpeakerListRow>> {
        self.speakers_by_activity(None)
    }

    /// The same view of one person, for the commands that return a profile
    /// after changing it.
    pub fn speaker_by_activity(&self, id: &str) -> Result<Option<SpeakerListRow>> {
        Ok(self.speakers_by_activity(Some(id))?.pop())
    }

    fn speakers_by_activity(&self, only: Option<&str>) -> Result<Vec<SpeakerListRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT s.id, s.name, s.notes, s.created_at,
                    (SELECT MAX(m.started_at)
                       FROM segments seg
                       JOIN meetings m ON m.id = seg.meeting_id
                      WHERE seg.speaker_id = s.id) AS last_seen
               FROM speakers s
              WHERE ?1 IS NULL OR s.id = ?1
              ORDER BY COALESCE(last_seen, s.created_at) DESC, s.name COLLATE NOCASE",
        )?;
        // Timestamps come back as strings and are parsed outside the
        // closure: rusqlite's row mapper can only fail with a rusqlite error.
        let rows = stmt.query_map(params![only], |row| {
            Ok((
                SpeakerRow {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    notes: row.get(2)?,
                },
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })?;

        let mut out = Vec::new();
        for row in rows {
            let (speaker, created_at, last_seen) = row?;
            out.push(SpeakerListRow {
                speaker,
                created_at: parse_rfc3339(&created_at)?,
                last_seen: last_seen.as_deref().map(parse_rfc3339).transpose()?,
            });
        }
        Ok(out)
    }

    /// Delete a speaker. Segments keep their text label but lose the
    /// registry link.
    pub fn delete_speaker(&self, id: &str) -> Result<bool> {
        let conn = self.lock();
        conn.execute(
            "UPDATE segments SET speaker_id = NULL WHERE speaker_id = ?1",
            params![id],
        )?;
        let n = conn.execute("DELETE FROM speakers WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    /// Delete each candidate speaker that is orphaned: no segment anywhere
    /// links to it and its notes are empty. A profile with notes survives
    /// (words the user wrote outrank tidiness), and one still linked
    /// somewhere is no orphan. Unknown ids are skipped. Returns the ids
    /// actually deleted.
    pub fn prune_orphaned_speakers(&self, candidates: &[String]) -> Result<Vec<String>> {
        let conn = self.lock();
        let mut deleted = Vec::new();
        for id in candidates {
            if deleted.contains(id) {
                continue;
            }
            let n = conn.execute(
                "DELETE FROM speakers
                  WHERE id = ?1
                    AND trim(notes) = ''
                    AND NOT EXISTS (SELECT 1 FROM segments WHERE speaker_id = ?1)",
                params![id],
            )?;
            if n > 0 {
                deleted.push(id.clone());
            }
        }
        Ok(deleted)
    }

    /// Relabel every segment linked to `speaker_id` (across meetings) with a
    /// new display name. Returns the affected meeting ids so the caller can
    /// regenerate those meetings' transcript documents.
    pub fn relabel_speaker_segments(&self, speaker_id: &str, name: &str) -> Result<Vec<String>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT DISTINCT meeting_id FROM segments
             WHERE speaker_id = ?1 AND speaker IS NOT ?2",
        )?;
        let meetings: Vec<String> = stmt
            .query_map(params![speaker_id, name], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        conn.execute(
            "UPDATE segments SET speaker = ?2 WHERE speaker_id = ?1",
            params![speaker_id, name],
        )?;
        Ok(meetings)
    }

    /// Within one meeting, link every segment labeled `label` to a registry
    /// speaker and relabel it with that person's name. Returns rows changed.
    pub fn assign_speaker_label(
        &self,
        meeting_id: &str,
        label: &str,
        speaker_id: &str,
        name: &str,
    ) -> Result<usize> {
        let n = self.lock().execute(
            "UPDATE segments SET speaker = ?4, speaker_id = ?3
             WHERE meeting_id = ?1 AND speaker = ?2",
            params![meeting_id, label, speaker_id, name],
        )?;
        Ok(n)
    }

    /// Merge one person's segments into another: repoint every segment
    /// linked to `source_id` at `target_id` and relabel it with the
    /// target's name, across all meetings. Returns the affected meeting
    /// ids so the caller can regenerate those transcript documents.
    pub fn merge_speaker_segments(
        &self,
        source_id: &str,
        target_id: &str,
        target_name: &str,
    ) -> Result<Vec<String>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT DISTINCT meeting_id FROM segments WHERE speaker_id = ?1",
        )?;
        let meetings: Vec<String> = stmt
            .query_map(params![source_id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        conn.execute(
            "UPDATE segments SET speaker_id = ?2, speaker = ?3 WHERE speaker_id = ?1",
            params![source_id, target_id, target_name],
        )?;
        Ok(meetings)
    }

    /// Link every unlinked segment whose label matches this person's name,
    /// case-insensitively, across all meetings. Names arrive through more
    /// than one path (typed live, applied by the notes-naming pass) and
    /// not every path sets the registry link; this is the catch-up that
    /// runs whenever a profile is created, renamed, or merged into. The
    /// label text itself does not change, so no transcript document needs
    /// regenerating. Returns rows changed.
    ///
    /// A generic "Speaker N" name adopts nothing: those labels are
    /// per-meeting placeholders, and two meetings' "Speaker 2" are
    /// different people.
    pub fn adopt_segments_by_name(&self, speaker_id: &str, name: &str) -> Result<usize> {
        if embral_types::is_generic_speaker_label(name) {
            return Ok(0);
        }
        let n = self.lock().execute(
            "UPDATE segments SET speaker_id = ?1
             WHERE speaker_id IS NULL AND lower(speaker) = lower(?2)",
            params![speaker_id, name],
        )?;
        Ok(n)
    }

    /// The meetings a person spoke in, newest first: the profile pane's
    /// record.
    pub fn speaker_meetings(&self, speaker_id: &str) -> Result<Vec<SpeakerMeetingRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT m.id, m.title, m.started_at, COUNT(*)
               FROM segments seg
               JOIN meetings m ON m.id = seg.meeting_id
              WHERE seg.speaker_id = ?1
              GROUP BY m.id
              ORDER BY m.started_at DESC",
        )?;
        let rows = stmt.query_map(params![speaker_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (meeting_id, title, started_at, segment_count) = row?;
            out.push(SpeakerMeetingRow {
                meeting_id,
                title,
                started_at: parse_rfc3339(&started_at)?,
                segment_count,
            });
        }
        Ok(out)
    }

    /// What one person said in one meeting, in transcript order.
    pub fn speaker_segments(
        &self,
        speaker_id: &str,
        meeting_id: &str,
    ) -> Result<Vec<TranscriptionSegment>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT speaker, speaker_id, text, start_secs, end_secs FROM segments
             WHERE meeting_id = ?1 AND speaker_id = ?2 ORDER BY idx",
        )?;
        let rows = stmt.query_map(params![meeting_id, speaker_id], |row| {
            Ok(TranscriptionSegment {
                speaker: row.get(0)?,
                speaker_id: row.get(1)?,
                text: row.get(2)?,
                start: row.get(3)?,
                end: row.get(4)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    // --- Pending name suggestions (from the user's typed notes) ---

    /// Persist a meeting's pending name suggestions (a JSON array; `[]`
    /// clears them).
    pub fn set_name_suggestions(&self, meeting_id: &str, json: &str) -> Result<()> {
        self.lock().execute(
            "UPDATE meetings SET name_suggestions = ?2 WHERE id = ?1",
            params![meeting_id, json],
        )?;
        Ok(())
    }

    pub fn get_name_suggestions(&self, meeting_id: &str) -> Result<String> {
        Ok(self
            .lock()
            .query_row(
                "SELECT name_suggestions FROM meetings WHERE id = ?1",
                params![meeting_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or_else(|| "[]".to_string()))
    }

    // --- Stars ---

    /// Persist a meeting's user-starred moments (a JSON array of seconds;
    /// `[]` clears).
    pub fn set_stars(&self, meeting_id: &str, json: &str) -> Result<()> {
        self.lock().execute(
            "UPDATE meetings SET stars_json = ?2 WHERE id = ?1",
            params![meeting_id, json],
        )?;
        Ok(())
    }

    pub fn get_stars(&self, meeting_id: &str) -> Result<String> {
        Ok(self
            .lock()
            .query_row(
                "SELECT stars_json FROM meetings WHERE id = ?1",
                params![meeting_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or_else(|| "[]".to_string()))
    }

    // --- The user's raw live notes ---

    pub fn set_notes(&self, meeting_id: &str, notes: &str) -> Result<()> {
        self.lock().execute(
            "UPDATE meetings SET notes = ?2 WHERE id = ?1",
            params![meeting_id, notes],
        )?;
        Ok(())
    }

    pub fn get_notes(&self, meeting_id: &str) -> Result<String> {
        Ok(self
            .lock()
            .query_row(
                "SELECT notes FROM meetings WHERE id = ?1",
                params![meeting_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or_default())
    }

    // --- Text read out of a meeting's images ---

    /// Every image of this meeting that has been read, newest name last.
    /// Ordered by filename, which is also paste order (`img-01`, `img-02`, …),
    /// so a chunk's position tracks where the image sits in the notes.
    pub fn image_text(&self, meeting_id: &str) -> Result<Vec<(String, String)>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT filename, ocr_text FROM image_text
             WHERE meeting_id = ?1 AND ocr_engine IS NOT NULL
             ORDER BY filename",
        )?;
        let rows = stmt
            .query_map(params![meeting_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Which of this meeting's images have already been read: the filenames
    /// only, so the sweep can diff them against the asset directory without
    /// pulling every passage into memory.
    pub fn image_text_filenames(&self, meeting_id: &str) -> Result<Vec<String>> {
        let conn = self.lock();
        let mut stmt =
            conn.prepare("SELECT filename FROM image_text WHERE meeting_id = ?1")?;
        let rows = stmt
            .query_map(params![meeting_id], |r| r.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Record what an engine read. `engine` names which one, so a library
    /// that has moved between machines says where each passage came from.
    pub fn set_image_text(
        &self,
        meeting_id: &str,
        filename: &str,
        ocr_text: &str,
        engine: &str,
    ) -> Result<()> {
        self.lock().execute(
            "INSERT INTO image_text (meeting_id, filename, ocr_text, ocr_engine)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(meeting_id, filename)
             DO UPDATE SET ocr_text = excluded.ocr_text, ocr_engine = excluded.ocr_engine",
            params![meeting_id, filename, ocr_text, engine],
        )?;
        Ok(())
    }

    // --- Dictation history ---

    /// Store one finished dictation; returns its row id.
    pub fn add_dictation(
        &self,
        raw_text: &str,
        cleaned_text: Option<&str>,
        app: Option<&str>,
    ) -> Result<i64> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO dictations (raw_text, cleaned_text, app, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![raw_text, cleaned_text, app, rfc3339(&Utc::now())],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Newest-first dictation history.
    pub fn list_dictations(&self, limit: u32) -> Result<Vec<DictationRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, raw_text, cleaned_text, app, created_at FROM dictations
             ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(DictationRow {
                id: row.get(0)?,
                raw_text: row.get(1)?,
                cleaned_text: row.get(2)?,
                app: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    pub fn delete_dictation(&self, id: i64) -> Result<bool> {
        let n = self
            .lock()
            .execute("DELETE FROM dictations WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    /// Drop dictations older than `days` (0 = keep everything). Returns rows
    /// removed; the janitor logs it.
    pub fn prune_dictations(&self, days: u32) -> Result<usize> {
        if days == 0 {
            return Ok(0);
        }
        let cutoff = rfc3339(&(Utc::now() - chrono::Duration::days(days as i64)));
        let n = self.lock().execute(
            "DELETE FROM dictations WHERE created_at < ?1",
            params![cutoff],
        )?;
        Ok(n)
    }

    // --- Full clears (the scoped reset; commands.rs::reset_app_data) ---

    /// Delete every meeting (segments cascade, FTS rows via trigger).
    /// Returns rows removed. The caller removes the meetings' files first;
    /// the rows carry the paths.
    pub fn clear_meetings(&self) -> Result<usize> {
        Ok(self.lock().execute("DELETE FROM meetings", [])?)
    }

    /// Delete every speaker profile; segments keep their text labels but
    /// lose the registry links, same rule as [`Self::delete_speaker`].
    pub fn clear_speakers(&self) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "UPDATE segments SET speaker_id = NULL WHERE speaker_id IS NOT NULL",
            [],
        )?;
        conn.execute("DELETE FROM speakers", [])?;
        Ok(())
    }

    /// Delete the whole dictation history. Returns rows removed.
    pub fn clear_dictations(&self) -> Result<usize> {
        Ok(self.lock().execute("DELETE FROM dictations", [])?)
    }

    /// Keep only the newest `count` dictations (0 = keep everything).
    /// Returns rows removed; the janitor logs it.
    pub fn prune_dictations_beyond(&self, count: u32) -> Result<usize> {
        if count == 0 {
            return Ok(0);
        }
        let n = self.lock().execute(
            "DELETE FROM dictations WHERE id NOT IN
             (SELECT id FROM dictations ORDER BY id DESC LIMIT ?1)",
            params![count as i64],
        )?;
        Ok(n)
    }

}

const MEETING_COLS: &str = "id, title, started_at, duration_seconds, summary, transcript,
                            attendees, audio_path";

fn row_to_speaker(row: &rusqlite::Row<'_>) -> rusqlite::Result<SpeakerRow> {
    Ok(SpeakerRow {
        id: row.get(0)?,
        name: row.get(1)?,
        notes: row.get(2)?,
    })
}

fn row_to_meeting(row: &rusqlite::Row<'_>) -> rusqlite::Result<MeetingRow> {
    let started_at: String = row.get(2)?;
    let attendees: String = row.get(6)?;
    Ok(MeetingRow {
        id: row.get(0)?,
        title: row.get(1)?,
        started_at: parse_rfc3339(&started_at).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, e.into())
        })?,
        duration_seconds: row.get::<_, i64>(3)? as u64,
        summary: row.get(4)?,
        transcript: row.get(5)?,
        attendees: serde_json::from_str(&attendees).unwrap_or_default(),
        audio_path: row.get(7)?,
    })
}

fn rfc3339(dt: &DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn parse_rfc3339(s: &str) -> anyhow::Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(s)
        .with_context(|| format!("bad timestamp in db: {s}"))?
        .with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn mk(id: &str, title: &str, day: u32, notes: &str, transcript: &str) -> MeetingRow {
        MeetingRow {
            id: id.to_string(),
            title: title.to_string(),
            started_at: Utc.with_ymd_and_hms(2026, 6, day, 10, 0, 0).unwrap(),
            duration_seconds: 600,
            summary: notes.to_string(),
            transcript: transcript.to_string(),
            attendees: vec!["Alice".into(), "Bob".into()],
            audio_path: format!("audio/{id}.mp3"),
        }
    }

    #[test]
    fn crud_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        let m = mk("m1", "Pipeline Review", 1, "# Notes", "transcript body");
        db.upsert_meeting(&m).unwrap();

        let got = db.get_meeting("m1").unwrap().unwrap();
        assert_eq!(got, m);
        assert_eq!(db.meeting_count().unwrap(), 1);

        assert!(db.delete_meeting("m1").unwrap());
        assert!(db.get_meeting("m1").unwrap().is_none());
        assert!(!db.delete_meeting("m1").unwrap());
    }

    #[test]
    fn upsert_updates_in_place() {
        let db = Db::open_in_memory().unwrap();
        let mut m = mk("m1", "Old Title", 1, "old", "t");
        db.upsert_meeting(&m).unwrap();
        m.title = "New Title".into();
        m.summary = "new notes".into();
        db.upsert_meeting(&m).unwrap();

        let got = db.get_meeting("m1").unwrap().unwrap();
        assert_eq!(got.title, "New Title");
        assert_eq!(db.meeting_count().unwrap(), 1);
    }

    #[test]
    fn list_orders_desc_and_filters() {
        let db = Db::open_in_memory().unwrap();
        db.upsert_meeting(&mk("a", "First", 1, "", "")).unwrap();
        db.upsert_meeting(&mk("b", "Second", 2, "", "")).unwrap();
        db.upsert_meeting(&mk("c", "Third", 3, "", "")).unwrap();

        let all = db.list_meetings(None, None).unwrap();
        assert_eq!(
            all.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["c", "b", "a"]
        );

        let limited = db.list_meetings(Some(2), None).unwrap();
        assert_eq!(limited.len(), 2);

        let since = Utc.with_ymd_and_hms(2026, 6, 2, 0, 0, 0).unwrap();
        let recent = db.list_meetings(None, Some(since)).unwrap();
        assert_eq!(
            recent.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["c", "b"]
        );
    }

    /// Walk the whole list a page at a time, the way the meetings list does.
    fn page_through(db: &Db, size: u32) -> Vec<String> {
        let mut seen = Vec::new();
        let mut cursor: Option<MeetingCursor> = None;
        loop {
            let page = db.list_meetings_page(Some(size), None, cursor.as_ref()).unwrap();
            let Some(last) = page.last() else { break };
            cursor = Some(last.cursor());
            seen.extend(page.iter().map(|m| m.id.clone()));
            if page.len() < size as usize {
                break;
            }
        }
        seen
    }

    #[test]
    fn pages_cover_the_list_exactly_once() {
        let db = Db::open_in_memory().unwrap();
        // "c1" and "c2" start in the same second, which a bulk import
        // produces easily. Ordering on the timestamp alone leaves the pair
        // in no fixed order, so a page boundary between them could hand
        // back one of them twice and never the other.
        for (id, day) in [("a", 1), ("b", 2), ("c1", 3), ("c2", 3), ("e", 5)] {
            db.upsert_meeting(&mk(id, "T", day, "", "")).unwrap();
        }

        assert_eq!(page_through(&db, 2), vec!["e", "c2", "c1", "b", "a"]);
        // The page size must not change which meetings the user can reach.
        assert_eq!(page_through(&db, 1), page_through(&db, 5));
    }

    #[test]
    fn a_delete_mid_scroll_does_not_skip_the_next_row() {
        let db = Db::open_in_memory().unwrap();
        for (id, day) in [("a", 1), ("b", 2), ("c", 3), ("d", 4), ("e", 5)] {
            db.upsert_meeting(&mk(id, "T", day, "", "")).unwrap();
        }

        let first = db.list_meetings_page(Some(2), None, None).unwrap();
        assert_eq!(first.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["e", "d"]);

        // Deleting a row the user has already scrolled past pulls every
        // later row up by one. An offset of 2 would resume at "b" and lose
        // "c"; the cursor still names the row the page ended on.
        db.delete_meeting("e").unwrap();
        let next = db
            .list_meetings_page(Some(2), None, Some(&first[1].cursor()))
            .unwrap();
        assert_eq!(next.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["c", "b"]);
    }

    #[test]
    fn segments_roundtrip_and_cascade() {
        let db = Db::open_in_memory().unwrap();
        db.upsert_meeting(&mk("m1", "T", 1, "", "")).unwrap();
        let segs = vec![
            TranscriptionSegment {
                speaker: Some("Speaker 1".into()),
                speaker_id: Some("sp-alice".into()),
                text: "hello there".into(),
                start: 0.0,
                end: 1.5,
            },
            TranscriptionSegment {
                speaker: None,
                speaker_id: None,
                text: "second utterance".into(),
                start: 2.0,
                end: 3.0,
            },
        ];
        db.replace_segments("m1", &segs).unwrap();
        assert_eq!(db.get_segments("m1").unwrap().len(), 2);
        let got = db.get_segments("m1").unwrap();
        assert_eq!(got[0].speaker.as_deref(), Some("Speaker 1"));
        assert_eq!(got[0].speaker_id.as_deref(), Some("sp-alice"));
        assert_eq!(got[1].text, "second utterance");
        assert_eq!(got[1].speaker_id, None);

        // Replacing overwrites.
        db.replace_segments("m1", &segs[..1]).unwrap();
        assert_eq!(db.get_segments("m1").unwrap().len(), 1);

        // Deleting the meeting cascades.
        db.delete_meeting("m1").unwrap();
        assert!(db.get_segments("m1").unwrap().is_empty());
    }

    /// The profiles list is ordered by who you last met with, so the query has
    /// to find each person's newest meeting, not their first, and not the
    /// newest meeting overall.
    #[test]
    fn speakers_are_ordered_by_their_last_meeting() {
        let db = Db::open_in_memory().unwrap();
        db.upsert_speaker(&mk_speaker("sp_a", "Alice")).unwrap();
        db.upsert_speaker(&mk_speaker("sp_b", "Bob")).unwrap();
        db.upsert_speaker(&mk_speaker("sp_c", "Never Seen")).unwrap();

        // Alice speaks only in the older meeting. Bob speaks in both, so his
        // first meeting ties Alice's and only his newest one ranks him ahead.
        db.upsert_meeting(&mk("m_old", "Old", 1, "", "")).unwrap();
        db.upsert_meeting(&mk("m_new", "New", 9, "", "")).unwrap();
        db.replace_segments(
            "m_old",
            &[
                seg(Some("Alice"), Some("sp_a"), 0.0),
                seg(Some("Bob"), Some("sp_b"), 2.0),
            ],
        )
        .unwrap();
        db.replace_segments("m_new", &[seg(Some("Bob"), Some("sp_b"), 0.0)])
            .unwrap();

        let rows = db.list_speakers_by_activity().unwrap();
        let order: Vec<&str> = rows.iter().map(|r| r.speaker.name.as_str()).collect();

        // Never Seen has no meeting, so it sorts on created_at, which is now
        // and so newer than either meeting (they are dated in the past).
        assert_eq!(order, vec!["Never Seen", "Bob", "Alice"]);

        let bob = rows.iter().find(|r| r.speaker.id == "sp_b").unwrap();
        assert_eq!(bob.last_seen.unwrap(), Utc.with_ymd_and_hms(2026, 6, 9, 10, 0, 0).unwrap());
        let alice = rows.iter().find(|r| r.speaker.id == "sp_a").unwrap();
        assert_eq!(alice.last_seen.unwrap(), Utc.with_ymd_and_hms(2026, 6, 1, 10, 0, 0).unwrap());
        // Never in a meeting: no last-seen at all, and the list falls back to
        // when they were created rather than dropping them off the bottom.
        assert!(rows.iter().find(|r| r.speaker.id == "sp_c").unwrap().last_seen.is_none());
    }

    #[test]
    fn export_records_match_index_shape() {
        let db = Db::open_in_memory().unwrap();
        db.upsert_meeting(&mk("m1", "T", 1, "", "")).unwrap();
        let recs = db.export_records().unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].id, "m1");
        assert_eq!(recs[0].chunks, 1);
        assert_eq!(recs[0].audio_path, "audio/m1.mp3");
    }

    #[test]
    fn import_legacy_is_transactional_bulk() {
        let db = Db::open_in_memory().unwrap();
        let rows = vec![mk("a", "A", 1, "na", "ta"), mk("b", "B", 2, "nb", "tb")];
        assert_eq!(db.import_legacy(&rows).unwrap(), 2);
        assert_eq!(db.meeting_count().unwrap(), 2);
    }

    fn mk_speaker(id: &str, name: &str) -> SpeakerRow {
        SpeakerRow {
            id: id.to_string(),
            name: name.to_string(),
            notes: String::new(),
        }
    }

    fn seg(speaker: Option<&str>, speaker_id: Option<&str>, start: f64) -> TranscriptionSegment {
        TranscriptionSegment {
            speaker: speaker.map(String::from),
            speaker_id: speaker_id.map(String::from),
            text: "words".into(),
            start,
            end: start + 1.0,
        }
    }

    #[test]
    fn speakers_crud_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        let alice = mk_speaker("sp-a", "Alice");
        let bob = mk_speaker("sp-b", "bob");
        db.upsert_speaker(&alice).unwrap();
        db.upsert_speaker(&bob).unwrap();

        assert_eq!(db.get_speaker("sp-a").unwrap().unwrap(), alice);
        // Ordered by name, case-insensitively.
        let all = db.list_speakers().unwrap();
        assert_eq!(
            all.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            vec!["sp-a", "sp-b"]
        );

        let mut renamed = alice.clone();
        renamed.name = "Alice B".into();
        db.upsert_speaker(&renamed).unwrap();
        assert_eq!(db.get_speaker("sp-a").unwrap().unwrap().name, "Alice B");

        assert!(db.delete_speaker("sp-a").unwrap());
        assert!(db.get_speaker("sp-a").unwrap().is_none());
        assert!(!db.delete_speaker("sp-a").unwrap());
    }

    #[test]
    fn delete_speaker_unlinks_segments() {
        let db = Db::open_in_memory().unwrap();
        db.upsert_meeting(&mk("m1", "T", 1, "", "")).unwrap();
        db.upsert_speaker(&mk_speaker("sp-a", "Alice")).unwrap();
        db.replace_segments("m1", &[seg(Some("Alice"), Some("sp-a"), 0.0)])
            .unwrap();

        db.delete_speaker("sp-a").unwrap();
        let got = db.get_segments("m1").unwrap();
        assert_eq!(got[0].speaker.as_deref(), Some("Alice")); // label survives
        assert_eq!(got[0].speaker_id, None); // link cleared
    }

    #[test]
    fn prune_deletes_only_unlinked_noteless_speakers() {
        let db = Db::open_in_memory().unwrap();
        db.upsert_meeting(&mk("m1", "T", 1, "", "")).unwrap();
        // Orphan: no segments, no notes (the typo profile).
        db.upsert_speaker(&mk_speaker("sp-typo", "Jhon")).unwrap();
        // Still linked: keeps its history.
        db.upsert_speaker(&mk_speaker("sp-live", "John")).unwrap();
        db.replace_segments("m1", &[seg(Some("John"), Some("sp-live"), 0.0)])
            .unwrap();
        // Unlinked but annotated: the user's words protect it.
        let mut noted = mk_speaker("sp-noted", "Maya");
        noted.notes = "prefers async updates".into();
        db.upsert_speaker(&noted).unwrap();

        let candidates: Vec<String> = ["sp-typo", "sp-live", "sp-noted", "sp-ghost"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let deleted = db.prune_orphaned_speakers(&candidates).unwrap();

        assert_eq!(deleted, vec!["sp-typo".to_string()]);
        assert!(db.get_speaker("sp-typo").unwrap().is_none());
        assert!(db.get_speaker("sp-live").unwrap().is_some());
        assert!(db.get_speaker("sp-noted").unwrap().is_some());
    }

    #[test]
    fn relabel_and_assign_speaker_segments() {
        let db = Db::open_in_memory().unwrap();
        db.upsert_meeting(&mk("m1", "A", 1, "", "")).unwrap();
        db.upsert_meeting(&mk("m2", "B", 2, "", "")).unwrap();
        db.upsert_speaker(&mk_speaker("sp-a", "Alice")).unwrap();

        db.replace_segments(
            "m1",
            &[
                seg(Some("Speaker 1"), None, 0.0),
                seg(Some("Speaker 2"), None, 2.0),
            ],
        )
        .unwrap();
        db.replace_segments("m2", &[seg(Some("Alice"), Some("sp-a"), 0.0)])
            .unwrap();

        // Confirming a suggestion: label → registry link + name, one meeting.
        let n = db
            .assign_speaker_label("m1", "Speaker 1", "sp-a", "Alice")
            .unwrap();
        assert_eq!(n, 1);
        let m1 = db.get_segments("m1").unwrap();
        assert_eq!(m1[0].speaker.as_deref(), Some("Alice"));
        assert_eq!(m1[0].speaker_id.as_deref(), Some("sp-a"));
        assert_eq!(m1[1].speaker.as_deref(), Some("Speaker 2")); // untouched

        // Renaming the person relabels linked segments across meetings and
        // reports which meetings changed.
        let mut affected = db.relabel_speaker_segments("sp-a", "Alicia").unwrap();
        affected.sort();
        assert_eq!(affected, vec!["m1".to_string(), "m2".to_string()]);
        assert_eq!(
            db.get_segments("m2").unwrap()[0].speaker.as_deref(),
            Some("Alicia")
        );
        // No-op rename touches nothing.
        assert!(db.relabel_speaker_segments("sp-a", "Alicia").unwrap().is_empty());
    }

    #[test]
    fn speaker_record_lists_meetings_and_their_lines() {
        let db = Db::open_in_memory().unwrap();
        db.upsert_meeting(&mk("m1", "A", 1, "", "")).unwrap();
        db.upsert_meeting(&mk("m2", "B", 2, "", "")).unwrap();
        db.upsert_speaker(&mk_speaker("sp-a", "Alice")).unwrap();
        db.replace_segments(
            "m1",
            &[
                seg(Some("Alice"), Some("sp-a"), 0.0),
                seg(Some("Bob"), None, 2.0),
                seg(Some("Alice"), Some("sp-a"), 4.0),
            ],
        )
        .unwrap();
        db.replace_segments("m2", &[seg(Some("Alice"), Some("sp-a"), 0.0)])
            .unwrap();

        let record = db.speaker_meetings("sp-a").unwrap();
        assert_eq!(record.len(), 2);
        assert_eq!(record[0].meeting_id, "m2"); // newest first
        assert_eq!(record[1].segment_count, 2); // Bob's line is not hers

        let lines = db.speaker_segments("sp-a", "m1").unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].start, 0.0);
        assert_eq!(lines[1].start, 4.0);
    }

    #[test]
    fn merge_repoints_and_relabels_across_meetings() {
        let db = Db::open_in_memory().unwrap();
        db.upsert_meeting(&mk("m1", "A", 1, "", "")).unwrap();
        db.upsert_meeting(&mk("m2", "B", 2, "", "")).unwrap();
        db.upsert_speaker(&mk_speaker("sp-john", "John")).unwrap();
        db.upsert_speaker(&mk_speaker("sp-js", "John Smith")).unwrap();
        db.replace_segments(
            "m1",
            &[
                seg(Some("John"), Some("sp-john"), 0.0),
                seg(Some("Bob"), None, 2.0),
            ],
        )
        .unwrap();
        db.replace_segments("m2", &[seg(Some("John"), Some("sp-john"), 0.0)])
            .unwrap();

        let mut affected = db
            .merge_speaker_segments("sp-john", "sp-js", "John Smith")
            .unwrap();
        affected.sort();
        assert_eq!(affected, vec!["m1".to_string(), "m2".to_string()]);

        let m1 = db.get_segments("m1").unwrap();
        assert_eq!(m1[0].speaker.as_deref(), Some("John Smith"));
        assert_eq!(m1[0].speaker_id.as_deref(), Some("sp-js"));
        assert_eq!(m1[1].speaker.as_deref(), Some("Bob")); // untouched
        assert_eq!(
            db.get_segments("m2").unwrap()[0].speaker_id.as_deref(),
            Some("sp-js")
        );

        // Nothing is linked to the source anymore.
        assert!(db
            .merge_speaker_segments("sp-john", "sp-js", "John Smith")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn adopt_links_name_only_segments() {
        let db = Db::open_in_memory().unwrap();
        db.upsert_meeting(&mk("m1", "A", 1, "", "")).unwrap();
        db.upsert_meeting(&mk("m2", "B", 2, "", "")).unwrap();
        db.upsert_speaker(&mk_speaker("sp-a", "Alice B")).unwrap();
        db.replace_segments(
            "m1",
            &[
                seg(Some("alice b"), None, 0.0),         // case aside, hers
                seg(Some("Alice B"), Some("sp-x"), 2.0), // already someone's
                seg(Some("Speaker 2"), None, 4.0),       // not hers
            ],
        )
        .unwrap();
        db.replace_segments("m2", &[seg(Some("Alice B"), None, 0.0)])
            .unwrap();

        assert_eq!(db.adopt_segments_by_name("sp-a", "Alice B").unwrap(), 2);

        let m1 = db.get_segments("m1").unwrap();
        assert_eq!(m1[0].speaker_id.as_deref(), Some("sp-a"));
        assert_eq!(m1[0].speaker.as_deref(), Some("alice b")); // label untouched
        assert_eq!(m1[1].speaker_id.as_deref(), Some("sp-x")); // never stolen
        assert_eq!(m1[2].speaker_id, None);
        assert_eq!(
            db.get_segments("m2").unwrap()[0].speaker_id.as_deref(),
            Some("sp-a")
        );

        // Adopted segments count toward the person's activity.
        let row = db.speaker_by_activity("sp-a").unwrap().unwrap();
        assert_eq!(
            row.last_seen.unwrap(),
            Utc.with_ymd_and_hms(2026, 6, 2, 10, 0, 0).unwrap()
        );

        // A generic name adopts nothing: two meetings' "Speaker 2" are
        // different people.
        db.upsert_speaker(&mk_speaker("sp-g", "Speaker 2")).unwrap();
        assert_eq!(db.adopt_segments_by_name("sp-g", "Speaker 2").unwrap(), 0);
        assert_eq!(db.get_segments("m1").unwrap()[2].speaker_id, None);
    }

    #[test]
    fn name_suggestions_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        db.upsert_meeting(&mk("m1", "T", 1, "", "")).unwrap();
        assert_eq!(db.get_name_suggestions("m1").unwrap(), "[]");
        db.set_name_suggestions("m1", r#"[{"label":"Speaker 1","name":"John"}]"#)
            .unwrap();
        assert_eq!(
            db.get_name_suggestions("m1").unwrap(),
            r#"[{"label":"Speaker 1","name":"John"}]"#
        );
        // Unknown meeting reads as empty, not an error.
        assert_eq!(db.get_name_suggestions("nope").unwrap(), "[]");
    }

    #[test]
    fn reopen_is_idempotent_across_migrations() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("embral.db");
        {
            let db = Db::open(&path).unwrap();
            db.upsert_meeting(&mk("m1", "T", 1, "", "")).unwrap();
        }
        let db = Db::open(&path).unwrap();
        db.upsert_speaker(&mk_speaker("sp-a", "Alice")).unwrap();
        assert_eq!(db.list_speakers().unwrap().len(), 1);
    }

    #[test]
    fn read_only_sees_the_writers_data_concurrently() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("embral.db");
        let writer = Db::open(&path).unwrap();
        writer
            .upsert_meeting(&mk("m1", "Pipeline Review", 1, "# Notes", "budget talk"))
            .unwrap();

        // The writer stays open: this is the app-running case.
        let reader = Db::open_read_only(&path).unwrap();
        assert_eq!(reader.list_meetings(None, None).unwrap().len(), 1);
        assert_eq!(
            reader.get_meeting("m1").unwrap().unwrap().title,
            "Pipeline Review"
        );

        // A commit after the reader opened is visible on its next query.
        writer.upsert_meeting(&mk("m2", "Later", 2, "", "")).unwrap();
        assert_eq!(reader.list_meetings(None, None).unwrap().len(), 2);
    }

    #[test]
    fn read_only_refuses_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("embral.db");
        drop(Db::open(&path).unwrap());

        let reader = Db::open_read_only(&path).unwrap();
        assert!(reader.upsert_meeting(&mk("m1", "T", 1, "", "")).is_err());
    }

    #[test]
    fn read_only_never_creates_a_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.db");
        assert!(Db::open_read_only(&path).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn schema_version_matches_this_build_after_open() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.schema_version().unwrap(), latest_schema_version());
    }

    fn insert_chunk(db: &Db, meeting_id: &str, idx: i64, text: &str) -> i64 {
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO chunks (meeting_id, source, chunk_index, text,
                                     embedding_text, content_hash)
                 VALUES (?1, 'transcript', ?2, ?3, ?3, 'h')",
                params![meeting_id, idx, text],
            )?;
            Ok(conn.last_insert_rowid())
        })
        .unwrap()
    }

    fn fts_hits(db: &Db, needle: &str) -> Vec<i64> {
        db.with_conn(|conn| {
            let mut stmt =
                conn.prepare("SELECT rowid FROM chunks_fts WHERE chunks_fts MATCH ?1")?;
            let ids = stmt
                .query_map([needle], |r| r.get::<_, i64>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(ids)
        })
        .unwrap()
    }

    #[test]
    fn chunk_fts_follows_text_but_not_bookkeeping_updates() {
        let db = Db::open_in_memory().unwrap();
        db.upsert_meeting(&mk("m1", "T", 1, "", "")).unwrap();
        let id = insert_chunk(&db, "m1", 0, "the quarterly budget review");

        assert_eq!(fts_hits(&db, "budget"), vec![id]);

        // Bookkeeping updates (reorder, embedding progress) skip the FTS
        // trigger entirely: it is scoped to `UPDATE OF text`.
        db.with_conn(|conn| {
            conn.execute(
                "UPDATE chunks SET chunk_index = 7, embedded_with = 'model-x' WHERE id = ?1",
                [id],
            )?;
            Ok(())
        })
        .unwrap();
        assert_eq!(fts_hits(&db, "budget"), vec![id]);

        // A text update re-indexes: old term gone, new term found.
        db.with_conn(|conn| {
            conn.execute("UPDATE chunks SET text = 'hiring pipeline' WHERE id = ?1", [id])?;
            Ok(())
        })
        .unwrap();
        assert!(fts_hits(&db, "budget").is_empty());
        assert_eq!(fts_hits(&db, "hiring"), vec![id]);

        // The external-content index and its table still agree.
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO chunks_fts(chunks_fts) VALUES ('integrity-check')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn chunks_require_exactly_one_owner() {
        let db = Db::open_in_memory().unwrap();
        db.upsert_meeting(&mk("m1", "T", 1, "", "")).unwrap();
        let both_null = db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO chunks (source, chunk_index, text, embedding_text, content_hash)
                 VALUES ('transcript', 0, 't', 't', 'h')",
                [],
            )?;
            Ok(())
        });
        assert!(both_null.is_err());

        let dictation_id = db.add_dictation("raw", None, None).unwrap();
        let both_set = db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO chunks (meeting_id, dictation_id, source, chunk_index,
                                     text, embedding_text, content_hash)
                 VALUES ('m1', ?1, 'transcript', 0, 't', 't', 'h')",
                [dictation_id],
            )?;
            Ok(())
        });
        assert!(both_set.is_err());
    }

    #[test]
    fn deleting_the_owner_cascades_chunks_and_their_fts_rows() {
        let db = Db::open_in_memory().unwrap();
        db.upsert_meeting(&mk("m1", "T", 1, "", "")).unwrap();
        insert_chunk(&db, "m1", 0, "cascade target words");
        assert_eq!(fts_hits(&db, "cascade").len(), 1);

        db.delete_meeting("m1").unwrap();
        let remaining: i64 = db
            .with_conn(|conn| {
                Ok(conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))?)
            })
            .unwrap();
        assert_eq!(remaining, 0);
        assert!(fts_hits(&db, "cascade").is_empty());
    }

    #[test]
    fn vec0_round_trips_in_memory() {
        let db = Db::open_in_memory().unwrap();
        db.with_conn(|conn| {
            conn.execute_batch(
                "CREATE VIRTUAL TABLE v USING vec0(embedding float[4])",
            )?;
            for (rowid, vec) in [
                (1i64, [1.0f32, 0.0, 0.0, 0.0]),
                (2, [0.0, 1.0, 0.0, 0.0]),
                (3, [0.9, 0.1, 0.0, 0.0]),
            ] {
                conn.execute(
                    "INSERT INTO v(rowid, embedding) VALUES (?1, ?2)",
                    params![rowid, embedding_to_blob(&vec)],
                )?;
            }
            let query = embedding_to_blob(&[1.0f32, 0.0, 0.0, 0.0]);
            let mut stmt = conn.prepare(
                "SELECT rowid FROM v WHERE embedding MATCH ?1 AND k = 2 ORDER BY distance",
            )?;
            let ids = stmt
                .query_map([query], |r| r.get::<_, i64>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            assert_eq!(ids, vec![1, 3]);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn vec0_knn_works_through_a_read_only_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("embral.db");
        {
            let writer = Db::open(&path).unwrap();
            writer
                .with_conn(|conn| {
                    conn.execute_batch(
                        "CREATE VIRTUAL TABLE v USING vec0(embedding float[4])",
                    )?;
                    conn.execute(
                        "INSERT INTO v(rowid, embedding) VALUES (1, ?1)",
                        params![embedding_to_blob(&[0.5f32, 0.5, 0.0, 0.0])],
                    )?;
                    Ok(())
                })
                .unwrap();
        }
        let reader = Db::open_read_only(&path).unwrap();
        let hit: i64 = reader
            .with_conn(|conn| {
                Ok(conn.query_row(
                    "SELECT rowid FROM v WHERE embedding MATCH ?1 AND k = 1 ORDER BY distance",
                    params![embedding_to_blob(&[0.4f32, 0.6, 0.0, 0.0])],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(hit, 1);
    }

    #[test]
    fn stars_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        db.upsert_meeting(&mk("m1", "T", 1, "", "")).unwrap();
        assert_eq!(db.get_stars("m1").unwrap(), "[]");
        db.set_stars("m1", "[12.5,340.0]").unwrap();
        assert_eq!(db.get_stars("m1").unwrap(), "[12.5,340.0]");
        assert_eq!(db.get_stars("missing").unwrap(), "[]");
    }

    #[test]
    fn notes_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        db.upsert_meeting(&mk("m1", "T", 1, "", "")).unwrap();
        assert_eq!(db.get_notes("m1").unwrap(), "");
        db.set_notes("m1", "- remember the budget").unwrap();
        assert_eq!(db.get_notes("m1").unwrap(), "- remember the budget");
        assert_eq!(db.get_notes("missing").unwrap(), "");
    }

    #[test]
    fn image_text_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        db.upsert_meeting(&mk("m1", "T", 1, "", "")).unwrap();
        assert!(db.image_text("m1").unwrap().is_empty());

        db.set_image_text("m1", "img-02.png", "Q4 forecast", "windows").unwrap();
        db.set_image_text("m1", "img-01.png", "Q3 revenue", "windows").unwrap();
        // Paste order, not insert order; the filenames carry it.
        assert_eq!(
            db.image_text("m1").unwrap(),
            vec![
                ("img-01.png".to_string(), "Q3 revenue".to_string()),
                ("img-02.png".to_string(), "Q4 forecast".to_string()),
            ]
        );

        // Reading an image again replaces what was there rather than
        // stacking a second row on the same file.
        db.set_image_text("m1", "img-01.png", "Q3 revenue 4.2M", "windows").unwrap();
        assert_eq!(db.image_text("m1").unwrap().len(), 2);
        assert_eq!(db.image_text("m1").unwrap()[0].1, "Q3 revenue 4.2M");

        // An image that read as blank is still an answer: it stays out of
        // the index but must not look pending to the sweep.
        db.set_image_text("m1", "img-03.png", "", "windows").unwrap();
        assert_eq!(db.image_text_filenames("m1").unwrap().len(), 3);
    }

    #[test]
    fn dictations_crud_and_prune() {
        let db = Db::open_in_memory().unwrap();
        let id1 = db.add_dictation("raw one", None, None).unwrap();
        let id2 = db
            .add_dictation("raw two", Some("Cleaned two."), Some("notepad.exe"))
            .unwrap();
        assert!(id2 > id1);

        let rows = db.list_dictations(10).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, id2); // newest first
        assert_eq!(rows[0].cleaned_text.as_deref(), Some("Cleaned two."));
        assert_eq!(rows[0].app.as_deref(), Some("notepad.exe"));
        assert_eq!(rows[1].raw_text, "raw one");
        assert_eq!(db.list_dictations(1).unwrap().len(), 1);

        assert!(db.delete_dictation(id1).unwrap());
        assert!(!db.delete_dictation(id1).unwrap());

        // Retention 0 keeps everything; a 1-day window keeps today's rows.
        assert_eq!(db.prune_dictations(0).unwrap(), 0);
        assert_eq!(db.prune_dictations(1).unwrap(), 0);
        assert_eq!(db.list_dictations(10).unwrap().len(), 1);

        // Backdate the remaining row and prune it out.
        db.lock()
            .execute(
                "UPDATE dictations SET created_at = '2020-01-01T00:00:00Z'",
                [],
            )
            .unwrap();
        assert_eq!(db.prune_dictations(30).unwrap(), 1);
        assert!(db.list_dictations(10).unwrap().is_empty());
    }

    #[test]
    fn clears_scope_exactly_their_own_table() {
        let db = Db::open_in_memory().unwrap();

        // One of everything: a meeting with a segment linked to a speaker,
        // and a dictation.
        db.upsert_meeting(&mk("m1", "Reset drill", 1, "# Notes", "body"))
            .unwrap();
        db.upsert_speaker(&mk_speaker("sp1", "Ada")).unwrap();
        db.replace_segments(
            "m1",
            &[embral_types::TranscriptionSegment {
                speaker: Some("Ada".into()),
                speaker_id: Some("sp1".into()),
                text: "hello".into(),
                start: 0.0,
                end: 1.0,
            }],
        )
        .unwrap();
        db.add_dictation("raw", None, None).unwrap();

        // Dictations go alone.
        assert_eq!(db.clear_dictations().unwrap(), 1);
        assert!(db.list_dictations(10).unwrap().is_empty());
        assert_eq!(db.list_meetings(None, None).unwrap().len(), 1);

        // Speakers go; the meeting's segments keep their text label.
        db.clear_speakers().unwrap();
        assert!(db.list_speakers().unwrap().is_empty());
        let segs = db.get_segments("m1").unwrap();
        assert_eq!(segs[0].speaker.as_deref(), Some("Ada"));
        assert_eq!(segs[0].speaker_id, None);

        // Meetings last.
        assert_eq!(db.clear_meetings().unwrap(), 1);
        assert!(db.list_meetings(None, None).unwrap().is_empty());
        assert!(db.get_segments("m1").unwrap().is_empty());
    }

    #[test]
    fn count_prune_keeps_the_newest() {
        let db = Db::open_in_memory().unwrap();
        for i in 0..5 {
            db.add_dictation(&format!("raw {i}"), None, None).unwrap();
        }

        // 0 = the criterion is off.
        assert_eq!(db.prune_dictations_beyond(0).unwrap(), 0);
        // Keeping more than exist deletes nothing.
        assert_eq!(db.prune_dictations_beyond(10).unwrap(), 0);

        assert_eq!(db.prune_dictations_beyond(2).unwrap(), 3);
        let rows = db.list_dictations(10).unwrap();
        assert_eq!(rows.len(), 2);
        // The newest two survived.
        assert_eq!(rows[0].raw_text, "raw 4");
        assert_eq!(rows[1].raw_text, "raw 3");
    }

    #[test]
    fn embedding_blob_roundtrip() {
        let e = vec![0.25f32, -1.5, 3.25e-3, f32::MAX];
        assert_eq!(blob_to_embedding(&embedding_to_blob(&e)), e);
        assert!(blob_to_embedding(&[]).is_empty());
    }

    #[test]
    fn persists_to_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("embral.db");
        {
            let db = Db::open(&path).unwrap();
            db.upsert_meeting(&mk("m1", "Persisted", 1, "", "")).unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(db.get_meeting("m1").unwrap().unwrap().title, "Persisted");
    }
}
