//! What an interrupted recording leaves behind, and what launch does with
//! it ([recording.md](../../docs/recording.md) §Crash recovery).
//!
//! A recording holds everything that matters in memory until
//! `finalize_meeting` runs: segments in `AppState::current_segments`, the
//! notes draft in `recording_drafts`, stars in `stars`. A force-quit, a
//! panic, a power cut, or an OS kill takes all of it and leaves a WAV
//! nobody references. So the live recording mirrors those three things
//! into a scratch directory as they happen, and the next launch turns the
//! leftovers into an ordinary meeting.
//!
//! **One subdirectory per unfinalized meeting.** Each meeting's scratch
//! lives at `in_progress/<meeting_id>/` and is cleared only by its own
//! id, only after its finalize returns — so a slow finalize can never
//! destroy a successor recording's live scratch, and a recording that
//! starts while an orphan waits for rescue leaves the orphan alone.
//! `current.txt` names the recording in flight; it is what the stop
//! path, asset paste, and the janitor read.
//!
//! The scratch is deliberately *not* the database. Writing segments to
//! SQLite mid-recording would mean creating the meeting row at start, and
//! an in-progress row leaks into every listing query, the search index,
//! and the janitor. These files live until their meeting commits
//! ([storage.md](../../docs/storage.md)).

use std::path::{Path, PathBuf};

use embral_types::TranscriptionSegment;

use crate::commands::Star;

/// Bytes per second of recorded audio: 16 kHz, mono, f32.
const BYTES_PER_SEC: u64 = 16_000 * 4;

/// How much audio an interrupted recording needs before it is worth
/// keeping. Below this it is a mis-click or a start that crashed
/// immediately; recovering it would run the whole summarize pipeline over
/// nothing and leave an empty meeting in the list. The user is not asked —
/// approving your own meeting is a chore, and after a crash you may not
/// remember there was one.
const MIN_RECOVERABLE_SECS: u64 = 10;

/// How many launches may try to rescue one scratch before giving up. A
/// rescue that keeps dying is evidence the data itself crashes the
/// pipeline, and retrying it at every launch would make the app
/// unusable — but one crash mid-rescue (the app closed during the slow
/// finalize) must not silently burn the only copy of a meeting.
pub const MAX_RESCUE_ATTEMPTS: u32 = 3;

/// Everything the interrupted recording managed to write down.
pub struct Interrupted {
    pub meeting_id: String,
    pub segments: Vec<TranscriptionSegment>,
    pub user_notes: Option<String>,
    pub user_title: Option<String>,
    pub stars: Vec<Star>,
}

/// The user's own words and marks, rewritten whole on every change (they
/// are small and always superseded, unlike segments, which only ever grow).
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Drafts {
    notes: String,
    title: String,
    #[serde(default)]
    stars: Vec<f64>,
}

fn dir(base: &Path) -> PathBuf {
    base.join("in_progress")
}

fn current_file(base: &Path) -> PathBuf {
    dir(base).join("current.txt")
}

fn meeting_dir(base: &Path, meeting_id: &str) -> PathBuf {
    dir(base).join(meeting_id)
}

fn segments_file(base: &Path, meeting_id: &str) -> PathBuf {
    meeting_dir(base, meeting_id).join("segments.jsonl")
}

fn drafts_file(base: &Path, meeting_id: &str) -> PathBuf {
    meeting_dir(base, meeting_id).join("drafts.json")
}

fn attempts_file(base: &Path, meeting_id: &str) -> PathBuf {
    meeting_dir(base, meeting_id).join("attempts.txt")
}

