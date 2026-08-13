//! The moving parts between the audio bridge and a transcription session:
//! the slot the bridge feeds, the buffer that holds audio while a session
//! open is pending, the lane state a recording's streams share, and the
//! pump that carries one stream's events onto the recording's channel.
//!
//! A recording can run more than one session over its lifetime — the
//! cloud→local fallback, and a cloud stream closed at pause and reopened
//! at resume ([transcription.md]). This module is what makes those
//! handovers safe: audio buffers rather than drops while no session is
//! live, a replacement stream's from-zero timestamps are shifted onto the
//! recording's clock, and a stream the recording has moved past cannot
//! install itself or raise the fallback machinery.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

use super::{TranscriptionEvent, TranscriptionSession, SOURCE_SAMPLE_RATE};

/// Upper bound on audio held while a session open is pending: 60 s at the
/// 16 kHz source rate. A handshake takes a couple of seconds; the cap only
/// matters when a connect hangs, and the recording itself is never at risk
/// (the WAV has everything).
const MAX_BUFFERED_SAMPLES: usize = 60 * 16_000;

/// Time the post-stop pipeline (and every retired-session cleanup) is
/// willing to block waiting for a session to finalize tail audio. A
/// streaming provider can hold its socket open for ~60 s post-stop with
/// empty heartbeats while processing a backlog — but no NEW tokens arrive
/// during that window; waiting past ~5–10 s buys nothing.
pub(crate) const SESSION_FINISH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

/// Ceiling on one send into a session while the slot lock is held (the
/// bridge's live sends, install's buffered drain). Every provider's
/// `send_audio` already bounds itself ([transcription.md] §Provider
/// contract — the relay's socket deadline fires first, with the precise
/// error); this is the backstop that keeps the hold bounded whatever a
/// provider does, because pause, stop, and the watchers all queue on this
/// lock ([recording.md] §Lifecycle).
pub(crate) const BRIDGE_SEND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(6);

/// What the audio bridge is feeding right now.
pub enum SessionSlot {
    /// A live session: audio goes straight to it.
    Streaming(Box<dyn TranscriptionSession>),
    /// A session open is pending: audio waits here.
    Buffering(AudioBuffer),
    /// Transcription is over for this recording: audio goes to disk only.
    Off,
}

pub type SharedSlot = Arc<Mutex<SessionSlot>>;

/// Audio captured while no session is live, bounded by
/// [`MAX_BUFFERED_SAMPLES`]. Overflow drops the oldest chunks — the newest
/// audio is the most likely to still matter to a session that eventually
/// opens, and the offset math self-corrects because dropped chunks were
/// counted into the stream clock but are absent from the buffer.
#[derive(Default)]
pub struct AudioBuffer {
    chunks: VecDeque<Vec<f32>>,
    samples: usize,
}

impl AudioBuffer {
    pub fn push(&mut self, chunk: Vec<f32>) {
        self.samples += chunk.len();
        self.chunks.push_back(chunk);
        while self.samples > MAX_BUFFERED_SAMPLES {
            let Some(front) = self.chunks.pop_front() else {
                break;
            };
            self.samples -= front.len();
        }
    }

    pub fn samples(&self) -> usize {
        self.samples
    }

    fn take(&mut self) -> VecDeque<Vec<f32>> {
        self.samples = 0;
        std::mem::take(&mut self.chunks)
    }
}

/// What a stream reopened after a pause asks the vendor for: the
/// recording's start-time choices, not the current settings — the lane is
/// fixed for the meeting. The session token is deliberately absent; it is
/// read fresh at reopen, so signing out during a pause fails the
/// handshake into the ordinary fallback.
#[derive(Clone)]
pub struct CloudStreamRequest {
    pub language_hints: Option<Vec<String>>,
    pub diarization: bool,
}

/// State one recording's streams share, whoever currently holds the slot.
/// A fresh one is made per recording (`AppState` swaps the `Arc`), so
/// there is no reset choreography — tasks from a previous recording hold
/// their own retired lane and can never confuse the new one.
pub struct StreamLane {
    /// Samples fed through the bridge so far — the recording's stream
    /// clock. Excludes paused spans (paused capture callbacks discard
    /// before the bridge), like the WAV and the segment timeline.
    pub samples_sent: AtomicU64,
    /// Bumped when the recording moves past the current stream (pause on
    /// a cloud lane, stop). An open that resolves against a stale
    /// generation retires its session instead of installing it, and a
    /// stale stream's `Failed`/`Interim` events stop at its pump.
    pub generation: AtomicU64,
    /// The recording's event channel, cloned into each stream's pump.
    /// Taken at stop, so the forwarder ends on channel close once the
    /// last pump drains.
    pub event_tx: std::sync::Mutex<Option<mpsc::UnboundedSender<TranscriptionEvent>>>,
    /// Whether the installed (or last-installed) stream is a cloud
    /// stream — only those are closed at pause and reopened at resume,
    /// and a lane that fell back to local stays local ([transcription.md]).
    pub stream_is_cloud: AtomicBool,
    /// Highest "Speaker N" any stream has produced this recording,
    /// tracked even while the label layer strips: a stream opened
    /// mid-recording numbers its speakers after every one already seen
    /// ([speakers.md]).
    pub max_speaker_number: AtomicUsize,
    /// What a post-pause reopen asks the vendor for; `None` on lanes that
    /// never were cloud.
    pub cloud_reopen: std::sync::Mutex<Option<CloudStreamRequest>>,
}

