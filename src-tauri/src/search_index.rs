//! Keeping the retrieval index true to the library, from the app side:
//! chunk+FTS sync runs inline in every text-mutation path (cheap, never
//! fails a save), while embeddings run in a background worker. The app
//! cannot link ort (sherpa's /MT onnxruntime is already in this exe), so
//! inference happens in a spawned `embral-mcp embed` child speaking JSON
//! lines over stdio; all database writes stay here.

use std::time::{Duration, Instant};

use embral_db::Db;
use embral_search::{SqliteVecIndex, VectorIndex};
use serde_json::json;
use tauri::Manager;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::AppState;

/// Kill the embed child after this long unused; loading again costs ~1 s.
const IDLE_EVICT: Duration = Duration::from_secs(10 * 60);
/// The worker's fallback tick when no mutation notifies it.
const FALLBACK_TICK: Duration = Duration::from_secs(5 * 60);
const EMBED_BATCH: usize = 8;
const READY_TIMEOUT: Duration = Duration::from_secs(60);
const REPLY_TIMEOUT: Duration = Duration::from_secs(120);

pub struct SearchRuntime {
    pipe: tokio::sync::Mutex<Option<EmbedPipe>>,
    last_used: std::sync::Mutex<Instant>,
    /// Mutation paths ping this; the worker drains pending embeddings.
    pub notify: tokio::sync::Notify,
}

impl Default for SearchRuntime {
    fn default() -> Self {
        SearchRuntime {
            pipe: tokio::sync::Mutex::new(None),
            last_used: std::sync::Mutex::new(Instant::now()),
            notify: tokio::sync::Notify::new(),
        }
    }
}

impl SearchRuntime {
    fn touch(&self) {
        *self.last_used.lock().expect("last_used poisoned") = Instant::now();
    }

    fn idle_for(&self) -> Duration {
        self.last_used.lock().expect("last_used poisoned").elapsed()
    }

    /// Kill the embed child (model delete needs its file handles gone; the
    /// idle evictor wants its ~200 MB back).
    pub async fn shutdown(&self) {
        if let Some(mut pipe) = self.pipe.lock().await.take() {
            let _ = pipe.child.start_kill();
        }
    }

    /// Best-effort sync shutdown for the app-exit path. An orphaned child
    /// also exits on its own when its stdin pipe closes with us.
    pub fn shutdown_blocking(&self) {
        if let Ok(mut guard) = self.pipe.try_lock() {
            if let Some(mut pipe) = guard.take() {
                let _ = pipe.child.start_kill();
            }
        }
    }

    pub async fn embed_query(&self, text: &str) -> Result<Vec<f32>, String> {
        let reply = self.request(json!({ "query": text })).await?;
        serde_json::from_value(reply["vector"].clone())
            .map_err(|e| format!("bad vector from the embed worker: {e}"))
    }

    pub async fn embed_passages(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        let reply = self.request(json!({ "passages": texts })).await?;
        serde_json::from_value(reply["vectors"].clone())
            .map_err(|e| format!("bad vectors from the embed worker: {e}"))
    }

    /// Whether the embed child is currently alive (the palette uses this to
    /// decide "warm enough to wait for" vs "start a warm-up, answer FTS-only").
    pub async fn is_warm(&self) -> bool {
        self.pipe.lock().await.is_some()
    }

    /// Spawn the child in the background so a keystroke never waits on a
    /// model load.
    pub fn warm_up(handle: tauri::AppHandle) {
        if !embral_search::model::present() {
            return;
        }
        tauri::async_runtime::spawn(async move {
            let state = handle.state::<AppState>();
            let mut guard = state.search.pipe.lock().await;
            if guard.is_none() {
                match EmbedPipe::spawn().await {
                    Ok(pipe) => *guard = Some(pipe),
                    Err(e) => tracing::warn!("embed worker warm-up failed: {e}"),
                }
            }
        });
    }

    async fn request(&self, body: serde_json::Value) -> Result<serde_json::Value, String> {
        self.touch();
        let mut guard = self.pipe.lock().await;
        if guard.is_none() {
            *guard = Some(EmbedPipe::spawn().await?);
        }
        let pipe = guard.as_mut().expect("just spawned");
        match pipe.request(&body).await {
            Ok(reply) => Ok(reply),
            Err(e) => {
                // A broken pipe stays broken; drop it so the next call
                // starts fresh.
                if let Some(mut dead) = guard.take() {
                    let _ = dead.child.start_kill();
                }
                Err(e)
            }
        }
    }
}

struct EmbedPipe {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    stdout: BufReader<tokio::process::ChildStdout>,
}

impl EmbedPipe {
    async fn spawn() -> Result<EmbedPipe, String> {
        let (path, exists) = crate::mcp_clients::server_binary()?;
        if !exists {
            return Err(format!(
                "the embral-mcp binary isn't built (expected at {})",
                path.display()
            ));
        }
        let mut cmd = tokio::process::Command::new(&path);
        cmd.arg("embed")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        crate::platform::hide_console_tokio(&mut cmd);
        crate::platform::prepare_spawn_tokio(&mut cmd);
        let mut child = cmd.spawn().map_err(|e| format!("spawn embed worker: {e}"))?;
        if let Some(pid) = child.id() {
            crate::platform::watch_child(pid);
        }
        let stdin = child.stdin.take().ok_or("no stdin pipe")?;
        let stdout = BufReader::new(child.stdout.take().ok_or("no stdout pipe")?);
        let mut pipe = EmbedPipe { child, stdin, stdout };

        let ready = pipe.read_line(READY_TIMEOUT).await?;
        if ready["ready"] != true {
            let _ = pipe.child.start_kill();
            return Err(ready["error"]
                .as_str()
                .unwrap_or("embed worker failed to start")
                .to_string());
        }
        Ok(pipe)
    }

