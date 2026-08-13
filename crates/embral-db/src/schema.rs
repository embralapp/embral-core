//! Schema creation and versioned migrations.
//!
//! `meta.schema_version` records the applied version; migrations are plain
//! SQL batches applied in order inside a transaction. Adding a table/column
//! later = append a new entry to `MIGRATIONS`.

use anyhow::Result;
use rusqlite::Connection;

const MIGRATIONS: &[&str] = &[
    // v1. Initial schema: meetings + segments + external-content FTS index.
    r#"
    CREATE TABLE meetings (
        id               TEXT PRIMARY KEY,
        title            TEXT NOT NULL,
        started_at       TEXT NOT NULL,
        duration_seconds INTEGER NOT NULL,
        notes_md         TEXT NOT NULL DEFAULT '',
        transcript_md    TEXT NOT NULL DEFAULT '',
        attendees        TEXT NOT NULL DEFAULT '[]',
        audio_path       TEXT NOT NULL DEFAULT '',
        notes_path       TEXT NOT NULL DEFAULT '',
        transcript_path  TEXT NOT NULL DEFAULT '',
        created_at       TEXT NOT NULL,
        updated_at       TEXT NOT NULL
    );

    CREATE INDEX idx_meetings_started_at ON meetings(started_at DESC);

    CREATE TABLE segments (
        id         INTEGER PRIMARY KEY,
        meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
        idx        INTEGER NOT NULL,
        speaker    TEXT,
        text       TEXT NOT NULL,
        start_secs REAL NOT NULL,
        end_secs   REAL NOT NULL,
        UNIQUE(meeting_id, idx)
    );

    CREATE VIRTUAL TABLE meetings_fts USING fts5(
        title, notes_md, transcript_md,
        content='meetings', content_rowid='rowid'
    );

    CREATE TRIGGER meetings_ai AFTER INSERT ON meetings BEGIN
        INSERT INTO meetings_fts(rowid, title, notes_md, transcript_md)
        VALUES (new.rowid, new.title, new.notes_md, new.transcript_md);
    END;

    CREATE TRIGGER meetings_ad AFTER DELETE ON meetings BEGIN
        INSERT INTO meetings_fts(meetings_fts, rowid, title, notes_md, transcript_md)
        VALUES ('delete', old.rowid, old.title, old.notes_md, old.transcript_md);
    END;

    CREATE TRIGGER meetings_au AFTER UPDATE ON meetings BEGIN
        INSERT INTO meetings_fts(meetings_fts, rowid, title, notes_md, transcript_md)
        VALUES ('delete', old.rowid, old.title, old.notes_md, old.transcript_md);
        INSERT INTO meetings_fts(rowid, title, notes_md, transcript_md)
        VALUES (new.rowid, new.title, new.notes_md, new.transcript_md);
    END;
    "#,
    // v2. Speaker registry: known people, their voice-reference embeddings,
    // a registry link per segment, and per-meeting match suggestions.
    r#"
    CREATE TABLE speakers (
        id         TEXT PRIMARY KEY,
        name       TEXT NOT NULL,
        emails     TEXT NOT NULL DEFAULT '[]',
        notes      TEXT NOT NULL DEFAULT '',
        is_you     INTEGER NOT NULL DEFAULT 0,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    );

    CREATE TABLE voice_refs (
        id                INTEGER PRIMARY KEY,
        speaker_id        TEXT NOT NULL REFERENCES speakers(id) ON DELETE CASCADE,
        kind              TEXT NOT NULL,
        slot              INTEGER,
        embedding         BLOB NOT NULL,
        dim               INTEGER NOT NULL,
        clip_path         TEXT,
        source_meeting_id TEXT,
        created_at        TEXT NOT NULL
    );

    CREATE INDEX idx_voice_refs_speaker ON voice_refs(speaker_id);

    ALTER TABLE segments ADD COLUMN speaker_id TEXT;
    ALTER TABLE meetings ADD COLUMN speaker_suggestions TEXT NOT NULL DEFAULT '[]';
    "#,
    // v3. Timestamped chapters on meetings + dictation history.
    r#"
    ALTER TABLE meetings ADD COLUMN chapters_json TEXT NOT NULL DEFAULT '[]';

    CREATE TABLE dictations (
        id           INTEGER PRIMARY KEY,
        raw_text     TEXT NOT NULL,
        cleaned_text TEXT,
        app          TEXT,
        created_at   TEXT NOT NULL
    );
    "#,
    // v4. User-starred moments replace AI chapters (existing chapter data
    // is discarded deliberately; pre-release no-compat).
    r#"
    ALTER TABLE meetings DROP COLUMN chapters_json;
    ALTER TABLE meetings ADD COLUMN stars_json TEXT NOT NULL DEFAULT '[]';
    "#,
    // v5. The user's raw live notes, stored verbatim for the Notes tab
    // (previously they only existed appended inside the summary document).
    r#"
    ALTER TABLE meetings ADD COLUMN user_notes TEXT NOT NULL DEFAULT '';
    "#,
    // v6. Speaker emails are gone. Nothing ever read them: matching is
    // embedding-only and meeting attendees are display names. They were a hook
    // for calendar matching, which is not built and not planned.
    r#"
    ALTER TABLE speakers DROP COLUMN emails;
    "#,
    // v7. The chunk-level retrieval index: passages over transcripts, user
    // notes, summaries, and dictations (embral-search owns the SQL that
    // fills and queries these). The vec0 vector table is deliberately not
    // here: its dimensions belong to the embedding model, so embral-search
    // creates and versions it outside migrations (meta keys
    // embedding_model / embedding_dim).
    r#"
    CREATE TABLE chunks (
        id             INTEGER PRIMARY KEY,
        meeting_id     TEXT REFERENCES meetings(id) ON DELETE CASCADE,
        dictation_id   INTEGER REFERENCES dictations(id) ON DELETE CASCADE,
        source         TEXT NOT NULL,
        chunk_index    INTEGER NOT NULL,
        text           TEXT NOT NULL,
        embedding_text TEXT NOT NULL,
        start_secs     REAL,
        end_secs       REAL,
        speakers       TEXT NOT NULL DEFAULT '[]',
        speaker_ids    TEXT NOT NULL DEFAULT '[]',
        content_hash   TEXT NOT NULL,
        embedded_with  TEXT,
        CHECK ((meeting_id IS NULL) != (dictation_id IS NULL))
    );

    CREATE INDEX idx_chunks_meeting   ON chunks(meeting_id);
    CREATE INDEX idx_chunks_dictation ON chunks(dictation_id);
    CREATE INDEX idx_chunks_pending   ON chunks(embedded_with) WHERE embedded_with IS NULL;

    CREATE VIRTUAL TABLE chunks_fts USING fts5(
        text, content='chunks', content_rowid='id'
    );

    CREATE TRIGGER chunks_ai AFTER INSERT ON chunks BEGIN
        INSERT INTO chunks_fts(rowid, text) VALUES (new.id, new.text);
    END;

    CREATE TRIGGER chunks_ad AFTER DELETE ON chunks BEGIN
        INSERT INTO chunks_fts(chunks_fts, rowid, text)
        VALUES ('delete', old.id, old.text);
    END;

    -- Column-scoped, unlike meetings_au: the indexer's chunk_index and
    -- embedded_with updates must not re-tokenize FTS; changed content
    -- arrives as delete+insert.
    CREATE TRIGGER chunks_au AFTER UPDATE OF text ON chunks BEGIN
        INSERT INTO chunks_fts(chunks_fts, rowid, text)
        VALUES ('delete', old.id, old.text);
        INSERT INTO chunks_fts(rowid, text) VALUES (new.id, new.text);
    END;
    "#,
    // v8. The meeting-level FTS index dies: both former consumers (the
    // palette, the MCP server) now search chunk passages, and nothing reads
    // meetings_fts anymore. This also retires its au trigger, which
    // re-tokenized all three columns on every meetings UPDATE.
    r#"
    DROP TRIGGER meetings_ai;
    DROP TRIGGER meetings_ad;
    DROP TRIGGER meetings_au;
    DROP TABLE meetings_fts;
    "#,
    // v9. Voice matching is gone: no more voice-reference embeddings,
    // "sounds like" suggestions, or the is_you flag (whose only consumer was
    // the mic-dominance "you" prior). Diarization, the registry of named
    // people, and segment speaker links all stay.
    r#"
    DROP TABLE voice_refs;
    ALTER TABLE meetings DROP COLUMN speaker_suggestions;
    ALTER TABLE speakers DROP COLUMN is_you;
    "#,
    // v10. Pending notes-based name suggestions ("Speaker 1 looks like
    // John", derived from the user's typed notes) as a per-meeting JSON
    // array, kept until confirmed or dismissed.
    r#"
    ALTER TABLE meetings ADD COLUMN name_suggestions TEXT NOT NULL DEFAULT '[]';
    "#,
    // v11. The summary and transcript documents live in the database only.
    // The markdown files they pointed at were generated exports nothing read
    // (integrations.md), and keeping them meant renaming, deleting and
    // pruning two files at five lifecycle sites. The files already on disk
    // are left where they are; the markdown export owns that job now.
    r#"
    ALTER TABLE meetings DROP COLUMN notes_path;
    ALTER TABLE meetings DROP COLUMN transcript_path;
    "#,
    // v12. A meeting's three documents get their real names: `summary`
    // (what the LLM wrote), `notes` (what the user typed) and `transcript`.
    // `notes_md` used to hold the summary while the user's own typing
    // lived in `user_notes`, so "notes" meant opposite things either side of
    // the command layer: `update_meeting_notes` wrote the summary, and the
    // MCP server translated `notes_md` into `summary_md` on the way out. The
    // `_md` suffix goes with them: all three are markdown, so it
    // distinguished nothing.
    //
    // `notes` deliberately does not become `notes_md`. Reusing the
    // old name for a different document would let any missed reference in
    // a hand-written SQL string read the wrong one in silence; as it stands
    // a stale `notes_md` fails loudly with "no such column".
    r#"
    ALTER TABLE meetings RENAME COLUMN notes_md TO summary;
    ALTER TABLE meetings RENAME COLUMN user_notes TO notes;
    ALTER TABLE meetings RENAME COLUMN transcript_md TO transcript;
    "#,
    // v13. The text OCR read out of a pasted image. It belongs to an image
    // rather than to a document, so it cannot live on `meetings`, and
    // deriving it on the fly would re-OCR the whole library on every index
    // sync. `ocr_engine NULL` means "not read yet", the same idiom as
    // `chunks.embedded_with`.
    //
    // A recorded engine mismatch does not re-OCR, unlike the vector
    // index's model mismatch. There the old data is unusable: a vector
    // from another model does not live in the same space. Here it is a
    // string, and a string Vision produced reads perfectly well on Windows.
    //
    // Rows appear only once the meeting row does. During a live recording
    // there is none (finalize creates it), so this key would reject the
    // insert, which is also why OCR never runs while the recording does.
    r#"
    CREATE TABLE image_text (
        meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
        filename   TEXT NOT NULL,
        ocr_text   TEXT NOT NULL DEFAULT '',
        ocr_engine TEXT,
        PRIMARY KEY (meeting_id, filename)
    );
    "#,
    // v14. Which image an `image_text` chunk was read out of, so a search
    // hit can point at it. NULL for every other source, which is most rows.
    //
    // The passage's own text is not enough to find the image: it lives
    // inside a PNG, so unlike a notes or summary hit there is nothing in the
    // document to scroll to. This is the column 15b deliberately left out,
    // on the grounds that a jump-to-the-image feature would be the thing
    // that earned it.
    r#"
    ALTER TABLE chunks ADD COLUMN image_filename TEXT;
    "#,
    // v15. Link name-only segments to the registry. Names arrived through
    // paths that never set `speaker_id` (chiefly the notes-naming pass,
    // which links only to profiles that already existed), so a person's
    // history could be split between linked segments and plain-text
    // look-alikes. From here on the app adopts strays whenever a profile is
    // created, renamed, or merged into; this backfills everything from
    // before that rule. `MIN(s.id)` makes a duplicate-name registry
    // deterministic; duplicates are exactly what merge then fixes. A
    // profile named like a generic "Speaker N" label adopts nothing (the
    // GLOB, slightly wider than the Rust-side check): those labels are
    // per-meeting placeholders, and two meetings' "Speaker 2" are
    // different people.
    r#"
    UPDATE segments SET speaker_id =
        (SELECT MIN(s.id) FROM speakers s
          WHERE lower(s.name) = lower(segments.speaker)
            AND s.name NOT GLOB 'Speaker [0-9]*')
    WHERE speaker_id IS NULL AND speaker IS NOT NULL
      AND EXISTS (SELECT 1 FROM speakers s
                   WHERE lower(s.name) = lower(segments.speaker)
                     AND s.name NOT GLOB 'Speaker [0-9]*');
    "#,
];

