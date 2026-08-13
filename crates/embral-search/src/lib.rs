//! The retrieval engine: passage-level chunking of meetings and dictations,
//! an incremental embedding index (sqlite-vec behind [`vector::VectorIndex`]),
//! and hybrid FTS+vector search with rank fusion.
//!
//! This crate is synchronous and Tauri-free. All SQL runs through
//! [`embral_db::Db::with_conn`]: the schema (v7 `chunks` + `chunks_fts`)
//! lives in embral-db, the queries live here. The app is the only writer;
//! the MCP server queries the same file read-only
//! ([integrations.md](../../../docs/integrations.md)).

pub mod chunker;
pub mod index;
pub mod model;
pub mod retrieval;
pub mod vector;

#[cfg(test)]
mod bench;

pub use chunker::{chunk_dictation, chunk_meeting, BuiltChunk, MeetingDocs, Source};
pub use index::{
    backfill_missing, clear_index, next_pending, pending_count, store_embeddings, sweep_vectors,
    sync_dictation, sync_meeting, SyncStats,
};
pub use retrieval::{search, Hit, Mode, OwnerKind, SearchArgs};
pub use vector::{SqliteVecIndex, VectorIndex};
