//! A short-lived PulseAudio connection, shared by the two introspection
//! scans (`mic_users.rs`, `audio_apps.rs`). Served by PipeWire's
//! pulse-compatibility layer on every desktop that matters, and by real
//! PulseAudio on legacy systems; one protocol covers both
//! ([260801-linux-port.md]).
//!
//! Drop order matters here. Rust drops struct fields in declaration
//! order, and dropping the `Mainloop` while the `Context` still owns IO
//! events trips a C-side assertion and `abort()`s the whole process:
//! `Assertion '!e->dead' failed at mainloop.c:207, mainloop_io_free()`. Not
//! an `Err`: an abort, with no Rust-level way to catch it. So the context is
//! declared first (dropped first) and disconnected explicitly. This cost the
//! Phase −1 spike an afternoon and would have read as a mystery crash.
//!
//! A connection per scan rather than one kept open: the scans run every 3 s
//! from `spawn_blocking`, a session connect is sub-millisecond on a local
//! socket, and a long-lived connection would need its own thread to pump the
//! mainloop plus reconnect logic for a server restart. Absence of a server
//! is a normal state (`None`), not an error; it degrades to "no apps seen".
//!
//! [260801-linux-port.md]: ../../../../docs/plans/260801-linux-port.md

use std::cell::RefCell;
use std::rc::Rc;

use libpulse_binding::context::{Context, FlagSet, State};
use libpulse_binding::mainloop::standard::{IterateResult, Mainloop};
use libpulse_binding::proplist::Proplist;

use crate::platform::types::AppId;

/// How long to spin waiting for the server before giving up. A local socket
/// answers immediately; this only bounds a pathological server.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);

/// A monitor source to record, plus the geometry the server will hand over.
#[derive(Debug, Clone)]
pub struct MonitorTarget {
    /// The monitor source's own name, from `SinkInfo::monitor_source_name`.
    pub source_name: String,
    /// The sink's human-facing description, for the log line only.
    pub sink_description: String,
    pub channels: u8,
    pub rate: u32,
}

pub struct Pulse {
    // Declared before `mainloop` on purpose; see the module doc.
    ctx: Context,
    mainloop: Mainloop,
}

impl Drop for Pulse {
    fn drop(&mut self) {
        self.ctx.disconnect();
    }
}

impl Pulse {
    /// Connect to the session's sound server. `None` when there is none:
    /// the honest inert value, and a real state on a machine with no audio
    /// stack rather than only an error path.
    pub fn connect() -> Option<Self> {
        let mut mainloop = Mainloop::new()?;
        let mut ctx = Context::new(&mainloop, "embral")?;
        ctx.connect(None, FlagSet::NOFLAGS, None).ok()?;

        let deadline = std::time::Instant::now() + CONNECT_TIMEOUT;
        loop {
            match mainloop.iterate(false) {
                IterateResult::Quit(_) | IterateResult::Err(_) => return None,
                IterateResult::Success(_) => {}
            }
            match ctx.get_state() {
                State::Ready => break,
                State::Failed | State::Terminated => return None,
                _ if std::time::Instant::now() > deadline => {
                    tracing::debug!("sound server did not become ready in time");
                    return None;
                }
                _ => {}
            }
        }
        Some(Pulse { ctx, mainloop })
    }

    /// Pump the mainloop until `done` flips, or the deadline passes. The
    /// deadline matters: a wedged server must not hang the 3 s detection
    /// tick forever.
    fn pump(&mut self, done: &Rc<RefCell<bool>>) {
        let deadline = std::time::Instant::now() + CONNECT_TIMEOUT;
        while !*done.borrow() {
            if std::time::Instant::now() > deadline {
                tracing::debug!("sound-server introspection timed out");
                return;
            }
            match self.mainloop.iterate(true) {
                IterateResult::Quit(_) | IterateResult::Err(_) => return,
                IterateResult::Success(_) => {}
            }
        }
    }

