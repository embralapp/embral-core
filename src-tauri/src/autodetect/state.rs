//! Pure detection logic: the tick-driven call state machine and the
//! process-name matcher. No OS calls, fully unit-tested; the WASAPI scan and
//! policy handling live in the sibling modules.

/// One observation per poll tick: is some candidate app using the mic?
/// Emitted transitions tell the poller when a call started or ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Detection {
    /// A call has been continuously present for the arming window.
    Start(String),
    /// The call has been gone for the grace window.
    Stop,
}

#[derive(Debug)]
enum Phase {
    Idle,
    /// Mic in use; waiting out the detection delay before acting.
    Arming { app: String, ticks: u32 },
    /// A call is considered live.
    Active,
    /// Call disappeared; waiting out the grace window before declaring it over.
    Grace { ticks: u32 },
}

pub struct Detector {
    phase: Phase,
    /// Consecutive candidate ticks required before `Start` (≥1).
    arm_ticks: u32,
    /// Consecutive empty ticks required before `Stop` (≥1).
    grace_ticks: u32,
}

impl Detector {
    pub fn new(arm_ticks: u32, grace_ticks: u32) -> Self {
        Detector {
            phase: Phase::Idle,
            arm_ticks: arm_ticks.max(1),
            grace_ticks: grace_ticks.max(1),
        }
    }

    /// Whether a call is currently considered live (Active or in Grace).
    /// Test-only today; production consumers read the emitted transitions.
    #[cfg(test)]
    pub fn call_live(&self) -> bool {
        matches!(self.phase, Phase::Active | Phase::Grace { .. })
    }

    /// The current phase, for the poller's transition log; a stuck Active
    /// state was invisible in the field before this existed.
    pub fn phase_name(&self) -> &'static str {
        match self.phase {
            Phase::Idle => "idle",
            Phase::Arming { .. } => "arming",
            Phase::Active => "active",
            Phase::Grace { .. } => "grace",
        }
    }

    /// Feed one tick's observation; returns a transition when one fires.
    pub fn tick(&mut self, candidate: Option<&str>) -> Option<Detection> {
        match (&mut self.phase, candidate) {
            (Phase::Idle, Some(app)) => {
                if self.arm_ticks <= 1 {
                    self.phase = Phase::Active;
                    Some(Detection::Start(app.to_string()))
                } else {
                    self.phase = Phase::Arming {
                        app: app.to_string(),
                        ticks: 1,
                    };
                    None
                }
            }
            (Phase::Idle, None) => None,
            (Phase::Arming { app, ticks }, Some(current)) => {
                // Track the most recent candidate name while arming.
                *app = current.to_string();
                *ticks += 1;
                if *ticks >= self.arm_ticks {
                    let app = app.clone();
                    self.phase = Phase::Active;
                    Some(Detection::Start(app))
                } else {
                    None
                }
            }
            (Phase::Arming { .. }, None) => {
                self.phase = Phase::Idle;
                None
            }
            (Phase::Active, Some(_)) => None,
            (Phase::Active, None) => {
                if self.grace_ticks <= 1 {
                    self.phase = Phase::Idle;
                    Some(Detection::Stop)
                } else {
                    self.phase = Phase::Grace { ticks: 1 };
                    None
                }
            }
            // Call resumed inside the grace window: still the same call.
            (Phase::Grace { .. }, Some(_)) => {
                self.phase = Phase::Active;
                None
            }
            (Phase::Grace { ticks }, None) => {
                *ticks += 1;
                if *ticks >= self.grace_ticks {
                    self.phase = Phase::Idle;
                    Some(Detection::Stop)
                } else {
                    None
                }
            }
        }
    }
}

/// Whether a process name counts as a meeting app. Case-insensitive, `.exe`
/// stripped, substring match in either direction so "ms-teams" matches
/// "ms-teams.exe" and "zoom" matches "Zoom.exe".
pub fn match_app(app: &crate::platform::types::AppId, allowlist: &[String]) -> bool {
    app.identities()
        .any(|identity| match_identity(identity, allowlist))
}

