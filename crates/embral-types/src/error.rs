//! `AppError` — the typed error the Tauri commands return and the error
//! events carry, so no user-facing failure crosses to the frontend as a bare
//! English string. Serialized with an internal `code` tag; the frontend maps
//! each code to a catalog sentence (`src/lib/copy/en/errors.ts`), interpolating
//! any carried data (a path, an id, a technical detail).
//!
//! The `Display` text is the current English, kept for `tracing` logs and as
//! the fallback the frontend shows for `Internal`. **Wording is Phase 4's** —
//! these strings move verbatim; the frontend catalog is where they get edited.
//!
//! Adding a variant here means adding its code to the frontend's
//! `AppErrorCode` union and `copy.errors` map, or `npm run check` fails — the
//! same completeness gate the copy catalog uses. `code_of` + the unit test
//! below are the Rust side of that contract.

use std::fmt;

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "code", rename_all = "camelCase")]
pub enum AppError {
    // --- Recording / dictation / import state guards ---
    NotConfigured,
    BusyDictating,
    NoActiveRecording,
    /// A second start arrived while one recording was already running —
    /// the record button, the hotkey, and auto-start all reach the same
    /// command, and nothing downstream of it is idempotent.
    AlreadyRecording,
    CantImportWhileRecording,
    ImportAlreadyRunning,
    NeedsLocalModel,
    FileNotFound { path: String },
    AlreadyDownloading,
    StopRecordingBeforeReset,
    StopDictatingBeforeReset,
    CantDictateWhileRecording,
    DictationAlreadyRunning,
    DictationModelMissing { model_id: String },
    CloudSignInRequired,
    NoDictationRunning,

    // --- `update_guard` reasons (a success value, not a thrown error) ---
    RecordingInProgress,
    DictationInProgress,
    ImportInProgress,

    // --- Data / lookup errors ---
    TitleEmpty,
    SpeakerNameEmpty,
    SuggestionNotPending,
    NoStructuredTranscript,
    MeetingNotFound { id: String },

    // --- Emitted on the processing / transcription event channels ---
    EncodeFailed { detail: String },
    ImportFailed { detail: String },
    DictationStartFailed { detail: String },
    CloudUnreachable,
    /// Cloud transcription is selected but no device is signed in — the
    /// recording lands on the configured fallback instead of waiting out
    /// a handshake that cannot succeed.
    CloudSignedOut,

    /// A webhook test delivery failed; `detail` is the transport's own
    /// words (a refused connection, a status code) — the diagnostic the
    /// settings row exists to show.
    WebhookTestFailed { detail: String },

    /// Everything currently `.map_err(|e| e.to_string())?` — a DB/IO/serde
    /// failure or a "shouldn't happen" race. Shown as a generic sentence; the
    /// `detail` is for the log, not the screen.
    Internal { detail: String },
}

impl AppError {
    /// Wrap an incidental error (DB, IO, serde, a panic message) as `Internal`.
    /// The mechanical replacement for `.map_err(|e| e.to_string())`.
    pub fn internal(e: impl fmt::Display) -> Self {
        AppError::Internal {
            detail: e.to_string(),
        }
    }

