//! Recording lifecycle commands: start/pause/resume/stop, in-recording
//! stars and live speaker renames, and the detection prompt responses.

use embral_types::{AppConfig, AppError};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Mutex;

use crate::audio::recorder::Recorder;
use crate::autodetect::silence::LivenessTracker;
use crate::transcription::stream::{
    SessionSlot, SharedSlot, StreamLane, SESSION_FINISH_TIMEOUT,
};
use crate::transcription::{self, stream, TranscriptionEvent};
use crate::{epoch_ms, AppState};

use super::finalize::{finalize_meeting, AudioSource};
use super::support::*;

/// Whether the configured local model is on disk: the gate for falling
/// back from cloud transcription mid-recording.
#[cfg(feature = "cloud")]
fn local_model_present(config: &AppConfig) -> bool {
    embral_engine::catalog::find(&config.meeting_asr_model()).is_some_and(|m| m.present())
}

#[cfg(feature = "cloud")]
use crate::transcription::TranscriptionProvider;

/// Distinct speakers past which live diarization is treated as having
/// failed rather than having found a crowd. Meetings with more real voices
/// than this exist, but a clusterer that keeps opening speakers is far
/// commoner, and confidently wrong labels cost the reader more than no
/// labels do ([speakers.md]).
const MAX_LIVE_SPEAKERS: usize = 6;

/// Whether the live labels have stopped being believable.
fn diarization_has_run_away(distinct_speakers: usize) -> bool {
    distinct_speakers > MAX_LIVE_SPEAKERS
}

/// Drop every speaker label from the accumulated segments. Turning
/// diarization off part-way through must not leave a half-labelled
/// transcript; that reads as "the app lost track", which is worse than
/// a transcript that never claimed to know.
async fn strip_speakers(segments: &crate::SharedSegments) {
    for seg in segments.lock().await.iter_mut() {
        seg.speaker = None;
        seg.speaker_id = None;
    }
}

/// The label layer for one finalized segment ([speakers.md] §Live labels):
/// when labeling is off, strip; otherwise count the provider's own label
/// for the runaway guard (a rename changes the name, not the cluster),
/// and then apply any user rename. Returns true when this very segment
/// pushed the distinct-label count past the ceiling; the caller stands
/// labeling down for the whole recording.
fn label_segment(
    seg: &mut embral_types::TranscriptionSegment,
    labeling_on: bool,
    seen: &std::sync::Mutex<std::collections::HashSet<String>>,
    renames: &std::collections::HashMap<String, String>,
) -> bool {
    if !labeling_on {
        seg.speaker = None;
        seg.speaker_id = None;
        return false;
    }
    let Some(label) = seg.speaker.clone() else {
        return false;
    };
    let distinct = {
        let mut seen = seen.lock().expect("live speaker labels poisoned");
        seen.insert(label.clone());
        seen.len()
    };
    if diarization_has_run_away(distinct) {
        seg.speaker = None;
        seg.speaker_id = None;
        return true;
    }
    if let Some(new_name) = renames.get(&label) {
        seg.speaker = Some(new_name.clone());
    }
    false
}

/// The local provider for this recording's settings: the standing local
/// choice, the signed-out swap, and the mid-recording fallback all build
/// exactly this.
fn local_provider(
    config: &AppConfig,
    engine: Arc<embral_engine::Engine>,
) -> transcription::local::LocalProvider {
    transcription::local::LocalProvider::new(
        engine,
        config.meeting_asr_model(),
        config.vocabulary.clone(),
        config.diarization_enabled,
    )
}

/// Unwind a start that already built its lane but cannot go on: no open
/// still in flight may install (generation), the forwarder ends (its
/// channel closes when the lane's sender drops), and any session already
/// installed is retired.
async fn abandon_started_lane(lane: &Arc<StreamLane>, slot: &SharedSlot) {
    lane.bump_generation();
    lane.take_event_tx();
    let mut guard = slot.lock().await;
    if let SessionSlot::Streaming(session) = std::mem::replace(&mut *guard, SessionSlot::Off) {
        stream::finish_detached(session, "start aborted");
    }
}

/// How long a session open may take before it is treated as failed. The
/// relay handshake normally answers within a couple of seconds; a hung
/// connect must not leave a recording silently buffering forever with no
/// banner and no fallback.
const OPEN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// Open a session off the recording's critical path and put it behind the
/// bridge when it arrives; audio waits in the slot's buffer meanwhile. A
/// connect failure, or a hang past [`OPEN_TIMEOUT`], reports through
/// the recording's event channel like any mid-recording death, but only
/// while this open is still the current generation's: nobody wants
/// banners from a stream the recording already moved past.
fn spawn_stream_open(
    app: AppHandle,
    provider: Arc<dyn transcription::TranscriptionProvider>,
    is_cloud: bool,
    generation: u64,
    lane: Arc<StreamLane>,
    slot: SharedSlot,
) {
    tauri::async_runtime::spawn(async move {
        let (stream_tx, stream_rx) = tokio::sync::mpsc::unbounded_channel::<TranscriptionEvent>();
        let session = match tokio::time::timeout(OPEN_TIMEOUT, provider.start_session(stream_tx))
            .await
        {
            Ok(Ok(session)) => session,
            Ok(Err(e)) => {
                report_stream_failure(&lane, generation, e.to_string());
                return;
            }
            Err(_) => {
                report_stream_failure(
                    &lane,
                    generation,
                    format!(
                        "the connection attempt gave up after {} seconds",
                        OPEN_TIMEOUT.as_secs()
                    ),
                );
                return;
            }
        };
        let state = app.state::<AppState>();
        stream::install_stream(
            &lane,
            &slot,
            &state.live_speaker_labels,
            &state.last_liveness_at,
            session,
            stream_rx,
            is_cloud,
            generation,
        )
        .await;
    });
}

/// A stream failure (an open that never connected, or a live session the
/// bridge retired for stalling) reports as `TranscriptionEvent::Failed`
/// so the forwarder's one failure path handles it, unless the recording
/// has already moved past the stream, in which case nobody is owed
/// anything.
fn report_stream_failure(lane: &StreamLane, generation: u64, message: String) {
    if lane.current_generation() != generation {
        tracing::info!("stream failed after the recording moved on: {message}");
        return;
    }
    let Some(tx) = lane.clone_event_tx() else {
        tracing::info!("stream failed after the recording ended: {message}");
        return;
    };
    let _ = tx.send(TranscriptionEvent::Failed { message });
}

/// One choke point for every start path (button, hotkey, detection accept,
/// auto-start): a refused start counts once, whatever refused it.
#[tauri::command]
pub async fn start_recording(app: AppHandle, state: State<'_, AppState>) -> Result<(), AppError> {
    let result = start_recording_inner(app, &state).await;
    if result.is_err() {
        crate::telemetry::track(
            &state,
            "error",
            serde_json::json!({ "category": "recording_start_failed" }),
        );
    }
    result
}