/// Open the scratch for a recording that is starting: its own
/// subdirectory, and the current-marker the stop path reads. A leftover
/// subdirectory under the *same* id is cleared first (ids are
/// timestamp-random, so this is paranoia, not policy); anyone else's
/// scratch is none of this recording's business — an orphan waiting for
/// rescue survives every later recording.
pub fn begin(base: &Path, meeting_id: &str) {
    let own = meeting_dir(base, meeting_id);
    if own.exists() {
        if let Err(e) = std::fs::remove_dir_all(&own) {
            tracing::warn!("could not reset the recovery scratch: {e}");
        }
    }
    if let Err(e) = std::fs::create_dir_all(&own) {
        tracing::warn!("could not open the recovery scratch: {e}");
        return;
    }
    if let Err(e) = std::fs::write(current_file(base), meeting_id) {
        tracing::warn!("could not write the recovery scratch marker: {e}");
    }
}

/// Append one finalized segment to its own meeting's scratch. The caller
/// names the meeting the segment belongs to — the event forwarder pins
/// the id it was built with, so a retired stream's tail landing while a
/// successor records can never leak into the successor's scratch.
pub fn append_segment(base: &Path, meeting_id: &str, segment: &TranscriptionSegment) {
    if !meeting_dir(base, meeting_id).is_dir() {
        return;
    }
    let Ok(mut line) = serde_json::to_string(segment) else {
        return;
    };
    line.push('\n');
    use std::io::Write;
    let appended = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(segments_file(base, meeting_id))
        .and_then(|mut f| f.write_all(line.as_bytes()));
    if let Err(e) = appended {
        tracing::warn!("could not record a segment for recovery: {e}");
    }
}

/// Mirror the notes/title draft and the stars for the recording in
/// flight (drafts are the user's live typing — they always belong to the
/// current recording). Driven by the frontend's existing debounce, so
/// this is not a per-keystroke write. No recording, no write.
pub fn write_drafts(base: &Path, notes: &str, title: &str, stars: &[f64]) {
    let Some(meeting_id) = active_meeting_id(base) else {
        return;
    };
    if !meeting_dir(base, &meeting_id).is_dir() {
        return;
    }
    let drafts = Drafts {
        notes: notes.to_string(),
        title: title.to_string(),
        stars: stars.to_vec(),
    };
    let written = serde_json::to_string(&drafts)
        .map_err(std::io::Error::other)
        .and_then(|json| std::fs::write(drafts_file(base, &meeting_id), json));
    if let Err(e) = written {
        tracing::warn!("could not mirror the notes draft for recovery: {e}");
    }
}

/// One meeting's scratch is done with (its finalize returned, or its
/// rescue gave up): remove that subdirectory and nothing else. The
/// current-marker goes too when it names this id — a *different* id in
/// the marker means a successor recording already owns it.
pub fn clear_for(base: &Path, meeting_id: &str) {
    if active_meeting_id(base).as_deref() == Some(meeting_id) {
        if let Err(e) = std::fs::remove_file(current_file(base)) {
            tracing::warn!("could not clear the recovery scratch marker: {e}");
        }
    }
    let scratch = meeting_dir(base, meeting_id);
    if scratch.exists() {
        if let Err(e) = std::fs::remove_dir_all(&scratch) {
            tracing::warn!("could not clear the recovery scratch: {e}");
        }
    }
}

/// Launch found a current-marker written by a process that is dead (this
/// runs before anything in this process can record): the marker is
/// stale — retire it so the id it names is rescuable like any other
/// leftover. Its subdirectory is untouched.
pub fn clear_stale_current(base: &Path) {
    let marker = current_file(base);
    if marker.exists() {
        if let Err(e) = std::fs::remove_file(&marker) {
            tracing::warn!("could not retire the stale recording marker: {e}");
        }
    }
}

/// The in-flight meeting's id, for the stop path (and asset paste and the
/// janitor's live-recording guard).
pub fn active_meeting_id(base: &Path) -> Option<String> {
    let id = std::fs::read_to_string(current_file(base)).ok()?;
    let id = id.trim().to_string();
    (!id.is_empty()).then_some(id)
}

/// Every meeting with a scratch waiting: launch's rescue worklist.
pub fn pending(base: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir(base)) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    // Meeting ids sort chronologically, so rescues land oldest-first.
    ids.sort();
    ids
}