    /// The serialized `code` tag. Kept in lock-step with the frontend
    /// `AppErrorCode` union; the unit test pins every arm.
    pub fn code(&self) -> &'static str {
        match self {
            AppError::NotConfigured => "notConfigured",
            AppError::BusyDictating => "busyDictating",
            AppError::NoActiveRecording => "noActiveRecording",
            AppError::AlreadyRecording => "alreadyRecording",
            AppError::CantImportWhileRecording => "cantImportWhileRecording",
            AppError::ImportAlreadyRunning => "importAlreadyRunning",
            AppError::NeedsLocalModel => "needsLocalModel",
            AppError::FileNotFound { .. } => "fileNotFound",
            AppError::AlreadyDownloading => "alreadyDownloading",
            AppError::StopRecordingBeforeReset => "stopRecordingBeforeReset",
            AppError::StopDictatingBeforeReset => "stopDictatingBeforeReset",
            AppError::CantDictateWhileRecording => "cantDictateWhileRecording",
            AppError::DictationAlreadyRunning => "dictationAlreadyRunning",
            AppError::DictationModelMissing { .. } => "dictationModelMissing",
            AppError::CloudSignInRequired => "cloudSignInRequired",
            AppError::NoDictationRunning => "noDictationRunning",
            AppError::RecordingInProgress => "recordingInProgress",
            AppError::DictationInProgress => "dictationInProgress",
            AppError::ImportInProgress => "importInProgress",
            AppError::TitleEmpty => "titleEmpty",
            AppError::SpeakerNameEmpty => "speakerNameEmpty",
            AppError::SuggestionNotPending => "suggestionNotPending",
            AppError::NoStructuredTranscript => "noStructuredTranscript",
            AppError::MeetingNotFound { .. } => "meetingNotFound",
            AppError::EncodeFailed { .. } => "encodeFailed",
            AppError::ImportFailed { .. } => "importFailed",
            AppError::DictationStartFailed { .. } => "dictationStartFailed",
            AppError::CloudUnreachable => "cloudUnreachable",
            AppError::CloudSignedOut => "cloudSignedOut",
            AppError::WebhookTestFailed { .. } => "webhookTestFailed",
            AppError::Internal { .. } => "internal",
        }
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::NotConfigured => f.write_str(
                "Transcription isn't set up yet — download the speech model or sign in from Settings.",
            ),
            AppError::BusyDictating => {
                f.write_str("Can't record during a dictation — finish it first.")
            }
            AppError::NoActiveRecording => f.write_str("No active recording"),
            AppError::AlreadyRecording => f.write_str("A recording is already running"),
            AppError::CantImportWhileRecording => {
                f.write_str("Can't import while a recording is in progress.")
            }
            AppError::ImportAlreadyRunning => f.write_str("An import is already in progress."),
            AppError::NeedsLocalModel => f.write_str(
                "Importing needs a local speech model — download one in Settings → Transcription.",
            ),
            AppError::FileNotFound { path } => write!(f, "File not found: {path}"),
            AppError::AlreadyDownloading => f.write_str("This model is already downloading."),
            AppError::StopRecordingBeforeReset => {
                f.write_str("Stop the recording before resetting")
            }
            AppError::StopDictatingBeforeReset => f.write_str("Stop dictating before resetting"),
            AppError::CantDictateWhileRecording => {
                f.write_str("Can't dictate during a meeting recording")
            }
            AppError::DictationAlreadyRunning => f.write_str("Dictation is already running"),
            AppError::DictationModelMissing { model_id } => write!(
                f,
                "The dictation speech model isn't downloaded ({model_id}) — check Settings → Transcription"
            ),
            AppError::CloudSignInRequired => {
                f.write_str("Sign in on the Account page to dictate with embral cloud")
            }
            AppError::NoDictationRunning => f.write_str("No dictation running"),
            AppError::RecordingInProgress => f.write_str("A recording is in progress"),
            AppError::DictationInProgress => f.write_str("A dictation is in progress"),
            AppError::ImportInProgress => f.write_str("An import is in progress"),
            AppError::TitleEmpty => f.write_str("Meeting title cannot be empty"),
            AppError::SpeakerNameEmpty => f.write_str("Speaker name cannot be empty"),
            AppError::SuggestionNotPending => f.write_str("That suggestion is no longer pending"),
            AppError::NoStructuredTranscript => {
                f.write_str("This meeting has no structured transcript to edit")
            }
            AppError::MeetingNotFound { id } => write!(f, "Meeting {id} not found"),
            AppError::EncodeFailed { detail } => write!(f, "encode failed: {detail}"),
            AppError::ImportFailed { detail } => write!(f, "Import failed: {detail}"),
            AppError::DictationStartFailed { detail } => {
                write!(f, "Dictation couldn't start: {detail}")
            }
            AppError::CloudUnreachable => f.write_str("embral cloud is unreachable"),
            AppError::CloudSignedOut => f.write_str("no embral cloud account is signed in"),
            AppError::WebhookTestFailed { detail } => {
                write!(f, "Test delivery failed: {detail}")
            }
            AppError::Internal { detail } => f.write_str(detail),
        }
    }
}

impl std::error::Error for AppError {}

// A bare string error (the pervasive `.map_err(|e| e.to_string())?` and
// `state.db().await?`) folds into `Internal` so `?` keeps working unchanged in
// commands that now return `AppError`. Both arms are std types — no dependency
// creeps into this crate.
impl From<String> for AppError {
    fn from(detail: String) -> Self {
        AppError::Internal { detail }
    }
}

impl From<&str> for AppError {
    fn from(detail: &str) -> Self {
        AppError::Internal {
            detail: detail.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every variant, so the code strings are pinned against the frontend
    /// `AppErrorCode` union. A new variant added without a line here fails to
    /// compile (non-exhaustive match), which is the reminder to update the
    /// frontend too.
    fn all_variants() -> Vec<AppError> {
        use AppError::*;
        let sample = || "x".to_string();
        vec![
            NotConfigured,
            BusyDictating,
            NoActiveRecording,
            AlreadyRecording,
            CantImportWhileRecording,
            ImportAlreadyRunning,
            NeedsLocalModel,
            FileNotFound { path: sample() },
            AlreadyDownloading,
            StopRecordingBeforeReset,
            StopDictatingBeforeReset,
            CantDictateWhileRecording,
            DictationAlreadyRunning,
            DictationModelMissing { model_id: sample() },
            CloudSignInRequired,
            NoDictationRunning,
            RecordingInProgress,
            DictationInProgress,
            ImportInProgress,
            TitleEmpty,
            SpeakerNameEmpty,
            SuggestionNotPending,
            NoStructuredTranscript,
            MeetingNotFound { id: sample() },
            EncodeFailed { detail: sample() },
            ImportFailed { detail: sample() },
            DictationStartFailed { detail: sample() },
            CloudUnreachable,
            CloudSignedOut,
            WebhookTestFailed { detail: sample() },
            Internal { detail: sample() },
        ]
    }

    #[test]
    fn serializes_with_a_code_tag_matching_code_of() {
        for err in all_variants() {
            let value = serde_json::to_value(&err).expect("serialize");
            let code = value
                .get("code")
                .and_then(|c| c.as_str())
                .expect("a string code tag");
            assert_eq!(code, err.code(), "serde tag and code() disagree");
        }
    }

    #[test]
    fn carries_structured_data() {
        let v = serde_json::to_value(AppError::FileNotFound {
            path: "/tmp/x.wav".into(),
        })
        .unwrap();
        assert_eq!(v["code"], "fileNotFound");
        assert_eq!(v["path"], "/tmp/x.wav");
    }

    #[test]
    fn internal_wraps_a_display() {
        let e = AppError::internal(std::io::Error::other("disk gone"));
        assert_eq!(e.code(), "internal");
        assert_eq!(e.to_string(), "disk gone");
    }
}
