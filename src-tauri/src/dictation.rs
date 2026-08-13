//! Dictation: hotkey-driven mic-only speech-to-text into any app.
//!
//! Flow: global hotkey → mic stream into a transcription session from the
//! provider seam (dictation's own provider tree — on-device or the cloud
//! relay) → on stop: optional AI cleanup → clipboard (+ optional paste into
//! the focused app) → history row in the DB. The overlay window shows the
//! words in ~realtime over a mic level visualizer ([dictation.md]).
//!
//! Dictation and meeting recording are mutually exclusive — both need the
//! microphone and the transcription engine's attention.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use embral_types::{AppConfig, AppError};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Mutex;

use crate::audio::recorder::MicStream;
use crate::transcription::{TranscriptionEvent, TranscriptionSession};
use crate::AppState;

/// A tap shorter than this is toggle mode; holding longer means push-to-talk
/// (stop on release).
pub const HOLD_THRESHOLD: Duration = Duration::from_millis(700);

/// Label of the overlay window.
const OVERLAY: &str = "dictation";

/// How long stop() waits for the session to flush its tail. Short on
/// purpose: the fallback now delivers everything that was on screen
/// (finalized segments plus the interim tail), so timing out costs nothing
/// visible — a healthy cloud end takes ~150 ms, a healthy local one less.
const FINISH_TIMEOUT: Duration = Duration::from_secs(3);

/// How long the concurrent session connect may take before the degrade
/// chain (or a start failure) kicks in. Bounds how long stop() can wait on
/// an in-flight connect.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

pub struct ActiveDictation {
    mic: Option<MicStream>,
    /// Shared with the audio bridge; stop() takes it to call finish(). The
    /// session connects concurrently with the mic — `None` until then.
    session: Arc<Mutex<Option<Box<dyn TranscriptionSession>>>>,
    bridge: tokio::task::JoinHandle<()>,
    /// The concurrent session connect; stop() and cancel() await it so an
    /// instant press-release can't race the connection (and no session is
    /// ever left behind unfinished).
    connect: tokio::task::JoinHandle<()>,
    /// Segment texts mirrored by the event consumer — the fallback when
    /// finish() errors or times out, so a flaky session still delivers what
    /// was heard.
    heard: Arc<std::sync::Mutex<Vec<String>>>,
    /// The latest interim (committed text + tentative tail), joined onto the
    /// mirrored segments by the fallback: the words on the screen are the
    /// floor of what gets delivered, tentative or not.
    last_interim: Arc<std::sync::Mutex<String>>,
    /// When the session started — telemetry's duration bucket.
    started: std::time::Instant,
}

/// Pure decision rule for the dictation hotkey (unit-tested): what to do on
/// a press/release given whether a session is active and how long ago the
/// initiating press happened.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum HotkeyAction {
    Start,
    Stop,
    Nothing,
}

pub fn on_press(active: bool) -> HotkeyAction {
    if active {
        HotkeyAction::Stop // second tap ends a toggle-mode session
    } else {
        HotkeyAction::Start
    }
}

pub fn on_release(active: bool, held: Duration) -> HotkeyAction {
    if active && held >= HOLD_THRESHOLD {
        HotkeyAction::Stop // push-to-talk: released after holding
    } else {
        HotkeyAction::Nothing // short tap: stay in toggle mode
    }
}

/// Whether this dictation configuration requires the on-device model on
/// disk before starting: the device is the primary, or it is where an
/// out-of-hours cloud session lands. Cloud with "disabled" needs nothing
/// local — failing without a fallback is what the user asked for.
fn needs_local_model(config: &AppConfig) -> bool {
    match config.dictation_provider {
        embral_types::TranscriptionProvider::Local => true,
        #[cfg(feature = "cloud")]
        embral_types::TranscriptionProvider::Cloud => {
            config.dictation_out_of_hours == embral_types::CloudOutOfHours::Local
        }
    }
}

