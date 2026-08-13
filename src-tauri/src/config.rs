use anyhow::Result;
use embral_types::{AppConfig, TranscriptionProvider};
use std::path::PathBuf;

use crate::platform::types::PowerSource;

pub fn config_file_path() -> PathBuf {
    // EMBRAL_DATA_DIR redirects the whole {home}/embral root — a development
    // affordance for scratch libraries (configuration.md).
    if let Ok(dir) = std::env::var("EMBRAL_DATA_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir).join("config.json");
        }
    }
    dirs::home_dir()
        .expect("cannot find home dir")
        .join("embral")
        .join("config.json")
}

/// What reading a config file found. `Corrupt` means the file exists but no
/// longer parses — the caller gets defaults, but the file was copied aside
/// first: silently defaulting over a real config would permanently destroy
/// it (session token included) on the next save.
enum LoadedConfig {
    Loaded(AppConfig),
    Missing,
    Corrupt,
}

fn load_from(path: &std::path::Path) -> Result<LoadedConfig> {
    if !path.exists() {
        return Ok(LoadedConfig::Missing);
    }
    let text = std::fs::read_to_string(path)?;
    match serde_json::from_str::<AppConfig>(&text) {
        Ok(config) => Ok(LoadedConfig::Loaded(config)),
        Err(e) => {
            let backup = path.with_extension("json.corrupt");
            match std::fs::copy(path, &backup) {
                Ok(_) => tracing::error!(
                    "config.json does not parse ({e}); using defaults — the file was copied to {}",
                    backup.display()
                ),
                Err(copy_err) => tracing::error!(
                    "config.json does not parse ({e}) and could not be backed up ({copy_err}); using defaults"
                ),
            }
            Ok(LoadedConfig::Corrupt)
        }
    }
}

pub fn load_config() -> Result<AppConfig> {
    let mut config = match load_from(&config_file_path())? {
        LoadedConfig::Loaded(config) => config,
        LoadedConfig::Missing | LoadedConfig::Corrupt => return Ok(AppConfig::default()),
    };
    // Older configs stored the `~` shorthand; the UI shows the OS-native
    // absolute path, so normalize once on load (resolution is unchanged).
    if config.storage_dir.starts_with('~') {
        config.storage_dir = embral_types::resolve_storage_path(&config.storage_dir)
            .to_string_lossy()
            .to_string();
    }
    // The cloud URL used to be materialized into config.json; a stored value
    // equal to the production default is that old default, not a
    // customization — clear it so `cloud_url()` can pick per build (dev
    // builds talk to the local server).
    #[cfg(feature = "cloud")]
    if config.cloud_api_url == embral_types::DEFAULT_CLOUD_URL {
        config.cloud_api_url = String::new();
    }
    Ok(config)
}

pub fn save_config(config: &AppConfig) -> Result<()> {
    let path = config_file_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(config)?)?;
    Ok(())
}

/// Whether the selected local model's files are actually on disk. A deleted
/// managed model degrades to a clean "not configured" gate rather than a
/// cryptic load failure when recording starts.
fn local_model_ready(config: &AppConfig) -> bool {
    embral_engine::catalog::find(&config.meeting_asr_model()).is_some_and(|m| m.present())
}

/// Who transcribes the recording about to start. `transcription_provider` is
/// the standing choice — the account plumbing owns it (`adopt_cloud_provider`
/// at sign-in, `revert_to_device` at sign-out) — and the power policy is a
/// lens over it, applied per recording and never written back.
///
/// Read once, at `start_recording`: a meeting does not change hands when
/// someone plugs in halfway through.
pub fn provider_for_power(config: &AppConfig, power: PowerSource) -> TranscriptionProvider {
    #[cfg(feature = "cloud")]
    {
        power_lens(
            config.transcription_provider.clone(),
            config.transcription_power_policy,
            power,
        )
    }
    #[cfg(not(feature = "cloud"))]
    {
        // Nothing to route to: the offline build has one provider.
        let _ = power;
        config.transcription_provider.clone()
    }
}

