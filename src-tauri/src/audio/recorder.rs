use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SampleRate, StreamConfig};
use hound::{WavSpec, WavWriter};
use std::collections::VecDeque;
use std::io::BufWriter;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use super::meter::LevelTap;
use super::pipeline::{Pipeline, TARGET_SAMPLE_RATE};

/// How far the loopback (system-audio) buffer may run ahead of the mic before
/// the oldest samples are dropped. The mic is the master clock; the loopback
/// stream fills a buffer that the mic drains and mixes. Bounds added latency
/// and unbounded growth from clock drift between the two capture devices;
/// fires rarely at realistic drift, and the discontinuity when it does is small
/// and bounded.
const MAX_LOOPBACK_LAG_SECS: usize = 2;

/// How often the WAV header is rewritten mid-recording so the file on disk
/// stays playable ([recording.md] §Crash recovery). This is the ceiling on
/// audio a crash can strand: the samples are already written, but a header
/// that predates them says the file is empty.
const FLUSH_EVERY: std::time::Duration = std::time::Duration::from_secs(5);

/// How long stopping waits for the capture thread. Teardown is normally
/// milliseconds; the wait exists because the thread's exit ends by
/// dropping OS audio streams, and a wedged driver can park that call
/// indefinitely, which must cost a leaked thread, not a stop that never
/// returns ([recording.md] §Dual-stream capture).
const CAPTURE_JOIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Join `thread`, giving up after `deadline`. `true` means it exited. On
/// `false` the thread is left running detached (along with the helper
/// waiting on it); the caller carries on, and the leak is the accepted
/// cost of never hanging on a wedged OS call.
fn join_with_deadline(thread: std::thread::JoinHandle<()>, deadline: std::time::Duration) -> bool {
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let waiter = std::thread::Builder::new()
        .name("capture-join".into())
        .spawn(move || {
            let _ = thread.join();
            let _ = done_tx.send(());
        });
    match waiter {
        Ok(_) => done_rx.recv_timeout(deadline).is_ok(),
        // The helper failed to spawn (resource exhaustion); its closure,
        // and the join handle inside it, are gone, so the capture thread
        // is left detached exactly as on a timeout.
        Err(e) => {
            tracing::warn!("no thread for the bounded join ({e}); capture left detached");
            false
        }
    }
}

/// One secondary source's buffer: resampled 16 kHz mono samples waiting for
/// the master clock to drain them.
type Ring = Arc<Mutex<VecDeque<f32>>>;

/// Every secondary source currently feeding the mix: output endpoints, per
/// app captures, extra microphones. Sources join and leave during a
/// recording (the source picker), so this is a live registry rather than a
/// fixed pair: whoever holds it can add a ring, and dropping a source's
/// handle removes it.
#[derive(Clone, Default)]
pub struct SourceMix {
    rings: Arc<Mutex<Vec<Ring>>>,
}

impl SourceMix {
    /// Register a source and hand back its ring to push into.
    pub fn add(&self) -> Ring {
        let ring: Ring = Arc::new(Mutex::new(VecDeque::new()));
        self.rings.lock().unwrap().push(ring.clone());
        ring
    }

    /// Unregister a source (its remaining samples are dropped with it).
    fn remove(&self, ring: &Ring) {
        self.rings
            .lock()
            .unwrap()
            .retain(|r| !Arc::ptr_eq(r, ring));
    }

    /// Register a source and hand back a handle that unregisters itself.
    fn join(&self) -> Arc<Membership> {
        Arc::new(Membership {
            mix: self.clone(),
            ring: self.add(),
        })
    }

    /// Drain up to `len` samples from every source and sum them. Each ring
    /// contributes what it has: a source that is silent, just added, or
    /// mid-reopen contributes less, so one source stalling never
    /// silences the others.
    fn drain_sum(&self, len: usize) -> Vec<f32> {
        let mut mixed = vec![0.0f32; len];
        for ring in self.rings.lock().unwrap().iter() {
            let mut buf = ring.lock().unwrap();
            let take = len.min(buf.len());
            for (slot, s) in mixed.iter_mut().zip(buf.drain(..take)) {
                *slot += s;
            }
        }
        mixed
    }
}