impl StreamLane {
    pub fn new(event_tx: mpsc::UnboundedSender<TranscriptionEvent>) -> Self {
        Self {
            samples_sent: AtomicU64::new(0),
            generation: AtomicU64::new(0),
            event_tx: std::sync::Mutex::new(Some(event_tx)),
            stream_is_cloud: AtomicBool::new(false),
            max_speaker_number: AtomicUsize::new(0),
            cloud_reopen: std::sync::Mutex::new(None),
        }
    }

    /// The between-recordings placeholder: no channel, so any straggler
    /// task that reads it behaves exactly as after a stop.
    pub fn idle() -> Self {
        Self::new_disconnected()
    }

    fn new_disconnected() -> Self {
        Self {
            samples_sent: AtomicU64::new(0),
            generation: AtomicU64::new(0),
            event_tx: std::sync::Mutex::new(None),
            stream_is_cloud: AtomicBool::new(false),
            max_speaker_number: AtomicUsize::new(0),
            cloud_reopen: std::sync::Mutex::new(None),
        }
    }

    pub fn current_generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// The recording is moving past the current stream; opens in flight
    /// under the old generation retire themselves on arrival.
    pub fn bump_generation(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    pub fn clone_event_tx(&self) -> Option<mpsc::UnboundedSender<TranscriptionEvent>> {
        self.event_tx
            .lock()
            .expect("lane event channel poisoned")
            .clone()
    }

    /// Drop the lane's sender (stop does this): once every pump ends too,
    /// the recording's channel closes and the forwarder exits.
    pub fn take_event_tx(&self) {
        drop(
            self.event_tx
                .lock()
                .expect("lane event channel poisoned")
                .take(),
        );
    }
}

/// End the current stream at pause, when the lane is a cloud lane: bump
/// the generation — an open still in flight must retire on arrival, not
/// install a live, metering stream into a paused recording — and take the
/// session out for a detached bounded finish (the finish flushes in-flight
/// text, so the transcript catches up to the pause point). Local streams
/// stay put: they cost nothing while idle and keep their clustering
/// context ([transcription.md]).
pub async fn pause_stream(lane: &Arc<StreamLane>, slot: &SharedSlot) {
    if !lane.stream_is_cloud.load(Ordering::Acquire) {
        return;
    }
    lane.bump_generation();
    let mut guard = slot.lock().await;
    if matches!(&*guard, SessionSlot::Streaming(_)) {
        let SessionSlot::Streaming(session) =
            std::mem::replace(&mut *guard, SessionSlot::Buffering(AudioBuffer::default()))
        else {
            unreachable!("checked above");
        };
        drop(guard);
        tracing::info!("pause ends the cloud stream; resume opens a fresh one");
        finish_detached(session, "paused");
    }
}

/// Finish a session this recording will not stream to — off the caller's
/// path, bounded. Dropping one instead would leave its socket open: a
/// session owns a receive task that only `finish` reels in, and for a
/// cloud stream an open socket is an open metering row ([server.md]).
pub fn finish_detached(
    session: Box<dyn TranscriptionSession>,
    why: &'static str,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        match tokio::time::timeout(SESSION_FINISH_TIMEOUT, session.finish()).await {
            Ok(Ok(_)) => tracing::info!("retired session ({why}) finished cleanly"),
            Ok(Err(e)) => tracing::info!("retired session ({why}) finish errored: {e}"),
            Err(_) => tracing::warn!("retired session ({why}) finish timed out"),
        }
    })
}

/// What became of one chunk offered to the slot.
pub enum Delivered {
    /// A live session took it.
    Sent,
    /// No live session; it waits in the buffer.
    Buffered,
    /// Transcription is over for this recording; the chunk goes to disk
    /// only (the recorder already wrote it).
    Off,
    /// The live session refused the chunk (send error) or sat on it past
    /// [`BRIDGE_SEND_TIMEOUT`]. The slot has already been flipped to
    /// buffering — with this chunk as its first entry, so the audio still
    /// reaches whatever session comes next — and the session is handed
    /// back for the caller to retire and report **off** the slot lock.
    Stalled {
        session: Box<dyn TranscriptionSession>,
        why: String,
    },
}

