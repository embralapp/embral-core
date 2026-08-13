//! Who is using the microphone right now?
//!
//! Core Audio's HAL keeps an object per audio client process; per-object
//! `kAudioProcessPropertyIsRunningInput` says "this process has an active
//! input stream", the same predicate as the Windows WASAPI session scan,
//! and no TCC permission gates the property reads. Property listeners
//! on these objects are unreliable (macOS 15.0.x), so the caller's 3 s
//! poll drives this. Any HAL failure degrades to an empty list;
//! detection must never take the app down.
//!
//! Identity: the capturing process is often a helper
//! (`com.google.Chrome.helper`), so each hit carries every identity the
//! platform has (the HAL's bundle id, `NSRunningApplication`'s app-level
//! name/bundle where the pid is an app, and the executable basename via
//! `proc_pidpath` as the floor), and the matcher tests them all.

use std::ffi::c_void;
use std::ptr::NonNull;

use objc2_core_audio::{
    kAudioHardwarePropertyProcessObjectList, kAudioObjectPropertyElementMain,
    kAudioObjectPropertyScopeGlobal, kAudioObjectSystemObject, kAudioProcessPropertyBundleID,
    kAudioProcessPropertyIsRunningInput, kAudioProcessPropertyPID, AudioObjectGetPropertyData,
    AudioObjectGetPropertyDataSize, AudioObjectID, AudioObjectPropertyAddress,
    AudioObjectPropertySelector,
};

use crate::platform::types::AppId;

/// Apps with an active microphone stream, excluding `exclude_pid` (our own
/// recorder).
pub fn processes_using_microphone(exclude_pid: u32) -> Vec<AppId> {
    let mut apps: Vec<AppId> = Vec::new();
    for object in process_objects() {
        if read_scalar::<u32>(object, kAudioProcessPropertyIsRunningInput).unwrap_or(0) == 0 {
            continue;
        }
        let Some(pid) = read_scalar::<i32>(object, kAudioProcessPropertyPID) else {
            continue;
        };
        if pid <= 0 || pid as u32 == exclude_pid {
            continue;
        }
        let app = app_id(pid, read_cfstring(object, kAudioProcessPropertyBundleID));
        // One row per app: helpers and their owner share a bundle-id
        // prefix or an exe, and the first observation carries enough
        // identity for the matcher.
        let key = |a: &AppId| {
            a.bundle_id
                .clone()
                .or_else(|| a.exe.clone())
                .unwrap_or_else(|| a.pid.to_string())
        };
        if !apps.iter().any(|a| key(a) == key(&app)) {
            apps.push(app);
        }
    }
    apps
}

/// Every identity we can resolve for a pid the HAL says is capturing.
fn app_id(pid: i32, hal_bundle_id: Option<String>) -> AppId {
    let running =
        objc2_app_kit::NSRunningApplication::runningApplicationWithProcessIdentifier(pid);
    let (app_bundle, display_name) = running
        .map(|app| {
            (
                app.bundleIdentifier().map(|s| s.to_string()),
                app.localizedName().map(|s| s.to_string()),
            )
        })
        .unwrap_or((None, None));
    AppId {
        pid: pid as u32,
        exe: exe_basename(pid),
        // The HAL's id names the actual capturing process (helpers
        // included); the app-level id fills in when the HAL has none.
        bundle_id: hal_bundle_id.or(app_bundle),
        display_name,
    }
}

/// Executable basename via `proc_pidpath`; works for helpers that aren't
/// "applications". Shared with `input::focused_app`.
pub(super) fn exe_basename(pid: i32) -> Option<String> {
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr() as *mut c_void, buf.len() as u32) };
    if n <= 0 {
        return None;
    }
    let path = String::from_utf8_lossy(&buf[..n as usize]).to_string();
    std::path::Path::new(&path)
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
}

fn address(selector: AudioObjectPropertySelector) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    }
}

/// The HAL's process-object list (empty on any error).
fn process_objects() -> Vec<AudioObjectID> {
    let mut addr = address(kAudioHardwarePropertyProcessObjectList);
    let mut size: u32 = 0;
    let status = unsafe {
        AudioObjectGetPropertyDataSize(
            kAudioObjectSystemObject as AudioObjectID,
            NonNull::from(&mut addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
        )
    };
    if status != 0 || size == 0 {
        return Vec::new();
    }
    let count = size as usize / std::mem::size_of::<AudioObjectID>();
    let mut list = vec![0 as AudioObjectID; count];
    let status = unsafe {
        AudioObjectGetPropertyData(
            kAudioObjectSystemObject as AudioObjectID,
            NonNull::from(&mut addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::new(list.as_mut_ptr() as *mut c_void).expect("vec ptr"),
        )
    };
    if status != 0 {
        return Vec::new();
    }
    list.truncate(size as usize / std::mem::size_of::<AudioObjectID>());
    list
}

/// A fixed-size scalar property, `None` on any error.
fn read_scalar<T: Copy + Default>(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
) -> Option<T> {
    let mut addr = address(selector);
    let mut value = T::default();
    let mut size = std::mem::size_of::<T>() as u32;
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&mut addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::new(&mut value as *mut T as *mut c_void).expect("value ptr"),
        )
    };
    (status == 0 && size as usize == std::mem::size_of::<T>()).then_some(value)
}

/// A CFString property (the caller owns the returned reference per the
/// get-property convention), `None` on error or empty.
fn read_cfstring(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
) -> Option<String> {
    use objc2_core_foundation::CFString;

    let mut addr = address(selector);
    let mut string_ref: *mut CFString = std::ptr::null_mut();
    let mut size = std::mem::size_of::<*mut CFString>() as u32;
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&mut addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::new(&mut string_ref as *mut *mut CFString as *mut c_void).expect("ref ptr"),
        )
    };
    if status != 0 {
        return None;
    }
    let string = unsafe { objc2_core_foundation::CFRetained::from_raw(NonNull::new(string_ref)?) };
    let s = string.to_string();
    (!s.is_empty()).then_some(s)
}

#[cfg(test)]
mod tests {
    /// Live probe of the real HAL scan; run manually while some app holds
    /// a microphone stream:
    /// `cargo test -p embral --lib scan_lists -- --ignored --nocapture`.
    /// (On a machine with no input device it prints an empty list and
    /// proves the property reads don't error.)
    #[test]
    #[ignore = "manual probe; needs an app actively capturing the mic"]
    fn scan_lists_mic_users() {
        let apps = super::processes_using_microphone(0);
        eprintln!("active mic sessions: {apps:?}");
    }
}