/// Whether an interrupted recording carries enough audio to be worth
/// keeping. Pure so the threshold is testable; the caller supplies the
/// WAV's size on disk, which is the honest measure — the header may be up
/// to one flush interval behind the samples actually written.
pub fn worth_recovering(wav_bytes: u64) -> bool {
    wav_bytes.saturating_sub(WAV_HEADER_ALLOWANCE) / BYTES_PER_SEC >= MIN_RECOVERABLE_SECS
}

/// Generous allowance for the RIFF header, so a header layout change can
/// never make a silent file look like a recoverable one.
const WAV_HEADER_ALLOWANCE: u64 = 1024;

/// Count one rescue attempt for this scratch and say how many there have
/// been, this one included. Counted *before* the rescue's finalize runs,
/// so an attempt that crashes the app still counts. A missing or torn
/// counter reads as zero — the error direction that retries rather than
/// discards.
pub fn note_attempt(base: &Path, meeting_id: &str) -> u32 {
    let path = attempts_file(base, meeting_id);
    let so_far: u32 = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    let now = so_far.saturating_add(1);
    if let Err(e) = std::fs::write(&path, now.to_string()) {
        tracing::warn!("could not count the rescue attempt: {e}");
    }
    now
}

/// What launch should do with one pending scratch.
pub enum RescuePlan {
    /// Worth keeping: run finalize, then `clear_for` — in that order, so
    /// the app closing mid-rescue retries at the next launch.
    Rescue(Interrupted),
    /// Nothing to do (too short; already discarded terminally).
    Nothing,
    /// Three rescues started and none committed: the data itself is what
    /// crashes. The scratch is gone, the audio stays in `audio/`.
    GaveUp,
}

/// Decide one pending scratch's fate, counting this attempt first so an
/// attempt that crashes the app still counts against the cap.
pub fn plan_rescue(base: &Path, meeting_id: &str, wav: &Path) -> RescuePlan {
    let attempts = note_attempt(base, meeting_id);
    if attempts > MAX_RESCUE_ATTEMPTS {
        tracing::error!(
            meeting_id,
            "recovery was attempted {MAX_RESCUE_ATTEMPTS} times and never committed — giving up; the audio stays in the library's audio directory"
        );
        clear_for(base, meeting_id);
        return RescuePlan::GaveUp;
    }
    match peek(base, meeting_id, wav) {
        Some(found) => RescuePlan::Rescue(found),
        None => RescuePlan::Nothing,
    }
}

/// Read what one interrupted recording left, **without clearing it** —
/// the scratch outlives the rescue's finalize exactly as it outlives the
/// stop's, so a crash mid-rescue retries at the next launch. `None` when
/// there is nothing worth keeping; the too-short case is terminal (the
/// orphan WAV, pasted images, and scratch are removed — a mis-click
/// needs no retry).
pub fn peek(base: &Path, meeting_id: &str, wav: &Path) -> Option<Interrupted> {
    let bytes = std::fs::metadata(wav).map(|m| m.len()).unwrap_or(0);

    if !worth_recovering(bytes) {
        tracing::info!(
            meeting_id,
            bytes,
            "an interrupted recording was too short to keep — discarding it"
        );
        let _ = std::fs::remove_file(wav);
        // Images pasted into notes that never became a meeting have nothing
        // left to belong to.
        crate::commands::remove_meeting_assets(base, meeting_id);
        clear_for(base, meeting_id);
        return None;
    }

    let segments = read_segments(base, meeting_id);
    let drafts: Drafts = std::fs::read_to_string(drafts_file(base, meeting_id))
        .ok()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default();

    tracing::info!(
        meeting_id,
        segments = segments.len(),
        secs = bytes / BYTES_PER_SEC,
        "recovering an interrupted recording"
    );

    Some(Interrupted {
        meeting_id: meeting_id.to_string(),
        segments,
        user_notes: (!drafts.notes.is_empty()).then_some(drafts.notes),
        user_title: (!drafts.title.trim().is_empty()).then_some(drafts.title),
        stars: drafts
            .stars
            .into_iter()
            .map(|seconds| Star {
                seconds,
                note_block: None,
            })
            .collect(),
    })
}

