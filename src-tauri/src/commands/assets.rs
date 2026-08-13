//! Storing an image pasted into one of a meeting's documents
//! ([storage.md](../../../docs/storage.md) §Layout).

use embral_notes::assets;
use embral_types::AppError;
use std::io::Write;
use tauri::State;

use crate::AppState;

/// Save pasted image bytes under the meeting's asset directory and return
/// the storage-root-relative link the markdown should carry.
///
/// The bytes arrive as a raw IPC body, not a JSON argument. Tauri
/// serializes command arguments as JSON, where a `Vec<u8>` becomes an array
/// of integers, roughly four times the size plus a parse, which on a
/// full-screen screenshot is a visible freeze of the window. There is no
/// size cap: images are stored exactly as pasted (owner's call: lossless
/// always), which makes the transport choice matter more, not less.
///
/// The meeting id rides in the `x-meeting-id` header. Absent means "the
/// recording happening now", read from the recovery scratch, because the
/// live notes editor has no meeting id of its own; the recording owns it.
#[tauri::command]
pub async fn save_note_asset(
    request: tauri::ipc::Request<'_>,
    state: State<'_, AppState>,
) -> Result<String, AppError> {
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err(AppError::internal("image bytes must be sent as a raw body"));
    };

    // The claimed type is not evidence; the bytes are. This also refuses
    // anything that is not an image before it reaches the disk.
    let Some(ext) = assets::sniff_image_ext(bytes) else {
        return Err(AppError::internal(
            "that does not look like a PNG, JPEG, GIF or WebP",
        ));
    };

    let config = state.config.lock().await.clone();
    let base = crate::storage::storage_base(&config.storage_dir);

    let meeting_id = match request.headers().get("x-meeting-id") {
        Some(value) => value
            .to_str()
            .map_err(|_| AppError::internal("x-meeting-id is not valid text"))?
            .to_string(),
        None => crate::recovery::active_meeting_id(&base)
            .ok_or_else(|| AppError::internal("no meeting is recording to attach this to"))?,
    };
    // The id names a directory, so it may not be a path itself.
    if meeting_id.is_empty() || meeting_id.contains(['/', '\\', '.', ':']) {
        return Err(AppError::internal("that is not a meeting id"));
    }

    let dir_rel = assets::asset_dir_rel(&meeting_id);
    let dir = crate::commands::resolve_indexed_path(&base, &dir_rel)?;
    std::fs::create_dir_all(&dir).map_err(AppError::internal)?;

    // Allocate by creating the file, not by checking whether a name is
    // free: two fast pastes both scan an empty directory and both pick
    // img-01. `create_new` makes the winner unambiguous and the loser
    // tries the next number.
    let mut attempt = 0;
    loop {
        let existing: Vec<String> = std::fs::read_dir(&dir)
            .map_err(AppError::internal)?
            .filter_map(|entry| Some(entry.ok()?.file_name().to_string_lossy().to_string()))
            .collect();
        let name = assets::next_asset_name(&existing, ext);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join(&name))
        {
            Ok(mut file) => {
                file.write_all(bytes).map_err(AppError::internal)?;
                let link = assets::link_rel(&meeting_id, &name);
                tracing::info!(
                    "saved {} bytes of {ext} for {meeting_id} as {link}",
                    bytes.len()
                );
                return Ok(link);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && attempt < 50 => {
                attempt += 1;
            }
            Err(e) => return Err(AppError::internal(e)),
        }
    }
}

/// The storage directory as an absolute path, so the frontend can turn a
/// stored relative link into something the webview can load. Deliberately a
/// command rather than a field on `AppConfig`: the config holds what the
/// user chose (which may be `~/embral`), not its resolution.
#[tauri::command]
pub async fn storage_root(state: State<'_, AppState>) -> Result<String, AppError> {
    let config = state.config.lock().await.clone();
    Ok(crate::storage::storage_base(&config.storage_dir)
        .to_string_lossy()
        .to_string())
}
