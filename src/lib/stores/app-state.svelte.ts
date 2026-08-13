import type { InterimSegment, MeetingStar, TranscriptionSegment } from '$lib/types';
import { copy } from '$lib/copy';

export type AppView =
  | 'idle'
  | 'recording'
  | 'processing'
  | 'speakers'
  | 'dictation'
  | 'settings';
export type ProcessingStep =
  | 'transcribing-import'
  | 'finalizing-transcript'
  | 'generating-notes'
  | null;

/** A just-stopped meeting not yet written to the database: the Meetings
 * page shows it immediately (transcript already in hand, notes and audio
 * marked in progress) until `notes-generation-complete` replaces it with
 * the persisted record. */
export interface PendingMeeting {
  title: string;
  /** Recorded length, frozen at stop. */
  durationSeconds: number;
  segments: TranscriptionSegment[];
  /** Starred moments with their notes anchors, snapshotted at stop. */
  stars: MeetingStar[];
  /** The user's raw notes, snapshotted at stop (the pending Notes tab). */
  userNotes: string;
  /** The encoded MP3, as soon as finalize produces it (playable well
   * before the notes finish). */
  audioPath: string | null;
  error: string | null;
}

let _view = $state<AppView>('idle');
let _isRecording = $state(false);
let _isPaused = $state(false);
let _segments = $state<TranscriptionSegment[]>([]);
// Single live-preview slot. The transcription contract is single-stream: at
// most one utterance is in flight at a time (the local engine finalizes on
// speaker change to conform; any future provider must too). A Segment always
// means the current interim has been committed.
let _interim = $state<InterimSegment | null>(null);
// The recording clock, derived from the backend's start instant so elapsed
// time survives view remounts. Paused spans are accumulated here (every
// pause path funnels through setPaused); stopping freezes the clock via
// `recordingEndedAt` so the processing view can show the final duration.
let _recordingStartedAt = $state<number | null>(null);
let _recordingEndedAt = $state<number | null>(null);
let _pausedAccumMs = $state(0);
let _pausedSince = $state<number | null>(null);
// The latest ~10 Hz per-band mic/system spectra feeding the header meter
// (one stationary bar per band).
let _levelBands = $state<{ mic: number[]; system: number[] }>({
  mic: [],
  system: []
});
/** One starred moment: the id ties the notes-gutter widget, the live
 * transcript marker, and the backend accumulator entry together so a
 * gutter click can remove all three. */
export interface StarredMoment {
  id: number;
  seconds: number;
}
// Starred moments of the current recording; mirrors the backend
// accumulator for live display.
let _stars = $state<StarredMoment[]>([]);
let _nextStarId = 1;
let _recordingSnapshotProvider:
  | (() => { notes: string; title: string; starBlocks: Map<number, number> })
  | null = null;
let _processingStep = $state<ProcessingStep>(null);
let _error = $state<string | null>(null);
// The silence check-in ("Still recording?"): minutes without speech, or
// null when no check-in is up.
let _silenceNoticeMinutes = $state<number | null>(null);
// Shadow mode: the user has asked the screen to stop announcing that a
// meeting is being recorded. Its own control, deliberately not tied to
// the transcript being shut: collapsing a pane for room is not the same
// request as going quiet. Per-recording, never remembered: a recording
// that starts with no visible indication because of a choice made weeks
// ago is a trap, and the tray staying lit is not enough on its own.
let _shadowMode = $state(false);
// Whether the running recording is labeling speakers. Mirrors the
// backend's per-recording flag, which starts from the setting and can be
// turned off by the header toggle or the runaway guard.
let _liveDiarization = $state(true);
// Which of the two turned it off. The header note names the reason,
// because "the app found too many voices" is news to the reader, while
// "you turned this off" is not.
let _diarizationRunaway = $state(false);
// Mid-recording provider swap notice (cloud -> local); shown as a quiet
// banner in the recording view, cleared when the next recording starts.
let _fallbackNotice = $state<string | null>(null);
// Import flow: the transcription fraction 0..1 while the file is being
// processed, null when no import is running.
let _importFraction = $state<number | null>(null);
// A detected call awaiting the user's decision (prompt policy).
let _detectedApp = $state<string | null>(null);
// The finalize-in-flight meeting shown on the Meetings page after stop.
let _pendingMeeting = $state<PendingMeeting | null>(null);
// Title typed in the recording header, handed to the pending meeting by the
// stop path (tray/auto stops have none and fall back to a placeholder).
let _pendingTitleHint = $state<string | null>(null);
// One-shot deep-link targets consumed by the destination view on arrival.
let _settingsTarget = $state<string | null>(null);
let _profilesCreateRequest = $state(false);