/// Feed one captured chunk to whatever holds the slot. This is the audio
/// bridge's hot path, and it is the one place a send happens under the
/// slot lock — bounded by [`BRIDGE_SEND_TIMEOUT`], because a send that
/// could pend indefinitely would hold pause, stop, and the watchers with
/// it (the 2026-08 stop hang). The sample count and the buffer/send
/// decision stay under one hold so the install-time offset math is exact.
pub async fn deliver_chunk(lane: &Arc<StreamLane>, slot: &SharedSlot, chunk: Vec<f32>) -> Delivered {
    let mut guard = slot.lock().await;
    lane.samples_sent
        .fetch_add(chunk.len() as u64, Ordering::Relaxed);
    match &mut *guard {
        SessionSlot::Streaming(session) => {
            let why = match tokio::time::timeout(BRIDGE_SEND_TIMEOUT, session.send_audio(&chunk))
                .await
            {
                Ok(Ok(())) => return Delivered::Sent,
                Ok(Err(e)) => e.to_string(),
                Err(_) => format!(
                    "one audio send sat for {} seconds",
                    BRIDGE_SEND_TIMEOUT.as_secs()
                ),
            };
            let mut buf = AudioBuffer::default();
            buf.push(chunk);
            let SessionSlot::Streaming(session) =
                std::mem::replace(&mut *guard, SessionSlot::Buffering(buf))
            else {
                unreachable!("matched above");
            };
            Delivered::Stalled { session, why }
        }
        SessionSlot::Buffering(buf) => {
            buf.push(chunk);
            Delivered::Buffered
        }
        SessionSlot::Off => Delivered::Off,
    }
}

/// How stop's step one left the stream.
#[derive(Debug, PartialEq, Eq)]
pub enum FinishOutcome {
    /// The live session flushed its tail (or errored trying — either way
    /// it returned) in time.
    Finished,
    /// The live session's finish outlived its deadline; the segments
    /// accumulated so far stand.
    FinishTimedOut,
    /// A session open was still pending; this many source samples of tail
    /// audio are on the WAV but never reached a transcriber.
    PendingOpen(usize),
    /// Transcription was already over for this recording.
    Off,
    /// The slot lock itself could not be had in time — something wedged is
    /// holding it. A detached reaper finishes whatever it finds once the
    /// hold clears; stop proceeds without it.
    SlotHeld,
}

/// End the current stream at stop, every part under `deadline`: the slot
/// take first (a stalled bridge send can hold the lock for a few seconds;
/// nothing may hold it forever, but stop does not bet the meeting on
/// that), then the session's own finish. The finish's return value is
/// deliberately unused — segments come from the recording's accumulator —
/// so every outcome here lets finalize proceed.
pub async fn finish_current_stream(slot: &SharedSlot, deadline: std::time::Duration) -> FinishOutcome {
    let Ok(mut guard) = tokio::time::timeout(deadline, slot.lock()).await else {
        tracing::warn!(
            "the session slot is still held after {deadline:?} — finalizing without it"
        );
        // Off the stop path, take as long as it takes: whatever wedged the
        // slot eventually lets go (bounded sends see to it), and the
        // session it leaves behind still owns a socket worth closing.
        let reaper_slot = slot.clone();
        tokio::spawn(async move {
            if let SessionSlot::Streaming(session) =
                std::mem::replace(&mut *reaper_slot.lock().await, SessionSlot::Off)
            {
                finish_detached(session, "reaped after stop");
            }
        });
        return FinishOutcome::SlotHeld;
    };
    match std::mem::replace(&mut *guard, SessionSlot::Off) {
        SessionSlot::Streaming(session) => {
            drop(guard);
            match tokio::time::timeout(deadline, session.finish()).await {
                Ok(Ok(_)) => {
                    tracing::info!("Transcription session finished cleanly");
                    FinishOutcome::Finished
                }
                Ok(Err(e)) => {
                    tracing::warn!("Transcription session finish errored: {e}");
                    FinishOutcome::Finished
                }
                Err(_) => {
                    tracing::warn!(
                        "Transcription session finish timed out after {deadline:?} — using segments accumulated so far"
                    );
                    FinishOutcome::FinishTimedOut
                }
            }
        }
        SessionSlot::Buffering(buf) => {
            drop(guard);
            if buf.samples() > 0 {
                // A session open was still pending; this tail is on the
                // WAV but never reached a transcriber.
                tracing::info!(
                    "stopped while a session open was pending — {:.1}s of audio goes untranscribed",
                    buf.samples() as f64 / SOURCE_SAMPLE_RATE
                );
            }
            FinishOutcome::PendingOpen(buf.samples())
        }
        SessionSlot::Off => FinishOutcome::Off,
    }
}