async fn start_recording_inner(app: AppHandle, state: &State<'_, AppState>) -> Result<(), AppError> {
    let config = state.config.lock().await.clone();

    // Nothing downstream is idempotent: a second start overwrites the
    // recorder, the session, and `in_progress.txt` while the first
    // recording's capture threads and transcription session keep running.
    if state.recorder.lock().await.is_some() {
        tracing::warn!("start requested while already recording — ignoring");
        return Err(AppError::AlreadyRecording);
    }
    // Who transcribes this meeting: the standing choice, bent by the power
    // policy. Read once, here: the provider is fixed for the meeting, and the
    // record gate has to ask about the provider the meeting really uses.
    let power = crate::platform::power_source();
    let provider_choice = crate::config::provider_for_power(&config, power);
    if provider_choice != config.transcription_provider {
        tracing::info!(
            "power source is {power:?} — this meeting transcribes with {provider_choice:?}"
        );
    }
    if let Some(gap) = crate::config::missing_prerequisite(&config, &provider_choice) {
        tracing::warn!("refusing to record — {gap}");
        return Err(AppError::NotConfigured);
    }
    if state.dictating.load(std::sync::atomic::Ordering::Acquire) {
        return Err(AppError::BusyDictating);
    }

    let base = crate::storage::storage_base(&config.storage_dir);
    crate::storage::init_storage_dirs(&base).map_err(|e| e.to_string())?;

    let meeting_id = crate::storage::generate_meeting_id();
    let wav_path = base.join("audio").join(format!("{}.wav", meeting_id));

    // Reset the backend-side segment accumulator. The event forwarder below
    // populates it; stop_recording reads from it as source of truth.
    state.current_segments.lock().await.clear();
    // This recording's own diarization standing, from the setting. It can
    // only go off from here, by the toggle or the runaway guard.
    state.live_diarization.store(
        config.diarization_enabled,
        std::sync::atomic::Ordering::Release,
    );
    state
        .live_speaker_labels
        .lock()
        .expect("live speaker labels poisoned")
        .clear();
    state.live_label_renames.lock().await.clear();
    state.stars.lock().await.clear();
    state.star_anchors.lock().await.clear();
    let segments_acc = state.current_segments.clone();

    // Build transcription provider and open session. Signed out with
    // cloud selected, the relay handshake cannot succeed, and waiting out
    // its timeout would cost the first seconds of every meeting, so the
    // configured fallback applies immediately instead.
    #[cfg(feature = "cloud")]
    let signed_out_cloud = provider_choice == embral_types::TranscriptionProvider::Cloud
        && config.cloud_session_token.is_empty();
    #[cfg(not(feature = "cloud"))]
    let signed_out_cloud = false;

    let provider = if signed_out_cloud {
        tracing::info!("cloud selected but signed out — transcribing on this device");
        Arc::new(local_provider(&config, state.engine.clone()))
            as Arc<dyn transcription::TranscriptionProvider>
    } else {
        transcription::build_provider(&provider_choice, &config, state.engine.clone())
    };
    let capabilities = provider.capabilities();
    // Snapshot for stop_recording: whether this session's labels are final
    // (cloud live diarization) or a provisional preview (local live labels).
    state
        .labels_authoritative
        .store(capabilities.labels_authoritative, std::sync::atomic::Ordering::Release);

    // The recording's event channel, stream lane, and session slot. Every
    // session this recording runs (the first, a mid-recording fallback, a
    // post-pause reopen) reaches the forwarder below through a per-stream
    // pump on this one channel (`transcription::stream`).
    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel::<TranscriptionEvent>();
    let lane = Arc::new(StreamLane::new(event_tx));
    let slot: SharedSlot = Arc::new(Mutex::new(SessionSlot::Buffering(Default::default())));
    *state.lane.lock().expect("lane poisoned") = lane.clone();

    // Which kind of stream the open (below, off the critical path) will
    // produce. The record gate and the signed-out swap already ran, so
    // this is settled before capture starts.
    #[cfg(feature = "cloud")]
    let provider_is_cloud =
        provider_choice == embral_types::TranscriptionProvider::Cloud && !signed_out_cloud;
    #[cfg(not(feature = "cloud"))]
    let provider_is_cloud = false;
    // The lane's kind is settled now, not at install: a pause landing
    // while the first open is still in flight must already know it is
    // pausing a cloud lane, or the retired open would never be reopened.
    lane.stream_is_cloud
        .store(provider_is_cloud, std::sync::atomic::Ordering::Release);
    // What a post-pause reopen asks the vendor for: the recording's
    // start-time choices; only the token is read fresh at reopen.
    #[cfg(feature = "cloud")]
    if provider_is_cloud {
        *lane
            .cloud_reopen
            .lock()
            .expect("cloud reopen request poisoned") = Some(stream::CloudStreamRequest {
            language_hints: config.language_hints(),
            diarization: config.diarization_enabled,
        });
    }

    // Audio bridge: drain audio chunks into whatever holds the slot: a
    // live session, the buffer in front of a pending one, or nothing.
    let (audio_tx, mut audio_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<f32>>();
    let slot_for_bridge = slot.clone();
    let lane_for_bridge = lane.clone();

    tokio::spawn(async move {
        tracing::info!("Audio bridge task started");
        let mut chunk_n: usize = 0;
        let mut total_samples: usize = 0;
        // One line per stretch without a live session, not ten a second.
        let mut idle_logged = false;
        while let Some(chunk) = audio_rx.recv().await {
            if chunk_n == 0 {
                tracing::info!(
                    "Audio bridge: first chunk received ({} samples)",
                    chunk.len()
                );
            }
            total_samples += chunk.len();
            match stream::deliver_chunk(&lane_for_bridge, &slot_for_bridge, chunk).await {
                stream::Delivered::Sent => {
                    if chunk_n == 0 {
                        tracing::info!("Audio bridge: first send_audio call succeeded");
                    }
                    idle_logged = false;
                }
                stream::Delivered::Buffered => {
                    if !idle_logged {
                        idle_logged = true;
                        tracing::info!(
                            "Audio bridge: buffering from chunk {} while a session opens",
                            chunk_n
                        );
                    }
                }
                stream::Delivered::Off => {
                    // Expected steady state when transcription is off: the
                    // recorder keeps writing audio, nothing consumes the
                    // stream.
                    if !idle_logged {
                        idle_logged = true;
                        tracing::info!(
                            "Audio bridge: no transcription session from chunk {} — audio continues to disk only",
                            chunk_n
                        );
                    }
                }
                stream::Delivered::Stalled { session, why } => {
                    // The session stopped taking audio. Retire it off the
                    // slot lock and report through the one failure path;
                    // the forwarder falls back or turns the slot off. The
                    // slot is buffering again, so this cannot repeat for
                    // the same corpse. Stop's suppression is the taken
                    // event channel; a stall concluding just as a pause
                    // lands reports too, which only hastens the fallback a
                    // genuinely stalled lane was headed for anyway.
                    idle_logged = false;
                    tracing::error!(
                        "Audio bridge: the live session stopped taking audio at chunk {chunk_n}: {why}"
                    );
                    stream::finish_detached(session, "stopped taking audio");
                    report_stream_failure(
                        &lane_for_bridge,
                        lane_for_bridge.current_generation(),
                        why,
                    );
                }
            }
            if (chunk_n + 1) % 50 == 0 {
                tracing::debug!(
                    "Audio bridge stats: {} chunks forwarded, {} total samples",
                    chunk_n + 1,
                    total_samples
                );
            }
            chunk_n += 1;
        }
        tracing::info!(
            "Audio bridge task exiting: {} chunks forwarded, {} total samples",
            chunk_n,
            total_samples
        );
    });

    // Event forwarder: emit transcription-{interim,segment} Tauri events AND
    // accumulate Segments into the AppState-owned Vec (source of truth).
    // It ends when the recording's channel closes: stop takes the lane's
    // sender, and each stream's pump drops its clone as its stream drains.
    let app_clone = app.clone();
    let segments_acc_for_forwarder = segments_acc.clone();
    let recovery_base = base.clone();
    // Pinned here: a retired stream's tail segment can arrive while a
    // successor recording is live, and it must go into this meeting's
    // scratch, not whichever one is current by then.
    let recovery_meeting_id = meeting_id.clone();
    let lane_for_forwarder = lane.clone();
    #[cfg(feature = "cloud")]
    let slot_for_forwarder = slot.clone();
    let forwarder = tokio::spawn(async move {
        #[cfg(feature = "cloud")]
        let mut fallen_back = false;
        // The check-in's word evidence: words count as they arrive on
        // screen, not only when their utterance closes ([detection.md]
        // §Auto-stop on silence).
        let mut liveness = LivenessTracker::default();
        while let Some(event) = event_rx.recv().await {
            match event {
                TranscriptionEvent::Interim { segment, tentative } => {
                    // Flat wire shape so the frontend interim payload reads as a
                    // TranscriptionSegment with one extra optional field.
                    #[derive(serde::Serialize)]
                    struct InterimPayload<'a> {
                        speaker: Option<&'a str>,
                        text: &'a str,
                        start: f64,
                        end: f64,
                        tentative_text: Option<&'a str>,
                    }
                    let payload = InterimPayload {
                        speaker: segment.speaker.as_deref(),
                        text: &segment.text,
                        start: segment.start,
                        end: segment.end,
                        tentative_text: tentative.as_deref(),
                    };
                    if liveness.observe_interim(&segment.text) {
                        app_clone
                            .state::<AppState>()
                            .last_liveness_at
                            .store(epoch_ms(), std::sync::atomic::Ordering::Release);
                    }
                    let _ = app_clone.emit("transcription-interim", &payload);
                }
                TranscriptionEvent::Segment(mut seg) => {
                    let state = app_clone.state::<AppState>();
                    // Track the highest vendor-numbered label even while the
                    // label layer strips: a stream opened mid-recording
                    // numbers its speakers after every label already
                    // produced, seen or not ([speakers.md]).
                    if let Some(n) = seg
                        .speaker
                        .as_deref()
                        .and_then(embral_types::generic_speaker_number)
                    {
                        lane_for_forwarder
                            .max_speaker_number
                            .fetch_max(n, std::sync::atomic::Ordering::Relaxed);
                    }
                    // The live label layer. Off (by the header toggle, or
                    // because the guard tripped) means no label reaches
                    // the transcript at all, for local and cloud alike
                    // ([speakers.md]).
                    let tripped = {
                        use std::sync::atomic::Ordering;
                        let labeling_on = state.live_diarization.load(Ordering::Acquire);
                        let renames = state.live_label_renames.lock().await;
                        label_segment(&mut seg, labeling_on, &state.live_speaker_labels, &renames)
                    };
                    if tripped {
                        // Not a crowd, but a clusterer inventing people. Turn
                        // labeling off exactly as the button does, including for
                        // what is already on screen.
                        use std::sync::atomic::Ordering;
                        state.live_diarization.store(false, Ordering::Release);
                        strip_speakers(&segments_acc_for_forwarder).await;
                        let distinct = state
                            .live_speaker_labels
                            .lock()
                            .expect("live speaker labels poisoned")
                            .len();
                        tracing::info!(
                            distinct,
                            "too many speakers — diarization off for this recording"
                        );
                        let _ = app_clone.emit(
                            "diarization-disabled",
                            serde_json::json!({ "speakers": distinct }),
                        );
                    }
                    segments_acc_for_forwarder.lock().await.push(seg.clone());
                    // Straight to the recovery scratch too: until finalize
                    // runs, this Vec is the only copy of the meeting.
                    crate::recovery::append_segment(&recovery_base, &recovery_meeting_id, &seg);
                    // The check-in's clock: final words landed, and the
                    // next utterance's committed text starts from nothing.
                    if liveness.observe_segment() {
                        app_clone
                            .state::<AppState>()
                            .last_liveness_at
                            .store(epoch_ms(), std::sync::atomic::Ordering::Release);
                    }
                    let _ = app_clone.emit("transcription-segment", &seg);
                }
                TranscriptionEvent::Failed { message } => {
                    // Mid-recording session death. Cloud builds swap in a
                    // local session behind the same slot; the offline core
                    // just ends the stream (local sessions don't emit this
                    // today).
                    #[cfg(feature = "cloud")]
                    {
                        let state = app_clone.state::<AppState>();
                        if fallen_back {
                            // The replacement died too. Transcription is
                            // over for this recording; going quiet without
                            // saying so would read as the app breaking.
                            tracing::error!("transcription failed after fallback: {message}");
                            let _ = app_clone.emit(
                                "transcription-failed",
                                &AppError::Internal { detail: message.clone() },
                            );
                            crate::telemetry::track(
                                &state,
                                "error",
                                serde_json::json!({ "category": "transcription_failed" }),
                            );
                            break;
                        }
                        fallen_back = true;
                        // Whatever happens next, this lane is done with
                        // cloud: a pause must not reopen a stream that just
                        // proved itself unreachable (mid-meeting reconnect
                        // is deliberately not a thing, [transcription.md]).
                        lane_for_forwarder
                            .stream_is_cloud
                            .store(false, std::sync::atomic::Ordering::Release);
                        // The recording moves past the dead stream before
                        // anything else: one death can reach this arm twice
                        // (the bridge retires a stalled session while its
                        // receive task notices the dropped socket), and a
                        // second `Failed` from the same corpse would read
                        // as the replacement dying. Stale, the dead session's
                        // pump filters it; its tail segments still arrive.
                        lane_for_forwarder.bump_generation();
                        // Retire the dead session if it still holds the
                        // slot (a failed open never installed one) and
                        // buffer audio while the lane decides what's next.
                        {
                            let mut guard = slot_for_forwarder.lock().await;
                            if matches!(&*guard, SessionSlot::Streaming(_)) {
                                let SessionSlot::Streaming(dead) = std::mem::replace(
                                    &mut *guard,
                                    SessionSlot::Buffering(Default::default()),
                                ) else {
                                    unreachable!("checked above");
                                };
                                stream::finish_detached(dead, "failed");
                            }
                        }
                        let config = state.config.lock().await.clone();
                        match crate::config::on_cloud_failure(
                            config.cloud_out_of_hours,
                            local_model_present(&config),
                        ) {
                            crate::config::CloudFailureAction::DisableTranscription => {
                                // The recording and the notes go on; the
                                // transcript ends here, as configured.
                                tracing::warn!(
                                    "cloud transcription ended ({message}); recording continues without a transcript"
                                );
                                *slot_for_forwarder.lock().await = SessionSlot::Off;
                                let _ = app_clone.emit(
                                    "transcription-disabled",
                                    &AppError::Internal { detail: message.clone() },
                                );
                                break;
                            }
                            crate::config::CloudFailureAction::Fail => {
                                tracing::error!(
                                    "cloud transcription failed with no local model to fall back to: {message}"
                                );
                                *slot_for_forwarder.lock().await = SessionSlot::Off;
                                let _ = app_clone.emit(
                                    "transcription-failed",
                                    &AppError::Internal { detail: message.clone() },
                                );
                                crate::telemetry::track(
                                    &app_clone.state::<AppState>(),
                                    "error",
                                    serde_json::json!({ "category": "transcription_failed" }),
                                );
                                break;
                            }
                            crate::config::CloudFailureAction::SwitchToLocal => {}
                        }
                        // Local live labels are provisional again.
                        state
                            .labels_authoritative
                            .store(false, std::sync::atomic::Ordering::Release);
                        let local = local_provider(&config, state.engine.clone());
                        let (local_tx, local_rx) =
                            tokio::sync::mpsc::unbounded_channel::<TranscriptionEvent>();
                        match local.start_session(local_tx).await {
                            Ok(new_session) => {
                                let installed = stream::install_stream(
                                    &lane_for_forwarder,
                                    &slot_for_forwarder,
                                    &state.live_speaker_labels,
                                    &state.last_liveness_at,
                                    new_session,
                                    local_rx,
                                    false,
                                    lane_for_forwarder.current_generation(),
                                )
                                .await;
                                if installed {
                                    tracing::warn!(
                                        "cloud transcription failed; switched to local: {message}"
                                    );
                                    let _ = app_clone.emit(
                                        "transcription-fallback",
                                        &AppError::Internal { detail: message.clone() },
                                    );
                                } else {
                                    // The recording moved on mid-swap (a
                                    // pause or stop); nothing to announce.
                                    tracing::info!(
                                        "local fallback arrived after the recording moved on"
                                    );
                                }
                            }
                            Err(e) => {
                                tracing::error!("local fallback failed to start: {e}");
                                *slot_for_forwarder.lock().await = SessionSlot::Off;
                                let _ = app_clone.emit(
                                    "transcription-failed",
                                    &AppError::Internal { detail: message.clone() },
                                );
                                crate::telemetry::track(
                                    &app_clone.state::<AppState>(),
                                    "error",
                                    serde_json::json!({ "category": "transcription_failed" }),
                                );
                                break;
                            }
                        }
                    }
                    #[cfg(not(feature = "cloud"))]
                    {
                        tracing::error!("transcription session failed: {message}");
                        let _ = app_clone.emit(
                            "transcription-failed",
                            &AppError::Internal { detail: message.clone() },
                        );
                        crate::telemetry::track(
                            &app_clone.state::<AppState>(),
                            "error",
                            serde_json::json!({ "category": "transcription_failed" }),
                        );
                        break;
                    }
                }
                TranscriptionEvent::Done => break,
            }
        }
    });
    *state
        .forwarder_task
        .lock()
        .expect("forwarder handle poisoned") = Some(forwarder);

    // Start recorder (this also writes WAV to disk)
    let mic = Some(config.mic_device.as_str()).filter(|s| !s.trim().is_empty());
    let output = Some(config.output_device.as_str()).filter(|s| !s.trim().is_empty());
    // ~10 Hz pre-mix band spectra for the recording view's live meter.
    // Paused callbacks discard samples before the tap, so pausing silences
    // it.
    let app_level = app.clone();
    let level_cb: Box<dyn Fn(&[f32], &[f32]) + Send> = Box::new(move |mic, system| {
        let _ = app_level.emit(
            "audio-level",
            serde_json::json!({ "mic": mic, "system": system }),
        );
    });
    // Fresh recording, default selection: everything the machine plays.
    // The source picker narrows it live, and the lane re-reads this on
    // every supervision tick.
    *state
        .system_audio_wanted
        .lock()
        .expect("system audio selection poisoned") =
        crate::platform::types::SystemAudioWanted::Everything;
    state
        .extra_mics
        .lock()
        .expect("extra mics poisoned")
        .clear();
    let wanted_handle = app.clone();
    let wanted: Box<dyn Fn() -> crate::platform::types::SystemAudioWanted + Send> =
        Box::new(move || {
            wanted_handle
                .state::<AppState>()
                .system_audio_wanted
                .lock()
                .expect("system audio selection poisoned")
                .clone()
        });
    let mics_handle = app.clone();
    let extra_mics: Box<dyn Fn() -> Vec<String> + Send> = Box::new(move || {
        mics_handle
            .state::<AppState>()
            .extra_mics
            .lock()
            .expect("extra mics poisoned")
            .clone()
    });
    let recorder = match Recorder::start(
        wav_path,
        Some(audio_tx),
        mic,
        output,
        Some(level_cb),
        wanted,
        extra_mics,
    ) {
        Ok(recorder) => recorder,
        Err(e) => {
            abandon_started_lane(&lane, &slot).await;
            return Err(e.to_string().into());
        }
    };

    *state.recorder.lock().await = Some(recorder);
    // Share the slot with AppState so stop_recording can take ownership
    // later. The bridge holds its own clone; the slot is never swapped out
    // from under it; only its contents change hands.
    *state.session.lock().await = Some(slot.clone());

    // Fresh recording, fresh draft mirror.
    *state.recording_drafts.lock().expect("drafts poisoned") = None;
    // Arm the silence check-in for this recording. The order is the
    // safety here: the clock rebaselines and the generation moves on
    // before the watcher spawns, and the slot cannot turn Streaming until
    // the open below, so a straggler watcher from a recording stopped
    // moments ago exits on the generation check, or at worst reads this
    // fresh clock and stays quiet.
    state
        .last_liveness_at
        .store(epoch_ms(), std::sync::atomic::Ordering::Release);
    state
        .silence_notice_at
        .store(0, std::sync::atomic::Ordering::Release);
    let generation = state
        .recording_generation
        .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
        + 1;
    spawn_silence_watcher(app.clone(), generation);

    // Open the recovery scratch: which meeting is in flight (the stop path
    // reads it back), and from here on its segments, notes, and stars as
    // they arrive ([recording.md] §Crash recovery).
    crate::recovery::begin(&base, &meeting_id);

    // Emit recording-started with provider capabilities and the start
    // instant; the frontend derives elapsed time from it, so the timer
    // survives view remounts instead of restarting from a local counter.
    let started_at = epoch_ms();
    state
        .recording_started_at_ms
        .store(started_at, std::sync::atomic::Ordering::Release);
    app.emit(
        "recording-started",
        serde_json::json!({ "capabilities": capabilities, "started_at": started_at }),
    )
    .map_err(|e| e.to_string())?;

    if let Err(e) = crate::tray::update_tray_recording_state(&app, true) {
        tracing::warn!("failed to update tray icon: {e}");
    }

    // Capture is already rolling; the session opens beside it, with audio
    // waiting in the slot's buffer, so the record button is instant and the
    // first words still reach the transcript. A connect failure or
    // timeout lands in the forwarder's `Failed` machinery exactly like a
    // mid-recording death; the record gate has already refused the
    // configurations that would leave that machinery nothing to do
    // ([transcription.md]).
    spawn_stream_open(
        app.clone(),
        provider,
        provider_is_cloud,
        lane.current_generation(),
        lane.clone(),
        slot.clone(),
    );

    // Say so: cloud was chosen, this recording is on-device. The same
    // banner a mid-recording fallback raises; silently downgrading would
    // leave the user believing they got cloud quality.
    if signed_out_cloud {
        let _ = app.emit("transcription-fallback", &AppError::CloudSignedOut);
    }

    Ok(())
}

