import { describe, expect, it } from 'vitest';
import { usesLocalLlm } from './llmUsage';
import type { AppConfig } from '$lib/types';

/** Mirrors `llm::uses_local_llm` in src-tauri, the same drift hazard the
 * asrModel mirror guards: the Performance settings would show while the
 * backend refuses to keep the model warm, or vice versa. */
function config(fields: Partial<AppConfig>): AppConfig {
  return {
    summaries_enabled: false,
    summaries_profile_id: 'builtin',
    dictation_cleanup: 'off',
    cloud_session_token: '',
    ...fields
  } as AppConfig;
}

describe('usesLocalLlm', () => {
  it('counts builtin summaries, including the legacy empty id', () => {
    expect(usesLocalLlm(config({ summaries_enabled: true }))).toBe(true);
    expect(
      usesLocalLlm(config({ summaries_enabled: true, summaries_profile_id: '' }))
    ).toBe(true);
    expect(
      usesLocalLlm(config({ summaries_enabled: true, summaries_profile_id: 'cloud' }))
    ).toBe(false);
  });

  it('counts on-device cleanup on its own', () => {
    expect(usesLocalLlm(config({ dictation_cleanup: 'on_device' }))).toBe(true);
  });

  it('counts cloud cleanup only while signed out — the degrade chain lands here', () => {
    expect(usesLocalLlm(config({ dictation_cleanup: 'cloud' }))).toBe(true);
    expect(
      usesLocalLlm(config({ dictation_cleanup: 'cloud', cloud_session_token: 'tok' }))
    ).toBe(false);
  });

  it('is false when every engine has left the device', () => {
    expect(usesLocalLlm(config({}))).toBe(false);
  });
});
