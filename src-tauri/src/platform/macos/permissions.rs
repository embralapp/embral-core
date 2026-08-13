//! TCC permission state ([shell.md](../../../../docs/shell.md) §Onboarding).
//!
//! The microphone is the boot milestone's one gate: the OS prompts on
//! first capture, and a denial makes every later stream silently useless,
//! so onboarding asks up front and settings can point at System Settings
//! when denied. Accessibility gates dictation's auto-paste (posting
//! synthetic keystrokes). System-audio TCC has no public query API; that
//! stays probe-only in `loopback.rs`.

use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};

use crate::platform::types::PermissionState;

fn map(status: AVAuthorizationStatus) -> PermissionState {
    match status {
        AVAuthorizationStatus::Authorized => PermissionState::Granted,
        AVAuthorizationStatus::NotDetermined => PermissionState::NotDetermined,
        // Denied and Restricted both need System Settings to fix.
        _ => PermissionState::Denied,
    }
}

/// The audio media-type constant (a linked static; present on every
/// supported macOS).
fn media_type_audio() -> &'static objc2_foundation::NSString {
    unsafe { AVMediaTypeAudio.expect("AVMediaTypeAudio missing") }
}

/// The microphone permission right now, without prompting.
pub fn check_microphone() -> PermissionState {
    // SAFETY: a status query on the documented authorization API.
    unsafe { map(AVCaptureDevice::authorizationStatusForMediaType(media_type_audio())) }
}

/// Ask for the microphone. Prompts only in the NotDetermined state (the OS
/// ignores repeat requests otherwise); resolves once the user answers.
pub async fn request_microphone() -> PermissionState {
    let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
    // The block is scoped so only ObjC's own copy survives the call; the
    // RcBlock handle is !Send and must not be held across the await.
    {
        let tx = std::sync::Mutex::new(Some(tx));
        let handler = block2::RcBlock::new(move |granted: objc2::runtime::Bool| {
            if let Some(tx) = tx.lock().expect("mic-permission handler poisoned").take() {
                let _ = tx.send(granted.as_bool());
            }
        });
        // SAFETY: the documented request API; it copies the escaping
        // handler, which fires exactly once on an arbitrary dispatch
        // queue; the oneshot absorbs that.
        unsafe {
            AVCaptureDevice::requestAccessForMediaType_completionHandler(
                media_type_audio(),
                &handler,
            );
        }
    }
    match rx.await {
        Ok(true) => PermissionState::Granted,
        Ok(false) => PermissionState::Denied,
        Err(_) => check_microphone(),
    }
}

/// Whether this process is a trusted Accessibility client (the gate on
/// posting synthetic keystrokes). AX has no "never asked" query; an
/// untrusted process reads as denied, and [`request_accessibility`]'s
/// prompt is how the ask happens.
pub fn check_accessibility() -> PermissionState {
    let trusted = unsafe { objc2_application_services::AXIsProcessTrustedWithOptions(None) };
    if trusted {
        PermissionState::Granted
    } else {
        PermissionState::Denied
    }
}

/// Ask for Accessibility: shows the system's one-time prompt (it opens
/// System Settings; granting needs an app restart to matter for an
/// already-running event post, but the state itself reads live). Returns
/// the current state; the prompt resolves out-of-band.
pub fn request_accessibility() -> PermissionState {
    use objc2_foundation::{NSDictionary, NSNumber, NSString};

    let key = unsafe { objc2_application_services::kAXTrustedCheckOptionPrompt };
    // Toll-free bridge: the CFString key is an NSString, the NSDictionary
    // is the CFDictionary the AX call wants.
    let ns_key = unsafe { &*(key as *const _ as *const NSString) };
    let options = NSDictionary::from_retained_objects(
        &[ns_key],
        &[unsafe {
            objc2::rc::Retained::cast_unchecked::<objc2::runtime::AnyObject>(NSNumber::new_bool(
                true,
            ))
        }],
    );
    let trusted = unsafe {
        objc2_application_services::AXIsProcessTrustedWithOptions(Some(
            &*(objc2::rc::Retained::as_ptr(&options)
                as *const objc2_core_foundation::CFDictionary),
        ))
    };
    if trusted {
        PermissionState::Granted
    } else {
        PermissionState::Denied
    }
}
