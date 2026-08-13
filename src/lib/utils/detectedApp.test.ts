import { describe, expect, it } from 'vitest';
import { displayAppName, groupAudioApps } from './detectedApp';

describe('displayAppName', () => {
  it('maps the known meeting apps to their catalog names', () => {
    expect(displayAppName('Zoom.exe')).toBe('Zoom');
    expect(displayAppName('ms-teams.exe')).toBe('Teams');
    expect(displayAppName('msedge.exe')).toBe('Edge');
    expect(displayAppName('com.google.Chrome.helper')).toBe('Chrome');
    expect(displayAppName('webex.exe')).toBe('Webex');
  });

  it('maps Linux identities, which are bare process names', () => {
    // No `.exe` to strip here; pulse reports the binary name directly.
    expect(displayAppName('zoom')).toBe('Zoom');
    // The Linux Teams client's binary; `teams` catches it by substring.
    expect(displayAppName('teams-for-linux')).toBe('Teams');
    // Edge is `msedge` on Linux as on Windows, not macOS's bare `edge`.
    expect(displayAppName('msedge')).toBe('Edge');
    // Chromium is its own name, not "Chrome": neither string contains the
    // other, so without its own token this would fall through to the
    // cleaned-stem path and read as a raw process name.
    expect(displayAppName('chromium')).toBe('Chromium');
    expect(displayAppName('chromium-browser')).toBe('Chromium');
    // And Chrome still resolves to Chrome, not Chromium.
    expect(displayAppName('google-chrome')).toBe('Chrome');
    expect(displayAppName('chrome')).toBe('Chrome');
  });

  it('cleans up anything outside the known set', () => {
    // Raw exe text must never reach the user, even for unknown apps
    // (the Always policy detects any mic holder).
    expect(displayAppName('obscureapp.exe')).toBe('Obscureapp');
    expect(displayAppName('')).toBe('');
  });
});

describe('groupAudioApps', () => {
  it('collapses one app holding several sessions into a single row', () => {
    // The field case: Zoom listed twice, two pids, nothing on screen to
    // tell them apart, so the checkbox meant nothing.
    const groups = groupAudioApps([
      { pid: 100, name: 'Zoom.exe' },
      { pid: 200, name: 'Zoom.exe' }
    ]);
    expect(groups).toEqual([{ label: 'Zoom', pids: [100, 200] }]);
  });

  it('groups by the displayed name, not the raw process name', () => {
    // Chrome's audio and its helper are one app to the reader.
    const groups = groupAudioApps([
      { pid: 1, name: 'chrome.exe' },
      { pid: 2, name: 'com.google.Chrome.helper' }
    ]);
    expect(groups).toEqual([{ label: 'Chrome', pids: [1, 2] }]);
  });

  it('sorts by name so a refresh cannot reorder rows under the pointer', () => {
    // The list refreshes every 3 s while open; the scan's own order is
    // not guaranteed stable.
    const groups = groupAudioApps([
      { pid: 3, name: 'Zoom.exe' },
      { pid: 1, name: 'Discord.exe' },
      { pid: 2, name: 'Slack.exe' }
    ]);
    expect(groups.map((g) => g.label)).toEqual(['Discord', 'Slack', 'Zoom']);
  });

  it('ignores a repeated pid and survives an empty list', () => {
    expect(groupAudioApps([{ pid: 5, name: 'Zoom.exe' }, { pid: 5, name: 'Zoom.exe' }])).toEqual([
      { label: 'Zoom', pids: [5] }
    ]);
    expect(groupAudioApps([])).toEqual([]);
  });
});
