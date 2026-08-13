//! Settings-backed commands: config get/save, the scoped reset, the
//! update guard, audio-device lists, the export-filename preview, and the
//! folder openers.

use embral_types::{AppConfig, AppError};
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

use crate::AppState;

use super::support::*;

#[tauri::command]
pub async fn get_config() -> Result<AppConfig, AppError> {
    crate::config::load_config().map_err(AppError::internal)
}

/// Restore every setting to its default (onboarding included, so it runs
/// again on the next frontend config load). Meetings, profiles, and
/// downloaded models are untouched.
#[derive(serde::Deserialize)]
pub struct ResetScopes {
    pub settings: bool,
    pub meetings: bool,
    pub profiles: bool,
    pub dictations: bool,
    pub models: bool,
}

/// Whether installing an update (which restarts the app) is safe right
/// now. Returns the human-readable reason to wait, or `None` when clear;
/// the updater UI refuses the restart while any of these are live, so an
/// update can never kill a recording, a dictation, an import, or a voice
/// enrollment mid-flight.
#[tauri::command]
pub async fn update_guard(state: State<'_, AppState>) -> Result<Option<AppError>, AppError> {
    use std::sync::atomic::Ordering;
    if state.recorder.lock().await.is_some() {
        return Ok(Some(AppError::RecordingInProgress));
    }
    if state.dictating.load(Ordering::Acquire) {
        return Ok(Some(AppError::DictationInProgress));
    }
    if state.importing.load(Ordering::Acquire) {
        return Ok(Some(AppError::ImportInProgress));
    }
    Ok(None)
}

/// The scoped reset behind About → Reset…: each flag deletes one body of
/// data outright: config to defaults, meetings (rows + their files),
/// speaker profiles, dictation history, downloaded models.
/// Refused while anything is using the mic; no scope is reversible.
#[tauri::command]
pub async fn reset_app_data(
    scopes: ResetScopes,
    state: State<'_, AppState>,
) -> Result<AppConfig, AppError> {
    if state.recorder.lock().await.is_some() {
        return Err(AppError::StopRecordingBeforeReset);
    }
    if state.dictating.load(std::sync::atomic::Ordering::Acquire) {
        return Err(AppError::StopDictatingBeforeReset);
    }

    let config = state.config.lock().await.clone();
    let base = crate::storage::storage_base(&config.storage_dir);

    if scopes.meetings || scopes.profiles || scopes.dictations {
        let db = state.db().await?;

        if scopes.meetings {
            // Files first (the rows carry the paths), then the rows, then
            // the index the MCP servers read.
            for row in db.list_meetings(None, None).map_err(|e| e.to_string())? {
                remove_indexed_file(&base, &row.audio_path)?;
                crate::commands::remove_meeting_assets(&base, &row.id);
            }
            let n = db.clear_meetings().map_err(|e| e.to_string())?;
            crate::storage::export_index(&db, &base).map_err(|e| e.to_string())?;
            tracing::info!(removed = n, "reset cleared meetings");
        }

        if scopes.profiles {
            db.clear_speakers().map_err(|e| e.to_string())?;
            // Voice clips are no longer recorded; sweep any left behind by
            // older versions.
            let _ = std::fs::remove_dir_all(base.join("voices"));
            tracing::info!("reset cleared speaker profiles");
        }

        if scopes.dictations {
            let n = db.clear_dictations().map_err(|e| e.to_string())?;
            tracing::info!(removed = n, "reset cleared dictation history");
        }

        if scopes.meetings && scopes.dictations {
            // Nothing owns a chunk anymore; drop the whole search index.
            if let Err(e) = embral_search::clear_index(&db) {
                tracing::warn!("reset couldn't clear the search index: {e:#}");
            }
        } else if scopes.meetings || scopes.dictations {
            crate::search_index::after_delete(&db);
        }
    }

    if scopes.models {
        // The LLM sidecar holds its weights open; release before deleting;
        // same for the embedding worker and its model files.
        state.llm.shutdown();
        state.search.shutdown().await;
        for model in embral_engine::catalog::MODELS {
            state.engine.evict(model.id);
            if let Err(e) = embral_engine::catalog::delete(model.id) {
                tracing::warn!(model = model.id, "reset couldn't delete model: {e}");
            }
        }
        tracing::info!("reset cleared downloaded models");
    }

    if scopes.settings {
        #[allow(unused_mut)]
        let mut fresh = AppConfig::default();
        // Defaults have telemetry on; the reset severed the old identity,
        // so a fresh install id is created like a first boot, and the sync
        // mirror follows the default rather than assuming "off".
        #[cfg(feature = "cloud")]
        {
            fresh.telemetry_install_id = uuid::Uuid::new_v4().to_string();
            state
                .telemetry
                .enabled
                .store(fresh.telemetry_enabled, std::sync::atomic::Ordering::Release);
        }
        crate::config::save_config(&fresh).map_err(|e| e.to_string())?;
        *state.config.lock().await = fresh;
    }

    Ok(state.config.lock().await.clone())
}

