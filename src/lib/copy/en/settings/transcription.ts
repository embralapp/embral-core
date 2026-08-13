// Settings → Transcription. The model library (managed here; selection lives
// on the Meetings/Dictation pages) and the vocabulary boost, plus the copy
// for two shared controls: the provider/language block and the accuracy
// picker, both also used by the Dictation and Meetings pages.

export const transcription = {
  speechRecognition: 'Speech recognition',
  supportingModels: 'Supporting models',

  vocabulary: {
    _group: 'Vocabulary boost',
    custom: {
      label: 'Custom vocabulary',
      sub: 'Names and jargon to listen for more carefully',
      placeholder: 'Type a word and press Enter'
    },
    remove: (word: string) => `Remove ${word}`
  },

  // The shared TranscriptionBlock: provider, out-of-hours, language. The
  // provider labels themselves are common.providers.
  block: {
    powerPolicy: {
      label: 'Choose by power source',
      sub: 'Cloud transcription when on battery, local transcription when plugged in'
    },
    outOfHours: {
      label: 'When cloud hours run out',
      switchToDevice: 'Switch to this device',
      disable: 'Disable transcription'
    },
    language: {
      label: 'Language',
      english: 'English',
      all: 'All languages'
    }
  },

  // The shared SpeechModelPicker: the on-device accuracy tier.
  picker: {
    label: 'Accuracy',
    manageModels: 'Manage models',
    accuracyAria: 'Transcription accuracy',
    // Keyed by the fixed tier order (fast → balanced → accurate).
    tiers: {
      fast: 'Fast',
      balanced: 'Balanced',
      accurate: 'Accurate'
    },
    downloading: (pct: number) => `Downloading the model... ${pct}%`,
    needsDownload: 'This level needs a one-time model download.',
    download: (size: string) => `Download (~${size})`
  }
};
