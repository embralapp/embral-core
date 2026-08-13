use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

mod error;
pub use error::AppError;

// --- Core types ---

/// A single meeting in the index. The summary and transcript documents are
/// database columns, not files, so the only path here is the audio's.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingRecord {
    pub id: String,
    pub title: String,
    pub date: DateTime<Utc>,
    pub duration_seconds: u64,
    pub chunks: u32,
    pub audio_path: String,
}

/// Summary returned by list commands (subset of MeetingRecord).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingSummary {
    pub id: String,
    pub title: String,
    pub date: DateTime<Utc>,
    pub duration_seconds: u64,
}

impl From<&MeetingRecord> for MeetingSummary {
    fn from(r: &MeetingRecord) -> Self {
        MeetingSummary {
            id: r.id.clone(),
            title: r.title.clone(),
            date: r.date,
            duration_seconds: r.duration_seconds,
        }
    }
}

/// A session-generated numbered speaker label ("Speaker 3") — a per-meeting
/// placeholder, not a person's name. The naming pass may only rename these,
/// and adopt-by-name must never treat one as an identity: two meetings'
/// "Speaker 2" are different people.
pub fn is_generic_speaker_label(label: &str) -> bool {
    generic_speaker_number(label).is_some()
}

/// The number in a session-generated "Speaker N" label, `None` for anything
/// else (a person's name, or a shape no session produces). A stream opened
/// mid-recording numbers its speakers after the highest already seen, which
/// is what needs the number itself rather than the yes/no above.
pub fn generic_speaker_number(label: &str) -> Option<usize> {
    let n = label.strip_prefix("Speaker ")?;
    if n.is_empty() || !n.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    n.parse().ok()
}

#[cfg(test)]
mod speaker_label_tests {
    use super::*;

    #[test]
    fn generic_labels_carry_their_number() {
        assert_eq!(generic_speaker_number("Speaker 1"), Some(1));
        assert_eq!(generic_speaker_number("Speaker 12"), Some(12));
        assert!(is_generic_speaker_label("Speaker 3"));
    }

    #[test]
    fn names_and_malformed_labels_do_not() {
        for label in ["Alice", "Speaker", "Speaker ", "Speaker one", "speaker 2", "Speaker 2b"] {
            assert_eq!(generic_speaker_number(label), None, "{label}");
            assert!(!is_generic_speaker_label(label), "{label}");
        }
    }
}

/// A single transcription segment from a WebSocket streaming session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionSegment {
    pub speaker: Option<String>, // None when provider doesn't support diarization
    /// Registry link (`speakers.id`) when this segment's speaker has been
    /// matched or confirmed as a known person; `None` for unmatched labels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker_id: Option<String>,
    pub text: String,
    pub start: f64,
    pub end: f64,
}

/// Full transcript accumulated from a WebSocket session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiarizedTranscript {
    pub segments: Vec<TranscriptionSegment>,
}

/// Which transcription backend to use. The offline core has exactly one;
/// the cloud edition adds metered transcription through embral's backend.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptionProvider {
    /// On-device transcription via the sherpa-onnx engine.
    #[default]
    #[serde(alias = "parakeet")]
    Local,
    /// Metered cloud transcription through embral's own backend.
    #[cfg(feature = "cloud")]
    #[serde(alias = "soniox")]
    Cloud,
}

/// The language transcription runs in. Owned at the transcription level and
/// read by both providers: the cloud relay turns `English` into a Soniox
/// language hint (and `Multilingual` into none, so the vendor auto-detects),
/// while the local engine picks the model family.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptionLanguage {
    #[default]
    English,
    /// Any supported language, detected as it is spoken.
    Multilingual,
}

/// What a cloud recording does when the account's hours — subscription plus
/// purchased — run out. A *connection* drop always falls back to the device;
/// this is only about hours.
#[cfg(feature = "cloud")]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CloudOutOfHours {
    /// Finish the meeting on this device.
    #[default]
    Local,
    /// Stop transcribing. The recording and the user's notes continue; no
    /// transcript is written past the cutoff.
    Disabled,
}

/// Whether the machine's power source gets a say in who transcribes a
/// meeting. Plugged in means a desk, which means CPU headroom; on battery
/// the cloud spends someone else's cycles. A separate field rather than a
/// third `TranscriptionProvider`, because the provider is a standing choice
/// the account plumbing writes (adopting cloud at sign-in, reverting at
/// sign-out) and this is a lens over it.
#[cfg(feature = "cloud")]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PowerPolicy {
    /// The power source is nobody's business; `transcription_provider` runs.
    #[default]
    Off,
    /// Cloud while on battery, this device while plugged in.
    CloudOnBattery,
}

/// Which transport an LLM profile speaks. `Builtin` is the bundled
/// llama-server sidecar (OpenAI protocol on a loopback port, resolved at
/// run time); `Custom` is any OpenAI-compatible base URL — not user-facing,
/// kept as the generic transport the cloud relay will ride (R7).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LlmProvider {
    #[default]
    Builtin,
    Custom,
}

impl LlmProvider {
    /// Whether prompts stay on this machine.
    pub fn is_local(self) -> bool {
        matches!(self, LlmProvider::Builtin)
    }
}

