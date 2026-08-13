mod audio;
mod autodetect;
#[cfg(feature = "cloud")]
mod cloud;
mod commands;
mod config;
mod dictation;
mod hotkey;
mod llm;
mod mcp_clients;
mod notes_matching;
mod notice;
mod ocr;
mod platform;
mod recovery;
mod refinement;
mod search_index;
mod speaker_commands;
mod speakers;
mod storage;
mod system_specs;
mod telemetry;
mod transcription;
mod tray;
mod window_rescue;

use std::sync::Arc;
use tauri::Manager;

pub use transcription::stream::SharedSlot;

/// Source of truth for the in-progress recording's finalized segments. The
/// event-forwarder task in `start_recording` appends every `Segment` event to
/// this Vec; `stop_recording` reads from it after the recv-task has had a
/// bounded window to finalize. This decouples segment ownership from the
/// (sometimes very slow) `TranscriptionSession::finish()` return value.
pub type SharedSegments = Arc<tokio::sync::Mutex<Vec<embral_types::TranscriptionSegment>>>;

pub struct AppState {
    pub recorder: tokio::sync::Mutex<Option<audio::recorder::Recorder>>,
    /// The current recording's session slot: what the audio bridge feeds
    /// (a live session, a buffer while one opens, or nothing). `None`
    /// outer = no recording.
    pub session: tokio::sync::Mutex<Option<SharedSlot>>,
    /// The current recording's stream lane (clock, generation, event
    /// channel); swapped wholesale at each start. See
    /// `transcription::stream`.
    pub lane: std::sync::Mutex<Arc<transcription::stream::StreamLane>>,
    /// The current recording's event-forwarder task. Stop awaits it
    /// (bounded) before snapshotting segments, so a draining stream's
    /// tail always reaches finalize.
    pub forwarder_task: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    pub config: tokio::sync::Mutex<embral_types::AppConfig>,
    pub current_segments: SharedSegments,
    /// Warm local speech engine: recognizers load once per app run and stay
    /// cached, so recording start is instant after the first use.
    pub engine: Arc<embral_engine::Engine>,
    /// The open database, tagged with the storage base it belongs to so a
    /// `storage_dir` change in Settings transparently reopens against the new
    /// location (see [`AppState::db`]).
    db: tokio::sync::Mutex<Option<(std::path::PathBuf, Arc<embral_db::Db>)>>,
    /// Model ids with a download in flight, so concurrent `download_asr_model`
    /// calls for the same model are rejected rather than racing on files.
    pub model_downloads: std::sync::Mutex<std::collections::HashSet<String>>,
    /// True while an import is transcribing, so concurrent imports (and
    /// imports during recordings) are rejected.
    pub importing: Arc<std::sync::atomic::AtomicBool>,
    /// True when the current recording was started by meeting detection (or
    /// by accepting its prompt) — only such recordings may auto-stop.
    pub auto_started: std::sync::atomic::AtomicBool,
    /// The active session provider's `labels_authoritative` capability,
    /// snapshotted at start so `stop_recording` can tell the finalize
    /// pipeline whether provider labels must be kept or re-diarized.
    pub labels_authoritative: std::sync::atomic::AtomicBool,
    /// Whether *this recording* is labeling speakers. Starts from
    /// `diarization_enabled` and can go false mid-recording — the
    /// transcript header's toggle, or the runaway guard when the clusterer
    /// keeps inventing people ([speakers.md]). It is the flag finalize
    /// honors, not the config field.
    pub live_diarization: std::sync::atomic::AtomicBool,
    /// Distinct speaker labels this recording has produced, which is what
    /// the runaway guard counts. Cleared at start.
    pub live_speaker_labels: std::sync::Mutex<std::collections::HashSet<String>>,
    /// Live speaker renames (label edits during the recording): old label →
    /// user-given name. Applied to already-accumulated segments when set and
    /// to every later segment by the event forwarder; cleared at start.
    pub live_label_renames:
        tokio::sync::Mutex<std::collections::HashMap<String, String>>,
    /// User-starred moments (seconds into the recording), accumulated by
    /// `star_moment` and drained by `stop_recording`; cleared at start.
    pub stars: tokio::sync::Mutex<Vec<f64>>,
    /// Where each star sits in the user's notes, sent by the frontend on
    /// `recording-stopped` (`set_star_anchors`) and merged into the stars
    /// before they persist; cleared at start.
    pub star_anchors: tokio::sync::Mutex<Vec<commands::Star>>,
    /// True after the user dismissed the "call detected" prompt; suppresses
    /// re-prompting until the current call ends.
    pub detection_dismissed: std::sync::atomic::AtomicBool,
    /// The source picker's choices for the running recording, re-read by
    /// the capture threads on every tick. Reset to the defaults at start:
    /// everything the machine plays, and the configured mic alone.
    pub system_audio_wanted: std::sync::Mutex<platform::types::SystemAudioWanted>,
    pub extra_mics: std::sync::Mutex<Vec<String>>,
    /// The frontend's notes/title drafts, mirrored (debounced) during a
    /// recording so a stop the frontend never answers — the handshake
    /// fallback — still saves the human's words. Cleared at start.
    pub recording_drafts: std::sync::Mutex<Option<(String, String)>>,
    /// Epoch-ms the current recording started — `recording_status`'s clock
    /// source, so a window that missed `recording-started` (hidden webviews
    /// get throttled and drop events) can rebuild the timer on focus.
    pub recording_started_at_ms: std::sync::atomic::AtomicU64,
    /// Epoch-ms of the current recording's last sign of life — the silence
    /// check-in's clock ([detection.md]): advanced by transcribed words as
    /// they arrive (new final tokens in an interim, a segment closing) and
    /// by notes or title edits reaching `sync_recording_drafts`.
    /// Rebaselined at start, on resume, by "Keep recording", and by every
    /// session install (`install_stream`) — a transcription outage is not
    /// silence.
    pub last_liveness_at: std::sync::atomic::AtomicU64,
    /// The silence check-in's standing: 0 = none showing, `u64::MAX` =
    /// stood down until liveness resumes, else the epoch-ms it fired.
    pub silence_notice_at: std::sync::atomic::AtomicU64,
    /// Which recording the silence watcher belongs to — bumped at every
    /// start. A watcher whose generation moved on exits instead of racing
    /// the successor's watcher on the shared check-in state (a stop and
    /// restart inside one 15 s tick would otherwise leave two running).
    pub recording_generation: std::sync::atomic::AtomicU64,
    /// The built-in LLM child process (llama-server), started on demand.
    pub llm: llm::LlmSidecar,
    /// The search-index runtime: the embed child process (`embral-mcp
    /// embed`) and the worker's wake-up bell.
    pub search: search_index::SearchRuntime,
    /// The running dictation session, if any.
    pub dictation: tokio::sync::Mutex<Option<dictation::ActiveDictation>>,
    /// Mirror of `dictation.is_some()` readable from sync contexts (the
    /// global-shortcut handler decides tap-vs-hold without locking).
    pub dictating: std::sync::atomic::AtomicBool,
    /// When the dictation hotkey press that started the session happened.
    pub dictation_pressed_at: std::sync::Mutex<Option<std::time::Instant>>,
    /// Opt-in telemetry queue + enabled mirror — cloud edition only; the
    /// shared call sites go through `telemetry`'s no-op facade
    /// ([telemetry.md]).
    #[cfg(feature = "cloud")]
    pub telemetry: cloud::telemetry::Telemetry,
}

