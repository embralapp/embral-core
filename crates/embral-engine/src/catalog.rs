//! Model registry + managed downloads.
//!
//! Every model the local engine can run is described here: its files, where
//! they download from, and their published sizes (used as progress-bar
//! denominators and truncation floors, never as exact-equality gates).
//!
//! Two source shapes:
//! - `Files`: individual files fetched from Hugging Face `resolve/main` URLs
//!   (streamed to `{name}.tmp`, renamed on completion; a present non-tmp file
//!   is always whole).
//! - `Archive`: a `.tar.bz2` GitHub release asset from which named members
//!   are extracted (some sherpa-onnx models are published only that way).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use futures_util::StreamExt;
use serde::Serialize;

/// What a file is for, so the engine can find the right path regardless of
/// the file's on-disk name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileRole {
    Encoder,
    Decoder,
    Joiner,
    Tokens,
    CnnBilstm,
    BpeVocab,
    Vad,
    /// Pyannote speaker-segmentation model (who talks when).
    Segmentation,
    /// Speaker-embedding extractor (voice fingerprints for live labels).
    SpeakerEmbedding,
    /// The llama-server executable (built-in LLM runtime).
    LlamaServer,
    /// GGUF language-model weights for the built-in LLM.
    Gguf,
    /// Text-embedding model for semantic search (run by embral-embedder,
    /// not by sherpa).
    TextEmbedding,
    /// HuggingFace tokenizer.json paired with the embedding model.
    TokenizerJson,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelKind {
    StreamingAsr,
    /// Full-context models decoded per VAD speech segment: highest accuracy,
    /// with interims produced by periodic partial decodes.
    OfflineAsr,
    Punctuation,
    /// Speaker diarization + voice-embedding models (who spoke when).
    SpeakerId,
    /// Built-in language model pieces (llama-server runtime, GGUF weights).
    Llm,
    /// Text embeddings for semantic search ([storage.md](../../../docs/storage.md);
    /// consumed by embral-search/embral-embedder, never by sherpa).
    Embedding,
}

impl ModelKind {
    /// Whether this model can drive a transcription session.
    pub fn is_asr(self) -> bool {
        matches!(self, ModelKind::StreamingAsr | ModelKind::OfflineAsr)
    }
}

pub struct ModelFile {
    pub role: FileRole,
    pub name: &'static str,
    pub url: &'static str,
    pub bytes: u64,
}

pub enum ModelSource {
    Files(&'static [ModelFile]),
    Archive {
        url: &'static str,
        bytes: u64,
        /// (role, member basename) pairs to extract from the archive.
        members: &'static [(FileRole, &'static str)],
    },
    /// A `.zip` whose entire contents are extracted flat (an executable and
    /// the DLLs it needs). Presence = the named exe plus a completion marker
    /// written after full extraction.
    ZipAll {
        url: &'static str,
        bytes: u64,
        /// (role, exe basename): the file `role_path` resolves to.
        exe: (FileRole, &'static str),
    },
    /// `ZipAll`'s `.tar.gz` twin (llama.cpp ships tarballs on macOS).
    /// Unix mode bits are preserved so the exe stays executable.
    TarAll {
        url: &'static str,
        bytes: u64,
        /// (role, exe basename): the file `role_path` resolves to.
        exe: (FileRole, &'static str),
    },
}

pub struct KnownModel {
    pub id: &'static str,
    pub display_name: &'static str,
    pub kind: ModelKind,
    pub source: ModelSource,
    pub languages: &'static [&'static str],
    /// One-line description surfaced in the UI.
    pub note: &'static str,
    /// Whether the sherpa runtime supports hotword biasing for this model
    /// (requires `modified_beam_search`).
    pub supports_hotwords: bool,
    /// Whether the model emits punctuated, cased text natively (NeMo-family
    /// models do; icefall Zipformers emit ALL-CAPS unpunctuated text and need
    /// the punctuation model).
    pub native_punctuation: bool,
}

/// A file is considered truncated if below this fraction of its published
/// size (protects against partials that somehow survived without `.tmp`).
const MIN_SIZE_FRACTION: f64 = 0.5;

