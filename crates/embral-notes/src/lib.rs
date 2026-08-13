//! Provider-agnostic meeting-note generation for Embral.
//!
//! This crate owns everything between "we have a transcript" and "we have
//! structured markdown notes", independent of the Tauri app so it can be
//! unit-tested without GTK/WebKit:
//!
//! - [`prompt`]: the shared system prompt + user-message builder.
//! - [`providers`]: the OpenAI-protocol transport (sidecar + custom).
//! - [`text`]: title extraction/replacement + filename sanitization.
//! - [`matching`]: naming diarized speakers from the user's typed notes.
//! - [`integrations`]: the post-meeting Obsidian/Markdown export and the
//!   meeting-finished webhook payload.
//!
//! The Tauri crate builds a [`providers::NotesConfig`] from its `AppConfig` and
//! calls [`refine_notes`]; everything else stays here.

pub mod assets;
pub mod integrations;
pub mod matching;
pub mod ocr;
pub mod prompt;
pub mod providers;
pub mod text;
pub mod transcript;

pub use providers::NotesConfig;
pub use text::{apply_title, extract_title, sanitize_filename};

use anyhow::Result;

/// Generate structured meeting notes from a transcript (and optional live user
/// notes) using the provider selected in `cfg`. `custom_prompt` is the user's
/// full replacement prompt body ("" = the built-in default; the output
/// contract is appended either way).
#[allow(clippy::too_many_arguments)]
pub async fn refine_notes(
    cfg: &NotesConfig,
    meeting_id: &str,
    start_time: &str,
    duration_minutes: u32,
    meeting_title: Option<&str>,
    transcript: &str,
    user_notes: Option<&str>,
    custom_prompt: &str,
    image_text: &[(String, String)],
) -> Result<String> {
    let user_message = prompt::build_user_message(
        meeting_id,
        start_time,
        duration_minutes,
        meeting_title,
        transcript,
        user_notes,
        image_text,
    );
    providers::generate(cfg, &prompt::system_prompt(custom_prompt), &user_message).await
}

/// Clean up (or execute the instruction inside) a raw dictation transcript.
pub async fn clean_dictation(cfg: &NotesConfig, raw: &str) -> Result<String> {
    providers::generate(
        cfg,
        prompt::DICTATION_SYSTEM_PROMPT,
        &prompt::build_dictation_message(raw),
    )
    .await
}

/// Load the cleanup prompt into the engine's cache with a one-token call
/// (the built-in engine caches by prompt prefix, and the cleanup prompt is
/// constant) so the first real cleanup pays generation time only.
pub async fn prime_dictation(cfg: &NotesConfig) -> Result<()> {
    providers::prime(
        cfg,
        prompt::DICTATION_SYSTEM_PROMPT,
        &prompt::build_dictation_message(""),
    )
    .await
}