/// One synthesis engine. These are fixed per edition (no user-defined
/// registry): the offline core has exactly the built-in engine; the cloud
/// edition adds embral cloud with R7. Summaries and dictation each select
/// an engine by id (`""` = none).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LlmProfile {
    pub id: String,
    pub name: String,
    pub provider: LlmProvider,
    /// Empty → the provider's default model.
    #[serde(default)]
    pub model: String,
    /// Base URL; empty → resolved at run time (Builtin = the sidecar port).
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub api_key: String,
}

/// Id of the always-present built-in (on-device) engine.
pub const BUILTIN_PROFILE_ID: &str = "builtin";

/// Id of the cloud synthesis engine (cloud builds only).
#[cfg(feature = "cloud")]
pub const CLOUD_PROFILE_ID: &str = "cloud";

impl LlmProfile {
    pub fn builtin() -> LlmProfile {
        LlmProfile {
            id: BUILTIN_PROFILE_ID.to_string(),
            name: "Built-in (on-device)".to_string(),
            provider: LlmProvider::Builtin,
            model: String::new(),
            endpoint: String::new(),
            api_key: String::new(),
        }
    }

    /// The cloud engine rides the generic `Custom` (OpenAI-style)
    /// transport; endpoint and key resolve at run time from the signed-in
    /// device's config, and the relay pins the actual model server-side.
    #[cfg(feature = "cloud")]
    pub fn cloud() -> LlmProfile {
        LlmProfile {
            id: CLOUD_PROFILE_ID.to_string(),
            name: "embral cloud".to_string(),
            provider: LlmProvider::Custom,
            model: String::new(),
            endpoint: String::new(),
            api_key: String::new(),
        }
    }
}

/// The synthesis engines available to this build.
#[cfg(not(feature = "cloud"))]
pub fn available_profiles() -> Vec<LlmProfile> {
    vec![LlmProfile::builtin()]
}

/// The synthesis engines available to this build.
#[cfg(feature = "cloud")]
pub fn available_profiles() -> Vec<LlmProfile> {
    vec![LlmProfile::builtin(), LlmProfile::cloud()]
}

/// How dictations are cleaned up after transcription.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum DictationCleanup {
    /// The cleanup relay (a server-pinned model chain). A signed-in
    /// choice: signing in adopts it, signing out reverts to on-device
    /// ([cloud-seam.md]); the resolve-at-use degrade to the on-device
    /// model survives only as a safety net for stale configs.
    #[cfg(feature = "cloud")]
    Cloud,
    /// The built-in on-device model. The default in every build — cloud
    /// engines are adopted at sign-in, never presumed.
    #[default]
    OnDevice,
    /// Raw transcription, verbatim.
    Off,
}

/// Which tab a meeting opens on. Named for the three documents a meeting
/// carries: the AI summary, the user's own notes, the transcript. A meeting
/// with no summary opens on notes whatever this says (the tab isn't there).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OpenMeetingTab {
    #[default]
    Summary,
    Notes,
    Transcript,
}

/// What an unanswered silence check-in does after its grace window.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SilenceUnanswered {
    /// Stop and finalize the recording (the forgotten-recording guard).
    #[default]
    Stop,
    /// Keep recording; the check-in stands down until speech resumes.
    Keep,
}

/// Which recordings stop on their own when the detected call ends.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutoStopScope {
    /// Recordings never stop on call end.
    Never,
    /// Only recordings detection started (or the user accepted from the
    /// detection prompt).
    #[default]
    AutoStarted,
    /// Every recording — a call ending stops whatever is being recorded.
    All,
}

/// What embral does when it detects a meeting app using the microphone.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutoStartPolicy {
    /// Record any app with an active microphone stream.
    Always,
    /// Record only apps on the allowlist.
    Selective,
    /// Notify and ask before recording (allowlisted apps).
    #[default]
    Prompt,
    /// Never detect; record only when asked.
    Manual,
}

/// UI color scheme preference.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

/// How exported markdown carries its metadata (title/date/participants).
/// `Frontmatter` suits Obsidian and folder-based tools; `Inline` renders a
/// human-readable block under the heading instead.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExportMetadataFormat {
    #[default]
    Frontmatter,
    Inline,
}

/// HTTP method for a webhook delivery ([integrations.md] §Webhooks).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WebhookMethod {
    #[default]
    Post,
    Put,
}

/// One outbound webhook destination ([integrations.md] §Webhooks).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct WebhookDestination {
    /// Where the meeting-finished payload goes. Rows with an empty URL are
    /// skipped at fire time (a half-typed settings row is not an error).
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub method: WebhookMethod,
    /// Full meeting content (summary, the user's notes, transcript) rides
    /// along only while this is on; the default payload is metadata alone.
    /// Content leaving the machine wants an explicit gate per destination.
    #[serde(default)]
    pub include_content: bool,
}

/// Whether the post-meeting pass names diarized speakers from the notes
/// the user typed during the meeting ([speakers.md]).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NotesNamingMode {
    /// Never look at the notes; labels stay "Speaker 1/2/…".
    Off,
    /// Surface "looks like …" suggestions for approval.
    #[default]
    Suggest,
    /// Apply names without asking.
    Automatic,
}