/// Streaming Zipformer transducers use an fp32 decoder deliberately: the
/// decoder is tiny and quantizing it costs accuracy (upstream guidance).
pub const MODELS: &[KnownModel] = &[
    KnownModel {
        id: "zipformer-en-small",
        display_name: "English fast",
        kind: ModelKind::StreamingAsr,
        languages: &["en"],
        note: "Lowest CPU use for older machines; worse accuracy",
        supports_hotwords: true,
        native_punctuation: false,
        source: ModelSource::Files(&[
            ModelFile {
                role: FileRole::Encoder,
                name: "encoder.int8.onnx",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-en-20M-2023-02-17/resolve/main/encoder-epoch-99-avg-1.int8.onnx",
                bytes: 42_845_182,
            },
            ModelFile {
                role: FileRole::Decoder,
                name: "decoder.onnx",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-en-20M-2023-02-17/resolve/main/decoder-epoch-99-avg-1.onnx",
                bytes: 2_092_272,
            },
            ModelFile {
                role: FileRole::Joiner,
                name: "joiner.int8.onnx",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-en-20M-2023-02-17/resolve/main/joiner-epoch-99-avg-1.int8.onnx",
                bytes: 259_572,
            },
            ModelFile {
                role: FileRole::Tokens,
                name: "tokens.txt",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-en-20M-2023-02-17/resolve/main/tokens.txt",
                bytes: 5_048,
            },
        ]),
    },
    KnownModel {
        id: "zipformer-en",
        display_name: "English balanced",
        kind: ModelKind::StreamingAsr,
        languages: &["en"],
        note: "Lower CPU use; balanced accuracy",
        supports_hotwords: true,
        native_punctuation: false,
        source: ModelSource::Files(&[
            ModelFile {
                role: FileRole::Encoder,
                name: "encoder.int8.onnx",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-en-2023-06-26/resolve/main/encoder-epoch-99-avg-1-chunk-16-left-128.int8.onnx",
                bytes: 71_083_163,
            },
            ModelFile {
                role: FileRole::Decoder,
                name: "decoder.onnx",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-en-2023-06-26/resolve/main/decoder-epoch-99-avg-1-chunk-16-left-128.onnx",
                bytes: 2_092_621,
            },
            ModelFile {
                role: FileRole::Joiner,
                name: "joiner.int8.onnx",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-en-2023-06-26/resolve/main/joiner-epoch-99-avg-1-chunk-16-left-128.int8.onnx",
                bytes: 259_335,
            },
            ModelFile {
                role: FileRole::Tokens,
                name: "tokens.txt",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-en-2023-06-26/resolve/main/tokens.txt",
                bytes: 5_048,
            },
        ]),
    },
    KnownModel {
        id: "parakeet-tdt-en",
        display_name: "English accurate",
        kind: ModelKind::OfflineAsr,
        languages: &["en"],
        note: "Higher CPU use; best accuracy",
        supports_hotwords: true,
        native_punctuation: true,
        source: ModelSource::Files(&[
            ModelFile {
                role: FileRole::Encoder,
                name: "encoder.int8.onnx",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8/resolve/main/encoder.int8.onnx",
                bytes: 652_184_296,
            },
            ModelFile {
                role: FileRole::Decoder,
                name: "decoder.int8.onnx",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8/resolve/main/decoder.int8.onnx",
                bytes: 7_257_753,
            },
            ModelFile {
                role: FileRole::Joiner,
                name: "joiner.int8.onnx",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8/resolve/main/joiner.int8.onnx",
                bytes: 1_739_080,
            },
            ModelFile {
                role: FileRole::Tokens,
                name: "tokens.txt",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8/resolve/main/tokens.txt",
                bytes: 9_384,
            },
            ModelFile {
                role: FileRole::Vad,
                name: "silero_vad.onnx",
                url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx",
                bytes: 643_854,
            },
        ]),
    },
    KnownModel {
        id: "parakeet-tdt-v3",
        display_name: "Multilingual",
        kind: ModelKind::OfflineAsr,
        languages: &["*"],
        note: "25 languages incl. English; high CPU use & accuracy",
        supports_hotwords: true,
        native_punctuation: true,
        source: ModelSource::Files(&[
            ModelFile {
                role: FileRole::Encoder,
                name: "encoder.int8.onnx",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/resolve/main/encoder.int8.onnx",
                bytes: 652_184_281,
            },
            ModelFile {
                role: FileRole::Decoder,
                name: "decoder.int8.onnx",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/resolve/main/decoder.int8.onnx",
                bytes: 11_845_275,
            },
            ModelFile {
                role: FileRole::Joiner,
                name: "joiner.int8.onnx",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/resolve/main/joiner.int8.onnx",
                bytes: 6_355_277,
            },
            ModelFile {
                role: FileRole::Tokens,
                name: "tokens.txt",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/resolve/main/tokens.txt",
                bytes: 93_939,
            },
            ModelFile {
                role: FileRole::Vad,
                name: "silero_vad.onnx",
                url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx",
                bytes: 643_854,
            },
        ]),
    },
    KnownModel {
        id: "speaker-id",
        display_name: "Speaker identification",
        kind: ModelKind::SpeakerId,
        languages: &["*"],
        note: "Tells speakers apart in recordings",
        supports_hotwords: false,
        native_punctuation: false,
        source: ModelSource::Files(&[
            ModelFile {
                role: FileRole::Segmentation,
                name: "segmentation.onnx",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-pyannote-segmentation-3-0/resolve/main/model.onnx",
                bytes: 5_992_913,
            },
            ModelFile {
                role: FileRole::SpeakerEmbedding,
                name: "embedding.onnx",
                // The release tag's spelling ("recongition") is upstream's.
                url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/nemo_en_titanet_small.onnx",
                bytes: 40_257_283,
            },
            ModelFile {
                role: FileRole::Vad,
                name: "silero_vad.onnx",
                url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx",
                bytes: 643_854,
            },
        ]),
    },
    KnownModel {
        id: "punct-en",
        display_name: "English punctuation",
        kind: ModelKind::Punctuation,
        languages: &["en"],
        note: "Adds grammar to fast & balanced models",
        supports_hotwords: false,
        native_punctuation: false,
        source: ModelSource::Archive {
            url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/punctuation-models/sherpa-onnx-online-punct-en-2024-08-06.tar.bz2",
            bytes: 30_667_839,
            // The archive ships fp32 + int8 variants; we extract only the
            // int8 one (7.5 MB): same graph, lighter to load and run.
            members: &[
                (FileRole::CnnBilstm, "model.int8.onnx"),
                (FileRole::BpeVocab, "bpe.vocab"),
            ],
        },
    },
    // The llama.cpp runtime is the one per-platform binary in the catalog;
    // each target carries its own entry (same id, its own artifact;
    // upstream ships a .zip on Windows and a .tar.gz on macOS and Linux).
    #[cfg(windows)]
    KnownModel {
        id: "llama-server",
        display_name: "Summary engine",
        kind: ModelKind::Llm,
        languages: &["*"],
        note: "Local runtime for summaries and dictation cleanup",
        supports_hotwords: false,
        native_punctuation: false,
        source: ModelSource::ZipAll {
            url: "https://github.com/ggml-org/llama.cpp/releases/download/b9925/llama-b9925-bin-win-cpu-x64.zip",
            bytes: 17_507_535,
            exe: (FileRole::LlamaServer, "llama-server.exe"),
        },
    },
    #[cfg(target_os = "macos")]
    KnownModel {
        id: "llama-server",
        display_name: "Summary engine",
        kind: ModelKind::Llm,
        languages: &["*"],
        note: "Local runtime for summaries and dictation cleanup",
        supports_hotwords: false,
        native_punctuation: false,
        source: ModelSource::TarAll {
            url: "https://github.com/ggml-org/llama.cpp/releases/download/b9925/llama-b9925-bin-macos-arm64.tar.gz",
            bytes: 11_146_856,
            exe: (FileRole::LlamaServer, "llama-server"),
        },
    },
    // Upstream's ubuntu-x64 build. It is glibc-linked against Ubuntu's, so
    // it runs on the port's declared floor (Debian 12 / Ubuntu 22.04, glibc
    // 2.35) and newer. A future catalog bump has to keep checking that,
    // or this entry moves to a self-built artifact
    // ([260801-linux-port.md](../../../docs/plans/260801-linux-port.md)).
    #[cfg(target_os = "linux")]
    KnownModel {
        id: "llama-server",
        display_name: "Summary engine",
        kind: ModelKind::Llm,
        languages: &["*"],
        note: "Local runtime for summaries and dictation cleanup",
        supports_hotwords: false,
        native_punctuation: false,
        source: ModelSource::TarAll {
            url: "https://github.com/ggml-org/llama.cpp/releases/download/b9925/llama-b9925-bin-ubuntu-x64.tar.gz",
            bytes: 15_898_134,
            exe: (FileRole::LlamaServer, "llama-server"),
        },
    },
    KnownModel {
        id: "qwen3-4b",
        display_name: "Built-in language model",
        kind: ModelKind::Llm,
        languages: &["*"],
        note: "Local LLM for summaries and dictation clean-up",
        supports_hotwords: false,
        native_punctuation: false,
        source: ModelSource::Files(&[ModelFile {
            role: FileRole::Gguf,
            name: "qwen3-4b-instruct.gguf",
            url: "https://huggingface.co/unsloth/Qwen3-4B-Instruct-2507-GGUF/resolve/main/Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
            bytes: 2_497_281_120,
        }]),
    },
    KnownModel {
        // Identity mirrored in embral-search/src/model.rs (this crate can't
        // depend on ort, that one can't depend on sherpa); a consistency
        // test in src-tauri keeps the pair honest.
        id: "embedding-multilingual",
        display_name: "Semantic search",
        kind: ModelKind::Embedding,
        languages: &["*"],
        note: "Search by meaning, in app and via MCP",
        supports_hotwords: false,
        native_punctuation: false,
        source: ModelSource::Files(&[
            ModelFile {
                role: FileRole::TextEmbedding,
                name: "model_quantized.onnx",
                url: "https://huggingface.co/Xenova/multilingual-e5-small/resolve/main/onnx/model_quantized.onnx",
                bytes: 118_308_185,
            },
            ModelFile {
                role: FileRole::TokenizerJson,
                name: "tokenizer.json",
                url: "https://huggingface.co/Xenova/multilingual-e5-small/resolve/main/tokenizer.json",
                bytes: 17_082_730,
            },
        ]),
    },
];