    /// The default sink's monitor source: what system-audio capture
    /// records ([recording.md](../../../../docs/recording.md)).
    ///
    /// `SinkInfo::monitor_source_name` is authoritative, so this never builds
    /// the `"<sink>.monitor"` string by hand. The convention held on every
    /// sink measured, but the field is the field.
    ///
    /// The native channel count and rate come back too, because the capture
    /// asks the server for native geometry and lets `audio/pipeline.rs` do
    /// the downmix and resample, the same division of labour as the WASAPI
    /// and Core Audio capture paths, rather than having the server convert.
    pub fn default_monitor(&mut self) -> Option<MonitorTarget> {
        let default_sink = {
            let name = Rc::new(RefCell::new(None::<String>));
            let done = Rc::new(RefCell::new(false));
            {
                let name = name.clone();
                let done = done.clone();
                self.ctx.introspect().get_server_info(move |info| {
                    *name.borrow_mut() = info.default_sink_name.as_deref().map(str::to_string);
                    *done.borrow_mut() = true;
                });
            }
            self.pump(&done);
            let resolved = name.borrow().clone();
            resolved?
        };

        let target = Rc::new(RefCell::new(None::<MonitorTarget>));
        let done = Rc::new(RefCell::new(false));
        {
            let target = target.clone();
            let done = done.clone();
            let wanted = default_sink.clone();
            self.ctx.introspect().get_sink_info_list(move |res| match res {
                libpulse_binding::callbacks::ListResult::Item(sink) => {
                    if sink.name.as_deref() == Some(wanted.as_str()) {
                        if let Some(monitor) = sink.monitor_source_name.as_deref() {
                            *target.borrow_mut() = Some(MonitorTarget {
                                source_name: monitor.to_string(),
                                sink_description: sink
                                    .description
                                    .as_deref()
                                    .unwrap_or(&wanted)
                                    .to_string(),
                                channels: sink.sample_spec.channels,
                                rate: sink.sample_spec.rate,
                            });
                        }
                    }
                }
                _ => *done.borrow_mut() = true,
            });
        }
        self.pump(&done);
        let found = target.borrow().clone();
        found
    }

    /// Apps with an open record stream: detection's signal.
    pub fn record_streams(&mut self, exclude_pid: u32) -> Vec<AppId> {
        let found = Rc::new(RefCell::new(Vec::new()));
        let done = Rc::new(RefCell::new(false));
        {
            let found = found.clone();
            let done = done.clone();
            self.ctx
                .introspect()
                .get_source_output_info_list(move |res| match res {
                    libpulse_binding::callbacks::ListResult::Item(o) => {
                        if let Some(app) = app_from_props(&o.proplist, exclude_pid) {
                            found.borrow_mut().push(app);
                        }
                    }
                    _ => *done.borrow_mut() = true,
                });
        }
        self.pump(&done);
        let apps = found.borrow().clone();
        dedupe(apps)
    }

    /// Apps with an open playback stream: the source picker's rows.
    pub fn playback_streams(&mut self, exclude_pid: u32) -> Vec<AppId> {
        let found = Rc::new(RefCell::new(Vec::new()));
        let done = Rc::new(RefCell::new(false));
        {
            let found = found.clone();
            let done = done.clone();
            self.ctx
                .introspect()
                .get_sink_input_info_list(move |res| match res {
                    libpulse_binding::callbacks::ListResult::Item(i) => {
                        if let Some(app) = app_from_props(&i.proplist, exclude_pid) {
                            found.borrow_mut().push(app);
                        }
                    }
                    _ => *done.borrow_mut() = true,
                });
        }
        self.pump(&done);
        let apps = found.borrow().clone();
        dedupe(apps)
    }
}

/// A stream's properties → the identity the matcher tests
/// ([detection.md](../../../../docs/detection.md) §Matching).
///
/// Deliberately not filtered on `corked`. A corked stream is one the
/// client paused, and an app that still holds the source is still in a call,
/// which is the whole basis of detection's grace budget ("mute does not
/// release the capture session", detection.md §Signal). Treating corked as
/// "gone" would end a meeting the moment someone muted.
///
/// `exe` gets `application.process.binary` and `display_name` gets
/// `application.name`; either alone is enough for the matcher, and having
/// both is useful redundancy. Measured against a real Zoom-in-Chrome call:
/// `binary = "chrome"` and `name = "Google Chrome input"`, and the `chrome`
/// token matches both. The `" input"` suffix pulse appends to a record
/// stream's app name is harmless through `displayAppName`'s token map, wrong
/// if ever shown raw.
fn app_from_props(props: &Proplist, exclude_pid: u32) -> Option<AppId> {
    let pid: u32 = props
        .get_str(libpulse_binding::proplist::properties::APPLICATION_PROCESS_ID)
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    let exe = props
        .get_str(libpulse_binding::proplist::properties::APPLICATION_PROCESS_BINARY)
        .filter(|s| !s.trim().is_empty());
    let display_name = props
        .get_str(libpulse_binding::proplist::properties::APPLICATION_NAME)
        .filter(|s| !s.trim().is_empty());
    // No identity at all is not an app we can match or name.
    if exe.is_none() && display_name.is_none() {
        return None;
    }
    if is_our_own(pid, exclude_pid, exe.as_deref(), display_name.as_deref()) {
        return None;
    }
    Some(AppId {
        pid,
        exe,
        bundle_id: None,
        display_name,
    })
}