/// The rule itself. Plugged in means a desk, which means CPU headroom, so
/// the device transcribes; on battery the cloud spends someone else's
/// cycles. `Unknown` — a platform that cannot answer — leaves the standing
/// choice alone in both directions: a guess must never be the thing that
/// sends a meeting's audio off the machine.
#[cfg(feature = "cloud")]
fn power_lens(
    configured: TranscriptionProvider,
    policy: embral_types::PowerPolicy,
    power: PowerSource,
) -> TranscriptionProvider {
    match (policy, power) {
        (embral_types::PowerPolicy::CloudOnBattery, PowerSource::Battery) => {
            TranscriptionProvider::Cloud
        }
        (embral_types::PowerPolicy::CloudOnBattery, PowerSource::Plugged) => {
            TranscriptionProvider::Local
        }
        _ => configured,
    }
}

/// Why this config cannot start a recording, when it cannot — `None` means
/// it can. The gate's reasoning in words rather than a bare bool: a refused
/// auto-start happens while the user is in a call looking at something
/// else, and "Transcription isn't set up yet" in the log names none of the
/// three things that might be missing.
///
/// Cloud asks two questions, not one. *Something* must be able to
/// transcribe: a signed-in device, or the local model the fallback lands on
/// — a signed-out user whose fallback is "switch to this device" is
/// configured, and refusing to record was the bug (the recording would have
/// fallen back anyway). And whatever the account state, the chosen failure
/// path must exist: "switch to this device" needs the model; "disable
/// transcription" needs nothing, because a recording with no transcript is
/// exactly what it asks for. Hours running out degrades at the relay, not
/// here.
///
/// The rules themselves live in `local_gap` / `cloud_gap` — pure, and
/// tested — because `local_model_ready` reads the real catalog, which a
/// unit test can't stage.
///
/// `provider` is the one this recording will actually use — the standing
/// choice as bent by [`provider_for_power`] — not the config field, so the
/// gate asks about the lane the meeting is really headed down.
pub fn missing_prerequisite(config: &AppConfig, provider: &TranscriptionProvider) -> Option<String> {
    let model = config.meeting_asr_model();
    let local_ready = local_model_ready(config);
    match provider {
        embral_types::TranscriptionProvider::Local => local_gap(local_ready, &model),
        #[cfg(feature = "cloud")]
        embral_types::TranscriptionProvider::Cloud => cloud_gap(
            !config.cloud_session_token.is_empty(),
            local_ready,
            config.cloud_out_of_hours,
            &model,
        ),
    }
}

fn local_gap(local_ready: bool, model: &str) -> Option<String> {
    (!local_ready)
        .then(|| format!("provider is this device, but the model '{model}' is not on disk"))
}

#[cfg(feature = "cloud")]
fn cloud_gap(
    signed_in: bool,
    local_ready: bool,
    out_of_hours: embral_types::CloudOutOfHours,
    model: &str,
) -> Option<String> {
    let fallback_disables = out_of_hours == embral_types::CloudOutOfHours::Disabled;
    if !signed_in && !local_ready {
        Some(format!(
            "provider is cloud but no account is signed in, and the fallback \
             model '{model}' is not on disk"
        ))
    } else if !fallback_disables && !local_ready {
        Some(format!(
            "provider is cloud with 'switch to this device' as the fallback, \
             but the model '{model}' is not on disk"
        ))
    } else {
        None
    }
}

/// What a failing cloud session does — at start (connect refused: out of
/// hours, unreachable) and mid-recording (the relay's 402 cut, a drop).
/// Pure and tested; the recording itself never stops for any of these.
#[cfg(feature = "cloud")]
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum CloudFailureAction {
    /// Swap in a local session (the "switch to this device" setting).
    SwitchToLocal,
    /// Keep recording with no transcription (the "disable transcription"
    /// setting — honored for every failure shape, not only hours: the user
    /// said this app should not transcribe on the device).
    DisableTranscription,
    /// Nothing to switch to: surface the failure.
    Fail,
}

#[cfg(feature = "cloud")]
pub fn on_cloud_failure(
    out_of_hours: embral_types::CloudOutOfHours,
    local_model_present: bool,
) -> CloudFailureAction {
    match out_of_hours {
        embral_types::CloudOutOfHours::Disabled => CloudFailureAction::DisableTranscription,
        embral_types::CloudOutOfHours::Local if local_model_present => {
            CloudFailureAction::SwitchToLocal
        }
        embral_types::CloudOutOfHours::Local => CloudFailureAction::Fail,
    }
}

