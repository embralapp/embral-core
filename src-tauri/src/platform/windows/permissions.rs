//! TCC permission state: Windows never gates these capabilities.

use crate::platform::types::PermissionState;

/// The microphone permission right now. Windows: not a thing.
pub fn check_microphone() -> PermissionState {
    PermissionState::NotRequired
}

/// Ask for the microphone. Windows: nothing to ask.
pub async fn request_microphone() -> PermissionState {
    PermissionState::NotRequired
}

/// Synthetic keystrokes need no permission on Windows.
pub fn check_accessibility() -> PermissionState {
    PermissionState::NotRequired
}

/// Nothing to ask on Windows.
pub fn request_accessibility() -> PermissionState {
    PermissionState::NotRequired
}
