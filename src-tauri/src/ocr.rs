//! Reading the text out of a meeting's pasted images and keeping it true to
//! what is on disk ([storage.md](../../docs/storage.md) §The chunk index).
//!
//! The OS call is behind the platform layer; this module owns the when and
//! the what: which files still need reading, what gets stored, and the
//! background sweep that catches everything finalize did not.
//!
//! The sweep is also the backfill. It works by diffing `assets/{id}/`
//! against the stored rows, so images that predate this feature are picked
//! up on the next pass with no boot-time special case.

use std::path::{Path, PathBuf};

use embral_db::Db;
use embral_notes::assets;

use crate::platform::types::Recognized;

/// Images read per sweep pass. Small, like the embedding batch, so the
/// database lock is never held long and a first-run backfill of a large
/// library stays in the background where it belongs.
pub const SWEEP_BATCH: usize = 8;

/// Which engine wrote a passage. Recorded so a library that has moved
/// between machines says where each reading came from: Vision and
/// `Windows.Media.Ocr` are not equally good, and the column is the only
/// place that difference is visible after the fact.
fn engine_name() -> &'static str {
    std::env::consts::OS
}

/// One image, start to finish. A file we cannot read is `Failed` rather
/// than `Unavailable`: it is an answer about this file, so the caller
/// retires it instead of retrying it forever.
fn read_file(path: &Path) -> Recognized {
    match std::fs::read(path) {
        Ok(bytes) => crate::platform::recognize_text(&bytes),
        Err(e) => Recognized::Failed(format!("read {}: {e}", path.display())),
    }
}

/// Read these images and hand back what each one says, in the order given.
///
/// Storing is deliberately not done here: at finalize the meeting row does
/// not exist yet, so the caller decides when the rows can be written. A
/// `Failed` image comes back with empty text (an answer, so it is stamped
/// and not retried); `Unavailable` stops the run, because nothing after it
/// would fare any better.
pub fn read_images(base: &Path, meeting_id: &str, filenames: &[String]) -> Vec<(String, String)> {
    let Some(dir) = meeting_asset_dir(base, meeting_id) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for filename in filenames {
        match read_file(&dir.join(filename)) {
            Recognized::Text(text) => out.push((filename.clone(), text)),
            Recognized::Failed(why) => {
                tracing::debug!("could not read {meeting_id}/{filename}: {why}");
                out.push((filename.clone(), String::new()));
            }
            Recognized::Unavailable => {
                tracing::info!("no OCR engine on this machine; images stay unread");
                break;
            }
        }
    }
    out
}

/// Record what was read. Indexing is the caller's next move.
pub fn store(db: &Db, meeting_id: &str, readings: &[(String, String)]) {
    for (filename, text) in readings {
        if let Err(e) = db.set_image_text(meeting_id, filename, text, engine_name()) {
            tracing::warn!("storing image text for {meeting_id}/{filename} failed: {e:#}");
        }
    }
}

/// The images in this meeting's asset directory, by name. Empty when the
/// meeting never had one, which is most of them.
pub fn stored_images(base: &Path, meeting_id: &str) -> Vec<String> {
    let Some(dir) = meeting_asset_dir(base, meeting_id) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    names
}

fn meeting_asset_dir(base: &Path, meeting_id: &str) -> Option<PathBuf> {
    let rel = assets::asset_dir_rel(meeting_id);
    crate::commands::resolve_indexed_path(base, &rel).ok()
}