/// One identity string (exe name, bundle id, display name) against the
/// allowlist: case-insensitive, `.exe`-stripped, bidirectional substring,
/// which is what lets a bare entry like "chrome" catch `chrome.exe`,
/// `com.google.Chrome.helper`, and "Google Chrome" alike.
fn match_identity(identity: &str, allowlist: &[String]) -> bool {
    let name = identity
        .to_lowercase()
        .trim_end_matches(".exe")
        .to_string();
    if name.is_empty() {
        return false;
    }
    allowlist.iter().any(|entry| {
        let entry = entry.to_lowercase().trim_end_matches(".exe").to_string();
        !entry.is_empty() && (name.contains(&entry) || entry.contains(&name))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn exe(name: &str) -> crate::platform::types::AppId {
        crate::platform::types::AppId::from_exe(1, name.to_string())
    }

    #[test]
    fn matcher_is_case_insensitive_and_exe_agnostic() {
        let allow = list(&["zoom", "ms-teams", "chrome"]);
        assert!(match_app(&exe("Zoom.exe"), &allow));
        assert!(match_app(&exe("ms-teams.exe"), &allow));
        assert!(match_app(&exe("chrome"), &allow));
        assert!(match_app(&exe("GoogleChrome.exe"), &allow)); // substring
        assert!(!match_app(&exe("notepad.exe"), &allow));
        assert!(!match_app(&exe(""), &allow));
    }

    /// Linux identities, taken verbatim from a live Zoom-in-Chrome call and
    /// the real process names of the Linux clients. The vocabulary is bare
    /// process names (no `.exe`, no bundle ids), so these are the fixtures
    /// that prove the platform-keyed default list actually matches what
    /// `mic_users.rs` reports there ([260801-linux-port.md] Phase 2).
    ///
    /// [260801-linux-port.md]: ../../../docs/plans/260801-linux-port.md
    #[test]
    fn matcher_handles_linux_identities() {
        // The Linux default list, verbatim from embral-types.
        let allow = list(&[
            "zoom", "teams", "chrome", "chromium", "msedge", "firefox", "slack", "discord",
            "webex",
        ]);

        // A real browser call, measured: pulse reports the binary and an
        // app name with " input" appended for a record stream. Either
        // identity alone must be enough.
        let browser_call = crate::platform::types::AppId {
            pid: 4564, // Chrome's audio.mojom.AudioService child, not the browser
            exe: Some("chrome".into()),
            bundle_id: None,
            display_name: Some("Google Chrome input".into()),
        };
        assert!(match_app(&browser_call, &allow), "a Zoom call in Chrome is detected");
        // And with only the suffixed display name to go on.
        let name_only = crate::platform::types::AppId {
            pid: 4564,
            exe: None,
            bundle_id: None,
            display_name: Some("Google Chrome input".into()),
        };
        assert!(match_app(&name_only, &allow), "the suffixed name still matches");

        // The native Linux clients, by their actual binary names.
        for binary in [
            "zoom",             // zoom desktop
            "teams-for-linux",  // the Teams client's real binary
            "slack",
            "discord",
            "firefox",
            "msedge",           // Edge is msedge on Linux, as on Windows
            "chromium",
            "chromium-browser", // some distros' package name
        ] {
            assert!(match_app(&exe(binary), &allow), "{binary} must be detected");
        }

        // Things that must not trip detection.
        for binary in ["gnome-terminal", "code", "spotify", "pipewire", "pulseaudio"] {
            assert!(!match_app(&exe(binary), &allow), "{binary} is not a meeting");
        }

        // Chrome and Chromium stay distinct, the pair substring matching
        // cannot collapse, which is why both need their own entry. With only
        // `chrome` allowed, a Chromium call goes undetected.
        let chrome_only = list(&["chrome"]);
        assert!(match_app(&exe("google-chrome"), &chrome_only));
        assert!(
            !match_app(&exe("chromium"), &chrome_only),
            "this is exactly why the Linux list carries chromium separately"
        );
    }

    #[test]
    fn matcher_accepts_any_identity_the_platform_has() {
        let allow = list(&["zoom", "chrome", "slack"]);
        // A macOS-shaped observation: bundle id + display name, no exe.
        let helper = crate::platform::types::AppId {
            pid: 7,
            exe: None,
            bundle_id: Some("com.google.Chrome.helper".into()),
            display_name: Some("Google Chrome".into()),
        };
        assert!(match_app(&helper, &allow), "helper bundle ids match brand tokens");
        let zoom = crate::platform::types::AppId {
            pid: 8,
            exe: Some("zoom.us".into()),
            bundle_id: Some("us.zoom.xos".into()),
            display_name: None,
        };
        assert!(match_app(&zoom, &allow));
        let other = crate::platform::types::AppId {
            pid: 9,
            exe: None,
            bundle_id: Some("com.apple.notes".into()),
            display_name: Some("Notes".into()),
        };
        assert!(!match_app(&other, &allow));
    }

    #[test]
    fn start_fires_after_arming_window() {
        let mut d = Detector::new(3, 2);
        assert_eq!(d.tick(Some("zoom")), None);
        assert_eq!(d.tick(Some("zoom")), None);
        assert_eq!(d.tick(Some("zoom")), Some(Detection::Start("zoom".into())));
        assert!(d.call_live());
        // Steady state produces nothing.
        assert_eq!(d.tick(Some("zoom")), None);
    }

    #[test]
    fn arming_resets_when_candidate_disappears() {
        let mut d = Detector::new(3, 2);
        d.tick(Some("zoom"));
        d.tick(Some("zoom"));
        assert_eq!(d.tick(None), None); // back to idle
        d.tick(Some("zoom"));
        d.tick(Some("zoom"));
        assert_eq!(d.tick(Some("zoom")), Some(Detection::Start("zoom".into())));
    }

    #[test]
    fn stop_fires_after_grace_and_rejoin_cancels_it() {
        let mut d = Detector::new(1, 3);
        assert_eq!(d.tick(Some("zoom")), Some(Detection::Start("zoom".into())));
        // Call drops…
        assert_eq!(d.tick(None), None);
        assert_eq!(d.tick(None), None);
        // …but rejoins inside the grace window: same call, no events.
        assert_eq!(d.tick(Some("zoom")), None);
        assert!(d.call_live());
        // Drops again and stays gone.
        assert_eq!(d.tick(None), None);
        assert_eq!(d.tick(None), None);
        assert_eq!(d.tick(None), Some(Detection::Stop));
        assert!(!d.call_live());
    }

    #[test]
    fn single_tick_windows_fire_immediately() {
        let mut d = Detector::new(1, 1);
        assert_eq!(d.tick(Some("meet")), Some(Detection::Start("meet".into())));
        assert_eq!(d.tick(None), Some(Detection::Stop));
    }

    #[test]
    fn arming_tracks_latest_candidate_name() {
        let mut d = Detector::new(2, 1);
        d.tick(Some("chrome"));
        assert_eq!(d.tick(Some("zoom")), Some(Detection::Start("zoom".into())));
    }
}