/// One secondary source's place in the mix, for as long as its capture
/// lives. Dropping it (with the stream whose sink owns it) takes the ring
/// back out; without this, every reopened endpoint left a dead ring behind
/// for the mic callback to lock on every single block.
struct Membership {
    mix: SourceMix,
    ring: Ring,
}

impl Drop for Membership {
    fn drop(&mut self) {
        self.mix.remove(&self.ring);
    }
}

/// What a capture stream does with each block of resampled 16 kHz mono output.
///
/// Capture streams run on independent clocks and fire callbacks at unrelated
/// times, so we can't just append them into one file; doing so interleaves
/// ~64 ms chunks and yields a doubled-length, garbled recording. Instead the
/// primary mic acts as the master clock: input devices deliver callbacks
/// continuously (silence samples included), whereas system-audio capture goes
/// quiet (no callbacks) when nothing is playing.
#[derive(Clone)]
enum MixSink {
    /// Mic, the master clock. Owns the WAV writer and the transcription
    /// channel. For each resampled block it drains an equal number of samples
    /// from every secondary source (silence-padded when a source is short),
    /// sums them, and writes the single mixed stream out.
    Primary {
        wav_writer: Arc<Mutex<Option<WavWriter<BufWriter<std::fs::File>>>>>,
        audio_tx: Option<tokio::sync::mpsc::UnboundedSender<Vec<f32>>>,
        sources: SourceMix,
        /// Live ~10 Hz spectrum tap for the recording view's meter.
        level: Option<Arc<Mutex<LevelTap>>>,
    },
    /// A secondary source; pushes resampled samples into its own ring for
    /// the primary to drain. Produces no WAV / transcription output itself.
    /// Holding the membership is what keeps the ring registered: when this
    /// sink dies with its stream, the ring leaves the mix.
    Secondary { source: Arc<Membership> },
    /// Stream blocks straight into a channel: dictation's mic-only live
    /// path (no WAV, no loopback). Dropping the stream drops this sender,
    /// which is how the consumer learns the capture ended.
    Tx {
        tx: tokio::sync::mpsc::UnboundedSender<Vec<f32>>,
    },
}

impl MixSink {
    /// Consume one block of this stream's resampled 16 kHz mono samples.
    fn consume(&self, resampled: &[f32]) {
        match self {
            MixSink::Tx { tx } => {
                let _ = tx.send(resampled.to_vec());
            }
            MixSink::Secondary { source } => {
                let mut buf = source.ring.lock().unwrap();
                buf.extend(resampled.iter().copied());
                // Cap latency / unbounded growth from device-clock drift.
                let cap = TARGET_SAMPLE_RATE as usize * MAX_LOOPBACK_LAG_SECS;
                if buf.len() > cap {
                    let excess = buf.len() - cap;
                    buf.drain(..excess);
                }
            }
            MixSink::Primary {
                wav_writer,
                audio_tx,
                sources,
                level,
            } => {
                // Mix in every secondary source aligned sample-for-sample.
                // A source with nothing buffered (silent, absent, or
                // mid-reopen) contributes silence, so the mix degrades to
                // whatever is actually producing audio.
                let others = sources.drain_sum(resampled.len());
                if let Some(level) = level {
                    if let Ok(mut tap) = level.lock() {
                        tap.push_block(resampled, &others);
                    }
                }
                let mut mixed = resampled.to_vec();
                for (slot, other) in mixed.iter_mut().zip(others) {
                    // Sum-and-clamp: keeps each source at full volume (only
                    // one is usually active), hard-clipping the rare moment
                    // several peak together rather than halving everything.
                    *slot = (*slot + other).clamp(-1.0, 1.0);
                }

                if let Ok(mut guard) = wav_writer.lock() {
                    if let Some(w) = guard.as_mut() {
                        for &s in &mixed {
                            let _ = w.write_sample(s);
                        }
                    }
                }
                if let Some(tx) = audio_tx {
                    if let Err(e) = tx.send(mixed) {
                        tracing::error!("mix audio_tx send failed (channel closed?): {}", e);
                    }
                }
            }
        }
    }
}