/// Read up to `budget` images that nothing has read yet, across the whole
/// library, and re-index every meeting that gained text.
///
/// Only meetings that exist are considered: a live recording has no row
/// yet, which is exactly the behaviour we want. OCR must not compete with
/// transcription for the CPU while the meeting is still running.
///
/// Returns how many images were read, so the caller knows whether to come
/// straight back for more.
pub fn sweep(db: &Db, base: &Path, budget: usize) -> usize {
    let dir = base.join("assets");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return 0;
    };
    let mut read = 0usize;

    for entry in entries.flatten() {
        if read >= budget {
            break;
        }
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let meeting_id = entry.file_name().to_string_lossy().to_string();
        match db.get_meeting(&meeting_id) {
            Ok(Some(_)) => {}
            // No row: a live recording, or an orphan the janitor will remove.
            Ok(None) => continue,
            Err(e) => {
                tracing::warn!("looking up {meeting_id} failed: {e:#}");
                continue;
            }
        }

        let known = db.image_text_filenames(&meeting_id).unwrap_or_default();
        let pending: Vec<String> = stored_images(base, &meeting_id)
            .into_iter()
            .filter(|name| !known.iter().any(|k| k == name))
            .take(budget - read)
            .collect();
        if pending.is_empty() {
            continue;
        }

        let readings = read_images(base, &meeting_id, &pending);
        if readings.is_empty() {
            // The engine is unavailable; the rest of the library will fare
            // no better this pass.
            break;
        }
        read += readings.len();
        store(db, &meeting_id, &readings);
        if let Err(e) = embral_search::sync_meeting(db, &meeting_id) {
            tracing::warn!("re-indexing {meeting_id} after OCR failed: {e:#}");
        }
    }

    if read > 0 {
        tracing::info!("read text out of {read} image(s)");
    }
    read
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The smallest valid PNG: 1×1, transparent. It has no text in it, so
    /// the engine answers with an empty string, which is still an answer,
    /// and the point of the test is what happens to the row afterwards.
    const TINY_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    /// Whether this platform has an OCR engine at all. Windows and macOS
    /// read with a built-in one; Linux ships none by design
    /// (`platform/linux/ocr.rs`), and `Recognized::Unavailable` means the
    /// sweep stamps nothing and leaves images pending. The two sweep tests
    /// below assert both shapes rather than skipping on the stub platform,
    /// so "the engine is missing" stays a tested behavior and not a hole.
    fn ocr_available() -> bool {
        !matches!(
            crate::platform::recognize_text(TINY_PNG),
            crate::platform::types::Recognized::Unavailable
        )
    }

    fn meeting(id: &str) -> embral_db::MeetingRow {
        embral_db::MeetingRow {
            id: id.to_string(),
            title: "Planning".to_string(),
            started_at: chrono::Utc::now(),
            duration_seconds: 60,
            summary: String::new(),
            transcript: String::new(),
            attendees: Vec::new(),
            audio_path: String::new(),
        }
    }

    /// The sweep reads what is on disk but has no row yet, leaves the live
    /// recording alone, and does not read the same image twice.
    #[test]
    fn the_sweep_reads_once_and_skips_meetings_that_have_no_row() {
        let base = std::env::temp_dir().join(format!("embral-ocr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("assets/m1")).unwrap();
        // `live` stands in for the recording in flight: its images exist,
        // its meeting row does not.
        std::fs::create_dir_all(base.join("assets/live")).unwrap();
        std::fs::write(base.join("assets/m1/img-01.png"), TINY_PNG).unwrap();
        std::fs::write(base.join("assets/live/img-01.png"), TINY_PNG).unwrap();

        let db = Db::open_in_memory().unwrap();
        db.upsert_meeting(&meeting("m1")).unwrap();

        if !ocr_available() {
            // No engine here: nothing is stamped, so the image stays pending
            // and a platform that later gains an engine would still find it. The
            // sweep is a no-op rather than a destructive one, which is the
            // whole of the contract on this platform.
            assert_eq!(sweep(&db, &base, SWEEP_BATCH), 0);
            assert!(db.image_text_filenames("m1").unwrap().is_empty());
            let _ = std::fs::remove_dir_all(&base);
            return;
        }

        assert_eq!(sweep(&db, &base, SWEEP_BATCH), 1);
        assert_eq!(db.image_text_filenames("m1").unwrap(), vec!["img-01.png"]);
        assert!(db.image_text_filenames("live").unwrap().is_empty());

        // A second pass has nothing left to do; the stamp is what says so,
        // even though the reading itself was empty.
        assert_eq!(sweep(&db, &base, SWEEP_BATCH), 0);

        // A newly pasted image is picked up on the next pass.
        std::fs::write(base.join("assets/m1/img-02.png"), TINY_PNG).unwrap();
        assert_eq!(sweep(&db, &base, SWEEP_BATCH), 1);
        assert_eq!(db.image_text_filenames("m1").unwrap().len(), 2);

        let _ = std::fs::remove_dir_all(&base);
    }

    /// A file that is not an image is an answer about that file: stamped so
    /// the sweep stops coming back to it, with nothing to index.
    #[test]
    fn a_file_that_is_not_an_image_is_retired_rather_than_retried() {
        let base = std::env::temp_dir().join(format!("embral-ocr-junk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("assets/m1")).unwrap();
        std::fs::write(base.join("assets/m1/notes.txt"), b"not an image").unwrap();

        let db = Db::open_in_memory().unwrap();
        db.upsert_meeting(&meeting("m1")).unwrap();

        if !ocr_available() {
            // Without an engine there is no way to tell "not an image" from
            // "not read yet", so nothing is retired; the file stays pending
            // rather than being wrongly stamped as answered.
            assert_eq!(sweep(&db, &base, SWEEP_BATCH), 0);
            assert!(db.image_text("m1").unwrap().is_empty());
            let _ = std::fs::remove_dir_all(&base);
            return;
        }

        assert_eq!(sweep(&db, &base, SWEEP_BATCH), 1);
        assert_eq!(db.image_text("m1").unwrap(), vec![("notes.txt".to_string(), String::new())]);
        assert_eq!(sweep(&db, &base, SWEEP_BATCH), 0);

        let _ = std::fs::remove_dir_all(&base);
    }
}
