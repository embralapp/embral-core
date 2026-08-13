// Settings → General. Appearance, storage folder, audio devices,
// notifications. (Privacy/telemetry is a cloud-only row and lives with the
// cloud copy.)

export const general = {
  appearance: {
    _group: 'Appearance',
    theme: {
      label: 'Color scheme',
      // Keyed by the Theme union.
      options: {
        system: 'System',
        light: 'Light',
        dark: 'Dark'
      }
    },
    indicator: {
      label: 'Recording indicator color',
      // The default: follow the Windows accent color.
      accent: 'Windows accent',
      // The preset swatches, keyed by the component's fixed palette.
      colors: {
        red: 'Red',
        orange: 'Orange',
        green: 'Green',
        blue: 'Blue',
        purple: 'Purple',
        pink: 'Pink'
      }
    }
  },

  storage: {
    _group: 'Storage',
    folder: { label: 'Storage folder' },
    browse: 'Browse...'
  },

  audio: {
    _group: 'Audio',
    // Shown for the "system default" device and in each device list.
    systemDefault: 'System default',
    mic: { label: 'Microphone' },
    systemAudio: {
      label: 'System audio',
      sub: 'Lets calls on headphones still record everyone'
    },
    refresh: {
      label: 'Refresh devices',
      button: 'Refresh'
    }
  },

  notifications: {
    _group: 'Notifications',
    summaryReady: { label: 'Summary ready' },
    recordingStarted: { label: 'Recording started' },
    callDetected: {
      label: 'Call detected',
      sub: 'Only when embral is set to ask before recording'
    },
    updateReady: { label: 'Update ready' }
  }
};
