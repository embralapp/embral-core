//! Which apps hold an open microphone stream: detection's signal
//! ([detection.md](../../../../docs/detection.md) §Signal).
//!
//! PulseAudio's source-output list: every client with a record stream
//! open, which is exactly the question detection asks. Served by PipeWire's
//! pulse layer in practice (see `pulse.rs` for why that protocol).
//!
//! Verified against a live Zoom-in-Chrome call: the stream reports
//! `application.process.binary = "chrome"` and
//! `application.name = "Google Chrome input"`, so the allowlist's bare
//! `chrome` token matches on either identity. Easier than the macOS scan,
//! which often has only a helper's bundle id to go on.
//!
//! The pid, though, is a helper's (Chrome's
//! `--utility-sub-type=audio.mojom.AudioService` child, not the browser), so
//! nothing may build process-tree logic on it. It is carried for logging and
//! for the picker's grouping, both of which tolerate it.

use crate::platform::types::AppId;

pub fn processes_using_microphone(exclude_pid: u32) -> Vec<AppId> {
    let Some(mut pulse) = super::pulse::Pulse::connect() else {
        // No sound server: nothing is recording, as far as we can tell. The
        // inert value, not an error; detection sees no calls.
        return Vec::new();
    };
    pulse.record_streams(exclude_pid)
}
