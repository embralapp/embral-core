//! Dev-only fixture commands for staged screenshot moments
//! (configuration.md §`EMBRAL_DATA_DIR`): both are inert unless the
//! data-dir override is set, so every normal launch gets `None` / an
//! error and no shipped behavior changes.

use embral_types::AppError;
use tauri::AppHandle;

fn fixture_dir() -> Option<std::path::PathBuf> {
    std::env::var("EMBRAL_DATA_DIR")
        .ok()
        .filter(|d| !d.trim().is_empty())
        .map(std::path::PathBuf::from)
}

/// The staged-moment fixture, when the sandbox provides one
/// (`{EMBRAL_DATA_DIR}/fixture.json`). The frontend hydrates the
/// live-recording view or the dictation overlay from it.
#[tauri::command]
pub async fn fixture_state() -> Result<Option<serde_json::Value>, AppError> {
    let Some(dir) = fixture_dir() else {
        return Ok(None);
    };
    let Ok(text) = std::fs::read_to_string(dir.join("fixture.json")) else {
        return Ok(None);
    };
    Ok(serde_json::from_str(&text).ok())
}

/// Surface the dictation overlay without a session, so a staged overlay
/// moment can render and be captured.
#[tauri::command]
pub async fn fixture_show_overlay(app: AppHandle) -> Result<(), AppError> {
    if fixture_dir().is_none() {
        return Err("fixture commands are dev-only (EMBRAL_DATA_DIR)"
            .to_string()
            .into());
    }
    crate::dictation::show_overlay(&app)
}