pub fn find(id: &str) -> Option<&'static KnownModel> {
    MODELS.iter().find(|m| m.id == id)
}

/// `%LOCALAPPDATA%/embral/models`: machine-local replaceable blobs, separate
/// from the user's storage_dir. (The retired Parakeet files lived under
/// `models/parakeet`; these live under `models/{model_id}`.)
pub fn models_root() -> PathBuf {
    dirs::data_local_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("embral")
        .join("models")
}

pub fn model_dir(id: &str) -> PathBuf {
    models_root().join(id)
}

/// Marker written after a `ZipAll` archive is fully extracted (the exe alone
/// can't prove its DLL siblings all made it).
const ZIP_COMPLETE_MARKER: &str = ".extracted";

impl KnownModel {
    /// (role, on-disk filename) for every file this model needs locally.
    pub fn expected_files(&self) -> Vec<(FileRole, &'static str)> {
        match &self.source {
            ModelSource::Files(files) => files.iter().map(|f| (f.role, f.name)).collect(),
            ModelSource::Archive { members, .. } => members.to_vec(),
            ModelSource::ZipAll { exe, .. } | ModelSource::TarAll { exe, .. } => vec![*exe],
        }
    }

    pub fn role_path(&self, role: FileRole) -> Option<PathBuf> {
        self.expected_files()
            .iter()
            .find(|(r, _)| *r == role)
            .map(|(_, name)| model_dir(self.id).join(name))
    }