impl AppState {
    pub fn new(config: embral_types::AppConfig) -> Self {
        Self {
            recorder: tokio::sync::Mutex::new(None),
            session: tokio::sync::Mutex::new(None),
            lane: std::sync::Mutex::new(Arc::new(transcription::stream::StreamLane::idle())),
            forwarder_task: std::sync::Mutex::new(None),
            config: tokio::sync::Mutex::new(config),
            current_segments: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            engine: Arc::new(embral_engine::Engine::new()),
            db: tokio::sync::Mutex::new(None),
            model_downloads: std::sync::Mutex::new(std::collections::HashSet::new()),
            importing: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            auto_started: std::sync::atomic::AtomicBool::new(false),
            labels_authoritative: std::sync::atomic::AtomicBool::new(false),
            live_diarization: std::sync::atomic::AtomicBool::new(false),
            live_speaker_labels: std::sync::Mutex::new(std::collections::HashSet::new()),
            live_label_renames: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            stars: tokio::sync::Mutex::new(Vec::new()),
            star_anchors: tokio::sync::Mutex::new(Vec::new()),
            detection_dismissed: std::sync::atomic::AtomicBool::new(false),
            system_audio_wanted: std::sync::Mutex::new(
                platform::types::SystemAudioWanted::default(),
            ),
            extra_mics: std::sync::Mutex::new(Vec::new()),
            recording_drafts: std::sync::Mutex::new(None),
            recording_started_at_ms: std::sync::atomic::AtomicU64::new(0),
            last_liveness_at: std::sync::atomic::AtomicU64::new(0),
            silence_notice_at: std::sync::atomic::AtomicU64::new(0),
            recording_generation: std::sync::atomic::AtomicU64::new(0),
            llm: llm::LlmSidecar::default(),
            search: search_index::SearchRuntime::default(),
            dictation: tokio::sync::Mutex::new(None),
            dictating: std::sync::atomic::AtomicBool::new(false),
            dictation_pressed_at: std::sync::Mutex::new(None),
            #[cfg(feature = "cloud")]
            telemetry: cloud::telemetry::Telemetry::default(),
        }
    }

