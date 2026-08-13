import { invoke } from "@tauri-apps/api/core";
import type { AppConfig } from "$lib/types";
import { modelsStore } from "$lib/stores/models.svelte";
import { meetingAsrModel } from "$lib/utils/asrModel";

function isTauri() {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

let _config = $state<AppConfig | null>(null);
let _isLoading = $state(false);

export const configStore = {
  get config() {
    return _config;
  },
  get isLoading() {
    return _isLoading;
  },
  get isConfigured(): boolean {
    if (!_config) return false;
    // Configured iff the model that would actually run is on disk (statuses
    // come from the engine catalog via modelsStore.refresh()). Cloud needs the
    // local model too: it is what a dropped connection falls back to.
    return modelsStore.status(meetingAsrModel(_config))?.present ?? false;
  },

  async load() {
    if (!isTauri()) {
      console.warn("Not running inside Tauri — skipping config load");
      return;
    }
    _isLoading = true;
    try {
      _config = await invoke<AppConfig>("get_config");
      // `isConfigured` reads model statuses, so load them now, not first when
      // a settings page happens to mount (the record button gates on this).
      await modelsStore.refresh();
    } finally {
      _isLoading = false;
    }
  },

  async save(config: AppConfig) {
    if (!isTauri()) {
      console.warn("Not running inside Tauri — skipping config save");
      return;
    }
    await invoke("save_config", { config });
    _config = config;
  },
};