/// Start a dictation session: a transcription session from the provider
/// seam, the overlay indicator, the mic streaming into it.
pub async fn start(app: &AppHandle) -> Result<(), AppError> {
    let state = app.state::<AppState>();
    if state.recorder.lock().await.is_some() {
        return Err(AppError::CantDictateWhileRecording);
    }
    let mut slot = state.dictation.lock().await;
    if slot.is_some() {
        return Err(AppError::DictationAlreadyRunning);
    }

    let config = state.config.lock().await.clone();
    if needs_local_model(&config) {
        let model_id = config.dictation_asr_model_id();
        if !state.engine.model_present(&model_id) {
            return Err(AppError::DictationModelMissing { model_id });
        }
    }
    #[cfg(feature = "cloud")]
    if config.dictation_provider == embral_types::TranscriptionProvider::Cloud
        && config.cloud_session_token.is_empty()
    {
        return Err(AppError::CloudSignInRequired);
    }

    // The overlay and the mic come first — the hotkey must respond
    // instantly, and the first words must be captured even while weights
    // come up or the relay connects. The session connects concurrently; the
    // bridge buffers mic audio until it's up. (The synchronous checks above
    // still fail before anything shows, so a refused start leaves no stuck
    // indicator; an async connect failure tears the overlay down.)
    show_overlay(app)?;

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<f32>>();
    let mic_device = {
        let name = config.mic_device.trim();
        if name.is_empty() { None } else { Some(name.to_string()) }
    };
    let mic = match MicStream::start(mic_device.as_deref(), tx) {
        Ok(mic) => mic,
        Err(e) => {
            hide_overlay(app);
            return Err(AppError::internal(e));
        }
    };

    let (event_tx, mut event_rx) =
        tokio::sync::mpsc::unbounded_channel::<TranscriptionEvent>();
    let session_arc: Arc<Mutex<Option<Box<dyn TranscriptionSession>>>> =
        Arc::new(Mutex::new(None));

    // The concurrent connect. On failure it takes the whole dictation down
    // (the slot lock serializes it against start() finishing and against
    // stop(), so there is no half-torn-down state to race).
    let app_for_connect = app.clone();
    let engine = state.engine.clone();
    let config_for_connect = config.clone();
    let session_for_connect = session_arc.clone();
    let connect_event_tx = event_tx.clone();
    let connect = tokio::spawn(async move {
        let config = config_for_connect;
        let provider = crate::transcription::build_dictation_provider(&config, engine.clone());
        let session_result =
            match tokio::time::timeout(CONNECT_TIMEOUT, provider.start_session(connect_event_tx.clone()))
                .await
            {
                Ok(result) => result,
                Err(_) => Err(anyhow::anyhow!("timed out connecting")),
            };
        // A cloud refusal (out of hours, unreachable) degrades per
        // dictation's out-of-hours setting, same rule as meetings
        // (`on_cloud_failure`).
        #[cfg(feature = "cloud")]
        let session_result = match session_result {
            Err(e) if config.dictation_provider == embral_types::TranscriptionProvider::Cloud => {
                let model_present = engine.model_present(&config.dictation_asr_model_id());
                match crate::config::on_cloud_failure(config.dictation_out_of_hours, model_present)
                {
                    crate::config::CloudFailureAction::SwitchToLocal => {
                        tracing::warn!("cloud dictation unavailable ({e}); using this device");
                        crate::transcription::build_local_dictation_provider(
                            &config,
                            engine.clone(),
                        )
                        .start_session(connect_event_tx.clone())
                        .await
                    }
                    crate::config::CloudFailureAction::DisableTranscription => Err(anyhow::anyhow!(
                        "cloud dictation is unavailable ({e}), and dictation is set not to use this device"
                    )),
                    crate::config::CloudFailureAction::Fail => Err(e),
                }
            }
            other => other,
        };
        match session_result {
            Ok(session) => {
                *session_for_connect.lock().await = Some(session);
            }
            Err(e) => abort_start(&app_for_connect, e.to_string()).await,
        }
    });

    // Audio bridge: mic chunks into the session, whoever the provider is.
    // Chunks that arrive before the session is connected are held and
    // flushed once it is — nothing said from the first instant is lost.
    // A LevelTap (the meeting meter's band-spectrum helper, mic-only) rides
    // along, feeding the overlay's visualizer at ~10 Hz.
    let session_for_bridge = session_arc.clone();
    let app_for_levels = app.clone();
    let bridge = tokio::spawn(async move {
        let mut level_tap = crate::audio::meter::LevelTap::new(Box::new(move |mic, _| {
            let _ = app_for_levels.emit_to(OVERLAY, "dictation-level", mic);
        }));
        let mut pending: Vec<Vec<f32>> = Vec::new();
        while let Some(chunk) = rx.recv().await {
            level_tap.push_block(&chunk, &[]);
            let guard = session_for_bridge.lock().await;
            match guard.as_ref() {
                Some(s) => {
                    for held in pending.drain(..) {
                        if let Err(e) = s.send_audio(&held).await {
                            tracing::warn!("dictation send_audio failed: {e}");
                        }
                    }
                    if let Err(e) = s.send_audio(&chunk).await {
                        tracing::warn!("dictation send_audio failed: {e}");
                    }
                }
                None => pending.push(chunk),
            }
        }
        // The mic is gone. stop() awaits the connect before draining us, so
        // if the session came up after the last chunk, flush what was held.
        if !pending.is_empty() {
            let guard = session_for_bridge.lock().await;
            if let Some(s) = guard.as_ref() {
                for held in pending.drain(..) {
                    if let Err(e) = s.send_audio(&held).await {
                        tracing::warn!("dictation send_audio failed: {e}");
                    }
                }
            }
        }
    });

    // Event consumer: finish() is the source of truth for the text; Segments
    // are mirrored (the finish-timeout fallback) and the latest Interim kept
    // beside them (the fallback's fallback — a short session may end before
    // any segment finalizes). `Failed` mid-session means the session is
    // gone — deliver what was heard instead of dictating into the void
    // (cloud cut off, connection drop; dictations are seconds long, there
    // is no mid-session swap).
    let heard: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let last_interim: Arc<std::sync::Mutex<String>> =
        Arc::new(std::sync::Mutex::new(String::new()));
    let heard_for_consumer = heard.clone();
    let interim_for_consumer = last_interim.clone();
    let app_for_consumer = app.clone();
    tokio::spawn(async move {
        // The overlay's live view: everything finalized so far, plus the
        // interim's committed part, with the unstable tail separate (it
        // renders dim and may change).
        let emit_text = |app: &AppHandle, heard: &[String], interim: &str, tentative: &str| {
            let mut text = heard.join(" ");
            if !interim.is_empty() {
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(interim);
            }
            let _ = app.emit_to(
                OVERLAY,
                "dictation-text",
                serde_json::json!({ "text": text, "tentative": tentative }),
            );
        };
        while let Some(event) = event_rx.recv().await {
            match event {
                TranscriptionEvent::Segment(seg) => {
                    let text = seg.text.trim().to_string();
                    if !text.is_empty() {
                        heard_for_consumer
                            .lock()
                            .expect("dictation heard poisoned")
                            .push(text);
                    }
                    // A finalized segment supersedes the interim preview.
                    interim_for_consumer
                        .lock()
                        .expect("dictation interim poisoned")
                        .clear();
                    let heard = heard_for_consumer
                        .lock()
                        .expect("dictation heard poisoned")
                        .clone();
                    emit_text(&app_for_consumer, &heard, "", "");
                }
                TranscriptionEvent::Interim { segment, tentative } => {
                    let committed = segment.text.trim().to_string();
                    // The tail's leading space (or absence) is the word
                    // boundary — a spaceless tail continues the committed
                    // text's last word. Concatenate verbatim, never insert.
                    let tail = tentative.as_deref().unwrap_or("").trim_end().to_string();
                    {
                        let text = format!("{committed}{tail}").trim().to_string();
                        if !text.is_empty() {
                            *interim_for_consumer
                                .lock()
                                .expect("dictation interim poisoned") = text;
                        }
                    }
                    let heard = heard_for_consumer
                        .lock()
                        .expect("dictation heard poisoned")
                        .clone();
                    emit_text(&app_for_consumer, &heard, &committed, &tail);
                }
                TranscriptionEvent::Failed { message } => {
                    tracing::warn!(
                        "dictation transcription ended early ({message}); delivering what was heard"
                    );
                    if let Err(e) = stop(&app_for_consumer).await {
                        tracing::warn!("auto-stop after dictation failure: {e}");
                    }
                    break;
                }
                TranscriptionEvent::Done => break,
            }
        }
    });

    *slot = Some(ActiveDictation {
        mic: Some(mic),
        session: session_arc,
        bridge,
        connect,
        heard,
        last_interim,
        started: std::time::Instant::now(),
    });
    state.dictating.store(true, Ordering::Release);
    let _ = app.emit_to(OVERLAY, "dictation-started", ());
    let _ = app.emit("dictation-active", true);

    // When cleanup will run on the built-in model, start loading it now and
    // prime its prompt cache — by stop time cleanup pays generation time
    // only, instead of a cold llama-server start (~5 s) plus processing the
    // constant cleanup prompt (~3 s) inside "Finishing up…".
    if crate::llm::cleanup_uses_builtin(&config) {
        let app_for_warm = app.clone();
        let config_for_warm = config.clone();
        tokio::spawn(async move {
            let state = app_for_warm.state::<AppState>();
            let Some(cfg) =
                crate::llm::resolved_cleanup_config(&state.llm, &config_for_warm).await
            else {
                return;
            };
            if let Err(e) = embral_notes::prime_dictation(&cfg).await {
                tracing::debug!("cleanup prompt priming skipped: {e}");
            }
        });
    }

    tracing::info!(provider = ?config.dictation_provider, "dictation started");
    Ok(())
}

