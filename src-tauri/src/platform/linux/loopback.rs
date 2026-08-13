//! System-audio capture ([recording.md](../../../../docs/recording.md)):
//! a record stream on the default sink's monitor source.
//!
//! Every sink has a monitor carrying whatever that sink is playing, so
//! recording the default sink's monitor captures everything the machine
//! plays. No permission gates it (there is nothing to be denied, unlike
//! macOS's tap consent), and no sound server at all degrades to mic-only.
//!
//! Three decisions, each from the Phase −1 spike rather than guesswork
//! ([260801-linux-port.md]):
//!
//! - Native geometry, not server-side conversion. The server would
//!   accept a request for 16 kHz mono against a 48 kHz stereo monitor and
//!   convert for us. We ask for the monitor's own channels and rate instead
//!   (f32 samples, which is a cheap reinterpretation) and let
//!   `audio/pipeline.rs` downmix and resample, the same division of labour
//!   as the WASAPI and Core Audio capture paths. One resampler, tested
//!   once, feeding the mixer canonical 16 kHz mono from every source.
//! - `monitor_source_name`, never a built string. `SinkInfo` states the
//!   monitor outright; the `"<sink>.monitor"` convention held everywhere it
//!   was checked, but the field is what this reads.
//! - A default-sink change mid-recording does not crash, and the stream
//!   stays pinned to the monitor it opened: Windows' existing blind spot,
//!   shared rather than fixed. It bites less here than on Windows, because
//!   PipeWire's `module-stream-restore` keeps apps on remembered sinks, so
//!   they often do not follow a default change either.
//!
//! `preferred_device` is ignored, as on macOS. The picker's output list
//! comes from cpal, whose Linux names are ALSA PCMs (`sysdefault:CARD=…`)
//! rather than pulse sinks, so nothing here could match one reliably.
//! Pinning system audio to a chosen device would mean enumerating sinks in
//! the picker instead: a real design, backlogged rather than faked.
//!
//! [260801-linux-port.md]: ../../../../docs/plans/260801-linux-port.md

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use libpulse_binding::sample::{Format, Spec};
use libpulse_binding::stream::Direction;

use crate::audio::pipeline::Pipeline;
use crate::platform::types::{
    CaptureCommand, SystemAudioSinkFactory, SystemAudioSource, SystemAudioWanted,
};

/// How much audio to pull per read. Small enough that a stop is noticed
/// promptly (the loop only checks between reads) and large enough not to
/// spin: ~20 ms.
const READ_MS: u32 = 20;

/// A running system-audio capture feeding 16 kHz mono blocks into its sink.
///
/// Not `Send` (the pulse stream belongs to the thread that opened it): built
/// and owned by the recorder's dedicated system-audio thread, like the
/// Windows loopback stream and the macOS tap.
pub struct SystemAudioCapture {
    stream: libpulse_simple_binding::Simple,
    pipeline: Pipeline,
    /// Interleaved f32 scratch, one read's worth.
    buf: Vec<f32>,
    paused: Arc<AtomicBool>,
    sink: Box<dyn Fn(&[f32]) + Send>,
}

impl SystemAudioCapture {
    /// Hold the system-audio capture until `stop_rx` closes: the platform
    /// layer's blocking entry point. Start once, announce, pump, drop:
    /// there is no per-tick reopen to supervise (the Windows side's job),
    /// because one monitor carries the whole machine's output.
    pub fn run(
        sink_factory: SystemAudioSinkFactory,
        paused: Arc<AtomicBool>,
        preferred_device: Option<&str>,
        // The picker's selection, ignored here: a monitor is one global
        // mixdown, so there is no per-app stream to narrow to. The same
        // compromise macOS makes, and structural to the pulse protocol.
        _wanted: Box<dyn Fn() -> SystemAudioWanted + Send>,
        stop_rx: std::sync::mpsc::Receiver<CaptureCommand>,
        on_source: Box<dyn Fn(SystemAudioSource) + Send>,
    ) {
        let Some(mut capture) = Self::start(sink_factory(), paused, preferred_device) else {
            tracing::warn!("system-audio capture unavailable — recording mic only");
            return;
        };
        // A monitor captures everything the machine plays, with no
        // per-device notion to report.
        on_source(SystemAudioSource::Everything { devices: 0 });

        loop {
            // A closed channel is the stop. A `Reconfigure` is a picker
            // change this platform cannot act on, so it is drained and
            // ignored.
            if matches!(
                stop_rx.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Disconnected)
            ) {
                break;
            }
            if !capture.pump() {
                // The monitor went away: sink removed, or the server
                // restarted. The recording continues mic-only rather than
                // failing outright.
                tracing::warn!("system-audio monitor ended — continuing mic only");
                break;
            }
        }
    }

    /// Open a record stream on the default sink's monitor. `None` on any
    /// failure (no server, no default sink, a format the server refuses),
    /// and the mixer degrades to mic-only.
    pub fn start(
        sink: Box<dyn Fn(&[f32]) + Send>,
        paused: Arc<AtomicBool>,
        _preferred_device: Option<&str>,
    ) -> Option<Self> {
        // The connection is only needed to resolve the monitor; the capture
        // itself rides its own stream. Dropping it here also exercises the
        // ordered teardown `pulse.rs` documents.
        let target = {
            let mut pulse = super::pulse::Pulse::connect()?;
            pulse.default_monitor()?
        };

        let spec = Spec {
            format: Format::F32le,
            channels: target.channels,
            rate: target.rate,
        };
        if !spec.is_valid() {
            tracing::warn!(
                channels = target.channels,
                rate = target.rate,
                "monitor reports an unusable format"
            );
            return None;
        }

        let stream = match libpulse_simple_binding::Simple::new(
            None,
            "embral",
            Direction::Record,
            Some(&target.source_name),
            "system audio",
            &spec,
            None,
            None,
        ) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(monitor = %target.source_name, "could not open monitor: {e}");
                return None;
            }
        };

        let pipeline = match Pipeline::new(target.channels as usize, target.rate) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("system-audio pipeline: {e}");
                return None;
            }
        };

        let frames = (target.rate * READ_MS / 1000).max(1) as usize;
        tracing::info!(
            monitor = %target.source_name,
            sink = %target.sink_description,
            channels = target.channels,
            rate = target.rate,
            "system-audio capture open"
        );
        Some(Self {
            stream,
            pipeline,
            buf: vec![0f32; frames * target.channels as usize],
            paused,
            sink,
        })
    }

    /// Read one block and feed it through the pipeline. `false` when the
    /// stream is finished and the capture should give up.
    fn pump(&mut self) -> bool {
        {
            // SAFETY: a `&mut [f32]` viewed as the bytes it already is:
            // same allocation, same lifetime, 4 bytes per sample, and the
            // stream was opened as `F32le` so the server writes exactly this
            // layout. `Simple::read` fills the whole slice or errors.
            let bytes = unsafe {
                std::slice::from_raw_parts_mut(
                    self.buf.as_mut_ptr() as *mut u8,
                    std::mem::size_of_val(self.buf.as_slice()),
                )
            };
            if let Err(e) = self.stream.read(bytes) {
                tracing::debug!("monitor read failed: {e}");
                return false;
            }
        }
        // Paused: keep draining the stream (so the server does not back up)
        // but feed the mixer nothing.
        if self.paused.load(Ordering::Relaxed) {
            return true;
        }
        // Split the borrow: `push` needs `&mut pipeline` while the emit
        // closure needs `&sink`.
        let Self {
            pipeline, buf, sink, ..
        } = self;
        pipeline.push(buf, &mut |chunk| sink(chunk));
        true
    }
}
