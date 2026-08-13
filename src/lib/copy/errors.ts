// Maps a caught backend failure to a catalog sentence. Commands reject with a
// serialized `AppError` (crates/embral-types/src/error.rs) — a `{ code, … }`
// object; the error events carry the same shape. Anything else (an
// un-converted command that still returns a string, a thrown JS Error) falls
// through to its own text.
//
// `AppErrorCode` mirrors the Rust `code` tags. It is the frontend half of the
// contract: `en/errors.ts` is `satisfies Record<AppErrorCode, …>`, so a code
// added here without its copy — or copy without its code — fails
// `npm run check`. The Rust `code()` unit test pins the other half.

import { copy } from '$lib/copy';

export type AppErrorCode =
  | 'notConfigured'
  | 'busyDictating'
  | 'noActiveRecording'
  | 'alreadyRecording'
  | 'cantImportWhileRecording'
  | 'importAlreadyRunning'
  | 'needsLocalModel'
  | 'fileNotFound'
  | 'alreadyDownloading'
  | 'stopRecordingBeforeReset'
  | 'stopDictatingBeforeReset'
  | 'cantDictateWhileRecording'
  | 'dictationAlreadyRunning'
  | 'dictationModelMissing'
  | 'cloudSignInRequired'
  | 'noDictationRunning'
  | 'recordingInProgress'
  | 'dictationInProgress'
  | 'importInProgress'
  | 'titleEmpty'
  | 'speakerNameEmpty'
  | 'suggestionNotPending'
  | 'noStructuredTranscript'
  | 'meetingNotFound'
  | 'encodeFailed'
  | 'importFailed'
  | 'dictationStartFailed'
  | 'cloudUnreachable'
  | 'cloudSignedOut'
  | 'webhookTestFailed'
  | 'internal';

type AppErrorPayload = { code: AppErrorCode } & Record<string, unknown>;

function isAppError(e: unknown): e is AppErrorPayload {
  return (
    typeof e === 'object' &&
    e !== null &&
    typeof (e as { code?: unknown }).code === 'string'
  );
}

const str = (v: unknown): string => (typeof v === 'string' ? v : String(v ?? ''));

/** A user-facing sentence for any caught error or error-event payload. */
export function errorMessage(e: unknown): string {
  if (isAppError(e)) {
    const t = copy.errors;
    switch (e.code) {
      case 'fileNotFound':
        return t.fileNotFound(str(e.path));
      case 'meetingNotFound':
        return t.meetingNotFound(str(e.id));
      case 'dictationModelMissing':
        return t.dictationModelMissing(str(e.modelId ?? e.model_id));
      case 'encodeFailed':
        return t.encodeFailed(str(e.detail));
      case 'importFailed':
        return t.importFailed(str(e.detail));
      case 'dictationStartFailed':
        return t.dictationStartFailed(str(e.detail));
      case 'webhookTestFailed':
        return t.webhookTestFailed(str(e.detail));
      case 'internal':
        return t.internal(str(e.detail));
      default: {
        // The no-arg codes are plain strings in the catalog.
        const msg = (t as Record<string, unknown>)[e.code];
        if (typeof msg === 'string') return msg;
        // A code the frontend doesn't know yet (Rust ahead of the union):
        // prefer any detail it carried over an opaque code.
        return str(e.detail) || t.internal('');
      }
    }
  }
  if (typeof e === 'string') return e;
  if (e instanceof Error) return e.message;
  // A shape nobody typed (a foreign rejection, a bare object): show its
  // contents, never "[object Object]".
  try {
    return copy.errors.internal(JSON.stringify(e) ?? String(e));
  } catch {
    return copy.errors.internal(String(e));
  }
}
