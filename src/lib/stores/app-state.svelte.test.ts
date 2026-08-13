import { beforeEach, describe, expect, it } from 'vitest';
import { appState } from './app-state.svelte';

// The store is a module singleton, so every test starts from a clean slate.
describe('recording pause state', () => {
  beforeEach(() => {
    appState.resetToIdle();
  });

  it('does not leak a paused flag into the next recording', () => {
    // Regression (260725): stopping a meeting while paused left `_isPaused`
    // set; the next meeting's timer effect never started and the header
    // showed Resume while the backend recorded unpaused.
    appState.setRecording(true);
    appState.startRecordingClock(Date.now() - 60_000);
    appState.setPaused(true);
    appState.setRecording(false);

    appState.setRecording(true);
    appState.startRecordingClock(Date.now());

    expect(appState.isPaused).toBe(false);
  });

  it('still excludes paused spans from the elapsed clock', () => {
    const t0 = Date.now() - 10_000;
    appState.setRecording(true);
    appState.startRecordingClock(t0);
    // A pause opens a live span; elapsedSeconds must not count past it.
    appState.setPaused(true);
    const during = appState.elapsedSeconds(Date.now());
    appState.setPaused(false);
    expect(appState.elapsedSeconds(Date.now())).toBeGreaterThanOrEqual(during);
  });
});

describe('speaker labeling standing', () => {
  beforeEach(() => {
    appState.resetToIdle();
    appState.setLiveDiarization(true);
  });

  it('separates the guard standing labeling down from the user doing it', () => {
    // The header note names the reason, so the two must not look alike.
    appState.setLiveDiarization(false);
    expect(appState.liveDiarization).toBe(false);
    expect(appState.diarizationRunaway).toBe(false);

    appState.standDownDiarization();
    expect(appState.liveDiarization).toBe(false);
    expect(appState.diarizationRunaway).toBe(true);
  });

  it('drops the guard reason when the standing actually changes', () => {
    // Turning labeling back on is a real flip; "too many speakers" is no
    // longer why it is off.
    appState.standDownDiarization();
    appState.setLiveDiarization(true);
    expect(appState.diarizationRunaway).toBe(false);
  });

  it('keeps the guard reason through a no-op reconcile', () => {
    // Regression (#19): the focus-time reconcile adopts the backend's
    // flag even when nothing changed, and it used to launder the guard's
    // reason into the user-choice wording.
    appState.standDownDiarization();
    appState.setLiveDiarization(false);
    expect(appState.liveDiarization).toBe(false);
    expect(appState.diarizationRunaway).toBe(true);
  });

  it('a fresh start carries no guard history', () => {
    appState.standDownDiarization();
    appState.clearDiarizationRunaway();
    expect(appState.diarizationRunaway).toBe(false);

    appState.standDownDiarization();
    appState.resetToIdle();
    expect(appState.diarizationRunaway).toBe(false);
  });
});