#[cfg(test)]
mod tests {
    use super::*;

    /// A database created at v1 (real R1-era schema, with data) migrates to
    /// the current version with v2 tables/columns usable and old rows intact.
    #[test]
    fn v1_database_upgrades_in_place() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
            .unwrap();
        conn.execute_batch(MIGRATIONS[0]).unwrap();
        conn.execute(
            "INSERT INTO meta(key, value) VALUES ('schema_version', '1')",
            [],
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO meetings (id, title, started_at, duration_seconds, created_at, updated_at)
             VALUES ('m1', 'Old', '2026-01-01T00:00:00Z', 60, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
             INSERT INTO segments (meeting_id, idx, speaker, text, start_secs, end_secs)
             VALUES ('m1', 0, 'Speaker 1', 'hi', 0.0, 1.0);",
        )
        .unwrap();

        migrate(&conn).unwrap();

        let version: String = conn
            .query_row("SELECT value FROM meta WHERE key = 'schema_version'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len().to_string());
        // Old data intact; the v2 registry link defaults to NULL.
        let speaker_id: Option<String> = conn
            .query_row("SELECT speaker_id FROM segments", [], |r| r.get(0))
            .unwrap();
        assert_eq!(speaker_id, None);
        // New tables usable.
        conn.execute(
            "INSERT INTO speakers (id, name, created_at, updated_at)
             VALUES ('sp', 'Alice', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
    }

    /// v6 drops speaker emails. A registry that has emails must survive the
    /// drop with its people intact: the column goes, nobody goes with it.
    #[test]
    fn v6_drops_emails_and_keeps_the_people() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
            .unwrap();
        // Stand the database up at v5, the version before the drop.
        for migration in &MIGRATIONS[..5] {
            conn.execute_batch(migration).unwrap();
        }
        conn.execute(
            "INSERT INTO meta(key, value) VALUES ('schema_version', '5')",
            [],
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO speakers (id, name, emails, notes, is_you, created_at, updated_at)
             VALUES ('sp_a', 'Alice', '[\"alice@example.com\"]', 'note', 1,
                     '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');",
        )
        .unwrap();

        migrate(&conn).unwrap();

        let version: String = conn
            .query_row("SELECT value FROM meta WHERE key = 'schema_version'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len().to_string());

        // The person survived, with everything that was not an email.
        let (name, notes): (String, String) = conn
            .query_row(
                "SELECT name, notes FROM speakers WHERE id = 'sp_a'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(name, "Alice");
        assert_eq!(notes, "note");

        // And the column is really gone.
        assert!(conn
            .query_row("SELECT emails FROM speakers", [], |r| r.get::<_, String>(0))
            .is_err());
    }

    /// v9 drops voice matching. A registry with voice references, pending
    /// suggestions, and a "you" flag keeps its people and segment links; only
    /// the matching machinery disappears.
    #[test]
    fn v9_drops_voice_matching_and_keeps_the_registry() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
            .unwrap();
        // Stand the database up at v8, the version before the drop.
        for migration in &MIGRATIONS[..8] {
            conn.execute_batch(migration).unwrap();
        }
        conn.execute(
            "INSERT INTO meta(key, value) VALUES ('schema_version', '8')",
            [],
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO speakers (id, name, notes, is_you, created_at, updated_at)
             VALUES ('sp_a', 'Alice', 'note', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
             INSERT INTO voice_refs (speaker_id, kind, slot, embedding, dim, created_at)
             VALUES ('sp_a', 'enrolled', 0, x'00000000', 1, '2026-01-01T00:00:00Z');
             INSERT INTO meetings (id, title, started_at, duration_seconds, speaker_suggestions, created_at, updated_at)
             VALUES ('m1', 'Old', '2026-01-01T00:00:00Z', 60,
                     '[{\"label\":\"Speaker 1\",\"speaker_id\":\"sp_a\"}]',
                     '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
             INSERT INTO segments (meeting_id, idx, speaker, speaker_id, text, start_secs, end_secs)
             VALUES ('m1', 0, 'Alice', 'sp_a', 'hi', 0.0, 1.0);",
        )
        .unwrap();

        migrate(&conn).unwrap();

        // The person and the segment's registry link survived.
        let name: String = conn
            .query_row("SELECT name FROM speakers WHERE id = 'sp_a'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(name, "Alice");
        let speaker_id: Option<String> = conn
            .query_row("SELECT speaker_id FROM segments", [], |r| r.get(0))
            .unwrap();
        assert_eq!(speaker_id, Some("sp_a".into()));

        // The matching machinery is really gone.
        assert!(conn
            .query_row("SELECT COUNT(*) FROM voice_refs", [], |r| r.get::<_, i64>(0))
            .is_err());
        assert!(conn
            .query_row("SELECT speaker_suggestions FROM meetings", [], |r| r
                .get::<_, String>(0))
            .is_err());
        assert!(conn
            .query_row("SELECT is_you FROM speakers", [], |r| r.get::<_, i64>(0))
            .is_err());
    }

    /// v12 renames all three documents to what they actually are. Each must
    /// arrive under its new name with its content unmoved: the whole hazard
    /// of this migration is the summary and the notes swapping places.
    #[test]
    fn v12_renames_the_documents_without_swapping_them() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
            .unwrap();
        for migration in &MIGRATIONS[..11] {
            conn.execute_batch(migration).unwrap();
        }
        conn.execute(
            "INSERT INTO meta(key, value) VALUES ('schema_version', '11')",
            [],
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO meetings (id, title, started_at, duration_seconds, notes_md,
                                   user_notes, transcript_md, created_at, updated_at)
             VALUES ('m1', 'Old', '2026-01-01T00:00:00Z', 60,
                     'THE SUMMARY', 'WHAT I TYPED', 'WHO SAID WHAT',
                     '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');",
        )
        .unwrap();

        migrate(&conn).unwrap();

        let (summary, notes, transcript): (String, String, String) = conn
            .query_row(
                "SELECT summary, notes, transcript FROM meetings WHERE id = 'm1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(summary, "THE SUMMARY");
        assert_eq!(notes, "WHAT I TYPED");
        assert_eq!(transcript, "WHO SAID WHAT");

        // The old names are gone, so a missed reference fails loudly rather
        // than silently reading the other document.
        for gone in ["notes_md", "user_notes", "transcript_md"] {
            assert!(conn
                .query_row(&format!("SELECT {gone} FROM meetings"), [], |r| r
                    .get::<_, String>(0))
                .is_err());
        }
    }

    /// v14 records which image a chunk was read out of. Chunks already in
    /// the index must survive it: they are expensive to rebuild, since
    /// every one of them carries an embedding.
    #[test]
    fn v14_adds_the_image_filename_without_disturbing_existing_chunks() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
            .unwrap();
        for migration in &MIGRATIONS[..13] {
            conn.execute_batch(migration).unwrap();
        }
        conn.execute(
            "INSERT INTO meta(key, value) VALUES ('schema_version', '13')",
            [],
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO meetings (id, title, started_at, duration_seconds,
                                   created_at, updated_at)
             VALUES ('m1', 'Planning', '2026-01-01T00:00:00Z', 60,
                     '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
             INSERT INTO chunks (meeting_id, source, chunk_index, text, embedding_text,
                                 speakers, speaker_ids, content_hash, embedded_with)
             VALUES ('m1', 'user_notes', 0, 'what I typed', 'header\nwhat I typed',
                     '[]', '[]', 'abc123', 'embedding-multilingual');",
        )
        .unwrap();

        migrate(&conn).unwrap();

        // The existing chunk kept its text and its embedding stamp: a
        // migration that re-pends the index would re-embed the library.
        let (text, embedded, image): (String, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT text, embedded_with, image_filename FROM chunks",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(text, "what I typed");
        assert_eq!(embedded.as_deref(), Some("embedding-multilingual"));
        assert_eq!(image, None, "non-image sources name no image");
    }

    /// v15 links name-only segments to the registry profile whose name they
    /// carry: the backfill behind adopt-on-write. A segment already linked,
    /// or whose label the registry does not know, is left alone.
    #[test]
    fn v15_links_name_only_segments_to_their_profiles() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
            .unwrap();
        for migration in &MIGRATIONS[..14] {
            conn.execute_batch(migration).unwrap();
        }
        conn.execute(
            "INSERT INTO meta(key, value) VALUES ('schema_version', '14')",
            [],
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO meetings (id, title, started_at, duration_seconds,
                                   created_at, updated_at)
             VALUES ('m1', 'Planning', '2026-01-01T00:00:00Z', 60,
                     '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
             INSERT INTO speakers (id, name, created_at, updated_at)
             VALUES ('sp_dana', 'Dana Smith',
                     '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z'),
                    ('sp_gen', 'Speaker 2',
                     '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
             INSERT INTO segments (meeting_id, idx, speaker, speaker_id,
                                   text, start_secs, end_secs)
             VALUES ('m1', 0, 'dana smith', NULL,   'notes-named, case aside', 0, 1),
                    ('m1', 1, 'Dana Smith', 'sp_x', 'already linked',          1, 2),
                    ('m1', 2, 'Speaker 2',  NULL,   'a generic label',         2, 3),
                    ('m1', 3, 'Speaker 9',  NULL,   'unknown to the registry', 3, 4);",
        )
        .unwrap();

        migrate(&conn).unwrap();

        let links: Vec<Option<String>> = conn
            .prepare("SELECT speaker_id FROM segments ORDER BY idx")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        // The generic-named profile adopts nothing: "Speaker 2" is a
        // per-meeting placeholder, not a person.
        assert_eq!(
            links,
            vec![Some("sp_dana".into()), Some("sp_x".into()), None, None]
        );
    }

    /// v13 adds the per-image OCR text. Existing meetings must survive, and
    /// the rows must go when their meeting does: the asset directory is
    /// already pruned on delete, and a stale row would keep answering
    /// searches for a meeting that no longer exists.
    #[test]
    fn v13_adds_image_text_and_it_dies_with_its_meeting() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
            .unwrap();
        for migration in &MIGRATIONS[..12] {
            conn.execute_batch(migration).unwrap();
        }
        conn.execute(
            "INSERT INTO meta(key, value) VALUES ('schema_version', '12')",
            [],
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO meetings (id, title, started_at, duration_seconds, summary,
                                   created_at, updated_at)
             VALUES ('m1', 'Planning', '2026-01-01T00:00:00Z', 60, 'THE SUMMARY',
                     '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');",
        )
        .unwrap();

        migrate(&conn).unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON").unwrap();

        // The meeting came through the migration untouched.
        let summary: String = conn
            .query_row("SELECT summary FROM meetings WHERE id = 'm1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(summary, "THE SUMMARY");

        conn.execute_batch(
            "INSERT INTO image_text (meeting_id, filename, ocr_text, ocr_engine)
             VALUES ('m1', 'img-01.png', 'Q3 revenue 4.2M', 'windows');",
        )
        .unwrap();
        // Not read yet: the engine column is what says so.
        conn.execute_batch(
            "INSERT INTO image_text (meeting_id, filename) VALUES ('m1', 'img-02.png');",
        )
        .unwrap();
        let pending: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM image_text WHERE ocr_engine IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pending, 1);

        conn.execute("DELETE FROM meetings WHERE id = 'm1'", []).unwrap();
        let left: i64 = conn
            .query_row("SELECT COUNT(*) FROM image_text", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0, "image text must cascade with its meeting");
    }

    /// v11 drops the two markdown export paths. The documents themselves
    /// must come through untouched: the paths went, the meetings did not.
    #[test]
    fn v11_drops_the_export_paths_and_keeps_the_documents() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
            .unwrap();
        for migration in &MIGRATIONS[..10] {
            conn.execute_batch(migration).unwrap();
        }
        conn.execute(
            "INSERT INTO meta(key, value) VALUES ('schema_version', '10')",
            [],
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO meetings (id, title, started_at, duration_seconds, notes_md,
                                   transcript_md, notes_path, transcript_path, audio_path,
                                   created_at, updated_at)
             VALUES ('m1', 'Old', '2026-01-01T00:00:00Z', 60, '# Old', 'said a thing',
                     'notes/old.md', 'transcripts/old.md', 'audio/old.mp3',
                     '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');",
        )
        .unwrap();

        migrate(&conn).unwrap();

        // `migrate` always runs to head, so v12 has renamed the documents by
        // the time we look.
        let (notes, transcript, audio): (String, String, String) = conn
            .query_row(
                "SELECT summary, transcript, audio_path FROM meetings WHERE id = 'm1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(notes, "# Old");
        assert_eq!(transcript, "said a thing");
        // Audio keeps its file, and so its path: it is not an export.
        assert_eq!(audio, "audio/old.mp3");

        for gone in ["notes_path", "transcript_path"] {
            assert!(conn
                .query_row(&format!("SELECT {gone} FROM meetings"), [], |r| r
                    .get::<_, String>(0))
                .is_err());
        }
    }

    /// v10 adds the pending name-suggestion column with an empty default.
    #[test]
    fn v10_adds_name_suggestions_with_empty_default() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
            .unwrap();
        for migration in &MIGRATIONS[..9] {
            conn.execute_batch(migration).unwrap();
        }
        conn.execute(
            "INSERT INTO meta(key, value) VALUES ('schema_version', '9')",
            [],
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO meetings (id, title, started_at, duration_seconds, created_at, updated_at)
             VALUES ('m1', 'Old', '2026-01-01T00:00:00Z', 60, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');",
        )
        .unwrap();

        migrate(&conn).unwrap();

        let suggestions: String = conn
            .query_row("SELECT name_suggestions FROM meetings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(suggestions, "[]");
    }
}

/// The version this build writes: what `meta.schema_version` becomes after
/// [`migrate`].
pub fn latest_version() -> i64 {
    MIGRATIONS.len() as i64
}

pub fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
    )?;
    let current: i64 = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .map(|v| v.parse().unwrap_or(0))
        .unwrap_or(0);

    for (i, sql) in MIGRATIONS.iter().enumerate() {
        let version = (i + 1) as i64;
        if version <= current {
            continue;
        }
        conn.execute_batch("BEGIN")?;
        let applied = conn.execute_batch(sql).and_then(|()| {
            conn.execute(
                "INSERT INTO meta(key, value) VALUES ('schema_version', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [version.to_string()],
            )
            .map(|_| ())
        });
        match applied {
            Ok(()) => conn.execute_batch("COMMIT")?,
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(anyhow::anyhow!("migration to v{version} failed: {e}"));
            }
        }
    }
    Ok(())
}
