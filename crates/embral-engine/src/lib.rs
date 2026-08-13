//! Local speech engine for Embral, built on sherpa-onnx.
//!
//! - [`catalog`]: model registry, presence probing, managed downloads.
//! - [`hotwords`]: vocabulary boost (phrases → BPE hotword lines).
//! - [`Engine`]: warm recognizer cache; hands out sessions.
//! - [`LocalSession`] / [`SessionEvent`]: one live transcription session.
//! - [`speakers`]: pure diarization math over [`Engine::diarize`] /
//!   [`Engine::embed`] outputs (live clustering, segment labeling).
//!
//! The crate is Tauri-free: the app adapts `LocalSession` to its own
//! `TranscriptionSession` trait and forwards progress callbacks to events.

pub mod catalog;
pub mod decode;
mod engine;
pub mod hotwords;
mod session;
pub mod speakers;

pub use catalog::{DownloadProgress, ModelKind, ModelStatus};
pub use engine::{DiarizedSpan, Engine, SPEAKER_ID_MODEL};
pub use session::{LocalSession, SessionEvent};

/// End-to-end smoke test: set `EMBRAL_TEST_WAV` to a 16 kHz mono WAV (and
/// optionally `EMBRAL_TEST_MODEL` to any downloaded catalog model id), then
/// `cargo test -p embral-engine -- --ignored`.
#[cfg(test)]
mod integration {
    use super::*;

    #[test]
    #[ignore = "requires downloaded models and EMBRAL_TEST_WAV"]
    fn transcribes_a_wav_end_to_end() {
        let wav_path = std::env::var("EMBRAL_TEST_WAV").expect("set EMBRAL_TEST_WAV");
        let mut reader = hound::WavReader::open(&wav_path).expect("open wav");
        let spec = reader.spec();
        assert_eq!(spec.sample_rate, 16_000, "test wav must be 16 kHz");
        let samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => reader.samples::<f32>().map(|s| s.unwrap()).collect(),
            hound::SampleFormat::Int => reader
                .samples::<i16>()
                .map(|s| s.unwrap() as f32 / i16::MAX as f32)
                .collect(),
        };

        let model =
            std::env::var("EMBRAL_TEST_MODEL").unwrap_or_else(|_| "zipformer-en".to_string());
        let engine = Engine::new();
        let mut session = engine
            .create_session(
                &model,
                &["embral".to_string()], // exercise the hotwords path too
                false,
            )
            .expect("create session");

