//! The tray icon and the running window's taskbar icon. Both carry the bare
//! embral mark in whichever shade the OS shell needs: white on a dark
//! surface, black on a light one. The shade and the accent come from
//! `crate::platform::theme_snapshot()`, and the platform's theme watcher
//! refreshes the icons on change ([shell.md]). The installed icon set
//! (Start menu, installer) is static and keeps its dark tile; only these
//! two runtime surfaces follow the theme.
//!
//! While recording, the whole tray mark (circle and lines) is tinted at
//! runtime in the OS accent color (or the `tray_recording_color` preset).

use anyhow::Result;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::{
    image::Image,
    menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    App, AppHandle, Manager,
};

const TRAY_ID: &str = "main";
const MARK_WHITE_32: &[u8] = include_bytes!("../icons/mark-white-32.png");
const MARK_BLACK_32: &[u8] = include_bytes!("../icons/mark-black-32.png");
const MARK_WHITE_64: &[u8] = include_bytes!("../icons/mark-white-64.png");
const MARK_BLACK_64: &[u8] = include_bytes!("../icons/mark-black-64.png");
const TRAY_SIZE: u32 = 32;

/// Whether a recording is running; the refresh derives the tray icon from it.
static RECORDING: AtomicBool = AtomicBool::new(false);

/// The `tray_recording_color` override, parsed once at config load/save.
/// `None` = follow the OS accent color.
static RECORDING_COLOR: Mutex<Option<[u8; 3]>> = Mutex::new(None);

pub fn create_tray(app: &App) -> Result<()> {
    let show = MenuItemBuilder::with_id("show", "Show Window").build(app)?;
    let quit = MenuItemBuilder::with_id("quit", "Quit").build(app)?;
    let sep = PredefinedMenuItem::separator(app)?;

    let menu = MenuBuilder::new(app).items(&[&show, &sep, &quit]).build()?;

    // Template platforms recolor the mark from its alpha channel (the
    // white variant carries the shape); elsewhere the shade tracks the
    // taskbar theme.
    let idle = if crate::platform::TRAY_IDLE_IS_TEMPLATE
        || !crate::platform::theme_snapshot().is_light
    {
        MARK_WHITE_32
    } else {
        MARK_BLACK_32
    };
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(Image::from_bytes(idle)?)
        .icon_as_template(crate::platform::TRAY_IDLE_IS_TEMPLATE)
        .menu(&menu)
        // On Windows the menu belongs to right-click only: opening it on
        // left-click too raced the toggle below (the menu flashed, or
        // swallowed the click). On macOS the menu on click is the
        // platform convention, so the toggle never fires there.
        .show_menu_on_left_click(crate::platform::TRAY_MENU_ON_LEFT_CLICK)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => {
                if let Some(w) = app.get_webview_window("main") {
                    // `unminimize` first, matching the single-instance rescue
                    // (`lib.rs`): a window that was minimised rather than
                    // hidden stays invisible through `show()` alone, and on
                    // Wayland the two states are harder to tell apart than
                    // elsewhere; the app reporting itself visible while
                    // nothing is on screen is exactly the reachable-nowhere
                    // failure this menu item exists to prevent.
                    let _ = w.unminimize();
                    crate::window_rescue::ensure_on_screen(&w);
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Some(w) = app.get_webview_window("main") {
                    if w.is_visible().unwrap_or(false) {
                        let _ = w.hide();
                    } else {
                        crate::window_rescue::ensure_on_screen(&w);
                        let _ = w.show();
                        let _ = w.set_focus();
                    }
                }
            }
        })
        .build(app)?;

    refresh(app.handle())?;
    // Failure inside the watcher degrades to startup-time icons.
    let handle = app.handle().clone();
    crate::platform::watch_theme(Box::new(move || {
        if let Err(e) = refresh(&handle) {
            tracing::warn!("tray theme watcher: refresh failed: {e}");
        }
    }));
    Ok(())
}