#[tauri::command]
pub async fn pause_recording(app: AppHandle, state: State<'_, AppState>) -> Result<(), AppError> {
    if let Some(recorder) = state.recorder.lock().await.as_ref() {
        recorder.pause();
        tracing::info!("recording paused");
    }
    // A cloud stream held open across a pause meters the whole pause
    // against the account's hours (the vendor bills stream duration; the
    // relay keeps silent streams alive with keepalives). End it instead;
    // resume opens a fresh one ([transcription.md]).
    {
        let lane = state.lane.lock().expect("lane poisoned").clone();
        let slot = state.session.lock().await.clone();
        if let Some(slot) = slot {
            stream::pause_stream(&lane, &slot).await;
        }
    }
    // Pausing is an answer: take any silence check-in down (the watcher
    // itself skips paused ticks).
    state
        .silence_notice_at
        .store(0, std::sync::atomic::Ordering::Release);
    let _ = app.emit("silence-cleared", ());
    if let Err(e) = crate::tray::update_tray_recording_state(&app, false) {
        tracing::warn!("failed to update tray icon: {e}");
    }
    Ok(())
}

#[tauri::command]
pub async fn resume_recording(app: AppHandle, state: State<'_, AppState>) -> Result<(), AppError> {
    if let Some(recorder) = state.recorder.lock().await.as_ref() {
        recorder.resume();
        tracing::info!("recording resumed — silence clock rebaselined");
    }
    // Reopen the stream the pause ended: fresh socket, fresh clustering
    // (numbered past the labels already seen), the recording's start-time
    // hints, today's token. A reopen failure lands in the forwarder's
    // `Failed` machinery like any mid-recording death; resume itself
    // cannot fail ([transcription.md]).
    #[cfg(feature = "cloud")]
    {
        use std::sync::atomic::Ordering;
        let lane = state.lane.lock().expect("lane poisoned").clone();
        let slot = state.session.lock().await.clone();
        let request = lane
            .cloud_reopen
            .lock()
            .expect("cloud reopen request poisoned")
            .clone();
        if let (Some(slot), Some(request)) = (slot, request) {
            let waiting = lane.stream_is_cloud.load(Ordering::Acquire)
                && matches!(&*slot.lock().await, SessionSlot::Buffering(_));
            if waiting {
                let config = state.config.lock().await.clone();
                let provider = Arc::new(crate::cloud::transcription::RelayProvider::new(
                    config.cloud_session_token.clone(),
                    config.cloud_url(),
                    request.language_hints,
                    request.diarization,
                    lane.max_speaker_number.load(Ordering::Acquire),
                )) as Arc<dyn transcription::TranscriptionProvider>;
                spawn_stream_open(
                    app.clone(),
                    provider,
                    true,
                    lane.current_generation(),
                    lane.clone(),
                    slot,
                );
            }
        }
    }
    // A paused span is intentional quiet, not silence; the check-in's
    // clock restarts here.
    state
        .last_liveness_at
        .store(epoch_ms(), std::sync::atomic::Ordering::Release);
    state
        .silence_notice_at
        .store(0, std::sync::atomic::Ordering::Release);
    if let Err(e) = crate::tray::update_tray_recording_state(&app, true) {
        tracing::warn!("failed to update tray icon: {e}");
    }
    Ok(())
}

