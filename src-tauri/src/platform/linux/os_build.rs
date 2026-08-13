//! The OS version string telemetry reports ([telemetry.md](../../../../docs/telemetry.md)).
//!
//! Telemetry has no separate platform field, so unlike the Windows twin
//! (a bare build number) and the macOS one (a bare `major.minor.patch`),
//! this string has to identify Linux and the distribution on its own:
//! "Debian GNU/Linux 13 (trixie) 6.12.48+deb13-amd64" rather than a version
//! that could be anything. Distro plus kernel is also the pair that
//! actually explains a Linux bug report.

/// `PRETTY_NAME` from `/etc/os-release` plus the kernel release, each
/// degrading to "unknown" independently so one missing piece never costs
/// the other.
#[cfg_attr(not(feature = "cloud"), allow(dead_code))]
pub fn os_build() -> String {
    format!("{} {}", pretty_name(), kernel_release())
}

/// The distribution's own display name. `/etc/os-release` is the
/// freedesktop standard and present on every distribution we target;
/// `/usr/lib/os-release` is the fallback the spec defines for systems where
/// `/etc` is empty.
fn pretty_name() -> String {
    for path in ["/etc/os-release", "/usr/lib/os-release"] {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        for line in text.lines() {
            if let Some(value) = line.strip_prefix("PRETTY_NAME=") {
                let value = value.trim().trim_matches('"');
                if !value.is_empty() {
                    return value.to_string();
                }
            }
        }
    }
    "Linux (unknown distribution)".to_string()
}

/// The kernel release (`uname -r`), read through the syscall rather than by
/// spawning a process.
fn kernel_release() -> String {
    // SAFETY: `uname` fills a caller-owned struct and returns 0 on success.
    let mut info: libc::utsname = unsafe { std::mem::zeroed() };
    if unsafe { libc::uname(&mut info) } != 0 {
        return "unknown-kernel".to_string();
    }
    let bytes: Vec<u8> = info
        .release
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).to_string()
}

#[cfg(test)]
mod tests {
    /// The string must name Linux on its own (telemetry has no platform
    /// field to lean on) and must carry both halves.
    #[test]
    fn identifies_the_distribution_and_the_kernel() {
        let build = super::os_build();
        assert!(!build.trim().is_empty());
        // Two halves, whitespace-separated, neither one empty.
        assert!(build.contains(' '), "expected 'distro kernel', got {build:?}");
        assert_ne!(super::kernel_release(), "unknown-kernel");
        assert!(!super::pretty_name().is_empty());
    }
}