/// How eagerly diarization splits voices apart. Maps to the pyannote
/// clustering threshold: a larger threshold merges more (fewer speakers),
/// so Low = the strongest merging.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiarizationSensitivity {
    /// Fewer speakers: only clearly distinct voices split.
    Low,
    #[default]
    Medium,
    /// More speakers: similar voices split apart sooner.
    High,
}

impl DiarizationSensitivity {
    pub fn clustering_threshold(self) -> f32 {
        match self {
            DiarizationSensitivity::Low => 0.65,
            DiarizationSensitivity::Medium => 0.5,
            DiarizationSensitivity::High => 0.35,
        }
    }
}

/// Capability flags advertised by a transcription provider to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    /// Speaker labels from this provider are final: the post-meeting speaker
    /// pipeline must not relabel them. `false` for the local provider, whose
    /// live labels are a provisional preview the pipeline overwrites.
    pub labels_authoritative: bool,
    pub max_session_minutes: u32,
}

/// The cloud backend release builds talk to.
#[cfg(feature = "cloud")]
pub const DEFAULT_CLOUD_URL: &str = "https://cloud.embral.app";
/// Where dev builds look instead: the local server's default bind
/// (`server` listens on 0.0.0.0:8080).
#[cfg(feature = "cloud")]
pub const DEV_CLOUD_URL: &str = "http://localhost:8080";

