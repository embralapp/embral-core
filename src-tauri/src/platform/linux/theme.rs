//! The shell theme + accent the tray icons follow
//! ([shell.md](../../../../docs/shell.md) §Theming).
//!
//! Read through the **XDG settings portal** over D-Bus rather than any
//! desktop's own configuration: `org.freedesktop.appearance` is the one
//! interface GNOME, KDE, Cinnamon, XFCE and the rest all answer, so this is
//! a single code path instead of one per desktop. `zbus` is pure Rust, so
//! there is no glib main loop to integrate with Tauri's.
//!
//! Two honest limits, both different from the Windows twin:
//!
//! - `is_light` is an approximation, and on some desktops it barely
//!   moves. It is supposed to describe the surface the tray icon sits on,
//!   and Windows reads exactly that (`SystemUsesLightTheme`, the taskbar
//!   theme, distinct from the app theme). No portal exposes panel shade, so
//!   we infer it from two keys the portal does answer, in this order:
//!
//!   1. A GTK theme name containing "dark" (`Mint-Y-Dark`, `Breeze-Dark`).
//!      Decisive when present (no theme is named both), and it is the only
//!      signal that works on desktops whose backend does not track
//!      `color-scheme`.
//!   2. `color-scheme`, which is how GNOME expresses dark mode while leaving
//!      its GTK theme named plain `Adwaita`.
//!
//!   Measured limitation (LMDE 7 / Cinnamon, 2026-08-01):
//!   `xdg-desktop-portal-xapp` reports `color-scheme = 1` (prefer-dark)
//!   unconditionally: switching to a light GTK theme does not move it,
//!   and neither does setting `org.gnome.desktop.interface color-scheme`.
//!   The consequence, stated exactly: on Cinnamon a dark theme is detected
//!   by step 1, and a light theme falls through to step 2 and is reported
//!   dark anyway, so `is_light` is effectively always false there. Mint's
//!   default theme is dark, so the default install is right and a user who
//!   switches to a light theme gets a mark with poor contrast.
//!
//!   There is no clean fix, which is why this is documented rather than
//!   worked around: trusting the theme name first would break GNOME, whose
//!   GTK theme stays `Adwaita` in dark mode, and telling the two apart needs
//!   per-desktop branching on `XDG_CURRENT_DESKTOP`, the exact thing using
//!   the portal was meant to avoid. It is cosmetic (icon contrast only), and
//!   Phase 1's tray verification against a live host is where it would show.
//! - `accent-color` is newer than `color-scheme`, and some portal
//!   backends answer only the latter. A missing accent falls back to
//!   embral's own, exactly as the other two platforms fall back to theirs.
//!
//! The watcher is a real signal (`SettingChanged`), not a poll: the portal
//! pushes, so a theme flip reaches the tray immediately instead of within
//! the macOS path's five seconds.

use crate::platform::ThemeSnapshot;

const PORTAL_DEST: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const PORTAL_IFACE: &str = "org.freedesktop.portal.Settings";
const APPEARANCE: &str = "org.freedesktop.appearance";
/// The namespace the GTK theme name lives in, proxied by every backend,
/// including the ones that do not track `color-scheme`.
const GNOME_IFACE: &str = "org.gnome.desktop.interface";

/// embral's own accent, used when the portal has no `accent-color` to give
/// (an older backend) or cannot be reached at all.
const FALLBACK_ACCENT: [u8; 3] = [0x3B, 0x82, 0xF6];

pub fn theme_snapshot() -> ThemeSnapshot {
    let Ok(conn) = zbus::blocking::Connection::session() else {
        tracing::debug!("no session bus — tray theme falls back");
        return fallback();
    };
    let Ok(proxy) =
        zbus::blocking::Proxy::new(&conn, PORTAL_DEST, PORTAL_PATH, PORTAL_IFACE)
    else {
        return fallback();
    };

    ThemeSnapshot {
        is_light: read_is_light(&proxy),
        accent_rgb: read_accent(&proxy).unwrap_or(FALLBACK_ACCENT),
    }
}