/// Put a freshly opened session behind the audio bridge: deliver the audio
/// that buffered while the open was pending, then stream, with a pump
/// carrying its events onto the recording's channel. Returns `false` —
/// after finishing the newcomer, which is what closes its socket — when
/// the recording has moved past this open (pause or stop advanced the
/// generation) or the slot is no longer waiting for a session.
///
/// `seen_labels` is the runaway guard's distinct-label set: it clears at
/// every install because the guard counts one clustering run, not the
/// union of every stream's numbering ([speakers.md]).
///
/// `liveness_clock` is the silence check-in's clock, rebaselined here —
/// under the slot lock, so a watcher tick that sees the slot streaming
/// always reads a clock that already restarted — because the quiet before
/// this install was a stretch with no transcriber, not silence
/// ([detection.md] §Auto-stop on silence).
pub async fn install_stream(
    lane: &Arc<StreamLane>,
    slot: &SharedSlot,
    seen_labels: &std::sync::Mutex<std::collections::HashSet<String>>,
    liveness_clock: &AtomicU64,
    session: Box<dyn TranscriptionSession>,
    stream_rx: mpsc::UnboundedReceiver<TranscriptionEvent>,
    is_cloud: bool,
    generation: u64,
) -> bool {
    let mut guard = slot.lock().await;
    let stale = lane.current_generation() != generation;
    if stale || !matches!(&*guard, SessionSlot::Buffering(_)) {
        drop(guard);
        finish_detached(session, if stale { "superseded" } else { "slot taken" });
        return false;
    }
    let Some(event_tx) = lane.clone_event_tx() else {
        drop(guard);
        finish_detached(session, "recording ended");
        return false;
    };
    let SessionSlot::Buffering(buf) = &mut *guard else {
        unreachable!("checked above");
    };
    // The new stream's clock zero is where its first audio sample sits on
    // the recording's clock: everything the bridge has counted, minus
    // what waited in the buffer for this very session. The bridge counts
    // under the slot lock we hold, so the arithmetic is exact.
    let buffered = buf.samples();
    let offset_secs =
        (lane.samples_sent.load(Ordering::Acquire) as f64 - buffered as f64) / SOURCE_SAMPLE_RATE;
    for chunk in buf.take() {
        // Bounded like every send under this lock. A session whose first
        // sends fail (or stall) is dead on arrival; install it anyway —
        // its own failure report, or the bridge retiring it on the next
        // live chunk, drives the fallback.
        match tokio::time::timeout(BRIDGE_SEND_TIMEOUT, session.send_audio(&chunk)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                tracing::warn!("buffered audio not delivered to the new session: {e}");
                break;
            }
            Err(_) => {
                tracing::warn!(
                    "buffered audio not delivered to the new session: one send sat for {} seconds",
                    BRIDGE_SEND_TIMEOUT.as_secs()
                );
                break;
            }
        }
    }
    seen_labels
        .lock()
        .expect("live speaker labels poisoned")
        .clear();
    lane.stream_is_cloud.store(is_cloud, Ordering::Release);
    liveness_clock.store(crate::epoch_ms(), Ordering::Release);
    *guard = SessionSlot::Streaming(session);
    drop(guard);
    spawn_pump(stream_rx, event_tx, offset_secs, generation, lane.clone());
    true
}

