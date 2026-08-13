// Settings → About. App identity, the updater row, diagnostics, credits, and
// the reset dialog.

export const about = {
  tagline: 'Privacy forward meeting transcripts and dictation',
  version: (version: string) => `version ${version}`,

  updates: {
    _group: 'Updates',
    ready: (version: string) => `Version ${version} is ready`,
    upToDate: 'Up to date',
    // The reason is a backend string; this keeps the English tail attached to
    // it until Phase 3 turns backend errors into codes (docs/copy.md).
    blocked: (reason: string) => `${reason} - finish, then update.`,
    installing: 'Installing...',
    restartAndUpdate: 'Restart and update',
    // Shown only where installing the update runs the package manager, so
    // the authentication dialog isn't a surprise (Linux .deb/.rpm; an
    // AppImage swaps itself in place and asks nothing).
    needsPassword: 'Your system will ask for your password to install this',
    checkForUpdates: 'Check for updates'
  },

  diagnostics: {
    _group: 'Diagnostics',
    logs: {
      label: 'Logs',
      sub: 'Attach the latest file when reporting a problem',
      button: 'Open logs folder'
    },
    notesFolder: {
      label: 'Storage folder',
      button: 'Open storage folder'
    },
    reset: {
      label: 'Reset app',
      button: 'Reset...'
    }
  },

  credits: {
    _group: 'On the shoulders of',
    // What each library does. Names, licenses, and URLs are data and stay in
    // the component.
    what: {
      sherpa: 'speech runtime',
      parakeet: 'speech recognition & speaker embeddings',
      zipformer: 'speech recognition',
      pyannote: 'speaker diarization',
      titanet: 'voice recognition',
      silero: 'voice activity detection',
      qwen: 'summaries & dictation cleanup',
      llama: 'on-device LLM runtime'
    }
  },

  resetDialog: {
    title: 'Reset embral',
    description: 'Selected items will be irreversibly reset',
    // The erasable scopes, keyed by the ScopeKey union.
    scopes: {
      settings: 'Settings',
      meetings: 'Meetings',
      profiles: 'Profiles',
      dictations: 'Dictations',
      models: 'Models'
    },
    cancel: 'Cancel',
    resetting: 'Resetting...',
    reset: 'Reset'
  }
};
