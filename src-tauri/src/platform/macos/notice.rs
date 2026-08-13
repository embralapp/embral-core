//! Notice-window styling ([shell.md](../../../../docs/shell.md) §Notices):
//! make the notice a non-activating panel, so even a button click on it
//! never activates the app. A notice matters most mid-call, and pulling
//! focus off the meeting app would be the worst moment to do it. The
//! Windows twin gets the same guarantee from `WS_EX_NOACTIVATE`.
//!
//! AppKit only honors `NonactivatingPanel` on an `NSPanel`, and Tauri
//! builds an `NSWindow`, so the live window's class is swapped (the
//! technique the tauri-nspanel plugin runs in production). One wrinkle
//! tauri-nspanel ignores: the window isn't a plain `NSWindow` but tao's
//! subclass, which carries one extra ivar, so the swap target is a
//! runtime-registered `NSPanel` subclass padded to the same instance
//! size. Same layout, panel behavior; tao's overrides stop mattering (a
//! notice wants stock panel behavior) and its ivar sits inert for the
//! window's lifetime. If a future tao changes its layout the swap is
//! skipped with a warning; the notice still shows, it just loses the
//! never-activate guarantee. A side benefit while it holds: panels stay
//! out of Mission Control and the Window menu, which is what
//! `skip_taskbar` (a Windows/Linux-only flag) cannot do here.

use std::sync::OnceLock;

use objc2::runtime::{AnyClass, AnyObject, ClassBuilder};
use objc2::ClassType;
use objc2_app_kit::{NSPanel, NSWindowCollectionBehavior, NSWindowStyleMask};

/// The registered swap target: an `NSPanel` subclass whose instance size
/// matches the given window class, or None when padding can't reconcile
/// them. Registered once; every notice window has the same class.
fn panel_class(window_class: &AnyClass) -> Option<&'static AnyClass> {
    static CLASS: OnceLock<Option<&'static AnyClass>> = OnceLock::new();
    *CLASS.get_or_init(|| {
        let base = NSPanel::class();
        let padding = window_class
            .instance_size()
            .checked_sub(base.instance_size())?;
        let mut builder = ClassBuilder::new(c"EmbralNoticePanel", base)?;
        for i in 0..padding {
            // Byte-sized ivars pad without alignment surprises.
            let name = std::ffi::CString::new(format!("_pad{i}")).ok()?;
            builder.add_ivar::<u8>(&name);
        }
        Some(builder.register())
    })
}

/// Apply the macOS panel behaviors to the notice's NSWindow. Takes the
/// Tauri window and extracts the native handle here, so the caller needs no
/// `cfg` (`platform/mod.rs`). Must run on the main thread; the caller uses
/// the window's main-thread hook.
pub fn style_notice(window: &tauri::WebviewWindow) {
    let Ok(native_window) = window.ns_window() else {
        return;
    };
    if native_window.is_null() {
        return;
    }
    let object = unsafe { &*(native_window as *const AnyObject) };
    let Some(class) = panel_class(object.class()) else {
        tracing::warn!(
            class = %object.class(),
            "notice window class can't be padded to a panel — clicks will activate"
        );
        return;
    };
    // SAFETY: `class` descends from NSPanel with the exact instance size
    // of the window's current class (see module doc), and we are on the
    // main thread with the live window Tauri just built.
    let panel: &NSPanel = unsafe {
        AnyObject::set_class(object, class);
        &*(native_window as *const NSPanel)
    };
    panel.setStyleMask(panel.styleMask() | NSWindowStyleMask::NonactivatingPanel);
    panel.setBecomesKeyOnlyIfNeeded(true);
    // Panels hide when their app deactivates by default, and embral is
    // by definition inactive whenever a notice matters.
    panel.setHidesOnDeactivate(false);
    // Same reach as the dictation overlay: every Space, over full-screen
    // meeting apps.
    panel.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
}
