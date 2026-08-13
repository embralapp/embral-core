// The settings navigation: group headings and page names.
//
// Single source for two consumers that must never drift: SettingsLayout's
// rail (which also uses the page name as the heading of the page it opens) and
// the command palette's "Settings → …" deep links. The order of the pages
// themselves is information architecture and stays in SettingsLayout
// (docs/shell.md).

export const nav = {
  groups: {
    application: 'Application',
    models: 'Models',
    integrations: 'Integrations'
  },

  /** Keyed by SettingsLayout's SectionId. `account` exists in cloud builds
   * only, but its name lives here with the rest: the build gates the page,
   * not the word. */
  sections: {
    general: 'General',
    meetings: 'Meetings',
    dictation: 'Dictation',
    account: 'Account',
    about: 'About',
    transcription: 'Transcription',
    synthesis: 'Synthesis',
    markdown: 'Markdown',
    webhooks: 'Webhooks',
    mcp: 'MCP'
  },

  /** Shown while the config draft is still loading. */
  loading: 'Loading...'
};