/// A meeting recording's capture: mic + system audio mixed to one 16 kHz
/// WAV and (optionally) a transcription channel.
///
/// `cpal::Stream` is `!Send` on every platform, so a dedicated capture
/// thread builds and owns the streams; this façade holds the thread's join
/// handle and a stop channel and is genuinely `Send`, no `unsafe impl`.
pub struct Recorder {
    paused: Arc<AtomicBool>,
    wav_path: PathBuf,
    wav_writer: Arc<Mutex<Option<WavWriter<BufWriter<std::fs::File>>>>>,
    /// Wakes the system-audio capture when the source selection changes, so a
    /// checkbox applies at once instead of on the next supervision tick.
    /// Closing it is also how that capture learns to stop, so `shutdown`
    /// must drop it rather than leave it to the struct's own drop.
    reconfigure_tx: Option<std::sync::mpsc::Sender<crate::platform::types::CaptureCommand>>,
    /// The same, for the extra-microphone supervisor, which is the loop the
    /// capture thread parks on, so this sender is the stop signal.
    mic_reconfigure_tx: Option<std::sync::mpsc::Sender<crate::platform::types::CaptureCommand>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Recorder {
    pub fn start(
        wav_path: PathBuf,
        audio_tx: Option<tokio::sync::mpsc::UnboundedSender<Vec<f32>>>,
        mic_device: Option<&str>,
        output_device: Option<&str>,
        level_cb: Option<Box<dyn Fn(&[f32], &[f32]) + Send>>,
        // The source picker's live choices, read whenever a capture rebuilds.
        wanted: Box<dyn Fn() -> crate::platform::types::SystemAudioWanted + Send>,
        extra_mics: Box<dyn Fn() -> Vec<String> + Send>,
    ) -> Result<Self> {
        let paused = Arc::new(AtomicBool::new(false));

        let spec = WavSpec {
            channels: 1,
            sample_rate: TARGET_SAMPLE_RATE,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };

        if let Some(parent) = wav_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let writer = WavWriter::create(&wav_path, spec)?;
        let wav_writer = Arc::new(Mutex::new(Some(writer)));

        // Every secondary source feeds this registry; the mic stream (master
        // clock) drains and sums it. Sources join and leave live.
        let sources = SourceMix::default();

        let mic_sink = MixSink::Primary {
            wav_writer: wav_writer.clone(),
            audio_tx,
            sources: sources.clone(),
            level: level_cb.map(|cb| Arc::new(Mutex::new(LevelTap::new(cb)))),
        };

        let mic_device = mic_device.map(str::to_owned);
        let output_device = output_device.map(str::to_owned);
        let thread_paused = paused.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<()>>();
        // Created here so the façade can wake the capture; the receiver moves
        // into the capture thread below.
        let (sys_stop_tx, sys_stop_rx) =
            std::sync::mpsc::channel::<crate::platform::types::CaptureCommand>();
        let reconfigure_tx = sys_stop_tx.clone();
        // The mic supervisor's own wake channel: closing it (with the
        // recorder's stop) ends the loop and drops every extra stream.
        let (mic_cmd_tx, mic_cmd_rx) =
            std::sync::mpsc::channel::<crate::platform::types::CaptureCommand>();
        let mic_sources = sources.clone();
        let thread_paused_mics = paused.clone();
        let flush_writer = wav_writer.clone();

        let thread = std::thread::Builder::new()
            .name("audio-capture".into())
            .spawn(move || {
                // Build on this thread; the streams never leave it.
                // Bound (not `_`) so the stream lives until the thread exits.
                let _mic_stream = match build_mic_stream(
                    mic_device.as_deref(),
                    thread_paused.clone(),
                    mic_sink,
                    "mic",
                ) {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                tracing::info!("Mic input stream started");

                // Ready on the mic alone: recording must start instantly.
                // The system-audio capture lives on its own detached
                // thread: on macOS its first use can block on the OS
                // consent prompt for as long as the user takes to answer, and
                // neither recording start nor stop may wait on that. The
                // mixer runs mic-only until the loopback buffer fills; if
                // the recorder stops while consent is still pending, the
                // capture tears itself down the moment creation resolves.
                let _ = ready_tx.send(Ok(()));

                let spawned = std::thread::Builder::new()
                    .name("system-audio".into())
                    .spawn(move || {
                        // The platform owns the capture's whole lifecycle
                        // (open, supervise, reopen) on this thread; the
                        // capture handles are !Send. Every capture it opens
                        // asks for its own ring, so sources sum rather than
                        // interleave and one closing never disturbs another.
                        let sink_factory: crate::platform::types::SystemAudioSinkFactory =
                            Box::new(move || {
                                let sink = MixSink::Secondary {
                                    source: sources.join(),
                                };
                                Box::new(move |block: &[f32]| sink.consume(block))
                            });
                        crate::platform::SystemAudioCapture::run(
                            sink_factory,
                            thread_paused,
                            output_device.as_deref(),
                            wanted,
                            sys_stop_rx,
                            // Every (re)open says what it is capturing. This
                            // is the log's line, not the UI's: the source
                            // picker already shows the choice on screen, and
                            // the log is where a wrong source gets diagnosed
                            // after the fact.
                            Box::new(|source| tracing::info!(?source, "system-audio source")),
                        );
                    });
                if spawned.is_err() {
                    tracing::warn!("system-audio thread failed to spawn — recording mic only");
                }

                // Extra microphones join and leave while recording; the
                // primary mic above stays put because it owns the master
                // clock. Each extra gets its own ring, exactly like an
                // output endpoint, so they sum rather than interleave.
                let mut extras: Vec<(String, cpal::Stream)> = Vec::new();
                loop {
                    let wanted_mics = extra_mics();
                    extras.retain(|(name, _)| wanted_mics.iter().any(|w| w == name));
                    for name in &wanted_mics {
                        if extras.iter().any(|(open, _)| open == name) {
                            continue;
                        }
                        let sink = MixSink::Secondary {
                            source: mic_sources.join(),
                        };
                        match build_mic_stream(
                            Some(name),
                            thread_paused_mics.clone(),
                            sink,
                            "extra-mic",
                        ) {
                            Ok(stream) => {
                                tracing::info!(device = %name, "extra microphone added");
                                extras.push((name.clone(), stream));
                            }
                            Err(e) => {
                                tracing::warn!(device = %name, "extra microphone unavailable: {e}")
                            }
                        }
                    }
                    // Woken by a selection change, the flush tick, or the
                    // recorder stopping.
                    match mic_cmd_rx.recv_timeout(FLUSH_EVERY) {
                        Ok(_) => continue,
                        // Nothing to reconfigure: rewrite the WAV header so
                        // the file on disk is playable from end to end. A
                        // crash otherwise leaves a header claiming no
                        // samples over a full meeting of audio; this bounds
                        // what recovery can lose to one interval.
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                            if let Ok(mut guard) = flush_writer.lock() {
                                if let Some(w) = guard.as_mut() {
                                    if let Err(e) = w.flush() {
                                        tracing::warn!("could not flush the wav header: {e}");
                                    }
                                }
                            }
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
                // Signal the system-audio thread without joining it: it may
                // still be inside the consent-blocked create call.
                drop(sys_stop_tx);
                // mic_stream dropped here, flushing the last callbacks.
            })?;

        match ready_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                let _ = thread.join();
                return Err(e);
            }
            Err(_) => return Err(anyhow!("audio capture thread died during startup")),
        }

        Ok(Self {
            paused,
            wav_path,
            wav_writer,
            reconfigure_tx: Some(reconfigure_tx),
            mic_reconfigure_tx: Some(mic_cmd_tx),
            thread: Some(thread),
        })
    }

    /// The source selection changed: rebuild the system-audio capture now
    /// rather than on its next tick.
    pub fn reconfigure_sources(&self) {
        if let Some(tx) = &self.reconfigure_tx {
            let _ = tx.send(crate::platform::types::CaptureCommand::Reconfigure);
        }
    }

    /// The extra-microphone selection changed.
    pub fn reconfigure_mics(&self) {
        if let Some(tx) = &self.mic_reconfigure_tx {
            let _ = tx.send(crate::platform::types::CaptureCommand::Reconfigure);
        }
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }

    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
    }

    /// Stop every capture and wait (bounded) for the thread that owns
    /// them.
    ///
    /// Dropping the senders before the join is the whole point: the
    /// capture thread parks on the extra-microphone channel, so a join that
    /// still held `mic_reconfigure_tx` could never return; stop hung, the
    /// WAV was never finalized, and the next record press stacked a second
    /// recording on top of the first. The join itself is bounded too: the
    /// thread's exit ends by dropping OS audio streams, and a wedged
    /// driver parking that call must cost a leaked thread (and a mic that
    /// stays busy until the driver lets go), not a stop that never
    /// returns.
    fn shutdown(&mut self) {
        self.mic_reconfigure_tx.take();
        self.reconfigure_tx.take();
        if let Some(thread) = self.thread.take() {
            if !join_with_deadline(thread, CAPTURE_JOIN_TIMEOUT) {
                tracing::error!(
                    "capture thread still running {CAPTURE_JOIN_TIMEOUT:?} after stop — leaking it; the microphone may stay busy until the audio stack lets go"
                );
            }
        }
    }

    /// Stop capturing; returns the finalized WAV's path.
    ///
    /// The WAV finalizes even when the capture thread had to be leaked:
    /// taking the writer out under its lock starves any callback still
    /// running (they skip on `None`), so the header gets its true sample
    /// count and the meeting stays salvageable.
    pub fn stop(mut self) -> Result<PathBuf> {
        self.shutdown();
        if let Ok(mut guard) = self.wav_writer.lock() {
            if let Some(writer) = guard.take() {
                writer.finalize()?;
            }
        }
        Ok(self.wav_path.clone())
    }
}

impl Drop for Recorder {
    /// A dropped-not-stopped recorder (error paths) must not leak the
    /// capture thread; the WAV stays unfinalized, matching the old
    /// stream-drop behavior.
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// A live mic-only 16 kHz stream feeding a channel: dictation's capture.
/// Dropping it stops the stream and closes the channel (the capture thread
/// owns the `!Send` stream; the drop joins it, so the channel is closed by
/// the time drop returns).
pub struct MicStream {
    stop_tx: Option<std::sync::mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl MicStream {
    pub fn start(
        mic_device: Option<&str>,
        tx: tokio::sync::mpsc::UnboundedSender<Vec<f32>>,
    ) -> Result<MicStream> {
        let mic_device = mic_device.map(str::to_owned);
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<()>>();

        let thread = std::thread::Builder::new()
            .name("dictation-capture".into())
            .spawn(move || {
                let stream = match build_mic_stream(
                    mic_device.as_deref(),
                    Arc::new(AtomicBool::new(false)),
                    MixSink::Tx { tx },
                    "dictation",
                ) {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(()));
                let _ = stop_rx.recv();
                drop(stream);
            })?;

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(MicStream {
                stop_tx: Some(stop_tx),
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => Err(anyhow!("dictation capture thread died during startup")),
        }
    }
}

impl Drop for MicStream {
    fn drop(&mut self) {
        self.stop_tx.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Resolve the configured-or-default input device and build its stream.
/// A configured-but-missing device falls back to the default with a warning;
/// an unplugged USB mic must not break recording.
fn build_mic_stream(
    preferred: Option<&str>,
    paused: Arc<AtomicBool>,
    sink: MixSink,
    label: &'static str,
) -> Result<cpal::Stream> {
    let host = cpal::default_host();
    let device = host
        .input_devices()
        .ok()
        .and_then(|devices| find_device(devices, preferred, label))
        .or_else(|| host.default_input_device())
        .ok_or_else(|| anyhow!("No default input device found"))?;
    let name = device.name().unwrap_or_else(|_| "<unknown>".to_string());
    let config = device.default_input_config()?;
    tracing::info!(
        "[{}] input: device='{}', sample_rate={} Hz, channels={}, format={:?}",
        label,
        name,
        config.sample_rate().0,
        config.channels(),
        config.sample_format()
    );
    let stream = build_stream(
        label,
        &device,
        &config,
        paused,
        Box::new(move |b| sink.consume(b)),
        None,
    )?;
    stream.play()?;
    Ok(stream)
}

/// Find a device by name among `devices`, or `None` to use the default.
pub(crate) fn find_device(
    devices: impl Iterator<Item = cpal::Device>,
    preferred: Option<&str>,
    kind: &str,
) -> Option<cpal::Device> {
    let name = preferred?.trim();
    if name.is_empty() {
        return None;
    }
    for device in devices {
        if device.name().map(|n| n == name).unwrap_or(false) {
            tracing::info!("[{}] using configured device '{}'", kind, name);
            return Some(device);
        }
    }
    tracing::warn!(
        "[{}] configured device '{}' not found — falling back to system default",
        kind,
        name
    );
    None
}

/// Build (not play) a cpal input stream that feeds resampled 16 kHz mono
/// blocks to `on_block`. Portable cpal glue: the mic on every platform, and
/// the WASAPI loopback trick, all come through here.
pub(crate) fn build_stream(
    label: &'static str,
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    paused: Arc<AtomicBool>,
    on_block: Box<dyn Fn(&[f32]) + Send>,
    // Set on any stream error, so a supervisor can reopen; errors were
    // once log-only and a dead loopback capture meant silent mic-only.
    dead: Option<Arc<AtomicBool>>,
) -> Result<cpal::Stream> {
    let channels = config.channels() as usize;
    let device_rate = config.sample_rate().0;
    let sample_format = config.sample_format();

    let mut pipeline = Pipeline::new(channels, device_rate)?;
    tracing::info!(
        "[{}] Resampler: {} Hz -> {} Hz, input_frames_per_chunk={}",
        label,
        device_rate,
        TARGET_SAMPLE_RATE,
        pipeline.required_input_frames()
    );
    let callback_count = AtomicUsize::new(0);
    let chunk_count = Arc::new(AtomicUsize::new(0));

    let stream_config = StreamConfig {
        channels: config.channels(),
        sample_rate: SampleRate(device_rate),
        buffer_size: cpal::BufferSize::Default,
    };

    let error_cb = move |e: cpal::StreamError| {
        tracing::error!("[{}] Audio stream error: {}", label, e);
        if let Some(flag) = &dead {
            flag.store(true, Ordering::SeqCst);
        }
    };

    macro_rules! build_typed {
        ($ty:ty, $convert:expr) => {{
            let paused = paused.clone();
            let chunk_count = chunk_count.clone();

            device.build_input_stream(
                &stream_config,
                move |data: &[$ty], _| {
                    let cb_n = callback_count.fetch_add(1, Ordering::Relaxed);
                    if cb_n == 0 {
                        tracing::info!(
                            "[{}] First audio callback fired ({} samples, {} channels)",
                            label,
                            data.len(),
                            channels
                        );
                    }

                    if paused.load(Ordering::SeqCst) {
                        return;
                    }
                    let interleaved: Vec<f32> = data.iter().map(|s| $convert(*s)).collect();
                    let max_amp = interleaved.iter().fold(0.0f32, |m, &s| m.max(s.abs()));

                    pipeline.push(&interleaved, &mut |resampled| {
                        on_block(resampled);
                        let n = chunk_count.fetch_add(1, Ordering::Relaxed);
                        if n == 0 {
                            tracing::info!(
                                "[{}] First resampled chunk produced ({} samples, max_amp={:.4})",
                                label,
                                resampled.len(),
                                max_amp
                            );
                        }
                        // Every ~50 chunks (~3.2s of audio) emit a stats line
                        if (n + 1) % 50 == 0 {
                            tracing::debug!(
                                "[{}] Audio capture stats: {} chunks emitted, latest max_amp={:.4}",
                                label,
                                n + 1,
                                max_amp
                            );
                        }
                    });
                },
                error_cb,
                None,
            )
        }};
    }

    let stream = match sample_format {
        SampleFormat::F32 => build_typed!(f32, |s: f32| s)?,
        SampleFormat::I16 => build_typed!(i16, |s: i16| s as f32 / i16::MAX as f32)?,
        SampleFormat::U16 => build_typed!(u16, |s: u16| (s as f32 / u16::MAX as f32) * 2.0 - 1.0)?,
        SampleFormat::I8 => build_typed!(i8, |s: i8| s as f32 / i8::MAX as f32)?,
        SampleFormat::I32 => build_typed!(i32, |s: i32| s as f32 / i32::MAX as f32)?,
        SampleFormat::F64 => build_typed!(f64, |s: f64| s as f32)?,
        fmt => return Err(anyhow!("Unsupported sample format: {:?}", fmt)),
    };

    Ok(stream)
}

#[cfg(test)]
mod mix_tests {
    use super::*;

    fn push(ring: &Ring, samples: &[f32]) {
        ring.lock().unwrap().extend(samples.iter().copied());
    }

    #[test]
    fn sources_sum_rather_than_interleave() {
        // Two outputs playing at once must add together; appending them
        // would double the recording's length and garble it.
        let mix = SourceMix::default();
        let a = mix.add();
        let b = mix.add();
        push(&a, &[0.1, 0.1, 0.1]);
        push(&b, &[0.2, 0.2, 0.2]);
        let summed = mix.drain_sum(3);
        assert_eq!(summed.len(), 3);
        for s in summed {
            assert!((s - 0.3).abs() < 1e-6, "expected 0.3, got {s}");
        }
    }

    #[test]
    fn a_short_source_pads_with_silence_instead_of_shifting() {
        // A source that just opened (or went quiet) contributes what it
        // has; the rest of the block stays whatever the others produced.
        let mix = SourceMix::default();
        let full = mix.add();
        let short = mix.add();
        push(&full, &[0.5, 0.5, 0.5, 0.5]);
        push(&short, &[0.5]);
        let summed = mix.drain_sum(4);
        assert!((summed[0] - 1.0).abs() < 1e-6);
        assert!((summed[3] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn no_sources_is_silence_not_a_panic() {
        // Mic-only recording: nothing registered, every block still mixes.
        let mix = SourceMix::default();
        assert_eq!(mix.drain_sum(2), vec![0.0, 0.0]);
    }

    #[test]
    fn a_removed_source_stops_contributing() {
        let mix = SourceMix::default();
        let keep = mix.add();
        let drop_me = mix.add();
        push(&keep, &[0.25, 0.25]);
        push(&drop_me, &[0.25, 0.25]);
        mix.remove(&drop_me);
        let summed = mix.drain_sum(2);
        assert!((summed[0] - 0.25).abs() < 1e-6, "only the kept source remains");
    }

    #[test]
    fn a_closed_capture_takes_its_ring_out_of_the_mix() {
        // Endpoints reopen (stream death, device changes) for the whole
        // length of a meeting. Each reopen used to leave its ring behind
        // forever, so the mic callback locked a growing list of dead rings
        // on every block.
        let mix = SourceMix::default();
        let staying = mix.join();
        let closing = mix.join();
        assert_eq!(mix.rings.lock().unwrap().len(), 2);
        push(&closing.ring, &[0.5, 0.5]);
        drop(closing);
        assert_eq!(mix.rings.lock().unwrap().len(), 1, "the ring left with it");
        push(&staying.ring, &[0.25, 0.25]);
        assert_eq!(mix.drain_sum(2), vec![0.25, 0.25]);
    }

    #[test]
    fn a_shared_membership_survives_until_the_last_holder() {
        // The sink is boxed and may be cloned along the way; the ring must
        // leave on the last drop, not the first.
        let mix = SourceMix::default();
        let source = mix.join();
        let second = source.clone();
        drop(source);
        assert_eq!(mix.rings.lock().unwrap().len(), 1);
        drop(second);
        assert!(mix.rings.lock().unwrap().is_empty());
    }

    #[test]
    fn the_bounded_join_waits_for_a_thread_that_exits() {
        let thread = std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(50));
        });
        assert!(join_with_deadline(
            thread,
            std::time::Duration::from_secs(5)
        ));
    }

    #[test]
    fn the_bounded_join_gives_up_on_a_stuck_thread() {
        // A thread that never exits: the hung-driver shape. It leaks
        // with the test process, which is the same bargain the recorder
        // makes.
        let (never_tx, never_rx) = std::sync::mpsc::channel::<()>();
        std::mem::forget(never_tx);
        let thread = std::thread::spawn(move || {
            let _ = never_rx.recv();
        });
        assert!(!join_with_deadline(
            thread,
            std::time::Duration::from_millis(100)
        ));
    }
}

/// Starting and stopping a real recorder. Needs a working input device, so
/// it is `#[ignore]`d like the other hardware probes; run it with
/// `cargo test -p embral -- --ignored recorder_stops`.
#[cfg(test)]
mod hardware_tests {
    use super::*;

    #[test]
    #[ignore = "needs a real input device"]
    fn recorder_stops_instead_of_hanging_forever() {
        // The capture thread parks on the extra-microphone channel, whose
        // only sender lives in the `Recorder`. A `shutdown` that joined
        // before dropping it could never return: stop hung, the WAV was
        // never finalized, and the next record press stacked a second
        // recording on the first.
        let wav = std::env::temp_dir().join("embral-recorder-stop-test.wav");
        let recorder = Recorder::start(
            wav.clone(),
            None,
            None,
            None,
            None,
            Box::new(|| crate::platform::types::SystemAudioWanted::Everything),
            Box::new(Vec::new),
        )
        .expect("recorder starts");

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(recorder.stop());
        });
        let stopped = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("stop returned within 10s");
        assert!(stopped.is_ok(), "stop finalized the wav: {stopped:?}");
        let _ = std::fs::remove_file(&wav);
    }

    #[test]
    #[ignore = "needs a real input device; runs for ~7s"]
    fn the_wav_is_playable_before_the_recording_stops() {
        // Crash resilience ([recording.md] §Crash recovery): the header is
        // rewritten every FLUSH_EVERY, so a recording killed mid-meeting
        // leaves a file that opens and plays instead of one claiming to
        // hold no samples at all.
        let wav = std::env::temp_dir().join("embral-recorder-flush-test.wav");
        let recorder = Recorder::start(
            wav.clone(),
            None,
            None,
            None,
            None,
            Box::new(|| crate::platform::types::SystemAudioWanted::Everything),
            Box::new(Vec::new),
        )
        .expect("recorder starts");

        std::thread::sleep(FLUSH_EVERY + std::time::Duration::from_secs(2));
        let reader = hound::WavReader::open(&wav).expect("the in-flight wav opens");
        let frames = reader.len();
        drop(recorder);
        assert!(
            frames > TARGET_SAMPLE_RATE,
            "the header counted {frames} samples, expected over a second's worth"
        );
        let _ = std::fs::remove_file(&wav);
    }
}