    pub fn total_bytes(&self) -> u64 {
        match &self.source {
            ModelSource::Files(files) => files.iter().map(|f| f.bytes).sum(),
            ModelSource::Archive { bytes, .. }
            | ModelSource::ZipAll { bytes, .. }
            | ModelSource::TarAll { bytes, .. } => *bytes,
        }
    }

    pub fn present(&self) -> bool {
        let dir = model_dir(self.id);
        match &self.source {
            ModelSource::Files(files) => files.iter().all(|f| {
                std::fs::metadata(dir.join(f.name))
                    .map(|m| m.len() as f64 >= f.bytes as f64 * MIN_SIZE_FRACTION)
                    .unwrap_or(false)
            }),
            // Extracted members' exact sizes aren't published; extraction is
            // atomic per-member (tmp+rename), so existence suffices.
            ModelSource::Archive { members, .. } => {
                members.iter().all(|(_, name)| dir.join(name).is_file())
            }
            ModelSource::ZipAll { exe, .. } | ModelSource::TarAll { exe, .. } => {
                dir.join(exe.1).is_file() && dir.join(ZIP_COMPLETE_MARKER).is_file()
            }
        }
    }
}

/// Status surfaced to the settings UI.
#[derive(Debug, Clone, Serialize)]
pub struct ModelStatus {
    pub id: String,
    pub display_name: String,
    pub kind: ModelKind,
    pub note: String,
    /// ISO codes, or `["*"]` for language-independent models: the UI's
    /// language facet.
    pub languages: Vec<String>,
    pub present: bool,
    pub total_bytes: u64,
    pub dir: String,
    pub supports_hotwords: bool,
    pub native_punctuation: bool,
}

pub fn statuses() -> Vec<ModelStatus> {
    MODELS
        .iter()
        .map(|m| ModelStatus {
            id: m.id.to_string(),
            display_name: m.display_name.to_string(),
            kind: m.kind,
            note: m.note.to_string(),
            languages: m.languages.iter().map(|s| s.to_string()).collect(),
            present: m.present(),
            total_bytes: m.total_bytes(),
            dir: model_dir(m.id).to_string_lossy().to_string(),
            supports_hotwords: m.supports_hotwords,
            native_punctuation: m.native_punctuation,
        })
        .collect()
}

/// Byte-level progress for one model download.
#[derive(Debug, Clone, Serialize)]
pub struct DownloadProgress {
    pub model_id: String,
    pub file_name: String,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
}

/// Emit progress at most every this many bytes, to avoid flooding listeners.
const PROGRESS_STRIDE: u64 = 4 * 1024 * 1024;

