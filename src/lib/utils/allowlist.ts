// The meeting-app allowlist as the settings grid sees it
// (docs/detection.md §Matching).
//
// The grid's contract is "this checkbox decides whether this app is
// detected", and for that to be true the checkbox has to reason about the
// allowlist the way the detector does (case-insensitively, bidirectional
// substring), not by exact string equality.
//
// Testing equality instead was a real bug: Windows' default list shipped
// both `teams` and `ms-teams` while the grid only had a `teams` checkbox, so
// unchecking Teams removed `teams`, left `ms-teams` behind, and detection
// kept firing against a box that showed unchecked. The rule below is the
// same one `match_identity` applies in `src-tauri/src/autodetect/state.rs`;
// it lives here too because this is the surface that has to agree with it.

/** Normalize an allowlist entry or a grid token the way the detector does. */
function normalize(value: string): string {
  return value.toLowerCase().replace(/\.exe$/, '');
}

/**
 * Whether an allowlist `entry` covers the same app as a grid `token`:
 * bidirectional substring, so `teams` covers `ms-teams` and `teams-for-linux`
 * alike. Neither `chrome` nor `chromium` contains the other, so those stay
 * separate, which is why each needs its own entry.
 */
export function entryCoversToken(entry: string, token: string): boolean {
  const e = normalize(entry);
  const t = normalize(token);
  if (!e || !t) return false;
  return e.includes(t) || t.includes(e);
}

/** Whether this app is currently detected; any covering entry counts. */
export function isAppEnabled(allowlist: string[], token: string): boolean {
  return allowlist.some((entry) => entryCoversToken(entry, token));
}

/**
 * Turn this app off: drop every entry that would still detect it, not just
 * the one spelled like the token. Leaving a covering entry behind is what
 * made the checkbox lie.
 */
export function disableApp(allowlist: string[], token: string): string[] {
  return allowlist.filter((entry) => !entryCoversToken(entry, token));
}

/** Turn this app on, without duplicating an entry that already covers it. */
export function enableApp(allowlist: string[], token: string): string[] {
  return isAppEnabled(allowlist, token) ? allowlist : [...allowlist, token];
}
