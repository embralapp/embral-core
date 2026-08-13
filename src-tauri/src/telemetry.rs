//! The telemetry facade ([telemetry.md]). Telemetry is a cloud-edition
//! feature: its machinery (queue, flusher, install id, endpoint) lives in
//! `cloud/telemetry.rs`, compiled only in cloud builds and kept out of the
//! open-core repo. The shared call sites go through this facade, which
//! compiles to a no-op in offline builds; the public repo shows exactly
//! which moments the cloud edition can count, and that its own build counts
//! nothing.
//!
//! The pure vocabulary helpers (app-name normalization, duration buckets)
//! stay here: they are harmless, and keeping them beside their tests keeps
//! the closed sets visible where the call sites use them.

/// Queue one event from the vocabulary. A no-op in offline builds and
/// whenever the user hasn't opted in.
#[allow(unused_variables)]
pub fn track(state: &crate::AppState, name: &str, props: serde_json::Value) {
    #[cfg(feature = "cloud")]
    crate::cloud::telemetry::track(state, name, props);
}

/// Ask the flusher to send now instead of at the next tick (events that
/// precede an imminent exit). A no-op in offline builds, where its only
/// caller (`cloud/commands.rs`) is not compiled either.
#[allow(unused_variables)]
#[cfg_attr(not(feature = "cloud"), allow(dead_code))]
pub fn flush_soon(state: &crate::AppState) {
    #[cfg(feature = "cloud")]
    crate::cloud::telemetry::flush_soon(state);
}

/// Normalize a detected meeting app's process name to the closed set the
/// vocabulary allows: the raw value is an exe name and must not leave the
/// machine ([telemetry.md]).
pub fn normalize_detected_app(process: &str) -> &'static str {
    let p = process.to_ascii_lowercase();
    for known in [
        // `chromium` is its own token, not merged into `chrome`: neither
        // string contains the other, so a Linux Chromium user would
        // otherwise report as "other" and be invisible.
        "zoom", "teams", "chrome", "chromium", "msedge", "firefox", "slack", "discord", "webex",
    ] {
        if p.contains(known) {
            return match known {
                "zoom" => "zoom",
                "teams" => "teams",
                "chrome" => "chrome",
                "chromium" => "chromium",
                "msedge" => "msedge",
                "firefox" => "firefox",
                "slack" => "slack",
                "discord" => "discord",
                _ => "webex",
            };
        }
    }
    "other"
}

/// Meeting length → the vocabulary's coarse bucket.
pub fn meeting_bucket(secs: u64) -> &'static str {
    match secs {
        s if s < 5 * 60 => "lt5m",
        s if s < 20 * 60 => "5to20m",
        s if s < 60 * 60 => "20to60m",
        _ => "gt60m",
    }
}

/// Dictation length → the vocabulary's coarse bucket.
pub fn dictation_bucket(secs: u64) -> &'static str {
    match secs {
        s if s < 10 => "lt10s",
        s if s <= 30 => "10to30s",
        _ => "gt30s",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detected_apps_normalize_to_the_closed_set() {
        assert_eq!(normalize_detected_app("Zoom.exe"), "zoom");
        assert_eq!(normalize_detected_app("ms-teams.exe"), "teams");
        assert_eq!(normalize_detected_app("msedge.exe"), "msedge");
        // Linux identities are bare process names.
        assert_eq!(normalize_detected_app("teams-for-linux"), "teams");
        assert_eq!(normalize_detected_app("msedge"), "msedge");
        // Chromium is its own bucket, and must not be merged into "chrome"
        // (nor vice versa): neither name contains the other.
        assert_eq!(normalize_detected_app("chromium"), "chromium");
        assert_eq!(normalize_detected_app("chromium-browser"), "chromium");
        assert_eq!(normalize_detected_app("chrome"), "chrome");
        assert_eq!(normalize_detected_app("google-chrome"), "chrome");
        // Anything unknown collapses to "other"; exe names never leave.
        assert_eq!(normalize_detected_app("obscure-voip-tool.exe"), "other");
    }

    #[test]
    fn buckets_are_coarse_and_total() {
        assert_eq!(meeting_bucket(0), "lt5m");
        assert_eq!(meeting_bucket(299), "lt5m");
        assert_eq!(meeting_bucket(300), "5to20m");
        assert_eq!(meeting_bucket(1200), "20to60m");
        assert_eq!(meeting_bucket(3600), "gt60m");
        assert_eq!(dictation_bucket(9), "lt10s");
        assert_eq!(dictation_bucket(30), "10to30s");
        assert_eq!(dictation_bucket(31), "gt30s");
    }
}
