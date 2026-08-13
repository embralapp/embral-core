//! What is playing audio right now: a stub on macOS.
//!
//! The Core Audio tap this platform uses is a global mixdown (every
//! process except our own), so there is no per-app list to choose from
//! yet. An empty list means the source picker shows no app rows and the
//! recording captures everything, which is the tap's behavior anyway.
//! Per-process taps are a later port item ([260725-macos-port.md]).

use crate::platform::types::AppId;

pub fn apps_playing_audio(_exclude_pid: u32) -> Vec<AppId> {
    Vec::new()
}
