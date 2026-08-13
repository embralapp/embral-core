//! Vector storage behind a trait: sqlite-vec today (owner decision, eyes
//! open on pre-1.0), swappable without touching retrieval. Vectors are
//! always rebuildable from `chunks.embedding_text`, so drop + re-embed is
//! the universal escape hatch.

use anyhow::Result;
use embral_db::rusqlite::{params, Connection, OptionalExtension};
use embral_db::{blob_to_embedding, embedding_to_blob};

use crate::model;

pub const META_MODEL_KEY: &str = "embedding_model";
pub const META_DIM_KEY: &str = "embedding_dim";
const TABLE: &str = "chunk_vectors";

pub trait VectorIndex {
    /// Create the vector table for the current model; a recorded identity
    /// mismatch (different model or dims) drops it and marks every chunk
    /// pending again. App-side only; readers never call this.
    fn ensure(&self, conn: &Connection) -> Result<()>;
    fn upsert(&self, conn: &Connection, chunk_id: i64, vector: &[f32]) -> Result<()>;
    /// Nearest chunks as `(chunk_id, distance)`, best first. A missing
    /// table is the model-less library: empty result, not an error.
    fn knn(&self, conn: &Connection, query: &[f32], k: usize) -> Result<Vec<(i64, f64)>>;
    /// Remove vectors whose chunk row is gone (chunks cascade from their
    /// owners; vec0 has no foreign keys).
    fn sweep_orphans(&self, conn: &Connection) -> Result<usize>;
    fn clear(&self, conn: &Connection) -> Result<()>;
}

pub struct SqliteVecIndex;

fn table_exists(conn: &Connection) -> Result<bool> {
    let found: Option<String> = conn
        .query_row(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [TABLE],
            |r| r.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

fn meta_get(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
        .optional()?)
}

fn meta_set(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

impl VectorIndex for SqliteVecIndex {
    fn ensure(&self, conn: &Connection) -> Result<()> {
        let recorded_model = meta_get(conn, META_MODEL_KEY)?;
        let recorded_dim = meta_get(conn, META_DIM_KEY)?;
        let matches = recorded_model.as_deref() == Some(model::MODEL_ID)
            && recorded_dim.as_deref() == Some(&model::DIM.to_string());

        if table_exists(conn)? && !matches {
            tracing::info!(
                old_model = recorded_model.as_deref().unwrap_or("?"),
                new_model = model::MODEL_ID,
                "embedding identity changed; rebuilding the vector index"
            );
            conn.execute_batch(&format!("DROP TABLE {TABLE}"))?;
            conn.execute("UPDATE chunks SET embedded_with = NULL", [])?;
        }
        conn.execute_batch(&format!(
            "CREATE VIRTUAL TABLE IF NOT EXISTS {TABLE} USING vec0(embedding float[{}])",
            model::DIM
        ))?;
        meta_set(conn, META_MODEL_KEY, model::MODEL_ID)?;
        meta_set(conn, META_DIM_KEY, &model::DIM.to_string())?;
        Ok(())
    }

    fn upsert(&self, conn: &Connection, chunk_id: i64, vector: &[f32]) -> Result<()> {
        // vec0 has no upsert; delete-then-insert is the idiom.
        conn.execute(&format!("DELETE FROM {TABLE} WHERE rowid = ?1"), [chunk_id])?;
        conn.execute(
            &format!("INSERT INTO {TABLE}(rowid, embedding) VALUES (?1, ?2)"),
            params![chunk_id, embedding_to_blob(vector)],
        )?;
        Ok(())
    }

    fn knn(&self, conn: &Connection, query: &[f32], k: usize) -> Result<Vec<(i64, f64)>> {
        if !table_exists(conn)? {
            return Ok(Vec::new());
        }
        let mut stmt = conn.prepare(&format!(
            "SELECT rowid, distance FROM {TABLE}
             WHERE embedding MATCH ?1 AND k = ?2
             ORDER BY distance"
        ))?;
        let rows = stmt
            .query_map(params![embedding_to_blob(query), k as i64], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn sweep_orphans(&self, conn: &Connection) -> Result<usize> {
        if !table_exists(conn)? {
            return Ok(0);
        }
        let n = conn.execute(
            &format!("DELETE FROM {TABLE} WHERE rowid NOT IN (SELECT id FROM chunks)"),
            [],
        )?;
        Ok(n)
    }

    fn clear(&self, conn: &Connection) -> Result<()> {
        if table_exists(conn)? {
            conn.execute(&format!("DELETE FROM {TABLE}"), [])?;
        }
        Ok(())
    }
}

/// Round-trip helper for tests and diagnostics.
pub fn stored_vector(conn: &Connection, chunk_id: i64) -> Result<Option<Vec<f32>>> {
    if !table_exists(conn)? {
        return Ok(None);
    }
    let blob: Option<Vec<u8>> = conn
        .query_row(
            &format!("SELECT embedding FROM {TABLE} WHERE rowid = ?1"),
            [chunk_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(blob.map(|b| blob_to_embedding(&b)))
}
