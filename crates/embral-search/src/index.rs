//! Keeping `chunks` true to the library: incremental by content hash, so
//! an edit re-embeds only what actually changed. Chunk+FTS sync is cheap
//! and runs inside the mutation paths; embedding is the slow part and runs
//! in the app's background worker (`embed_pending`).

use anyhow::Result;
use chrono::{DateTime, Utc};
use embral_db::rusqlite::{params, Connection, OptionalExtension};
use embral_db::Db;

use crate::chunker::{chunk_dictation, chunk_meeting, BuiltChunk, MeetingDocs};
use crate::model;
use crate::vector::{SqliteVecIndex, VectorIndex};

#[derive(Debug, Default, PartialEq)]
pub struct SyncStats {
    pub inserted: usize,
    pub kept: usize,
    pub deleted: usize,
}

enum Owner<'a> {
    Meeting(&'a str),
    Dictation(i64),
}

impl Owner<'_> {
    fn where_clause(&self) -> &'static str {
        match self {
            Owner::Meeting(_) => "meeting_id = ?1",
            Owner::Dictation(_) => "dictation_id = ?1",
        }
    }
}

/// Rebuild one owner's chunks against `built`, preserving rows (and their
/// embeddings) whose content hash survives. Everything runs in one write
/// transaction.
fn apply(conn: &Connection, owner: &Owner, built: &[BuiltChunk]) -> Result<SyncStats> {
    let tx = conn.unchecked_transaction()?;
    let mut stats = SyncStats::default();

    // Existing rows for this owner, grouped (source, hash) → ids (a multiset:
    // duplicate passages are legal and each id is consumed at most once).
    let owner_param: embral_db::rusqlite::types::Value = match owner {
        Owner::Meeting(id) => (*id).to_string().into(),
        Owner::Dictation(id) => (*id).into(),
    };
    let mut existing: std::collections::HashMap<(String, String), Vec<i64>> = Default::default();
    {
        let mut stmt = tx.prepare(&format!(
            "SELECT id, source, content_hash FROM chunks WHERE {}",
            owner.where_clause()
        ))?;
        let rows = stmt.query_map([&owner_param], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        })?;
        for row in rows {
            let (id, source, hash) = row?;
            existing.entry((source, hash)).or_default().push(id);
        }
    }

    for chunk in built {
        let key = (chunk.source.as_str().to_string(), chunk.content_hash.clone());
        if let Some(id) = existing.get_mut(&key).and_then(Vec::pop) {
            // Same content: keep the row and its embedding; refresh bookkeeping.
            // (No text update; the FTS trigger must not fire.)
            tx.execute(
                "UPDATE chunks SET chunk_index = ?2, start_secs = ?3, end_secs = ?4 WHERE id = ?1",
                params![id, chunk.chunk_index, chunk.start_secs, chunk.end_secs],
            )?;
            stats.kept += 1;
        } else {
            let (meeting_id, dictation_id): (Option<&str>, Option<i64>) = match owner {
                Owner::Meeting(id) => (Some(id), None),
                Owner::Dictation(id) => (None, Some(*id)),
            };
            tx.execute(
                "INSERT INTO chunks (meeting_id, dictation_id, source, chunk_index, text,
                                     embedding_text, start_secs, end_secs, speakers,
                                     speaker_ids, content_hash, embedded_with,
                                     image_filename)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, NULL, ?12)",
                params![
                    meeting_id,
                    dictation_id,
                    chunk.source.as_str(),
                    chunk.chunk_index,
                    chunk.text,
                    chunk.embedding_text,
                    chunk.start_secs,
                    chunk.end_secs,
                    serde_json::to_string(&chunk.speakers)?,
                    serde_json::to_string(&chunk.speaker_ids)?,
                    chunk.content_hash,
                    chunk.image_filename,
                ],
            )?;
            stats.inserted += 1;
        }
    }

    // Whatever wasn't consumed no longer exists in the document.
    for ids in existing.into_values() {
        for id in ids {
            tx.execute("DELETE FROM chunks WHERE id = ?1", [id])?;
            tx.execute("DELETE FROM chunk_vectors WHERE rowid = ?1", [id]).ok();
            stats.deleted += 1;
        }
    }

    tx.commit()?;
    Ok(stats)
}