    async fn request(&mut self, body: &serde_json::Value) -> Result<serde_json::Value, String> {
        let mut line = body.to_string();
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("write to embed worker: {e}"))?;
        self.stdin
            .flush()
            .await
            .map_err(|e| format!("flush to embed worker: {e}"))?;
        let reply = self.read_line(REPLY_TIMEOUT).await?;
        if let Some(error) = reply["error"].as_str() {
            return Err(error.to_string());
        }
        Ok(reply)
    }

    async fn read_line(&mut self, timeout: Duration) -> Result<serde_json::Value, String> {
        let mut line = String::new();
        let read = tokio::time::timeout(timeout, self.stdout.read_line(&mut line))
            .await
            .map_err(|_| "embed worker timed out".to_string())?
            .map_err(|e| format!("read from embed worker: {e}"))?;
        if read == 0 {
            return Err("embed worker exited".into());
        }
        serde_json::from_str(&line).map_err(|e| format!("bad reply from embed worker: {e}"))
    }
}

// --- The hooks mutation paths call (non-fatal: indexing never fails a save) ---

pub fn sync_meeting(db: &Db, runtime: &SearchRuntime, meeting_id: &str) {
    if let Err(e) = embral_search::sync_meeting(db, meeting_id) {
        tracing::warn!(meeting_id, "search index sync failed: {e:#}");
    }
    runtime.notify.notify_one();
}

pub fn sync_dictation(db: &Db, runtime: &SearchRuntime, dictation_id: i64) {
    if let Err(e) = embral_search::sync_dictation(db, dictation_id) {
        tracing::warn!(dictation_id, "search index sync failed: {e:#}");
    }
    runtime.notify.notify_one();
}

/// After deletes/prunes: chunks cascaded with their owners; their vectors
/// are orphans until swept.
pub fn after_delete(db: &Db) {
    match embral_search::sweep_vectors(db) {
        Ok(n) if n > 0 => tracing::debug!(swept = n, "vector orphans removed"),
        Err(e) => tracing::warn!("vector sweep failed: {e:#}"),
        _ => {}
    }
}

/// Read every image nothing has read yet, a batch at a time.
///
/// The OS call is synchronous and takes ~100 ms an image, so it runs on a
/// blocking thread; the loop ends when a pass reads nothing, which is both
/// "all caught up" and "this machine has no engine".
async fn drain_ocr(state: &tauri::State<'_, AppState>) {
    let Ok(db) = state.db().await else { return };
    let base = {
        let config = state.config.lock().await;
        crate::storage::storage_base(&config.storage_dir)
    };
    loop {
        let (db, base) = (db.clone(), base.clone());
        let read = tokio::task::spawn_blocking(move || {
            crate::ocr::sweep(&db, &base, crate::ocr::SWEEP_BATCH)
        })
        .await;
        match read {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => {
                tracing::warn!("the OCR sweep panicked: {e}");
                break;
            }
        }
    }
}

/// The background embedding worker: backfill once at boot, then drain
/// pending chunks (newest owners first) whenever a mutation pings, in
/// small batches so the palette never waits long on the DB lock.
pub fn spawn_worker(handle: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(10)).await;
        let state = handle.state::<AppState>();

        if let Ok(db) = state.db().await {
            match embral_search::backfill_missing(&db) {
                Ok(n) if n > 0 => tracing::info!(owners = n, "search index backfilled"),
                Err(e) => tracing::warn!("search index backfill failed: {e:#}"),
                _ => {}
            }
        }
        state.search.notify.notify_one();

        loop {
            tokio::select! {
                _ = state.search.notify.notified() => {}
                _ = tokio::time::sleep(FALLBACK_TICK) => {}
            }

            if state.search.idle_for() > IDLE_EVICT {
                state.search.shutdown().await;
            }

            // OCR first: an image read this pass becomes chunks that the
            // embedding drain below picks up without waiting for another
            // wake-up. It runs whether or not the embedding model is
            // present; FTS alone already makes the text findable.
            drain_ocr(&state).await;

            if !embral_search::model::present() {
                continue;
            }
            let Ok(db) = state.db().await else { continue };
            if let Err(e) = db.with_conn(|conn| SqliteVecIndex.ensure(conn)) {
                tracing::warn!("vector index ensure failed: {e:#}");
                continue;
            }

            loop {
                let pending = match embral_search::next_pending(&db, EMBED_BATCH) {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!("pending lookup failed: {e:#}");
                        break;
                    }
                };
                if pending.is_empty() {
                    break;
                }
                let texts: Vec<String> = pending.iter().map(|(_, t)| t.clone()).collect();
                match state.search.embed_passages(&texts).await {
                    Ok(vectors) => {
                        let rows: Vec<(i64, Vec<f32>)> =
                            pending.iter().map(|(id, _)| *id).zip(vectors).collect();
                        if let Err(e) = embral_search::store_embeddings(&db, &rows) {
                            tracing::warn!("storing embeddings failed: {e:#}");
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::warn!("embedding batch failed: {e}");
                        break;
                    }
                }
            }
        }
    });
}