#[cfg(all(test, feature = "cloud"))]
mod configured_tests {
    use embral_types::{AppConfig, CloudOutOfHours, TranscriptionProvider};

    /// The shipped rule, with the model question answered by the caller —
    /// `missing_prerequisite` reads the real catalog, which a unit test
    /// can't stage. `true` = this config can record.
    fn cloud_ok(signed_in: bool, local_ready: bool, out_of_hours: CloudOutOfHours) -> bool {
        super::cloud_gap(signed_in, local_ready, out_of_hours, "parakeet-tdt-en").is_none()
    }

    #[test]
    fn signed_out_with_a_local_model_can_still_record() {
        // The field bug: cloud selected, signed out, fallback "switch to
        // this device", model present — auto-start refused the meeting
        // even though the recording would have fallen back anyway.
        assert!(cloud_ok(false, true, CloudOutOfHours::Local));
    }

    #[test]
    fn nothing_to_transcribe_with_is_not_configured() {
        // Signed out, no model: neither lane can produce a transcript.
        assert!(!cloud_ok(false, false, CloudOutOfHours::Local));
        assert!(!cloud_ok(false, false, CloudOutOfHours::Disabled));
    }

    #[test]
    fn a_signed_in_account_needs_the_model_only_for_the_device_fallback() {
        assert!(cloud_ok(true, false, CloudOutOfHours::Disabled));
        assert!(!cloud_ok(true, false, CloudOutOfHours::Local));
        assert!(cloud_ok(true, true, CloudOutOfHours::Local));
    }

    #[test]
    fn the_local_provider_gate_is_purely_a_model_question() {
        assert!(super::local_gap(true, "parakeet-tdt-en").is_none());
        assert!(super::local_gap(false, "parakeet-tdt-en").is_some());
    }

    #[test]
    fn every_refusal_names_the_model_it_wanted() {
        // The field's one refusal logged "Transcription isn't set up yet",
        // which does not say whether the model, the account, or the
        // fallback was the problem — the whole point of the reason string.
        // The real case: language switched to multilingual, whose model
        // was never downloaded, so the *effective* model is not the one
        // named in Settings.
        let refusals = [
            super::cloud_gap(false, false, CloudOutOfHours::Local, "parakeet-tdt-v3"),
            super::cloud_gap(true, false, CloudOutOfHours::Local, "parakeet-tdt-v3"),
            super::local_gap(false, "parakeet-tdt-v3"),
        ];
        for refusal in refusals {
            let reason = refusal.expect("this config cannot record");
            assert!(reason.contains("parakeet-tdt-v3"), "{reason}");
        }
    }

    #[test]
    fn a_real_config_reaches_the_pure_rule() {
        // `missing_prerequisite` is only a catalog lookup away from the
        // rule above; this pins the wiring (the model presence itself
        // depends on the test machine, so the answer isn't asserted).
        let mut config = AppConfig::default();
        config.transcription_provider = TranscriptionProvider::Cloud;
        config.cloud_session_token = String::new();
        config.cloud_out_of_hours = CloudOutOfHours::Disabled;
        let _ = super::missing_prerequisite(&config, &TranscriptionProvider::Cloud);
    }

    #[test]
    fn the_gate_asks_about_the_lane_the_power_policy_chose() {
        // Cloud configured, plugged in, policy on → the recording runs on
        // this device, so the gate must be the *local* question. Asking the
        // cloud question here would let a meeting through with no model on
        // disk and nothing to transcribe it with.
        let mut config = AppConfig::default();
        config.transcription_provider = TranscriptionProvider::Cloud;
        config.transcription_power_policy = embral_types::PowerPolicy::CloudOnBattery;
        let provider = super::provider_for_power(&config, super::PowerSource::Plugged);
        assert_eq!(provider, TranscriptionProvider::Local);
        let _ = super::missing_prerequisite(&config, &provider);
    }
}

#[cfg(all(test, feature = "cloud"))]
mod power_tests {
    use super::*;
    use embral_types::PowerPolicy;