/// Download every missing file of `model_id` into its managed dir. Already
/// complete files/members are skipped, so re-runs resume at file granularity.
pub async fn download(model_id: &str, progress: impl Fn(DownloadProgress) + Send) -> Result<()> {
    let model = find(model_id).ok_or_else(|| anyhow!("unknown model id: {model_id}"))?;
    let dir = model_dir(model_id);
    tokio::fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("create model dir {}", dir.display()))?;

    // Clear stale partials from a previously-killed run.
    if let Ok(mut entries) = tokio::fs::read_dir(&dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            if entry.path().extension().is_some_and(|e| e == "tmp") {
                let _ = tokio::fs::remove_file(entry.path()).await;
            }
        }
    }

    let client = reqwest::Client::new();
    let total = model.total_bytes();

    match &model.source {
        ModelSource::Files(files) => {
            let mut completed: u64 = 0;
            for f in *files {
                let dest = dir.join(f.name);
                if std::fs::metadata(&dest)
                    .map(|m| m.len() as f64 >= f.bytes as f64 * MIN_SIZE_FRACTION)
                    .unwrap_or(false)
                {
                    completed += f.bytes;
                    progress(DownloadProgress {
                        model_id: model_id.to_string(),
                        file_name: f.name.to_string(),
                        downloaded_bytes: completed,
                        total_bytes: total,
                    });
                    continue;
                }
                stream_to_file(&client, f.url, &dest, |file_bytes| {
                    progress(DownloadProgress {
                        model_id: model_id.to_string(),
                        file_name: f.name.to_string(),
                        downloaded_bytes: completed + file_bytes,
                        total_bytes: total,
                    })
                })
                .await
                .with_context(|| format!("download {}", f.name))?;
                completed += f.bytes;
            }
        }
        ModelSource::Archive { url, members, .. } => {
            if model.present() {
                return Ok(());
            }
            let archive_name = "archive.tar.bz2";
            let tmp = dir.join(archive_name);
            stream_to_file(&client, url, &tmp, |bytes| {
                progress(DownloadProgress {
                    model_id: model_id.to_string(),
                    file_name: archive_name.to_string(),
                    downloaded_bytes: bytes,
                    total_bytes: total,
                })
            })
            .await
            .context("download model archive")?;

            // Extraction is CPU-bound; run it off the async thread.
            let tmp2 = tmp.clone();
            let dir2 = dir.clone();
            let members2: Vec<(FileRole, &'static str)> = members.to_vec();
            tokio::task::spawn_blocking(move || extract_members(&tmp2, &dir2, &members2))
                .await
                .context("extract task panicked")??;
            let _ = tokio::fs::remove_file(&tmp).await;

            if !model.present() {
                bail!("archive did not contain all expected model files");
            }
        }
        ModelSource::ZipAll { url, exe, .. } | ModelSource::TarAll { url, exe, .. } => {
            if model.present() {
                return Ok(());
            }
            let is_tar = matches!(&model.source, ModelSource::TarAll { .. });
            let archive_name = if is_tar { "archive.tar.gz" } else { "archive.zip" };
            let tmp = dir.join(archive_name);
            stream_to_file(&client, url, &tmp, |bytes| {
                progress(DownloadProgress {
                    model_id: model_id.to_string(),
                    file_name: archive_name.to_string(),
                    downloaded_bytes: bytes,
                    total_bytes: total,
                })
            })
            .await
            .context("download archive")?;

            let tmp2 = tmp.clone();
            let dir2 = dir.clone();
            tokio::task::spawn_blocking(move || {
                if is_tar {
                    extract_tar_all(&tmp2, &dir2)
                } else {
                    extract_zip_all(&tmp2, &dir2)
                }
            })
            .await
            .context("extract task panicked")??;
            let _ = tokio::fs::remove_file(&tmp).await;

            if !dir.join(exe.1).is_file() {
                bail!("archive did not contain {}", exe.1);
            }
            std::fs::write(dir.join(ZIP_COMPLETE_MARKER), b"")
                .context("write extraction marker")?;
        }
    }
    Ok(())
}

/// Extract every file in a `.zip` flat into `dir` (leading archive folders
/// stripped), each via tmp+rename.
fn extract_zip_all(archive: &Path, dir: &Path) -> Result<()> {
    let file = std::fs::File::open(archive).context("open downloaded archive")?;
    let mut zip = zip::ZipArchive::new(file).context("read zip archive")?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        if entry.is_dir() {
            continue;
        }
        let Some(base) = entry
            .enclosed_name()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        else {
            continue;
        };
        let tmp = dir.join(format!("{base}.extract.tmp"));
        {
            let mut out =
                std::fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
            std::io::copy(&mut entry, &mut out)
                .with_context(|| format!("write {}", tmp.display()))?;
        }
        let dest = dir.join(&base);
        std::fs::rename(&tmp, &dest)
            .with_context(|| format!("replace {} (is it held open?)", dest.display()))?;
    }
    Ok(())
}

