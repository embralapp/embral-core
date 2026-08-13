// Shared model-management copy — the ModelCard footer, used by both the
// Transcription and Synthesis pages so the library reads as one surface.

export const models = {
  card: {
    download: 'Download',
    redownload: 'Re-download',
    remove: 'Remove',
    downloading: (pct: number) => `Downloading... ${pct}%`
  }
};