/// Whether a stream is one of ours. We record, so our own capture must never
/// read as a meeting.
///
/// **The pid check alone cannot do this on Linux**, and the reason is
/// structural: our mic stream goes through cpal → ALSA → PipeWire's *ALSA*
/// compatibility layer, not the pulse client library, and that layer sets no
/// `application.process.id` and no `application.process.binary` at all. The
/// only identity it publishes is `application.name = "PipeWire ALSA
/// [embral]"`. So the pid parses to 0 and `exclude_pid` has nothing to match.
///
/// Measured, not theorised: the detection log read
/// `mic sessions changed now=["chrome", "PipeWire ALSA [embral]"]` during a
/// real call. Under the `prompt` and `selective` policies the allowlist
/// rejects that name and nothing goes wrong, which is exactly why this hid;
/// but `Always` takes any mic user, so embral would have detected itself
/// and auto-started a recording of its own recording.
///
/// Hence the fallback: match our own program name in either identity. Erring
/// toward excluding one stream too many is the safe direction: a missed
/// detection is a nuisance, self-detection is a loop.
fn is_our_own(pid: u32, exclude_pid: u32, exe: Option<&str>, display_name: Option<&str>) -> bool {
    if pid != 0 && pid == exclude_pid {
        return true;
    }
    let ids: Vec<String> = [exe, display_name]
        .into_iter()
        .flatten()
        .map(|s| s.to_lowercase())
        .collect();
    own_markers()
        .iter()
        .any(|own| ids.iter().any(|id| id.contains(own.as_str())))
}

/// The names by which one of our own streams can be recognised, lowercased.
///
/// Two sources, unioned, because neither alone is trustworthy:
///
/// - `CARGO_PKG_NAME` is what the shipped binary is called and is stable in
///   every build, including under `cargo test`, where `current_exe()` is
///   the test harness (`embral_lib-<hash>`) rather than the app. Relying on
///   `current_exe` alone made this module's own regression test fail, which
///   is a fair warning about relying on it in a bundle.
/// - `current_exe()`'s file name covers a rename of the binary without a
///   rename of the package, and costs one syscall once.
fn own_markers() -> &'static [String] {
    use std::sync::OnceLock;
    static MARKERS: OnceLock<Vec<String>> = OnceLock::new();
    MARKERS.get_or_init(|| {
        let mut out = vec![env!("CARGO_PKG_NAME").to_lowercase()];
        if let Some(exe) = std::env::current_exe()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()))
        {
            if !exe.is_empty() && !out.contains(&exe) {
                out.push(exe);
            }
        }
        out.retain(|s| !s.is_empty());
        out
    })
}

