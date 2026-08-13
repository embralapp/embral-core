//! The built-in LLM sidecar: a managed llama-server child process.
//!
//! Summaries and dictation cleanup with the `builtin` provider talk to a
//! local llama-server (llama.cpp) over its OpenAI-compatible loopback
//! endpoint. The runtime and weights are catalog downloads (`llama-server`,
//! `qwen3-4b`); this module owns the process: start on demand, health-gate
//! before first use, evict after idling (unless keep-warm), kill on app exit.
//!
//! The speech engine stays loaded for instant recording; the LLM is the
//! memory-hungry piece that justifies eviction.

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use embral_engine::catalog::{self, FileRole};

struct Running {
    child: Child,
    port: u16,
}

pub struct LlmSidecar {
    /// std Mutex so the app-exit handler (sync context) can kill the child.
    running: std::sync::Mutex<Option<Running>>,
    /// Serializes cold starts; the health poll happens inside this section.
    start_lock: tokio::sync::Mutex<()>,
    last_used: std::sync::Mutex<Instant>,
}

impl Default for LlmSidecar {
    fn default() -> Self {
        Self {
            running: std::sync::Mutex::new(None),
            start_lock: tokio::sync::Mutex::new(()),
            last_used: std::sync::Mutex::new(Instant::now()),
        }
    }
}

/// Model load on CPU takes a while on first touch; the health poll waits this
/// long before declaring the start failed.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(120);
const HEALTH_POLL: Duration = Duration::from_millis(500);
/// Context window requested from llama-server. Qwen3-4B supports far more,
/// but meeting transcripts fit and RAM stays modest.
const CONTEXT_TOKENS: u32 = 16_384;

