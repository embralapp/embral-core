// Strings shared across surfaces — kept here ONLY where a spec mandates one
// label set, or two components must not drift. Duplicate by default
// everywhere else (docs/copy.md): the same English word is often two
// different words in another language, so sharing a key is a claim that they
// must always translate together.

export const common = {
  // The cloud-or-device label set that every provider/engine selector uses,
  // in Settings and onboarding alike — mandated by docs/shell.md so the app
  // never calls the same choice by two names. "embral cloud" always leads.
  providers: {
    cloud: 'embral cloud',
    localModel: 'local model'
  },

  // Generic actions on shared components (the confirm dialog, the vendored
  // dialog's close control).
  cancel: 'Cancel',
  delete: 'Delete',
  close: 'Close',
  // Sending a banner away once it has been read (distinct from `close`,
  // which shuts a thing you opened).
  dismiss: 'Dismiss',

  // The draggable pane divider's accessible name, and what it becomes
  // once the pane it borders has been dragged shut.
  resizePanels: 'Resize panels',
  reopenPane: 'Reopen panel',

  // The full-size image overlay every notes surface shares (a click on
  // an image in the editor opens it; Esc or a click closes it).
  imageViewer: 'Image viewer',

  // The shared press-a-combo hotkey control (settings and onboarding).
  hotkey: {
    pressCombo: 'Press a combo...',
    notSet: 'Not set',
    clear: 'Clear',
    defaultAria: 'Hotkey'
  },

  // The Accessibility hint under dictation's auto-paste switch. Renders
  // only where the OS gates synthetic keystrokes (macOS) and only while
  // the permission is missing.
  axAccess: {
    needed: 'Pasting into other apps needs the Accessibility permission',
    allow: 'Allow',
    openSettings: 'Open System Settings'
  },

  // The shared microphone-access card (onboarding and settings). Renders
  // only where the OS actually gates the mic (macOS); on a denial the
  // button deep-links to the OS privacy pane.
  micAccess: {
    ask: 'embral needs the microphone to record meetings and take dictation',
    allow: 'Allow microphone',
    denied: 'Microphone access denied; enable in System Settings to record',
    openSettings: 'Open System Settings'
  }
};
