//! Linux implementations of the platform contract (see `platform/mod.rs`).
//!
//! The port's third sibling directory ([260801-linux-port.md]). Landing over
//! the plan's phases: the modules that are real from the start are the ones
//! with a plain POSIX or freedesktop answer (supervisor, proc, power,
//! os_build, mcp_paths, permissions, ocr); audio, input, and theme arrive in
//! their own phases and are inert until then. A stub means "feature absent",
//! never a crash; callers degrade by contract.
//!
//! Two contract items are deliberately not Linux capabilities at all, and
//! their modules say so rather than promising a later phase: there is no
//! in-box OCR engine to reach for, and Claude Desktop has no Linux build.
//!
//! [260801-linux-port.md]: ../../../../docs/plans/260801-linux-port.md

pub mod audio_apps;
pub mod input;
pub mod loopback;
pub mod mcp_paths;
pub mod mic_users;
pub mod notice;
pub mod ocr;
pub mod os_build;
pub mod overlay;
pub mod permissions;
pub mod power;

pub mod proc;
pub mod pulse;
pub mod supervisor;
pub mod theme;

pub use audio_apps::apps_playing_audio;
pub use input::{focused_app, paste_keystroke};
pub use loopback::SystemAudioCapture;
pub use mic_users::processes_using_microphone;
pub use notice::style_notice;
pub use ocr::recognize_text;
#[cfg_attr(not(feature = "cloud"), allow(unused_imports))]
pub use os_build::os_build;
pub use overlay::style_overlay;
pub use power::power_source;
pub use proc::{exe_name, find_cli, hide_console, hide_console_tokio};
pub use supervisor::{
    kill_children_with_us, prepare_spawn, prepare_spawn_tokio, run_reaper, watch_child,
};
pub use theme::{theme_snapshot, watch_theme};

/// Linux tray icons are plain images delivered by file path through
/// libayatana-appindicator; there is no template/recolor protocol, so the
/// panel shade drives which mark is shown instead.
pub const TRAY_IDLE_IS_TEMPLATE: bool = false;

/// StatusNotifier items are menu-only here, and that is **measured, not
/// assumed (2026-08-02, Cinnamon): the item libayatana-appindicator
/// publishes exposes `Scroll` and `SecondaryActivate` but no `Activate`
/// method at all, and calling it returns `UnknownMethod`. `Activate` is what a host
/// invokes on a primary click, so there is nothing for Cinnamon to call and
/// no click event can reach the app however this flag is set.
///
/// Which means setting this `false` to get Windows' left-click-opens-window
/// would leave left-click doing nothing at all, strictly worse than a
/// menu. The menu is therefore the whole interaction, with "Show Window"
/// first in it. The upstream path, if this ever matters enough: Tauri moving
/// to `libayatana-appindicator-glib`, which the current library's own
/// deprecation warning points at.
pub const TRAY_MENU_ON_LEFT_CLICK: bool = true;
