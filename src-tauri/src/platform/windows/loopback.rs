//! System-audio capture ([recording.md](../../../../docs/recording.md)):
//! the device path (the WASAPI loopback trick, an output device opened
//! as a cpal input) plus its supervision loop: reopen when the stream
//! dies, follow the default output when no device is pinned. The
//! per-process capture (`process_loopback.rs`) outranks this whole file
//! when a detected call's pid is available.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::platform::types::{
    CaptureCommand, SystemAudioSinkFactory, SystemAudioSource, SystemAudioWanted,
};

/// Supervision cadence: how often a live capture re-checks its stream and
/// the default output, and how long a failed open waits before retrying (a
/// headset plugged in mid-meeting starts capturing on the next tick).
const SUPERVISE_EVERY: Duration = Duration::from_secs(5);

pub struct SystemAudioCapture;

impl SystemAudioCapture {
    /// Hold the system-audio capture until `stop_rx` closes. Blocking: the
    /// capture handles are `!Send`, so their whole lifecycle lives on the
    /// calling (`system-audio`) thread. Every (re)open announces what is
    /// being captured through `on_source`.
    pub fn run(
        sink_factory: SystemAudioSinkFactory,
        paused: Arc<AtomicBool>,
        preferred_device: Option<&str>,
        // Read fresh each tick: the source picker changes this mid-recording.
        wanted: Box<dyn Fn() -> SystemAudioWanted + Send>,
        stop_rx: Receiver<CaptureCommand>,
        on_source: Box<dyn Fn(SystemAudioSource) + Send>,
    ) {
        // Two modes, re-chosen every tick from the picker's selection.
        // Everything (the default) captures each active output endpoint,
        // so which endpoint a meeting app is pinned to stops mattering,
        // the bug "follow the default output" could never fix. A narrowed
        // selection captures those apps' own audio instead: the only way
        // to leave one app out of the mix.
        let mut open: Vec<OpenEndpoint> = Vec::new();
        let mut apps: Vec<super::process_loopback::AppCapture> = Vec::new();
        let mut current = SystemAudioWanted::Everything;
        let mut first_pass = true;

        loop {
            let next = wanted();
            if first_pass || next != current {
                // The selection changed: tear the old captures down whole,
                // so nothing is captured twice.
                open.clear();
                apps.clear();
                current = next;
                first_pass = false;
            }

            match current.clone() {
                SystemAudioWanted::Everything => {
                    let devices = wanted_endpoints(preferred_device);
                    let live: Vec<(String, bool)> = open
                        .iter()
                        .map(|e| (e.name.clone(), e.dead.load(Ordering::SeqCst)))
                        .collect();
                    let (add, drop_these) = endpoint_diff(&devices, &live);
                    open.retain(|e| {
                        let keep = !drop_these.contains(&e.name);
                        if !keep {
                            tracing::info!(device = %e.name, "output endpoint closing");
                        }
                        keep
                    });
                    for name in add {
                        if let Some(endpoint) =
                            open_endpoint(&name, sink_factory(), paused.clone())
                        {
                            open.push(endpoint);
                        }
                    }
                    if open.is_empty() {
                        tracing::warn!("no output endpoint could be captured — mic only for now");
                    }
                    on_source(SystemAudioSource::Everything {
                        devices: open.len(),
                    });
                }
                SystemAudioWanted::Apps(pids) => {
                    // Drop the ones whose app went away; open the rest.
                    apps.retain(|a| pids.contains(&a.pid) && a.alive());
                    for pid in &pids {
                        if apps.iter().any(|a| a.pid == *pid) {
                            continue;
                        }
                        match super::process_loopback::AppCapture::start(
                            *pid,
                            sink_factory(),
                            paused.clone(),
                        ) {
                            Some(capture) => apps.push(capture),
                            // One app failing is not fatal; the others
                            // keep recording and the picker keeps working.
                            None => tracing::warn!(pid, "app capture unavailable — skipping it"),
                        }
                    }
                    if apps.is_empty() && !pids.is_empty() {
                        // Nothing could be captured per-app: recording
                        // everything beats recording nothing.
                        tracing::warn!(
                            "no app could be captured — falling back to capturing everything"
                        );
                        current = SystemAudioWanted::Everything;
                        continue;
                    }
                    on_source(SystemAudioSource::Apps {
                        names: apps.iter().map(|a| a.name.clone()).collect(),
                    });
                }
            }

            match stop_rx.recv_timeout(SUPERVISE_EVERY) {
                // A picker change wakes us at once; a timeout is the
                // routine re-check of dead streams and the endpoint set.
                Ok(CaptureCommand::Reconfigure) => {}
                Err(RecvTimeoutError::Timeout) => {}
                // The channel closed: the recorder is gone.
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }
}

/// One captured output endpoint. Dropping it stops the stream and takes
/// its ring out of the mix.
struct OpenEndpoint {
    name: String,
    dead: Arc<AtomicBool>,
    _stream: cpal::Stream,
}

/// The endpoints we should be capturing: the pinned device alone when
/// Settings names one (and it exists), otherwise every active output.
fn wanted_endpoints(preferred_device: Option<&str>) -> Vec<String> {
    let host = cpal::default_host();
    let all: Vec<String> = host
        .output_devices()
        .map(|it| it.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default();
    let pin = preferred_device.map(str::trim).filter(|p| !p.is_empty());
    match pin {
        Some(pin) if all.iter().any(|n| n == pin) => vec![pin.to_string()],
        Some(pin) => {
            tracing::warn!(
                device = pin,
                "configured output device is absent — capturing every output instead"
            );
            all
        }
        None => all,
    }
}

fn open_endpoint(
    name: &str,
    sink: Box<dyn Fn(&[f32]) + Send>,
    paused: Arc<AtomicBool>,
) -> Option<OpenEndpoint> {
    let host = cpal::default_host();
    let device = host
        .output_devices()
        .ok()?
        .find(|d| d.name().map(|n| n == name).unwrap_or(false))?;
    let config = device.default_output_config().ok()?;
    tracing::info!(
        "Loopback output: device='{}', sample_rate={} Hz, channels={}, format={:?}",
        name,
        config.sample_rate().0,
        config.channels(),
        config.sample_format()
    );
    let dead = Arc::new(AtomicBool::new(false));
    let stream = crate::audio::recorder::build_stream(
        "loopback",
        &device,
        &config,
        paused,
        sink,
        Some(dead.clone()),
    )
    .map_err(|e| tracing::warn!(device = name, "loopback stream unavailable: {e}"))
    .ok()?;
    if stream.play().is_err() {
        tracing::warn!(device = name, "loopback stream play() failed");
        return None;
    }
    Some(OpenEndpoint {
        name: name.to_string(),
        dead,
        _stream: stream,
    })
}

/// Which of `wanted` still need opening, and which open endpoints must go
/// (endpoint vanished, or its stream died). Pure so the supervision rule
/// is testable without audio hardware.
pub(crate) fn endpoint_diff(
    wanted: &[String],
    open: &[(String, bool)],
) -> (Vec<String>, Vec<String>) {
    let drop_these: Vec<String> = open
        .iter()
        .filter(|(name, dead)| *dead || !wanted.iter().any(|w| w == name))
        .map(|(name, _)| name.clone())
        .collect();
    let add_these: Vec<String> = wanted
        .iter()
        .filter(|w| {
            !open
                .iter()
                .any(|(name, dead)| name == *w && !*dead)
        })
        .cloned()
        .collect();
    (add_these, drop_these)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_new_endpoint_is_opened_and_a_vanished_one_dropped() {
        // Headset plugged in, monitor unplugged, between two ticks.
        let (add, drop) = endpoint_diff(
            &names(&["Speakers", "Headset"]),
            &[("Speakers".into(), false), ("Monitor".into(), false)],
        );
        assert_eq!(add, names(&["Headset"]));
        assert_eq!(drop, names(&["Monitor"]));
    }

    #[test]
    fn a_dead_stream_is_dropped_and_reopened() {
        // Errors were once log-only, leaving a silently mic-only meeting.
        let (add, drop) = endpoint_diff(&names(&["Speakers"]), &[("Speakers".into(), true)]);
        assert_eq!(add, names(&["Speakers"]));
        assert_eq!(drop, names(&["Speakers"]));
    }

    #[test]
    fn a_steady_set_does_nothing() {
        let (add, drop) = endpoint_diff(
            &names(&["Speakers", "Headset"]),
            &[("Speakers".into(), false), ("Headset".into(), false)],
        );
        assert!(add.is_empty() && drop.is_empty());
    }
}
