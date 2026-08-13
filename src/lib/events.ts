import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type {
  InterimSegment,
  MeetingRecord,
  ProviderCapabilities,
  TranscriptionSegment
} from '$lib/types';
import { appState } from '$lib/stores/app-state.svelte';
import { modelsStore } from '$lib/stores/models.svelte';
import { meetingsStore, PENDING_MEETING_ID } from '$lib/stores/meetings.svelte';
import { configStore } from '$lib/stores/config.svelte';
import { dictationStore } from '$lib/stores/dictation.svelte';
import { updaterStore } from '$lib/stores/updater.svelte';
import { copy } from '$lib/copy';
import { errorMessage } from '$lib/copy/errors';
import { displayAppName } from '$lib/utils/detectedApp';
import { fixtureActive } from '$lib/fixture';
import type { ModelProgress } from '$lib/types';

/// Whether a notification event may fire, per the user's notification config.
/// There is no master switch — each event owns its own toggle.
function notificationsAllowed(
  event: 'summary_ready' | 'recording_started' | 'call_detected'
): boolean {
  const cfg = configStore.config;
  if (!cfg) return false;
  switch (event) {
    case 'summary_ready':
      return cfg.notify_summary_ready;
    case 'recording_started':
      return cfg.notify_recording_started;
    case 'call_detected':
      return cfg.notify_call_detected;
  }
}

// Module-level registry of the listeners installed by the most recent
// `setupEventListeners` call. Vite HMR can remount the page component
// (which calls `setupEventListeners` again from `onMount`) without
// destroying the prior Tauri listeners, so without this guard each HMR
// cycle stacks another full set of listeners — observed in practice as
// transcript segments being persisted N× (one duplicate per HMR cycle).
let _activeUnlisteners: UnlistenFn[] | null = null;