/// Tear down a dictation whose session never connected: the mic, the
/// overlay, the flags. Runs on the connect task; the slot lock serializes it
/// against start() (which holds the lock until the slot is filled) and
/// stop() (which may have already taken the dictation — then there is
/// nothing left to tear down here).
async fn abort_start(app: &AppHandle, message: String) {
    tracing::warn!("dictation start failed: {message}");
    let state = app.state::<AppState>();
    if let Some(mut active) = state.dictation.lock().await.take() {
        active.mic.take();
        let _ = active.bridge.await;
    }
    state.dictating.store(false, Ordering::Release);
    hide_overlay(app);
    let _ = app.emit("dictation-active", false);
    let _ = app.emit("processing-error", &AppError::DictationStartFailed { detail: message.to_string() });
}

/// Stop the session and run the output pipeline. Returns the pasted text.
pub async fn stop(app: &AppHandle) -> Result<String, AppError> {
    let state = app.state::<AppState>();
    let Some(mut active) = state.dictation.lock().await.take() else {
        return Err(AppError::NoDictationRunning);
    };
    state.dictating.store(false, Ordering::Release);
    let _ = app.emit_to(OVERLAY, "dictation-finishing", ());

    // An instant press-release can beat the session connect; wait for it to
    // settle (it is internally bounded) so the buffered audio still lands.
    let _ = active.connect.await;
    // Dropping the mic ends the stream; the bridge drains the last chunks
    // into the session, then finish() flushes the tail and returns every
    // finalized segment (the seam's contract).
    active.mic.take();
    let _ = active.bridge.await;
    // The fallback when finish() errors or times out: the mirrored segments
    // plus the last interim (committed + tentative) — everything that was on
    // screen gets delivered, locked as-is, rather than waiting on a stream
    // that isn't answering.
    let heard_mirror = active.heard.clone();
    let interim_mirror = active.last_interim.clone();
    let heard_fallback = move || {
        let heard = heard_mirror.lock().expect("dictation heard poisoned");
        let interim = interim_mirror
            .lock()
            .expect("dictation interim poisoned")
            .clone();
        fallback_text(&heard, &interim)
    };
    let raw = match active.session.lock().await.take() {
        Some(session) => {
            match tokio::time::timeout(FINISH_TIMEOUT, session.finish()).await {
                Ok(Ok(segments)) => join_segments(segments.iter().map(|s| s.text.as_str())),
                outcome => {
                    match outcome {
                        Ok(Err(e)) => tracing::warn!("dictation finish errored: {e}"),
                        _ => tracing::warn!("dictation finish timed out"),
                    }
                    heard_fallback()
                }
            }
        }
        // The session already died mid-dictation (the Failed auto-stop):
        // the mirrors are all there is.
        None => heard_fallback(),
    };

    let config = state.config.lock().await.clone();
    // The overlay stays up — "Finishing up…" — until the text is actually
    // delivered; cleanup can take seconds and a vanished overlay with no
    // text yet reads as a lost dictation.
    let _ = app.emit("dictation-active", false);

    if raw.is_empty() {
        hide_overlay_if_idle(app, &state);
        let _ = app.emit_to(OVERLAY, "dictation-complete", "");
        return Ok(String::new());
    }

    let focused = crate::platform::focused_app().map(|a| a.label().to_string());

    // Cleanup per the configured tier; every failure shape delivers the raw
    // text rather than losing the dictation. The *resolved* tier (cloud
    // degrades to on-device while signed out) feeds telemetry.
    let cleanup_cfg = crate::llm::resolved_cleanup_config(&state.llm, &config).await;
    let cleanup_tier = match &cleanup_cfg {
        None => "off",
        Some(cfg) if cfg.provider == embral_types::LlmProvider::Builtin => "on_device",
        Some(_) => "cloud",
    };
    let cleaned = match cleanup_cfg {
        Some(cfg) => match embral_notes::clean_dictation(&cfg, &raw).await {
            Ok(text) => {
                state.llm.touch();
                Some(text)
            }
            Err(e) => {
                tracing::warn!("dictation cleanup failed — using raw text: {e}");
                crate::telemetry::track(
                    &state,
                    "error",
                    serde_json::json!({ "category": "cleanup_failed" }),
                );
                None
            }
        },
        None => None,
    };

    let output = cleaned.clone().unwrap_or_else(|| raw.clone());

    let delivery = if config.dictation_auto_paste {
        "paste"
    } else if config.dictation_copy_clipboard {
        "clipboard"
    } else {
        "history"
    };
    crate::telemetry::track(
        &state,
        "dictation_used",
        serde_json::json!({
            "provider": config.dictation_provider,
            "cleanup": cleanup_tier,
            "duration_bucket": crate::telemetry::dictation_bucket(active.started.elapsed().as_secs()),
            "delivery": delivery,
        }),
    );

    // History first — losing the paste is recoverable, losing the text isn't.
    if let Ok(db) = state.db().await {
        match db.add_dictation(&raw, cleaned.as_deref(), focused.as_deref()) {
            Ok(id) => crate::search_index::sync_dictation(&db, &state.search, id),
            Err(e) => tracing::warn!("failed to save dictation history: {e}"),
        }
    }

    deliver(
        &output,
        config.dictation_copy_clipboard,
        config.dictation_auto_paste,
    );
    hide_overlay_if_idle(app, &state);
    let _ = app.emit_to(OVERLAY, "dictation-complete", &output);
    let _ = app.emit("dictation-complete", &output);
    tracing::info!(
        chars = output.len(),
        cleaned = cleaned.is_some(),
        app = focused.as_deref().unwrap_or("?"),
        "dictation finished"
    );
    Ok(output)
}

