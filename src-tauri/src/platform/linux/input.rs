//! Keystroke synthesis + focused-app identity: the dictation output's
//! platform surface ([dictation.md](../../../../docs/dictation.md)).
//!
//! X11, always. `lib.rs` pins GTK to the X11 backend on Linux, so a
//! Wayland session runs the app through Xwayland and this module always has
//! a real X server to talk to. There is no session sniffing here and no
//! Wayland branch: an absent X server is an error like any other.
//!
//! The one thing that narrowing does not buy: XTEST events reach X11 and
//! Xwayland windows only. On a Wayland session a natively Wayland focused
//! app will not receive the paste, and `_NET_ACTIVE_WINDOW` cannot see it
//! either; `focused_app` returns `None` and the text stays on the clipboard,
//! which is the floor the output pipeline guarantees regardless.
//!
//! [260801-linux-port.md]: ../../../../docs/plans/260801-linux-port.md

use anyhow::{anyhow, Context as _, Result};
use x11rb::connection::Connection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};
use x11rb::protocol::xtest::ConnectionExt as _;

use crate::platform::types::AppId;

/// Synthesize the platform paste chord (Ctrl+V) into the focused app, via
/// XTEST, the X11 extension for exactly this, ungated (any client on the
/// display may use it, which is why Wayland refuses to have it).
pub fn paste_keystroke() -> Result<()> {
    // No session sniffing: `lib.rs` pins GTK to the X11 backend, so either an
    // X server answers (a real one, or Xwayland) or there is nothing to paste
    // into and the connection error says so.
    let (conn, _screen) = x11rb::connect(None)
        .context("no X server for the paste chord — the text is on the clipboard")?;

    let control = keycode_for(&conn, KEYSYM_CONTROL_L)?
        .ok_or_else(|| anyhow!("no keycode for Control on this keymap"))?;
    let v = keycode_for(&conn, KEYSYM_LOWER_V)?
        .ok_or_else(|| anyhow!("no keycode for V on this keymap"))?;

    // Press and release through the XTEST fake-input path; window 0 targets
    // whatever has focus, which is the point.
    //
    // Each event is synced through before the next is sent, and the
    // events are spaced. Posting all four with identical zero timestamps
    // in one flush left the releases unprocessed on a real server;
    // measured: the desktop's Control modifier stayed logically held after
    // the chord, turning every subsequent keystroke into a Ctrl-chord until
    // something released it. `sync()` is a full round-trip, so when it
    // returns the server has consumed the event; the pause gives the
    // focused client distinct timestamps, the same reason xdotool defaults
    // to 12 ms between synthetic events.
    for (kind, key) in [
        (x11rb::protocol::xproto::KEY_PRESS_EVENT, control),
        (x11rb::protocol::xproto::KEY_PRESS_EVENT, v),
        (x11rb::protocol::xproto::KEY_RELEASE_EVENT, v),
        (x11rb::protocol::xproto::KEY_RELEASE_EVENT, control),
    ] {
        conn.xtest_fake_input(kind, key, 0, 0u32, 0, 0, 0)
            .context("post the paste key event")?;
        conn.sync().context("sync the paste key event")?;
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    Ok(())
}

/// The app that currently has focus: the paste target (the overlay never
/// takes focus). `_NET_ACTIVE_WINDOW` on the root, then that window's
/// `_NET_WM_PID`, then `/proc` for the executable name. Any gap along the
/// way is `None`: an EWMH-illiterate window manager, a window that never
/// set its pid, a process that exited between reads.
pub fn focused_app() -> Option<AppId> {
    let (conn, screen) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots.get(screen)?.root;

    let net_active = atom(&conn, b"_NET_ACTIVE_WINDOW")?;
    let net_pid = atom(&conn, b"_NET_WM_PID")?;

    let active = conn
        .get_property(false, root, net_active, AtomEnum::WINDOW, 0, 1)
        .ok()?
        .reply()
        .ok()?
        .value32()?
        .next()
        .filter(|&w| w != 0)?;

    let pid = conn
        .get_property(false, active, net_pid, AtomEnum::CARDINAL, 0, 1)
        .ok()?
        .reply()
        .ok()?
        .value32()?
        .next()
        .filter(|&p| p != 0)?;

    if pid == std::process::id() {
        return None;
    }

    // The executable's real basename; `comm` truncates at 15 bytes, so the
    // /proc/exe symlink is the better identity.
    let exe = std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()));

    // WM_CLASS's class half is the app-level identity ("Google-chrome"),
    // stable across windows, unlike the content-driven title.
    let display_name = wm_class(&conn, active);

    if exe.is_none() && display_name.is_none() {
        return None;
    }
    Some(AppId {
        pid,
        exe,
        bundle_id: None,
        display_name,
    })
}

