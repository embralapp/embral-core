//! The embedding model's identity: one place for everything the engine
//! needs to know about it. The download itself is managed by the catalog in
//! embral-engine, which repeats these facts (engine can't depend on this
//! crate, this crate can't depend on sherpa); a consistency test in
//! src-tauri pins the pair.

use std::path::PathBuf;

pub const MODEL_ID: &str = "embedding-multilingual";
pub const MODEL_FILE: &str = "model_quantized.onnx";
pub const TOKENIZER_FILE: &str = "tokenizer.json";
pub const MODEL_BYTES: u64 = 118_308_185;
pub const TOKENIZER_BYTES: u64 = 17_082_730;
/// multilingual-e5-small's output width; also the vec0 column width.
pub const DIM: usize = 384;

/// Mirrors `catalog::model_dir(MODEL_ID)`: `%LOCALAPPDATA%/embral/models/{id}`.
pub fn model_dir() -> PathBuf {
    dirs::data_local_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("embral")
        .join("models")
        .join(MODEL_ID)
}

pub fn model_path() -> PathBuf {
    model_dir().join(MODEL_FILE)
}

pub fn tokenizer_path() -> PathBuf {
    model_dir().join(TOKENIZER_FILE)
}

/// Same presence rule as the catalog: every file at least half its
/// published size (the download's truncation floor).
pub fn present() -> bool {
    let ok = |path: PathBuf, bytes: u64| {
        std::fs::metadata(path)
            .map(|m| m.len() * 2 >= bytes)
            .unwrap_or(false)
    };
    ok(model_path(), MODEL_BYTES) && ok(tokenizer_path(), TOKENIZER_BYTES)
}