/// Star the current moment of the running recording. Stars live in an
/// AppState accumulator (like segments) so every stop path (button,
/// hotkey, tray, auto-stop) persists them.
/// Star the current moment. Splits the in-flight utterance so words spoken
/// after the star start a new segment, and returns the star's timestamp on
/// the segment timeline (the session's stream clock; the wall clock
/// runs ahead of it by the processing backlog, and a wall-clock star would
/// sort after the very words spoken before it). Falls back to the caller's
/// wall-clock `seconds` when the session can't report a split point.
#[tauri::command]
pub async fn star_moment(state: State<'_, AppState>, seconds: f64) -> Result<f64, AppError> {
    if state.recorder.lock().await.is_none() {
        return Err(AppError::NoActiveRecording);
    }

    // Take the reply handle without holding the outer session lock across
    // the inner await: the bridge can hold the slot for seconds around a
    // stalling send, and the outer lock is what start and stop queue on.
    let slot = state.session.lock().await.clone();
    let reply = match slot {
        Some(shared) => match &*shared.lock().await {
            SessionSlot::Streaming(session) => session.split_utterance(),
            _ => None,
        },
        None => None,
    };

    let mut star_secs = seconds.max(0.0);
    if let Some(rx) = reply {
        if let Ok(Ok(boundary)) =
            tokio::time::timeout(std::time::Duration::from_millis(800), rx).await
        {
            star_secs = boundary;
        }
    }

    let stars = {
        let mut stars = state.stars.lock().await;
        stars.push(star_secs);
        stars.clone()
    };
    // A star is a deliberate mark on the meeting; it survives the process
    // dying alongside the notes it belongs with.
    {
        let config = state.config.lock().await;
        let base = crate::storage::storage_base(&config.storage_dir);
        let drafts = state.recording_drafts.lock().expect("drafts poisoned").clone();
        let (notes, title) = drafts.unwrap_or_default();
        crate::recovery::write_drafts(&base, &notes, &title, &stars);
    }
    crate::telemetry::track(&state, "star_used", serde_json::json!({}));
    Ok(star_secs)
}