impl LlmSidecar {
    /// Base URL of a healthy sidecar (e.g. `http://127.0.0.1:8641/v1`),
    /// starting it first when needed.
    pub async fn ensure_running(&self) -> Result<String> {
        self.touch();
        let _starting = self.start_lock.lock().await;

        // Already up (and the process didn't die behind our back)?
        {
            let mut guard = self.running.lock().expect("llm mutex poisoned");
            if let Some(r) = guard.as_mut() {
                match r.child.try_wait() {
                    Ok(None) => {
                        let port = r.port;
                        drop(guard);
                        return Ok(base_url(port));
                    }
                    _ => {
                        tracing::warn!("llama-server exited unexpectedly; restarting");
                        *guard = None;
                    }
                }
            }
        }

        let runtime = catalog::find("llama-server").ok_or_else(|| anyhow!("no runtime entry"))?;
        let weights = catalog::find("qwen3-4b").ok_or_else(|| anyhow!("no weights entry"))?;
        if !runtime.present() || !weights.present() {
            bail!(
                "the built-in model isn't downloaded — get \"Summary engine\" and \"Built-in language model\" in Settings"
            );
        }
        let exe = runtime
            .role_path(FileRole::LlamaServer)
            .ok_or_else(|| anyhow!("runtime path missing"))?;
        let gguf = weights
            .role_path(FileRole::Gguf)
            .ok_or_else(|| anyhow!("weights path missing"))?;

        // Let the OS pick a free port; the tiny window between drop and spawn
        // is acceptable for a loopback dev-style service.
        let port = std::net::TcpListener::bind(("127.0.0.1", 0))
            .and_then(|l| l.local_addr())
            .context("find a free port")?
            .port();

        tracing::info!(port, "starting llama-server");
        let mut command = Command::new(&exe);
        command
            .arg("-m")
            .arg(&gguf)
            .args(["--host", "127.0.0.1", "--port"])
            .arg(port.to_string())
            .args(["-c", &CONTEXT_TOKENS.to_string(), "--no-webui"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        crate::platform::hide_console(&mut command);
        crate::platform::prepare_spawn(&mut command);
        let mut child = command.spawn().context("spawn llama-server")?;
        crate::platform::watch_child(child.id());

        // Wait until the model is loaded (/health flips 503 → 200).
        let client = reqwest::Client::new();
        let health = format!("http://127.0.0.1:{port}/health");
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        let started = Instant::now();
        loop {
            if let Ok(Some(status)) = child.try_wait() {
                bail!("llama-server exited during startup ({status})");
            }
            if let Ok(resp) = client.get(&health).send().await {
                if resp.status().is_success() {
                    break;
                }
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                bail!("llama-server did not become healthy within {STARTUP_TIMEOUT:?}");
            }
            tokio::time::sleep(HEALTH_POLL).await;
        }
        tracing::info!(
            port,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "llama-server ready"
        );

        *self.running.lock().expect("llm mutex poisoned") = Some(Running { child, port });
        self.touch();
        Ok(base_url(port))
    }

    /// Record activity; postpones idle eviction.
    pub fn touch(&self) {
        *self.last_used.lock().expect("llm mutex poisoned") = Instant::now();
    }

    pub fn is_running(&self) -> bool {
        self.running
            .lock()
            .expect("llm mutex poisoned")
            .is_some()
    }

    /// Kill the child (idle eviction / app exit). Safe from sync contexts.
    pub fn shutdown(&self) {
        let mut guard = self.running.lock().expect("llm mutex poisoned");
        if let Some(mut r) = guard.take() {
            tracing::info!("stopping llama-server");
            let _ = r.child.kill();
            let _ = r.child.wait();
        }
    }

    /// Called by the periodic janitor.
    pub fn evict_if_idle(&self, keep_warm: bool, idle_minutes: u32) {
        if keep_warm || !self.is_running() {
            return;
        }
        let idle = Duration::from_secs(u64::from(idle_minutes.max(1)) * 60);
        let last = *self.last_used.lock().expect("llm mutex poisoned");
        if last.elapsed() >= idle {
            tracing::info!(
                idle_minutes,
                "built-in model idle — releasing its memory"
            );
            self.shutdown();
        }
    }
}

fn base_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/v1")
}

/// Resolve a profile into a ready-to-call transport config: the builtin
/// engine gets the running sidecar's port; the cloud engine (cloud builds
/// only) gets the relay endpoint with this device's session token.
pub async fn resolved_notes_config(
    sidecar: &LlmSidecar,
    config: &embral_types::AppConfig,
    profile: &embral_types::LlmProfile,
) -> Result<embral_notes::NotesConfig> {
    let mut cfg = crate::refinement::notes_config(profile);
    if profile.provider == embral_types::LlmProvider::Builtin {
        cfg.endpoint = sidecar.ensure_running().await?;
    }
    #[cfg(feature = "cloud")]
    if profile.id == embral_types::CLOUD_PROFILE_ID {
        anyhow::ensure!(
            !config.cloud_session_token.is_empty(),
            "sign in to your embral account to use cloud summaries"
        );
        cfg.endpoint = format!("{}/v1", config.cloud_url().trim_end_matches('/'));
        cfg.api_key = config.cloud_session_token.clone();
    }
    #[cfg(not(feature = "cloud"))]
    let _ = config;
    Ok(cfg)
}

/// Whether anything in the current configuration actually uses the built-in
/// model: summaries on the builtin engine, dictation cleanup on-device, or
/// cloud cleanup while signed out (the degrade chain falls back to it then).
/// The performance knobs (`llm_keep_warm`, `llm_idle_minutes`) follow this:
/// keep-warm must not pin ~3 GB after a one-off use when every engine has
/// since left the device. The settings UI mirrors this rule
/// (`utils/llmUsage.ts`); keep the two in step.
pub fn uses_local_llm(config: &embral_types::AppConfig) -> bool {
    let summaries_local = config.summaries_enabled
        && (config.summaries_profile_id.is_empty()
            || config.summaries_profile_id == embral_types::BUILTIN_PROFILE_ID);
    summaries_local || cleanup_uses_builtin(config)
}

/// Whether dictation cleanup would run on the built-in model right now:
/// the on-device tier, or the cloud tier while signed out (its degrade
/// target). Dictation start uses this to prewarm the sidecar so cleanup
/// doesn't cold-start it inside the stop pipeline.
pub fn cleanup_uses_builtin(config: &embral_types::AppConfig) -> bool {
    match config.dictation_cleanup {
        embral_types::DictationCleanup::OnDevice => true,
        #[cfg(feature = "cloud")]
        embral_types::DictationCleanup::Cloud => config.cloud_session_token.is_empty(),
        embral_types::DictationCleanup::Off => false,
    }
}

/// The transport dictation cleanup runs with, per the configured tier,
/// degrading rather than blocking, because cleanup must never cost the user
/// their dictation: Cloud while signed out falls to the on-device model
/// (a stale-config safety net only; sign-out reverts the stored tier to
/// on-device, [cloud-seam.md]); an unavailable on-device model (or `Off`)
/// is `None`, and the caller delivers the raw text. A request failure on
/// whichever transport this returns degrades at the call site the same way.
pub async fn resolved_cleanup_config(
    sidecar: &LlmSidecar,
    config: &embral_types::AppConfig,
) -> Option<embral_notes::NotesConfig> {
    match config.dictation_cleanup {
        embral_types::DictationCleanup::Off => None,
        #[cfg(feature = "cloud")]
        embral_types::DictationCleanup::Cloud if !config.cloud_session_token.is_empty() => {
            let mut cfg = crate::refinement::notes_config(&embral_types::LlmProfile::cloud());
            // The dictation relay, not the synthesis one: its own pinned
            // model chain and its own metering server-side.
            cfg.endpoint = format!(
                "{}/v1/dictation",
                config.cloud_url().trim_end_matches('/')
            );
            cfg.api_key = config.cloud_session_token.clone();
            Some(cfg)
        }
        // OnDevice, and Cloud while signed out (sign-out reversion makes
        // that a stale-config edge case): the built-in model.
        _ => {
            let mut cfg =
                crate::refinement::notes_config(&embral_types::LlmProfile::builtin());
            match sidecar.ensure_running().await {
                Ok(endpoint) => {
                    cfg.endpoint = endpoint;
                    Some(cfg)
                }
                Err(e) => {
                    tracing::warn!("cleanup model unavailable — using raw text: {e}");
                    None
                }
            }
        }
    }
}

/// The transport the notes-naming pass runs with: the summaries engine
/// when one resolves (the sidecar is typically already going to be used by
/// the summary anyway), else the built-in model, else `None`, in which case
/// the pass silently skips and speakers keep their labels.
pub async fn resolved_naming_config(
    sidecar: &LlmSidecar,
    config: &embral_types::AppConfig,
) -> Option<embral_notes::NotesConfig> {
    if let Some(profile) = crate::refinement::summaries_profile(config) {
        match resolved_notes_config(sidecar, config, &profile).await {
            Ok(cfg) => return Some(cfg),
            Err(e) => tracing::warn!(
                "naming: summaries engine unavailable ({e}); trying the built-in model"
            ),
        }
    }
    let mut cfg = crate::refinement::notes_config(&embral_types::LlmProfile::builtin());
    match sidecar.ensure_running().await {
        Ok(endpoint) => {
            cfg.endpoint = endpoint;
            Some(cfg)
        }
        Err(e) => {
            tracing::warn!("naming model unavailable — speakers keep their labels: {e}");
            None
        }
    }
}

/// Real-weights e2e: download `llama-server` + `qwen3-4b` from Settings (or
/// manually into the models dir), then
/// `cargo test -p embral --lib builtin_llm -- --ignored --nocapture`.
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires the llama-server and qwen3-4b downloads; loads ~2.5 GB"]
    async fn builtin_llm_generates_structured_notes() {
        let sidecar = LlmSidecar::default();
        let profile = embral_types::LlmProfile::builtin();
        let config = embral_types::AppConfig::default();
        let cfg = resolved_notes_config(&sidecar, &config, &profile)
            .await
            .expect("sidecar starts");

        let transcript = "\
Speaker 1: Good morning everyone, thanks for joining the quarterly planning call. Today we need to cover the migration status and the onboarding redesign.\n\n\
Speaker 2: Thanks. On my side the database migration finished on schedule and error rates stayed flat all week, so I propose we widen the rollout to fifty percent on Friday.\n\n\
Speaker 1: Agreed. Second topic: several customers said the new onboarding form is confusing. Dana will set up a design review for early next week and bring the latest mockups.\n\n\
Speaker 2: Sounds good. Last thing — budget: we are about eight percent under for the quarter, so there is room for the extra support hire we discussed.";

        let notes = embral_notes::refine_notes(
            &cfg,
            "260708T120000_test",
            "2026-07-08T12:00:00Z",
            4,
            None,
            transcript,
            None,
            "",
            &[],
        )
        .await
        .expect("refine notes");
        eprintln!("--- notes ---\n{notes}\n-------------");
        sidecar.shutdown();

        assert!(notes.contains("# "), "has a title heading");
        assert!(notes.contains("## Key Takeaways"));
        assert!(notes.contains("## Next Steps"));
    }