export async function setupEventListeners(): Promise<UnlistenFn[]> {
  if (_activeUnlisteners) {
    for (const fn of _activeUnlisteners) {
      try {
        fn();
      } catch {
        // Listener already torn down; ignore.
      }
    }
    _activeUnlisteners = null;
  }

  const unlisteners: UnlistenFn[] = [];

  unlisteners.push(
    await listen<{ capabilities: ProviderCapabilities; started_at: number }>(
      'recording-started',
      async (e) => {
        appState.setView('recording');
        appState.setRecording(true);
        appState.startRecordingClock(e.payload.started_at);
        appState.clearSegments();
        appState.setError(null);
        appState.setFallbackNotice(null);
        appState.setDetectedApp(null);
        appState.setProviderCapabilities(e.payload.capabilities);
        // Labeling starts from the setting every recording, exactly as the
        // backend's own per-recording flag does — otherwise a meeting that
        // stood labeling down leaves the next one's header saying so while
        // the backend is happily labeling. The guard history clears first:
        // with the setting off, adopting it is a no-op call, which now
        // preserves the reason by design.
        appState.clearDiarizationRunaway();
        appState.setLiveDiarization(configStore.config?.diarization_enabled ?? true);

        // Notices fire whatever the window state — tray, minimized, or
        // open; the per-event toggle is the only gate.
        if (notificationsAllowed('recording_started')) {
          await invoke('notify', {
            payload: {
              kind: 'recording_started',
              title: copy.notifications.os.recordingStarted.title,
              actions: [],
              sticky: false
            }
          });
        }
      }
    )
  );

  unlisteners.push(
    await listen<{ mic: number[]; system: number[] }>('audio-level', (e) => {
      appState.setAudioLevel(e.payload.mic, e.payload.system);
    })
  );

  // The encoded MP3 lands well before the notes finish — the pending
  // meeting mounts its player as soon as it exists.
  unlisteners.push(
    await listen<string>('pending-audio-ready', (e) => {
      appState.setPendingAudioPath(e.payload);
    })
  );

  unlisteners.push(
    await listen<{ minutes: number; stops_at_ms?: number | null }>(
      'silence-notice',
      async (e) => {
        // The silence check-in ("Still recording?"): banner in-app plus the
        // notice, whatever the window state — this event precedes a
        // possible auto-stop, so it has no per-event toggle; turning the
        // feature off is silence_stop_minutes = 0.
        appState.setSilenceNotice(e.payload.minutes);
        await invoke('notify', {
          payload: {
            kind: 'silence',
            title: copy.notifications.os.stillRecording.title,
            actions: [
              // Shorter words than the in-app banner's, same commands: the
              // notice row also holds the title and the countdown.
              { id: 'keep', label: copy.notifications.os.stillRecording.keep },
              { id: 'stop', label: copy.notifications.os.stillRecording.stop }
            ],
            sticky: true,
            // The decision deadline — present only when unanswered means
            // stop; the notice renders it as a countdown.
            countdown_until_ms: e.payload.stops_at_ms ?? null
          }
        });
      }
    )
  );

  unlisteners.push(
    await listen('silence-cleared', () => {
      appState.setSilenceNotice(null);
    })
  );

  unlisteners.push(
    await listen('recording-start-failed', async (e) => {
      // A detected call that could not be recorded: the in-app error slot
      // plus a notice, because the user is by definition elsewhere.
      const reason = errorMessage(e.payload);
      appState.setError(reason);
      await invoke('notify', {
        payload: {
          kind: 'start_failed',
          title: copy.notifications.os.startFailed.title,
          actions: [],
          sticky: false,
          target: { kind: 'app' }
        }
      });
    })
  );

  unlisteners.push(
    await listen<{ kind: string; id?: string }>('notice-navigate', async (e) => {
      // A notice's body-click: the backend already surfaced the window;
      // land where the news lives.
      if (e.payload.kind === 'meeting' && e.payload.id) {
        appState.setView('idle');
        await meetingsStore.refreshAndSelect(e.payload.id);
      } else if (e.payload.kind === 'updates') {
        appState.openSettings('about');
      }
    })
  );

  unlisteners.push(
    await listen('meeting-dismissed', () => {
      // Dismissed from the notice window (or anywhere): the in-app banner
      // comes down with it.
      appState.setDetectedApp(null);
    })
  );

  unlisteners.push(
    await listen('stop-requested', () => {
      // A backend-initiated stop (call-end auto-stop, silence): performed
      // from here so the notes draft and title travel exactly like a stop
      // from the button; the backend falls back to a bare stop only if
      // this listener never answers.
      const snapshot = appState.recordingSnapshot();
      appState.setPendingTitleHint(snapshot?.title ?? '');
      invoke('stop_recording', {
        userNotes: snapshot?.notes ?? null,
        meetingTitle: snapshot?.title ?? null
      }).catch((e) => {
        // A backend-requested stop that fails must be visible like a
        // button stop's failure, not only a console line.
        console.error('requested stop failed:', e);
        appState.setError(errorMessage(e));
      });
    })
  );

  unlisteners.push(
    await listen('recording-stopped', () => {
      // Capture where each star sits in the notes before the recording
      // view unmounts, so the saved notes can re-anchor them.
      const anchors = appState.collectStarAnchors();
      if (anchors.length > 0) {
        invoke('set_star_anchors', { anchors }).catch((e) =>
          console.error('set_star_anchors failed:', e)
        );
      }
      // Straight back to the Meetings page: the just-stopped meeting shows
      // there immediately as a pending entry (transcript in hand, notes and
      // audio in progress) instead of a separate processing screen.
      appState.setRecording(false);
      appState.setSilenceNotice(null);
      appState.beginPendingMeeting();
      appState.setView('idle');
      void meetingsStore.select(PENDING_MEETING_ID);
    })
  );

  unlisteners.push(
    await listen<TranscriptionSegment>('transcription-segment', (e) => {
      appState.addSegment(e.payload);
    })
  );

  // Cloud transcription died mid-recording and a local session took over
  // (cloud builds only ever emit this). The recording continues; the
  // banner explains the switch.
  unlisteners.push(
    await listen<{ speakers: number }>('diarization-disabled', (e) => {
      // The runaway guard: more voices than a meeting plausibly has, so
      // the backend stood labeling down and stripped what it had. Say so
      // in the error slot — silently losing the speaker names would read
      // as the app breaking.
      appState.standDownDiarization();
      appState.stripSpeakers();
      appState.setError(copy.meetings.live.tooManySpeakers(e.payload.speakers));
    })
  );

  unlisteners.push(
    await listen('transcription-fallback', async (e) => {
      appState.setFallbackNotice(
        copy.notifications.notices.switchedToLocal(errorMessage(e.payload))
      );
      // A mid-recording provider switch is news about the recording, so it
      // rides the recording toggle now that there is no master switch.
      if (notificationsAllowed('recording_started')) {
        await invoke('notify', {
          payload: {
            kind: 'switched_to_local',
            title: copy.notifications.os.switchedToLocal.title,
            actions: [],
            sticky: false
          }
        });
      }
    })
  );

  // The session died with nothing to fall back to; live transcription is
  // over for this recording (audio capture itself continues).
  unlisteners.push(
    await listen('transcription-failed', (e) => {
      appState.setError(
        copy.notifications.notices.transcriptionStopped(errorMessage(e.payload))
      );
    })
  );

  // Cloud hours ran out (or the cloud refused) with "disable transcription"
  // chosen: the recording and notes continue, deliberately without a
  // transcript. A notice, not an error — this is the configured behavior.
  unlisteners.push(
    await listen('transcription-disabled', (e) => {
      appState.setFallbackNotice(
        copy.notifications.notices.transcriptionOff(errorMessage(e.payload))
      );
    })
  );

  unlisteners.push(
    await listen<InterimSegment>('transcription-interim', (e) => {
      appState.setInterim(e.payload);
    })
  );

  unlisteners.push(
    await listen<string>('transcription-final-complete', () => {
      appState.setProcessingStep('finalizing-transcript');
    })
  );

  unlisteners.push(
    await listen('notes-generation-started', () => {
      appState.setProcessingStep('generating-notes');
    })
  );

  unlisteners.push(
    await listen<MeetingRecord>('notes-generation-complete', async (e) => {
      appState.setImporting(false);
      // Imports still use the processing view; live meetings finish on the
      // Meetings page with the pending entry selected.
      const wasProcessing = appState.view === 'processing';
      const pendingSelected = meetingsStore.selectedId === PENDING_MEETING_ID;
      if (wasProcessing) {
        appState.resetToIdle();
      }
      if (wasProcessing || pendingSelected || meetingsStore.selectedId === null) {
        await meetingsStore.refreshAndSelect(e.payload.id);
      } else {
        // The user moved on to another meeting — don't steal the selection.
        await meetingsStore.load();
      }
      // Clear the pending entry only after the real record took over the
      // selection, so the detail pane never blinks through an empty state.
      appState.clearPendingMeeting();

      if (notificationsAllowed('summary_ready')) {
        await invoke('notify', {
          payload: {
            kind: 'notes_ready',
            title: copy.notifications.os.notesReady.title,
            actions: [],
            sticky: false,
            target: { kind: 'meeting', id: e.payload.id }
          }
        });
      }
    })
  );

  unlisteners.push(
    await listen<{ meeting_id: string; title: string; url: string }>(
      'webhook-delivery-failed',
      async (e) => {
        // Fired after the backend's last retry. Configuring a webhook is
        // the opt-in, so no notification toggle gates this — a silent
        // failure would be the worst outcome. The meeting is one click
        // away via the target; the URL and error are in the log.
        await invoke('notify', {
          payload: {
            kind: 'webhook_failed',
            title: copy.notifications.os.webhookFailed.title,
            actions: [],
            sticky: false,
            target: { kind: 'meeting', id: e.payload.meeting_id }
          }
        });
      }
    )
  );

  unlisteners.push(
    await listen('processing-error', (e) => {
      const message = errorMessage(e.payload);
      appState.setError(message);
      appState.setPendingError(message);
    })
  );

  // Meeting detection (prompt policy): show the in-app banner and, when the
  // window is hidden, a notification. The banner clears when the call ends
  // or a recording starts.
  unlisteners.push(
    await listen<{ app: string }>('meeting-detected', async (e) => {
      appState.setDetectedApp(e.payload.app);
      if (notificationsAllowed('call_detected')) {
        await invoke('notify', {
          payload: {
            kind: 'call_detected',
            title: copy.notifications.os.callDetected.title(
              displayAppName(e.payload.app)
            ),
            actions: [{ id: 'accept', label: copy.shell.detectionBanner.record }],
            sticky: true
          }
        });
      }
    })
  );

  unlisteners.push(
    await listen('meeting-ended', () => {
      appState.setDetectedApp(null);
    })
  );

  // Import flow: file transcription progress renders in the processing view;
  // completion arrives through the shared notes-generation-complete path.
  unlisteners.push(
    await listen('import-started', () => {
      appState.setImporting(true);
      appState.setError(null);
      appState.setView('processing');
      appState.setProcessingStep('transcribing-import');
    })
  );

  unlisteners.push(
    await listen<{ fraction: number }>('import-progress', (e) => {
      appState.setImportFraction(e.payload.fraction);
    })
  );

  // Local model downloads — handled globally so progress and the
  // configured-state refresh survive leaving the Settings view mid-download.
  unlisteners.push(
    await listen<ModelProgress>('model-download-progress', (e) => {
      modelsStore._onProgress(e.payload);
    })
  );

  unlisteners.push(
    await listen('model-download-complete', () => {
      // Refresh statuses so `configStore.isConfigured` (and the record
      // button) update immediately.
      modelsStore._onComplete();
    })
  );

  unlisteners.push(
    await listen<boolean>('dictation-active', (e) => {
      dictationStore._setActive(e.payload);
    })
  );

  _activeUnlisteners = unlisteners;
  installSyncHooks();
  // Catch up immediately too: a recording may already be running when this
  // window's page (re)loads.
  void syncRecordingStatus();
  scheduleStartupUpdateCheck();
  return unlisteners;
}

