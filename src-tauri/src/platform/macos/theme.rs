//! The OS shell theme the tray icons follow ([shell.md](../../../../docs/shell.md)).
//!
//! Everything reads through `NSUserDefaults`' global domain: thread-safe,
//! no AppKit: `AppleInterfaceStyle` is `"Dark"` in dark mode and absent in
//! light; `AppleAccentColor` is the accent's palette index and absent for
//! the default (multicolour, which renders blue). The idle menu-bar icon
//! is a template image and recolors itself, so `is_light` only matters to
//! the window-icon path (a no-op on macOS anyway); the accent drives the
//! recording tint and the settings swatch.

use objc2_foundation::{ns_string, NSUserDefaults};

use crate::platform::ThemeSnapshot;

/// macOS's default accent (system blue), used for the absent
/// (multicolour) setting and any unknown index.
const FALLBACK_ACCENT: [u8; 3] = [0x00, 0x7a, 0xff];

/// The fixed macOS accent palette, by `AppleAccentColor` index.
fn accent_for_index(index: i64) -> [u8; 3] {
    match index {
        -1 => [0x8c, 0x8c, 0x8c], // graphite
        0 => [0xff, 0x52, 0x59],  // red
        1 => [0xf7, 0x82, 0x1b],  // orange
        2 => [0xff, 0xc6, 0x00],  // yellow
        3 => [0x62, 0xba, 0x46],  // green
        4 => FALLBACK_ACCENT,     // blue
        5 => [0xa5, 0x50, 0xa7],  // purple
        6 => [0xf7, 0x4f, 0x9e],  // pink
        _ => FALLBACK_ACCENT,
    }
}

pub fn theme_snapshot() -> ThemeSnapshot {
    let defaults = NSUserDefaults::standardUserDefaults();
    let is_light = defaults
        .stringForKey(ns_string!("AppleInterfaceStyle"))
        .map(|s| s.to_string() != "Dark")
        .unwrap_or(true); // absent = light, the macOS default
    // objectForKey distinguishes "absent" (multicolour → blue) from index 0
    // (red); integerForKey alone couldn't.
    let accent_rgb = if defaults.objectForKey(ns_string!("AppleAccentColor")).is_some() {
        accent_for_index(defaults.integerForKey(ns_string!("AppleAccentColor")) as i64)
    } else {
        FALLBACK_ACCENT
    };
    ThemeSnapshot { is_light, accent_rgb }
}

/// Fire `on_change` when the appearance or accent changes. A lazy poll:
/// the reads are two defaults lookups, and a theme flip taking a few
/// seconds to reach a menu-bar tint is imperceptible. (A
/// distributed-notification observer is the upgrade if it ever matters.)
pub fn watch_theme(on_change: Box<dyn Fn() + Send>) {
    std::thread::Builder::new()
        .name("theme-watch".into())
        .spawn(move || {
            let mut last = snapshot_key();
            loop {
                std::thread::sleep(std::time::Duration::from_secs(5));
                let now = snapshot_key();
                if now != last {
                    last = now;
                    on_change();
                }
            }
        })
        .ok();
}

fn snapshot_key() -> (bool, [u8; 3]) {
    let s = theme_snapshot();
    (s.is_light, s.accent_rgb)
}