/// Abort without any output. The session still gets a bounded finish so
/// engine streams and relay sockets close cleanly; the result is discarded.
pub async fn cancel(app: &AppHandle) -> Result<(), AppError> {
    let state = app.state::<AppState>();
    let Some(mut active) = state.dictation.lock().await.take() else {
        return Ok(());
    };
    state.dictating.store(false, Ordering::Release);
    crate::telemetry::track(&state, "dictation_cancelled", serde_json::json!({}));
    active.mic.take();
    // Wait for an in-flight connect so a just-created session still gets its
    // bounded finish instead of leaking.
    let _ = active.connect.await;
    let _ = active.bridge.await;
    if let Some(session) = active.session.lock().await.take() {
        let _ = tokio::time::timeout(FINISH_TIMEOUT, session.finish()).await;
    }
    hide_overlay(app);
    // The overlay clears its words on this event; without it a cancelled
    // dictation's text would flash on the next show.
    let _ = app.emit_to(OVERLAY, "dictation-complete", "");
    let _ = app.emit("dictation-active", false);
    tracing::info!("dictation cancelled");
    Ok(())
}

/// Non-empty segment texts joined into one line of dictated speech.
fn join_segments<'a>(texts: impl Iterator<Item = &'a str>) -> String {
    texts
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// What the fallback delivers: the mirrored finalized segments with the
/// last interim (committed + tentative) joined on the end — exactly the
/// text that was on the overlay. The two never overlap: a finalized
/// Segment clears the interim mirror, and the interim's committed part
/// only holds text not yet flushed as a segment.
fn fallback_text(heard: &[String], interim: &str) -> String {
    join_segments(
        heard
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(interim)),
    )
}

