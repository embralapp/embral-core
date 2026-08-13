//! Where this machine's power comes from
//! ([transcription.md](../../../../docs/transcription.md) §Provider selection).

use crate::platform::types::PowerSource;

/// `BatteryFlag` value meaning "this system has no battery": a desktop.
/// Reported alone, never OR'd with the charge-level bits (1/2/4/8), and
/// distinct from 255, which means the flag itself is unreadable.
const BATTERY_FLAG_NO_BATTERY: u8 = 128;
/// `ACLineStatus`: running on wall power.
const AC_LINE_ONLINE: u8 = 1;
/// `ACLineStatus`: running on battery.
const AC_LINE_OFFLINE: u8 = 0;

/// Read once per recording, so a cheap synchronous call is right.
pub fn power_source() -> PowerSource {
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    let mut status = SYSTEM_POWER_STATUS::default();
    // SAFETY: the documented API filling a stack struct we own.
    if unsafe { GetSystemPowerStatus(&mut status) }.is_err() {
        return PowerSource::Unknown;
    }
    classify(status.ACLineStatus, status.BatteryFlag)
}

/// The mapping, split out so it can be tested without an OS call.
/// `ACLineStatus` is 255 on machines that won't say; a system with no
/// battery is at a desk by definition, whatever it claims about AC.
fn classify(ac_line_status: u8, battery_flag: u8) -> PowerSource {
    if battery_flag == BATTERY_FLAG_NO_BATTERY {
        return PowerSource::Plugged;
    }
    match ac_line_status {
        AC_LINE_ONLINE => PowerSource::Plugged,
        AC_LINE_OFFLINE => PowerSource::Battery,
        _ => PowerSource::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_desktop_with_no_battery_is_plugged_in() {
        // 255 = "unknown" AC status, which desktops do report. The absent
        // battery is the stronger signal and settles it.
        assert_eq!(classify(255, BATTERY_FLAG_NO_BATTERY), PowerSource::Plugged);
        assert_eq!(classify(0, BATTERY_FLAG_NO_BATTERY), PowerSource::Plugged);
    }

    #[test]
    fn a_laptop_follows_the_ac_line() {
        // BatteryFlag 1 = high charge, 2 = low, 8 = charging.
        assert_eq!(classify(AC_LINE_ONLINE, 8), PowerSource::Plugged);
        assert_eq!(classify(AC_LINE_OFFLINE, 1), PowerSource::Battery);
    }

    #[test]
    fn an_unreadable_line_status_is_unknown_not_a_guess() {
        // 255 in both fields is "the OS won't say", not "no battery",
        // which is the specific value 128.
        assert_eq!(classify(255, 255), PowerSource::Unknown);
    }
}
