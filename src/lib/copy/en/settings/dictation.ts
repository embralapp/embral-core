// Settings → Dictation. The dictation hotkey, its own transcription tree,
// output handling, and history retention — plus the "what cleanup does"
// dialog.

export const dictation = {
  start: {
    _group: 'Start dictating',
    hotkey: {
      label: 'Hotkey',
      aria: 'Dictation hotkey'
    }
  },

  transcription: {
    _group: 'Transcription',
    // This page's prompt into the shared TranscriptionBlock.
    providerLabel: 'Dictate with'
  },

  output: {
    _group: 'Output',
    copyClipboard: { label: 'Copy to clipboard' },
    autoPaste: { label: 'Auto-paste on completion' },
    cleanup: {
      label: 'Clean up with AI',
      infoAria: 'What cleanup does',
      // cloud / on_device use the shared provider labels; "off" is this
      // page's own. Shared with onboarding's dictation step.
      off: 'no cleanup'
    }
  },

  history: {
    _group: 'History',
    autoDelete: {
      label: 'Auto-delete history',
      sub: 'Use 0 to ignore a criterion'
    },
    deleteAfter: {
      label: 'Delete after',
      unit: 'days'
    },
    keepLast: {
      label: 'Keep only the last',
      unit: 'dictations'
    }
  },

  // The "what cleanup does" explainer dialog. The examples are English-shaped
  // on purpose (fillers, capitalization) — a translator rewrites them.
  cleanupDialog: {
    title: 'What cleanup does',
    description:
      'Send raw dictation to a language model for cleanup; your original speech will still be available in the Dictations tab',
    fillers: {
      heading: 'Punctuation and fillers',
      input: 'um so i think we should uh move the meeting to thursday',
      output: '→ I think we should move the meeting to Thursday'
    },
    formatting: {
      heading: 'Spoken formatting',
      input: "first item new line second item new paragraph and that's it",
      // Newlines render as line breaks via whitespace-pre-line; the blank line
      // is the "new paragraph".
      output: "→ First item\nSecond item\n\nAnd that's it"
    },
    instruction: {
      heading: 'Instruction mode',
      input:
        'Lead with an instruction: "make a bulleted list milk eggs flour"',
      // Separators are en-spaces (&ensp; in the original), preserved exactly.
      output: '→ • Milk • Eggs • Flour'
    }
  }
};