const KEYSYM_LOWER_V: u32 = 0x0076;
const KEYSYM_CONTROL_L: u32 = 0xffe3;

/// First keycode whose keysym table contains `keysym`, from the server's
/// own keyboard mapping; keycodes are layout-dependent, so this is looked
/// up per call rather than hardcoded.
fn keycode_for(conn: &impl Connection, keysym: u32) -> Result<Option<u8>> {
    let setup = conn.setup();
    let (min, max) = (setup.min_keycode, setup.max_keycode);
    let mapping = conn
        .get_keyboard_mapping(min, max - min + 1)
        .context("request the keyboard mapping")?
        .reply()
        .context("read the keyboard mapping")?;
    let per = mapping.keysyms_per_keycode as usize;
    for (i, chunk) in mapping.keysyms.chunks(per).enumerate() {
        if chunk.contains(&keysym) {
            return Ok(Some(min + i as u8));
        }
    }
    Ok(None)
}

fn atom(conn: &impl Connection, name: &[u8]) -> Option<u32> {
    Some(conn.intern_atom(false, name).ok()?.reply().ok()?.atom)
}

/// WM_CLASS is two NUL-terminated strings (instance, then class); the
/// class is the one that names the app.
fn wm_class(conn: &impl Connection, window: u32) -> Option<String> {
    let reply = conn
        .get_property(false, window, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 256)
        .ok()?
        .reply()
        .ok()?;
    let value = reply.value;
    let mut parts = value.split(|&b| b == 0).filter(|s| !s.is_empty());
    let _instance = parts.next();
    let class = parts.next().or(_instance)?;
    let class = String::from_utf8_lossy(class).to_string();
    (!class.is_empty()).then_some(class)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// End-to-end paste mechanism: stage text on the clipboard (arboard, as
    /// the real output pipeline does), post the chord, and keep serving the
    /// selection while the target reads it: on X11 the clipboard is a
    /// conversation, not a buffer, and exits with the process. Orchestrated
    /// from a shell with a `zenity --entry` holding focus.
    #[test]
    #[ignore = "manual probe; pastes into whatever holds focus"]
    fn x11_paste_end_to_end() {
        let mut cb = arboard::Clipboard::new().expect("clipboard");
        cb.set_text("PASTE-PROBE-OK".to_string()).expect("stage text");
        std::thread::sleep(std::time::Duration::from_millis(300));
        eprintln!("focused before paste: {:?}", focused_app());
        paste_keystroke().expect("post the chord");
        // Serve the selection long enough for the target to convert it.
        std::thread::sleep(std::time::Duration::from_secs(2));
    }

    /// Isolation probe: post a bare `v` (no modifier) through the same
    /// fake-input path. If a "v" lands in the focused entry, the mechanism
    /// works and any paste failure is the chord; if not, the fake-input
    /// call itself is wrong.
    #[test]
    #[ignore = "manual probe; types into whatever holds focus"]
    fn x11_type_bare_v() {
        let (conn, _s) = x11rb::connect(None).expect("connect");
        let v = keycode_for(&conn, KEYSYM_LOWER_V).unwrap().expect("keycode");
        eprintln!("using keycode {v} for v");
        conn.xtest_fake_input(x11rb::protocol::xproto::KEY_PRESS_EVENT, v, 0, 0u32, 0, 0, 0)
            .unwrap();
        conn.xtest_fake_input(x11rb::protocol::xproto::KEY_RELEASE_EVENT, v, 0, 0u32, 0, 0, 0)
            .unwrap();
        conn.flush().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(500));
    }

    /// Drives the real X server: reports whatever has focus while the test
    /// runs, and posts an actual Ctrl+V into it.
    #[test]
    #[ignore = "manual probe; reads focus and posts a real Ctrl+V"]
    fn x11_input_probe() {
        eprintln!("X display: {:?}", std::env::var("DISPLAY").ok());
        eprintln!("focused app: {:?}", focused_app());
        paste_keystroke().expect("paste chord posts");
        eprintln!("paste chord posted");
    }
}
