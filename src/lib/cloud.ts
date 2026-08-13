// The frontend half of the cloud seam (see docs/cloud-seam.md).
//
// This is the only file outside src/lib/cloud/ that may reference that
// directory, and it must do so through `import.meta.glob`, never a
// literal `import('./cloud/…')`: the public-repo filter drops
// src/lib/cloud/ wholesale, and a literal specifier fails svelte-check and
// rollup the moment the directory is gone (found by the first real
// open-core build). The glob resolves to an empty map in that tree, so
// the offline build still compiles without the directory.
// Cloud UI renders only when the build was produced with
// VITE_EMBRAL_CLOUD=1 (paired with the Rust `cloud` cargo feature).
import type { Component } from 'svelte';

export const CLOUD_ENABLED = import.meta.env.VITE_EMBRAL_CLOUD === '1';

const cloudComponents = import.meta.glob('./cloud/*.svelte');

async function loadCloudComponent(name: string): Promise<Component<any> | null> {
  if (!CLOUD_ENABLED) return null;
  const load = cloudComponents[`./cloud/${name}.svelte`];
  if (!load) return null;
  const module = (await load()) as { default: Component<any> };
  return module.default;
}

/** Lazily load the Account settings section (cloud builds only). */
export function loadAccountSection() {
  return loadCloudComponent('AccountSection');
}

/** Lazily load the sidebar's hours-remaining ring (cloud builds only). */
export function loadHoursRing() {
  return loadCloudComponent('HoursRing');
}

/** Lazily load onboarding's account step (cloud builds only). */
export function loadOnboardingAccountStep() {
  return loadCloudComponent('OnboardingAccountStep');
}

/** Lazily load onboarding's plans page (cloud builds only). */
export function loadOnboardingPlansStep() {
  return loadCloudComponent('OnboardingPlansStep');
}

/** Lazily load the welcome page's telemetry opt-in checkbox (cloud builds only). */
export function loadTelemetryOptIn() {
  return loadCloudComponent('TelemetryOptIn');
}

/** Lazily load the General settings' telemetry toggle row (cloud builds only). */
export function loadTelemetrySetting() {
  return loadCloudComponent('TelemetrySetting');
}

/** The window event that says this device's account or hours changed: signing
 * in or out, a purchase coming back from the browser, a device revoked. The
 * hours ring and the settings rail re-read on it.
 *
 * It lives here rather than in the cloud-only tree because one listener
 * (`settings/SettingsLayout.svelte`) is shared-tree code and may not import
 * from `src/lib/cloud/`. A bare string in nine places was one typo away from
 * silently never refreshing. */
export const CLOUD_CHANGED_EVENT = 'embral:cloud-changed';