/// Application configuration stored at ~/embral/config.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub transcription_provider: TranscriptionProvider,
    /// The language meetings and dictation are transcribed in.
    #[serde(default)]
    pub transcription_language: TranscriptionLanguage,
    /// Cloud-edition only: whether the machine's power source overrides
    /// `transcription_provider` for meetings. Read once, at
    /// `start_recording`.
    #[cfg(feature = "cloud")]
    #[serde(default)]
    pub transcription_power_policy: PowerPolicy,
    /// Cloud-edition only: what a cloud recording does when the account's
    /// hours run out mid-meeting.
    #[cfg(feature = "cloud")]
    #[serde(default)]
    pub cloud_out_of_hours: CloudOutOfHours,
    /// Cloud-edition only: a backend base-URL override; empty = this
    /// build's default (release: [`DEFAULT_CLOUD_URL`], dev:
    /// [`DEV_CLOUD_URL`]). Read through [`AppConfig::cloud_url`], never
    /// directly. The relay WebSocket derives from it.
    #[cfg(feature = "cloud")]
    #[serde(default)]
    pub cloud_api_url: String,
    /// The signed-in device's session token; empty = signed out.
    #[cfg(feature = "cloud")]
    #[serde(default)]
    pub cloud_session_token: String,
    /// Signed-in email, kept for display while offline.
    #[cfg(feature = "cloud")]
    #[serde(default)]
    pub cloud_account_email: String,
    /// Random per-install id (uuid), minted at first sign-in and never
    /// cleared — the server dedupes this install's sessions by it (the
    /// device *name* is display-only; names collide across machines).
    #[cfg(feature = "cloud")]
    #[serde(default)]
    pub cloud_device_id: String,
    pub storage_dir: String,
    pub retain_audio: bool,
    /// Model id from the local engine's catalog (e.g. `zipformer-en`) used by
    /// the `Local` transcription provider.
    #[serde(default = "default_local_asr_model")]
    pub local_asr_model: String,
    /// Words/phrases the local transcriber should listen for more carefully
    /// (proper nouns, project names). Empty = vocabulary boost off.
    #[serde(default)]
    pub vocabulary: Vec<String>,

    // --- Post-meeting integrations ---
    /// When true, a copy of each finished meeting's notes is written into
    /// `obsidian_vault_dir`.
    #[serde(default)]
    pub obsidian_export_enabled: bool,
    /// Absolute path to an Obsidian vault (or any folder) to mirror notes into.
    #[serde(default)]
    pub obsidian_vault_dir: String,
    /// Outbound webhooks fired when a meeting finalizes
    /// ([integrations.md] §Webhooks). Empty = off.
    #[serde(default)]
    pub webhooks: Vec<WebhookDestination>,
    /// Filename template for exported copies (auto-export/Obsidian), with
    /// tokens {date} {time} {year} {month} {day} {hour} {minute} {title}.
    /// Internal library filenames are unaffected.
    #[serde(default = "default_export_filename_template")]
    pub export_filename_template: String,
    /// Metadata style for exported copies.
    #[serde(default)]
    pub export_metadata_format: ExportMetadataFormat,
    /// Whether the exported markdown carries the AI summary.
    #[serde(default = "default_true")]
    pub export_include_summary: bool,
    /// Whether it carries the user's own typed notes.
    #[serde(default = "default_true")]
    pub export_include_notes: bool,
    /// Whether it carries the full transcript.
    #[serde(default = "default_true")]
    pub export_include_transcript: bool,

    // --- Appearance & app behavior ---
    /// UI color scheme.
    #[serde(default)]
    pub theme: Theme,
    /// Color of the tray's recording disc as `#RRGGBB`; empty = follow the
    /// Windows accent color.
    #[serde(default)]
    pub tray_recording_color: String,
    /// Microphone device name; empty = system default.
    #[serde(default)]
    pub mic_device: String,
    /// Output device to capture system audio from (loopback); empty = default.
    #[serde(default)]
    pub output_device: String,
    /// Notify when a meeting's summary is ready.
    #[serde(default = "default_true")]
    pub notify_summary_ready: bool,
    /// Notify when a recording starts while the window is hidden.
    #[serde(default = "default_true")]
    pub notify_recording_started: bool,
    /// Notify when an app update has been downloaded and is ready to
    /// install.
    #[serde(default = "default_true")]
    pub notify_update_available: bool,
    /// Auto-delete audio files older than this many days (0 = never).
    /// Transcripts and notes are always kept.
    #[serde(default)]
    pub audio_retention_days: u32,
    /// Whether the first-run setup wizard has been completed.
    #[serde(default)]
    pub onboarding_completed: bool,

    // --- Telemetry (cloud edition only, [telemetry.md]) ---
    /// Usage telemetry. On by default in the cloud edition; the onboarding
    /// checkbox (pre-checked, on the closing page) and the General-settings
    /// toggle are the only writers — unchecking either opts out.
    #[cfg(feature = "cloud")]
    #[serde(default = "default_true")]
    pub telemetry_enabled: bool,
    /// Persistent per-install id (random UUID): minted at first boot (or
    /// when telemetry is re-enabled), cleared on opt-out — re-enabling
    /// mints a fresh one. Empty while disabled.
    #[cfg(feature = "cloud")]
    #[serde(default)]
    pub telemetry_install_id: String,
    /// Date (YYYY-MM-DD) the daily `config_snapshot` event last fired.
    #[cfg(feature = "cloud")]
    #[serde(default)]
    pub telemetry_last_snapshot: String,

    // --- Meeting detection & automation ---
    /// What to do when a meeting app starts using the microphone.
    #[serde(default)]
    pub auto_start_policy: AutoStartPolicy,
    /// App names considered "meeting apps" for the selective/prompt policies
    /// (matched case-insensitively against process names, `.exe` ignored).
    #[serde(default = "default_auto_detect_apps")]
    pub auto_detect_apps: Vec<String>,
    /// Consecutive seconds of mic use before detection acts (filters chimes).
    #[serde(default = "default_detection_delay_secs")]
    pub detection_delay_secs: u32,
    /// Which recordings stop when the detected call ends. The short settling
    /// delay before that happens is not a setting (see `autodetect`).
    #[serde(default)]
    pub auto_stop: AutoStopScope,
    /// Minutes without a transcribed word before a recording checks in
    /// ("Still recording?"); 0 = never. Paused spans and recordings with
    /// transcription disabled never count as silence.
    #[serde(default = "default_silence_stop_minutes")]
    pub silence_stop_minutes: u32,
    /// What an unanswered check-in does after its fixed two-minute grace.
    #[serde(default)]
    pub silence_stop_unanswered: SilenceUnanswered,
    /// Notify when a call is detected (prompt policy).
    #[serde(default = "default_true")]
    pub notify_call_detected: bool,
    /// Global shortcut that starts/stops recording; empty = unset.
    #[serde(default)]
    pub record_hotkey: String,

    // --- Speakers ---
    /// Sidebar expanded (labels visible) vs collapsed to icons.
    #[serde(default)]
    pub sidebar_expanded: bool,
    /// Days after which whole meetings (notes, transcript, audio, row) are
    /// deleted; 0 = keep forever.
    #[serde(default)]
    pub meeting_retention_days: u32,
    /// Whether meetings get speaker labels at all: gates the live labels
    /// and the post-meeting speaker pipeline together.
    #[serde(default = "default_true")]
    pub diarization_enabled: bool,
    /// How eagerly diarization splits voices apart.
    #[serde(default)]
    pub diarization_sensitivity: DiarizationSensitivity,
    /// Whether speakers get named from the user's typed notes.
    #[serde(default)]
    pub notes_naming_mode: NotesNamingMode,

    // --- Synthesis ---
    /// Whether meetings are summarized at all. Off: no summary is written and
    /// the meeting is its notes + transcript ([synthesis.md]). Defaults on —
    /// summarizing is what the app is for, and onboarding asks anyway.
    #[serde(default = "default_true")]
    pub summaries_enabled: bool,
    /// Engine that writes meeting summaries: "builtin", or "cloud" in cloud
    /// builds. Only consulted while `summaries_enabled`.
    #[serde(default = "default_summaries_profile_id")]
    pub summaries_profile_id: String,
    /// The user's full summary prompt; empty = the built-in default. The
    /// locked output contract is appended either way (embral-notes::prompt).
    #[serde(default)]
    pub summary_prompt: String,
    /// Which tab a meeting opens on.
    #[serde(default)]
    pub open_meeting_tab: OpenMeetingTab,
    /// Keep the built-in LLM loaded between uses (vs evict when idle).
    #[serde(default)]
    pub llm_keep_warm: bool,
    /// Minutes of inactivity before the built-in LLM is evicted.
    #[serde(default = "default_llm_idle_minutes")]
    pub llm_idle_minutes: u32,

    // --- Dictation ---
    /// Global dictation shortcut; empty = unset. Tap toggles; holding it
    /// works push-to-talk style.
    #[serde(default)]
    pub dictation_hotkey: String,
    /// Where dictation transcribes — independent of meetings (cloud meetings
    /// with on-device dictation is a legitimate combination). Gaining hours
    /// adopts cloud for both, like meetings.
    #[serde(default)]
    pub dictation_provider: TranscriptionProvider,
    /// What a cloud dictation does when the hours run out. `disabled` means
    /// the dictation fails with a clear error — there is no "keep recording"
    /// for dictation.
    #[cfg(feature = "cloud")]
    #[serde(default)]
    pub dictation_out_of_hours: CloudOutOfHours,
    /// Dictation's own language, independent of meetings'.
    #[serde(default)]
    pub dictation_language: TranscriptionLanguage,
    /// ASR model for on-device dictation; empty = same as meetings.
    #[serde(default)]
    pub dictation_asr_model: String,
    /// How dictations are cleaned up. Cloud degrades to on-device while
    /// signed out; a failure on either delivers the raw text.
    #[serde(default)]
    pub dictation_cleanup: DictationCleanup,
    /// Leave the finished text on the clipboard.
    #[serde(default = "default_true")]
    pub dictation_copy_clipboard: bool,
    /// Paste into the focused app on finish. With the clipboard switch off,
    /// the text is still staged there to synthesize Ctrl+V, then the
    /// previous clipboard contents come back.
    #[serde(default = "default_true")]
    pub dictation_auto_paste: bool,
    /// Master switch for history pruning; the two criteria below apply only
    /// while this is on (and 0 disables a criterion individually).
    #[serde(default)]
    pub dictation_auto_delete: bool,
    /// Delete dictations older than N days.
    #[serde(default)]
    pub dictation_retention_days: u32,
    /// Keep only the newest N dictations.
    #[serde(default)]
    pub dictation_retention_count: u32,
}

