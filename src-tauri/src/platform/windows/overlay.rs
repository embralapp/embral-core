//! Overlay-window styling; nothing extra on Windows: `always_on_top` +
//! `skip_taskbar` already behave.

/// Apply platform panel behaviors to the overlay's window. No-op. Takes the
/// Tauri window like its siblings so the caller needs no `cfg`.
pub fn style_overlay(_window: &tauri::WebviewWindow) {}
