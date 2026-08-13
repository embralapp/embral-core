use anyhow::Result;
use async_trait::async_trait;
use embral_types::{AppConfig, ProviderCapabilities, TranscriptionSegment};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

pub mod local;
pub mod stream;

/// Cadence for the standardized per-session info heartbeat. Every provider
/// emits one `heartbeat` line at this interval while audio is flowing, so a
/// long recording produces a steady, low-volume liveness signal rather than a
/// per-frame firehose.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(20);

/// 16 kHz mono is the canonical source rate fed to every provider (the recorder
/// resamples to it before fan-out), so audio-seconds == samples / 16000
/// uniformly, even for providers that re-resample internally.
pub(crate) const SOURCE_SAMPLE_RATE: f64 = 16_000.0;

/// Lock-free counters backing the standardized session heartbeat and finish
/// summary shared by every provider, so the log shape can't drift between
/// them.
///
/// - `frames` — audio ingest calls (one per `send_audio` / processed chunk).
/// - `samples` — 16 kHz source samples ingested, for the audio-seconds figure.
/// - `segments` — finalized utterances emitted.
///
/// Providers call [`on_audio`](SessionStats::on_audio) from their audio-ingest
/// path (which also emits the throttled heartbeat), [`on_segment`] whenever they
/// emit a finalized `Segment`, and [`finish`] once during teardown. All logging
/// is left to the caller's span so lines are tagged with `provider`.
pub(crate) struct SessionStats {
    frames: AtomicU64,
    samples: AtomicU64,
    segments: AtomicUsize,
    /// Millis-since-`started` at which the last heartbeat fired (0 = none yet).
    last_heartbeat: AtomicU64,
    started: Instant,
}

impl SessionStats {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            frames: AtomicU64::new(0),
            samples: AtomicU64::new(0),
            segments: AtomicUsize::new(0),
            last_heartbeat: AtomicU64::new(0),
            started: Instant::now(),
        })
    }

    /// Record one ingested audio frame of `samples` 16 kHz source samples, then
    /// emit the heartbeat if [`HEARTBEAT_INTERVAL`] has elapsed since the last.
    pub(crate) fn on_audio(&self, samples: usize) {
        self.frames.fetch_add(1, Ordering::Relaxed);
        self.samples.fetch_add(samples as u64, Ordering::Relaxed);
        self.maybe_heartbeat();
    }

    /// Record one finalized segment.
    pub(crate) fn on_segment(&self) {
        self.segments.fetch_add(1, Ordering::Relaxed);
    }

    fn audio_secs(&self) -> u64 {
        (self.samples.load(Ordering::Relaxed) as f64 / SOURCE_SAMPLE_RATE) as u64
    }

    fn maybe_heartbeat(&self) {
        let elapsed_ms = self.started.elapsed().as_millis() as u64;
        let last = self.last_heartbeat.load(Ordering::Relaxed);
        if elapsed_ms.saturating_sub(last) < HEARTBEAT_INTERVAL.as_millis() as u64 {
            return;
        }
        // Claim this slot; if a concurrent ingest beat us to it, let them log.
        if self
            .last_heartbeat
            .compare_exchange(last, elapsed_ms, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        tracing::info!(
            audio_secs = self.audio_secs(),
            segments = self.segments.load(Ordering::Relaxed),
            "heartbeat"
        );
    }

    /// Emit the standardized one-line finish summary. `reason` is one of
    /// `clean` / `timeout` / `error`.
    pub(crate) fn finish(&self, reason: &str) {
        tracing::info!(
            frames = self.frames.load(Ordering::Relaxed),
            audio_secs = self.audio_secs(),
            segments = self.segments.load(Ordering::Relaxed),
            reason,
            "finish"
        );
    }
}

/// Events streamed from a transcription session to the rest of the app.
#[derive(Debug)]
pub enum TranscriptionEvent {
    /// Live, in-progress preview for the currently-spoken utterance. The
    /// frontend replaces any prior `Interim` with the latest one; `Interim`s
    /// are cleared automatically when a `Segment` arrives.
    ///
    /// `segment.text` holds the **stable** portion of the in-flight utterance
    /// (tokens the provider has already finalized). `tentative` carries the
    /// unstable trailing hypothesis that should be rendered with reduced
    /// emphasis since it can change on the next message. Providers without a
    /// tentative/final split leave it `None`.
    Interim {
        segment: TranscriptionSegment,
        tentative: Option<String>,
    },
    /// A finalized utterance — appended to the persistent transcript.
    Segment(TranscriptionSegment),
    /// The session died mid-recording (connection lost, hours used up).
    /// The forwarder may swap in a replacement session that keeps feeding
    /// this same channel; `Done` still ends it. Only the cloud session
    /// constructs it today, but the variant (and its forwarder arm) stay
    /// ungated — any provider may fail.
    #[cfg_attr(not(feature = "cloud"), allow(dead_code))]
    Failed { message: String },
    /// Session has ended; no more events will arrive on this channel.
    Done,
}

