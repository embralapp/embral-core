//! The LLM transport for note generation.
//!
//! Both engines speak the OpenAI chat/completions protocol: `Builtin` is the
//! bundled llama-server sidecar (endpoint resolved to its loopback port at
//! call time) and `Custom` is any other OpenAI-compatible base URL (the cloud
//! relay will use this). To keep this unit-testable without a network, the
//! wire concerns are split into pure functions ([`endpoint`],
//! [`request_body`], [`parse_response`]) and one thin async [`generate`]
//! that ties them together with `reqwest`.

use anyhow::{anyhow, Result};
use embral_types::LlmProvider;
use serde_json::{json, Value};

/// Everything one LLM call needs. Built from an `LlmProfile` by the caller
/// (see `src-tauri/src/refinement.rs`); for the `Builtin` provider the
/// caller resolves `endpoint` to the running sidecar's loopback URL first.
#[derive(Debug, Clone, Default)]
pub struct NotesConfig {
    pub provider: LlmProvider,
    /// Empty → the provider's built-in default (see [`model_for`]).
    pub model: String,
    /// Base URL; required. `Builtin` gets the runtime sidecar port,
    /// `Custom` whatever it was configured with.
    pub endpoint: String,
    pub api_key: String,
}

/// Resolve the effective model id: the override, or a per-provider default.
/// The built-in sidecar serves exactly one loaded model, so the name is
/// informational there.
pub fn model_for(cfg: &NotesConfig) -> String {
    let model = cfg.model.trim();
    if !model.is_empty() {
        return model.to_string();
    }
    match cfg.provider {
        LlmProvider::Builtin => "qwen3-4b",
        LlmProvider::Custom => "default",
    }
    .to_string()
}

/// The full request URL (OpenAI chat/completions on the configured base).
pub fn endpoint(cfg: &NotesConfig) -> Result<String> {
    let base = cfg.endpoint.trim();
    if base.is_empty() {
        return Err(match cfg.provider {
            LlmProvider::Builtin => anyhow!(
                "the built-in model isn't running — its endpoint is resolved at call time"
            ),
            LlmProvider::Custom => anyhow!("this engine needs an endpoint URL"),
        });
    }
    Ok(format!("{}/chat/completions", base.trim_end_matches('/')))
}

/// Build the JSON request body (OpenAI chat shape for both engines).
pub fn request_body(cfg: &NotesConfig, system: &str, user: &str) -> Value {
    json!({
        "model": model_for(cfg),
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user },
        ],
    })
}

/// Extract the generated markdown from the JSON response.
pub fn parse_response(_provider: LlmProvider, body: &Value) -> Result<String> {
    let text = body
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(Value::as_str);
    text.map(strip_reasoning)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| anyhow!("empty or unexpected response from notes provider: {}", body))
}

/// Small local models (Qwen3 in particular) may emit `<think>…</think>`
/// reasoning before the answer; keep only what follows.
pub(crate) fn strip_reasoning(text: &str) -> String {
    match (text.find("<think>"), text.find("</think>")) {
        (Some(open), Some(close)) if open < close => {
            let mut out = String::new();
            out.push_str(&text[..open]);
            out.push_str(&text[close + "</think>".len()..]);
            out.trim().to_string()
        }
        _ => text.trim().to_string(),
    }
}

/// Apply auth headers: bearer token when a key is set (the sidecar needs
/// none; the cloud relay will authenticate this way).
fn apply_headers(cfg: &NotesConfig, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    let req = req.header("Content-Type", "application/json");
    if cfg.api_key.trim().is_empty() {
        req
    } else {
        req.header("Authorization", format!("Bearer {}", cfg.api_key))
    }
}

/// The request for [`prime`]: the normal body capped to a single token;
/// the point is the engine processing (and caching) the prompt, not the
/// reply.
pub fn prime_body(cfg: &NotesConfig, system: &str, user: &str) -> Value {
    let mut body = request_body(cfg, system, user);
    body["max_tokens"] = json!(1);
    body
}

