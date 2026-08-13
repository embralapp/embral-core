import { BUILTIN_PROFILE_ID, type AppConfig } from '$lib/types';

/** Whether anything in this configuration uses the built-in on-device model:
 * summaries on the builtin engine, dictation cleanup on-device, or cloud
 * cleanup while signed out (its degrade chain falls back to the device then).
 * Mirrors `llm::uses_local_llm` in src-tauri (keep the two in step); the
 * Performance settings render only while this is true, and the backend stops
 * honoring keep-warm at the same boundary. */
export function usesLocalLlm(config: AppConfig): boolean {
  const summariesLocal =
    config.summaries_enabled &&
    (!config.summaries_profile_id || config.summaries_profile_id === BUILTIN_PROFILE_ID);
  const cleanupLocal =
    config.dictation_cleanup === 'on_device' ||
    (config.dictation_cleanup === 'cloud' && !config.cloud_session_token);
  return summariesLocal || cleanupLocal;
}
