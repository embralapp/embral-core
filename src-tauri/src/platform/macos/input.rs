//! Keystroke synthesis + focused-app identity: the dictation output's
//! platform surface ([dictation.md](../../../../docs/dictation.md)).
//!
//! Posting synthetic events is gated on the Accessibility permission
//! (not Input Monitoring; we post, never listen), so the paste checks it
//! first and reports a denial instead of posting into the void; the text
//! is on the clipboard and in history either way.

use anyhow::{bail, Result};

use crate::platform::types::{AppId, PermissionState};

/// The ANSI-layout virtual key for `V` (virtual keys are positional, so
/// this is ⌘V on every layout).
const KVK_ANSI_V: u16 = 9;

/// Synthesize the platform paste chord (⌘V) into the focused app.
pub fn paste_keystroke() -> Result<()> {
    use objc2_core_graphics::{CGEvent, CGEventFlags, CGEventTapLocation};

    if super::permissions::check_accessibility() != PermissionState::Granted {
        bail!("auto-paste needs the Accessibility permission (System Settings → Privacy & Security)");
    }

    let chord = |down: bool| -> Result<()> {
        let Some(event) = CGEvent::new_keyboard_event(None, KVK_ANSI_V, down) else {
            bail!("couldn't create the paste key event");
        };
        CGEvent::set_flags(Some(&event), CGEventFlags::MaskCommand);
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
        Ok(())
    };
    chord(true)?;
    chord(false)
}

/// The app that currently has focus: the paste target (the overlay never
/// takes focus). NSWorkspace belongs to the main thread; a synchronous
/// hop covers callers on the runtime.
pub fn focused_app() -> Option<AppId> {
    fn read() -> Option<AppId> {
        let app = objc2_app_kit::NSWorkspace::sharedWorkspace().frontmostApplication()?;
        let pid = app.processIdentifier();
        if pid <= 0 || pid as u32 == std::process::id() {
            return None;
        }
        Some(AppId {
            pid: pid as u32,
            exe: super::mic_users::exe_basename(pid),
            bundle_id: app.bundleIdentifier().map(|s| s.to_string()),
            display_name: app.localizedName().map(|s| s.to_string()),
        })
    }
    if objc2::MainThreadMarker::new().is_some() {
        read()
    } else {
        let (tx, rx) = std::sync::mpsc::channel();
        dispatch2::DispatchQueue::main().exec_async(move || {
            let _ = tx.send(read());
        });
        rx.recv_timeout(std::time::Duration::from_secs(2)).ok().flatten()
    }
}
