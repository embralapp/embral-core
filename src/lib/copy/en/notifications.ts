// Event-driven status messages fired from events.ts and the updater store:
// OS notifications shown while the window is hidden, and the in-app notices
// that ride the recording banner or the error slot.
//
// These have no on-screen review path: an OS notification fires only when a
// recording starts with the window hidden, the updater message only on a real
// update. The type check and the corpus test are their safety net.
//
// The `message` arguments are the mapped sentence from a backend `AppError`
// (via errorMessage in ../errors.ts) interpolated into a frame, no longer a
// raw backend string (docs/copy.md).

import { plural } from '../plural';
import { locale } from './locale';

export const notifications = {
  // Notices: one line each. The title is the whole message, with the
  // answers beside it ([shell.md] §Notices). The notice row is a fixed
  // 360px; every label here is budgeted by notice-fit.test.ts, so a
  // longer word has to pass that test, not just read well.
  os: {
    recordingStarted: { title: 'Recording started' },
    switchedToLocal: { title: 'Switched to local transcription' },
    notesReady: { title: 'Meeting notes ready' },
    callDetected: {
      title: (app: string) => `${app} call detected`,
      starting: 'Starting recording...'
    },
    // The notice's answers run the same commands as the in-app banner's
    // "Keep recording" / "Stop recording"; the words are shorter because
    // this row also has to hold the title and the countdown.
    stillRecording: { title: 'Still recording?', keep: 'Continue', stop: 'Stop' },
    startFailed: { title: "Couldn't start recording" },
    updateReady: { title: 'Update ready' },
    // Fixed text on purpose: the meeting is one click away via the
    // notice's target, and a dynamic title would fight the fit budget.
    webhookFailed: { title: 'Webhook delivery failed' },
    // The countdown chip beside a notice's title: plain text on purpose,
    // so it cannot be mistaken for the logo's ring.
    countdown: (secs: number) => `${secs}s`,
    countdownAria: (secs: number) =>
      plural(locale, secs, {
        one: 'Stops in 1 second',
        other: `Stops in ${secs} seconds`
      })
  },

  // In-app notices shown in the recording banner or the error slot.
  notices: {
    switchedToLocal: (message: string) =>
      `Switched to local transcription (${message})`,
    transcriptionStopped: (message: string) =>
      `Live transcription stopped (${message})`,
    transcriptionOff: (message: string) =>
      `Transcription disabled (${message}); audio and notes will be saved`
  }
};
