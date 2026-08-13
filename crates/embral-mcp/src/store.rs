//! Where the library lives and how the server reaches it.
//!
//! The app is the only writer; this process opens the database **read-only,
//! per tool call**: the server outlives the app (clients keep it running),
//! and a held handle would block the app's own storage resets on Windows.

use std::path::PathBuf;

use embral_db::Db;

pub struct Store {
    pub storage_dir: PathBuf,
}

impl Store {
    /// `EMBRAL_STORAGE_DIR` (non-empty) wins, since it's what the `.mcpb`
    /// user_config feeds; else the app's own `config.json` in the default
    /// storage location, else that default itself.
    pub fn from_env() -> Store {
        if let Ok(dir) = std::env::var("EMBRAL_STORAGE_DIR") {
            if !dir.trim().is_empty() {
                return Store {
                    storage_dir: embral_types::resolve_storage_path(dir.trim()),
                };
            }
        }
        let default_base = embral_types::resolve_storage_path(&embral_types::default_storage_dir());
        let configured = std::fs::read_to_string(embral_types::config_path(&default_base))
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|cfg| cfg["storage_dir"].as_str().map(str::to_string))
            .filter(|dir| !dir.trim().is_empty());
        Store {
            storage_dir: configured
                .map(|dir| embral_types::resolve_storage_path(&dir))
                .unwrap_or(default_base),
        }
    }

    pub fn db_path(&self) -> PathBuf {
        self.storage_dir.join("embral.db")
    }

    /// Open the database read-only, refusing any schema this build was
    /// not compiled against. Older means the app hasn't migrated yet
    /// (running it once catches the library up). Newer means the app
    /// moved on while this server kept running (an update installed
    /// mid-session) and this binary's reads can no longer be trusted;
    /// the client has to restart to pick up the new server.
    pub fn open(&self) -> Result<Db, ToolError> {
        let db_path = self.db_path();
        if !db_path.is_file() {
            return Err(ToolError::StorageNotFound { db_path });
        }
        let db = Db::open_read_only(&db_path).map_err(ToolError::Db)?;
        let found = db.schema_version().map_err(ToolError::Db)?;
        check_schema(found, embral_db::latest_schema_version())?;
        Ok(db)
    }
}

/// The schema handshake: this server serves exactly the schema it was
/// built against, in both directions. A mismatch is always someone else's
/// move: the app's (migrate by opening it) or the client's (restart to
/// relaunch the server), and which one is the message's job to say.
pub(crate) fn check_schema(found: i64, expected: i64) -> Result<(), ToolError> {
    if found == expected {
        Ok(())
    } else {
        Err(ToolError::SchemaMismatch { found, expected })
    }
}

/// Tool failures the calling model can react to: rendered as execution
/// results (`is_error`), never protocol errors, in the envelope
/// `{ok:false, error:{code, message}}`.
#[derive(Debug)]
pub enum ToolError {
    StorageNotFound { db_path: PathBuf },
    MeetingNotFound { id: String },
    PassageNotFound { id: i64 },
    ImageNotFound { meeting_id: String, filename: String },
    InvalidArgument { message: String },
    SchemaMismatch { found: i64, expected: i64 },
    Db(anyhow::Error),
}

impl ToolError {
    pub fn code(&self) -> &'static str {
        match self {
            ToolError::StorageNotFound { .. } => "STORAGE_NOT_FOUND",
            ToolError::MeetingNotFound { .. } => "MEETING_NOT_FOUND",
            ToolError::PassageNotFound { .. } => "PASSAGE_NOT_FOUND",
            ToolError::ImageNotFound { .. } => "IMAGE_NOT_FOUND",
            ToolError::InvalidArgument { .. } => "INVALID_ARGUMENT",
            ToolError::SchemaMismatch { .. } => "SCHEMA_MISMATCH",
            ToolError::Db(_) => "DB_ERROR",
        }
    }

    pub fn message(&self) -> String {
        match self {
            ToolError::StorageNotFound { db_path } => format!(
                "No embral library at {} — record a meeting in embral first, \
                 or point EMBRAL_STORAGE_DIR at the storage folder.",
                db_path.display()
            ),
            ToolError::MeetingNotFound { id } => {
                format!("No meeting with id '{id}' — ids come from list_meetings or search_meetings.")
            }
            ToolError::PassageNotFound { id } => format!(
                "No passage {id} — re-run search; passage ids change when a \
                 meeting is edited."
            ),
            ToolError::ImageNotFound { meeting_id, filename } => format!(
                "No image '{filename}' in meeting {meeting_id} — filenames come \
                 from get_meeting's images list or a search hit's `image` field."
            ),
            ToolError::InvalidArgument { message } => message.clone(),
            ToolError::SchemaMismatch { found, expected } => {
                if found < expected {
                    format!(
                        "The library's schema is v{found} but this server expects v{expected} — \
                         open the embral app once to update it, then retry."
                    )
                } else {
                    format!(
                        "The library's schema is v{found} but this server expects v{expected} — \
                         embral was updated while this server kept running. Restart this MCP \
                         client to relaunch the server, then retry."
                    )
                }
            }
            ToolError::Db(e) => format!("Database error: {e:#}"),
        }
    }
}