export const appState = {
  get view() {
    return _view;
  },
  get isRecording() {
    return _isRecording;
  },
  get isPaused() {
    return _isPaused;
  },
  get segments() {
    return _segments;
  },
  get interim() {
    return _interim;
  },
  get processingStep() {
    return _processingStep;
  },
  get error() {
    return _error;
  },
  get importFraction() {
    return _importFraction;
  },
  get detectedApp() {
    return _detectedApp;
  },
  setDetectedApp(app: string | null) {
    _detectedApp = app;
  },

  /** Navigate to a specific settings page (palette deep links). */
  openSettings(section: string | null = null) {
    _settingsTarget = section;
    _view = 'settings';
  },
  /** Pending deep-link target; the settings layout clears it on arrival. */
  get settingsTarget() {
    return _settingsTarget;
  },
  clearSettingsTarget() {
    _settingsTarget = null;
  },

  /** Navigate to Profiles in create-a-profile mode. */
  openProfilesCreate() {
    _profilesCreateRequest = true;
    _view = 'speakers';
  },
  get profilesCreateRequest() {
    return _profilesCreateRequest;
  },
  clearProfilesCreateRequest() {
    _profilesCreateRequest = false;
  },

  setView(v: AppView) {
    _view = v;
  },
  get recordingStartedAt() {
    return _recordingStartedAt;
  },
  get levelBands() {
    return _levelBands;
  },
  /** Arm the recording clock from the backend's start instant (epoch ms). */
  startRecordingClock(startedAt: number) {
    _recordingStartedAt = startedAt;
    _recordingEndedAt = null;
    // A recording stopped while paused leaves the flag set; every new
    // recording starts unpaused (the backend's recorder always does).
    _isPaused = false;
    _pausedAccumMs = 0;
    _pausedSince = null;
    _levelBands = { mic: [], system: [] };
    _stars = [];
    // Every recording starts announcing itself; going quiet is a choice
    // made about the meeting in front of you, not a standing setting.
    _shadowMode = false;
  },
  get stars() {
    return _stars;
  },
  /** Record a starred moment at the given recording offset (seconds). */
  addStar(seconds: number): StarredMoment {
    const star = { id: _nextStarId++, seconds };
    _stars = [..._stars, star];
    return star;
  },
  removeStar(id: number): StarredMoment | undefined {
    const star = _stars.find((s) => s.id === id);
    if (star) _stars = _stars.filter((s) => s.id !== id);
    return star;
  },
  /** The live notes text, title draft, and each star's notes position,
   * supplied by the page that owns the notes editor and read at stop
   * (before the recording view unmounts). */
  setRecordingSnapshotProvider(
    fn: (() => { notes: string; title: string; starBlocks: Map<number, number> }) | null
  ) {
    _recordingSnapshotProvider = fn;
  },
  /** The current drafts, for stops the backend initiates (auto-stop,
   * silence); the stop must carry them exactly like the stop button does. */
  recordingSnapshot() {
    return _recordingSnapshotProvider?.() ?? null;
  },
  collectStarAnchors(): { seconds: number; note_block: number | null }[] {
    const blocks = _recordingSnapshotProvider?.().starBlocks ?? new Map<number, number>();
    return _stars.map((s) => ({
      seconds: s.seconds,
      note_block: blocks.get(s.id) ?? null
    }));
  },
  /** Recorded time in whole seconds at `now`, paused spans excluded. */
  elapsedSeconds(now: number = Date.now()): number {
    if (_recordingStartedAt === null) return 0;
    const end = _recordingEndedAt ?? now;
    const pausedLive = _pausedSince !== null ? end - _pausedSince : 0;
    const ms = end - _recordingStartedAt - _pausedAccumMs - pausedLive;
    return Math.max(0, Math.floor(ms / 1000));
  },
  setAudioLevel(mic: number[], system: number[]) {
    _levelBands = { mic, system };
  },
  setRecording(v: boolean) {
    _isRecording = v;
    if (!v && _recordingStartedAt !== null && _recordingEndedAt === null) {
      _recordingEndedAt = Date.now();
    }
  },
  setPaused(v: boolean) {
    if (v === _isPaused) return;
    _isPaused = v;
    const now = Date.now();
    if (v) {
      _pausedSince = now;
    } else if (_pausedSince !== null) {
      _pausedAccumMs += now - _pausedSince;
      _pausedSince = null;
    }
  },
  /** Adopt the backend's accumulated segments wholesale: the focus-time
   * reconcile (`recording_status`) replays what a hidden window's dropped
   * events would have built. */
  replaceSegments(segments: TranscriptionSegment[]) {
    _segments = [...segments];
  },
  /** Shadow mode: the user asked the screen to stop saying a meeting is
   * being recorded. Only meaningful while one is: the indicators it
   * suppresses don't exist otherwise. */
  get shadowMode() {
    return _isRecording && _shadowMode;
  },
  setShadowMode(v: boolean) {
    _shadowMode = v;
  },
  /** Speaker labeling for the running recording (the transcript header's
   * toggle, or the backend's runaway guard turning it off). */
  get liveDiarization() {
    return _liveDiarization;
  },
  setLiveDiarization(on: boolean) {
    // A real flip is no longer the guard's doing; a no-op call (the
    // focus reconcile adopting a backend flag that never changed) must
    // not turn the guard's reason into a user choice.
    if (on !== _liveDiarization) _diarizationRunaway = false;
    _liveDiarization = on;
  },
  /** The runaway guard stood labeling down: more voices than a meeting
   * plausibly has, so the labels were not believable ([speakers.md]). */
  standDownDiarization() {
    _liveDiarization = false;
    _diarizationRunaway = true;
  },
  /** A new recording starts with no guard history; called at
   * recording-started, where adopting the setting may be a no-op call
   * that would otherwise preserve a stale reason. */
  clearDiarizationRunaway() {
    _diarizationRunaway = false;
  },
  /** True only while labeling is off because of the guard. */
  get diarizationRunaway() {
    return _diarizationRunaway;
  },
  /** Drop the labels from what is already on screen: the backend has done
   * the same to its accumulator, and a half-labeled transcript reads as
   * the app having lost track of who is talking. */
  stripSpeakers() {
    _segments = _segments.map((s) => ({ ...s, speaker: null, speaker_id: undefined }));
  },
  addSegment(s: TranscriptionSegment) {
    _segments = [..._segments, s];
    // Single-stream: a finalized segment always supersedes the live preview.
    // On a speaker change the backend emits Segment(prev) then Interim(new),
    // so this clear is immediately followed by the new interim being set.
    _interim = null;
    // Tail segments flushed after stop still belong to the pending meeting.
    if (_pendingMeeting && !_isRecording) {
      _pendingMeeting = {
        ..._pendingMeeting,
        segments: [..._pendingMeeting.segments, s]
      };
    }
  },

  get pendingMeeting() {
    return _pendingMeeting;
  },
  setPendingTitleHint(title: string) {
    _pendingTitleHint = title.trim() || null;
  },
  /** Snapshot the just-stopped recording as the pending meeting (call after
   * `setRecording(false)` so the clock is frozen). */
  beginPendingMeeting() {
    const snapshot = _recordingSnapshotProvider?.();
    const blocks = snapshot?.starBlocks ?? new Map<number, number>();
    _pendingMeeting = {
      title: _pendingTitleHint ?? copy.meetings.newMeetingTitle,
      durationSeconds: this.elapsedSeconds(),
      segments: [..._segments],
      stars: _stars.map((s) => ({
        seconds: s.seconds,
        note_block: blocks.get(s.id) ?? null
      })),
      userNotes: snapshot?.notes ?? '',
      audioPath: null,
      error: null
    };
    _pendingTitleHint = null;
  },
  setPendingAudioPath(path: string) {
    if (_pendingMeeting) _pendingMeeting = { ..._pendingMeeting, audioPath: path };
  },
  setPendingError(error: string) {
    if (_pendingMeeting) _pendingMeeting = { ..._pendingMeeting, error };
  },
  clearPendingMeeting() {
    _pendingMeeting = null;
  },
  /** Relabel a live speaker across the accumulated segments (the backend
   * accumulator is renamed by `rename_live_speaker`; this mirrors it). */
  renameSpeaker(from: string, to: string) {
    _segments = _segments.map((s) =>
      s.speaker === from ? { ...s, speaker: to } : s
    );
  },
  setInterim(s: InterimSegment | null) {
    _interim = s;
  },
  clearSegments() {
    _segments = [];
    _interim = null;
  },
  setProcessingStep(s: ProcessingStep) {
    _processingStep = s;
  },
  setError(e: string | null) {
    _error = e;
  },
  setFallbackNotice(n: string | null) {
    _fallbackNotice = n;
  },
  get fallbackNotice() {
    return _fallbackNotice;
  },
  setSilenceNotice(minutes: number | null) {
    _silenceNoticeMinutes = minutes;
  },
  get silenceNoticeMinutes() {
    return _silenceNoticeMinutes;
  },
  /** Called at both ends of an import. Only the end matters now: it clears
   * the fraction the processing view reads. */
  setImporting(v: boolean) {
    if (!v) _importFraction = null;
  },
  setImportFraction(f: number | null) {
    _importFraction = f;
  },
  resetToIdle() {
    _view = 'idle';
    _isRecording = false;
    _isPaused = false;
    _diarizationRunaway = false;
    _segments = [];
    _interim = null;
    _processingStep = null;
    _error = null;
    _silenceNoticeMinutes = null;
    _importFraction = null;
    _recordingStartedAt = null;
    _recordingEndedAt = null;
    _pausedAccumMs = 0;
    _pausedSince = null;
    _levelBands = { mic: [], system: [] };
    _stars = [];
  }
};
