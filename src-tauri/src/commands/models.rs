//! Local model management (the sherpa-onnx engine catalog) plus the LLM
//! sidecar status and the summary-prompt parts the settings editor reads.

use embral_types::AppError;
use tauri::{AppHandle, Emitter, State};

use crate::AppState;

// --- Local model management (sherpa-onnx engine catalog) ---

#[tauri::command]
pub async fn asr_models_status() -> Result<Vec<embral_engine::ModelStatus>, AppError> {
    Ok(embral_engine::catalog::statuses())
}

/// The two halves of the summary prompt for the settings editor: the
/// editable default body and the locked output contract.
#[tauri::command]
pub async fn get_summary_prompt_parts() -> Result<serde_json::Value, AppError> {
    Ok(serde_json::json!({
        "default": embral_notes::prompt::DEFAULT_SUMMARY_PROMPT,
        "contract": embral_notes::prompt::OUTPUT_CONTRACT,
    }))
}


/// Download one catalog model, emitting `model-download-progress` throughout
/// and `model-download-complete` at the end. Concurrent downloads of the same
/// model are rejected; different models may download in parallel.
#[tauri::command]
pub async fn download_asr_model(
    app: AppHandle,
    state: State<'_, AppState>,
    model_id: String,
) -> Result<(), AppError> {
    {
        let mut in_flight = state.model_downloads.lock().expect("downloads mutex");
        if !in_flight.insert(model_id.clone()) {
            return Err(AppError::AlreadyDownloading);
        }
    }
    struct Guard<'a> {
        set: &'a std::sync::Mutex<std::collections::HashSet<String>>,
        id: String,
    }
    impl Drop for Guard<'_> {
        fn drop(&mut self) {
            self.set.lock().expect("downloads mutex").remove(&self.id);
        }
    }
    let _guard = Guard {
        set: &state.model_downloads,
        id: model_id.clone(),
    };

    // The sidecar holds the runtime exe/DLLs and the weights open; extracting
    // or renaming over them fails on Windows. Stop it first; it restarts on
    // next use.
    if matches!(model_id.as_str(), "llama-server" | "qwen3-4b") {
        state.llm.shutdown();
    }

    let app_progress = app.clone();
    if let Err(e) = embral_engine::catalog::download(&model_id, move |p| {
        let _ = app_progress.emit("model-download-progress", &p);
    })
    .await
    {
        crate::telemetry::track(
            &state,
            "error",
            serde_json::json!({ "category": "model_download_failed" }),
        );
        return Err(AppError::internal(e));
    }

    crate::telemetry::track(
        &state,
        "model_downloaded",
        serde_json::json!({ "model_id": model_id }),
    );
    let _ = app.emit(
        "model-download-complete",
        serde_json::json!({ "model_id": model_id }),
    );
    Ok(())
}

#[tauri::command]
pub async fn delete_asr_model(
    app: AppHandle,
    state: State<'_, AppState>,
    model_id: String,
) -> Result<(), AppError> {
    if model_id == embral_search::model::MODEL_ID {
        // The embed worker holds the model files open; release them first.
        state.search.shutdown().await;
    }
    // Same for the LLM sidecar: deleting the runtime or weights under a
    // running llama-server leaves NTFS delete-pending files that block
    // every re-download until the process dies.
    if matches!(model_id.as_str(), "llama-server" | "qwen3-4b") {
        state.llm.shutdown();
    }
    embral_engine::catalog::delete(&model_id).map_err(|e| e.to_string())?;
    // Drop any warm recognizer so a re-download loads fresh files.
    state.engine.evict(&model_id);
    crate::telemetry::track(
        &state,
        "model_deleted",
        serde_json::json!({ "model_id": model_id }),
    );
    let _ = app.emit(
        "model-download-complete",
        serde_json::json!({ "model_id": model_id }),
    );
    Ok(())
}
