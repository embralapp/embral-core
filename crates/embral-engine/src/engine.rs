//! The warm engine: loads recognizers once per app run and hands out
//! per-recording sessions.
//!
//! Keeping loaded models cached here is what makes recording start instant;
//! the old provider re-loaded ONNX weights at every record start. Loading is
//! synchronous and can take a few seconds cold; callers should invoke
//! `create_session` from a blocking context.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, Context, Result};
use sherpa_onnx::{
    OfflineRecognizer, OfflineRecognizerConfig, OfflineSpeakerDiarization,
    OfflineSpeakerDiarizationConfig, OnlinePunctuation, OnlinePunctuationConfig,
    OnlinePunctuationModelConfig, OnlineRecognizer, OnlineRecognizerConfig,
    SpeakerEmbeddingExtractor, SpeakerEmbeddingExtractorConfig, VadModelConfig,
    VoiceActivityDetector,
};

use crate::catalog::{self, FileRole, ModelKind};
use crate::hotwords;
use crate::session::LocalSession;

/// Endpoint rule defaults (sherpa-onnx upstream defaults): rule1 fires after
/// this much trailing silence even with no text; rule2 after this much
/// trailing silence once something was decoded; rule3 caps utterance length.
const RULE1_TRAILING_SILENCE_SECS: f32 = 2.4;
const RULE2_TRAILING_SILENCE_SECS: f32 = 1.2;
const RULE3_UTTERANCE_LENGTH_SECS: f32 = 20.0;
const HOTWORDS_SCORE: f32 = 1.5;
const NUM_THREADS: i32 = 2;
/// Offline decodes are batch-shaped and latency-tolerant; give them more
/// threads so long segments and imports finish quickly.
const OFFLINE_NUM_THREADS: i32 = 4;

// Silero VAD settings for the offline (segment-decoded) mode.
const VAD_THRESHOLD: f32 = 0.5;
const VAD_MIN_SILENCE_SECS: f32 = 0.5;
const VAD_MIN_SPEECH_SECS: f32 = 0.25;
const VAD_WINDOW_SAMPLES: i32 = 512;
/// Force-split runaway monologues so finals arrive and interim re-decodes
/// stay bounded.
const VAD_MAX_SPEECH_SECS: f32 = 20.0;
const VAD_BUFFER_SECS: f32 = 60.0;

/// Diarization turns shorter than this are noise; gaps shorter than the off
/// value merge adjacent turns (sherpa defaults).
const DIARIZATION_MIN_ON_SECS: f32 = 0.3;
const DIARIZATION_MIN_OFF_SECS: f32 = 0.5;

/// Cache key: hotword decoding needs `modified_beam_search`, plain decoding
/// uses the cheaper `greedy_search`, so they are distinct recognizers.
type RecognizerKey = (String, bool);

/// Catalog id of the diarization + voice-embedding model pack.
pub const SPEAKER_ID_MODEL: &str = "speaker-id";

/// One diarized stretch of speech: `cluster` is a per-recording speaker index
/// (0-based) with no meaning across recordings.
#[derive(Debug, Clone, PartialEq)]
pub struct DiarizedSpan {
    pub start: f64,
    pub end: f64,
    pub cluster: usize,
}

#[derive(Default)]
pub struct Engine {
    recognizers: Mutex<HashMap<RecognizerKey, Arc<OnlineRecognizer>>>,
    offline_recognizers: Mutex<HashMap<RecognizerKey, Arc<OfflineRecognizer>>>,
    punctuation: Mutex<Option<Arc<OnlinePunctuation>>>,
    /// Keyed by the clustering threshold's bits: the threshold is baked
    /// into a diarizer at construction, and the sensitivity setting varies
    /// it (a handful of values at most).
    diarizers: Mutex<HashMap<u32, Arc<OfflineSpeakerDiarization>>>,
    embedder: Mutex<Option<Arc<SpeakerEmbeddingExtractor>>>,
}

impl Engine {
    pub fn new() -> Engine {
        Engine::default()
    }

