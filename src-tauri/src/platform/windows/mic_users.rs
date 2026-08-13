//! Who is using the microphone right now?
//!
//! Enumerates WASAPI audio sessions on every active capture endpoint and
//! returns the process names holding an active stream, the most reliable
//! signal that a call is underway. Runs on a blocking thread each poll tick.
//! Any COM failure degrades to an empty list (detection must never take the
//! app down); warnings are throttled by the caller.

use windows::core::Interface;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Media::Audio::{
    eCapture, AudioSessionStateActive, IAudioSessionControl2, IAudioSessionManager2,
    IMMDeviceEnumerator, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
};
use windows::Win32::System::ProcessStatus::K32GetModuleBaseNameW;
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
};

use crate::platform::types::AppId;

/// Apps (identified by exe name, e.g. `Zoom.exe`) with an active microphone
/// session, excluding `exclude_pid` (our own recorder).
pub fn processes_using_microphone(exclude_pid: u32) -> Vec<AppId> {
    match scan(exclude_pid) {
        Ok(names) => names,
        Err(e) => {
            tracing::debug!("mic-session scan failed: {e}");
            Vec::new()
        }
    }
}

fn scan(exclude_pid: u32) -> windows::core::Result<Vec<AppId>> {
    unsafe {
        // Idempotent per-thread init; RPC_E_CHANGED_MODE (already initialized
        // with a different model) is fine for our read-only usage.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let devices = enumerator.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE)?;

        let mut names: Vec<AppId> = Vec::new();
        for i in 0..devices.GetCount()? {
            let device = match devices.Item(i) {
                Ok(d) => d,
                Err(_) => continue,
            };
            let manager: IAudioSessionManager2 = match device.Activate(CLSCTX_ALL, None) {
                Ok(m) => m,
                Err(_) => continue,
            };
            let sessions = match manager.GetSessionEnumerator() {
                Ok(s) => s,
                Err(_) => continue,
            };
            for j in 0..sessions.GetCount().unwrap_or(0) {
                let Ok(control) = sessions.GetSession(j) else {
                    continue;
                };
                let Ok(state) = control.GetState() else {
                    continue;
                };
                if state != AudioSessionStateActive {
                    continue;
                }
                let Ok(control2) = control.cast::<IAudioSessionControl2>() else {
                    continue;
                };
                let Ok(pid) = control2.GetProcessId() else {
                    continue;
                };
                if pid == 0 || pid == exclude_pid {
                    continue; // 0 = system sounds session
                }
                match process_name(pid) {
                    Some(name) => {
                        if !names.iter().any(|n| n.exe.as_deref() == Some(&name)) {
                            names.push(AppId::from_exe(pid, name));
                        }
                    }
                    // The implicit liveness check: a session whose process is
                    // gone can't be named, and can't hold a call open.
                    None => tracing::debug!(pid, "active mic session with no live process; skipped"),
                }
            }
        }
        Ok(names)
    }
}

#[cfg(test)]
mod tests {
    /// Live probe of the real COM scan; run manually while some app holds a
    /// microphone stream: `cargo test -p embral --lib scan_lists -- --ignored --nocapture`.
    #[test]
    #[ignore = "manual probe; needs an app actively capturing the mic"]
    fn scan_lists_mic_users() {
        let names = super::processes_using_microphone(0);
        eprintln!("active mic sessions: {names:?}");
    }
}

/// Executable base name for a PID (e.g. `Zoom.exe`). Also used by dictation
/// to identify the focused app.
pub fn process_name(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid).ok()?;
        let mut buf = [0u16; 260];
        let len = K32GetModuleBaseNameW(handle, None, &mut buf);
        let _ = CloseHandle(handle);
        if len == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}
