//! Where this machine's power comes from
//! ([transcription.md](../../../../docs/transcription.md) §Provider selection).
//!
//! Read straight from sysfs rather than through UPower's D-Bus service: the
//! question is asked once per recording, the files are three bytes each, and
//! a headless or trimmed system may have no UPower at all while
//! `/sys/class/power_supply` is always there.
//!
//! `type` distinguishes the supplies: `Mains` (an AC adapter, `online` is
//! 1 when the cable is live) from `Battery`. A machine with an online mains
//! supply (or with no battery at all, which is every desktop) reads as
//! `Plugged`, per the `PowerSource` contract's "is this thing at a desk".

use crate::platform::types::PowerSource;

const SUPPLIES: &str = "/sys/class/power_supply";

/// Read once per recording, so a handful of small file reads is right.
pub fn power_source() -> PowerSource {
    let Ok(entries) = std::fs::read_dir(SUPPLIES) else {
        // No sysfs class at all (a container, a very odd kernel): we cannot
        // answer, and the contract says never guess.
        return PowerSource::Unknown;
    };

    let mut saw_battery = false;
    for entry in entries.flatten() {
        let dir = entry.path();
        let kind = read_trimmed(&dir.join("type")).unwrap_or_default();
        match kind.as_str() {
            "Mains" | "USB" | "USB_PD" | "USB_PD_DRP" | "Wireless" => {
                // An online AC-ish supply settles it immediately.
                if read_trimmed(&dir.join("online")).as_deref() == Some("1") {
                    return PowerSource::Plugged;
                }
            }
            "Battery" => saw_battery = true,
            _ => {}
        }
    }

    if saw_battery {
        // A battery and no live adapter: running down the battery.
        PowerSource::Battery
    } else {
        // No battery in the machine: a desktop, the most desk-bound thing
        // there is.
        PowerSource::Plugged
    }
}

fn read_trimmed(path: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whatever this machine is, the answer must be decisive: `Unknown` is
    /// only for a platform that cannot answer, and Linux always can when
    /// the sysfs class exists.
    #[test]
    fn answers_decisively_on_a_real_machine() {
        let source = power_source();
        if std::path::Path::new(SUPPLIES).is_dir() {
            assert_ne!(
                source,
                PowerSource::Unknown,
                "sysfs power_supply exists, so the read must resolve"
            );
        }
    }
}
