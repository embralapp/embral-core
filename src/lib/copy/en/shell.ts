// The app shell: the sidebar rail, the titlebar, and the command palette.
// Visible on every screen.

export const shell = {
  /** The rail down the left edge. */
  sidebar: {
    // The record button is the rail's headline action. Its tooltip has three
    // states because the button means three different things.
    recordTip: {
      recording: 'Recording',
      ready: 'Start recording',
      notConfigured: 'Configure transcription in Settings first'
    },
    recordLabel: {
      recording: 'Recording...',
      // Shadow mode: the rail must not read as "a meeting is being
      // recorded" at a glance, so the row names the destination it opens
      // rather than the state it is in.
      shadow: 'Current meeting',
      idle: 'Record'
    },

    // Shared with the palette's "Go to" list, which must name the same places.
    nav: {
      meetings: 'Meetings',
      speakers: 'Profiles',
      dictation: 'Dictation'
    },

    settings: 'Settings',

    // Tooltip and aria-label both; the visible label stays "Collapse" because
    // it is only readable while the rail is open.
    collapseTip: {
      expanded: 'Collapse sidebar',
      collapsed: 'Expand sidebar'
    },
    collapseLabel: 'Collapse'
  },

  /** The window chrome across the top. */
  titleBar: {
    commandBar: {
      placeholder: 'Search or run a command...',
      shortcut: 'Ctrl+K'
    },
    minimize: 'Minimize',
    maximize: 'Maximize',
    close: 'Close to tray'
  },

  /** The Ctrl+K overlay. */
  palette: {
    dialogTitle: 'Search',
    dialogDescription: 'Search meetings, dictations, and commands',
    placeholder: 'Search meetings, dictations, and commands...',

    // Shown only once a search has finished: saying it mid-flight tells the
    // user their meeting isn't there a moment before it appears.
    empty: 'No results',
    searching: 'Searching...',

    groups: {
      meetings: 'Meetings',
      dictations: 'Dictations',
      actions: 'Actions',
      goTo: 'Go to'
    },

    actions: {
      stopRecording: 'Stop recording',
      startRecording: 'Start recording',
      startDictation: 'Start dictation',
      importRecording: 'Import a recording...',
      newProfile: 'New profile'
    },

    settings: 'Settings',
    /** A deep link to one settings page. */
    settingsPage: (page: string) => `Settings → ${page}`
  },

  /** The "a call is happening" banner across the top of the main area. The
   * label and the detail are two differently-weighted spans, so they stay two
   * complete strings rather than one. */
  detectionBanner: {
    label: 'Call in progress',
    detail: (app: string) => `(${app} is using your microphone)`,
    record: 'Record',
    dismiss: 'Dismiss'
  }
};
