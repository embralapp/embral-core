//! The shared post-capture pipeline: encode, refine, write notes, announce.
//! Driven by both `stop_recording` (live) and `import_recording` (files).

use embral_db::MeetingRow;
use embral_types::{AppConfig, AppError};
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};

use embral_notes::transcript::format_transcript;

use crate::audio::encoder;
use crate::AppState;

use super::support::*;

/// Where a finished meeting's audio comes from.
pub(crate) enum AudioSource {
    /// A recorder-written WAV: encoded to MP3, then the WAV is deleted
    /// (encode-then-delete even when audio isn't retained â€” unchanged
    /// pre-refactor behavior for live recordings).
    Wav(PathBuf),
    /// Decoded PCM from an import: encoded to MP3 only when audio is retained.
    /// Arc'd so the speaker pipeline can share it without copying an hour of
    /// PCM.
    Samples(Arc<Vec<f32>>),
}

/// Everything between "we have the finalized segments" and "the meeting is
/// saved and announced": MP3 encode, LLM refinement, markdown + DB writes,
/// index export, integrations, and the completion event. Shared by
/// `stop_recording` (live) and `import_recording` (files); behavior for the
/// live path is unchanged by the extraction.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn finalize_meeting(
    app: AppHandle,
    db: Arc<embral_db::Db>,
    base: PathBuf,
    config: AppConfig,
    meeting_id: String,
    started_at: chrono::DateTime<chrono::Utc>,
    mut segments: Vec<embral_types::TranscriptionSegment>,
    audio: AudioSource,
    labels_authoritative: bool,
    // User-starred moments (with their notes anchors when known).
    stars: Vec<Star>,
    user_notes: Option<String>,
    user_title: Option<String>,
    // Names renamed away from during the live session. A profile created
    // for one never gets linked (the pipeline only links final names), so
    // this is the only place it can be recognized as an orphan. Imports and
    // crash recovery have no live session and pass none.
    superseded_labels: Vec<String>,
) {
    // --- Speaker pipeline (before formatting, so names reach the transcript,
    // the notes LLM, and the attendee list). Authoritative provider labels
    // (cloud live diarization) are kept; the local provider's provisional
    // live labels are re-derived here from the full recording, which the
    // pipeline overwrites. A missing model or any failure degrades to the
    // labels we already have.
    let engine = app.state::<AppState>().engine.clone();
    if config.diarization_enabled
        && engine.speaker_id_present()
        && !segments.is_empty()
        && !(labels_authoritative && segments.iter().any(|s| s.speaker.is_some()))
    {
        let samples: Option<Arc<Vec<f32>>> = match &audio {
            AudioSource::Wav(p) => match crate::speakers::read_wav_16k(p) {
                Ok(s) => Some(Arc::new(s)),
                Err(e) => {
                    tracing::warn!("could not read recording for diarization: {e}");
                    None
                }
            },
            AudioSource::Samples(s) => Some(s.clone()),
        };
        if let Some(samples) = samples {
            let db2 = db.clone();
            let config2 = config.clone();
            let engine2 = engine.clone();
            let mut segs = segments.clone();
            let outcome = tokio::task::spawn_blocking(move || {
                let labeled = crate::speakers::run(&engine2, &db2, &config2, &samples, &mut segs);
                (segs, labeled)
            })
            .await;
            match outcome {
                Ok((segs, Ok(()))) => segments = segs,
                Ok((_, Err(e))) => tracing::warn!("speaker pipeline failed: {e}"),
                Err(e) => tracing::error!("speaker pipeline panicked: {e}"),
            }
        }
    }

    // --- Name speakers from the user's typed notes ([speakers.md]), before
    // formatting for the same reason as the pipeline above. Automatic mode
    // renames segments here; suggest mode returns pending suggestions that
    // persist below and surface in the meeting view.
    let name_suggestions = {
        let state = app.state::<AppState>();
        crate::notes_matching::run(
            &state.search,
            &state.llm,
            &db,
            &config,
            user_notes.as_deref().unwrap_or(""),
            &mut segments,
        )
        .await
    };
    let name_suggestions_json =
        serde_json::to_string(&name_suggestions).unwrap_or_else(|_| "[]".into());

    let transcript_text = format_transcript(&segments);
    let _ = app.emit("transcription-final-complete", &transcript_text);

    // Encode MP3 (non-fatal on failure; we can still write notes).
    let mp3_path = base.join("audio").join(format!("{}.mp3", meeting_id));
    // The recorder's WAV is deleted once the meeting is committed, not
    // here: everything between this point and the DB write (the speaker
    // pipeline, LLM refinement) can take a while, and a crash inside it
    // must leave crash recovery something to re-run from
    // ([recording.md] §Crash recovery).
    let mut wav_to_delete: Option<PathBuf> = None;
    match audio {
        AudioSource::Wav(wav_path) => {
            match encoder::encode_wav_to_mp3(&wav_path, &mp3_path) {
                Ok(()) => {
                    // Audio is playable well before the notes finish; let
                    // the pending meeting mount its player now. (The file
                    // is renamed at persist time; the completed detail
                    // brings the final path.)
                    let _ = app.emit(
                        "pending-audio-ready",
                        mp3_path.to_string_lossy().to_string(),
                    );
                }
                Err(e) => {
                    tracing::error!("MP3 encode failed: {}", e);
                    let _ = app.emit("processing-error", &AppError::EncodeFailed { detail: e.to_string() });
                    crate::telemetry::track(
                        &app.state::<AppState>(),
                        "error",
                        serde_json::json!({ "category": "encode_failed" }),
                    );
                }
            }
            wav_to_delete = Some(wav_path);
        }
        AudioSource::Samples(samples) => {
            if config.retain_audio {
                if let Err(e) = encoder::encode_samples_to_mp3(samples.as_slice(), crate::audio::SAMPLE_RATE_HZ, &mp3_path) {
                    tracing::error!("MP3 encode failed: {}", e);
                    let _ = app.emit("processing-error", &AppError::EncodeFailed { detail: e.to_string() });
                    crate::telemetry::track(
                        &app.state::<AppState>(),
                        "error",
                        serde_json::json!({ "category": "encode_failed" }),
                    );
                }
            }
        }
    }

    // LLM refinement.
    let _ = app.emit("notes-generation-started", ());

    // No segments: transcription was disabled or produced nothing. The
    // wall clock is the only duration signal left.
    let duration_minutes = segments
        .last()
        .map(|s| (s.end / 60.0).ceil() as u32)
        .unwrap_or_else(|| {
            (chrono::Utc::now().signed_duration_since(started_at).num_seconds() as f64 / 60.0)
                .ceil() as u32
        })
        .max(1);

    let start_time = started_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let fallback_attendees = attendees_from_segments(&segments);

    // Read the meeting's images now, before the summary is written, so the
    // prompt's inventory can say what each one contains and the model picks
    // the right screenshot rather than guessing from the prose around it.
    // It also means a just-finished meeting is searchable by its images at
    // once instead of waiting on the background sweep. The rows cannot land
    // yet (the meeting row does not exist until further down), so the
    // readings are held and stored after `upsert_meeting`.
    let image_readings = {
        let filenames = crate::ocr::stored_images(&base, &meeting_id);
        if filenames.is_empty() {
            Vec::new()
        } else {
            let (base, id) = (base.clone(), meeting_id.clone());
            tokio::task::spawn_blocking(move || crate::ocr::read_images(&base, &id, &filenames))
                .await
                .unwrap_or_default()
        }
    };
    // The prompt keys on the link the notes carry, not the bare filename.
    let image_text: Vec<(String, String)> = image_readings
        .iter()
        .map(|(filename, text)| {
            (
                embral_notes::assets::link_rel(&meeting_id, filename),
                text.clone(),
            )
        })
        .collect();

    // `None` = this meeting has no summary: either summaries are off, or the
    // engine failed, or there is nothing to summarize. Nothing fake is
    // written in its place; a "summary" that is a copy of the transcript
    // (or, on an empty one, an invention) is worse than no summary at all.
    let summary: Option<String> = match crate::refinement::summaries_profile(&config)
        .filter(|_| !segments.is_empty())
    {
        Some(profile) => {
            let sidecar = &app.state::<AppState>().llm;
            let generated = match crate::llm::resolved_notes_config(sidecar, &config, &profile).await {
                Ok(notes_cfg) => {
                    crate::refinement::refine_notes(
                        &notes_cfg,
                        &config,
                        &meeting_id,
                        &start_time,
                        duration_minutes,
                        user_title.as_deref(),
                        &transcript_text,
                        user_notes.as_deref(),
                        &image_text,
                    )
                    .await
                }
                Err(e) => Err(e),
            };
            sidecar.touch();
            match &generated {
                Ok(_) => {
                    // "cloud" is CLOUD_PROFILE_ID, spelled out because the
                    // constant is cfg-gated to the cloud edition.
                    let engine = match profile.id.as_str() {
                        "" | embral_types::BUILTIN_PROFILE_ID => "builtin",
                        "cloud" => "cloud",
                        _ => "custom",
                    };
                    crate::telemetry::track(
                        &app.state::<AppState>(),
                        "notes_generated",
                        serde_json::json!({ "engine": engine }),
                    );
                }
                Err(e) => {
                    tracing::error!("LLM refinement failed: {e}");
                    crate::telemetry::track(
                        &app.state::<AppState>(),
                        "error",
                        serde_json::json!({ "category": "notes_failed" }),
                    );
                }
            }
            generated.ok()
        }
        None => {
            if segments.is_empty() {
                tracing::info!("no transcript — nothing to summarize");
            } else {
                tracing::info!("summaries are off — this meeting keeps its notes and transcript");
            }
            None
        }
    };

    let summary = summary.map(|md| match user_title.as_deref() {
        Some(title) => crate::refinement::apply_title(&md, title),
        None => md,
    });

    // The model was shown the notes' image links and asked to reuse them
    // exactly. Asking is not a guarantee: it can produce a plausible-looking
    // path that points at nothing, and the notes it was shown are
    // user-typed, so `![](../../../secret.png)` is reachable through them
    // too. Keep only links this meeting actually owns.
    let summary = summary.map(|md| {
        let known: std::collections::HashSet<String> =
            embral_notes::assets::image_links(user_notes.as_deref().unwrap_or(""))
                .into_iter()
                .collect();
        let kept = embral_notes::assets::retain_known_images(&md, &known);
        if kept != md {
            tracing::warn!("dropped an image link the summary invented");
        }
        kept
    });

    let inferred_attendees = summary
        .as_deref()
        .map(parse_attendees)
        .unwrap_or_default();
    let attendees = if inferred_attendees.is_empty() {
        fallback_attendees
    } else {
        inferred_attendees
    };

    // The user's raw notes persist verbatim in their own column (the Notes
    // tab renders them with the star anchors); the summary document stays
    // pure synthesis.
    let title = user_title.clone().unwrap_or_else(|| {
        summary
            .as_deref()
            .and_then(crate::refinement::extract_title)
            .unwrap_or_else(|| "Untitled Meeting".to_string())
    });
    let frontmatter = canonical_frontmatter(&start_time, duration_minutes, &meeting_id, &attendees);
    let notes_document = summary
        .as_deref()
        .map(|md| prepend_frontmatter(md, &frontmatter));

    let safe_title = crate::refinement::sanitize_filename(&title);
    // meeting_id is "YYMMDDTHHMMSS_XXXXXX" â€” timestamp prefix is first 13 chars
    let ts_prefix = &meeting_id[..13.min(meeting_id.len())];
    let final_stem = format!("{} - {}", ts_prefix, safe_title);
    let final_audio_filename = format!("{}.mp3", final_stem);

    // No segments, no transcript document: same rule as the summary: an
    // empty shell helps nobody. Both documents go to the database below;
    // audio is the only file this meeting writes.
    let transcript_markdown = if segments.is_empty() {
        String::new()
    } else {
        format_transcript_document(
            &title,
            &meeting_id,
            &start_time,
            duration_minutes,
            &attendees,
            &transcript_text,
        )
    };

    let final_mp3_path = base.join("audio").join(&final_audio_filename);
    let mut retained_audio_filename = final_audio_filename.clone();
    if mp3_path.exists() {
        if let Err(e) = std::fs::rename(&mp3_path, &final_mp3_path) {
            tracing::warn!("Failed to rename MP3: {}", e);
            retained_audio_filename = format!("{}.mp3", meeting_id);
        }
    }

    let duration_secs = segments.last().map(|s| s.end as u64).unwrap_or_else(|| {
        chrono::Utc::now()
            .signed_duration_since(started_at)
            .num_seconds()
            .max(0) as u64
    });
    // True whether the rename succeeded (final path) or failed (id-named
    // fallback) â€” either way a playable file exists on disk.
    let audio_present = final_mp3_path.exists() || base.join(&retained_audio_filename).exists()
        || base.join("audio").join(&retained_audio_filename).exists();

    // Persist to the database (source of truth), then regenerate the
    // index.json export the MCP servers read.
    let row = MeetingRow {
        id: meeting_id.clone(),
        title: title.clone(),
        started_at,
        duration_seconds: duration_secs,
        summary: notes_document.clone().unwrap_or_default(),
        transcript: transcript_markdown.clone(),
        attendees: attendees.clone(),
        audio_path: if config.retain_audio && audio_present {
            format!("audio/{}", retained_audio_filename)
        } else {
            String::new()
        },
    };
    let record = row.to_record();

    if let Err(e) = db
        .upsert_meeting(&row)
        .and_then(|()| db.replace_segments(&meeting_id, &segments))
        .and_then(|()| db.set_name_suggestions(&meeting_id, &name_suggestions_json))
        .and_then(|()| {
            let json = serde_json::to_string(&stars).unwrap_or_else(|_| "[]".into());
            db.set_stars(&meeting_id, &json)
        })
        .and_then(|()| db.set_notes(&meeting_id, user_notes.as_deref().unwrap_or("")))
        .and_then(|()| crate::storage::export_index(&db, &base))
    {
        let _ = app.emit("processing-error", &AppError::Internal { detail: e.to_string() });
        crate::telemetry::track(
            &app.state::<AppState>(),
            "error",
            serde_json::json!({ "category": "save_failed" }),
        );
        return;
    }
    // Committed. The recorder's WAV has done its job; everything above
    // could still have needed it, and the early return on a failed save
    // deliberately leaves it for crash recovery to re-run from.
    if let Some(wav_path) = wav_to_delete {
        let _ = std::fs::remove_file(&wav_path);
    }
    // The meeting row exists now, so what the images said can be recorded
    // against it, and the sync below indexes it right after.
    crate::ocr::store(&db, &meeting_id, &image_readings);
    crate::search_index::sync_meeting(&db, &app.state::<AppState>().search, &meeting_id);

    // The segments are committed, so a superseded live name's profile can
    // be judged for what it is: if nothing links to it and it carries no
    // notes, it was a mislabel corrected mid-meeting, and it goes.
    if !superseded_labels.is_empty() {
        match db.list_speakers() {
            Ok(speakers) => {
                let candidates: Vec<String> = speakers
                    .into_iter()
                    .filter(|p| {
                        superseded_labels
                            .iter()
                            .any(|label| p.name.eq_ignore_ascii_case(label))
                    })
                    .map(|p| p.id)
                    .collect();
                match db.prune_orphaned_speakers(&candidates) {
                    Ok(pruned) if !pruned.is_empty() => {
                        tracing::info!(count = pruned.len(), "pruned orphaned speaker profiles");
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!("could not prune orphaned speakers: {e}"),
                }
            }
            Err(e) => tracing::warn!("could not list speakers for orphan pruning: {e}"),
        }
    }

    if !config.retain_audio {
        let _ = std::fs::remove_file(&final_mp3_path);
        let _ = std::fs::remove_file(&mp3_path);
    }

    // Best-effort fan-out to the Markdown export and the webhooks. The copy
    // carries what the include switches say (summary, the user's own notes,
    // transcript, each defaulting in; the webhook payload takes the parts
    // separately, and only for destinations that opted into content.
    let summary_body = summary.as_deref().unwrap_or("");
    let user_summary = user_notes.as_deref().unwrap_or("");
    let export_document = embral_notes::integrations::compose_export(
        &frontmatter,
        &title,
        config.export_include_summary.then_some(summary_body),
        config.export_include_notes.then_some(user_summary),
        config
            .export_include_transcript
            .then_some(transcript_text.as_str()),
    );
    crate::refinement::run_post_meeting_integrations(
        &app,
        &config,
        &record,
        &export_document,
        summary_body,
        user_summary,
        &transcript_text,
    );

    let _ = app.emit("notes-generation-complete", &record);
}