    /// Clone of the importing flag for a background task's drop guard.
    pub fn importing_handle(&self) -> Arc<std::sync::atomic::AtomicBool> {
        self.importing.clone()
    }

    /// The database for the *currently configured* storage dir, opening (and
    /// importing any legacy index.json) on first use or after the dir changes.
    pub async fn db(&self) -> Result<Arc<embral_db::Db>, String> {
        let base = {
            let config = self.config.lock().await;
            storage::storage_base(&config.storage_dir)
        };
        let mut guard = self.db.lock().await;
        if let Some((open_base, db)) = guard.as_ref() {
            if *open_base == base {
                return Ok(db.clone());
            }
        }
        let db = storage::open_db(&base).map_err(|e| e.to_string())?;
        let db = Arc::new(db);
        *guard = Some((base, db.clone()));
        Ok(db)
    }
}

/// Milliseconds since the Unix epoch — the one wall-clock read behind
/// event timestamps and the check-in's liveness clock.
pub(crate) fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// `%LOCALAPPDATA%/embral/logs` — next to the models dir, not user data.
pub fn logs_dir() -> std::path::PathBuf {
    dirs::data_local_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("embral")
        .join("logs")
}

/// The `--child-reaper` subprocess body (see `platform::supervisor`).
/// Every platform supplies one — a no-op where the OS already covers orphan
/// cleanup (Windows' job object, Linux's `PR_SET_PDEATHSIG`) and the flag is
/// never passed — so this call needs no `cfg`.
pub fn run_child_reaper() {
    platform::run_reaper();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // **embral is an X11 application on Linux.** Ask GTK for the X11 backend
    // before anything initialises it, so a Wayland session runs us through
    // Xwayland rather than as a native Wayland client.
    //
    // This is a deliberate narrowing, not a workaround for one bug. Wayland
    // withholds three things this app is built on, by design and from every
    // ordinary client: positioning your own windows (the notice belongs
    // bottom-right, and a compositor put it top-left instead), synthesising
    // keystrokes (dictation's auto-paste), and asking which window has focus.
    // Supporting both meant three degraded paths and a settings surface
    // apologising for them; running on X11 everywhere means one path that
    // works. Xwayland ships with every Wayland desktop, so this costs users
    // nothing but honesty in the docs.
    //
    // The one residue: on a Wayland session, auto-paste reaches X11 and
    // Xwayland targets but not windows that are natively Wayland — XTEST
    // cannot see them. Recording, transcription, notes and search are
    // unaffected either way.
    #[cfg(target_os = "linux")]
    std::env::set_var("GDK_BACKEND", "x11");

    // Default to `info`: a clean recording emits only the standardized
    // per-session spine (connect → ready → ~20s heartbeat → finish) plus any
    // warn/error. The per-message/per-frame firehose lives at `trace` — opt in
    // with e.g. `RUST_LOG=embral_lib=trace` for deep protocol debugging.
    //
    // Logs go to stderr AND a daily-rolling file under
    // `%LOCALAPPDATA%/embral/logs` (surfaced via Settings → About → Open logs
    // folder) so users can attach them to bug reports.
    {
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;

        let filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

        let logs_dir = logs_dir();
        let _ = std::fs::create_dir_all(&logs_dir);
        let file_appender = tracing_appender::rolling::daily(&logs_dir, "embral.log");
        let (file_writer, guard) = tracing_appender::non_blocking(file_appender);
        // The guard flushes on drop; the app runs for the process lifetime,
        // so parking it forever is the correct lifetime.
        Box::leak(Box::new(guard));

        tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer())
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(file_writer)
                    .with_ansi(false),
            )
            .init();
    }
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        "embral starting"
    );
    platform::kill_children_with_us();
    // A corrupt config.json is backed up + logged inside load_config; this
    // arm only fires on I/O failures, and those deserve a trace too.
    let config = config::load_config().unwrap_or_else(|e| {
        tracing::error!("config.json unreadable ({e}); using defaults");
        embral_types::AppConfig::default()
    });
    tray::set_recording_color(&config.tray_recording_color);
    // Read before the config moves into the app state: setup() needs these to
    // decide whether this launch opens the window (see below) and which
    // directory the webview may load files from.
    let needs_onboarding = !config.onboarding_completed;
    let startup_storage_dir = config.storage_dir.clone();

    let builder = tauri::Builder::default()
        // Registered first, so a second launch bails out before any heavy
        // init: the app lives in the tray, so re-launching the installed
        // shortcut must surface the running window, not stack a new process.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                window_rescue::ensure_on_screen(&w);
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init());
    // Self-updating is cloud-edition-only: the release channel serves
    // cloud installers, so an offline build carrying a live updater would
    // replace itself with the cloud edition on update. Source builds
    // update via git ([cloud-seam.md]). The crate stays a dependency in
    // both editions so the capability file's `updater:default` grant
    // stays valid; only the registration is gated.
    #[cfg(feature = "cloud")]
    let builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    builder
        .plugin(tauri_plugin_process::init())
        .plugin({
            use tauri_plugin_window_state::StateFlags;
            // VISIBLE is excluded: the plugin's restore would show+focus the
            // window, fighting the start-hidden design below. The dictation
            // overlay re-derives its own geometry on every show and must
            // never be shown or focused by the plugin.
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(StateFlags::SIZE | StateFlags::POSITION | StateFlags::MAXIMIZED)
                .with_denylist(&["dictation", "notice"])
                .build()
        })
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(hotkey::plugin())
        .manage(AppState::new(config))
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    let _ = window.hide();
                    api.prevent_close();
                }
            }
        })
        .setup(move |app| {
            // Before anything renders: the webview cannot load audio (or
            // note images) out of a storage dir the asset scope doesn't
            // cover, and the dir is a free-form picker.
            storage::allow_asset_access(
                app.handle(),
                &storage::storage_base(&startup_storage_dir),
            );
            tray::create_tray(app)?;
            // The app lives in the tray: launches land there rather than
            // opening the window (users open it from the tray icon), and
            // launch-at-login is always on — both by design, neither is a
            // setting. The exception is first run: until onboarding is
            // finished the window opens, so the installer's "run after
            // closing" box lands on the setup wizard instead of a silent
            // tray icon. The frontend already gates on the same flag.
            if let Some(w) = app.get_webview_window("main") {
                // The state plugin restored geometry at window-ready (before
                // this closure); repair it if the monitors changed since it
                // was saved.
                window_rescue::ensure_on_screen(&w);
                if needs_onboarding {
                    let _ = w.show();
                    let _ = w.set_focus();
                } else {
                    let _ = w.hide();
                }
            }
            {
                use tauri_plugin_autostart::ManagerExt;
                let autostart = app.autolaunch();
                if !autostart.is_enabled().unwrap_or(false) {
                    if let Err(e) = autostart.enable() {
                        tracing::warn!("failed to enable launch at login: {e}");
                    }
                }
            }
            // Audio janitor: prune old audio per the retention setting, on
            // startup and every 12 hours. Reads the config each tick so a
            // changed setting applies without restart; skips when disabled.
            {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    loop {
                        let state = handle.state::<AppState>();
                        let (days, meeting_days, dictation_days, dictation_count, base) = {
                            let config = state.config.lock().await;
                            // Both dictation criteria sit behind the one
                            // auto-delete switch; either at 0 is off.
                            let (d_days, d_count) = if config.dictation_auto_delete {
                                (config.dictation_retention_days, config.dictation_retention_count)
                            } else {
                                (0, 0)
                            };
                            (
                                config.audio_retention_days,
                                config.meeting_retention_days,
                                d_days,
                                d_count,
                                storage::storage_base(&config.storage_dir),
                            )
                        };
                        // Orphaned asset directories are residue, not a
                        // retention policy, so this runs whatever the
                        // retention settings say.
                        if let Ok(db) = state.db().await {
                            if let Err(e) = storage::prune_orphan_assets(&db, &base) {
                                tracing::warn!("asset janitor failed: {e}");
                            }
                        }
                        if days > 0 || meeting_days > 0 || dictation_days > 0 || dictation_count > 0 {
                            match state.db().await {
                                Ok(db) => {
                                    if meeting_days > 0 {
                                        match storage::prune_old_meetings(&db, &base, meeting_days)
                                        {
                                            Ok(n) if n > 0 => {
                                                tracing::info!(pruned = n, "janitor removed old meetings")
                                            }
                                            Ok(_) => {}
                                            Err(e) => tracing::warn!("meeting janitor failed: {e}"),
                                        }
                                    }
                                    if days > 0 {
                                        match storage::prune_old_audio(&db, &base, days) {
                                            Ok(n) if n > 0 => {
                                                tracing::info!(pruned = n, "janitor removed old audio")
                                            }
                                            Ok(_) => {}
                                            Err(e) => tracing::warn!("janitor failed: {e}"),
                                        }
                                    }
                                    match db.prune_dictations(dictation_days) {
                                        Ok(n) if n > 0 => {
                                            tracing::info!(pruned = n, "janitor removed old dictations")
                                        }
                                        Ok(_) => {}
                                        Err(e) => tracing::warn!("dictation janitor failed: {e}"),
                                    }
                                    match db.prune_dictations_beyond(dictation_count) {
                                        Ok(n) if n > 0 => {
                                            tracing::info!(
                                                pruned = n,
                                                "janitor trimmed dictations beyond the keep-count"
                                            )
                                        }
                                        Ok(_) => {}
                                        Err(e) => tracing::warn!("dictation janitor failed: {e}"),
                                    }
                                    // Pruned owners cascade their chunks; the
                                    // vectors are orphans until swept.
                                    search_index::after_delete(&db);
                                }
                                Err(e) => tracing::warn!("janitor could not open db: {e}"),
                            }
                        }
                        tokio::time::sleep(std::time::Duration::from_secs(12 * 3600)).await;
                    }
                });
            }
            // Built-in LLM idle eviction: check every minute; the sidecar
            // frees ~3 GB of RAM when it hasn't been used for a while.
            {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    loop {
                        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                        let state = handle.state::<AppState>();
                        let (keep_warm, idle_minutes) = {
                            let config = state.config.lock().await;
                            // Keep-warm only means something while a summary
                            // or cleanup engine actually lives on the device;
                            // otherwise a one-off use must not pin ~3 GB.
                            (
                                config.llm_keep_warm && llm::uses_local_llm(&config),
                                config.llm_idle_minutes,
                            )
                        };
                        state.llm.evict_if_idle(keep_warm, idle_minutes);
                    }
                });
            }
            // Update leftovers: remove mcp server binaries the installer
            // renamed aside while a client held them ([release.md]
            // §Installer hooks); still-locked ones wait for a later boot.
            #[cfg(windows)]
            mcp_clients::sweep_stale_servers();
            // The AppImage counterpart: refresh the stable per-user server
            // copy registrations point at, so clients spawn the new build
            // after an update ([integrations.md]).
            #[cfg(target_os = "linux")]
            mcp_clients::refresh_appimage_server_copy();
            // Recordings the last run never finished: turn them into
            // ordinary meetings, or discard ones that barely started. Its
            // synchronous part freezes the worklist before this line
            // returns, so detection (below) and the user cannot race the
            // rescue's reads ([recording.md] §Crash recovery).
            commands::recover_interrupted_recording(app.handle().clone());
            // Meeting auto-detection poller (policy-gated internally).
            autodetect::spawn(app.handle().clone());
            // Search-index worker: backfills chunks at boot, embeds pending
            // passages whenever a mutation pings it.
            search_index::spawn_worker(app.handle().clone());
            // Telemetry (cloud edition only, on by default until opted
            // out): mirror the flag, start the flusher, count the launch,
            // and fire the daily config snapshot when due.
            #[cfg(feature = "cloud")]
            {
                let state = app.state::<AppState>();
                let enabled = {
                    let mut config = state.config.blocking_lock();
                    // Default-on means the id must exist before the first
                    // flush — a first boot mints it here; opt-out clears
                    // it and re-enabling mints a fresh one (save_config).
                    if config.telemetry_enabled && config.telemetry_install_id.is_empty() {
                        config.telemetry_install_id = uuid::Uuid::new_v4().to_string();
                        if let Err(e) = config::save_config(&config) {
                            tracing::warn!("failed to persist telemetry install id: {e}");
                        }
                    }
                    config.telemetry_enabled
                };
                state
                    .telemetry
                    .enabled
                    .store(enabled, std::sync::atomic::Ordering::Release);
                cloud::telemetry::spawn_flusher(app.handle().clone());
                telemetry::track(&state, "app_started", serde_json::json!({}));
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    cloud::telemetry::maybe_snapshot(&handle.state::<AppState>()).await;
                });
            }
            // Register the record + dictation hotkeys from config (empty = none).
            {
                let (record, dictation) = {
                    let state = app.state::<AppState>();
                    let config = state.config.blocking_lock();
                    (config.record_hotkey.clone(), config.dictation_hotkey.clone())
                };
                if let Err(e) = hotkey::apply(app.handle(), &record, &dictation) {
                    tracing::warn!("{e}");
                }
            }
            Ok(())
        })
        .invoke_handler(app_handler())
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|app, event| {
            // Don't orphan the llama-server child when the app quits. (The
            // embed child also exits on its own when its stdin closes.)
            if let tauri::RunEvent::Exit = event {
                app.state::<AppState>().llm.shutdown();
                app.state::<AppState>().search.shutdown_blocking();
            }
        });
}

