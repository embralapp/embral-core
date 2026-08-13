import { describe, expect, it } from 'vitest';
import { disableApp, enableApp, entryCoversToken, isAppEnabled } from './allowlist';

describe('entryCoversToken', () => {
  it('matches bidirectionally, case-insensitively, ignoring .exe', () => {
    expect(entryCoversToken('ms-teams', 'teams')).toBe(true);
    expect(entryCoversToken('teams', 'ms-teams')).toBe(true);
    expect(entryCoversToken('Zoom.exe', 'zoom')).toBe(true);
    expect(entryCoversToken('teams-for-linux', 'teams')).toBe(true);
  });

  it('keeps Chrome and Chromium apart', () => {
    // The one brand pair where substring matching does not collapse them:
    // neither name contains the other, which is why each needs its own entry.
    expect(entryCoversToken('chromium', 'chrome')).toBe(false);
    expect(entryCoversToken('chrome', 'chromium')).toBe(false);
    expect(entryCoversToken('chromium-browser', 'chromium')).toBe(true);
    expect(entryCoversToken('google-chrome', 'chrome')).toBe(true);
  });

  it('never lets an empty value match everything', () => {
    expect(entryCoversToken('', 'zoom')).toBe(false);
    expect(entryCoversToken('zoom', '')).toBe(false);
    expect(entryCoversToken('.exe', 'zoom')).toBe(false);
  });
});

describe('the Windows ms-teams regression', () => {
  // Windows shipped `["zoom", "ms-teams", "teams", …]` as its default while
  // the grid only ever had a `teams` checkbox. With exact-equality checks,
  // unchecking Teams removed `teams`, left `ms-teams`, and detection kept
  // firing against a box that showed unchecked, so Teams detection could
  // not be turned off at all.
  const shipped = ['zoom', 'ms-teams', 'teams', 'chrome', 'msedge'];

  it('shows Teams as enabled when only the legacy entry is present', () => {
    expect(isAppEnabled(['zoom', 'ms-teams'], 'teams')).toBe(true);
  });

  it('unchecking Teams removes every entry that would still detect it', () => {
    const after = disableApp(shipped, 'teams');
    expect(after).not.toContain('teams');
    expect(after).not.toContain('ms-teams');
    // And says so when asked again: the box no longer lies.
    expect(isAppEnabled(after, 'teams')).toBe(false);
    // Everything else is untouched.
    expect(after).toEqual(['zoom', 'chrome', 'msedge']);
  });

  it('leaves Chrome alone when Chromium is switched off', () => {
    const after = disableApp(['chrome', 'chromium'], 'chromium');
    expect(after).toEqual(['chrome']);
    expect(isAppEnabled(after, 'chrome')).toBe(true);
  });
});

describe('enableApp', () => {
  it('adds the token when nothing covers the app', () => {
    expect(enableApp(['zoom'], 'teams')).toEqual(['zoom', 'teams']);
  });

  it('does not duplicate an entry that already covers the app', () => {
    // Re-checking a box whose app is covered by a legacy entry must not
    // append a second way to say the same thing.
    expect(enableApp(['zoom', 'ms-teams'], 'teams')).toEqual(['zoom', 'ms-teams']);
  });

  it('round-trips: off then on leaves the app enabled', () => {
    const off = disableApp(['zoom', 'ms-teams', 'teams'], 'teams');
    const on = enableApp(off, 'teams');
    expect(isAppEnabled(on, 'teams')).toBe(true);
    expect(on).toEqual(['zoom', 'teams']);
  });
});