    #[test]
    fn the_policy_off_leaves_the_standing_choice_alone() {
        for power in [PowerSource::Plugged, PowerSource::Battery, PowerSource::Unknown] {
            assert_eq!(
                power_lens(TranscriptionProvider::Cloud, PowerPolicy::Off, power),
                TranscriptionProvider::Cloud
            );
            assert_eq!(
                power_lens(TranscriptionProvider::Local, PowerPolicy::Off, power),
                TranscriptionProvider::Local
            );
        }
    }

    #[test]
    fn on_battery_goes_to_the_cloud_and_plugged_in_comes_home() {
        // Both directions, from either standing choice — the setting is a
        // policy about power, not a one-way nudge away from the device.
        for configured in [TranscriptionProvider::Local, TranscriptionProvider::Cloud] {
            assert_eq!(
                power_lens(
                    configured.clone(),
                    PowerPolicy::CloudOnBattery,
                    PowerSource::Battery
                ),
                TranscriptionProvider::Cloud
            );
            assert_eq!(
                power_lens(
                    configured.clone(),
                    PowerPolicy::CloudOnBattery,
                    PowerSource::Plugged
                ),
                TranscriptionProvider::Local
            );
        }
    }

    #[test]
    fn an_unknown_power_source_never_routes_audio_off_the_machine() {
        // The stub platform, or a failed OS call. Guessing "battery" would
        // silently start uploading a meeting the user expected to stay put.
        assert_eq!(
            power_lens(
                TranscriptionProvider::Local,
                PowerPolicy::CloudOnBattery,
                PowerSource::Unknown
            ),
            TranscriptionProvider::Local
        );
        assert_eq!(
            power_lens(
                TranscriptionProvider::Cloud,
                PowerPolicy::CloudOnBattery,
                PowerSource::Unknown
            ),
            TranscriptionProvider::Cloud
        );
    }

    #[test]
    fn a_real_config_reaches_the_rule() {
        let mut config = AppConfig::default();
        config.transcription_provider = TranscriptionProvider::Local;
        config.transcription_power_policy = PowerPolicy::CloudOnBattery;
        assert_eq!(
            provider_for_power(&config, PowerSource::Battery),
            TranscriptionProvider::Cloud
        );
    }
}

#[cfg(all(test, feature = "cloud"))]
mod tests {
    use super::*;
    use embral_types::CloudOutOfHours;

    #[test]
    fn disable_wins_regardless_of_the_local_model() {
        // The user said "don't transcribe on this device" — a downloaded
        // model doesn't override that, and a missing one doesn't error.
        assert_eq!(
            on_cloud_failure(CloudOutOfHours::Disabled, true),
            CloudFailureAction::DisableTranscription
        );
        assert_eq!(
            on_cloud_failure(CloudOutOfHours::Disabled, false),
            CloudFailureAction::DisableTranscription
        );
    }

    #[test]
    fn switch_to_device_needs_the_model() {
        assert_eq!(
            on_cloud_failure(CloudOutOfHours::Local, true),
            CloudFailureAction::SwitchToLocal
        );
        assert_eq!(
            on_cloud_failure(CloudOutOfHours::Local, false),
            CloudFailureAction::Fail
        );
    }
}

#[cfg(test)]
mod load_tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("embral-config-test-{}-{name}", std::process::id()))
    }

    #[test]
    fn a_corrupt_config_is_backed_up_not_destroyed() {
        let path = temp_path("corrupt.json");
        std::fs::write(&path, "{ not json").unwrap();

        assert!(matches!(load_from(&path).unwrap(), LoadedConfig::Corrupt));
        // The original survives untouched and the evidence copy exists — a
        // hand-recoverable session token beats a silent factory reset.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
        let backup = path.with_extension("json.corrupt");
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), "{ not json");

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&backup);
    }

    #[test]
    fn missing_and_valid_files_load_normally() {
        let missing = temp_path("missing.json");
        assert!(matches!(load_from(&missing).unwrap(), LoadedConfig::Missing));

        let valid = temp_path("valid.json");
        std::fs::write(
            &valid,
            serde_json::to_string(&embral_types::AppConfig::default()).unwrap(),
        )
        .unwrap();
        assert!(matches!(load_from(&valid).unwrap(), LoadedConfig::Loaded(_)));
        let _ = std::fs::remove_file(&valid);
    }
}
