//! Overlay-window styling ([dictation.md](../../../../docs/dictation.md)):
//! keep the dictation overlay from ever taking focus.
//!
//! Always-on-top does come from Tauri's own window flag on X11
//! (`_NET_WM_STATE_ABOVE`), and click-through from
//! `set_ignore_cursor_events`, so this module was planned as a no-op
//! ([260801-linux-port.md] said as much). That was wrong, and measurably
//! so. Tauri's `.focused(false)` builder flag is honoured at creation on
//! Windows and macOS, but on GTK the overlay still became the active window
//! the moment it was shown: `_NET_ACTIVE_WINDOW` moved to `'Dictation'`
//! (class `embral`) and the X input focus moved to its webview child.
//!
//! Two things broke as a result, one of them silently:
//!
//! - Auto-paste pasted nothing. XTEST delivers to the input-focus
//!   window, so the Ctrl+V chord went into the overlay's own webview
//!   instead of the user's app. `paste_keystroke` returned `Ok` (it had
//!   posted the events correctly), so nothing logged a failure.
//! - The overlay stole focus mid-typing, which is the single thing
//!   dictation's design says it must never do: the paste target has to keep
//!   focus, and the user should not have their window deactivated to dictate
//!   into it.
//!
//! The fix is the same pair of GTK hints `notice.rs` uses. `focus_on_map`
//! is the one that matters here and must be set before the window is
//! mapped (it is: `style_overlay` runs at creation, `show()` comes later);
//! `accept_focus` covers a later focus attempt. Both are widget-level
//! GtkWindow properties, so unlike the GDK-window calls they are safe on an
//! unrealized window.
//!
//! On Wayland none of this is needed (a client cannot take focus it was not
//! given) and the calls are harmless there. Wayland's other overlay
//! limits stand: no cross-workspace or above-fullscreen placement without
//! `wlr-layer-shell`, which GNOME does not implement and Tauri does not
//! expose, so a full-screen app covers the overlay. Documented degradation,
//! not something this module can fix.
//!
//! [260801-linux-port.md]: ../../../../docs/plans/260801-linux-port.md

/// Keep the overlay out of the focus chain. Takes the Tauri window and
/// reaches for the GTK handle here, so the caller needs no `cfg`
/// (`platform/mod.rs`). Must run on the main thread; the caller uses the
/// window's main-thread hook.
pub fn style_overlay(window: &tauri::WebviewWindow) {
    use gtk::prelude::GtkWindowExt;

    let Ok(gtk_window) = window.gtk_window() else {
        tracing::debug!("no GTK window for the overlay — it may take focus");
        return;
    };
    gtk_window.set_accept_focus(false);
    gtk_window.set_focus_on_map(false);
}
