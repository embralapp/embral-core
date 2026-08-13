//! Windows implementations of the platform contract (see `platform/mod.rs`).

pub mod audio_apps;
pub mod input;
pub mod loopback;
pub mod mcp_paths;
pub mod mic_users;
pub mod notice;
pub mod ocr;
pub mod os_build;
pub mod overlay;
pub mod power;
pub mod process_loopback;
pub mod permissions;
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

/// Windows tray icons are plain images; the taskbar shade drives which
/// mark is shown instead.
pub const TRAY_IDLE_IS_TEMPLATE: bool = false;

/// On Windows the menu belongs to right-click only: left-click toggles
/// the window, and a menu popping on the same click races that toggle:
/// the menu flashed, or swallowed the click entirely.
pub const TRAY_MENU_ON_LEFT_CLICK: bool = false;
