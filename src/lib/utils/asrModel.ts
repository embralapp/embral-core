import type { AppConfig } from '$lib/types';

/** The one catalog model covering languages beyond English. The accuracy tier
 * is an English concept; there is nothing to choose between here. Mirrors
 * `embral_types::MULTILINGUAL_ASR_MODEL`. */
export const MULTILINGUAL_ASR_MODEL = 'parakeet-tdt-v3';

/* The catalog ids the settings picker and onboarding both name (mirror
 * `embral-engine`'s catalog.rs). They live here, with the other model-id
 * knowledge, so a rename is one edit rather than a hunt through markup. */

/** The English accuracy tiers, fastest to most accurate. */
export const ASR_FAST = 'zipformer-en-small';
export const ASR_BALANCED = 'zipformer-en';
export const ASR_ACCURATE = 'parakeet-tdt-en';

/** The built-in synthesis engine: a runtime and its weights, downloaded as a
 * pair. */
export const LLM_RUNTIME = 'llama-server';
export const LLM_WEIGHTS = 'qwen3-4b';

/** The model on-device transcription actually runs. `local_asr_model` holds
 * the English accuracy choice, so another language overrides it rather than
 * overwriting it; switching back restores the tier the user picked. Mirrors
 * `AppConfig::meeting_asr_model` (keep the two in step). */
export function meetingAsrModel(config: AppConfig): string {
  return config.transcription_language === 'multilingual'
    ? MULTILINGUAL_ASR_MODEL
    : config.local_asr_model;
}

/** The model on-device dictation runs, governed by dictation's own
 * language; an empty setting follows the meeting model. Mirrors
 * `AppConfig::dictation_asr_model_id`. */
export function dictationAsrModel(config: AppConfig): string {
  if (config.dictation_language === 'multilingual') return MULTILINGUAL_ASR_MODEL;
  return config.dictation_asr_model.trim() || config.local_asr_model;
}