/// Parse the appended segments, skipping any trailing line a crash cut in
/// half — the whole point of one JSON object per line.
fn read_segments(base: &Path, meeting_id: &str) -> Vec<TranscriptionSegment> {
    let Ok(text) = std::fs::read_to_string(segments_file(base, meeting_id)) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(text: &str, start: f64) -> TranscriptionSegment {
        TranscriptionSegment {
            speaker: None,
            text: text.to_string(),
            start,
            end: start + 1.0,
            speaker_id: None,
        }
    }

    fn scratch_base(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!("embral-recovery-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    fn long_wav(base: &Path, meeting_id: &str) -> PathBuf {
        let wav = base.join(format!("{meeting_id}.wav"));
        std::fs::write(&wav, vec![0u8; (1024 + BYTES_PER_SEC * 30) as usize]).unwrap();
        wav
    }

    #[test]
    fn a_recording_that_died_in_its_first_seconds_is_not_worth_keeping() {
        // A mis-click, or a start that crashed at once. Recovering it would
        // summarize nothing and put an empty meeting in the list.
        assert!(!worth_recovering(0));
        assert!(!worth_recovering(1024)); // header only
        assert!(!worth_recovering(1024 + BYTES_PER_SEC * 9));
    }

    #[test]
    fn ten_seconds_of_audio_is_worth_keeping() {
        assert!(worth_recovering(1024 + BYTES_PER_SEC * 10));
        assert!(worth_recovering(1024 + BYTES_PER_SEC * 3600));
    }

    #[test]
    fn the_scratch_round_trips_what_the_recording_wrote() {
        let base = scratch_base("roundtrip");

        begin(&base, "m-123");
        append_segment(&base, "m-123", &seg("hello", 0.0));
        append_segment(&base, "m-123", &seg("there", 1.0));
        write_drafts(&base, "my notes", "My Meeting", &[4.5]);
        let wav = long_wav(&base, "m-123");

        let found = peek(&base, "m-123", &wav).expect("recoverable");
        assert_eq!(found.meeting_id, "m-123");
        assert_eq!(found.segments.len(), 2);
        assert_eq!(found.segments[1].text, "there");
        assert_eq!(found.user_notes.as_deref(), Some("my notes"));
        assert_eq!(found.user_title.as_deref(), Some("My Meeting"));
        assert_eq!(found.stars.len(), 1);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_peek_does_not_clear_so_a_crashed_rescue_retries() {
        // The rescue's finalize can crash (or the user can close the app
        // during it). The scratch must survive until the meeting commits.
        let base = scratch_base("peek-twice");
        begin(&base, "m-crash");
        append_segment(&base, "m-crash", &seg("survives", 0.0));
        let wav = long_wav(&base, "m-crash");

        assert!(peek(&base, "m-crash", &wav).is_some());
        let again = peek(&base, "m-crash", &wav).expect("still there after a crashed rescue");
        assert_eq!(again.segments.len(), 1);
        assert!(pending(&base).contains(&"m-crash".to_string()));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn an_orphan_survives_a_later_recording_and_its_finalize() {
        // The issue's scenario: a crashed recording's scratch must outlive
        // a full record-and-finalize cycle of a healthy successor and still
        // be offered for rescue at the next launch.
        let base = scratch_base("orphan");
        begin(&base, "m-orphan");
        append_segment(&base, "m-orphan", &seg("do not lose me", 0.0));
        clear_stale_current(&base); // the crashed process's marker retired at launch

        begin(&base, "m-healthy");
        append_segment(&base, "m-healthy", &seg("the new meeting", 0.0));
        assert_eq!(active_meeting_id(&base).as_deref(), Some("m-healthy"));
        clear_for(&base, "m-healthy"); // the healthy finalize returned

        assert!(active_meeting_id(&base).is_none());
        assert_eq!(pending(&base), vec!["m-orphan".to_string()]);
        let orphan_segments = read_segments(&base, "m-orphan");
        assert_eq!(orphan_segments.len(), 1);
        assert_eq!(orphan_segments[0].text, "do not lose me");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn clear_only_touches_its_own_meeting_and_marker() {
        let base = scratch_base("scoped-clear");
        begin(&base, "m-a");
        begin(&base, "m-b"); // b is now current

        clear_for(&base, "m-a");
        assert_eq!(
            active_meeting_id(&base).as_deref(),
            Some("m-b"),
            "clearing a finished meeting leaves the live marker alone"
        );
        assert_eq!(pending(&base), vec!["m-b".to_string()]);

        clear_for(&base, "m-b");
        assert!(active_meeting_id(&base).is_none());
        assert!(pending(&base).is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_tail_segment_lands_in_its_own_meetings_scratch() {
        // A retired stream's tail can arrive while a successor records; the
        // forwarder pins its meeting id, so the tail lands with its own
        // meeting and never contaminates the successor.
        let base = scratch_base("tail");
        begin(&base, "m-old");
        begin(&base, "m-new");
        append_segment(&base, "m-old", &seg("the tail", 99.0));
        append_segment(&base, "m-new", &seg("fresh words", 0.0));

        let old = read_segments(&base, "m-old");
        let new = read_segments(&base, "m-new");
        assert_eq!(old.len(), 1);
        assert_eq!(old[0].text, "the tail");
        assert_eq!(new.len(), 1);
        assert_eq!(new[0].text, "fresh words");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn attempts_count_up_and_a_torn_counter_reads_as_zero() {
        let base = scratch_base("attempts");
        begin(&base, "m-tries");
        assert_eq!(note_attempt(&base, "m-tries"), 1);
        assert_eq!(note_attempt(&base, "m-tries"), 2);
        assert_eq!(note_attempt(&base, "m-tries"), 3);

        // A torn write must err toward retrying, not discarding.
        std::fs::write(attempts_file(&base, "m-tries"), "no").unwrap();
        assert_eq!(note_attempt(&base, "m-tries"), 1);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn three_crashed_rescues_then_the_scratch_is_dropped_and_the_audio_kept() {
        let base = scratch_base("strikes");
        begin(&base, "m-cursed");
        append_segment(&base, "m-cursed", &seg("crashes the pipeline", 0.0));
        let wav = long_wav(&base, "m-cursed");

        // Three launches each start a rescue that never commits (the
        // caller would have cleared on commit).
        for _ in 0..3 {
            assert!(matches!(
                plan_rescue(&base, "m-cursed", &wav),
                RescuePlan::Rescue(_)
            ));
        }
        // The fourth launch gives up: scratch gone, audio untouched.
        assert!(matches!(
            plan_rescue(&base, "m-cursed", &wav),
            RescuePlan::GaveUp
        ));
        assert!(pending(&base).is_empty());
        assert!(wav.exists(), "giving up keeps the audio");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_half_written_segment_line_is_skipped_not_fatal() {
        // The crash can land mid-write; one JSON object per line means the
        // torn tail is the only casualty.
        let base = scratch_base("torn");
        begin(&base, "m-torn");
        append_segment(&base, "m-torn", &seg("complete", 0.0));
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(segments_file(&base, "m-torn"))
            .unwrap();
        f.write_all(b"{\"text\":\"tor").unwrap();
        drop(f);

        let segments = read_segments(&base, "m-torn");
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "complete");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_short_leftover_takes_its_orphan_wav_with_it() {
        let base = scratch_base("tiny");
        begin(&base, "m-tiny");
        let wav = base.join("m-tiny.wav");
        std::fs::write(&wav, vec![0u8; 2048]).unwrap();

        assert!(peek(&base, "m-tiny", &wav).is_none());
        assert!(!wav.exists(), "the orphan wav is cleaned up too");
        assert!(pending(&base).is_empty());
        assert!(active_meeting_id(&base).is_none(), "the marker went with it");
        let _ = std::fs::remove_dir_all(&base);
    }
}
