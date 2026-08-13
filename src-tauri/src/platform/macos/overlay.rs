//! Overlay-window styling ([dictation.md](../../../../docs/dictation.md)).
//!
//! Tauri's `always_on_top` alone doesn't surface a window over
//! full-screen Spaces; dictating into a full-screen browser showed no
//! overlay. The fix is NSWindow collection behavior: join every Space and
//! show over full-screen apps as a full-screen auxiliary. (Focus-stealing is handled
//! separately: the overlay ignores cursor events on every platform, so it
//! can never be clicked into activation.)

/// Apply the macOS panel behaviors to the overlay's NSWindow. Takes the
/// Tauri window and extracts the native handle here, so the caller needs no
/// `cfg` (`platform/mod.rs`). Must run on the main thread; the caller uses
/// the window's main-thread hook.
pub fn style_overlay(window: &tauri::WebviewWindow) {
    use objc2_app_kit::{NSWindow, NSWindowCollectionBehavior};

    let Ok(ns_window) = window.ns_window() else {
        return;
    };
    if ns_window.is_null() {
        return;
    }
    let window = unsafe { &*(ns_window as *const NSWindow) };
    window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
}