/// Carry one stream's events onto the recording's channel, shifted onto
/// the recording clock. Finalized `Segment`s always land — a stream being
/// drained after a pause still owes its tail. `Interim`s and `Failed`
/// stop once the stream's generation is stale: a retired stream's preview
/// must not overwrite the live one, and its death throes must not raise
/// banners or trigger the fallback machinery. `Done` ends the pump and
/// never reaches the forwarder — one stream ending is not the recording's
/// transcription ending.
fn spawn_pump(
    mut stream_rx: mpsc::UnboundedReceiver<TranscriptionEvent>,
    out_tx: mpsc::UnboundedSender<TranscriptionEvent>,
    offset_secs: f64,
    generation: u64,
    lane: Arc<StreamLane>,
) {
    tokio::spawn(async move {
        while let Some(event) = stream_rx.recv().await {
            let current = lane.current_generation() == generation;
            let sent = match event {
                TranscriptionEvent::Interim {
                    mut segment,
                    tentative,
                } => {
                    if !current {
                        continue;
                    }
                    segment.start += offset_secs;
                    segment.end += offset_secs;
                    out_tx.send(TranscriptionEvent::Interim { segment, tentative })
                }
                TranscriptionEvent::Segment(mut seg) => {
                    seg.start += offset_secs;
                    seg.end += offset_secs;
                    out_tx.send(TranscriptionEvent::Segment(seg))
                }
                TranscriptionEvent::Failed { message } => {
                    if !current {
                        tracing::info!("retired stream failed after handover: {message}");
                        continue;
                    }
                    out_tx.send(TranscriptionEvent::Failed { message })
                }
                TranscriptionEvent::Done => break,
            };
            if sent.is_err() {
                break; // the recording's channel closed under us
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use embral_types::TranscriptionSegment;
    use std::collections::HashSet;
    use std::sync::atomic::AtomicUsize;

    struct StubSession {
        finished: Arc<AtomicBool>,
        chunks_received: Arc<AtomicUsize>,
    }

    impl StubSession {
        fn new() -> (Box<Self>, Arc<AtomicBool>, Arc<AtomicUsize>) {
            let finished = Arc::new(AtomicBool::new(false));
            let chunks = Arc::new(AtomicUsize::new(0));
            (
                Box::new(Self {
                    finished: finished.clone(),
                    chunks_received: chunks.clone(),
                }),
                finished,
                chunks,
            )
        }
    }

    #[async_trait::async_trait]
    impl TranscriptionSession for StubSession {
        async fn send_audio(&self, _pcm_f32: &[f32]) -> anyhow::Result<()> {
            self.chunks_received.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn finish(self: Box<Self>) -> anyhow::Result<Vec<TranscriptionSegment>> {
            self.finished.store(true, Ordering::SeqCst);
            Ok(Vec::new())
        }
    }

    fn seg(text: &str, start: f64, end: f64) -> TranscriptionSegment {
        TranscriptionSegment {
            speaker: None,
            speaker_id: None,
            text: text.to_string(),
            start,
            end,
        }
    }

    async fn assert_becomes_true(flag: &Arc<AtomicBool>) {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !flag.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("flag never set");
    }

    #[test]
    fn the_buffer_caps_by_dropping_the_oldest() {
        let mut buf = AudioBuffer::default();
        // 70 one-second chunks into a 60 s cap.
        for i in 0..70 {
            buf.push(vec![i as f32; 16_000]);
        }
        assert!(buf.samples() <= MAX_BUFFERED_SAMPLES);
        let kept = buf.take();
        // The oldest chunks went; the newest survived.
        assert_eq!(kept.front().expect("has chunks")[0], 10.0);
        assert_eq!(kept.back().expect("has chunks")[0], 69.0);
        assert_eq!(buf.samples(), 0);
    }

    #[tokio::test]
    async fn install_delivers_the_buffered_audio_then_streams() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let lane = Arc::new(StreamLane::new(tx));
        // Three seconds captured; one of them waited in the buffer.
        lane.samples_sent.store(3 * 16_000, Ordering::Release);
        let mut buf = AudioBuffer::default();
        buf.push(vec![0.0; 16_000]);
        let slot: SharedSlot = Arc::new(Mutex::new(SessionSlot::Buffering(buf)));
        let labels = std::sync::Mutex::new(HashSet::from(["Speaker 1".to_string()]));

        let (session, _, chunks) = StubSession::new();
        let (_stream_tx, stream_rx) = mpsc::unbounded_channel();
        let clock = AtomicU64::new(0);
        let installed = install_stream(
            &lane,
            &slot,
            &labels,
            &clock,
            session,
            stream_rx,
            true,
            lane.current_generation(),
        )
        .await;

        assert!(installed);
        assert_eq!(chunks.load(Ordering::SeqCst), 1);
        assert!(matches!(&*slot.lock().await, SessionSlot::Streaming(_)));
        assert!(lane.stream_is_cloud.load(Ordering::Acquire));
        // A fresh clustering run: the guard's per-stream count restarts.
        assert!(labels.lock().unwrap().is_empty());
        // The quiet before this install had no transcriber; the check-in's
        // clock restarts rather than billing it as silence.
        assert!(clock.load(Ordering::Acquire) > 0);
    }

    #[tokio::test]
    async fn a_superseded_open_retires_its_session_and_leaves_the_buffer() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let lane = Arc::new(StreamLane::new(tx));
        let mut buf = AudioBuffer::default();
        buf.push(vec![0.0; 100]);
        let slot: SharedSlot = Arc::new(Mutex::new(SessionSlot::Buffering(buf)));
        let labels = std::sync::Mutex::new(HashSet::new());

        let gen = lane.current_generation();
        lane.bump_generation(); // pause or stop moved the recording on
        let (session, finished, chunks) = StubSession::new();
        let (_stream_tx, stream_rx) = mpsc::unbounded_channel();
        let clock = AtomicU64::new(0);
        let installed =
            install_stream(&lane, &slot, &labels, &clock, session, stream_rx, true, gen).await;

        assert!(!installed);
        assert_becomes_true(&finished).await;
        assert_eq!(chunks.load(Ordering::SeqCst), 0);
        // Nothing was installed, so the check-in's clock stays put.
        assert_eq!(clock.load(Ordering::Acquire), 0);
        // The buffered audio still waits for the open that IS current.
        match &*slot.lock().await {
            SessionSlot::Buffering(buf) => assert_eq!(buf.samples(), 100),
            _ => panic!("slot should still be buffering"),
        };
    }

    #[tokio::test]
    async fn a_taken_slot_refuses_a_second_install() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let lane = Arc::new(StreamLane::new(tx));
        let (first, first_finished, _) = StubSession::new();
        let slot: SharedSlot = Arc::new(Mutex::new(SessionSlot::Streaming(first)));
        let labels = std::sync::Mutex::new(HashSet::new());

        let (second, second_finished, _) = StubSession::new();
        let (_stream_tx, stream_rx) = mpsc::unbounded_channel();
        let clock = AtomicU64::new(0);
        let installed = install_stream(
            &lane,
            &slot,
            &labels,
            &clock,
            second,
            stream_rx,
            true,
            lane.current_generation(),
        )
        .await;

        assert!(!installed);
        assert_becomes_true(&second_finished).await;
        assert_eq!(clock.load(Ordering::Acquire), 0);
        // The stream that won the slot is untouched.
        assert!(!first_finished.load(Ordering::SeqCst));
        assert!(matches!(&*slot.lock().await, SessionSlot::Streaming(_)));
    }

    #[tokio::test]
    async fn after_stop_took_the_channel_an_open_retires_itself() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let lane = Arc::new(StreamLane::new(tx));
        lane.take_event_tx();
        let slot: SharedSlot = Arc::new(Mutex::new(SessionSlot::Buffering(AudioBuffer::default())));
        let labels = std::sync::Mutex::new(HashSet::new());

        let (session, finished, _) = StubSession::new();
        let (_stream_tx, stream_rx) = mpsc::unbounded_channel();
        let clock = AtomicU64::new(0);
        let installed = install_stream(
            &lane,
            &slot,
            &labels,
            &clock,
            session,
            stream_rx,
            true,
            lane.current_generation(),
        )
        .await;

        assert!(!installed);
        assert_becomes_true(&finished).await;
        assert_eq!(clock.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn pausing_a_cloud_stream_finishes_it_and_buffers() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let lane = Arc::new(StreamLane::new(tx));
        lane.stream_is_cloud.store(true, Ordering::Release);
        let (session, finished, _) = StubSession::new();
        let slot: SharedSlot = Arc::new(Mutex::new(SessionSlot::Streaming(session)));
        let before = lane.current_generation();

        pause_stream(&lane, &slot).await;

        assert_eq!(lane.current_generation(), before + 1);
        assert!(matches!(&*slot.lock().await, SessionSlot::Buffering(_)));
        assert_becomes_true(&finished).await;
    }

    #[tokio::test]
    async fn pausing_a_local_stream_leaves_it_alone() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let lane = Arc::new(StreamLane::new(tx));
        let (session, finished, _) = StubSession::new();
        let slot: SharedSlot = Arc::new(Mutex::new(SessionSlot::Streaming(session)));
        let before = lane.current_generation();

        pause_stream(&lane, &slot).await;

        assert_eq!(lane.current_generation(), before);
        assert!(matches!(&*slot.lock().await, SessionSlot::Streaming(_)));
        assert!(!finished.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn pausing_during_a_pending_open_retires_the_open_on_arrival() {
        // Pause lands while the cloud open is still in flight: the slot
        // keeps buffering, the generation moves on, and the open retires
        // its stream when it finally arrives instead of installing a
        // live, metering stream into a paused recording.
        let (tx, _rx) = mpsc::unbounded_channel();
        let lane = Arc::new(StreamLane::new(tx));
        lane.stream_is_cloud.store(true, Ordering::Release);
        let slot: SharedSlot =
            Arc::new(Mutex::new(SessionSlot::Buffering(AudioBuffer::default())));
        let labels = std::sync::Mutex::new(HashSet::new());
        let gen_at_spawn = lane.current_generation();

        pause_stream(&lane, &slot).await;
        assert!(matches!(&*slot.lock().await, SessionSlot::Buffering(_)));

        let (session, finished, _) = StubSession::new();
        let (_stream_tx, stream_rx) = mpsc::unbounded_channel();
        let clock = AtomicU64::new(0);
        let installed = install_stream(
            &lane,
            &slot,
            &labels,
            &clock,
            session,
            stream_rx,
            true,
            gen_at_spawn,
        )
        .await;
        assert!(!installed);
        assert_becomes_true(&finished).await;
        assert_eq!(clock.load(Ordering::Acquire), 0);
    }

    /// A session whose sends never resolve — the wedged socket of the
    /// 2026-08 stop hang.
    struct WedgedSession;

    #[async_trait::async_trait]
    impl TranscriptionSession for WedgedSession {
        async fn send_audio(&self, _pcm_f32: &[f32]) -> anyhow::Result<()> {
            std::future::pending().await
        }
        async fn finish(self: Box<Self>) -> anyhow::Result<Vec<TranscriptionSegment>> {
            Ok(Vec::new())
        }
    }

    /// A session whose sends fail outright.
    struct DeadSession;

    #[async_trait::async_trait]
    impl TranscriptionSession for DeadSession {
        async fn send_audio(&self, _pcm_f32: &[f32]) -> anyhow::Result<()> {
            Err(anyhow::anyhow!("the socket is gone"))
        }
        async fn finish(self: Box<Self>) -> anyhow::Result<Vec<TranscriptionSegment>> {
            Ok(Vec::new())
        }
    }

    #[tokio::test]
    async fn a_live_session_takes_the_chunk() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let lane = Arc::new(StreamLane::new(tx));
        let (session, _, chunks) = StubSession::new();
        let slot: SharedSlot = Arc::new(Mutex::new(SessionSlot::Streaming(session)));

        let outcome = deliver_chunk(&lane, &slot, vec![0.0; 160]).await;

        assert!(matches!(outcome, Delivered::Sent));
        assert_eq!(chunks.load(Ordering::SeqCst), 1);
        assert_eq!(lane.samples_sent.load(Ordering::Acquire), 160);
    }

    #[tokio::test(start_paused = true)]
    async fn a_stalled_send_hands_the_session_back_and_keeps_the_chunk() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let lane = Arc::new(StreamLane::new(tx));
        let slot: SharedSlot =
            Arc::new(Mutex::new(SessionSlot::Streaming(Box::new(WedgedSession))));

        let outcome = deliver_chunk(&lane, &slot, vec![0.0; 160]).await;

        let Delivered::Stalled { why, .. } = outcome else {
            panic!("expected a stall");
        };
        assert!(why.contains("seconds"), "unexpected reason: {why}");
        // The chunk waits for whatever session comes next instead of
        // vanishing…
        match &*slot.lock().await {
            SessionSlot::Buffering(buf) => assert_eq!(buf.samples(), 160),
            _ => panic!("slot should be buffering after a stall"),
        }
        // …and it stayed on the stream clock.
        assert_eq!(lane.samples_sent.load(Ordering::Acquire), 160);
    }

    #[tokio::test(start_paused = true)]
    async fn after_a_stall_the_next_chunk_buffers_without_a_session() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let lane = Arc::new(StreamLane::new(tx));
        let slot: SharedSlot =
            Arc::new(Mutex::new(SessionSlot::Streaming(Box::new(WedgedSession))));

        let first = deliver_chunk(&lane, &slot, vec![0.0; 100]).await;
        assert!(matches!(first, Delivered::Stalled { .. }));
        // The corpse is out of the slot, so the stall cannot repeat.
        let second = deliver_chunk(&lane, &slot, vec![0.0; 100]).await;
        assert!(matches!(second, Delivered::Buffered));
        match &*slot.lock().await {
            SessionSlot::Buffering(buf) => assert_eq!(buf.samples(), 200),
            _ => panic!("slot should still be buffering"),
        };
    }

    #[tokio::test]
    async fn a_send_error_hands_the_session_back() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let lane = Arc::new(StreamLane::new(tx));
        let slot: SharedSlot =
            Arc::new(Mutex::new(SessionSlot::Streaming(Box::new(DeadSession))));

        let outcome = deliver_chunk(&lane, &slot, vec![0.0; 100]).await;

        let Delivered::Stalled { why, .. } = outcome else {
            panic!("expected a stall");
        };
        assert!(why.contains("the socket is gone"), "unexpected reason: {why}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_wedged_session_still_installs_after_a_bounded_drain() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let lane = Arc::new(StreamLane::new(tx));
        let mut buf = AudioBuffer::default();
        buf.push(vec![0.0; 16_000]);
        buf.push(vec![0.0; 16_000]);
        let slot: SharedSlot = Arc::new(Mutex::new(SessionSlot::Buffering(buf)));
        let labels = std::sync::Mutex::new(HashSet::new());

        let (_stream_tx, stream_rx) = mpsc::unbounded_channel();
        let clock = AtomicU64::new(0);
        let installed = install_stream(
            &lane,
            &slot,
            &labels,
            &clock,
            Box::new(WedgedSession),
            stream_rx,
            true,
            lane.current_generation(),
        )
        .await;

        // The drain gave up within its deadline; the session installs
        // anyway, and the bridge's backstop retires it on the next live
        // chunk.
        assert!(installed);
        assert!(matches!(&*slot.lock().await, SessionSlot::Streaming(_)));
        assert!(clock.load(Ordering::Acquire) > 0);
    }

    /// A session that sends fine but whose finish never returns.
    struct WedgedFinishSession;

    #[async_trait::async_trait]
    impl TranscriptionSession for WedgedFinishSession {
        async fn send_audio(&self, _pcm_f32: &[f32]) -> anyhow::Result<()> {
            Ok(())
        }
        async fn finish(self: Box<Self>) -> anyhow::Result<Vec<TranscriptionSegment>> {
            std::future::pending().await
        }
    }

    #[tokio::test]
    async fn stop_finishes_a_live_session() {
        let (session, finished, _) = StubSession::new();
        let slot: SharedSlot = Arc::new(Mutex::new(SessionSlot::Streaming(session)));

        let outcome =
            finish_current_stream(&slot, std::time::Duration::from_secs(2)).await;

        assert_eq!(outcome, FinishOutcome::Finished);
        assert!(finished.load(Ordering::SeqCst));
        assert!(matches!(&*slot.lock().await, SessionSlot::Off));
    }

    #[tokio::test(start_paused = true)]
    async fn a_wedged_finish_times_out_and_stop_proceeds() {
        let slot: SharedSlot = Arc::new(Mutex::new(SessionSlot::Streaming(Box::new(
            WedgedFinishSession,
        ))));

        let outcome =
            finish_current_stream(&slot, std::time::Duration::from_secs(2)).await;

        assert_eq!(outcome, FinishOutcome::FinishTimedOut);
        assert!(matches!(&*slot.lock().await, SessionSlot::Off));
    }

    #[tokio::test(start_paused = true)]
    async fn a_held_slot_never_blocks_stop() {
        let (session, finished, _) = StubSession::new();
        let slot: SharedSlot = Arc::new(Mutex::new(SessionSlot::Streaming(session)));
        // Someone is sitting on the slot lock — the wedged-bridge shape.
        let held = slot.clone().lock_owned().await;

        let outcome =
            finish_current_stream(&slot, std::time::Duration::from_secs(2)).await;
        assert_eq!(outcome, FinishOutcome::SlotHeld);
        assert!(!finished.load(Ordering::SeqCst), "nothing finished while held");

        // The hold clears; the detached reaper closes the session out.
        drop(held);
        assert_becomes_true(&finished).await;
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if matches!(&*slot.lock().await, SessionSlot::Off) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the reaper turned the slot off");
    }

    #[tokio::test]
    async fn a_pending_open_tail_is_counted() {
        let mut buf = AudioBuffer::default();
        buf.push(vec![0.0; 100]);
        let slot: SharedSlot = Arc::new(Mutex::new(SessionSlot::Buffering(buf)));

        let outcome =
            finish_current_stream(&slot, std::time::Duration::from_secs(2)).await;

        assert_eq!(outcome, FinishOutcome::PendingOpen(100));
        assert!(matches!(&*slot.lock().await, SessionSlot::Off));
    }

    #[tokio::test]
    async fn the_pump_shifts_onto_the_recording_clock() {
        let (out_tx, mut out_rx) = mpsc::unbounded_channel();
        let (stream_tx, stream_rx) = mpsc::unbounded_channel();
        let (tx, _rx) = mpsc::unbounded_channel();
        let lane = Arc::new(StreamLane::new(tx));
        spawn_pump(stream_rx, out_tx, 100.0, lane.current_generation(), lane.clone());

        stream_tx
            .send(TranscriptionEvent::Segment(seg("hello", 1.0, 2.0)))
            .unwrap();
        match out_rx.recv().await.expect("segment lands") {
            TranscriptionEvent::Segment(s) => {
                assert_eq!(s.start, 101.0);
                assert_eq!(s.end, 102.0);
            }
            other => panic!("expected a segment, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_retired_stream_still_lands_segments_but_nothing_else() {
        let (out_tx, mut out_rx) = mpsc::unbounded_channel();
        let (stream_tx, stream_rx) = mpsc::unbounded_channel();
        let (tx, _rx) = mpsc::unbounded_channel();
        let lane = Arc::new(StreamLane::new(tx));
        spawn_pump(stream_rx, out_tx, 0.0, lane.current_generation(), lane.clone());
        lane.bump_generation(); // the recording moved on (pause)

        // The drain's tail segment must land; its interim and its failure
        // must not.
        stream_tx
            .send(TranscriptionEvent::Interim {
                segment: seg("stale preview", 0.0, 1.0),
                tentative: None,
            })
            .unwrap();
        stream_tx
            .send(TranscriptionEvent::Failed {
                message: "death throes".into(),
            })
            .unwrap();
        stream_tx
            .send(TranscriptionEvent::Segment(seg("the tail", 0.0, 1.0)))
            .unwrap();
        drop(stream_tx);

        match out_rx.recv().await.expect("the tail lands") {
            TranscriptionEvent::Segment(s) => assert_eq!(s.text, "the tail"),
            other => panic!("expected the tail segment, got {other:?}"),
        }
        assert!(out_rx.recv().await.is_none(), "nothing else forwarded");
    }

    #[tokio::test]
    async fn a_current_stream_forwards_its_failure_and_swallows_done() {
        let (out_tx, mut out_rx) = mpsc::unbounded_channel();
        let (stream_tx, stream_rx) = mpsc::unbounded_channel();
        let (tx, _rx) = mpsc::unbounded_channel();
        let lane = Arc::new(StreamLane::new(tx));
        spawn_pump(stream_rx, out_tx, 0.0, lane.current_generation(), lane.clone());

        stream_tx
            .send(TranscriptionEvent::Failed {
                message: "hours used up".into(),
            })
            .unwrap();
        stream_tx.send(TranscriptionEvent::Done).unwrap();
        drop(stream_tx);

        match out_rx.recv().await.expect("failure reaches the forwarder") {
            TranscriptionEvent::Failed { message } => assert_eq!(message, "hours used up"),
            other => panic!("expected the failure, got {other:?}"),
        }
        assert!(out_rx.recv().await.is_none(), "Done never leaves the pump");
    }
}