/// Turn speaker labeling on or off for the running recording: the
/// transcript header's toggle ([speakers.md] §Live labels).
///
/// Off strips the labels already accumulated as well as every later one,
/// so the transcript never reads as half-labelled, and it is what
/// `finalize_meeting` honors: a recording stopped with labeling off gets
/// no post-meeting speaker pass either. Turning it back on resumes
/// labeling for new segments only; the discarded ones are not restored,
/// because the post-meeting pass re-derives the whole meeting from its
/// audio anyway.
#[tauri::command]
pub async fn set_live_diarization(
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<(), AppError> {
    use std::sync::atomic::Ordering;
    if state.recorder.lock().await.is_none() {
        return Err(AppError::NoActiveRecording);
    }
    state.live_diarization.store(enabled, Ordering::Release);
    if !enabled {
        strip_speakers(&state.current_segments).await;
    }
    tracing::info!(enabled, "live diarization toggled");
    Ok(())
}

/// Remove one starred moment (a gutter-star click during the recording).
#[tauri::command]
pub async fn unstar_moment(state: State<'_, AppState>, seconds: f64) -> Result<(), AppError> {
    let mut stars = state.stars.lock().await;
    if let Some(idx) = stars.iter().position(|s| *s == seconds) {
        stars.remove(idx);
    }
    Ok(())
}

/// Record where each star sits in the user's notes, sent by the frontend
/// on `recording-stopped`, before finalize persists the stars.
#[tauri::command]
pub async fn set_star_anchors(
    state: State<'_, AppState>,
    anchors: Vec<Star>,
) -> Result<(), AppError> {
    *state.star_anchors.lock().await = anchors;
    Ok(())
}

/// Rename a live speaker label during a recording (a pill edit): rewrites
/// the accumulated segments and registers the rename for every later
/// segment of that cluster. The post-meeting pipeline keeps user-given
/// names for the clusters their segments cover (`speakers.rs`).
#[tauri::command]
pub async fn rename_live_speaker(
    state: State<'_, AppState>,
    from: String,
    to: String,
) -> Result<(), AppError> {
    let to = to.trim().to_string();
    if from.is_empty() || to.is_empty() || from == to {
        return Ok(());
    }

    {
        let mut renames = state.live_label_renames.lock().await;
        // Keep the map one-hop: renaming A→B then B→C must send future
        // A-labeled segments straight to C.
        for target in renames.values_mut() {
            if *target == from {
                *target = to.clone();
            }
        }
        renames.insert(from.clone(), to.clone());
    }

    let mut segments = state.current_segments.lock().await;
    for seg in segments.iter_mut() {
        if seg.speaker.as_deref() == Some(from.as_str()) {
            seg.speaker = Some(to.clone());
        }
    }
    Ok(())
}

/// The silence check-in ([detection.md] §Auto-stop on silence): watches
/// the current recording for a configured stretch with no sign of life
/// (no words arriving, no notes activity), raises "Still recording?", and
/// acts on the setting when the fixed grace runs out unanswered. One task
/// per recording, tied to it by `generation`: it exits with the recorder,
/// or the moment a successor recording arms its own watcher. Paused spans
/// and transcription-less recordings never count as silence.
fn spawn_silence_watcher(app: AppHandle, generation: u64) {
    use crate::autodetect::silence::{self, Notice, Verdict};
    use std::sync::atomic::Ordering;

    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(15)).await;
            let state = app.state::<AppState>();
            // Identity first: a stop and restart inside one tick leaves
            // no recorder-less moment for this watcher to exit on, and
            // two watchers would race on the shared check-in state.
            if state.recording_generation.load(Ordering::Acquire) != generation {
                tracing::info!("silence watcher retired: a newer recording owns the check-in");
                break;
            }
            let Some(paused) = state.recorder.lock().await.as_ref().map(|r| r.is_paused()) else {
                break; // the recording ended
            };
            let config = state.config.lock().await.clone();
            let threshold_secs = u64::from(config.silence_stop_minutes) * 60;
            // Only a live session can produce the words this clock counts:
            // transcription off, or an open still pending, must not run
            // a word-based clock against a wordless recording
            // ([detection.md]). Two statements on purpose: the outer lock
            // (which start and stop queue on) must be released before the
            // inner slot is awaited; the bridge can hold the slot for
            // seconds around a stalling send.
            let slot = state.session.lock().await.clone();
            let streaming = match slot {
                Some(slot) => matches!(&*slot.lock().await, SessionSlot::Streaming(_)),
                None => false,
            };
            if threshold_secs == 0 || paused || !streaming {
                continue;
            }
            let now = epoch_ms();
            let silence_secs =
                now.saturating_sub(state.last_liveness_at.load(Ordering::Acquire)) / 1000;
            let notice = match state.silence_notice_at.load(Ordering::Acquire) {
                0 => Notice::None,
                u64::MAX => Notice::StoodDown,
                at => Notice::Pending { age_secs: now.saturating_sub(at) / 1000 },
            };
            match silence::check(silence_secs, threshold_secs, notice) {
                Verdict::Quiet | Verdict::Waiting => {}
                Verdict::Notify => {
                    state.silence_notice_at.store(now, Ordering::Release);
                    // The decision deadline, anchored on the same instant
                    // the grace is measured from, so the notice's countdown
                    // and the Unanswered verdict share one clock. Only when
                    // unanswered means stop: under `keep` the check-in just
                    // stands down, and a countdown to a non-event misleads.
                    let stops_at_ms = matches!(
                        config.silence_stop_unanswered,
                        embral_types::SilenceUnanswered::Stop
                    )
                    .then(|| now + silence::GRACE_SECS * 1000);
                    tracing::info!(
                        minutes = config.silence_stop_minutes,
                        "silence check-in raised"
                    );
                    let _ = app.emit(
                        "silence-notice",
                        serde_json::json!({
                            "minutes": config.silence_stop_minutes,
                            "stops_at_ms": stops_at_ms,
                        }),
                    );
                }
                Verdict::Cleared => {
                    state.silence_notice_at.store(0, Ordering::Release);
                    let _ = app.emit("silence-cleared", ());
                }
                Verdict::Unanswered => {
                    let _ = app.emit("silence-cleared", ());
                    match config.silence_stop_unanswered {
                        embral_types::SilenceUnanswered::Stop => {
                            state.silence_notice_at.store(0, Ordering::Release);
                            tracing::info!("silence check-in unanswered — stopping the recording");
                            request_stop(&app);
                        }
                        embral_types::SilenceUnanswered::Keep => {
                            state.silence_notice_at.store(u64::MAX, Ordering::Release);
                            tracing::info!("silence check-in unanswered — recording continues");
                        }
                    }
                }
            }
        }
    });
}

