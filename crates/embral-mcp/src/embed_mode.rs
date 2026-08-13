//! `embral-mcp embed`: a piped embedding worker. The app cannot link ort
//! (its /MD objects can't share an exe with sherpa's /MT onnxruntime), so
//! it spawns this instead and speaks line-delimited JSON over stdio:
//!
//! ```text
//! → {"passages": ["text", …]}        ← {"vectors": [[f32, …], …]}
//! → {"query": "text"}                ← {"vector": [f32, …]}
//! ```
//!
//! One `{"ready": true}` (or `{"error": …}`) line precedes everything.
//! The process exits when stdin closes; an app crash cleans us up for
//! free. All database writes stay in the app; this mode is pure compute.

use std::io::{BufRead, Write};

use embral_embedder::Embedder;
use serde_json::json;

#[derive(serde::Deserialize)]
struct Request {
    passages: Option<Vec<String>>,
    query: Option<String>,
}

pub fn run() -> anyhow::Result<()> {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    let mut embedder = match Embedder::load_default() {
        Ok(e) => {
            writeln!(out, "{}", json!({ "ready": true }))?;
            out.flush()?;
            e
        }
        Err(e) => {
            writeln!(out, "{}", json!({ "error": format!("{e:#}") }))?;
            out.flush()?;
            return Ok(());
        }
    };

    for line in std::io::stdin().lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        writeln!(out, "{}", handle(&mut embedder, &line))?;
        out.flush()?;
    }
    Ok(())
}

fn handle(embedder: &mut Embedder, line: &str) -> String {
    let result = (|| -> anyhow::Result<serde_json::Value> {
        let request: Request = serde_json::from_str(line)?;
        if let Some(query) = request.query {
            return Ok(json!({ "vector": embedder.embed_query(&query)? }));
        }
        let passages = request.passages.unwrap_or_default();
        let texts: Vec<&str> = passages.iter().map(String::as_str).collect();
        Ok(json!({ "vectors": embedder.embed_passages(&texts)? }))
    })();
    match result {
        Ok(v) => v.to_string(),
        Err(e) => json!({ "error": format!("{e:#}") }).to_string(),
    }
}