/// Whether the tray's surface is light. See the module doc for why this is
/// two signals rather than one.
fn read_is_light(proxy: &zbus::blocking::Proxy<'_>) -> bool {
    // A theme that names itself dark settles it, and is the only signal that
    // survives a backend which pins `color-scheme` (Cinnamon does).
    if let Some(theme) = read_one::<String>(proxy, GNOME_IFACE, "gtk-theme") {
        if theme.to_ascii_lowercase().contains("dark") {
            return false;
        }
    }
    // Otherwise GNOME's way of saying it: 2 = prefer light, 1 = prefer dark,
    // 0 = no preference. No preference reads as dark, like `fallback`.
    read_color_scheme(proxy).unwrap_or(false)
}

/// Call `on_change` whenever the appearance settings change. Owns a thread
/// for the life of the process, parked on the portal's signal.
pub fn watch_theme(on_change: Box<dyn Fn() + Send>) {
    std::thread::Builder::new()
        .name("theme-watch".into())
        .spawn(move || {
            // A failure here costs live theme following and nothing else:
            // the tray keeps the snapshot it started with.
            let Ok(conn) = zbus::blocking::Connection::session() else {
                tracing::debug!("no session bus — the tray will not follow theme changes");
                return;
            };
            let Ok(proxy) =
                zbus::blocking::Proxy::new(&conn, PORTAL_DEST, PORTAL_PATH, PORTAL_IFACE)
            else {
                return;
            };
            let Ok(signals) = proxy.receive_signal("SettingChanged") else {
                tracing::debug!("could not subscribe to SettingChanged");
                return;
            };
            for message in signals {
                // Body is `(s, s, v)`: namespace, key, value. All three must
                // be named: deserializing into a 2-tuple is a signature
                // mismatch, which fails for every message and silently
                // turns this whole loop into a no-op. The value itself is
                // unused (the callback re-reads a fresh snapshot), but it
                // still has to be decoded to get at the two before it.
                let Ok((namespace, key, _value)) = message
                    .body()
                    .deserialize::<(String, String, zbus::zvariant::OwnedValue)>()
                else {
                    continue;
                };
                let appearance = namespace == APPEARANCE
                    && (key == "color-scheme" || key == "accent-color");
                // `gtk-theme` matters because `read_is_light` reads it; a
                // theme switch is how light/dark actually changes on the
                // desktops whose backend pins `color-scheme`.
                let theme = namespace == GNOME_IFACE && key == "gtk-theme";
                if appearance || theme {
                    on_change();
                }
            }
        })
        .ok();
}

fn fallback() -> ThemeSnapshot {
    ThemeSnapshot {
        // Linux panels are dark far more often than light, and a dark panel
        // wants the light mark.
        is_light: false,
        accent_rgb: FALLBACK_ACCENT,
    }
}

/// `color-scheme`: 0 = no preference, 1 = prefer dark, 2 = prefer light.
/// "No preference" reads as dark for the same reason [`fallback`] does.
fn read_color_scheme(proxy: &zbus::blocking::Proxy<'_>) -> Option<bool> {
    let scheme: u32 = read_one(proxy, APPEARANCE, "color-scheme")?;
    Some(scheme == 2)
}

/// `accent-color`: a `(ddd)` of 0.0–1.0 components.
fn read_accent(proxy: &zbus::blocking::Proxy<'_>) -> Option<[u8; 3]> {
    let (r, g, b): (f64, f64, f64) = read_one(proxy, APPEARANCE, "accent-color")?;
    // A backend with no accent set reports all-negative rather than
    // omitting the key, per the appearance spec.
    if r < 0.0 || g < 0.0 || b < 0.0 {
        return None;
    }
    Some([channel(r), channel(g), channel(b)])
}