/// The frontend's notes/title drafts, mirrored into the backend while
/// recording (debounced). A stop that arrives without them (the handshake
/// fallback when the frontend never answers) substitutes this mirror, so
/// the human's words survive every stop path.
#[tauri::command]
pub async fn sync_recording_drafts(
    notes: String,
    meeting_title: String,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    // Onto disk as well: the mirror below survives a stop the frontend
    // never answers, but only the scratch survives the process dying.
    // Debounced by the caller, so this is not a per-keystroke write.
    let base = crate::storage::storage_base(&state.config.lock().await.storage_dir);
    let stars = state.stars.lock().await.clone();
    crate::recovery::write_drafts(&base, &notes, &meeting_title, &stars);
    *state.recording_drafts.lock().expect("drafts poisoned") = Some((notes, meeting_title));
    // The notes-activity half of the check-in's liveness clock: typing or
    // pasting proves the user is working, even in a meeting the mic can't
    // hear ([detection.md] §Auto-stop on silence). Gated on a live
    // recorder because the caller's trailing debounce can land just after
    // a stop.
    if state.recorder.lock().await.is_some() {
        state
            .last_liveness_at
            .store(epoch_ms(), std::sync::atomic::Ordering::Release);
    }
    Ok(())
}

/// The backend's answer to "what is actually happening right now" — the
/// frontend reconciles against it on mount and on window focus, because a
/// hidden webview gets throttled and can drop the events it would
/// otherwise have built this state from (the auto-start-while-hidden bug).
#[derive(serde::Serialize)]
pub struct RecordingStatus {
    pub recording: bool,
    pub paused: bool,
    pub started_at_ms: u64,
    pub labels_authoritative: bool,
    /// This recording's live diarization standing, so a window that missed
    /// the toggle (or the runaway guard) shows the right button state.
    pub diarization: bool,
    pub segments: Vec<embral_types::TranscriptionSegment>,
    /// The picker's current choices, so a reopened window shows them
    /// checked rather than snapping back to the defaults.
    pub selected_apps: Option<Vec<u32>>,
    pub extra_mics: Vec<String>,
}

#[tauri::command]
pub async fn recording_status(state: State<'_, AppState>) -> Result<RecordingStatus, AppError> {
    use std::sync::atomic::Ordering;
    let (recording, paused) = match state.recorder.lock().await.as_ref() {
        Some(r) => (true, r.is_paused()),
        None => (false, false),
    };
    Ok(RecordingStatus {
        recording,
        paused,
        started_at_ms: state.recording_started_at_ms.load(Ordering::Acquire),
        labels_authoritative: state.labels_authoritative.load(Ordering::Acquire),
        diarization: state.live_diarization.load(Ordering::Acquire),
        segments: if recording {
            state.current_segments.lock().await.clone()
        } else {
            Vec::new()
        },
        selected_apps: match &*state
            .system_audio_wanted
            .lock()
            .expect("system audio selection poisoned")
        {
            crate::platform::types::SystemAudioWanted::Everything => None,
            crate::platform::types::SystemAudioWanted::Apps(pids) => Some(pids.clone()),
        },
        extra_mics: state.extra_mics.lock().expect("extra mics poisoned").clone(),
    })
}

/// The source picker's system-audio choice. `None` (nothing unchecked) is
/// everything the machine plays: the default, and the mode that needs no
/// per-app capture. A list narrows the recording to those apps' own audio.
#[tauri::command]
pub async fn set_system_audio_sources(
    apps: Option<Vec<u32>>,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    let wanted = match apps {
        None => crate::platform::types::SystemAudioWanted::Everything,
        Some(pids) => crate::platform::types::SystemAudioWanted::Apps(pids),
    };
    tracing::info!(?wanted, "system audio selection changed");
    *state
        .system_audio_wanted
        .lock()
        .expect("system audio selection poisoned") = wanted;
    // Apply now rather than on the lane's next tick.
    if let Some(recorder) = state.recorder.lock().await.as_ref() {
        recorder.reconfigure_sources();
    }
    Ok(())
}

/// The source picker's extra microphones (beyond the recording's primary
/// mic, which owns the master clock and cannot be removed mid-recording).
#[tauri::command]
pub async fn set_extra_mics(
    devices: Vec<String>,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    tracing::info!(?devices, "extra microphones changed");
    *state.extra_mics.lock().expect("extra mics poisoned") = devices;
    if let Some(recorder) = state.recorder.lock().await.as_ref() {
        recorder.reconfigure_mics();
    }
    Ok(())
}

/// A stop for surfaces that hold no drafts (the notice window): route
/// through the handshake like every backend-initiated stop.
#[tauri::command]
pub async fn request_stop_recording(app: AppHandle) -> Result<(), AppError> {
    request_stop(&app);
    Ok(())
}

/// The check-in's "Keep recording" answer: a fresh full silence window.
#[tauri::command]
pub async fn silence_keep_recording(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    use std::sync::atomic::Ordering;
    state.last_liveness_at.store(epoch_ms(), Ordering::Release);
    state.silence_notice_at.store(0, Ordering::Release);
    app.emit("silence-cleared", ()).map_err(|e| e.to_string())?;
    Ok(())
}

