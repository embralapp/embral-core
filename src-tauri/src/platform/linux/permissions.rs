//! Permission state: Linux never gates these capabilities.
//!
//! Nothing stands between a desktop app and the microphone here: PipeWire
//! and ALSA hand over a capture stream on request, and there is no TCC-style
//! prompt to check or trigger. Synthetic keystrokes are likewise ungated on
//! X11 (XTEST is open to any client on the display), which is exactly why
//! Wayland refuses them, but a Wayland refusal is a capability gap
//! reported by `input.rs`, not a permission the user can grant, so it does
//! not surface here.
//!
//! With everything `NotRequired`, the two macOS permission cards render
//! nothing on Linux; their `PermissionState` gating already handles that
//! ([shell.md](../../../../docs/shell.md) §Onboarding).

use crate::platform::types::PermissionState;

/// The microphone permission right now. Linux: not a thing.
pub fn check_microphone() -> PermissionState {
    PermissionState::NotRequired
}

/// Ask for the microphone. Linux: nothing to ask.
pub async fn request_microphone() -> PermissionState {
    PermissionState::NotRequired
}

/// Synthetic keystrokes need no permission on Linux. (Whether they are
/// possible is a display-server question; see `input.rs`.)
pub fn check_accessibility() -> PermissionState {
    PermissionState::NotRequired
}

/// Nothing to ask on Linux.
pub fn request_accessibility() -> PermissionState {
    PermissionState::NotRequired
}
