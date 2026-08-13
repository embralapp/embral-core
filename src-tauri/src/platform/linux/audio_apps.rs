//! What is playing audio right now: the source picker's app rows
//! ([recording.md](../../../../docs/recording.md)).
//!
//! PulseAudio's sink-input list, the playback mirror of `mic_users.rs`.
//!
//! These rows stay informational on Linux, as on macOS: the capture is a
//! monitor-source mixdown of everything the machine plays, so unchecking an
//! app cannot exclude it. That is structural to the pulse protocol rather
//! than unfinished work; per-process capture needs PipeWire's own graph API
//! ([260801-linux-port.md]). Showing the rows is still worth it: they tell
//! the user what is about to be captured.
//!
//! [260801-linux-port.md]: ../../../../docs/plans/260801-linux-port.md

use crate::platform::types::AppId;

pub fn apps_playing_audio(exclude_pid: u32) -> Vec<AppId> {
    let Some(mut pulse) = super::pulse::Pulse::connect() else {
        return Vec::new();
    };
    pulse.playback_streams(exclude_pid)
}
