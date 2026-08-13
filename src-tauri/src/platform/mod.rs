//! The platform layer: every OS-specific mechanism in the app lives behind
//! this module, `std::sys`-style: sibling directories with mirrored
//! filenames, selected at compile time. The rest of `src-tauri` calls
//! `crate::platform::…` and never names an OS API
//! ([architecture.md](../../docs/architecture.md) §Platform layer).
//!
//! ## The contract
//!
//! Each platform directory implements, in same-named files (landing over
//! the port's phases; see [260725-macos-port.md]):
//!
//! - `mic_users.rs`: `processes_using_microphone(exclude_pid) ->
//!   Vec<AppId>`, the apps holding an active microphone stream
//!   (detection's signal).
//! - `input.rs`: `paste_keystroke()` (synthesize the platform paste
//!   chord into the focused app) and `focused_app() -> Option<AppId>`.
//! - `theme.rs`: the OS shell theme + accent the tray icons follow, and
//!   a change watcher.
//! - `supervisor.rs`: children die with this process however it dies,
//!   plus `run_reaper()` for the `--child-reaper` subprocess body (a no-op
//!   where the OS already covers orphan cleanup).
//! - `proc.rs`: spawn decoration (`hide_console`), executable naming
//!   (`exe_name`), CLI resolution (`find_cli`).
//! - `mcp_paths.rs`: where AI clients keep their MCP configs.
//! - `overlay.rs`: `style_overlay(&WebviewWindow)`, extra panel behaviors
//!   for the dictation overlay (macOS joins Spaces; Windows no-op).
//! - `notice.rs`: `style_notice(&WebviewWindow)`, the notice window's
//!   never-activate styling (Windows `WS_EX_NOACTIVATE`; macOS swaps the
//!   window's class to a non-activating `NSPanel`). Both take the Tauri
//!   window, not a raw handle: extracting `hwnd()` / `ns_window()` is
//!   itself platform-specific and belongs in this layer.
//! - `power.rs`: `power_source() -> PowerSource`, wall power vs battery,
//!   read once per recording by the provider policy
//!   ([transcription.md](../../../docs/transcription.md)).
//! - `ocr.rs`: `recognize_text(bytes) -> Recognized`, the text inside a
//!   pasted image, read by the in-box engine
//!   (`Windows.Media.Ocr` / Vision) so nothing is downloaded or bundled.
//!   Takes bytes, not a path: both engines prefer them, and file IO stays
//!   in the callers above this layer
//!   ([storage.md](../../../docs/storage.md)).
//! - `os_build()`: the OS version string telemetry reports.
//!
//! ## Stub rule
//!
//! A platform that lacks a capability returns the inert value (`None`,
//! empty vec, no-op). Callers already degrade gracefully, so no caller ever
//! branches on the OS to decide what to do: `paste_keystroke` returning
//! `Err` is handled the same way everywhere, and nothing above this layer
//! asks "am I on Linux?" before choosing a path.
//!
//! That is a rule about capabilities, not about the token `cfg`. A
//! `#[cfg]` that plumbs a genuinely per-OS type is ordinary code, not a
//! leak (three window handles really are three different types), and
//! hiding it behind extra abstraction is usually worse than writing one
//! branch per OS in the one place it matters. Prefer moving such a thing
//! into this layer only when the knowledge itself is platform knowledge
//! and this module is its natural home (as with `style_notice` /
//! `style_overlay`, which take the Tauri window because "which handle does
//! this OS use" is exactly the sort of fact that belongs in here). Do not
//! go hunting for `cfg` to delete: an earlier reading of this paragraph as
//! a blanket ban sent a refactor through two shipping platforms to avoid a
//! single branch.
//!
//! [260725-macos-port.md]: ../../docs/plans/260725-macos-port.md

pub mod types;

/// The shell theme at this instant: what `theme_snapshot()` reports.
pub struct ThemeSnapshot {
    /// Whether the surface our icons sit on (taskbar / menu bar) is light.
    pub is_light: bool,
    /// The OS accent color, with a platform-appropriate fallback baked in.
    pub accent_rgb: [u8; 3],
}

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;