/// One row per app, as on macOS. An app commonly holds several streams at
/// once (Chrome's audio service opens one per tab), and the same label twice
/// is a row the reader cannot tell apart. Keyed on the identity rather than
/// the pid, because the pid belongs to a helper: a real Zoom-in-Chrome call
/// reports Chrome's `audio.mojom.AudioService` child, not the browser, so
/// two tabs could differ by pid while being one app to the user.
fn dedupe(apps: Vec<AppId>) -> Vec<AppId> {
    let mut out: Vec<AppId> = Vec::new();
    for app in apps {
        let key = |a: &AppId| {
            (
                a.exe.as_deref().unwrap_or("").to_lowercase(),
                a.display_name.as_deref().unwrap_or("").to_lowercase(),
            )
        };
        if !out.iter().any(|seen| key(seen) == key(&app)) {
            out.push(app);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(pairs: &[(&str, &str)]) -> Proplist {
        let mut p = Proplist::new().unwrap();
        for (k, v) in pairs {
            p.set_str(k, v).unwrap();
        }
        p
    }

    #[test]
    fn reads_the_identities_a_real_browser_call_reports() {
        // Verbatim from a live Zoom-in-Chrome call on the dev box.
        let p = props(&[
            ("application.process.id", "4564"),
            ("application.process.binary", "chrome"),
            ("application.name", "Google Chrome input"),
        ]);
        let app = app_from_props(&p, 999).expect("an identity");
        assert_eq!(app.pid, 4564);
        assert_eq!(app.exe.as_deref(), Some("chrome"));
        assert_eq!(app.display_name.as_deref(), Some("Google Chrome input"));
        assert_eq!(app.bundle_id, None, "Linux has no bundle ids");
        // The matcher tests every identity; both of these must reach it.
        let identities: Vec<&str> = app.identities().collect();
        assert!(identities.contains(&"chrome"));
        assert!(identities.contains(&"Google Chrome input"));
    }

    #[test]
    fn our_own_streams_are_excluded() {
        let ours = props(&[
            ("application.process.id", "4242"),
            ("application.process.binary", "embral"),
            ("application.name", "PipeWire ALSA [embral]"),
        ]);
        assert!(app_from_props(&ours, 4242).is_none(), "we are not a meeting");
        // And still ours when the pid does not match, which is the whole
        // point, since the ALSA bridge publishes no pid at all. An earlier
        // version of this test asserted the opposite and was wrong.
        assert!(
            app_from_props(&ours, 1).is_none(),
            "our own name identifies us regardless of pid"
        );
        // Someone else's stream, by contrast, survives either way.
        let theirs = props(&[
            ("application.process.id", "4242"),
            ("application.process.binary", "chrome"),
            ("application.name", "Google Chrome input"),
        ]);
        assert!(app_from_props(&theirs, 1).is_some());
    }

    /// The regression this file's `is_our_own` exists for, measured on a real
    /// call: our own capture appears in the scan with no process props at
    /// all, because cpal records through ALSA and PipeWire's ALSA layer
    /// publishes only a stream name. `exclude_pid` cannot see it.
    #[test]
    fn our_own_alsa_bridged_stream_is_excluded_without_a_pid() {
        // Verbatim shape of what the ALSA bridge publishes: name only.
        let p = props(&[("application.name", "PipeWire ALSA [embral]")]);
        // pid is absent → parses to 0 → the pid guard is useless here.
        assert!(
            app_from_props(&p, 4242).is_none(),
            "we must not detect our own recording as a meeting"
        );
    }

    #[test]
    fn own_name_matching_does_not_swallow_real_apps() {
        for (binary, name) in [
            ("chrome", "Google Chrome input"),
            ("zoom", "Zoom"),
            ("teams-for-linux", "Microsoft Teams"),
        ] {
            let p = props(&[
                ("application.process.id", "555"),
                ("application.process.binary", binary),
                ("application.name", name),
            ]);
            assert!(
                app_from_props(&p, 4242).is_some(),
                "{binary} is someone else's stream and must survive"
            );
        }
    }

    #[test]
    fn the_own_name_fallback_is_reachable_at_all() {
        // Guards the fallback itself: if the markers ever came up empty,
        // `is_our_own` would silently degrade to the useless pid check.
        let markers = own_markers();
        assert!(!markers.is_empty(), "we must know at least one of our own names");
        assert!(markers.iter().any(|m| m == "embral"), "the package name is a marker");
        for m in markers {
            assert!(
                is_our_own(0, 1, None, Some(&format!("PipeWire ALSA [{m}]"))),
                "marker {m} must be recognised in a stream name"
            );
        }
        assert!(!is_our_own(0, 1, Some("chrome"), Some("Google Chrome")));
    }

    #[test]
    fn a_stream_with_no_identity_is_skipped() {
        let p = props(&[("application.process.id", "7")]);
        assert!(app_from_props(&p, 0).is_none());
        // Whitespace is not an identity either; it would match every token.
        let blank = props(&[
            ("application.process.id", "7"),
            ("application.process.binary", "   "),
        ]);
        assert!(app_from_props(&blank, 0).is_none());
    }

    #[test]
    fn a_missing_pid_does_not_collide_with_the_exclusion() {
        // pid 0 means "pulse did not say". It must not be mistaken for the
        // excluded pid, and it must not suppress a real identity.
        let p = props(&[("application.process.binary", "zoom")]);
        let app = app_from_props(&p, 0).expect("still an app");
        assert_eq!(app.pid, 0);
        assert_eq!(app.exe.as_deref(), Some("zoom"));
    }

    #[test]
    fn one_app_holding_several_streams_becomes_one_row() {
        // Chrome's audio service opens a stream per tab, and its pid is the
        // helper's, so two rows can differ by pid yet be one app.
        let chrome = |pid| AppId {
            pid,
            exe: Some("chrome".into()),
            bundle_id: None,
            display_name: Some("Google Chrome".into()),
        };
        let zoom = AppId {
            pid: 10,
            exe: Some("zoom".into()),
            bundle_id: None,
            display_name: Some("Zoom".into()),
        };
        let rows = dedupe(vec![chrome(1), chrome(2), zoom.clone(), chrome(3)]);
        assert_eq!(rows.len(), 2, "one Chrome row and one Zoom row");
        assert_eq!(rows[0].exe.as_deref(), Some("chrome"));
        assert_eq!(rows[1].exe.as_deref(), Some("zoom"));
    }

    /// Live probe against whatever is running on this machine:
    /// `cargo test -p embral --lib pulse_live -- --ignored --nocapture`.
    /// The only way to see the real prop coverage; a green unit test says
    /// nothing about what a sound server actually hands over.
    #[test]
    #[ignore = "manual probe; reports the machine's live audio streams"]
    fn pulse_live_streams() {
        let Some(mut pulse) = Pulse::connect() else {
            eprintln!("no sound server — this is the degrade-to-nothing case");
            return;
        };
        let mine = std::process::id();
        eprintln!("recording apps: {:?}", pulse.record_streams(mine));
        eprintln!("playing apps:   {:?}", pulse.playback_streams(mine));
    }
}
