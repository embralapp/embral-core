//! The OS shell theme the tray icons follow ([shell.md](../../../../docs/shell.md)).
//!
//! On Windows the taskbar shade is the `SystemUsesLightTheme` registry
//! value (the taskbar theme; the sibling `AppsUseLightTheme` is the app
//! theme users set independently) and the accent is DWM's `AccentColor`;
//! a registry watcher fires the callback when either changes.

use crate::platform::ThemeSnapshot;

/// Windows' default accent blue: the accent when the registry read fails.
const FALLBACK_ACCENT: [u8; 3] = [0x00, 0x78, 0xd4];

pub fn theme_snapshot() -> ThemeSnapshot {
    ThemeSnapshot {
        is_light: taskbar_is_light(),
        accent_rgb: accent_color(),
    }
}

/// Whether the taskbar is light (missing value = dark, the Windows default).
fn taskbar_is_light() -> bool {
    read_hkcu_dword(
        windows::core::w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
        windows::core::w!("SystemUsesLightTheme"),
    ) == Some(1)
}

/// The user's Windows accent color; the stock blue when unreadable.
fn accent_color() -> [u8; 3] {
    read_hkcu_dword(
        windows::core::w!("Software\\Microsoft\\Windows\\DWM"),
        windows::core::w!("AccentColor"),
    )
    .map(accent_to_rgb)
    .unwrap_or(FALLBACK_ACCENT)
}

/// The DWM `AccentColor` value is ABGR (`0xAABBGGRR`).
fn accent_to_rgb(abgr: u32) -> [u8; 3] {
    [abgr as u8, (abgr >> 8) as u8, (abgr >> 16) as u8]
}

fn read_hkcu_dword(
    subkey: windows::core::PCWSTR,
    value: windows::core::PCWSTR,
) -> Option<u32> {
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};

    let mut data: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            subkey,
            value,
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut core::ffi::c_void),
            Some(&mut size),
        )
    };
    status.is_ok().then_some(data)
}

/// Fire `on_change` whenever the taskbar theme or accent color key changes;
/// the watcher lives for the process. Registry notifications are one-shot,
/// so the fired registration is re-armed after each wake. Failure to set
/// the watcher up leaves the icons correct for the values read at startup:
/// degraded, not broken.
pub fn watch_theme(on_change: Box<dyn Fn() + Send>) {
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
    use windows::Win32::System::Registry::{
        RegNotifyChangeKeyValue, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_NOTIFY,
        REG_NOTIFY_CHANGE_LAST_SET,
    };
    use windows::Win32::System::Threading::{CreateEventW, WaitForMultipleObjects, INFINITE};

    std::thread::spawn(move || {
        let paths = [
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("Software\\Microsoft\\Windows\\DWM"),
        ];
        let mut keys = [HKEY::default(); 2];
        let mut events = [HANDLE::default(); 2];
        for i in 0..2 {
            let opened = unsafe {
                RegOpenKeyExW(HKEY_CURRENT_USER, paths[i], Some(0), KEY_NOTIFY, &mut keys[i])
            };
            if opened.is_err() {
                tracing::warn!("tray theme watcher: failed to open a registry key");
                return;
            }
            match unsafe { CreateEventW(None, false, false, PCWSTR::null()) } {
                Ok(e) => events[i] = e,
                Err(_) => {
                    tracing::warn!("tray theme watcher: failed to create a wait event");
                    return;
                }
            }
        }
        let arm = |i: usize| -> bool {
            unsafe {
                RegNotifyChangeKeyValue(
                    keys[i],
                    false,
                    REG_NOTIFY_CHANGE_LAST_SET,
                    Some(events[i]),
                    true,
                )
            }
            .is_ok()
        };
        if !arm(0) || !arm(1) {
            tracing::warn!("tray theme watcher: failed to arm the notifications");
            return;
        }
        loop {
            let wait = unsafe { WaitForMultipleObjects(&events, false, INFINITE) };
            let idx = wait.0.wrapping_sub(WAIT_OBJECT_0.0) as usize;
            if idx >= events.len() {
                tracing::warn!("tray theme watcher: wait failed, stopping");
                return;
            }
            on_change();
            if !arm(idx) {
                tracing::warn!("tray theme watcher: failed to re-arm, stopping");
                return;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accent_dword_is_abgr() {
        // 0xAABBGGRR: alpha ff, blue d4, green 78, red 00, Windows blue.
        assert_eq!(accent_to_rgb(0xffd4_7800), [0x00, 0x78, 0xd4]);
    }
}
