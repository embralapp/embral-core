// Staged-state fixture for screenshot tooling: `fixture_state` returns a
// value only when the backend runs with the EMBRAL_DATA_DIR override and
// the sandbox provides a fixture file (configuration.md); every normal
// launch gets null and none of this runs. The staged moment's data is
// derived from a real imported meeting by the pipeline that writes the
// fixture; the components rendering it are the real ones.

import { invoke } from '@tauri-apps/api/core';
import { appState } from '$lib/stores/app-state.svelte';
import type { InterimSegment, TranscriptionSegment } from '$lib/types';

export interface RecordingFixture {
  title: string;
  elapsed_seconds: number;
  notes_markdown: string;
  segments: TranscriptionSegment[];
  interim: InterimSegment | null;
  stars: number[];
  level_mic: number[];
  level_system: number[];
  diarization: boolean;
}

export interface OverlayFixture {
  phase: 'listening' | 'finishing';
  text: string;
  tentative: string;
  bands: number[];
}

export interface FixtureState {
  recording?: RecordingFixture;
  overlay?: OverlayFixture;
}

let _active = false;

/** True once a fixture loaded: the focus-time recording reconcile must not
 * "correct" a staged moment against the backend's idle truth. */
export function fixtureActive(): boolean {
  return _active;
}

export async function loadFixture(): Promise<FixtureState | null> {
  const f = await invoke<FixtureState | null>('fixture_state').catch(() => null);
  if (f && (f.recording || f.overlay)) {
    _active = true;
    return f;
  }
  return null;
}

/** Hydrate the live-recording view's stores from the staged moment. The
 * caller owns the notes/title drafts (page-local state) and applies them
 * after a tick, past the fresh-recording draft clear. */
export function applyRecordingFixture(f: RecordingFixture) {
  appState.setRecording(true);
  appState.startRecordingClock(Date.now() - f.elapsed_seconds * 1000);
  appState.setLiveDiarization(f.diarization ?? true);
  appState.replaceSegments(f.segments ?? []);
  appState.setInterim(f.interim ?? null);
  appState.setAudioLevel(f.level_mic ?? [], f.level_system ?? []);
  for (const s of f.stars ?? []) appState.addStar(s);
  appState.setView('recording');
  // Deterministic hydration marker for the capture tooling's wait.
  document.documentElement.dataset.fixture = 'recording';
}