fn default_llm_idle_minutes() -> u32 {
    10
}

/// Default meeting-app allowlist, per platform. Entries are brand tokens
/// the bidirectional-substring matcher tests against every identity the
/// platform reports — exe names on Windows (`msedge.exe`), bundle ids and
/// display names on macOS (`us.zoom.xos`, "Google Chrome"), which is why
/// the macOS list says "edge"/"safari" where Windows says "msedge".
#[cfg(windows)]
fn default_auto_detect_apps() -> Vec<String> {
    // `ms-teams` used to sit beside `teams` here. It was redundant — `teams`
    // matches `ms-teams.exe` by substring — and worse than redundant: the
    // settings grid has only a `teams` checkbox, so the extra entry survived
    // an uncheck and kept Teams detected. Removed, and
    // `no_token_is_redundant` below keeps it from coming back.
    ["zoom", "teams", "chrome", "msedge", "firefox", "slack", "discord", "webex"]
        .map(String::from)
        .to_vec()
}

#[cfg(target_os = "macos")]
fn default_auto_detect_apps() -> Vec<String> {
    ["zoom", "teams", "chrome", "edge", "safari", "firefox", "slack", "discord", "webex"]
        .map(String::from)
        .to_vec()
}

/// Linux reports bare process names, so the tokens are the Windows ones
/// without `.exe` — Edge is `msedge` again, not macOS's `edge`. Two
/// differences from both siblings:
///
/// - **No `safari`**: there is no Safari for Linux. This arm exists largely
///   because the previous `cfg(not(windows))` fallthrough handed Linux the
///   macOS list and Safari with it.
/// - **`chromium` is its own entry**: the matcher's substring test is
///   bidirectional, but neither "chrome" nor "chromium" contains the other,
///   so `chrome` genuinely does not cover it.
///
/// Deliberately *not* here: `teams-for-linux`, the Linux Teams client's
/// binary name. `teams` already matches it by substring, and an entry with
/// no checkbox in the settings grid cannot be unchecked — the grid tests
/// exact membership. (Windows' `ms-teams` is exactly that wart; not
/// reproducing it here.) Every token below has a matching checkbox.
#[cfg(target_os = "linux")]
fn default_auto_detect_apps() -> Vec<String> {
    ["zoom", "teams", "chrome", "chromium", "msedge", "firefox", "slack", "discord", "webex"]
        .map(String::from)
        .to_vec()
}
fn default_detection_delay_secs() -> u32 {
    3
}

fn default_summaries_profile_id() -> String {
    BUILTIN_PROFILE_ID.to_string()
}

fn default_export_filename_template() -> String {
    "{date}-{time}-{title}".to_string()
}

fn default_silence_stop_minutes() -> u32 {
    5
}

fn default_true() -> bool {
    true
}

/// Default local ASR model id (see the `embral-engine` catalog).
pub const DEFAULT_LOCAL_ASR_MODEL: &str = "zipformer-en";

/// The one catalog model that covers languages beyond English. The accuracy
/// tier is an English concept — there is nothing to choose between here.
pub const MULTILINGUAL_ASR_MODEL: &str = "parakeet-tdt-v3";

fn default_local_asr_model() -> String {
    DEFAULT_LOCAL_ASR_MODEL.to_string()
}