/// The cloud session is server-managed state: the frontend never writes it
/// (sign-in/out mutate config directly in `cloud::commands`), so whatever a
/// save payload carries for these fields is stale by definition. A settings
/// draft snapshotted before a sign-in used to blank the token here; the
/// "randomly signed out" bug.
#[cfg(feature = "cloud")]
fn preserve_server_fields(incoming: &mut AppConfig, current: &AppConfig) {
    incoming.cloud_session_token = current.cloud_session_token.clone();
    incoming.cloud_account_email = current.cloud_account_email.clone();
}

/// `config` is mutated only in cloud builds (`preserve_server_fields` and
/// the telemetry install id), so the offline build sees a `mut` it never
/// uses.
#[cfg_attr(not(feature = "cloud"), allow(unused_mut))]
#[tauri::command]
pub async fn save_config(
    app: AppHandle,
    mut config: AppConfig,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    let hotkeys_changed = {
        let current = state.config.lock().await;
        #[cfg(feature = "cloud")]
        preserve_server_fields(&mut config, &current);
        current.record_hotkey != config.record_hotkey
            || current.dictation_hotkey != config.dictation_hotkey
    };
    // The telemetry install id (cloud edition) lives and dies with the
    // opt-in: minted when enabled without one, cleared (with the snapshot
    // date) on opt-out so opting out genuinely severs history
    // ([telemetry.md]).
    #[cfg(feature = "cloud")]
    {
        if config.telemetry_enabled && config.telemetry_install_id.is_empty() {
            config.telemetry_install_id = uuid::Uuid::new_v4().to_string();
        }
        if !config.telemetry_enabled {
            config.telemetry_install_id.clear();
            config.telemetry_last_snapshot.clear();
        }
        state
            .telemetry
            .enabled
            .store(config.telemetry_enabled, std::sync::atomic::Ordering::Release);
    }
    crate::config::save_config(&config).map_err(|e| e.to_string())?;
    // A moved library needs the webview's read permission to move with it,
    // or its audio and note images stop loading until the next launch.
    crate::storage::allow_asset_access(
        &app,
        &crate::storage::storage_base(&config.storage_dir),
    );
    let record = config.record_hotkey.clone();
    let dictation = config.dictation_hotkey.clone();
    // A changed recording-disc override applies on the spot.
    crate::tray::set_recording_color(&config.tray_recording_color);
    *state.config.lock().await = config;
    let _ = crate::tray::refresh(&app);
    if hotkeys_changed {
        // Surface an invalid combo to the settings UI; config stays saved so
        // the user can correct it.
        crate::hotkey::apply(&app, &record, &dictation)?;
    }
    Ok(())
}

/// Render the export filename template against a sample meeting, for the live
/// preview in Settings. Uses the same Rust renderer as real exports so the
/// preview can't drift.
#[tauri::command]
pub async fn preview_export_filename(template: String) -> Result<String, AppError> {
    let sample_time = chrono::Utc::now();
    let stem = embral_notes::integrations::render_filename(&template, "Weekly sync", &sample_time);
    Ok(format!("{stem}.md"))
}

/// Send a sample payload to one webhook destination so users can verify an
/// endpoint without recording a meeting. Takes the row's values directly;
/// race-free against the settings autosave debounce, and testable before
/// the row is ever saved.
#[tauri::command]
pub async fn test_webhook(
    url: String,
    method: embral_types::WebhookMethod,
    include_content: bool,
) -> Result<(), AppError> {
    let url = url.trim().to_string();
    if url.is_empty() {
        return Err(AppError::WebhookTestFailed {
            detail: "the URL is empty".to_string(),
        });
    }
    let record = embral_types::MeetingRecord {
        id: "000000T000000_sample".to_string(),
        title: "Webhook test from embral".to_string(),
        date: chrono::Utc::now(),
        duration_seconds: 60,
        chunks: 1,
        audio_path: String::new(),
    };
    let content = embral_notes::integrations::WebhookContent {
        summary_markdown: "# Webhook test\n\nA sample summary sent from embral settings.",
        notes_markdown: "Sample notes taken during the meeting.",
        transcript_markdown: "Sample transcript content.",
    };
    let payload =
        embral_notes::integrations::webhook_payload(&record, include_content.then_some(&content));
    embral_notes::integrations::send_webhook(&url, method, &payload)
        .await
        .map_err(|e| AppError::WebhookTestFailed {
            detail: format!("{e:#}"),
        })
}