#[async_trait]
pub trait TranscriptionProvider: Send + Sync {
    fn capabilities(&self) -> ProviderCapabilities;

    async fn start_session(
        &self,
        event_tx: mpsc::UnboundedSender<TranscriptionEvent>,
    ) -> Result<Box<dyn TranscriptionSession>>;
}

/// A streaming transcription session.
///
/// Implementations normalize their provider's native event stream into
/// [`TranscriptionSegment`]s under the following contract — downstream
/// consumers (`LiveTranscript.svelte`, `format_transcript` in `commands.rs`)
/// rely on these invariants:
///
/// 1. **Clean text.** `text` is canonical clean prose for one utterance:
///    trimmed, internally well-spaced, no leading or trailing whitespace.
///    Concatenating consecutive segments of the same speaker with a single
///    space yields a correctly-formatted transcript line.
/// 2. **Speaker labels.** `speaker` is `Some(_)` when the provider labels
///    utterances — either authoritatively
///    (`capabilities().labels_authoritative`, kept by the post-meeting
///    pipeline) or as the local provider's provisional live preview
///    (overwritten by the pipeline). Unlabeled utterances carry `None`.
/// 3. **Timing.** `start` and `end` are in seconds since session start. If
///    the provider's transcript event has no timing, synthesize from its
///    voice-activity events.
/// 4. **Interim vs Segment.** Emit `Interim` for the still-changing live
///    preview of the current utterance; emit `Segment` only when the
///    utterance has stabilized (speaker change, sentence-final punctuation,
///    pause-based timeout, VAD endpoint, or session end). `finish()` returns
///    only finalized Segments — never interim previews.
#[async_trait]
pub trait TranscriptionSession: Send + Sync + 'static {
    async fn send_audio(&self, pcm_f32: &[f32]) -> Result<()>;
    async fn finish(self: Box<Self>) -> Result<Vec<TranscriptionSegment>>;
    /// Force-finalize the in-flight utterance (a starred moment) so the
    /// words after it start a new segment. Returns a receiver for the
    /// split point on the segment timeline (the stream clock), so the
    /// star's timestamp orders correctly against segments. Best-effort;
    /// default `None` for providers that segment server-side.
    fn split_utterance(&self) -> Option<tokio::sync::oneshot::Receiver<f64>> {
        None
    }
}

/// `provider` is this recording's lane — the config's standing choice as
/// bent by the power policy (`config::provider_for_power`), which is why it
/// is passed rather than read off `config`.
pub fn build_provider(
    provider: &embral_types::TranscriptionProvider,
    config: &AppConfig,
    engine: Arc<embral_engine::Engine>,
) -> Arc<dyn TranscriptionProvider> {
    match provider {
        embral_types::TranscriptionProvider::Local => Arc::new(local::LocalProvider::new(
            engine,
            config.meeting_asr_model(),
            config.vocabulary.clone(),
            config.diarization_enabled,
        )),
        #[cfg(feature = "cloud")]
        embral_types::TranscriptionProvider::Cloud => {
            relay_provider(config, config.language_hints())
        }
    }
}

/// Dictation's provider: its own provider/language/model tree, and never
/// live speaker labels (one person is talking — it's dictation).
pub fn build_dictation_provider(
    config: &AppConfig,
    engine: Arc<embral_engine::Engine>,
) -> Arc<dyn TranscriptionProvider> {
    match config.dictation_provider {
        embral_types::TranscriptionProvider::Local => Arc::new(local::LocalProvider::new(
            engine,
            config.dictation_asr_model_id(),
            config.vocabulary.clone(),
            false,
        )),
        #[cfg(feature = "cloud")]
        embral_types::TranscriptionProvider::Cloud => {
            relay_provider(config, config.dictation_language_hints())
        }
    }
}

/// The dictation fallback provider, for when the cloud refuses at start.
pub fn build_local_dictation_provider(
    config: &AppConfig,
    engine: Arc<embral_engine::Engine>,
) -> Arc<dyn TranscriptionProvider> {
    Arc::new(local::LocalProvider::new(
        engine,
        config.dictation_asr_model_id(),
        config.vocabulary.clone(),
        false,
    ))
}

#[cfg(feature = "cloud")]
fn relay_provider(
    config: &AppConfig,
    language_hints: Option<Vec<String>>,
) -> Arc<dyn TranscriptionProvider> {
    Arc::new(crate::cloud::transcription::RelayProvider::new(
        config.cloud_session_token.clone(),
        config.cloud_url(),
        language_hints,
        config.diarization_enabled,
        // A recording's (or dictation's) first stream numbers from
        // Speaker 1; only a mid-recording reopen passes a base.
        0,
    ))
}