impl AppConfig {
    /// The model on-device transcription actually runs. `local_asr_model`
    /// holds the *English* accuracy choice, so selecting another language
    /// overrides it rather than overwriting it — switching back restores the
    /// tier the user picked.
    pub fn meeting_asr_model(&self) -> String {
        match self.transcription_language {
            TranscriptionLanguage::English => self.local_asr_model.clone(),
            TranscriptionLanguage::Multilingual => MULTILINGUAL_ASR_MODEL.to_string(),
        }
    }

    /// The model on-device dictation runs. Governed by dictation's *own*
    /// language; empty `dictation_asr_model` = follow the meeting model.
    pub fn dictation_asr_model_id(&self) -> String {
        match self.dictation_language {
            TranscriptionLanguage::Multilingual => MULTILINGUAL_ASR_MODEL.to_string(),
            TranscriptionLanguage::English => {
                let configured = self.dictation_asr_model.trim();
                if configured.is_empty() {
                    self.local_asr_model.clone()
                } else {
                    configured.to_string()
                }
            }
        }
    }

    /// The cloud backend this build talks to: the `cloud_api_url` override
    /// when set, else the production URL — or, in dev builds, the local
    /// server (`DEV_CLOUD_URL`), so `tauri dev` tests against
    /// `pnpm dev` on 8080 without touching config.
    #[cfg(feature = "cloud")]
    pub fn cloud_url(&self) -> String {
        let configured = self.cloud_api_url.trim();
        if configured.is_empty() {
            if cfg!(debug_assertions) {
                DEV_CLOUD_URL.to_string()
            } else {
                DEFAULT_CLOUD_URL.to_string()
            }
        } else {
            configured.to_string()
        }
    }

    /// The Soniox language hint for cloud sessions: a hint for English, and
    /// none at all for multilingual (the vendor then auto-detects).
    pub fn language_hints(&self) -> Option<Vec<String>> {
        Self::hints_for(self.transcription_language)
    }

    /// Dictation's own hint — its language is independent of meetings'.
    pub fn dictation_language_hints(&self) -> Option<Vec<String>> {
        Self::hints_for(self.dictation_language)
    }

    fn hints_for(language: TranscriptionLanguage) -> Option<Vec<String>> {
        match language {
            TranscriptionLanguage::English => Some(vec!["en".to_string()]),
            TranscriptionLanguage::Multilingual => None,
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            transcription_provider: TranscriptionProvider::default(),
            transcription_language: TranscriptionLanguage::default(),
            #[cfg(feature = "cloud")]
            transcription_power_policy: PowerPolicy::default(),
            #[cfg(feature = "cloud")]
            cloud_out_of_hours: CloudOutOfHours::default(),
            #[cfg(feature = "cloud")]
            cloud_api_url: String::new(),
            #[cfg(feature = "cloud")]
            cloud_session_token: String::new(),
            #[cfg(feature = "cloud")]
            cloud_account_email: String::new(),
            #[cfg(feature = "cloud")]
            cloud_device_id: String::new(),
            storage_dir: default_storage_dir(),
            retain_audio: true,
            local_asr_model: default_local_asr_model(),
            vocabulary: Vec::new(),
            obsidian_export_enabled: false,
            obsidian_vault_dir: String::new(),
            webhooks: Vec::new(),
            export_filename_template: default_export_filename_template(),
            export_metadata_format: ExportMetadataFormat::default(),
            export_include_summary: true,
            export_include_notes: true,
            export_include_transcript: true,
            theme: Theme::default(),
            tray_recording_color: String::new(),
            mic_device: String::new(),
            output_device: String::new(),
            notify_summary_ready: true,
            notify_recording_started: true,
            notify_update_available: true,
            audio_retention_days: 0,
            onboarding_completed: false,
            #[cfg(feature = "cloud")]
            telemetry_enabled: true,
            #[cfg(feature = "cloud")]
            telemetry_install_id: String::new(),
            #[cfg(feature = "cloud")]
            telemetry_last_snapshot: String::new(),
            auto_start_policy: AutoStartPolicy::default(),
            auto_detect_apps: default_auto_detect_apps(),
            detection_delay_secs: default_detection_delay_secs(),
            auto_stop: AutoStopScope::default(),
            silence_stop_minutes: default_silence_stop_minutes(),
            silence_stop_unanswered: SilenceUnanswered::default(),
            notify_call_detected: true,
            record_hotkey: String::new(),
            sidebar_expanded: false,
            meeting_retention_days: 0,
            diarization_enabled: true,
            diarization_sensitivity: DiarizationSensitivity::default(),
            notes_naming_mode: NotesNamingMode::default(),
            summaries_enabled: true,
            summaries_profile_id: default_summaries_profile_id(),
            summary_prompt: String::new(),
            open_meeting_tab: OpenMeetingTab::default(),
            llm_keep_warm: false,
            llm_idle_minutes: default_llm_idle_minutes(),
            dictation_hotkey: String::new(),
            dictation_provider: TranscriptionProvider::default(),
            #[cfg(feature = "cloud")]
            dictation_out_of_hours: CloudOutOfHours::default(),
            dictation_language: TranscriptionLanguage::default(),
            dictation_asr_model: String::new(),
            dictation_cleanup: DictationCleanup::default(),
            dictation_copy_clipboard: true,
            dictation_auto_paste: true,
            dictation_auto_delete: false,
            dictation_retention_days: 0,
            dictation_retention_count: 0,
        }
    }
}