pub fn sync_meeting(db: &Db, meeting_id: &str) -> Result<SyncStats> {
    let Some(meeting) = db.get_meeting(meeting_id)? else {
        // Deleted meeting: cascade already took the chunks; sweep vectors.
        sweep_vectors(db)?;
        return Ok(SyncStats::default());
    };
    let segments = db.get_segments(meeting_id)?;
    let user_notes = db.get_notes(meeting_id)?;
    let image_text = crate::chunker::referenced_image_text(
        meeting_id,
        &[&user_notes, &meeting.summary],
        &db.image_text(meeting_id)?,
    );
    let built = chunk_meeting(&MeetingDocs {
        title: &meeting.title,
        started_at: meeting.started_at,
        segments: &segments,
        user_notes: &user_notes,
        summary: &meeting.summary,
        transcript: &meeting.transcript,
        image_text: &image_text,
    });
    db.with_conn(|conn| apply(conn, &Owner::Meeting(meeting_id), &built))
}

pub fn sync_dictation(db: &Db, dictation_id: i64) -> Result<SyncStats> {
    db.with_conn(|conn| {
        let row: Option<(String, Option<String>, String)> = conn
            .query_row(
                "SELECT raw_text, cleaned_text, created_at FROM dictations WHERE id = ?1",
                [dictation_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((raw, cleaned, created_at)) = row else {
            return apply(conn, &Owner::Dictation(dictation_id), &[]);
        };
        let created = DateTime::parse_from_rfc3339(&created_at)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());
        let text = cleaned.as_deref().unwrap_or(&raw);
        let built = chunk_dictation(created, text);
        apply(conn, &Owner::Dictation(dictation_id), &built)
    })
}

/// Index owners that have no chunks at all: the first-run backfill and the
/// safety net behind missed hooks. Returns how many owners were synced.
/// Also evicts placeholder chunks an earlier chunker version indexed
/// (idempotent; the current chunker no longer produces them).
pub fn backfill_missing(db: &Db) -> Result<usize> {
    db.with_conn(|conn| {
        let n = conn.execute(
            "DELETE FROM chunks WHERE text = '_No transcript segments were captured._'",
            [],
        )?;
        if n > 0 {
            tracing::info!(removed = n, "evicted placeholder chunks");
        }
        Ok(())
    })?;
    let meeting_ids: Vec<String> = db.with_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id FROM meetings WHERE id NOT IN
               (SELECT meeting_id FROM chunks WHERE meeting_id IS NOT NULL)",
        )?;
        let ids = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(ids)
    })?;
    let dictation_ids: Vec<i64> = db.with_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id FROM dictations WHERE id NOT IN
               (SELECT dictation_id FROM chunks WHERE dictation_id IS NOT NULL)",
        )?;
        let ids = stmt
            .query_map([], |r| r.get::<_, i64>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(ids)
    })?;

    let mut synced = 0;
    for id in &meeting_ids {
        sync_meeting(db, id)?;
        synced += 1;
    }
    for id in &dictation_ids {
        sync_dictation(db, *id)?;
        synced += 1;
    }
    Ok(synced)
}

pub fn sweep_vectors(db: &Db) -> Result<usize> {
    db.with_conn(|conn| SqliteVecIndex.sweep_orphans(conn))
}

pub fn clear_index(db: &Db) -> Result<()> {
    db.with_conn(|conn| {
        conn.execute("DELETE FROM chunks", [])?;
        SqliteVecIndex.clear(conn)
    })
}

pub fn pending_count(db: &Db) -> Result<u64> {
    db.with_conn(|conn| {
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM chunks WHERE embedded_with IS NULL",
            [],
            |r| r.get(0),
        )?;
        Ok(n as u64)
    })
}

/// The chunks most worth embedding next: newest owners first, so a fresh
/// meeting becomes semantically searchable before old backlog.
pub fn next_pending(db: &Db, batch: usize) -> Result<Vec<(i64, String)>> {
    db.with_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT c.id, c.embedding_text FROM chunks c
             LEFT JOIN meetings m ON m.id = c.meeting_id
             LEFT JOIN dictations d ON d.id = c.dictation_id
             WHERE c.embedded_with IS NULL
             ORDER BY COALESCE(m.started_at, d.created_at) DESC, c.id
             LIMIT ?1",
        )?;
        let rows = stmt
            .query_map([batch as i64], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    })
}

