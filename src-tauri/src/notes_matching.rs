//! Naming diarized speakers from the user's typed notes: orchestration.
//!
//! The pure half (line splitting, evidence scoring, the prompt, reply
//! parsing) lives in `embral_notes::matching`; this module owns the async
//! pieces: embeddings through the search runtime's embed child, the one
//! LLM call per meeting, and applying the outcome per `notes_naming_mode`.
//! Every failure degrades to "no names"; the meeting is never blocked or
//! altered by a confused pass ([speakers.md](../../docs/speakers.md)).

use embral_db::Db;
use embral_notes::matching;
use embral_types::{AppConfig, NotesNamingMode, TranscriptionSegment};

use crate::llm::LlmSidecar;
use crate::search_index::SearchRuntime;

/// One pending "Speaker N looks like X" suggestion, persisted per meeting
/// (JSON in `meetings.name_suggestions`) until confirmed or dismissed.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NameSuggestion {
    pub label: String,
    pub name: String,
}

/// Note lines considered (embedding + prompt bound).
const NOTE_LINE_CAP: usize = 100;
/// Transcript paragraphs considered (embedding bound).
const CANDIDATE_CAP: usize = 200;

/// Run the naming pass over a finalized meeting's segments. Suggest mode
/// returns the pending suggestions to persist; automatic mode renames
/// `segments` in place and returns nothing. Runs regardless of where the
/// labels came from: cloud live diarization and the local pipeline both
/// produce the generic "Speaker N" labels this matches on.
pub async fn run(
    search: &SearchRuntime,
    sidecar: &LlmSidecar,
    db: &Db,
    config: &AppConfig,
    user_notes: &str,
    segments: &mut [TranscriptionSegment],
) -> Vec<NameSuggestion> {
    if config.notes_naming_mode == NotesNamingMode::Off || user_notes.trim().is_empty() {
        return Vec::new();
    }

    // The generic labels in play, first-seen order: the only ones the
    // pass may name (user-given names are already better information).
    let mut labels: Vec<String> = Vec::new();
    for seg in segments.iter() {
        if let Some(label) = seg.speaker.as_deref() {
            if matching::is_generic_label(label) && !labels.iter().any(|l| l == label) {
                labels.push(label.to_string());
            }
        }
    }
    if labels.is_empty() {
        return Vec::new();
    }

    let mut lines = matching::note_lines(user_notes);
    lines.truncate(NOTE_LINE_CAP);
    if lines.is_empty() {
        return Vec::new();
    }
    let paragraphs = embral_notes::transcript::paragraphs(segments);
    let candidates = matching::candidates(&paragraphs, CANDIDATE_CAP);
    if candidates.is_empty() {
        return Vec::new();
    }

    // The semantic part, degrading to keyword-only scoring when the
    // embedding model is absent or the embed child fails. The child
    // self-spawns if cold (~1 s), which finalize can afford.
    let (note_vecs, cand_vecs) = if embral_search::model::present() {
        let texts: Vec<String> = candidates.iter().map(|c| c.text.clone()).collect();
        match (
            search.embed_passages(&lines).await,
            search.embed_passages(&texts).await,
        ) {
            (Ok(nv), Ok(cv)) => (Some(nv), Some(cv)),
            (Err(e), _) | (_, Err(e)) => {
                tracing::warn!("naming pass embeddings unavailable — keyword matching only: {e}");
                (None, None)
            }
        }
    } else {
        (None, None)
    };

    let evidence = matching::evidence(
        &lines,
        &candidates,
        note_vecs.as_deref(),
        cand_vecs.as_deref(),
    );
    if evidence.is_empty() {
        tracing::info!("naming pass: no note line resembles the transcript; skipping");
        return Vec::new();
    }

    let Some(llm_cfg) = crate::llm::resolved_naming_config(sidecar, config).await else {
        return Vec::new();
    };
    let message = matching::build_naming_message(user_notes, &labels, &evidence);
    let reply = match embral_notes::providers::generate(
        &llm_cfg,
        matching::NAMING_SYSTEM_PROMPT,
        &message,
    )
    .await
    {
        Ok(reply) => reply,
        Err(e) => {
            tracing::warn!("naming pass LLM call failed — speakers keep their labels: {e}");
            return Vec::new();
        }
    };
    sidecar.touch();

    let assignments = matching::parse_assignments(&reply, &labels);
    if assignments.is_empty() {
        tracing::info!("naming pass: the notes identify nobody (model returned no assignments)");
        return Vec::new();
    }

    match config.notes_naming_mode {
        NotesNamingMode::Automatic => {
            // Link to an existing profile by name; never create one from an
            // unattended pass (approval is what justifies a new profile).
            let profile_id_by_name: std::collections::HashMap<String, String> = db
                .list_speakers()
                .map(|rows| rows.into_iter().map(|p| (p.name.to_lowercase(), p.id)).collect())
                .unwrap_or_default();
            let mut renamed = 0usize;
            for (label, name) in &assignments {
                let id = profile_id_by_name.get(&name.to_lowercase()).cloned();
                for seg in segments.iter_mut() {
                    if seg.speaker.as_deref() == Some(label.as_str()) {
                        seg.speaker = Some(name.clone());
                        seg.speaker_id = id.clone();
                        renamed += 1;
                    }
                }
            }
            tracing::info!(
                names = assignments.len(),
                segments = renamed,
                "naming pass applied names from the user's notes"
            );
            Vec::new()
        }
        NotesNamingMode::Suggest => {
            tracing::info!(suggestions = assignments.len(), "naming pass produced suggestions");
            assignments
                .into_iter()
                .map(|(label, name)| NameSuggestion { label, name })
                .collect()
        }
        NotesNamingMode::Off => unreachable!("gated above"),
    }
}
