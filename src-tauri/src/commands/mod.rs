//! Tauri commands, split by surface. Every command and shared helper is
//! re-exported here so `crate::commands::<name>` keeps resolving for
//! `lib.rs`'s handler list and for `storage.rs` / `speaker_commands.rs`.

mod assets;
mod dictation;
mod finalize;
mod fixture;
mod import;
mod meetings;
mod models;
mod permissions;
mod recording;
mod settings;
mod support;

pub use assets::*;
pub use dictation::*;
pub use fixture::*;
pub use import::*;
pub use meetings::*;
pub use models::*;
pub use permissions::*;
pub use recording::*;
pub use settings::*;
pub use support::*;