/// Finish the recordings the last run never stopped ([recording.md]
/// §Crash recovery). Called once from `setup()`; the rescued meetings run
/// the ordinary finalize pipeline in the background; a recovered meeting
/// is just a meeting.
///
/// Silent by design. Approving your own recording is a chore, and after a
/// crash the user may not remember there was one; the threshold in
/// `recovery::worth_recovering` is what keeps two-second orphans out of
/// the list instead of a prompt.
pub fn recover_interrupted_recording(app: AppHandle) {
    // The synchronous part is the point: the worklist is frozen and the
    // dead process's current-marker retired before this returns, so
    // nothing this run starts (detection is spawned right after, the
    // user a moment later) can race the rescue's reads.
    let config = match crate::config::load_config() {
        Ok(config) => config,
        Err(e) => {
            tracing::warn!("cannot look for an interrupted recording: {e}");
            return;
        }
    };
    let base = crate::storage::storage_base(&config.storage_dir);
    crate::recovery::clear_stale_current(&base);
    let worklist = crate::recovery::pending(&base);
    if worklist.is_empty() {
        // The usual launch: the last recording stopped normally. Said out
        // loud so a recovery that should have happened is diagnosable.
        tracing::info!("no interrupted recording to recover");
        return;
    }
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let config = state.config.lock().await.clone();
        let db = match state.db().await {
            Ok(db) => db,
            Err(e) => {
                // The scratches stay put; the next launch tries again.
                tracing::warn!("cannot recover the interrupted recordings: {e}");
                return;
            }
        };
        let audio_dir = base.join("audio");
        for meeting_id in worklist {
            let wav_path = audio_dir.join(format!("{meeting_id}.wav"));
            // The attempt is counted before finalize runs, so a rescue
            // that crashes the app still counts toward the three-strike
            // cap; past it the scratch is dropped and the audio kept.
            let found = match crate::recovery::plan_rescue(&base, &meeting_id, &wav_path) {
                crate::recovery::RescuePlan::Rescue(found) => found,
                crate::recovery::RescuePlan::Nothing
                | crate::recovery::RescuePlan::GaveUp => continue,
            };
            let started_at = meeting_start_time(&found.meeting_id);
            // Labels are never authoritative here: a cloud session that died
            // mid-recording left partial diarization at best, so the finalize
            // pipeline re-derives speakers from the audio it has.
            finalize_meeting(
                app.clone(),
                db.clone(),
                base.clone(),
                config.clone(),
                found.meeting_id,
                started_at,
                found.segments,
                AudioSource::Wav(wav_path),
                false,
                found.stars,
                found.user_notes,
                found.user_title,
                Vec::new(),
            )
            .await;
            // Committed (or the save failed and said so, the stop path's
            // exact semantics). Cleared only now: a crash anywhere above
            // retries at the next launch instead of losing the meeting.
            tracing::info!(meeting_id, "recovered meeting committed");
            crate::recovery::clear_for(&base, &meeting_id);
        }
    });
}

/// Ask the frontend to perform the stop, so the notes draft and title
/// travel with it exactly like a stop from the button; a direct backend
/// stop would finalize the meeting without them. The webview runs even
/// while the window is hidden; the timed fallback covers a frontend
/// that has stopped responding, not a normal path.
pub fn request_stop(app: &AppHandle) {
    let _ = app.emit("stop-requested", ());
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        let still_recording = app.state::<AppState>().recorder.lock().await.is_some();
        if still_recording {
            tracing::warn!("frontend did not answer stop-requested; stopping without the drafts");
            if let Err(e) = stop_recording(app.clone(), app.state(), None, None).await {
                tracing::warn!("fallback stop failed: {e}");
            }
        }
    });
}

/// One choke point for every stop path (button, palette, silence stop,
/// tray, handshake fallback): a failed stop is logged and counted once,
/// whatever refused it: the mirror of `start_recording`'s wrapper.
#[tauri::command]
pub async fn stop_recording(
    app: AppHandle,
    state: State<'_, AppState>,
    user_notes: Option<String>,
    meeting_title: Option<String>,
) -> Result<(), AppError> {
    let result = stop_recording_inner(app, &state, user_notes, meeting_title).await;
    if let Err(e) = &result {
        // A stop with nothing to stop is an expected race (a double-press,
        // a hotkey landing after an auto-stop); anything else failing
        // mid-stop is not.
        match e {
            AppError::NoActiveRecording => tracing::warn!("stop refused: {e}"),
            _ => tracing::error!("stop failed: {e}"),
        }
        crate::telemetry::track(
            &state,
            "error",
            serde_json::json!({ "category": "recording_stop_failed" }),
        );
    }
    result
}

