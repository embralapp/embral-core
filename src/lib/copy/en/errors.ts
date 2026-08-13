// Backend (Rust) error copy. Each key is an `AppError` code
// (crates/embral-types/src/error.rs); a caught command rejection or error
// event is mapped to one of these by errorMessage() in src/lib/copy/errors.ts.
//
// Strings are verbatim from the Rust Display text; wording belongs to the
// owner's editing pass (260720-copy-catalog.md, Phase 4), not this extraction. Functions carry the structured data the variant holds (a
// path, an id, a technical detail). `internal` is the catch-all for every
// incidental DB/IO/vendor failure: it shows the backend detail as-is, exactly
// as the raw error string did before typed errors.

import type { AppErrorCode } from '../errors';

export const errors = {
  notConfigured:
    "Transcription isn't set up yet - download a speech model or sign in from Settings",
  busyDictating: "Can't record during an active dictation",
  noActiveRecording: 'No active recording',
  alreadyRecording: 'A recording is already running',
  cantImportWhileRecording: "Can't import while a recording is in progress",
  importAlreadyRunning: 'An import is already in progress',
  needsLocalModel:
    'Local model required for importing; download one in Settings → Transcription',
  fileNotFound: (path: string) => `File not found: ${path}`,
  alreadyDownloading: 'This model is already downloading',
  stopRecordingBeforeReset: "Can't reset during active recording",
  stopDictatingBeforeReset: "Can't reset during active dictation",
  cantDictateWhileRecording: "Can't dictate during active recording",
  dictationAlreadyRunning: 'Dictation is already running',
  dictationModelMissing: (modelId: string) =>
    `Dictation speech model is missing (${modelId}); check Settings → Transcription`,
  cloudSignInRequired: 'Sign in on the Account page to dictate with embral cloud',
  noDictationRunning: 'No dictation running',
  recordingInProgress: 'A recording is in progress',
  dictationInProgress: 'A dictation is in progress',
  importInProgress: 'An import is in progress',
  titleEmpty: 'Meeting title cannot be empty',
  speakerNameEmpty: 'Speaker name cannot be empty',
  suggestionNotPending: 'That suggestion is no longer pending',
  noStructuredTranscript: 'This meeting has no structured transcript to edit',
  meetingNotFound: (id: string) => `Meeting ${id} not found`,
  encodeFailed: (detail: string) => `Encode failed: ${detail}`,
  importFailed: (detail: string) => `Import failed: ${detail}`,
  dictationStartFailed: (detail: string) => `Dictation couldn't start: ${detail}`,
  cloudUnreachable: 'embral cloud is unreachable',
  cloudSignedOut: 'no embral cloud account is signed in',
  webhookTestFailed: (detail: string) => `Test delivery failed: ${detail}`,
  // The generic tail: the backend detail, shown as-is (never a stray code).
  internal: (detail: string) => detail
} satisfies Record<AppErrorCode, string | ((arg: string) => string)>;