/// Record one embedded batch (vectors in, `embedded_with` stamped) in a
/// single short write transaction.
pub fn store_embeddings(db: &Db, rows: &[(i64, Vec<f32>)]) -> Result<()> {
    db.with_conn(|conn| {
        let tx = conn.unchecked_transaction()?;
        for (chunk_id, vector) in rows {
            SqliteVecIndex.upsert(&tx, *chunk_id, vector)?;
            tx.execute(
                "UPDATE chunks SET embedded_with = ?2 WHERE id = ?1",
                params![chunk_id, model::MODEL_ID],
            )?;
        }
        tx.commit()?;
        Ok(())
    })
}

#[cfg(test)]
pub(crate) mod test_support {
    use chrono::TimeZone;
    use embral_db::{Db, MeetingRow};
    use embral_types::TranscriptionSegment;

    pub fn seg(speaker: &str, text: &str, start: f64) -> TranscriptionSegment {
        TranscriptionSegment {
            speaker: Some(speaker.to_string()),
            speaker_id: Some(format!("id-{speaker}")),
            text: text.to_string(),
            start,
            end: start + 5.0,
        }
    }

    pub fn meeting(id: &str, title: &str, day: u32, attendees: &[&str]) -> MeetingRow {
        MeetingRow {
            id: id.to_string(),
            title: title.to_string(),
            started_at: chrono::Utc.with_ymd_and_hms(2026, 6, day, 10, 0, 0).unwrap(),
            duration_seconds: 600,
            summary: String::new(),
            transcript: String::new(),
            attendees: attendees.iter().map(|s| s.to_string()).collect(),
            audio_path: String::new(),
        }
    }

    /// ~250 words opening with `lead`, so each paragraph fills a chunk of
    /// its own (the packer's budget is 400 words, overlap cap 120).
    pub fn long_paragraph(lead: &str) -> String {
        format!("{lead} {}", "and the discussion continued at length. ".repeat(40))
    }

