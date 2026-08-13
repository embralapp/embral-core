import { describe, expect, it } from 'vitest';
import { dictationAsrModel, meetingAsrModel, MULTILINGUAL_ASR_MODEL } from './asrModel';
import type { AppConfig } from '$lib/types';

/**
 * These mirror `AppConfig::meeting_asr_model` and `dictation_asr_model_id` in
 * `crates/embral-types`. Two implementations of one rule will drift, and drift
 * between them is silent: the settings page would show one model while the
 * engine loaded another. This is the drift point a test guards and review
 * does not.
 */
function config(fields: Partial<AppConfig>): AppConfig {
  return {
    transcription_language: 'english',
    dictation_language: 'english',
    local_asr_model: 'zipformer-en',
    dictation_asr_model: '',
    ...fields
  } as AppConfig;
}

describe('meetingAsrModel', () => {
  it('is the configured English model', () => {
    expect(meetingAsrModel(config({ local_asr_model: 'parakeet-tdt-en' }))).toBe('parakeet-tdt-en');
  });

  it('is overridden — not overwritten — by the multilingual language', () => {
    // The tier the user picked survives in config while another language is
    // selected, so switching back restores it instead of resetting to default.
    // That is the whole reason the language overrides rather than writing the
    // model id.
    const multilingual = config({
      local_asr_model: 'parakeet-tdt-en',
      transcription_language: 'multilingual'
    });
    expect(meetingAsrModel(multilingual)).toBe(MULTILINGUAL_ASR_MODEL);
    expect(multilingual.local_asr_model).toBe('parakeet-tdt-en');

    expect(meetingAsrModel({ ...multilingual, transcription_language: 'english' })).toBe(
      'parakeet-tdt-en'
    );
  });
});

describe('dictationAsrModel', () => {
  it('follows the meeting model when unset', () => {
    // "" has always meant "same as meetings" backend-side.
    expect(
      dictationAsrModel(config({ local_asr_model: 'zipformer-en', dictation_asr_model: '' }))
    ).toBe('zipformer-en');
    // Whitespace is not a choice.
    expect(dictationAsrModel(config({ dictation_asr_model: '   ' }))).toBe('zipformer-en');
  });

  it('uses its own model when it has one', () => {
    expect(dictationAsrModel(config({ dictation_asr_model: 'zipformer-en-small' }))).toBe(
      'zipformer-en-small'
    );
  });

  it('follows its own language, not the meetings one', () => {
    // Meetings going multilingual must not drag dictation along…
    expect(
      dictationAsrModel(
        config({
          dictation_asr_model: 'zipformer-en-small',
          transcription_language: 'multilingual'
        })
      )
    ).toBe('zipformer-en-small');
    // …and dictation's own language override works like the meetings one.
    expect(
      dictationAsrModel(
        config({
          dictation_asr_model: 'zipformer-en-small',
          dictation_language: 'multilingual'
        })
      )
    ).toBe(MULTILINGUAL_ASR_MODEL);
  });
});
