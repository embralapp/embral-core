//! Notice-window styling ([shell.md](../../../../docs/shell.md) §Notices):
//! keep the notice from taking focus, so even a button click on it never
//! pulls focus off the meeting app: the same guarantee Windows gets from
//! `WS_EX_NOACTIVATE` and macOS from a non-activating `NSPanel`.
//!
//! Best-effort here, and the two display servers behave differently:
//!
//! - X11: GTK's `accept_focus` / `focus_on_map` map onto the ICCCM hints
//!   a cooperating window manager honors. Most do; a non-compliant one may
//!   still focus the notice, which costs focus and nothing else.
//! - Wayland: the protocol has no focus-stealing to opt out of (a
//!   client cannot take focus it was not given), so the guarantee holds
//!   for free and these calls are inert.
//!
//! Either way a failure is cosmetic: the notice still shows.

/// Ask the window manager not to focus the notice. Takes the Tauri window
/// and reaches for the GTK handle here, so the caller needs no `cfg`
/// (`platform/mod.rs`). Must run on the main thread; the caller uses the
/// window's main-thread hook.
pub fn style_notice(window: &tauri::WebviewWindow) {
    use gtk::prelude::GtkWindowExt;

    let Ok(gtk_window) = window.gtk_window() else {
        tracing::debug!("no GTK window for the notice — it may take focus");
        return;
    };
    gtk_window.set_accept_focus(false);
    gtk_window.set_focus_on_map(false);
}