    #[test]
    fn local_llm_usage_follows_its_consumers() {
        let mut config = embral_types::AppConfig::default();

        // The default config summarizes with the builtin engine.
        config.summaries_enabled = true;
        config.summaries_profile_id = embral_types::BUILTIN_PROFILE_ID.to_string();
        config.dictation_cleanup = embral_types::DictationCleanup::Off;
        assert!(uses_local_llm(&config));
        // "" has always meant builtin.
        config.summaries_profile_id = String::new();
        assert!(uses_local_llm(&config));

        // Summaries off (or on another engine) with cleanup off: nothing
        // uses the device.
        config.summaries_enabled = false;
        assert!(!uses_local_llm(&config));

        // On-device cleanup is a consumer on its own.
        config.dictation_cleanup = embral_types::DictationCleanup::OnDevice;
        assert!(uses_local_llm(&config));
    }

    #[cfg(feature = "cloud")]
    #[test]
    fn signed_out_cloud_cleanup_counts_as_local() {
        let mut config = embral_types::AppConfig::default();
        config.summaries_enabled = false;
        config.dictation_cleanup = embral_types::DictationCleanup::Cloud;

        // Signed out, the degrade chain falls back to the builtin model.
        config.cloud_session_token = String::new();
        assert!(uses_local_llm(&config));

        // Signed in, cloud cleanup stays in the cloud.
        config.cloud_session_token = "token".to_string();
        assert!(!uses_local_llm(&config));
    }
}
