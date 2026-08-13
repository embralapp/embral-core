// The friendly name for a detected app's raw process label ("Zoom.exe",
// "ms-teams.exe", "com.google.Chrome.helper"). The token set mirrors the
// detector's fixed meeting-app grid; anything outside it degrades to a
// cleaned-up process name; raw exe text never reaches the user.
import { copy } from '$lib/copy';

type AppKey = keyof typeof copy.settings.meetings.autoStart.apps.names;

const TOKENS: { token: string; key: AppKey }[] = [
  { token: 'zoom', key: 'zoom' },
  { token: 'teams', key: 'teams' },
  // Chromium before Chrome only for readability; the two cannot collide,
  // since neither name is a substring of the other. Without its own entry a
  // Linux Chromium would fall through to the cleaned-stem path below.
  { token: 'chromium', key: 'chromium' },
  { token: 'chrome', key: 'chrome' },
  { token: 'edge', key: 'edge' },
  { token: 'safari', key: 'safari' },
  { token: 'firefox', key: 'firefox' },
  { token: 'slack', key: 'slack' },
  { token: 'discord', key: 'discord' },
  { token: 'webex', key: 'webex' }
];

export function displayAppName(raw: string): string {
  const cleaned = raw.toLowerCase().replace(/\.exe$/, '');
  const known = TOKENS.find((t) => cleaned.includes(t.token));
  if (known) return copy.settings.meetings.autoStart.apps.names[known.key];
  const stem = raw.replace(/\.exe$/i, '').trim();
  return stem ? stem.charAt(0).toUpperCase() + stem.slice(1) : raw;
}

/** One row of the source picker's system-audio list. */
export type AppGroup = { label: string; pids: number[] };

/**
 * Collapse a render-session list into one row per app.
 *
 * An app commonly holds several audio sessions at once: Zoom listed twice
 * is two pids, indistinguishable to the reader, and per-app capture climbs
 * both to the same process tree anyway. Two identical rows make a checkbox
 * that cannot be reasoned about, so the row carries every pid behind the
 * name and toggles them together. Sorted by name so the 3 s refresh cannot
 * reorder rows under the pointer.
 */
export function groupAudioApps(apps: { pid: number; name: string }[]): AppGroup[] {
  const byLabel = new Map<string, number[]>();
  for (const app of apps) {
    const label = displayAppName(app.name);
    const pids = byLabel.get(label);
    if (pids) {
      if (!pids.includes(app.pid)) pids.push(app.pid);
    } else {
      byLabel.set(label, [app.pid]);
    }
  }
  return [...byLabel]
    .map(([label, pids]) => ({ label, pids }))
    .sort((a, b) => a.label.localeCompare(b.label));
}
