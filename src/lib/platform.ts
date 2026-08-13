// Which OS the app is running on. Resolved once at module load (the
// platform can't change under a running process) and safe in non-browser
// contexts (vitest, node tooling), where it reports Windows, the shipping
// default. Components branch on this for chrome and wording differences;
// everything functional stays in the backend's per-platform code.

export type Platform = 'windows' | 'macos' | 'linux';

function detect(): Platform {
  if (typeof navigator === 'undefined') return 'windows';
  const ua = navigator.platform || navigator.userAgent || '';
  if (/mac/i.test(ua)) return 'macos';
  // Linux is tested after macOS and before the Windows default. webkit2gtk
  // reports "Linux x86_64" in `navigator.platform`; the X11 token catches
  // the older spellings.
  if (/linux|x11/i.test(ua)) return 'linux';
  return 'windows';
}

export const platform: Platform = detect();

/** Derived, so the many `isMac` call sites keep reading naturally. */
export const isMac: boolean = platform === 'macos';
export const isLinux: boolean = platform === 'linux';