// One quiet update check per app run, well after boot so it never competes
// with the startup path — and skipped outright if a recording is already
// live (an auto-detected meeting can start before the timer fires). The
// flag lives at module level so HMR remounts don't stack timers.
/// Reconcile the UI with what the backend is actually doing. All of this
/// state normally arrives as events — but a hidden webview gets throttled
/// by the OS and can drop them wholesale (an auto-started recording once
/// left the surfaced window showing a dead idle shell). Called on mount
/// and whenever the window regains focus or visibility.
export async function syncRecordingStatus(): Promise<void> {
  // A staged screenshot moment ($lib/fixture) is deliberately not what the
  // backend is doing; reconciling would wipe it to idle.
  if (fixtureActive()) return;
  const status = await invoke<{
    recording: boolean;
    paused: boolean;
    started_at_ms: number;
    labels_authoritative: boolean;
    diarization: boolean;
    segments: TranscriptionSegment[];
    selected_apps: number[] | null;
    extra_mics: string[];
  }>('recording_status').catch(() => null);
  // Re-checked after the await: a fixture that loaded while this call was
  // in flight must not be reconciled away either.
  if (!status || fixtureActive()) return;

  if (status.recording && !appState.isRecording) {
    // The window missed recording-started: rebuild the live view.
    appState.setView('recording');
    appState.setRecording(true);
    appState.startRecordingClock(status.started_at_ms);
    appState.setPaused(status.paused);
    // Only the label authority survives backend-side; the session cap is
    // display-only and absent here (0 = no cap shown).
    appState.setProviderCapabilities({
      labels_authoritative: status.labels_authoritative,
      max_session_minutes: 0
    });
    appState.replaceSegments(status.segments);
    appState.setLiveDiarization(status.diarization);
    appState.setDetectedApp(null);
  } else if (!status.recording && appState.isRecording) {
    // The window missed the stop; the meeting persisted backend-side.
    appState.resetToIdle();
    await meetingsStore.load();
  }
  // The same throttling can strand the models statuses the record button
  // gates on ("Configure transcription" on a fully configured machine).
  if (!configStore.isConfigured) {
    void configStore.load();
  }
}

let _syncHooksInstalled = false;
function installSyncHooks() {
  if (_syncHooksInstalled) return;
  _syncHooksInstalled = true;
  window.addEventListener('focus', () => void syncRecordingStatus());
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'visible') void syncRecordingStatus();
  });
}

let _updateCheckScheduled = false;
function scheduleStartupUpdateCheck() {
  if (_updateCheckScheduled) return;
  _updateCheckScheduled = true;
  setTimeout(() => {
    if (appState.isRecording) return;
    void updaterStore.checkNow({ silent: true, notify: true });
  }, 30_000);
}
