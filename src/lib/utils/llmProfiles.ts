import type { LlmProfile } from '$lib/types';
import { BUILTIN_PROFILE_ID } from '$lib/types';

// Mirrors embral-types::available_profiles. The synthesis engines are fixed
// per edition (no user-defined registry): the offline core has exactly the
// built-in engine; the cloud edition adds embral cloud with R7.

export function builtinProfile(): LlmProfile {
  return {
    id: BUILTIN_PROFILE_ID,
    name: 'Built-in (on-device)',
    provider: 'builtin',
    model: '',
    endpoint: '',
    api_key: ''
  };
}

export function availableProfiles(): LlmProfile[] {
  return [builtinProfile()];
}