/// Hand the finished text to the user per the two output switches. Pasting
/// always stages the text on the clipboard (that is how Ctrl+V works); with
/// the clipboard switch *off*, the previous contents come back once the
/// target app has read it. With it on, the text stays. Neither switch: the
/// text lives only in history.
fn deliver(text: &str, copy: bool, paste: bool) {
    if !copy && !paste {
        return;
    }
    let mut guard = match clipboard().lock() {
        Ok(g) => g,
        Err(e) => {
            tracing::warn!("clipboard mutex poisoned: {e}");
            return;
        }
    };
    let Some(cb) = guard.as_mut() else {
        tracing::warn!("clipboard unavailable");
        return;
    };
    let previous = cb.get_text().ok();
    if let Err(e) = cb.set_text(text.to_string()) {
        tracing::warn!("clipboard write failed: {e}");
        return;
    }
    if !paste {
        return;
    }
    // Drop the mutex guard (not the clipboard) before the chord: the target
    // reads through arboard's own server thread, and the restore thread
    // below wants the same lock.
    drop(guard);
    if let Err(e) = crate::platform::paste_keystroke() {
        // The text is still on the clipboard and in history — a failed
        // paste degrades, it doesn't lose the dictation.
        tracing::warn!("paste keystroke failed: {e}");
    }
    // Give the target app a moment to read the clipboard, then put the
    // user's old contents back — only when they didn't ask to keep the
    // text there.
    if !copy {
        if let Some(prev) = previous {
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(600));
                if let Ok(mut guard) = clipboard().lock() {
                    if let Some(cb) = guard.as_mut() {
                        let _ = cb.set_text(prev);
                    }
                }
            });
        }
    }
}

