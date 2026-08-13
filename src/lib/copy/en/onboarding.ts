// The first-run onboarding wizard: the shell nav, the persistent download
// footer, and the six steps (models → meetings → dictation → mcp →
// export → welcome, which closes the flow). The cloud edition opens with
// its own account and plans steps; their copy lives with the cloud copy.

import { plural } from '../plural';
import { locale } from './locale';

export const onboarding = {
  // The wizard's footer nav.
  shell: {
    back: 'Back',
    continue: 'Continue',
    finish: 'Finish'
  },

  // The persistent aggregate-download line.
  download: {
    active: (n: number, pct: number) =>
      `Downloading ${n} ${plural(locale, n, { one: 'model', other: 'models' })} (${pct}%)`,
    failed: (name: string) => `${name} failed to download`,
    retry: 'Retry'
  },

  // The small recommended-option marker on the segmented switches.
  segmented: {
    recommended: 'Recommended for your computer'
  },

  // The closing page: the feature grid as a "here's what you have" landing.
  welcome: {
    title: "You're set up",
    intro: 'Private notes & dictation integrated with the tools you already use:',
    // The 3×2 feature grid.
    features: {
      meetings: {
        title: 'Meetings',
        body: 'Record, transcribe, summarize, & query'
      },
      dictation: {
        title: 'Dictation',
        body: 'Speak to type anywhere, with cleanup'
      },
      profiles: {
        title: 'Profiles',
        body: 'Track speakers across your meetings'
      },
      markdown: {
        title: 'Markdown',
        body: 'Export notes to Obsidian & other tools'
      },
      assistants: {
        title: 'AI assistants',
        body: 'Let Claude and Codex search your notes'
      },
      private: {
        title: 'Private',
        body: 'Your notes & dictations stay on your computer'
      }
    }
  },

  models: {
    title: 'Set up local models',
    intro: "Based on your computer's specs, we recommend:",
    language: 'Language',
    accuracy: 'Accuracy',
    // Keyed by TranscriptionLanguage.
    languageOptions: {
      english: 'English',
      multilingual: 'Multilingual'
    },
    // Keyed by the accuracy tier order.
    tierOptions: {
      fast: 'Fast',
      balanced: 'Balanced',
      accurate: 'Accurate'
    },
    // The download-unit rows. Titles here are fallbacks for when the model
    // catalog hasn't loaded a display name yet; the "why" line is always
    // catalog copy.
    units: {
      asrTitle: 'Transcription model',
      asrWhy: 'Transcription',
      asrWhyMultilingual: 'Transcription (25 languages)',
      punctTitle: 'English punctuation',
      punctWhy: 'Punctuation for the transcript',
      summariesTitle: 'Local summarization engine',
      summariesWhy: 'Language model and engine',
      speakersTitle: 'Speaker identification',
      speakersWhy: 'Tells speakers apart',
      searchTitle: 'Semantic search',
      searchWhy: 'Search by meaning'
    },
    downloadNone: 'Download none',
    downloadAll: (size: string) => `Download all (${size})`,
    downloadSelected: (size: string) => `Download selected (${size})`,
    ready: "Everything's ready",
    downloadingBackground: 'Downloading in background; you can keep going',
    lowSpace: 'Your drive is low on space; downloads may not fit',
    checking: 'Checking this computer...'
  },

  meetings: {
    title: 'Meetings',
    intro: 'All options can be adjusted later in Settings',
    autoStart: 'When a call is detected, start recording...',
    // Keyed by AutoStartPolicy (onboarding offers three of the four).
    autoStartOptions: {
      always: 'Automatically',
      prompt: 'After asking',
      manual: 'Never'
    },
    summarize: 'Summarize meetings',
    engine: 'Write summaries with',
    llmNudge: 'On-device summaries need a language model',
    download: 'Download',
    hotkey: 'Meeting hotkey',
    hotkeySub: 'Start or stop from anywhere',
    hotkeyAria: 'Meeting hotkey'
  },

  dictation: {
    title: 'Dictation',
    intro: 'Speech to text in any app, in realtime',
    hotkey: 'Dictation hotkey',
    hotkeyAria: 'Dictation hotkey',
    cleanup: 'Clean up with AI',
    cleanupSub: 'Remove filler words and fix phrasing',
    // cloud / on_device use the shared provider labels; "off" is its own.
    cleanupOff: 'no cleanup',
    copyClipboard: 'Copy to clipboard',
    autoPaste: 'Paste into the active app'
  },

  mcp: {
    title: 'Connect your AI assistants',
    intro: 'Search your meeting notes; other MCP clients can be added in settings',
    looking: 'Looking for installed clients...',
    none: 'No supported AI clients found on this machine; set this up later in Settings → MCP',
    clients: {
      claudeDesktop: 'Claude Desktop',
      claudeCode: 'Claude Code',
      codex: 'Codex'
    }
  },

  export: {
    title: 'Connect to your knowledge base',
    intro: 'Export meeting notes as markdown to a local folder, like an Obsidian vault',
    exportOnEnd: 'Export notes when a recording ends',
    folder: 'Folder',
    noFolder: 'No folder chosen yet',
    browse: 'Browse...',
    includeSummary: 'Include AI summary',
    includeNotes: 'Include your typed notes',
    includeTranscript: 'Include transcript',
    filenameNote: 'Adjust filename format in settings'
  }
};