#[cfg(test)]
mod language_tests {
    use super::*;

    #[test]
    fn multilingual_overrides_the_model_without_losing_the_english_tier() {
        let mut config = AppConfig::default();
        config.local_asr_model = "parakeet-tdt-en".to_string();
        config.dictation_asr_model = "zipformer-en-small".to_string();

        // Each surface follows its *own* language — meetings going
        // multilingual must not drag dictation along.
        config.transcription_language = TranscriptionLanguage::Multilingual;
        assert_eq!(config.meeting_asr_model(), MULTILINGUAL_ASR_MODEL);
        assert_eq!(config.dictation_asr_model_id(), "zipformer-en-small");

        config.dictation_language = TranscriptionLanguage::Multilingual;
        assert_eq!(config.dictation_asr_model_id(), MULTILINGUAL_ASR_MODEL);

        // Switching back restores the tier the user picked, not the default.
        config.transcription_language = TranscriptionLanguage::English;
        config.dictation_language = TranscriptionLanguage::English;
        assert_eq!(config.meeting_asr_model(), "parakeet-tdt-en");
        assert_eq!(config.dictation_asr_model_id(), "zipformer-en-small");
    }

    #[test]
    fn dictation_follows_meetings_when_unset() {
        let mut config = AppConfig::default();
        config.local_asr_model = "zipformer-en".to_string();
        config.dictation_asr_model = String::new();
        assert_eq!(config.dictation_asr_model_id(), "zipformer-en");
    }

    /// An existing config.json predates this release: it still carries the
    /// fields we deleted and none of the ones we added. It must still load —
    /// unknown keys ignored, new keys defaulted — and land on sensible values.
    /// (No compat shims; this only pins that the *schema* degrades cleanly.)
    #[test]
    fn a_config_from_before_this_release_still_loads() {
        let old = r#"{
            "transcription_provider": "local",
            "storage_dir": "C:\\Users\\x\\embral",
            "retain_audio": true,
            "local_asr_model": "parakeet-tdt-en",
            "notifications_enabled": true,
            "notify_notes_ready": true,
            "auto_stop_grace_secs": 30,
            "summaries_profile_id": "builtin"
        }"#;
        let config: AppConfig = serde_json::from_str(old).expect("old config still loads");

        assert_eq!(config.local_asr_model, "parakeet-tdt-en");
        assert_eq!(config.transcription_language, TranscriptionLanguage::English);
        // The engine survives; the on/off switch defaults on, so a user who
        // was getting summaries keeps getting them.
        assert!(config.summaries_enabled);
        assert_eq!(config.summaries_profile_id, "builtin");
        // The renamed notification toggle defaults on rather than vanishing.
        assert!(config.notify_summary_ready);
        // The export-include switches default on: an export that used to
        // carry everything keeps carrying everything.
        assert!(config.export_include_summary);
        assert!(config.export_include_notes);
        assert!(config.export_include_transcript);
    }

    #[test]
    #[cfg(feature = "cloud")]
    fn cloud_url_prefers_the_override_and_resolves_per_build() {
        let mut config = AppConfig::default();
        // The stored field is a pure override — empty by default.
        assert!(config.cloud_api_url.is_empty());
        let expected = if cfg!(debug_assertions) {
            DEV_CLOUD_URL
        } else {
            DEFAULT_CLOUD_URL
        };
        assert_eq!(config.cloud_url(), expected);

        config.cloud_api_url = "http://192.168.1.5:9999".into();
        assert_eq!(config.cloud_url(), "http://192.168.1.5:9999");
    }

    #[test]
    fn english_hints_the_vendor_and_multilingual_stays_silent() {
        let mut config = AppConfig::default();
        config.transcription_language = TranscriptionLanguage::English;
        assert_eq!(config.language_hints(), Some(vec!["en".to_string()]));

        // No hint at all: absent means "detect the language".
        config.transcription_language = TranscriptionLanguage::Multilingual;
        assert_eq!(config.language_hints(), None);
    }
}

#[cfg(test)]
mod llm_profile_tests {
    use super::*;

    #[test]
    #[cfg(not(feature = "cloud"))]
    fn available_profiles_is_builtin_first_and_only() {
        let profiles = available_profiles();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].id, BUILTIN_PROFILE_ID);
        assert_eq!(profiles[0].provider, LlmProvider::Builtin);
    }

    #[test]
    #[cfg(feature = "cloud")]
    fn available_profiles_is_builtin_then_cloud() {
        let profiles = available_profiles();
        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0].id, BUILTIN_PROFILE_ID);
        assert_eq!(profiles[1].id, CLOUD_PROFILE_ID);
        assert_eq!(profiles[1].provider, LlmProvider::Custom);
    }

    #[test]
    fn data_dir_env_overrides_storage_root() {
        std::env::set_var("EMBRAL_DATA_DIR", "X:\\scratch-library");
        assert_eq!(default_storage_dir(), "X:\\scratch-library");
        std::env::remove_var("EMBRAL_DATA_DIR");
        assert!(default_storage_dir().ends_with("embral"));
    }
}

