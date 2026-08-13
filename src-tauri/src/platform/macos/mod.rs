//! macOS implementations of the platform contract (see `platform/mod.rs`).
//!
//! Stubs today: each capability returns its inert value until the port
//! phase that implements it lands ([260725-macos-port.md]). Callers degrade
//! gracefully by contract, so a stub means "feature absent", never a crash.
//!
//! [260725-macos-port.md]: ../../../docs/plans/260725-macos-port.md

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
pub mod supervisor;
pub mod theme;

pub use audio_apps::apps_playing_audio;
pub use input::{focused_app, paste_keystroke};
pub use loopback::SystemAudioCapture;
pub use notice::style_notice;
pub use ocr::recognize_text;
pub use overlay::style_overlay;
pub use mic_users::processes_using_microphone;
#[cfg_attr(not(feature = "cloud"), allow(unused_imports))]
pub use os_build::os_build;
pub use power::power_source;
pub use proc::{exe_name, find_cli, hide_console, hide_console_tokio};
pub use supervisor::{
    kill_children_with_us, prepare_spawn, prepare_spawn_tokio, run_reaper, watch_child,
};
pub use theme::{theme_snapshot, watch_theme};

/// The idle menu-bar icon is a template image: the system recolors it from
/// its alpha channel, so it always matches the menu bar. (The recording
/// tint temporarily switches template off to keep its accent color.)
pub const TRAY_IDLE_IS_TEMPLATE: bool = true;

/// A menu-bar item opens its menu on click (the platform convention),
/// so the left-click window toggle never fires here.
pub const TRAY_MENU_ON_LEFT_CLICK: bool = true;
