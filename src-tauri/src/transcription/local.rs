//! Local on-device transcription via the shared `embral-engine` (sherpa-onnx).
//!
//! Thin bridge between the app's async `TranscriptionSession` contract and the
//! engine's synchronous `LocalSession`: audio chunks flow over a std channel
//! into a blocking task that feeds the engine and forwards its events. The
//! recognizer itself lives in the app-wide warm [`embral_engine::Engine`], so
//! only the first session pays the model-load cost; later recordings start
//! instantly.

use anyhow::Result;
use async_trait::async_trait;
use embral_engine::{Engine, SessionEvent};
use embral_types::{ProviderCapabilities, TranscriptionSegment};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;

use super::{SessionStats, TranscriptionEvent, TranscriptionProvider, TranscriptionSession};

enum InferMsg {
    Audio(Vec<f32>),
    /// Force-finalize the in-flight utterance (a starred moment); replies
    /// with the split point on the stream clock.
    Split(tokio::sync::oneshot::Sender<f64>),
    Finish,
}

pub struct LocalProvider {
    engine: Arc<Engine>,
    model_id: String,
    vocabulary: Vec<String>,
    /// Live provisional speaker labels; off when the user disabled
    /// speaker detection.
    live_speaker_labels: bool,
}

impl LocalProvider {
    pub fn new(
        engine: Arc<Engine>,
        model_id: String,
        vocabulary: Vec<String>,
        live_speaker_labels: bool,
    ) -> Self {
        Self {
            engine,
            model_id,
            vocabulary,
            live_speaker_labels,
        }
    }
}

pub struct LocalTranscriptionSession {
    infer_tx: std::sync::mpsc::Sender<InferMsg>,
    accumulated: Arc<Mutex<Vec<TranscriptionSegment>>>,
    inference_task: JoinHandle<anyhow::Result<()>>,
    stats: Arc<SessionStats>,
    span: tracing::Span,
}

fn forward_events(
    events: Vec<SessionEvent>,
    event_tx: &mpsc::UnboundedSender<TranscriptionEvent>,
    accumulated: &Arc<Mutex<Vec<TranscriptionSegment>>>,
    stats: &SessionStats,
) {
    for ev in events {
        match ev {
            SessionEvent::Interim {
                text,
                tentative,
                start,
                end,
            } => {
                let _ = event_tx.send(TranscriptionEvent::Interim {
                    segment: TranscriptionSegment {
                        speaker: None,
                        speaker_id: None,
                        text,
                        start,
                        end,
                    },
                    // Diff-derived (words agreeing with the previous decode
                    // are stable); already carries the leading-space
                    // word-boundary convention.
                    tentative,
                });
            }
            SessionEvent::Final {
                text,
                start,
                end,
                speaker,
            } => {
                let segment = TranscriptionSegment {
                    // Provisional live label ("Speaker N") when live labeling
                    // is active; the post-meeting pipeline overwrites these.
                    speaker,
                    speaker_id: None,
                    text,
                    start,
                    end,
                };
                stats.on_segment();
                accumulated.blocking_lock().push(segment.clone());
                let _ = event_tx.send(TranscriptionEvent::Segment(segment));
            }
        }
    }
}

#[async_trait]
impl TranscriptionProvider for LocalProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            // Live labels here are a provisional preview; the post-meeting
            // pipeline re-diarizes the whole recording and overwrites them.
            labels_authoritative: false,
            max_session_minutes: 600,
        }
    }

    async fn start_session(
        &self,
        event_tx: mpsc::UnboundedSender<TranscriptionEvent>,
    ) -> Result<Box<dyn TranscriptionSession>> {
        let engine = self.engine.clone();
        let model_id = self.model_id.clone();
        let vocabulary = self.vocabulary.clone();
        let live_speaker_labels = self.live_speaker_labels;

        let (infer_tx, infer_rx) = std::sync::mpsc::channel::<InferMsg>();
        let accumulated = Arc::new(Mutex::new(Vec::<TranscriptionSegment>::new()));
        let acc_clone = accumulated.clone();

        let span = tracing::info_span!("session", provider = "local");
        let span_task = span.clone();
        let stats = SessionStats::new();
        let stats_task = stats.clone();

        // The blocking task owns the engine session for its whole life: model
        // (warm-cache) load, per-chunk decode, and the final flush.
        let inference_task: JoinHandle<anyhow::Result<()>> =
            tokio::task::spawn_blocking(move || {
                let _enter = span_task.enter();
                let stats = stats_task;
                tracing::info!(model = %model_id, vocabulary = vocabulary.len(), "connect");
                let mut session =
                    engine.create_session(&model_id, &vocabulary, live_speaker_labels)?;
                tracing::info!("ready");

                loop {
                    match infer_rx.recv() {
                        Ok(InferMsg::Audio(chunk)) => {
                            stats.on_audio(chunk.len());
                            let events = session.accept(&chunk);
                            forward_events(events, &event_tx, &acc_clone, &stats);
                        }
                        Ok(InferMsg::Split(reply)) => {
                            let events = session.split_now();
                            forward_events(events, &event_tx, &acc_clone, &stats);
                            // All audio queued before the split has been
                            // processed, so the stream clock IS the
                            // boundary between spoken-before and -after.
                            let _ = reply.send(session.stream_secs());
                        }
                        Ok(InferMsg::Finish) | Err(_) => {
                            let events = session.finish();
                            forward_events(events, &event_tx, &acc_clone, &stats);
                            let _ = event_tx.send(TranscriptionEvent::Done);
                            stats.finish("clean");
                            return Ok(());
                        }
                    }
                }
            });

        Ok(Box::new(LocalTranscriptionSession {
            infer_tx,
            accumulated,
            inference_task,
            stats,
            span,
        }))
    }
}

#[async_trait]
impl TranscriptionSession for LocalTranscriptionSession {
    async fn send_audio(&self, pcm_f32: &[f32]) -> Result<()> {
        self.infer_tx
            .send(InferMsg::Audio(pcm_f32.to_vec()))
            .map_err(|_| anyhow::anyhow!("local inference task has exited"))
    }

    fn split_utterance(&self) -> Option<tokio::sync::oneshot::Receiver<f64>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.infer_tx.send(InferMsg::Split(tx)).ok()?;
        Some(rx)
    }

    async fn finish(self: Box<Self>) -> Result<Vec<TranscriptionSegment>> {
        let LocalTranscriptionSession {
            infer_tx,
            accumulated,
            inference_task,
            stats,
            span,
        } = *self;
        let _enter = span.enter();

        let _ = infer_tx.send(InferMsg::Finish);
        drop(infer_tx);

        match inference_task.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                stats.finish("error");
                return Err(e);
            }
            Err(e) => {
                stats.finish("error");
                return Err(anyhow::anyhow!("local inference task panicked: {e}"));
            }
        }

        let segments = accumulated.lock().await.clone();
        Ok(segments)
    }
}