/// The app's one clipboard handle, alive for the whole process.
///
/// **On X11 this is a correctness requirement, not an optimisation.** The
/// clipboard there is an ownership protocol, not a buffer: the owning client
/// serves the bytes when a target asks for them, which happens *after* the
/// paste chord is delivered. arboard's `Drop` destroys its selection window
/// and hands off to a clipboard manager if one is running — so a
/// per-call handle meant the window was already gone by the time the target
/// asked, and the paste arrived empty (measured on Cinnamon, which runs no
/// such manager: dictation pasted nothing at all while reporting success).
/// Plain "copy to clipboard" had the same hole — the text would vanish the
/// moment `deliver` returned.
///
/// Holding one handle for the process fixes both, and matches what a user
/// means by "it's on my clipboard": ours until they copy something else.
/// Inert on Windows and macOS, whose clipboards really are buffers — the
/// handle there is a cheap wrapper with no OS-level lock held.
fn clipboard() -> &'static std::sync::Mutex<Option<arboard::Clipboard>> {
    static CLIPBOARD: std::sync::OnceLock<std::sync::Mutex<Option<arboard::Clipboard>>> =
        std::sync::OnceLock::new();
    CLIPBOARD.get_or_init(|| {
        std::sync::Mutex::new(
            arboard::Clipboard::new()
                .inspect_err(|e| tracing::warn!("clipboard unavailable: {e}"))
                .ok(),
        )
    })
}

/// The overlay's fixed size: a status row over up to ~4 lines of live
/// words, with real margins all around.
const OVERLAY_SIZE: (f64, f64) = (440.0, 148.0);