/// Extract every file in a `.tar.gz` flat into `dir` (leading archive
/// folders stripped), each via tmp+rename: `extract_zip_all`'s tarball
/// twin. Unix mode bits carry over, so executables stay executable, and
/// symlink entries are recreated after the files are written; llama.cpp's
/// dylib version chains (`libggml.0.dylib -> libggml.0.15.3.dylib`) are
/// symlinks, and the exe links the versioned names.
fn extract_tar_all(archive: &Path, dir: &Path) -> Result<()> {
    let file = std::fs::File::open(archive).context("open downloaded archive")?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut tar = tar::Archive::new(decoder);
    // (link basename, target basename), created after the real files so a
    // link never dangles mid-extraction.
    let mut links: Vec<(String, String)> = Vec::new();
    for entry in tar.entries().context("read archive entries")? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let Some(base) = path.file_name().map(|n| n.to_string_lossy().to_string()) else {
            continue;
        };
        if entry.header().entry_type().is_symlink() {
            if let Some(target) = entry
                .link_name()
                .ok()
                .flatten()
                .and_then(|t| t.file_name().map(|n| n.to_string_lossy().to_string()))
            {
                links.push((base, target));
            }
            continue;
        }
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let tmp = dir.join(format!("{base}.extract.tmp"));
        {
            let mut out =
                std::fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
            std::io::copy(&mut entry, &mut out)
                .with_context(|| format!("write {}", tmp.display()))?;
        }
        #[cfg(unix)]
        if let Ok(mode) = entry.header().mode() {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode));
        }
        let dest = dir.join(&base);
        std::fs::rename(&tmp, &dest)
            .with_context(|| format!("replace {} (is it held open?)", dest.display()))?;
    }
    #[cfg(unix)]
    for (base, target) in links {
        let dest = dir.join(&base);
        let _ = std::fs::remove_file(&dest);
        std::os::unix::fs::symlink(&target, &dest)
            .with_context(|| format!("link {} -> {target}", dest.display()))?;
    }
    Ok(())
}

/// Stream `url` to `dest` via `{dest}.tmp` + rename, reporting bytes written.
async fn stream_to_file(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    progress: impl Fn(u64),
) -> Result<()> {
    use tokio::io::AsyncWriteExt;

    let tmp = dest.with_extension(match dest.extension() {
        Some(e) => format!("{}.tmp", e.to_string_lossy()),
        None => "tmp".to_string(),
    });
    let resp = client
        .get(url)
        .send()
        .await
        .and_then(|r| r.error_for_status())?;

    let mut file = tokio::fs::File::create(&tmp)
        .await
        .with_context(|| format!("create {}", tmp.display()))?;
    let mut written: u64 = 0;
    let mut last_emit: u64 = 0;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk).await?;
        written += chunk.len() as u64;
        if written - last_emit >= PROGRESS_STRIDE {
            last_emit = written;
            progress(written);
        }
    }
    file.flush().await?;
    drop(file);
    tokio::fs::rename(&tmp, dest)
        .await
        .with_context(|| format!("replace {} (is it held open?)", dest.display()))?;
    progress(written);
    Ok(())
}

