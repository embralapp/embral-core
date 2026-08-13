//! Where this machine's power comes from
//! ([transcription.md](../../../../docs/transcription.md) §Provider selection).
//!
//! IOKit's power-sources API has two shapes: the full
//! `IOPSCopyPowerSourcesInfo` / `IOPSGetPowerSourceDescription` pair, which
//! hands back CoreFoundation dictionaries to walk, and this one call, which
//! answers the only question we ask. `IOPSGetTimeRemainingEstimate` returns
//! a plain `CFTimeInterval` with no object to release, and its "unlimited"
//! sentinel means "on wall power", including on a Mac with no battery,
//! which is the answer we want for a desktop.

use crate::platform::types::PowerSource;

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    /// Seconds of battery left, or one of the sentinels below
    /// (`IOKit/ps/IOPowerSources.h`, macOS 10.7+).
    fn IOPSGetTimeRemainingEstimate() -> f64;
}

/// `kIOPSTimeRemainingUnlimited`: on an unlimited power source, wall power,
/// or no battery in the machine at all.
const TIME_REMAINING_UNLIMITED: f64 = -2.0;

/// Read once per recording, so a cheap synchronous call is right.
pub fn power_source() -> PowerSource {
    // SAFETY: a documented, argument-free query returning a scalar.
    let estimate = unsafe { IOPSGetTimeRemainingEstimate() };
    if estimate == TIME_REMAINING_UNLIMITED {
        PowerSource::Plugged
    } else {
        // Everything else (a real estimate, or
        // `kIOPSTimeRemainingUnknown`, -1.0, "on battery but the estimate
        // isn't ready") means the battery is carrying the machine.
        PowerSource::Battery
    }
}