/// Create (once) and show the overlay near the bottom of the current
/// monitor. Never focused — the paste target must keep focus.
/// `pub(crate)` for the dev fixture command (commands/fixture.rs).
pub(crate) fn show_overlay(app: &AppHandle) -> Result<(), AppError> {
    let (w, h) = OVERLAY_SIZE;
    let window = match app.get_webview_window(OVERLAY) {
        Some(w) => w,
        None => {
            let window = tauri::WebviewWindowBuilder::new(
                app,
                OVERLAY,
                tauri::WebviewUrl::App("/dictation".into()),
            )
            .title("Dictation")
            .inner_size(w, h)
            .decorations(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .focused(false)
            .resizable(false)
            .visible(false)
            .build()
            .map_err(|e| format!("overlay window failed: {e}"))?;
            // Platform panel behaviors (macOS: join every Space, ride
            // full-screen apps). Native-window access is main-thread work.
            {
                let styled = window.clone();
                let _ = window.run_on_main_thread(move || {
                    crate::platform::style_overlay(&styled);
                });
            }
            window
        }
    };

    let _ = window.set_size(tauri::LogicalSize::new(w, h));
    if let Ok(Some(monitor)) = window.current_monitor() {
        let scale = monitor.scale_factor();
        let screen = monitor.size().to_logical::<f64>(scale);
        let pos = monitor.position().to_logical::<f64>(scale);
        let _ = window.set_position(tauri::LogicalPosition::new(
            pos.x + (screen.width - w) / 2.0,
            pos.y + screen.height - h - 64.0,
        ));
    }
    window.show().map_err(|e| e.to_string())?;
    // Display-only: clicks pass straight through, so the overlay can never
    // steal focus from the paste target on any platform.
    //
    // **After `show()`, not at build time.** On Linux this reaches tao's
    // `CursorIgnoreEvents`, which unwraps the widget's GDK window — and that
    // does not exist until the window is realized. Setting it on the
    // still-invisible window aborted the whole process (a panic in a
    // non-unwinding context, so not even catchable): the first use of
    // dictation killed the app. Both calls travel the same ordered window-
    // request channel, so "after show" is ordering, not a race, and doing it
    // on every show rather than once at creation is idempotent.
    let _ = window.set_ignore_cursor_events(true);
    Ok(())
}

fn hide_overlay(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(OVERLAY) {
        let _ = w.hide();
    }
}

/// Hide the overlay unless a newer dictation has started meanwhile — the
/// slot is free during this stop()'s cleanup tail, so a quick next press
/// legitimately owns the overlay by the time we get here.
fn hide_overlay_if_idle(app: &AppHandle, state: &AppState) {
    if state.dictating.load(Ordering::Acquire) {
        return;
    }
    hide_overlay(app);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "cloud")]
    #[test]
    fn local_model_needed_only_where_the_device_transcribes() {
        use embral_types::{CloudOutOfHours, TranscriptionProvider};

        let mut config = AppConfig::default();

        // Device is the primary: always.
        config.dictation_provider = TranscriptionProvider::Local;
        assert!(needs_local_model(&config));

        // Cloud landing on the device out of hours: still needed.
        config.dictation_provider = TranscriptionProvider::Cloud;
        config.dictation_out_of_hours = CloudOutOfHours::Local;
        assert!(needs_local_model(&config));

        // Cloud with "disabled": failing without a fallback is the ask.
        config.dictation_out_of_hours = CloudOutOfHours::Disabled;
        assert!(!needs_local_model(&config));
    }

    #[test]
    fn fallback_delivers_everything_on_screen() {
        let heard = vec!["First sentence.".to_string(), "Second one.".to_string()];
        // Tentative tail included — the words were visible, they ship.
        assert_eq!(
            fallback_text(&heard, "and a trailing thought"),
            "First sentence. Second one. and a trailing thought"
        );
        // No interim: just the segments.
        assert_eq!(fallback_text(&heard, ""), "First sentence. Second one.");
        // Nothing finalized yet: the interim alone still delivers.
        assert_eq!(fallback_text(&[], "only tentative words"), "only tentative words");
        assert_eq!(fallback_text(&[], ""), "");
    }

    #[test]
    fn tap_toggles_and_hold_pushes_to_talk() {
        // Idle press starts.
        assert_eq!(on_press(false), HotkeyAction::Start);
        // Quick release (tap) keeps the session running…
        assert_eq!(on_release(true, Duration::from_millis(200)), HotkeyAction::Nothing);
        // …and the next press stops it.
        assert_eq!(on_press(true), HotkeyAction::Stop);
        // Holding past the threshold stops on release.
        assert_eq!(on_release(true, Duration::from_millis(900)), HotkeyAction::Stop);
        // Release with nothing running is inert.
        assert_eq!(on_release(false, Duration::from_secs(2)), HotkeyAction::Nothing);
    }
}
