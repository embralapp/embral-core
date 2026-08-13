//! What is playing audio right now?
//!
//! The sibling of `mic_users.rs`: the same WASAPI session walk, over
//! render endpoints instead of capture ones. It answers "which apps
//! could this recording include", which the source picker lists
//! ([recording.md](../../../../docs/recording.md) §Dual-stream capture).
//!
//! Read-only enumeration: no activation, no hosted COM object. Any COM
//! failure degrades to an empty list: the picker then shows no apps and
//! the recording keeps capturing everything, which is the safe default.

use windows::core::Interface;
use windows::Win32::Media::Audio::{
    eRender, AudioSessionStateExpired, IAudioSessionControl2, IAudioSessionManager2,
    IMMDeviceEnumerator, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
};

use crate::platform::types::AppId;

/// Apps with an audio session on any active output device, excluding our
/// own. Sessions that are merely inactive (an app between sounds) are
/// included: dropping them would make rows flicker in and out of the
/// picker between utterances. Expired sessions are skipped; that app is
/// gone.
pub fn apps_playing_audio(exclude_pid: u32) -> Vec<AppId> {
    match scan(exclude_pid) {
        Ok(apps) => apps,
        Err(e) => {
            tracing::debug!("render-session scan failed: {e}");
            Vec::new()
        }
    }
}

fn scan(exclude_pid: u32) -> windows::core::Result<Vec<AppId>> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let devices = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;

        let mut pids: Vec<u32> = Vec::new();
        for i in 0..devices.GetCount()? {
            let Ok(device) = devices.Item(i) else { continue };
            let Ok(manager) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else {
                continue;
            };
            let Ok(sessions) = manager.GetSessionEnumerator() else {
                continue;
            };
            for j in 0..sessions.GetCount().unwrap_or(0) {
                let Ok(control) = sessions.GetSession(j) else {
                    continue;
                };
                if control.GetState().is_ok_and(|s| s == AudioSessionStateExpired) {
                    continue;
                }
                let Ok(control2) = control.cast::<IAudioSessionControl2>() else {
                    continue;
                };
                let Ok(pid) = control2.GetProcessId() else {
                    continue;
                };
                // 0 is the system-sounds session; our own audio must never
                // feed back into a recording.
                if pid == 0 || pid == exclude_pid {
                    continue;
                }
                pids.push(pid);
            }
        }
        Ok(named(&pids))
    }
}

/// Pids → named apps, deduped, keeping first-seen order. Split out so the
/// list-shaping is testable without a COM session in the room.
fn named(pids: &[u32]) -> Vec<AppId> {
    let mut apps: Vec<AppId> = Vec::new();
    for &pid in pids {
        if apps.iter().any(|a| a.pid == pid) {
            continue;
        }
        let Some(name) = super::mic_users::process_name(pid) else {
            // The process died between enumeration and naming.
            continue;
        };
        apps.push(AppId::from_exe(pid, name));
    }
    apps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_app_on_two_endpoints_is_listed_once() {
        // An app rendering to both speakers and a headset has a session on
        // each; the picker must show one row, not two.
        let own = std::process::id();
        let listed = named(&[own, own]);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].pid, own);
        assert!(listed[0].exe.is_some(), "a live pid always names");
    }

    #[test]
    fn dead_pids_drop_out_rather_than_appearing_unnamed() {
        // Nothing can be named for a pid that no longer exists; a blank
        // row would be worse than no row.
        assert!(named(&[u32::MAX]).is_empty());
    }
}
