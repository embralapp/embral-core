//! The OS version string telemetry reports ([telemetry.md](../../../../docs/telemetry.md)).

/// The macOS version as `major.minor.patch` (e.g. "15.5.0"); the Windows
/// twin reports the build number. Both are opaque strings server-side.
#[cfg_attr(not(feature = "cloud"), allow(dead_code))]
pub fn os_build() -> String {
    let v = objc2_foundation::NSProcessInfo::processInfo().operatingSystemVersion();
    format!("{}.{}.{}", v.majorVersion, v.minorVersion, v.patchVersion)
}