// `generate_handler!` can't cfg individual entries, so the shared list
// lives in one macro and the cloud build appends its commands.
macro_rules! app_handler_with {
    ($($extra:path),* $(,)?) => {
        tauri::generate_handler![
            commands::reset_app_data,
            commands::start_recording,
            commands::pause_recording,
            commands::resume_recording,
            commands::silence_keep_recording,
            commands::sync_recording_drafts,
            commands::request_stop_recording,
            commands::recording_status,
            commands::fixture_state,
            commands::fixture_show_overlay,
            commands::list_audio_sources,
            commands::set_system_audio_sources,
            commands::set_extra_mics,
            notice::notify,
            notice::hide_notice,
            notice::current_notice,
            notice::open_from_notice,
            commands::rename_live_speaker,
            commands::set_live_diarization,
            commands::star_moment,
            commands::unstar_moment,
            commands::set_star_anchors,
            commands::stop_recording,
            commands::import_recording,
            commands::accept_detected_meeting,
            commands::dismiss_detected_meeting,
            commands::get_meetings,
            commands::get_meeting_records,
            commands::get_meeting,
            commands::get_meeting_detail,
            commands::update_meeting_title,
            commands::update_meeting_summary,
            commands::update_meeting_notes,
            commands::save_note_asset,
            commands::storage_root,
            commands::update_meeting_transcript,
            commands::delete_meeting,
            commands::delete_meetings,
            commands::search_library,
            commands::get_config,
            commands::save_config,
            commands::open_notes_folder,
            commands::list_audio_devices,
            commands::mic_permission,
            commands::update_needs_authentication,
            commands::request_mic_permission,
            commands::accessibility_permission,
            commands::request_accessibility_permission,
            commands::preview_export_filename,
            commands::test_webhook,
            commands::open_logs_folder,
            commands::update_guard,
            system_specs::system_specs,
            tray::system_accent_color,
            mcp_clients::mcp_setup_info,
            mcp_clients::mcp_clients_status,
            mcp_clients::mcp_register,
            mcp_clients::mcp_unregister,
            commands::asr_models_status,
            commands::download_asr_model,
            commands::delete_asr_model,
            commands::llm_status,
            commands::get_summary_prompt_parts,
            commands::start_dictation,
            commands::stop_dictation,
            commands::cancel_dictation,
            commands::list_dictations,
            commands::delete_dictation,
            speaker_commands::list_speakers,
            speaker_commands::upsert_speaker,
            speaker_commands::delete_speaker,
            speaker_commands::delete_speakers,
            speaker_commands::merge_speakers,
            speaker_commands::speaker_meetings,
            speaker_commands::speaker_segments,
            speaker_commands::confirm_name_suggestion,
            speaker_commands::dismiss_name_suggestion,
            speaker_commands::edit_segments,
            $($extra),*
        ]
    };
}

#[cfg(feature = "cloud")]
fn app_handler() -> impl Fn(tauri::ipc::Invoke) -> bool + Send + Sync + 'static {
    app_handler_with![
        cloud::commands::cloud_request_code,
        cloud::commands::cloud_verify_code,
        cloud::commands::cloud_account_status,
        cloud::commands::cloud_sign_out,
        cloud::commands::cloud_revoke_device,
        cloud::commands::cloud_billing_url,
        cloud::commands::cloud_billing_tiers,
        cloud::commands::cloud_adopt_provider,
        cloud::commands::telemetry_track,
    ]
}

#[cfg(not(feature = "cloud"))]
fn app_handler() -> impl Fn(tauri::ipc::Invoke) -> bool + Send + Sync + 'static {
    app_handler_with![]
}