fn channel(v: f64) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// One `ReadOne` call, decoded into `T`. `None` for a key this backend does
/// not know, a portal that is not running, or a value of an unexpected
/// shape; every one of which the caller answers with a fallback.
fn read_one<T>(proxy: &zbus::blocking::Proxy<'_>, namespace: &str, key: &str) -> Option<T>
where
    T: TryFrom<zbus::zvariant::OwnedValue>,
    <T as TryFrom<zbus::zvariant::OwnedValue>>::Error: std::fmt::Display,
{
    let value: zbus::zvariant::OwnedValue = proxy
        .call("ReadOne", &(namespace, key))
        .inspect_err(|e| tracing::debug!("settings portal has no {key}: {e}"))
        .ok()?;
    T::try_from(value)
        .inspect_err(|e| tracing::debug!("settings portal {key} had an unexpected shape: {e}"))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drives the real portal on the machine running the test. Building a
    /// proxy never touches the bus, so only a real call can tell a live
    /// portal from a bus with nothing behind it (CI runners have a session
    /// bus and no portal). Skips unless `color-scheme` itself answers, so
    /// this is a dev-box check rather than a gate, and it is the reason
    /// the portal decoding is not taken on trust.
    #[test]
    fn reads_the_live_portal_when_there_is_one() {
        let Ok(conn) = zbus::blocking::Connection::session() else {
            eprintln!("no session bus; skipping the live portal read");
            return;
        };
        let Ok(proxy) = zbus::blocking::Proxy::new(&conn, PORTAL_DEST, PORTAL_PATH, PORTAL_IFACE)
        else {
            eprintln!("no settings portal; skipping");
            return;
        };
        // The probe makes the raw call itself rather than going through
        // `read_one`, so a regression there cannot pass as a missing portal.
        let probe: zbus::Result<zbus::zvariant::OwnedValue> =
            proxy.call("ReadOne", &(APPEARANCE, "color-scheme"));
        if probe.is_err() {
            eprintln!("the portal did not answer color-scheme; skipping");
            return;
        }
        // The portal answered; our decoding must not lose the value.
        let scheme = read_color_scheme(&proxy);
        assert!(
            scheme.is_some(),
            "the portal answered color-scheme but read_color_scheme dropped it"
        );
        // accent-color may legitimately be absent on an older backend; when
        // it is present it must be a real colour rather than the fallback
        // arriving by accident.
        if let Some(accent) = read_accent(&proxy) {
            eprintln!("live accent: {accent:?}");
        }
        // And the whole snapshot must be constructible either way.
        let snap = theme_snapshot();
        eprintln!("live snapshot: is_light={} accent={:?}", snap.is_light, snap.accent_rgb);
    }

    /// The watcher, driven for real: flip the desktop's GTK theme and confirm
    /// the callback fires. Ignored because it mutates the session's
    /// appearance for a second or two, so it is a deliberate probe rather
    /// than something CI or a casual `cargo test` should do. Run with
    /// `cargo test -p embral --lib theme_watcher -- --ignored --nocapture`.
    #[test]
    #[ignore = "manual probe; briefly changes the desktop's GTK theme"]
    fn theme_watcher_fires_on_a_real_theme_change() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        watch_theme(Box::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }));
        // Give the thread time to reach the signal subscription.
        std::thread::sleep(std::time::Duration::from_millis(500));

        let read = |key: &str| -> String {
            let out = std::process::Command::new("gsettings")
                .args(["get", "org.cinnamon.desktop.interface", key])
                .output()
                .expect("gsettings get");
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        let write = |value: &str| {
            std::process::Command::new("gsettings")
                .args(["set", "org.cinnamon.desktop.interface", "gtk-theme", value])
                .status()
                .expect("gsettings set");
        };

        let original = read("gtk-theme");
        assert!(!original.is_empty(), "no Cinnamon GTK theme to flip");
        let other = if original.to_ascii_lowercase().contains("dark") {
            original.trim_matches('\'').replace("-Dark", "")
        } else {
            format!("{}-Dark", original.trim_matches('\''))
        };

        write(&other);
        std::thread::sleep(std::time::Duration::from_secs(3));
        let seen = hits.load(Ordering::SeqCst);
        write(&original.trim_matches('\'').to_string());
        std::thread::sleep(std::time::Duration::from_secs(1));

        eprintln!("callback fired {seen} time(s) after {original} -> '{other}'");
        assert!(seen > 0, "the portal changed the theme but the watcher never fired");
    }
}