/// Names of the machine's audio devices, for the Settings pickers. An empty
/// selection in config means "system default", so these lists are additive.
#[derive(serde::Serialize)]
pub struct AudioDevices {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
}

#[tauri::command]
pub async fn list_audio_devices() -> Result<AudioDevices, AppError> {
    // Device enumeration can block on driver calls; keep it off the runtime.
    tokio::task::spawn_blocking(|| {
        use cpal::traits::{DeviceTrait, HostTrait};
        fn names<I: Iterator<Item = cpal::Device>, E>(devices: Result<I, E>) -> Vec<String> {
            devices
                .map(|it| it.filter_map(|d| d.name().ok()).collect())
                .unwrap_or_default()
        }
        let host = cpal::default_host();
        AudioDevices {
            inputs: names(host.input_devices()),
            outputs: names(host.output_devices()),
        }
    })
    .await
    .map_err(AppError::internal)
}

/// What a running recording could capture: the apps currently playing
/// audio and the machine's microphones ([recording.md] §Dual-stream
/// capture). Drives the source picker, which polls this while it is open;
/// apps come and go mid-call.
#[derive(serde::Serialize)]
pub struct AudioSources {
    pub apps: Vec<AudioApp>,
    pub mics: Vec<String>,
    /// The recording's primary mic, resolved: the configured device, or
    /// the system default when config says "default". It owns the master
    /// clock, so the picker shows it checked and locked.
    pub primary_mic: String,
}

#[derive(serde::Serialize)]
pub struct AudioApp {
    pub pid: u32,
    pub name: String,
}

#[tauri::command]
pub async fn list_audio_sources(state: State<'_, AppState>) -> Result<AudioSources, AppError> {
    let own_pid = std::process::id();
    let configured = state.config.lock().await.mic_device.clone();
    tokio::task::spawn_blocking(move || {
        use cpal::traits::{DeviceTrait, HostTrait};
        let host = cpal::default_host();
        let apps = crate::platform::apps_playing_audio(own_pid)
            .into_iter()
            .map(|a| AudioApp {
                pid: a.pid,
                name: a.label().to_string(),
            })
            .collect();
        let mics: Vec<String> = host
            .input_devices()
            .map(|it| it.filter_map(|d| d.name().ok()).collect())
            .unwrap_or_default();
        // An empty config means the system default; a configured device
        // that has vanished falls back to it too, exactly like capture.
        let default_mic = host
            .default_input_device()
            .and_then(|d| d.name().ok())
            .unwrap_or_default();
        let primary_mic = match configured.trim() {
            "" => default_mic,
            name if mics.iter().any(|m| m == name) => name.to_string(),
            _ => default_mic,
        };
        AudioSources {
            apps,
            mics,
            primary_mic,
        }
    })
    .await
    .map_err(AppError::internal)
}

#[tauri::command]
pub async fn open_logs_folder<R: tauri::Runtime>(app: AppHandle<R>) -> Result<(), AppError> {
    let logs = crate::logs_dir();
    let _ = std::fs::create_dir_all(&logs);
    app.opener()
        .open_path(logs.to_string_lossy().to_string(), None::<&str>)
        .map_err(AppError::internal)
}

#[tauri::command]
pub async fn open_notes_folder<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    let config = state.config.lock().await;
    let base = crate::storage::storage_base(&config.storage_dir);
    drop(config);
    crate::telemetry::track(&state, "notes_folder_opened", serde_json::json!({}));
    // The storage root, not a `notes/` subdirectory. Notes and transcripts
    // were markdown files under `notes/` until schema v11 made them database
    // columns ([storage.md]); `init_storage_dirs` has created only `audio/`
    // and `assets/` ever since, so the old path opened nothing. This is the
    // folder the recovery button on a failed meeting has to reach.
    let _ = std::fs::create_dir_all(&base);
    app.opener()
        .open_path(base.to_string_lossy().to_string(), None::<&str>)
        .map_err(AppError::internal)
}

#[cfg(all(test, feature = "cloud"))]
mod tests {
    use super::*;

    #[test]
    fn a_stale_save_payload_cannot_clobber_the_session() {
        // The settings draft snapshots config when Settings opens; a sign-in
        // after that snapshot must survive any later save from it.
        let mut current = AppConfig::default();
        current.cloud_session_token = "tok".into();
        current.cloud_account_email = "a@b.com".into();

        let mut incoming = AppConfig::default();
        incoming.cloud_session_token = String::new();
        incoming.cloud_account_email = String::new();
        incoming.detection_delay_secs = 9; // a real edit rides along

        preserve_server_fields(&mut incoming, &current);
        assert_eq!(incoming.cloud_session_token, "tok");
        assert_eq!(incoming.cloud_account_email, "a@b.com");
        assert_eq!(incoming.detection_delay_secs, 9);
    }
}