/// Extract the named members (matched by basename, any leading archive dirs
/// ignored) from a `.tar.bz2` into `dir`, each via tmp+rename.
fn extract_members(archive: &Path, dir: &Path, members: &[(FileRole, &'static str)]) -> Result<()> {
    let file = std::fs::File::open(archive).context("open downloaded archive")?;
    let decoder = bzip2::read::BzDecoder::new(file);
    let mut tar = tar::Archive::new(decoder);

    let wanted: HashMap<&str, ()> = members.iter().map(|(_, n)| (*n, ())).collect();
    for entry in tar.entries().context("read archive entries")? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let Some(base) = path.file_name().and_then(|n| n.to_str()).map(String::from) else {
            continue;
        };
        if !wanted.contains_key(base.as_str()) {
            continue;
        }
        let tmp = dir.join(format!("{base}.extract.tmp"));
        {
            let mut out =
                std::fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
            std::io::copy(&mut entry, &mut out)
                .with_context(|| format!("write {}", tmp.display()))?;
        }
        let dest = dir.join(&base);
        std::fs::rename(&tmp, &dest)
            .with_context(|| format!("replace {} (is it held open?)", dest.display()))?;
    }
    Ok(())
}

/// Delete a model's managed directory.
pub fn delete(model_id: &str) -> Result<()> {
    let dir = model_dir(model_id);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for m in MODELS {
            assert!(seen.insert(m.id), "duplicate model id {}", m.id);
        }
    }

    #[test]
    fn asr_models_have_all_roles() {
        for m in MODELS.iter().filter(|m| m.kind.is_asr()) {
            let roles: Vec<FileRole> = m.expected_files().iter().map(|(r, _)| *r).collect();
            for needed in [
                FileRole::Encoder,
                FileRole::Decoder,
                FileRole::Joiner,
                FileRole::Tokens,
            ] {
                assert!(roles.contains(&needed), "{} missing {:?}", m.id, needed);
            }
            // Offline decoding is VAD-segmented; the VAD model is included.
            if m.kind == ModelKind::OfflineAsr {
                assert!(roles.contains(&FileRole::Vad), "{} missing Vad", m.id);
            }
        }
    }

    #[test]
    fn punctuation_model_has_required_roles() {
        let m = find("punct-en").unwrap();
        let roles: Vec<FileRole> = m.expected_files().iter().map(|(r, _)| *r).collect();
        assert!(roles.contains(&FileRole::CnnBilstm));
        assert!(roles.contains(&FileRole::BpeVocab));
    }

    #[test]
    fn speaker_id_model_has_required_roles() {
        let m = find("speaker-id").unwrap();
        assert_eq!(m.kind, ModelKind::SpeakerId);
        assert!(!m.kind.is_asr());
        let roles: Vec<FileRole> = m.expected_files().iter().map(|(r, _)| *r).collect();
        assert!(roles.contains(&FileRole::Segmentation));
        assert!(roles.contains(&FileRole::SpeakerEmbedding));
        assert!(roles.contains(&FileRole::Vad));
    }

    #[test]
    fn statuses_report_absent_models() {
        // Model dirs almost certainly don't exist under a test-only root; the
        // key property is that statuses() never panics and reports every model.
        let s = statuses();
        assert_eq!(s.len(), MODELS.len());
        assert!(s.iter().all(|st| st.total_bytes > 0));
    }

    #[cfg(windows)]
    #[test]
    fn llm_pack_has_runtime_and_weights() {
        let runtime = find("llama-server").unwrap();
        assert_eq!(runtime.kind, ModelKind::Llm);
        assert!(!runtime.kind.is_asr());
        assert_eq!(
            runtime
                .role_path(FileRole::LlamaServer)
                .unwrap()
                .file_name()
                .unwrap(),
            "llama-server.exe"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn llm_pack_has_runtime_and_weights() {
        let runtime = find("llama-server").unwrap();
        assert_eq!(runtime.kind, ModelKind::Llm);
        assert!(matches!(runtime.source, ModelSource::TarAll { .. }));
        assert_eq!(
            runtime
                .role_path(FileRole::LlamaServer)
                .unwrap()
                .file_name()
                .unwrap(),
            "llama-server"
        );
    }

    /// Without a Linux arm in the catalog every summary path here dies with
    /// "no runtime entry", so this test is the guard on that whole feature
    /// existing on the platform at all.
    #[cfg(target_os = "linux")]
    #[test]
    fn llm_pack_has_runtime_and_weights() {
        let runtime = find("llama-server").unwrap();
        assert_eq!(runtime.kind, ModelKind::Llm);
        assert!(!runtime.kind.is_asr());
        // A tar, like macOS; the tar path's mode-bit and symlink
        // handling is already `cfg(unix)`, so it carries over unchanged.
        assert!(matches!(runtime.source, ModelSource::TarAll { .. }));
        assert_eq!(
            runtime
                .role_path(FileRole::LlamaServer)
                .unwrap()
                .file_name()
                .unwrap(),
            "llama-server"
        );
        // The weights are portable; only the runtime is per-target.
        assert!(find("qwen3-4b").is_some());
    }

    /// Live probe of the real runtime download + spawn; run manually:
    /// `cargo test -p embral-engine --lib llama_runtime_downloads -- --ignored --nocapture`.
    /// Proves the tar source end-to-end on this machine: the archive
    /// extracts flat, the exe keeps its mode bits, and its @rpath dylibs
    /// resolve from the flattened dir.
    ///
    /// On Linux the same probe carries more weight: it is the only thing
    /// that proves upstream's ubuntu-x64 build actually runs on the
    /// distribution in front of you, glibc floor and bundled `.so`s
    /// included. A green `cargo test` says nothing about that; only
    /// spawning the binary does.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[tokio::test]
    #[ignore = "manual probe; downloads the ~11 MB runtime and spawns it"]
    async fn llama_runtime_downloads_and_runs() {
        download("llama-server", |p| {
            eprintln!("{} / {} bytes", p.downloaded_bytes, p.total_bytes)
        })
        .await
        .expect("download + extract");
        let exe = find("llama-server")
            .unwrap()
            .role_path(FileRole::LlamaServer)
            .unwrap();
        let out = std::process::Command::new(&exe)
            .arg("--version")
            .output()
            .expect("spawn llama-server");
        eprintln!(
            "exit: {:?}\nstderr: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(out.status.success(), "llama-server --version must run");
    }

    #[test]
    fn llm_weights_are_platform_neutral() {
        let weights = find("qwen3-4b").unwrap();
        assert_eq!(weights.kind, ModelKind::Llm);
        assert!(weights.role_path(FileRole::Gguf).is_some());
    }

    #[test]
    fn extract_zip_all_flattens_and_skips_dirs() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let archive_path = tmp.path().join("a.zip");
        {
            let f = std::fs::File::create(&archive_path)?;
            let mut w = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            w.add_directory("bin/", opts)?;
            w.start_file("bin/tool.exe", opts)?;
            std::io::Write::write_all(&mut w, b"exe-bytes")?;
            w.start_file("bin/helper.dll", opts)?;
            std::io::Write::write_all(&mut w, b"dll-bytes")?;
            w.finish()?;
        }
        let out = tmp.path().join("out");
        std::fs::create_dir_all(&out)?;
        extract_zip_all(&archive_path, &out)?;
        assert_eq!(std::fs::read(out.join("tool.exe"))?, b"exe-bytes");
        assert_eq!(std::fs::read(out.join("helper.dll"))?, b"dll-bytes");
        assert!(!out.join("bin").exists(), "flattened, no subdirs");
        Ok(())
    }

    #[test]
    fn extract_tar_all_flattens_and_keeps_modes() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let archive_path = tmp.path().join("a.tar.gz");
        {
            let f = std::fs::File::create(&archive_path)?;
            let gz = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
            let mut w = tar::Builder::new(gz);
            let add = |w: &mut tar::Builder<_>, path: &str, mode: u32, body: &[u8]| {
                let mut h = tar::Header::new_gnu();
                h.set_size(body.len() as u64);
                h.set_mode(mode);
                h.set_cksum();
                w.append_data(&mut h, path, body)
            };
            add(&mut w, "build/bin/llama-server", 0o755, b"exe-bytes")?;
            add(&mut w, "build/bin/libggml.0.1.dylib", 0o644, b"lib-bytes")?;
            // The version-chain symlink llama.cpp ships beside its dylibs.
            let mut h = tar::Header::new_gnu();
            h.set_size(0);
            h.set_entry_type(tar::EntryType::Symlink);
            h.set_cksum();
            w.append_link(&mut h, "build/bin/libggml.0.dylib", "libggml.0.1.dylib")?;
            w.into_inner()?.finish()?;
        }
        let out = tmp.path().join("out");
        std::fs::create_dir_all(&out)?;
        extract_tar_all(&archive_path, &out)?;
        assert_eq!(std::fs::read(out.join("llama-server"))?, b"exe-bytes");
        assert_eq!(std::fs::read(out.join("libggml.0.1.dylib"))?, b"lib-bytes");
        assert!(!out.join("build").exists(), "flattened, no subdirs");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(out.join("llama-server"))?.permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "the exe keeps its executable bits");
            // The version-chain symlink resolves through to the real dylib.
            let link = out.join("libggml.0.dylib");
            assert!(std::fs::symlink_metadata(&link)?.file_type().is_symlink());
            assert_eq!(std::fs::read(&link)?, b"lib-bytes");
        }
        Ok(())
    }

    #[test]
    fn extract_members_pulls_named_files() -> Result<()> {
        // Build a tiny tar.bz2 fixture: model-dir/wanted.bin + model-dir/skip.me
        let tmp = tempfile::tempdir()?;
        let archive_path = tmp.path().join("a.tar.bz2");
        {
            let f = std::fs::File::create(&archive_path)?;
            let enc = bzip2::write::BzEncoder::new(f, bzip2::Compression::fast());
            let mut builder = tar::Builder::new(enc);
            let mut header = tar::Header::new_gnu();
            header.set_size(5);
            header.set_cksum();
            builder.append_data(&mut header, "model-dir/wanted.bin", &b"hello"[..])?;
            let mut header2 = tar::Header::new_gnu();
            header2.set_size(4);
            header2.set_cksum();
            builder.append_data(&mut header2, "model-dir/skip.me", &b"nope"[..])?;
            builder.into_inner()?.finish()?;
        }
        let out_dir = tmp.path().join("out");
        std::fs::create_dir_all(&out_dir)?;
        extract_members(
            &archive_path,
            &out_dir,
            &[(FileRole::CnnBilstm, "wanted.bin")],
        )?;
        assert_eq!(std::fs::read(out_dir.join("wanted.bin"))?, b"hello");
        assert!(!out_dir.join("skip.me").exists());
        Ok(())
    }
}
