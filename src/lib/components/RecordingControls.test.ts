// @vitest-environment happy-dom
//
// The repo's first component-render test: shadow mode's whole point is
// what is NOT on screen, which only a mounted DOM can assert. The tauri
// bridge is mocked (no webview here); the harness supplies the tooltip
// provider +layout.svelte provides in the app.

import { beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null)
}));

import Harness from './RecordingControlsHarness.svelte';
import { appState } from '$lib/stores/app-state.svelte';
import { copy } from '$lib/copy';

const t = copy.meetings.recording;

function ariaLabels(): Set<string> {
  return new Set(
    Array.from(document.querySelectorAll('button[aria-label]'), (b) =>
      b.getAttribute('aria-label')
    ).filter((l): l is string => l !== null)
  );
}

describe('shadow mode hides the recording tells', () => {
  beforeEach(() => {
    appState.resetToIdle();
    appState.setShadowMode(false);
    document.body.innerHTML = '';
  });

  it('pause and stop leave with shadow mode and come back with it', () => {
    appState.setRecording(true);
    const app = mount(Harness, { target: document.body });
    flushSync();

    expect(ariaLabels().has(t.pauseAria)).toBe(true);
    expect(ariaLabels().has(t.stop)).toBe(true);
    expect(ariaLabels().has(t.shadowMode)).toBe(true);

    appState.setShadowMode(true);
    flushSync();
    // A red stop square beside a pause button reads as recording
    // controls on a shared screen — the tell shadow exists to prevent.
    expect(ariaLabels().has(t.pauseAria)).toBe(false);
    expect(ariaLabels().has(t.stop)).toBe(false);
    // The way back stays, and its accessible name holds still.
    expect(ariaLabels().has(t.shadowMode)).toBe(true);
    // The timer is gone too (no tabular-nums clock in the header).
    expect(document.querySelector('.tabular-nums')).toBeNull();

    appState.setShadowMode(false);
    flushSync();
    expect(ariaLabels().has(t.pauseAria)).toBe(true);
    expect(ariaLabels().has(t.stop)).toBe(true);

    unmount(app);
  });
});
