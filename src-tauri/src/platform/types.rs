//! Portable types shared by every platform implementation. Pure data: the
//! platform modules produce these, the rest of the app consumes them without
//! knowing which OS filled them in.

/// Where a platform permission stands. Windows reports `NotRequired` for
/// everything; macOS reports real TCC state where the OS exposes it.
/// Each platform constructs only its own variants, hence the allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub enum PermissionState {
    Granted,
    Denied,
    NotDetermined,
    /// This platform never gates the capability.
    NotRequired,
}

/// Where the machine's power is coming from
/// ([transcription.md](../../../docs/transcription.md) §Provider selection).
///
/// A machine with no battery at all reads as `Plugged`: the whole point of
/// the distinction is "is this thing at a desk", and a desktop is the most
/// desk-bound machine there is. `Unknown` is only for a platform that
/// cannot answer: the stub, or an OS call that failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerSource {
    /// Wall power (or a machine with no battery to run from).
    Plugged,
    /// Running down a battery.
    Battery,
    /// The platform did not answer. Callers keep whatever they would have
    /// done without a power reading; never guess.
    Unknown,
}

/// What an OCR engine made of one image
/// ([storage.md](../../../docs/storage.md) §The chunk index).
///
/// Three outcomes rather than an `Option`, because "we read nothing" and
/// "we could not read" have to be told apart: the first is an answer and
/// retires the image, the second must leave it pending. Each platform
/// constructs only the variants its engine can produce, hence the allow.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Recognized {
    /// The engine ran. Empty text is a legitimate result (a photo with no
    /// writing in it) and settles the image for good.
    Text(String),
    /// The engine ran and could not read this file: a truncated download, a
    /// format the decoder rejects, an image past the engine's size limit.
    /// The image is stamped so it is not retried forever.
    Failed(String),
    /// There is no OCR here: the stub platform, or no language pack
    /// installed. The image stays pending, and the caller stops: nothing
    /// else will fare better.
    Unavailable,
}

/// What the system-audio capture is recording right now, logged on every
/// (re)open ([recording.md](../../../docs/recording.md) §Dual-stream
/// capture). Diagnostic only: the picker is what shows the user the
/// choice, so this never crosses into the frontend.
/// The payload fields are read by the `Debug` formatting in that log line
/// and nowhere else, which the dead-code lint cannot see.
#[derive(Debug)]
#[allow(dead_code)]
pub enum SystemAudioSource {
    /// Everything the machine plays: every active output endpoint on
    /// Windows (so an app pinned to any of them is captured), the global
    /// tap on macOS. `devices` is 0 where the platform has no per-device
    /// notion.
    Everything { devices: usize },
    /// Exactly these apps' own audio, wherever they play (per-process
    /// capture; the picker excluded something).
    Apps { names: Vec<String> },
}

/// Builds a fresh 16 kHz-mono sink per capture attempt: every capture
/// gets its own ring in the mixer, so sources sum and one closing never
/// disturbs another.
pub type SystemAudioSinkFactory = Box<dyn Fn() -> Box<dyn Fn(&[f32]) + Send> + Send>;

/// What the recording should be capturing, as chosen in the source picker.
/// Read fresh on every supervision tick, so a checkbox applies live.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SystemAudioWanted {
    /// Everything the machine plays: the default, and the only mode that
    /// needs no per-process capture.
    #[default]
    Everything,
    /// Exactly these process trees (the picker's unchecked apps excluded).
    Apps(Vec<u32>),
}

/// Sent to the capture threads while a recording runs.
pub enum CaptureCommand {
    /// The selection changed; re-read it and rebuild.
    Reconfigure,
}

/// An application observed by the OS: a mic user, or the focused app.
///
/// Which fields are present varies by platform: Windows has executable
/// names (`Zoom.exe`); macOS often only has a bundle id, frequently for
/// the helper process actually holding the audio stream
/// (`com.google.Chrome.helper`), plus a display name for the owning app.
/// The matcher ([detection.md](../../../docs/detection.md)) accepts any of
/// them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppId {
    pub pid: u32,
    /// Executable base name (e.g. `Zoom.exe`, `zoom.us`).
    pub exe: Option<String>,
    /// Bundle identifier where the platform has one (macOS).
    pub bundle_id: Option<String>,
    /// Human-facing name where the platform has one.
    pub display_name: Option<String>,
}

impl AppId {
    /// An app known only by its executable name: the Windows case (and
    /// the matcher's tests; unused by the macOS scan, which fills bundle
    /// ids and display names directly).
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn from_exe(pid: u32, exe: String) -> Self {
        Self {
            pid,
            exe: Some(exe),
            bundle_id: None,
            display_name: None,
        }
    }

    /// The best human-facing label: what detection events, dictation
    /// history, and logs show. Prefers the executable name (the historical
    /// vocabulary on Windows), then the display name, then the bundle id.
    pub fn label(&self) -> &str {
        self.exe
            .as_deref()
            .or(self.display_name.as_deref())
            .or(self.bundle_id.as_deref())
            .unwrap_or("")
    }

    /// Every identity string the matcher may test against the allowlist.
    pub fn identities(&self) -> impl Iterator<Item = &str> {
        [
            self.exe.as_deref(),
            self.bundle_id.as_deref(),
            self.display_name.as_deref(),
        ]
        .into_iter()
        .flatten()
    }
}