async fn stop_recording_inner(
    app: AppHandle,
    state: &State<'_, AppState>,
    user_notes: Option<String>,
    meeting_title: Option<String>,
) -> Result<(), AppError> {
    let mut config = state.config.lock().await.clone();
    // This recording's standing wins over the setting: labeling turned off
    // mid-meeting (by the toggle or the runaway guard) must not come back
    // as a post-meeting speaker pass over the same audio.
    config.diarization_enabled = state
        .live_diarization
        .load(std::sync::atomic::Ordering::Acquire);
    let base = crate::storage::storage_base(&config.storage_dir);
    let db = state.db().await?;
    // Absent args mean a stop the frontend never answered (the handshake
    // fallback); the mirrored drafts stand in. A frontend stop always
    // sends its strings, empty included, so `None` is never "cleared".
    let (user_notes, meeting_title) = if user_notes.is_none() && meeting_title.is_none() {
        let mirrored = state.recording_drafts.lock().expect("drafts poisoned").clone();
        match mirrored {
            Some((notes, title)) => (Some(notes), Some(title)),
            None => (None, None),
        }
    } else {
        (user_notes, meeting_title)
    };
    let user_title = meeting_title
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    // Which meeting is in flight, from the recovery scratch. It is not
    // cleared here: finalize still has the slow part ahead of it (speaker
    // pipeline, LLM refinement), and a crash in there must still leave the
    // next launch something to re-run from. The background task below
    // clears it once the meeting is committed.
    let meeting_id =
        crate::recovery::active_meeting_id(&base).ok_or(AppError::NoActiveRecording)?;

    // --- Foreground (fast path): stop recorder, hand off to background. ---
    let recorder = state
        .recorder
        .lock()
        .await
        .take()
        .ok_or_else(|| AppError::internal("No active recorder"))?;
    // Blocking work (a bounded thread join, the WAV finalize) goes to the
    // blocking pool: the async workers stay free for the commands and
    // events a stop fans out.
    let wav_path = tauri::async_runtime::spawn_blocking(move || recorder.stop())
        .await
        .map_err(|e| AppError::internal(format!("recorder stop task died: {e}")))?
        .map_err(AppError::internal)?;

    // Bounded, not because anyone is expected to hold the outer lock for
    // long (nobody may) but because a stop that cannot have it must
    // surface as a failed stop (logged, counted, shown on screen) instead
    // of a command that never returns. The 2026-08 hang was exactly this
    // lock, held by a watcher that was itself stuck behind a stalled
    // socket send.
    let session_arc = match tokio::time::timeout(SESSION_FINISH_TIMEOUT, state.session.lock()).await
    {
        Ok(mut outer) => outer
            .take()
            .ok_or_else(|| AppError::internal("No active session"))?,
        Err(_) => {
            return Err(AppError::internal(
                "the recording session did not respond in time",
            ))
        }
    };

    // The recording is moving past whatever stream is running: no open
    // resolving from here on may install, and dropping the lane's sender
    // lets the forwarder end on channel close once the last stream drains,
    // which is what the background task below waits on before it
    // snapshots the segments.
    let lane = state.lane.lock().expect("lane poisoned").clone();
    lane.bump_generation();
    lane.take_event_tx();
    let forwarder = state
        .forwarder_task
        .lock()
        .expect("forwarder handle poisoned")
        .take();

    let segments_acc = state.current_segments.clone();

    // Whether this session's provider labels are final (snapshotted at start).
    let labels_authoritative = state
        .labels_authoritative
        .load(std::sync::atomic::Ordering::Acquire);

    // Starred moments accumulated during the recording (any stop path,
    // button, hotkey, tray, auto-stop, picks them up here). Their notes
    // anchors arrive from the frontend just after `recording-stopped`
    // fires, so the merge happens in the background task below.
    let star_seconds = std::mem::take(&mut *state.stars.lock().await);

    // Any stop (manual, hotkey, or auto) ends the auto-started tracking;
    // the swapped-out value feeds the telemetry event below.
    let auto_started = state
        .auto_started
        .swap(false, std::sync::atomic::Ordering::AcqRel);

    // Tell the UI we're done recording â€” it transitions to the processing view
    // (which renders the checklist). Everything below runs detached.
    app.emit("recording-stopped", ())
        .map_err(|e| e.to_string())?;
    if let Err(e) = crate::tray::update_tray_recording_state(&app, false) {
        tracing::warn!("failed to update tray icon: {e}");
    }

    // --- Background: bounded finish, encode, refine, write notes. ---
    let app_bg = app.clone();
    tokio::spawn(async move {
        // 1. End the current stream, every part under a deadline, the
        //    slot take included, because a stalled bridge send can hold
        //    the lock for a few seconds and finalize must not bet the
        //    meeting on it clearing ([recording.md] §Lifecycle). Source of
        //    truth for segments is `segments_acc`, populated by the event
        //    forwarder during recording; finish()'s return is unused.
        stream::finish_current_stream(&session_arc, SESSION_FINISH_TIMEOUT).await;
        // 2. Wait for the forwarder: it ends once every stream's pump has
        //    drained (a retired stream's tail can still be arriving), and
        //    only then is the accumulator complete.
        if let Some(handle) = forwarder {
            if tokio::time::timeout(SESSION_FINISH_TIMEOUT, handle).await.is_err() {
                tracing::warn!(
                    "event forwarder still draining after {:?} — snapshotting what has landed",
                    SESSION_FINISH_TIMEOUT
                );
            }
        }

        // Snapshot accumulated segments and hand off to the shared pipeline.
        let mut segments = segments_acc.lock().await.clone();
        // Streams hand over out of order only at the seams (a retired
        // stream's tail vs. its successor's first words); readers get time
        // order.
        segments.sort_by(|a, b| {
            a.start
                .partial_cmp(&b.start)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let started_at = meeting_start_time(&meeting_id);

        // The configured provider: neither the power policy's per-meeting
        // choice nor a mid-recording cloud→local fallback shows here; the
        // latter is error{transcription_failed} ([telemetry.md]).
        let duration_secs = segments.last().map(|s| s.end as u64).unwrap_or_else(|| {
            chrono::Utc::now()
                .signed_duration_since(started_at)
                .num_seconds()
                .max(0) as u64
        });
        crate::telemetry::track(
            &app_bg.state::<AppState>(),
            "meeting_recorded",
            serde_json::json!({
                "provider": config.transcription_provider,
                "duration_bucket": crate::telemetry::meeting_bucket(duration_secs),
                "auto_started": auto_started,
            }),
        );

        // Attach the notes anchors the frontend sent at stop (matched by
        // the exact timestamp; a missing anchor just means no notes line).
        let anchors =
            std::mem::take(&mut *app_bg.state::<AppState>().star_anchors.lock().await);
        // Names the user renamed away from during the session; finalize
        // prunes their profiles if nothing ends up linked to them. Generic
        // "Speaker N" labels never had profiles to begin with.
        let superseded_labels: Vec<String> =
            std::mem::take(&mut *app_bg.state::<AppState>().live_label_renames.lock().await)
                .into_keys()
                .filter(|label| !embral_types::is_generic_speaker_label(label))
                .collect();
        let stars: Vec<Star> = star_seconds
            .into_iter()
            .map(|seconds| Star {
                seconds,
                note_block: anchors
                    .iter()
                    .find(|a| a.seconds == seconds)
                    .and_then(|a| a.note_block),
            })
            .collect();
        let finalized_id = meeting_id.clone();
        finalize_meeting(
            app_bg,
            db,
            base.clone(),
            config,
            meeting_id,
            started_at,
            segments,
            AudioSource::Wav(wav_path),
            labels_authoritative,
            stars,
            user_notes,
            user_title,
            superseded_labels,
        )
        .await;
        // The meeting is committed (or its save failed and said so): the
        // scratch has nothing left to protect. Until this line, a crash
        // anywhere in finalize is recoverable at the next launch. Scoped
        // to this meeting's id: a successor recording may already be live
        // with its own scratch, and a slow finalize must not touch it.
        crate::recovery::clear_for(&base, &finalized_id);
    });

    Ok(())
}

/// Accept the "call detected" prompt: start recording and mark it
/// auto-started so it also auto-stops when the call ends.
#[tauri::command]
pub async fn accept_detected_meeting(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    state
        .auto_started
        .store(true, std::sync::atomic::Ordering::Release);
    crate::telemetry::track(
        &state,
        "detection_response",
        serde_json::json!({ "action": "accepted" }),
    );
    let result = start_recording(app.clone(), app.state()).await;
    if result.is_err() {
        state
            .auto_started
            .store(false, std::sync::atomic::Ordering::Release);
    }
    result
}

/// Dismiss the "call detected" prompt for the rest of the current call.
/// Emits `meeting-dismissed` so both prompt surfaces (in-app banner,
/// notice window) come down together whichever one answered.
#[tauri::command]
pub async fn dismiss_detected_meeting(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    state
        .detection_dismissed
        .store(true, std::sync::atomic::Ordering::Release);
    crate::telemetry::track(
        &state,
        "detection_response",
        serde_json::json!({ "action": "dismissed" }),
    );
    let _ = app.emit("meeting-dismissed", ());
    Ok(())
}

#[cfg(test)]
mod diarization_tests {
    use super::*;

    #[test]
    fn a_plausible_meeting_keeps_its_labels() {
        // Six people round a table is a meeting, not a malfunction.
        for distinct in 1..=MAX_LIVE_SPEAKERS {
            assert!(!diarization_has_run_away(distinct), "{distinct} speakers");
        }
    }

    #[test]
    fn one_speaker_too_many_stands_the_labels_down() {
        // The real failure is one voice splitting into a crowd, not a
        // genuinely large meeting: past the ceiling the labels have
        // stopped being evidence of anything.
        assert!(diarization_has_run_away(MAX_LIVE_SPEAKERS + 1));
        assert!(diarization_has_run_away(40));
    }

    fn labeled(speaker: &str) -> embral_types::TranscriptionSegment {
        embral_types::TranscriptionSegment {
            speaker: Some(speaker.to_string()),
            speaker_id: None,
            text: "words".to_string(),
            start: 0.0,
            end: 1.0,
        }
    }

    #[test]
    fn a_rename_does_not_double_count_its_cluster() {
        // The guard counts the provider's own labels: renaming Speaker 2
        // to Alice must not leave both in the distinct set, or six real
        // renames would trip a guard meant for a runaway clusterer.
        let seen = std::sync::Mutex::new(std::collections::HashSet::new());
        let renames =
            std::collections::HashMap::from([("Speaker 2".to_string(), "Alice".to_string())]);
        let mut seg = labeled("Speaker 2");
        assert!(!label_segment(&mut seg, true, &seen, &renames));
        assert_eq!(seg.speaker.as_deref(), Some("Alice"));
        let mut again = labeled("Speaker 2");
        assert!(!label_segment(&mut again, true, &seen, &renames));
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert!(seen.contains("Speaker 2"));
    }

    #[test]
    fn labeling_off_strips_and_counts_nothing() {
        let seen = std::sync::Mutex::new(std::collections::HashSet::new());
        let renames = std::collections::HashMap::new();
        let mut seg = labeled("Speaker 1");
        assert!(!label_segment(&mut seg, false, &seen, &renames));
        assert_eq!(seg.speaker, None);
        assert!(seen.lock().unwrap().is_empty());
    }

    #[test]
    fn the_label_past_the_ceiling_trips_the_guard() {
        let seen = std::sync::Mutex::new(std::collections::HashSet::new());
        let renames = std::collections::HashMap::new();
        for n in 1..=MAX_LIVE_SPEAKERS {
            let mut seg = labeled(&format!("Speaker {n}"));
            assert!(!label_segment(&mut seg, true, &seen, &renames), "{n}");
            assert!(seg.speaker.is_some());
        }
        let mut one_too_many = labeled("Speaker 7");
        assert!(label_segment(&mut one_too_many, true, &seen, &renames));
        // The tripping segment itself comes back bare.
        assert_eq!(one_too_many.speaker, None);
    }
}