/// Fire a one-token request so the engine caches the prompt prefix; the
/// reply is discarded.
pub async fn prime(cfg: &NotesConfig, system: &str, user: &str) -> Result<()> {
    let url = endpoint(cfg)?;
    let body = prime_body(cfg, system, user);
    let client = reqwest::Client::new();
    apply_headers(cfg, client.post(&url))
        .json(&body)
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

/// Call the configured engine and return the generated text.
pub async fn generate(cfg: &NotesConfig, system: &str, user: &str) -> Result<String> {
    let url = endpoint(cfg)?;
    let body = request_body(cfg, system, user);
    let client = reqwest::Client::new();
    let resp = apply_headers(cfg, client.post(&url))
        .json(&body)
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    parse_response(cfg.provider, &resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(provider: LlmProvider) -> NotesConfig {
        NotesConfig {
            provider,
            ..Default::default()
        }
    }

    #[test]
    fn default_models_per_provider() {
        assert_eq!(model_for(&cfg(LlmProvider::Builtin)), "qwen3-4b");
        assert_eq!(model_for(&cfg(LlmProvider::Custom)), "default");
    }

    #[test]
    fn model_override_wins() {
        let c = NotesConfig {
            provider: LlmProvider::Builtin,
            model: "llama3.2".into(),
            ..Default::default()
        };
        assert_eq!(model_for(&c), "llama3.2");
    }

    #[test]
    fn builtin_uses_the_runtime_endpoint_and_requires_one() {
        let c = NotesConfig {
            provider: LlmProvider::Builtin,
            endpoint: "http://127.0.0.1:8641/v1".into(),
            ..Default::default()
        };
        assert_eq!(
            endpoint(&c).unwrap(),
            "http://127.0.0.1:8641/v1/chat/completions"
        );
        assert!(endpoint(&cfg(LlmProvider::Builtin)).is_err());
    }

    #[test]
    fn custom_requires_an_endpoint_and_speaks_openai() {
        let c = NotesConfig {
            provider: LlmProvider::Custom,
            endpoint: "https://router.example.com/api/v1/".into(),
            ..Default::default()
        };
        assert_eq!(
            endpoint(&c).unwrap(),
            "https://router.example.com/api/v1/chat/completions"
        );
        assert!(endpoint(&cfg(LlmProvider::Custom)).is_err());
        let b = request_body(&c, "SYS", "USR");
        assert_eq!(b["messages"][0]["role"], "system");
    }

    #[test]
    fn openai_body_shape() {
        let c = NotesConfig {
            provider: LlmProvider::Builtin,
            endpoint: "http://127.0.0.1:8641/v1".into(),
            ..Default::default()
        };
        let b = request_body(&c, "SYS", "USR");
        assert_eq!(b["model"], "qwen3-4b");
        assert_eq!(b["messages"][0]["role"], "system");
        assert_eq!(b["messages"][0]["content"], "SYS");
        assert_eq!(b["messages"][1]["role"], "user");
        assert_eq!(b["messages"][1]["content"], "USR");
    }

    #[test]
    fn prime_body_is_the_request_capped_to_one_token() {
        let c = NotesConfig {
            provider: LlmProvider::Builtin,
            endpoint: "http://127.0.0.1:8641/v1".into(),
            ..Default::default()
        };
        let b = prime_body(&c, "SYS", "USR");
        assert_eq!(b["max_tokens"], 1);
        assert_eq!(b["messages"][0]["content"], "SYS");
        assert_eq!(b["messages"][1]["content"], "USR");
    }

    #[test]
    fn parse_openai_response() {
        let r = json!({ "choices": [ { "message": { "content": "# Notes" } } ] });
        assert_eq!(parse_response(LlmProvider::Builtin, &r).unwrap(), "# Notes");
        assert_eq!(parse_response(LlmProvider::Custom, &r).unwrap(), "# Notes");
    }

    #[test]
    fn reasoning_blocks_are_stripped() {
        let r = json!({ "choices": [ { "message": {
            "content": "<think>hmm, chapters…</think>\n# Notes\nbody"
        } } ] });
        assert_eq!(
            parse_response(LlmProvider::Builtin, &r).unwrap(),
            "# Notes\nbody"
        );
        // Unclosed tag passes through untouched.
        assert_eq!(strip_reasoning("<think>oops"), "<think>oops");
    }

    #[test]
    fn parse_rejects_empty_and_wrong_shape() {
        assert!(parse_response(LlmProvider::Builtin, &json!({ "choices": [] })).is_err());
        assert!(parse_response(LlmProvider::Builtin, &json!({ "error": "nope" })).is_err());
        let blank = json!({ "choices": [ { "message": { "content": "   " } } ] });
        assert!(parse_response(LlmProvider::Builtin, &blank).is_err());
    }
}