/// The one embedder this process holds, loaded lazily and retried gently.
/// Every failure shape degrades to `None`: search stays keyword-accurate
/// with no semantic leg, never erroring because a model is missing.
pub struct EmbedderSlot(std::sync::Mutex<SlotState>);

enum SlotState {
    NotLoaded,
    Loaded(Box<embral_embedder::Embedder>),
    /// Load or inference failed; retried after a cooldown so a model
    /// downloaded mid-session gets picked up without a restart.
    Failed(std::time::Instant),
}

const RETRY_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(5 * 60);

impl Default for EmbedderSlot {
    fn default() -> Self {
        EmbedderSlot(std::sync::Mutex::new(SlotState::NotLoaded))
    }
}

impl EmbedderSlot {
    pub fn embed_query(&self, text: &str) -> Option<Vec<f32>> {
        let mut slot = self.0.lock().expect("embedder slot poisoned");
        if let SlotState::Failed(at) = &*slot {
            if at.elapsed() < RETRY_COOLDOWN {
                return None;
            }
            *slot = SlotState::NotLoaded;
        }
        if matches!(&*slot, SlotState::NotLoaded) {
            if !embral_search::model::present() {
                return None;
            }
            match embral_embedder::Embedder::load_default() {
                Ok(embedder) => *slot = SlotState::Loaded(Box::new(embedder)),
                Err(e) => {
                    tracing::warn!("embedding model failed to load: {e:#}");
                    *slot = SlotState::Failed(std::time::Instant::now());
                    return None;
                }
            }
        }
        let SlotState::Loaded(embedder) = &mut *slot else {
            return None;
        };
        match embedder.embed_query(text) {
            Ok(vector) => Some(vector),
            Err(e) => {
                tracing::warn!("query embedding failed: {e:#}");
                *slot = SlotState::Failed(std::time::Instant::now());
                None
            }
        }
    }

    /// For `get_storage_status`: whether semantic search is live right now.
    pub fn loaded(&self) -> bool {
        matches!(&*self.0.lock().expect("embedder slot poisoned"), SlotState::Loaded(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guard_refuses_both_directions() {
        assert!(check_schema(4, 4).is_ok());
        // Older library: the app hasn't migrated it yet.
        let older = check_schema(3, 4).expect_err("older must refuse");
        assert_eq!(older.code(), "SCHEMA_MISMATCH");
        assert!(older.message().contains("open the embral app once"));
        // Newer library: this server outlived an app update.
        let newer = check_schema(5, 4).expect_err("newer must refuse");
        assert_eq!(newer.code(), "SCHEMA_MISMATCH");
        assert!(newer.message().contains("Restart this MCP client"));
    }

    /// The production arrangement in miniature: a real migrated library,
    /// its version doctored each way, opened through `Store::open`.
    #[test]
    fn open_refuses_a_library_from_another_build() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store {
            storage_dir: dir.path().to_path_buf(),
        };
        let latest = embral_db::latest_schema_version();
        let set_version = |v: i64| {
            let db = Db::open(&store.db_path()).unwrap();
            db.with_conn(|conn| {
                conn.execute(
                    "UPDATE meta SET value = ?1 WHERE key = 'schema_version'",
                    [v.to_string()],
                )?;
                Ok(())
            })
            .unwrap();
        };

        // Freshly migrated: opens.
        drop(Db::open(&store.db_path()).unwrap());
        assert!(store.open().is_ok());

        // The app moved on past this binary (an update mid-session).
        set_version(latest + 1);
        match store.open() {
            Err(ToolError::SchemaMismatch { found, expected }) => {
                assert_eq!(found, latest + 1);
                assert_eq!(expected, latest);
            }
            Err(e) => panic!("expected a newer-schema refusal, got {e:?}"),
            Ok(_) => panic!("a newer library must refuse to open"),
        }

        // A library the app hasn't migrated yet.
        set_version(latest - 1);
        match store.open() {
            Err(ToolError::SchemaMismatch { found, .. }) => assert_eq!(found, latest - 1),
            Err(e) => panic!("expected an older-schema refusal, got {e:?}"),
            Ok(_) => panic!("an older library must refuse to open"),
        }
    }
}