        let mut finals = Vec::new();
        for chunk in samples.chunks(1600) {
            for ev in session.accept(chunk) {
                if let SessionEvent::Final { text, .. } = ev {
                    finals.push(text);
                }
            }
        }
        for ev in session.finish() {
            if let SessionEvent::Final { text, .. } = ev {
                finals.push(text);
            }
        }
        let joined = finals.join(" ");
        eprintln!("transcript: {joined}");
        assert!(!joined.trim().is_empty(), "expected non-empty transcript");
    }

    /// Import-equivalent path: decode a bundled demo MP3 and transcribe the
    /// first two minutes with the offline model. Requires `parakeet-tdt-en`
    /// downloaded.
    #[test]
    #[ignore = "requires the parakeet-tdt-en model and decodes a demo mp3"]
    fn imports_demo_mp3_offline() {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/prepop/embral-demo/audio");
        let mp3 = std::fs::read_dir(&fixture)
            .expect("fixture dir")
            .find_map(|e| e.ok().map(|e| e.path()))
            .expect("a demo mp3");

        let mut samples = decode::decode_to_pcm16k(&mp3).expect("decode");
        samples.truncate(120 * 16_000); // first two minutes is plenty

        let engine = Engine::new();
        let mut session = engine
            .create_session("parakeet-tdt-en", &[], false)
            .expect("create offline session");

        let started = std::time::Instant::now();
        let mut finals = Vec::new();
        let mut interims = 0usize;
        for chunk in samples.chunks(4800) {
            for ev in session.accept(chunk) {
                match ev {
                    SessionEvent::Final { text, .. } => finals.push(text),
                    SessionEvent::Interim { .. } => interims += 1,
                }
            }
        }
        for ev in session.finish() {
            if let SessionEvent::Final { text, .. } = ev {
                finals.push(text);
            }
        }
        eprintln!(
            "decoded 120s in {:.1}s ({} finals, {} interims)",
            started.elapsed().as_secs_f32(),
            finals.len(),
            interims
        );
        eprintln!(
            "first final: {}",
            finals.first().cloned().unwrap_or_default()
        );
        assert!(finals.len() >= 3, "expected several utterances");
        assert!(interims >= 1, "expected live previews");
        // Native punctuation: some sentence-final punctuation should appear.
        assert!(finals.iter().any(|f| f.contains('.') || f.contains('?')));
    }

    /// Diarization e2e: set `EMBRAL_TEST_DIARIZE_WAV` to a 16 kHz mono WAV
    /// with two alternating speakers (e.g. two Windows TTS voices) and
    /// download the `speaker-id` model, then
    /// `cargo test -p embral-engine diarizes -- --ignored --nocapture`.
    #[test]
    #[ignore = "requires the speaker-id model and EMBRAL_TEST_DIARIZE_WAV"]
    fn diarizes_two_voices_end_to_end() {
        let wav_path =
            std::env::var("EMBRAL_TEST_DIARIZE_WAV").expect("set EMBRAL_TEST_DIARIZE_WAV");
        let mut reader = hound::WavReader::open(&wav_path).expect("open wav");
        let spec = reader.spec();
        assert_eq!(spec.sample_rate, 16_000, "test wav must be 16 kHz");
        let samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => reader.samples::<f32>().map(|s| s.unwrap()).collect(),
            hound::SampleFormat::Int => reader
                .samples::<i16>()
                .map(|s| s.unwrap() as f32 / i16::MAX as f32)
                .collect(),
        };

        let engine = Engine::new();
        let spans = engine.diarize(&samples, 0.5).expect("diarize");
        eprintln!("diarized spans: {spans:?}");
        let clusters: std::collections::BTreeSet<usize> =
            spans.iter().map(|s| s.cluster).collect();
        assert_eq!(clusters.len(), 2, "expected exactly two speakers");

        // Per-cluster audio → embeddings: the two speakers must be mutually
        // distant while two halves of the same speaker stay close.
        let cluster_audio = |cluster: usize| -> Vec<f32> {
            let mut out = Vec::new();
            for s in spans.iter().filter(|s| s.cluster == cluster) {
                let a = (s.start * 16_000.0) as usize;
                let b = ((s.end * 16_000.0) as usize).min(samples.len());
                out.extend_from_slice(&samples[a.min(b)..b]);
            }
            out
        };
        let (a, b) = (cluster_audio(0), cluster_audio(1));
        let ea = engine.embed(&a).expect("embed cluster 0");
        let eb = engine.embed(&b).expect("embed cluster 1");
        let cross = speakers::cosine(&ea, &eb);
        let ea1 = engine.embed(&a[..a.len() / 2]).expect("embed half");
        let ea2 = engine.embed(&a[a.len() / 2..]).expect("embed half");
        let same = speakers::cosine(&ea1, &ea2);
        eprintln!("cross-speaker cosine {cross:.3}, same-speaker cosine {same:.3}");
        // The live clusterer joins at this threshold: the same voice must
        // score above it (rejoins its cluster) and different voices below
        // it (split apart).
        assert!(
            cross < speakers::ONLINE_CLUSTER_THRESHOLD,
            "different voices should score below the clustering threshold"
        );
        assert!(
            same > speakers::ONLINE_CLUSTER_THRESHOLD,
            "the same voice should score above the clustering threshold"
        );
    }
}
