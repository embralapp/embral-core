//! Global hotkeys: one shortcut toggles recording, another drives dictation
//! (tap to toggle, hold for push-to-talk). Registered Rust-side (the handlers
//! must run without the webview); re-applied whenever the config changes.

use std::str::FromStr;
use std::sync::{Mutex, OnceLock};

use tauri::{AppHandle, Manager, Wry};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Shortcut, ShortcutState};

use crate::AppState;

/// Which parsed shortcut maps to which action: written by [`apply`], read by
/// the plugin handler (which only receives the fired `Shortcut`).
#[derive(Default)]
struct Routes {
    record: Option<Shortcut>,
    dictation: Option<Shortcut>,
    cancel: Option<Shortcut>,
}

/// Escape, held only while a dictation session is running ([`arm_cancel`]).
/// It has to be a global shortcut rather than a key handler on the overlay:
/// the overlay is built `.focused(false)` and ignores cursor events on every
/// platform, so it never holds keyboard focus and a keydown there would
/// never fire.
fn cancel_shortcut() -> Shortcut {
    Shortcut::new(None, Code::Escape)
}

fn routes() -> &'static Mutex<Routes> {
    static ROUTES: OnceLock<Mutex<Routes>> = OnceLock::new();
    ROUTES.get_or_init(|| Mutex::new(Routes::default()))
}

/// The plugin, with the dispatch handler built in. Registration of the actual
/// combos happens in [`apply`].
pub fn plugin() -> tauri::plugin::TauriPlugin<Wry> {
    tauri_plugin_global_shortcut::Builder::new()
        .with_handler(|app: &AppHandle<Wry>, shortcut, event| {
            let (is_record, is_dictation, is_cancel) = {
                let r = routes().lock().expect("hotkey routes poisoned");
                (
                    r.record.as_ref() == Some(shortcut),
                    r.dictation.as_ref() == Some(shortcut),
                    r.cancel.as_ref() == Some(shortcut),
                )
            };
            if is_record && event.state() == ShortcutState::Pressed {
                let handle = app.clone();
                tauri::async_runtime::spawn(async move {
                    toggle_recording(handle).await;
                });
            }
            if is_dictation {
                handle_dictation_key(app, event.state());
            }
            if is_cancel && event.state() == ShortcutState::Pressed {
                let handle = app.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = crate::dictation::cancel(&handle).await {
                        tracing::warn!("dictation cancel failed: {e}");
                    }
                });
            }
        })
        .build()
}

/// Tap-vs-hold dispatch for the dictation key. The decision rule itself is
/// pure and tested (`dictation::{on_press, on_release}`); this glue reads the
/// sync mirrors and spawns the async start/stop.
fn handle_dictation_key(app: &AppHandle, state_change: ShortcutState) {
    use crate::dictation::{on_press, on_release, HotkeyAction};

    let state = app.state::<AppState>();
    let active = state.dictating.load(std::sync::atomic::Ordering::Acquire);
    let action = match state_change {
        ShortcutState::Pressed => {
            let action = on_press(active);
            if action == HotkeyAction::Start {
                *state
                    .dictation_pressed_at
                    .lock()
                    .expect("pressed-at poisoned") = Some(std::time::Instant::now());
            }
            action
        }
        ShortcutState::Released => {
            let held = state
                .dictation_pressed_at
                .lock()
                .expect("pressed-at poisoned")
                .map(|t| t.elapsed())
                .unwrap_or_default();
            on_release(active, held)
        }
    };

    match action {
        HotkeyAction::Start => {
            let handle = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = crate::dictation::start(&handle).await {
                    tracing::warn!("dictation hotkey start failed: {e}");
                }
            });
        }
        HotkeyAction::Stop => {
            *state
                .dictation_pressed_at
                .lock()
                .expect("pressed-at poisoned") = None;
            let handle = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = crate::dictation::stop(&handle).await {
                    tracing::warn!("dictation hotkey stop failed: {e}");
                }
            });
        }
        HotkeyAction::Nothing => {}
    }
}

async fn toggle_recording(app: AppHandle) {
    let state = app.state::<AppState>();
    let recording = state.recorder.lock().await.is_some();
    if recording {
        // Through the frontend, like every backend-initiated stop, so the
        // notes and title drafts travel with it.
        crate::commands::request_stop(&app);
    } else if let Err(e) = crate::commands::start_recording(app.clone(), app.state()).await {
        tracing::warn!("hotkey start failed: {e}");
    }
}

/// Hold Escape for the life of a dictation session so it can be cancelled
/// without waiting for the words to land. Failure is not fatal: dictation
/// still starts, it just cannot be escaped (something else on the machine
/// already owns the key).
pub fn arm_cancel(app: &AppHandle) {
    let shortcut = cancel_shortcut();
    match app.global_shortcut().register(shortcut) {
        Ok(()) => {
            routes().lock().expect("hotkey routes poisoned").cancel = Some(shortcut);
        }
        Err(e) => tracing::warn!("Escape unavailable for cancelling dictation: {e}"),
    }
}

/// Release Escape at the end of a dictation session. Every exit path calls
/// this, so the key never stays grabbed once the overlay is gone.
pub fn disarm_cancel(app: &AppHandle) {
    let held = routes().lock().expect("hotkey routes poisoned").cancel.take();
    if let Some(shortcut) = held {
        if let Err(e) = app.global_shortcut().unregister(shortcut) {
            tracing::warn!("failed to release the dictation cancel key: {e}");
        }
    }
}

/// (Re)register both shortcuts (empty = none). Invalid combos return an error
/// string for the settings UI; a bad dictation combo doesn't unregister a
/// valid record combo.
pub fn apply(app: &AppHandle, record: &str, dictation: &str) -> Result<(), String> {
    let shortcuts = app.global_shortcut();
    if let Err(e) = shortcuts.unregister_all() {
        tracing::warn!("failed to clear previous hotkeys: {e}");
    }
    *routes().lock().expect("hotkey routes poisoned") = Routes::default();

    let mut errors: Vec<String> = Vec::new();
    let mut register = |label: &str, combo: &str| -> Option<Shortcut> {
        let combo = combo.trim();
        if combo.is_empty() {
            return None;
        }
        let parsed = match Shortcut::from_str(combo) {
            Ok(s) => s,
            Err(e) => {
                errors.push(format!("Couldn't parse {label} hotkey \"{combo}\": {e}"));
                return None;
            }
        };
        match shortcuts.register(parsed) {
            Ok(()) => {
                tracing::info!(combo, "{label} hotkey registered");
                Some(parsed)
            }
            Err(e) => {
                errors.push(format!("Couldn't register {label} hotkey \"{combo}\": {e}"));
                None
            }
        }
    };

    let record_shortcut = register("record", record);
    let dictation_shortcut = register("dictation", dictation);
    {
        let mut r = routes().lock().expect("hotkey routes poisoned");
        r.record = record_shortcut;
        r.dictation = dictation_shortcut;
        // `unregister_all` above dropped Escape too. A settings save during a
        // dictation session is rare but would otherwise leave that session
        // uncancellable.
        r.cancel = None;
    }
    if app
        .state::<AppState>()
        .dictating
        .load(std::sync::atomic::Ordering::Acquire)
    {
        arm_cancel(app);
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join(" "))
    }
}
