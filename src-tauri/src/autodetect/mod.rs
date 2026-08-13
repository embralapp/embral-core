//! Meeting auto-detection: notices when a meeting app is using the
//! microphone and starts/stops recording per the configured policy.
//!
//! - `state`: the pure tick state machine + app matcher (unit-tested).
//! - `crate::platform::mic_users`: the per-OS scan for processes with an
//!   active mic session.
//! - this module: the poll loop tying config, state, and actions together.

pub mod silence;
mod state;

use std::sync::atomic::Ordering;
use std::time::Duration;

use embral_types::{AutoStartPolicy, AutoStopScope};
use tauri::{AppHandle, Emitter, Manager};

use crate::AppState;
use state::{match_app, Detection, Detector};

/// Whether a call ending should stop the current recording.
fn should_auto_stop(scope: AutoStopScope, was_auto_started: bool) -> bool {
    match scope {
        AutoStopScope::Never => false,
        AutoStopScope::AutoStarted => was_auto_started,
        AutoStopScope::All => true,
    }
}

/// Poll cadence. The detection delay is quantized to this.
const POLL_SECS: u64 = 3;

/// Empty polls tolerated before a call counts as over. Not a setting: its
/// only job is surviving a blip (one scan where the app's capture session
/// reads as inactive), so it stays short. Genuinely back-to-back calls are
/// separate meetings and must record as two.
const AUTO_STOP_GRACE_TICKS: u32 = 2;

fn ticks_for(seconds: u32) -> u32 {
    seconds.div_ceil(POLL_SECS as u32).max(1)
}

/// Spawn the detection loop (called once from `setup()`).
pub fn spawn(handle: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let own_pid = std::process::id();
        // Recreated whenever the timing config changes.
        let mut detector = Detector::new(1, 1);
        let mut current_windows = (0u32, 0u32);
        // Last tick's matched-candidate labels and detector phase, so the
        // log carries transitions (apps appearing/vanishing, phase walks)
        // rather than a line per tick: the trail that answers "did the
        // detector ever see this call end?".
        let mut last_labels: Vec<String> = Vec::new();
        let mut last_phase = detector.phase_name();

        loop {
            tokio::time::sleep(Duration::from_secs(POLL_SECS)).await;

            let state = handle.state::<AppState>();
            let config = state.config.lock().await.clone();

            if config.auto_start_policy == AutoStartPolicy::Manual {
                // Fully off: also forget any in-flight call state.
                if current_windows != (0, 0) {
                    detector = Detector::new(1, 1);
                    current_windows = (0, 0);
                }
                continue;
            }

            let windows = (
                ticks_for(config.detection_delay_secs),
                AUTO_STOP_GRACE_TICKS,
            );
            if windows != current_windows {
                detector = Detector::new(windows.0, windows.1);
                current_windows = windows;
            }

            // A manual recording in progress: don't track calls against it
            // unless the scope says every recording stops on call end; the
            // Start arm below still refuses to act while recording, so
            // ticking here only ever feeds auto-stop.
            let recording = state.recorder.lock().await.is_some();
            if recording
                && !state.auto_started.load(Ordering::Acquire)
                && config.auto_stop != AutoStopScope::All
            {
                continue;
            }

            let candidates = tokio::task::spawn_blocking(move || {
                crate::platform::processes_using_microphone(own_pid)
            })
            .await
            .unwrap_or_default();

            let candidate = match config.auto_start_policy {
                // Always: any mic user counts, allowlisted ones preferred as
                // the reported name.
                AutoStartPolicy::Always => candidates
                    .iter()
                    .find(|c| match_app(c, &config.auto_detect_apps))
                    .or_else(|| candidates.first())
                    .cloned(),
                AutoStartPolicy::Selective | AutoStartPolicy::Prompt => candidates
                    .iter()
                    .find(|c| match_app(c, &config.auto_detect_apps))
                    .cloned(),
                AutoStartPolicy::Manual => None,
            };

            let labels: Vec<String> = candidates.iter().map(|c| c.label().to_string()).collect();
            if labels != last_labels {
                tracing::info!(now = ?labels, before = ?last_labels, "mic sessions changed");
                last_labels = labels;
            }

            let transition = detector.tick(candidate.as_ref().map(|c| c.label()));
            if detector.phase_name() != last_phase {
                tracing::info!(
                    from = last_phase,
                    to = detector.phase_name(),
                    "detection phase changed"
                );
                last_phase = detector.phase_name();
            }
            match transition {
                Some(Detection::Start(app)) => {
                    if recording {
                        continue; // an auto recording is already running
                    }
                    match config.auto_start_policy {
                        AutoStartPolicy::Always | AutoStartPolicy::Selective => {
                            tracing::info!(app, "call detected — starting recording");
                            state.auto_started.store(true, Ordering::Release);
                            if let Err(e) =
                                crate::commands::start_recording(handle.clone(), handle.state())
                                    .await
                            {
                                // The user asked for this recording and is
                                // not looking at the app; a log line alone
                                // means a meeting silently goes unrecorded.
                                tracing::warn!("auto-start failed: {e}");
                                state.auto_started.store(false, Ordering::Release);
                                let _ = handle.emit("recording-start-failed", &e);
                            }
                        }
                        AutoStartPolicy::Prompt => {
                            if !state.detection_dismissed.load(Ordering::Acquire) {
                                tracing::info!(app, "call detected — prompting");
                                // Normalized: the raw exe name never leaves
                                // the machine ([telemetry.md]).
                                crate::telemetry::track(
                                    &state,
                                    "meeting_detected",
                                    serde_json::json!({
                                        "app": crate::telemetry::normalize_detected_app(&app)
                                    }),
                                );
                                let _ = handle
                                    .emit("meeting-detected", serde_json::json!({ "app": app }));
                            }
                        }
                        AutoStartPolicy::Manual => {}
                    }
                }
                Some(Detection::Stop) => {
                    // The call is over: reset the per-call prompt suppression
                    // and clear any lingering prompt in the UI.
                    state.detection_dismissed.store(false, Ordering::Release);
                    let _ = handle.emit("meeting-ended", ());
                    let was_auto_started = state.auto_started.swap(false, Ordering::AcqRel);
                    if should_auto_stop(config.auto_stop, was_auto_started)
                        && state.recorder.lock().await.is_some()
                    {
                        tracing::info!(
                            scope = ?config.auto_stop,
                            was_auto_started,
                            "call ended — stopping recording"
                        );
                        crate::commands::request_stop(&handle);
                    }
                }
                None => {}
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_decides_which_recordings_stop_on_call_end() {
        use AutoStopScope::*;
        // Never: nothing stops. AutoStarted: only detection-started
        // recordings. All: whatever is recording when the call ends.
        assert!(!should_auto_stop(Never, true));
        assert!(!should_auto_stop(Never, false));
        assert!(should_auto_stop(AutoStarted, true));
        assert!(!should_auto_stop(AutoStarted, false));
        assert!(should_auto_stop(All, true));
        assert!(should_auto_stop(All, false));
    }
}