    /// A meeting whose transcript packs into exactly two chunks.
    pub fn seed_two_chunk_meeting(db: &Db, id: &str, title: &str) {
        db.upsert_meeting(&meeting(id, title, 1, &["Alice", "Bob"])).unwrap();
        db.replace_segments(
            id,
            &[
                seg("Alice", &long_paragraph("We reviewed the quarterly budget in detail."), 0.0),
                seg("Bob", &long_paragraph("Hiring plans move to next month."), 20.0),
            ],
        )
        .unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use crate::vector::{stored_vector, META_MODEL_KEY};

    fn embed_all(db: &Db) {
        db.with_conn(|conn| SqliteVecIndex.ensure(conn)).unwrap();
        let pending = next_pending(db, 100).unwrap();
        let rows: Vec<(i64, Vec<f32>)> = pending
            .iter()
            .map(|(id, _)| (*id, vec![*id as f32; model::DIM]))
            .collect();
        store_embeddings(db, &rows).unwrap();
    }

    #[test]
    fn sync_is_incremental_by_content() {
        let db = Db::open_in_memory().unwrap();
        seed_two_chunk_meeting(&db, "m1", "Planning");
        let stats = sync_meeting(&db, "m1").unwrap();
        assert_eq!(stats.inserted, 2);
        assert_eq!(pending_count(&db).unwrap(), 2);

        embed_all(&db);
        assert_eq!(pending_count(&db).unwrap(), 0);

        // Same content re-syncs to a no-op: rows and embeddings survive.
        let stats = sync_meeting(&db, "m1").unwrap();
        assert_eq!(stats, SyncStats { inserted: 0, kept: 2, deleted: 0 });
        assert_eq!(pending_count(&db).unwrap(), 0);

        // One edited paragraph re-pends only itself.
        db.replace_segments(
            "m1",
            &[
                seg("Alice", &long_paragraph("We reviewed the quarterly budget in detail."), 0.0),
                seg("Bob", &long_paragraph("Hiring plans are cancelled outright."), 20.0),
            ],
        )
        .unwrap();
        let stats = sync_meeting(&db, "m1").unwrap();
        assert_eq!(stats, SyncStats { inserted: 1, kept: 1, deleted: 1 });
        assert_eq!(pending_count(&db).unwrap(), 1);

        // A title rename feeds every embedding_text header: all re-pend.
        let mut renamed = meeting("m1", "Renamed Planning", 1, &["Alice", "Bob"]);
        renamed.transcript = String::new();
        db.upsert_meeting(&renamed).unwrap();
        db.replace_segments(
            "m1",
            &[
                seg("Alice", &long_paragraph("We reviewed the quarterly budget in detail."), 0.0),
                seg("Bob", &long_paragraph("Hiring plans are cancelled outright."), 20.0),
            ],
        )
        .unwrap();
        sync_meeting(&db, "m1").unwrap();
        assert_eq!(pending_count(&db).unwrap(), 2);
    }

    #[test]
    fn identity_change_rebuilds_and_repends() {
        let db = Db::open_in_memory().unwrap();
        seed_two_chunk_meeting(&db, "m1", "Planning");
        sync_meeting(&db, "m1").unwrap();
        embed_all(&db);

        let first_id = next_or_any_chunk(&db);
        assert!(db
            .with_conn(|conn| stored_vector(conn, first_id))
            .unwrap()
            .is_some());

        // Pretend a different model wrote the index.
        db.with_conn(|conn| {
            conn.execute(
                "UPDATE meta SET value = 'other-model' WHERE key = ?1",
                [META_MODEL_KEY],
            )?;
            Ok(())
        })
        .unwrap();
        db.with_conn(|conn| SqliteVecIndex.ensure(conn)).unwrap();

        assert_eq!(pending_count(&db).unwrap(), 2);
        assert!(db
            .with_conn(|conn| stored_vector(conn, first_id))
            .unwrap()
            .is_none());
    }

    fn next_or_any_chunk(db: &Db) -> i64 {
        db.with_conn(|conn| {
            Ok(conn.query_row("SELECT id FROM chunks LIMIT 1", [], |r| r.get(0))?)
        })
        .unwrap()
    }

    #[test]
    fn deletes_cascade_and_sweep_cleans_vectors() {
        let db = Db::open_in_memory().unwrap();
        seed_two_chunk_meeting(&db, "m1", "Planning");
        sync_meeting(&db, "m1").unwrap();
        embed_all(&db);

        db.delete_meeting("m1").unwrap();
        // Chunks cascaded; vectors are orphans until swept.
        assert_eq!(pending_count(&db).unwrap(), 0);
        let swept = sweep_vectors(&db).unwrap();
        assert_eq!(swept, 2);
    }

    #[test]
    fn backfill_finds_unindexed_owners() {
        let db = Db::open_in_memory().unwrap();
        seed_two_chunk_meeting(&db, "m1", "Planning");
        let d1 = db.add_dictation("note to self about invoices", None, None).unwrap();

        assert_eq!(backfill_missing(&db).unwrap(), 2);
        assert!(pending_count(&db).unwrap() >= 2);
        // Already-indexed owners aren't re-synced.
        assert_eq!(backfill_missing(&db).unwrap(), 0);

        // A deleted dictation's chunks go when re-synced.
        db.delete_dictation(d1).unwrap();
        sync_dictation(&db, d1).unwrap();
        let dictation_chunks: i64 = db
            .with_conn(|conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*) FROM chunks WHERE dictation_id IS NOT NULL",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(dictation_chunks, 0);
    }

    #[test]
    fn next_pending_prefers_newest_owners() {
        let db = Db::open_in_memory().unwrap();
        db.upsert_meeting(&meeting("old", "Old Meeting", 1, &[])).unwrap();
        db.replace_segments("old", &[seg("A", "old words", 0.0)]).unwrap();
        db.upsert_meeting(&meeting("new", "New Meeting", 20, &[])).unwrap();
        db.replace_segments("new", &[seg("A", "new words", 0.0)]).unwrap();
        backfill_missing(&db).unwrap();

        let pending = next_pending(&db, 10).unwrap();
        assert!(pending[0].1.contains("New Meeting"));
    }
}
