// Settings → Markdown. Auto-export to a notes folder, what to include, and
// the filename template.

export const markdown = {
  autoExport: {
    _group: 'Auto-export',
    enabled: {
      label: 'Export notes when a recording ends',
      sub: 'Saves a markdown copy into a folder of your choice, like an Obsidian vault'
    },
    folder: {
      label: 'Export folder',
      placeholder: 'Path to your vault or notes folder'
    },
    browse: 'Browse...'
  },

  include: {
    _group: 'Include',
    summary: { label: 'AI summary' },
    notes: { label: 'Your notes' },
    transcript: { label: 'Transcript' }
  },

  filename: {
    _group: 'Filename',
    template: { label: 'Filename template' },
    preview: 'Preview:',
    // The meaning shown beside each filename token. The token codes
    // themselves ({date}, {title}, …) are parsed by Rust and stay in the
    // component.
    tokens: {
      date: 'YYYY-MM-DD',
      time: 'HH-MM',
      year: 'YYYY',
      month: 'MM',
      day: 'DD',
      hour: 'HH',
      minute: 'MM',
      title: 'meeting title'
    },
    metadata: {
      label: 'Metadata format',
      // Keyed by the ExportMetadataFormat union.
      options: {
        frontmatter: 'YAML frontmatter',
        inline: 'Inline'
      }
    }
  }
};