// --- Path helpers ---

/// The OS-native default storage directory (e.g. `C:\Users\you\embral`).
/// `EMBRAL_DATA_DIR` overrides the whole root — a development affordance
/// for driving a real build against a scratch library (configuration.md);
/// on Windows the home dir comes from the known-folder API, so an env
/// override is the only way to redirect a build. Falls back to the `~`
/// shorthand only when no home dir can be resolved.
pub fn default_storage_dir() -> String {
    if let Ok(dir) = std::env::var("EMBRAL_DATA_DIR") {
        if !dir.trim().is_empty() {
            return dir;
        }
    }
    dirs::home_dir()
        .map(|home| home.join("embral").to_string_lossy().to_string())
        .unwrap_or_else(|| "~/embral".to_string())
}

/// Resolve storage base path, expanding ~ to the user's home directory.
pub fn resolve_storage_path(storage_dir: &str) -> PathBuf {
    if storage_dir.starts_with("~/") || storage_dir.starts_with("~\\") {
        if let Some(home) = dirs::home_dir() {
            return home.join(&storage_dir[2..]);
        }
    }
    PathBuf::from(storage_dir)
}

pub fn audio_dir(base: &Path) -> PathBuf {
    base.join("audio")
}

pub fn transcripts_dir(base: &Path) -> PathBuf {
    base.join("transcripts")
}

pub fn notes_dir(base: &Path) -> PathBuf {
    base.join("notes")
}

pub fn index_path(base: &Path) -> PathBuf {
    base.join("index.json")
}

pub fn config_path(base: &Path) -> PathBuf {
    base.join("config.json")
}

#[cfg(test)]
mod detect_default_tests {
    use super::*;

    /// Properties every platform's list must hold. There were no tests over
    /// these lists at all before the Linux arm existed, which is how the
    /// `cfg(not(windows))` fallthrough quietly handed Linux the macOS list.
    #[test]
    fn the_default_list_is_well_formed() {
        let apps = default_auto_detect_apps();
        assert!(!apps.is_empty(), "an empty list detects nothing");
        for app in &apps {
            assert_eq!(app, &app.to_lowercase(), "{app} must be lowercase");
            assert!(!app.contains(".exe"), "{app} carries an extension");
            assert_eq!(app.trim(), app, "{app} has stray whitespace");
            assert!(!app.is_empty(), "an empty token matches everything");
        }
        let mut sorted = apps.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), apps.len(), "the list repeats a token");
    }

    /// The regression that motivated splitting the arms: Linux used to fall
    /// through to the macOS list, Safari included, and would have shipped
    /// watching for a browser that does not exist there.
    #[test]
    #[cfg(target_os = "linux")]
    fn linux_does_not_inherit_the_macos_list() {
        let apps = default_auto_detect_apps();
        assert!(!apps.iter().any(|a| a == "safari"), "no Safari on Linux");
        // Edge's Linux process is `msedge`, as on Windows — not macOS's bare
        // `edge`, which came from a bundle id.
        assert!(apps.iter().any(|a| a == "msedge"));
        assert!(!apps.iter().any(|a| a == "edge"));
        // Chromium needs its own token: the matcher is bidirectional, but
        // neither brand name is a substring of the other.
        assert!(apps.iter().any(|a| a == "chromium"));
        assert!(!"chromium".contains("chrome"));
        assert!(!"chrome".contains("chromium"));
        // `teams` covers `teams-for-linux` by substring, so the client's own
        // binary name must not be a separate entry — an entry with no
        // checkbox in the settings grid could never be turned off.
        assert!(apps.iter().any(|a| a == "teams"));
        assert!(!apps.iter().any(|a| a == "teams-for-linux"));
        assert!("teams-for-linux".contains("teams"));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_keeps_its_bundle_id_vocabulary() {
        let apps = default_auto_detect_apps();
        assert!(apps.iter().any(|a| a == "safari"));
        assert!(apps.iter().any(|a| a == "edge"));
    }

    #[test]
    #[cfg(windows)]
    fn windows_keeps_its_exe_vocabulary() {
        let apps = default_auto_detect_apps();
        assert!(apps.iter().any(|a| a == "msedge"));
        assert!(!apps.iter().any(|a| a == "safari"));
    }

    /// No default token may be covered by another. The matcher's test is
    /// bidirectional substring, so a covered token detects nothing its
    /// coverer would not — and it is worse than dead weight: the settings
    /// grid has one checkbox per app, so an extra entry the grid cannot name
    /// survives an uncheck and keeps the app detected while the box reads
    /// off. That is exactly what Windows' `ms-teams` beside `teams` did.
    #[test]
    fn no_token_is_redundant() {
        let apps = default_auto_detect_apps();
        for (i, a) in apps.iter().enumerate() {
            for (j, b) in apps.iter().enumerate() {
                if i == j {
                    continue;
                }
                assert!(
                    !(a.contains(b.as_str()) || b.contains(a.as_str())),
                    "{a:?} and {b:?} cover each other — one of them cannot be \
                     switched off from the settings grid"
                );
            }
        }
    }
}