pub fn update_tray_recording_state(app: &AppHandle, recording: bool) -> Result<()> {
    RECORDING.store(recording, Ordering::Relaxed);
    refresh(app)
}

/// Re-parse the recording-disc override. Called at startup and whenever the
/// config is saved; anything that isn't `#RRGGBB` means "follow the accent".
pub fn set_recording_color(hex: &str) {
    *RECORDING_COLOR.lock().unwrap() = parse_hex(hex);
}

/// Re-derive both runtime icons from current state: the tray from the
/// recording flag, taskbar shade, and disc color; the window's taskbar icon
/// from the shade alone. The window may not exist yet during setup; it is
/// skipped, and the next refresh catches it.
pub fn refresh(app: &AppHandle) -> Result<()> {
    let light = crate::platform::theme_snapshot().is_light;
    let recording = RECORDING.load(Ordering::Relaxed);
    let tray_icon = if recording {
        recording_icon()?
    } else if crate::platform::TRAY_IDLE_IS_TEMPLATE || !light {
        Image::from_bytes(MARK_WHITE_32)?
    } else {
        Image::from_bytes(MARK_BLACK_32)?
    };
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        // The recording tint must keep its accent color, so template mode
        // pauses for it and resumes for the idle mark.
        let _ = tray.set_icon_as_template(crate::platform::TRAY_IDLE_IS_TEMPLATE && !recording);
        tray.set_icon(Some(tray_icon))?;
    }
    let window_bytes = if light { MARK_BLACK_64 } else { MARK_WHITE_64 };
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.set_icon(Image::from_bytes(window_bytes)?);
    }
    Ok(())
}

/// The current OS accent color as `#RRGGBB`: the swatch beside the
/// system-accent choice in settings.
#[tauri::command]
pub fn system_accent_color() -> String {
    let [r, g, b] = crate::platform::theme_snapshot().accent_rgb;
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// The recording tray icon: the whole mark tinted the override-or-accent
/// color.
fn recording_icon() -> Result<Image<'static>> {
    let color = RECORDING_COLOR
        .lock()
        .unwrap()
        .unwrap_or_else(|| crate::platform::theme_snapshot().accent_rgb);
    let mark = Image::from_bytes(MARK_WHITE_32)?;
    let buf = tint(mark.rgba(), color);
    Ok(Image::new_owned(buf, TRAY_SIZE, TRAY_SIZE))
}

/// `#RRGGBB` → RGB; anything else is None (= follow the accent).
fn parse_hex(hex: &str) -> Option<[u8; 3]> {
    let hex = hex.trim().strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(hex, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

/// Recolor a mark: every pixel's RGB becomes `rgb` while its alpha (the
/// mark's shape, anti-aliased edges included) stays.
fn tint(rgba: &[u8], rgb: [u8; 3]) -> Vec<u8> {
    let mut out = rgba.to_vec();
    for px in out.chunks_exact_mut(4) {
        px[0] = rgb[0];
        px[1] = rgb[1];
        px[2] = rgb[2];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_parses_and_rejects() {
        assert_eq!(parse_hex("#cc0000"), Some([0xcc, 0, 0]));
        assert_eq!(parse_hex(" #00FF7f "), Some([0, 0xff, 0x7f]));
        assert_eq!(parse_hex(""), None);
        assert_eq!(parse_hex("cc0000"), None);
        assert_eq!(parse_hex("#cc00"), None);
        assert_eq!(parse_hex("#zzzzzz"), None);
    }

    #[test]
    fn tint_recolors_but_keeps_the_shape() {
        // Opaque white, half-covered edge, empty background.
        let rgba = [255, 255, 255, 255, 255, 255, 255, 128, 0, 0, 0, 0];
        let out = tint(&rgba, [0xb9, 0x1c, 0x1c]);
        assert_eq!(out[0..4], [0xb9, 0x1c, 0x1c, 255]);
        assert_eq!(out[4..8], [0xb9, 0x1c, 0x1c, 128]);
        assert_eq!(out[8..12], [0xb9, 0x1c, 0x1c, 0]);
    }
}