    /// Whether the given ASR model (streaming or offline) is downloaded.
    pub fn model_present(&self, model_id: &str) -> bool {
        catalog::find(model_id)
            .filter(|m| m.kind.is_asr())
            .is_some_and(|m| m.present())
    }

    /// Drop any cached recognizer for `model_id` (called after model deletion).
    pub fn evict(&self, model_id: &str) {
        self.recognizers
            .lock()
            .expect("engine mutex poisoned")
            .retain(|(id, _), _| id != model_id);
        self.offline_recognizers
            .lock()
            .expect("engine mutex poisoned")
            .retain(|(id, _), _| id != model_id);
        if model_id == SPEAKER_ID_MODEL {
            self.diarizers.lock().expect("engine mutex poisoned").clear();
            *self.embedder.lock().expect("engine mutex poisoned") = None;
        }
    }

    /// Whether the speaker-identification model pack is downloaded.
    pub fn speaker_id_present(&self) -> bool {
        catalog::find(SPEAKER_ID_MODEL).is_some_and(|m| m.present())
    }

    /// Split a full 16 kHz mono recording into per-speaker spans. Cluster
    /// indices are recording-local. `clustering_threshold` comes from the
    /// speaker-sensitivity setting (larger = fewer speakers). CPU-bound;
    /// call from a blocking context.
    pub fn diarize(&self, samples: &[f32], clustering_threshold: f32) -> Result<Vec<DiarizedSpan>> {
        let diarizer = self.diarizer(clustering_threshold)?;
        let result = diarizer
            .process(samples)
            .ok_or_else(|| anyhow!("speaker diarization failed"))?;
        Ok(result
            .sort_by_start_time()
            .into_iter()
            .filter(|s| s.speaker >= 0)
            .map(|s| DiarizedSpan {
                start: s.start as f64,
                end: s.end as f64,
                cluster: s.speaker as usize,
            })
            .collect())
    }

    /// Compute a voice embedding for a 16 kHz mono clip.
    pub fn embed(&self, samples: &[f32]) -> Result<Vec<f32>> {
        let embedder = self.embedder()?;
        let stream = embedder
            .create_stream()
            .ok_or_else(|| anyhow!("failed to open embedding stream"))?;
        stream.accept_waveform(16_000, samples);
        stream.input_finished();
        embedder
            .compute(&stream)
            .ok_or_else(|| anyhow!("voice embedding failed (clip too short?)"))
    }

    fn diarizer(&self, clustering_threshold: f32) -> Result<Arc<OfflineSpeakerDiarization>> {
        let key = clustering_threshold.to_bits();
        {
            let guard = self.diarizers.lock().expect("engine mutex poisoned");
            if let Some(d) = guard.get(&key) {
                return Ok(d.clone());
            }
        }
        let model = self.speaker_id_model()?;
        let mut config = OfflineSpeakerDiarizationConfig::default();
        config.segmentation.pyannote.model = Some(
            model
                .role_path(FileRole::Segmentation)
                .ok_or_else(|| anyhow!("speaker-id model missing segmentation file"))?
                .to_string_lossy()
                .to_string(),
        );
        config.segmentation.num_threads = OFFLINE_NUM_THREADS;
        config.embedding = self.embedding_config(model)?;
        config.clustering.num_clusters = -1; // estimate from the audio
        config.clustering.threshold = clustering_threshold;
        config.min_duration_on = DIARIZATION_MIN_ON_SECS;
        config.min_duration_off = DIARIZATION_MIN_OFF_SECS;

        tracing::info!(clustering_threshold, "loading speaker diarization models");
        let started = std::time::Instant::now();
        let diarizer = OfflineSpeakerDiarization::create(&config).ok_or_else(|| {
            anyhow!("failed to load the speaker identification models — re-download them from Settings")
        })?;
        tracing::info!(
            elapsed_ms = started.elapsed().as_millis() as u64,
            "speaker diarization models loaded"
        );
        let arc = Arc::new(diarizer);
        self.diarizers
            .lock()
            .expect("engine mutex poisoned")
            .insert(key, arc.clone());
        Ok(arc)
    }

