//! The notice window: embral's own notification chrome on every platform
//! ([shell.md] §Notices). One lazily-created, reused window (frameless,
//! always-on-top, never focused, bottom-right of the current monitor)
//! replaces every OS toast. `platform::style_notice` supplies the
//! never-activate guarantee (`WS_EX_NOACTIVATE` on Windows, a
//! non-activating panel on macOS).
//!
//! Strings arrive pre-rendered from the frontend catalog: this module
//! displays them and never writes wording of its own.

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use embral_types::AppError;

const NOTICE: &str = "notice";

/// One fixed size for every notice: a single row of logo, one line of
/// text, and the answers ([shell.md] §Notices).
const NOTICE_SIZE: (f64, f64) = (360.0, 56.0);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoticeAction {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoticePayload {
    /// The notice's family (e.g. `call_detected`, `silence`); same-kind
    /// payloads always replace each other.
    pub kind: String,
    /// The one line of text; a notice carries no body.
    pub title: String,
    #[serde(default)]
    pub actions: Vec<NoticeAction>,
    /// Sticky notices never auto-dismiss and outrank transient ones.
    #[serde(default)]
    pub sticky: bool,
    /// When present, the page renders a countdown to this epoch-ms instant
    /// beside the title: a decision deadline. Chrome only: what the
    /// deadline means is the sender's business; this module still writes
    /// no wording.
    #[serde(default)]
    pub countdown_until_ms: Option<u64>,
    /// Where a click on the text goes (`open_from_notice`); absent = the app.
    #[serde(default)]
    pub target: Option<serde_json::Value>,
}

/// Body-click on a notice: surface the main window (rescued, shown,
/// focused; the tray's path) and tell it where the news lives.
#[tauri::command]
pub async fn open_from_notice(
    app: AppHandle,
    target: serde_json::Value,
) -> Result<(), AppError> {
    *CURRENT.lock().expect("notice state poisoned") = None;
    if let Some(w) = app.get_webview_window(NOTICE) {
        let _ = w.hide();
    }
    if let Some(main) = app.get_webview_window("main") {
        crate::window_rescue::ensure_on_screen(&main);
        let _ = main.unminimize();
        let _ = main.show();
        let _ = main.set_focus();
    }
    let _ = app.emit("notice-navigate", target);
    Ok(())
}

/// Whether an incoming payload may replace what is showing. A transient
/// notice must never clobber a live sticky one (a fallback toast arriving
/// mid silence check-in), but same-kind updates always apply, and sticky
/// replaces anything.
fn should_replace(current: Option<(&str, bool)>, kind: &str, sticky: bool) -> bool {
    match current {
        None => true,
        Some((current_kind, _)) if current_kind == kind => true,
        Some((_, current_sticky)) => sticky || !current_sticky,
    }
}

/// What is currently on the notice window. Cleared on hide so precedence
/// never blocks a fresh notice. The full payload is kept: the first show
/// races the webview's page load, so the page fetches this on mount
/// (`current_notice`) rather than trusting the one-shot emit to arrive.
static CURRENT: std::sync::Mutex<Option<NoticePayload>> = std::sync::Mutex::new(None);

#[tauri::command]
pub async fn notify(app: AppHandle, payload: NoticePayload) -> Result<(), AppError> {
    show_notice(&app, payload)
}

#[tauri::command]
pub async fn hide_notice(app: AppHandle) -> Result<(), AppError> {
    *CURRENT.lock().expect("notice state poisoned") = None;
    if let Some(w) = app.get_webview_window(NOTICE) {
        let _ = w.hide();
    }
    Ok(())
}

/// The payload currently showing: the notice page's source of truth on
/// mount; later updates arrive over `notice-payload`.
#[tauri::command]
pub async fn current_notice() -> Result<Option<NoticePayload>, AppError> {
    Ok(CURRENT.lock().expect("notice state poisoned").clone())
}

fn show_notice(app: &AppHandle, payload: NoticePayload) -> Result<(), AppError> {
    {
        let mut current = CURRENT.lock().expect("notice state poisoned");
        let showing = app
            .get_webview_window(NOTICE)
            .and_then(|w| w.is_visible().ok())
            .unwrap_or(false);
        let live = if showing { current.as_ref() } else { None };
        if !should_replace(
            live.map(|p| (p.kind.as_str(), p.sticky)),
            &payload.kind,
            payload.sticky,
        ) {
            tracing::debug!(kind = payload.kind, "notice dropped behind a sticky one");
            return Ok(());
        }
        *current = Some(payload.clone());
    }

    let (w, h) = NOTICE_SIZE;
    let window = match app.get_webview_window(NOTICE) {
        Some(w) => w,
        None => {
            let window = tauri::WebviewWindowBuilder::new(
                app,
                NOTICE,
                tauri::WebviewUrl::App("/notice".into()),
            )
            .title("Notifications")
            .inner_size(w, h)
            .decorations(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .focused(false)
            // Resizable, despite nothing being able to resize it. On GTK,
            // `resizable(false)` makes the window take the webview's natural
            // size and drops the size hints entirely; measured: the notice
            // came out 360x200 while asking for 360x56, and neither
            // `min_inner_size` nor `max_inner_size` could pull it back. The
            // same window with resizing left on honours its size, exactly as
            // the main window honours its 840x560 minimum. Nothing here is
            // draggable (no decorations, never focused), so this takes
            // nothing away; it is the only way to be the size we asked for.
            .resizable(true)
            .visible(false)
            .build()
            .map_err(|e| format!("notice window failed: {e}"))?;
            // Never activate: a notice matters most mid-call, and even a
            // button click must not pull focus off the meeting app.
            // Native-window access is main-thread work.
            {
                let styled = window.clone();
                let _ = window.run_on_main_thread(move || {
                    crate::platform::style_notice(&styled);
                });
            }
            window
        }
    };

    let _ = window.set_size(tauri::LogicalSize::new(w, h));

    // Bottom-right of the current monitor. Verified on X11 by screenshot:
    // the logical form places it correctly, and an earlier "it lands at
    // x=3088" reading was `wmctrl -lG` being misread, not a real bug.
    if let Ok(Some(monitor)) = window.current_monitor() {
        let scale = monitor.scale_factor();
        let screen = monitor.size().to_logical::<f64>(scale);
        let pos = monitor.position().to_logical::<f64>(scale);
        let _ = window.set_position(tauri::LogicalPosition::new(
            pos.x + screen.width - w - 16.0,
            pos.y + screen.height - h - 72.0,
        ));
    }
    let _ = app.emit_to(NOTICE, "notice-payload", &payload);
    window.show().map_err(|e| e.to_string())?;

    // Kept at info: the notice is chrome-less, so there is no frame to check
    // by eye, and a platform override reads as a design mistake rather than
    // what it is.
    tracing::info!(
        asked = ?(w, h),
        inner = ?window.inner_size(),
        scale = ?window.scale_factor(),
        "notice geometry"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_payload_without_a_countdown_still_parses() {
        // Six of the seven senders predate the field and never send it.
        let bare: NoticePayload =
            serde_json::from_str(r#"{"kind":"notes_ready","title":"Meeting notes ready"}"#)
                .expect("parses without the field");
        assert_eq!(bare.countdown_until_ms, None);
        let with: NoticePayload = serde_json::from_str(
            r#"{"kind":"silence","title":"Still recording?","countdown_until_ms":123}"#,
        )
        .expect("parses with the field");
        assert_eq!(with.countdown_until_ms, Some(123));
    }

    #[test]
    fn transients_never_clobber_a_live_sticky_notice() {
        // A fallback toast mid silence check-in must not replace the question.
        assert!(!should_replace(Some(("silence", true)), "switched_to_local", false));
        // Sticky replaces anything; transient replaces transient.
        assert!(should_replace(Some(("notes_ready", false)), "silence", true));
        assert!(should_replace(Some(("notes_ready", false)), "update_ready", false));
        // Same kind always updates (the silence minutes tick up).
        assert!(should_replace(Some(("silence", true)), "silence", true));
        // An empty window takes whatever comes.
        assert!(should_replace(None, "recording_started", false));
    }
}
