// The meetings surface: the library list, a meeting's detail (summary /
// notes / transcript), the still-processing pending view, import progress,
// the live recording controls and transcript, and the audio player. Reads in
// the order a meeting moves through the app: list → detail → pending →
// recording.

import { plural } from '../plural';
import type { Part } from '../types';
import { locale } from './locale';

export const meetings = {
  // The pending meeting's title before a real one is set (app-state store).
  newMeetingTitle: 'New meeting',

  // The date headers the list and the profiles list group rows under. Both
  // produced by and compared against these in utils/meetingFormat.ts, so the
  // labels must round-trip exactly. Weekday and month headers come from Intl
  // (system locale) and are not catalog copy.
  dateGroups: {
    today: 'Today',
    yesterday: 'Yesterday',
    lastWeek: 'Last week',
    lastMonth: 'Last month',
    earlier: 'Earlier'
  },

  // The native file-open dialog for importing a recording.
  importDialog: {
    title: 'Import a recording',
    filterName: 'Audio'
  },

  // The library list down the left.
  list: {
    loading: 'Loading meetings...',
    empty: 'No meetings yet...',
    finishingUp: 'Finishing up...',
    import: 'Import a recording',
    // The right-click menu on a row; it acts on the whole selection.
    menuDelete: (n: number) =>
      plural(locale, n, { one: 'Delete', other: `Delete ${n} meetings` })
  },

  // The pane shown when several meetings are selected at once.
  multiSelect: {
    selected: (n: number) => `${n} meetings selected`,
    delete: (n: number) => `Delete ${n} meetings`,
    hint: 'Or press Delete'
  },

  // The delete confirmation, shared by the multi-select pane and a single
  // meeting's detail (which always passes 1).
  deleteConfirm: {
    title: (n: number) =>
      plural(locale, n, {
        one: 'Delete meeting?',
        other: `Delete ${n} meetings?`
      }),
    body: (n: number) =>
      plural(locale, n, {
        one: 'Deleting this meeting will permanently delete its notes, transcript, and audio',
        other: `Deleting these ${n} meetings will permanently delete their notes, transcripts, and audio`
      }),
    confirm: (n: number) =>
      plural(locale, n, { one: 'Delete', other: `Delete ${n}` })
  },

  // A single meeting's detail view.
  detail: {
    back: 'Meetings',
    backAria: 'Back to meetings',
    titleAria: 'Meeting title',
    deleteMeeting: 'Delete meeting',
    // Keyed by DetailTab.
    tabs: {
      summary: 'Summary',
      notes: 'Notes',
      transcript: 'Transcript'
    },
    status: {
      saving: 'Saving...',
      saved: 'Saved',
      failed: "Couldn't save"
    },
    titleRequired: 'Title required',
    selectPrompt: 'Select a meeting to view its notes',
    loading: 'Loading meeting...',
    summaryPlaceholder: 'No summary saved yet',
    transcriptPlaceholder: 'No transcript saved yet',
    // The H1 written into the saved transcript document.
    transcriptHeading: (title: string) => `${title} Transcript`
  },

  // The pending view — a meeting whose summary is still generating.
  pending: {
    justNow: 'Just now',
    finalizing: 'Finalizing summary',
    // Keyed by PendingTab.
    tabs: {
      notes: 'Notes',
      transcript: 'Transcript'
    },
    noSpeech: 'No speech was transcribed'
  },

  // The import-processing screen.
  processing: {
    importing: 'Importing a recording...',
    // Keyed by the step id order.
    steps: {
      transcribing: 'Transcribing file',
      finalizing: 'Finalizing transcript',
      generating: 'Generating notes'
    },
    percent: (pct: number) => `— ${pct}%`,
    backToMeetings: 'Back to meetings',
    continueBackground: 'Continue in background',
    openNotesFolder: 'Open notes folder'
  },

  // The editable transcript (saved meetings). The name-suggestion sentence
  // is interrupted by styled spans and is extracted in S8.
  transcript: {
    playFromHere: 'Play from here',
    // The name-suggestion sentence, interrupted by the two styled name runs
    // (CopyParts): "{label} looks like {name} (from your notes)".
    suggestion: (label: string, name: string): Part[] => [
      { slot: 'strong', text: label },
      { slot: 'muted', text: ' looks like ' },
      { slot: 'strong', text: name },
      { slot: 'muted', text: ' (from your notes)' }
    ],
    suggestionApply: 'Apply',
    suggestionDismiss: 'Dismiss',
    renameSpeaker:
      "Rename this speaker (type another speaker's name to merge; right-click to remove from the transcript)",
    playFrom: (time: string) => `Play from ${time}`,
    changeSpeaker: "Change this sentence's speaker",
    changeTurnSpeaker: "Change this turn's speaker",
    assignSpeaker: 'Assign a speaker',
    splitSegment: 'Split a sentence',
    deleteSegment: 'Delete sentence',
    splitHint: 'Click inside a sentence where the split should happen',
    jumpToCurrent: 'Jump to current'
  },

  // The live transcript during recording.
  live: {
    nameSpeaker: 'Name this speaker',
    // The speaker-labeling toggle at the right of the transcript header.
    // Named for what the click does, not for the current state.
    diarizationOff: 'Stop labeling speakers',
    diarizationOn: 'Label speakers',
    // The note in the speaker row while labeling is off. Two of them: the
    // user's own choice is a fact they already know, while the guard
    // standing labeling down is news, and reads as a bug if unexplained.
    diarizationOffNote: 'Speaker labeling disabled',
    diarizationRunawayNote: 'Too many speakers to label',
    // Raised when the app stands labeling down on its own, having found
    // more voices than a meeting plausibly has.
    tooManySpeakers: (count: number) =>
      `Too many speakers to label`,
    listening: 'Listening...',
    jumpToLatest: 'Jump to latest'
  },

  // The silence check-in banner during a recording. The detail claims
  // only what the app can know: words it transcribed (or notes typed),
  // not whether anyone spoke.
  silence: {
    label: 'Still recording?',
    detail: (minutes: number) =>
      plural(locale, minutes, {
        one: 'No words transcribed for 1 minute.',
        other: `No words transcribed for ${minutes} minutes.`
      }),
    keep: 'Keep recording',
    stop: 'Stop recording'
  },

  // The source picker in the recording header: what this meeting is
  // capturing, changeable mid-recording.
  sources: {
    aria: 'Sources',
    systemAudio: 'System audio',
    microphones: 'Microphones',
    nothingPlaying: 'Nothing is playing audio right now',
    primary: 'main'
  },

  // The recording header controls.
  recording: {
    star: 'Star this moment (Ctrl+S)',
    starAria: 'Star this moment',
    titlePlaceholder: 'Meeting title',
    titleAria: 'Meeting title',
    resume: 'Resume',
    pause: 'Pause',
    resumeAria: 'Resume recording',
    pauseAria: 'Pause recording',
    // The quiet-the-indicators toggle. One name across both states: a
    // toggle button's accessible name should hold still while
    // `aria-pressed` carries the state, and the rail reads as a set of
    // named things rather than a set of instructions.
    shadowMode: 'Shadow mode',
    // The toggle's hover tip while shadow is on. The one sanctioned
    // instructional tooltip ([shell.md] §Recording): pause and stop are
    // hidden, and the way out has to be discoverable at the moment of
    // need without showing on the shared screen.
    shadowStopHint: (shortcut: string) =>
      `Shadow mode`,
    stop: 'Stop recording'
  },

  // The audio player at the foot of a meeting.
  player: {
    pause: 'Pause',
    play: 'Play',
    pauseAria: 'Pause audio',
    playAria: 'Play audio',
    position: 'Audio position',
    playFrom: (time: string) => `Play from ${time}`,
    noAudio: 'Audio was not retained for this meeting',
    // Frontend-authored playback failures shown under the player.
    errors: {
      couldNotPlay: 'Could not play audio',
      aborted: 'Audio load was aborted',
      network: 'Audio file could not be loaded',
      decode: 'Audio file could not be decoded',
      unsupported: 'Audio source is not supported'
    }
  },

  // The notes editor and its read-only view.
  notes: {
    placeholder: "Take notes or paste images to be included in the summary",
    emptyView: 'No notes were taken during this meeting'
  }
};