    fn embedder(&self) -> Result<Arc<SpeakerEmbeddingExtractor>> {
        {
            let guard = self.embedder.lock().expect("engine mutex poisoned");
            if let Some(e) = guard.as_ref() {
                return Ok(e.clone());
            }
        }
        let model = self.speaker_id_model()?;
        let config = self.embedding_config(model)?;
        tracing::info!("loading speaker embedding model");
        let extractor = SpeakerEmbeddingExtractor::create(&config).ok_or_else(|| {
            anyhow!("failed to load the voice embedding model — re-download it from Settings")
        })?;
        let arc = Arc::new(extractor);
        *self.embedder.lock().expect("engine mutex poisoned") = Some(arc.clone());
        Ok(arc)
    }

    fn speaker_id_model(&self) -> Result<&'static catalog::KnownModel> {
        let model = catalog::find(SPEAKER_ID_MODEL)
            .ok_or_else(|| anyhow!("speaker-id model missing from catalog"))?;
        if !model.present() {
            bail!("the speaker identification models are not downloaded — open Settings to download them");
        }
        Ok(model)
    }

    fn embedding_config(
        &self,
        model: &catalog::KnownModel,
    ) -> Result<SpeakerEmbeddingExtractorConfig> {
        let mut config = SpeakerEmbeddingExtractorConfig::default();
        config.model = Some(
            model
                .role_path(FileRole::SpeakerEmbedding)
                .ok_or_else(|| anyhow!("speaker-id model missing embedding file"))?
                .to_string_lossy()
                .to_string(),
        );
        config.num_threads = OFFLINE_NUM_THREADS;
        Ok(config)
    }

    /// Open a session on `model_id`, boosting `vocabulary` phrases when the
    /// model supports it. Loads and caches the recognizer on first use.
    ///
    /// `live_speaker_labels` asks for provisional per-utterance speaker
    /// labels (meeting recordings want them; dictation and imports don't).
    /// Honored only in the VAD-segmented mode with the speaker-id pack
    /// downloaded; otherwise the session runs unlabeled.
    pub fn create_session(
        &self,
        model_id: &str,
        vocabulary: &[String],
        live_speaker_labels: bool,
    ) -> Result<LocalSession> {
        let model = catalog::find(model_id)
            .filter(|m| m.kind.is_asr())
            .ok_or_else(|| anyhow!("unknown local ASR model: {model_id}"))?;
        if !model.present() {
            bail!(
                "local model '{}' is not downloaded — open Settings to download it",
                model.display_name
            );
        }

        // Encode vocabulary into hotword token lines (empty => no boosting).
        // Some models can't take hotwords at all (the sherpa runtime rejects
        // non-greedy decoding for them); the vocabulary is skipped with a log
        // line rather than failing the recording.
        let hotword_lines = if vocabulary.is_empty() {
            String::new()
        } else if !model.supports_hotwords {
            tracing::warn!(
                "vocabulary boost is not supported by model {} — ignoring {} phrase(s)",
                model_id,
                vocabulary.len()
            );
            String::new()
        } else {
            let tokens_path = model
                .role_path(FileRole::Tokens)
                .ok_or_else(|| anyhow!("model {model_id} has no tokens file"))?;
            let tokens_txt = std::fs::read_to_string(&tokens_path)
                .with_context(|| format!("read {}", tokens_path.display()))?;
            let (lines, skipped) = hotwords::encode_vocabulary(&tokens_txt, vocabulary);
            if !skipped.is_empty() {
                tracing::warn!(
                    "vocabulary boost: {} phrase(s) not encodable for {} and skipped: {:?}",
                    skipped.len(),
                    model_id,
                    skipped
                );
            }
            lines
        };
        let use_hotwords = !hotword_lines.is_empty();

        match model.kind {
            ModelKind::StreamingAsr => {
                let recognizer = self.recognizer(model_id, use_hotwords)?;
                let stream = if use_hotwords {
                    recognizer.create_stream_with_hotwords(&hotword_lines)
                } else {
                    recognizer.create_stream()
                };
                // Models that punctuate natively bypass the punctuation model
                // and the lowercase/polish pass entirely.
                let punct = if model.native_punctuation {
                    None
                } else {
                    self.punctuation()
                };
                Ok(LocalSession::streaming(
                    recognizer,
                    stream,
                    punct,
                    model.native_punctuation,
                ))
            }
            ModelKind::OfflineAsr => {
                let recognizer = self.offline_recognizer(model_id, use_hotwords)?;
                let vad = self.create_vad(model_id)?;
                let labeler = if live_speaker_labels && self.speaker_id_present() {
                    match self.embedder() {
                        Ok(e) => Some(crate::session::LiveLabeler::new(e)),
                        Err(e) => {
                            tracing::warn!("live speaker labels unavailable: {e}");
                            None
                        }
                    }
                } else {
                    None
                };
                Ok(LocalSession::offline(
                    recognizer,
                    vad,
                    hotword_lines,
                    model.native_punctuation,
                    labeler,
                ))
            }
            ModelKind::Punctuation | ModelKind::SpeakerId | ModelKind::Llm
            | ModelKind::Embedding => {
                unreachable!("filtered to ASR kinds above")
            }
        }
    }

    fn offline_recognizer(&self, model_id: &str, hotwords: bool) -> Result<Arc<OfflineRecognizer>> {
        let key = (model_id.to_string(), hotwords);
        {
            let cache = self
                .offline_recognizers
                .lock()
                .expect("engine mutex poisoned");
            if let Some(r) = cache.get(&key) {
                return Ok(r.clone());
            }
        }

        let model = catalog::find(model_id).expect("checked by caller");
        let path = |role: FileRole| -> Result<String> {
            let p = model
                .role_path(role)
                .ok_or_else(|| anyhow!("model {model_id} missing {role:?}"))?;
            Ok(p.to_string_lossy().to_string())
        };

        let mut config = OfflineRecognizerConfig::default();
        config.model_config.transducer.encoder = Some(path(FileRole::Encoder)?);
        config.model_config.transducer.decoder = Some(path(FileRole::Decoder)?);
        config.model_config.transducer.joiner = Some(path(FileRole::Joiner)?);
        config.model_config.tokens = Some(path(FileRole::Tokens)?);
        config.model_config.model_type = Some("nemo_transducer".to_string());
        config.model_config.num_threads = OFFLINE_NUM_THREADS;
        if hotwords {
            config.decoding_method = Some("modified_beam_search".to_string());
            config.hotwords_score = HOTWORDS_SCORE;
        }

        tracing::info!(model_id, hotwords, "loading offline ASR model");
        let started = std::time::Instant::now();
        let recognizer = OfflineRecognizer::create(&config).ok_or_else(|| {
            anyhow!("failed to load offline ASR model '{model_id}' — files may be corrupt; re-download it from Settings")
        })?;
        tracing::info!(
            model_id,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "offline ASR model loaded"
        );

        let arc = Arc::new(recognizer);
        self.offline_recognizers
            .lock()
            .expect("engine mutex poisoned")
            .insert(key, arc.clone());
        Ok(arc)
    }

    /// A fresh VAD per session (it carries stream state, unlike recognizers).
    fn create_vad(&self, model_id: &str) -> Result<VoiceActivityDetector> {
        let model = catalog::find(model_id).expect("checked by caller");
        let vad_path = model
            .role_path(FileRole::Vad)
            .ok_or_else(|| anyhow!("model {model_id} has no VAD file"))?;

        let mut config = VadModelConfig::default();
        config.silero_vad.model = Some(vad_path.to_string_lossy().to_string());
        config.silero_vad.threshold = VAD_THRESHOLD;
        config.silero_vad.min_silence_duration = VAD_MIN_SILENCE_SECS;
        config.silero_vad.min_speech_duration = VAD_MIN_SPEECH_SECS;
        config.silero_vad.window_size = VAD_WINDOW_SAMPLES;
        config.silero_vad.max_speech_duration = VAD_MAX_SPEECH_SECS;
        config.sample_rate = 16_000;
        config.num_threads = 1;

        VoiceActivityDetector::create(&config, VAD_BUFFER_SECS)
            .ok_or_else(|| anyhow!("failed to load the voice activity model — re-download '{model_id}' from Settings"))
    }

    fn recognizer(&self, model_id: &str, hotwords: bool) -> Result<Arc<OnlineRecognizer>> {
        let key = (model_id.to_string(), hotwords);
        {
            let cache = self.recognizers.lock().expect("engine mutex poisoned");
            if let Some(r) = cache.get(&key) {
                return Ok(r.clone());
            }
        }

        let model = catalog::find(model_id).expect("checked by caller");
        let path = |role: FileRole| -> Result<String> {
            let p = model
                .role_path(role)
                .ok_or_else(|| anyhow!("model {model_id} missing {role:?}"))?;
            Ok(p.to_string_lossy().to_string())
        };

        let mut config = OnlineRecognizerConfig::default();
        config.model_config.transducer.encoder = Some(path(FileRole::Encoder)?);
        config.model_config.transducer.decoder = Some(path(FileRole::Decoder)?);
        config.model_config.transducer.joiner = Some(path(FileRole::Joiner)?);
        config.model_config.tokens = Some(path(FileRole::Tokens)?);
        config.model_config.num_threads = NUM_THREADS;
        config.enable_endpoint = true;
        config.rule1_min_trailing_silence = RULE1_TRAILING_SILENCE_SECS;
        config.rule2_min_trailing_silence = RULE2_TRAILING_SILENCE_SECS;
        config.rule3_min_utterance_length = RULE3_UTTERANCE_LENGTH_SECS;
        if hotwords {
            config.decoding_method = Some("modified_beam_search".to_string());
            config.max_active_paths = 4;
            config.hotwords_score = HOTWORDS_SCORE;
        } else {
            config.decoding_method = Some("greedy_search".to_string());
        }

        tracing::info!(model_id, hotwords, "loading local ASR model");
        let started = std::time::Instant::now();
        let recognizer = OnlineRecognizer::create(&config).ok_or_else(|| {
            anyhow!("failed to load local ASR model '{model_id}' — files may be corrupt; re-download it from Settings")
        })?;
        tracing::info!(
            model_id,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "local ASR model loaded"
        );

        let arc = Arc::new(recognizer);
        self.recognizers
            .lock()
            .expect("engine mutex poisoned")
            .insert(key, arc.clone());
        Ok(arc)
    }

    /// The shared punctuation model, if downloaded (loaded once, kept warm).
    /// Absence is not an error; sessions fall back to naive casing.
    fn punctuation(&self) -> Option<Arc<OnlinePunctuation>> {
        let mut guard = self.punctuation.lock().expect("engine mutex poisoned");
        if let Some(p) = guard.as_ref() {
            return Some(p.clone());
        }
        let model = catalog::find("punct-en")?;
        if !model.present() {
            return None;
        }
        let config = OnlinePunctuationConfig {
            model: OnlinePunctuationModelConfig {
                cnn_bilstm: Some(
                    model
                        .role_path(FileRole::CnnBilstm)?
                        .to_string_lossy()
                        .to_string(),
                ),
                bpe_vocab: Some(
                    model
                        .role_path(FileRole::BpeVocab)?
                        .to_string_lossy()
                        .to_string(),
                ),
                ..Default::default()
            },
        };
        match OnlinePunctuation::create(&config) {
            Some(p) => {
                tracing::info!("punctuation model loaded");
                let arc = Arc::new(p);
                *guard = Some(arc.clone());
                Some(arc)
            }
            None => {
                tracing::warn!("punctuation model present but failed to load; using naive casing");
                None
            }
        }
    }
}
