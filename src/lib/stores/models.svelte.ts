import { errorMessage } from '$lib/copy/errors';
import { invoke } from "@tauri-apps/api/core";
import type { ModelProgress, ModelStatus } from "$lib/types";

function isTauri() {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

// Global (module-level) so download progress and status survive the user
// navigating away from the Settings view mid-download: the SettingsForm
// component unmounts, but this store and the backend download keep going.
// Progress/complete events are routed here from the global listener in
// events.ts; the form only renders from this store.
let _statuses = $state<ModelStatus[]>([]);
let _downloading = $state<Record<string, ModelProgress | null>>({});
let _errors = $state<Record<string, string>>({});

export const modelsStore = {
  get statuses() {
    return _statuses;
  },
  status(id: string): ModelStatus | undefined {
    return _statuses.find((s) => s.id === id);
  },
  isDownloading(id: string): boolean {
    return id in _downloading;
  },
  progress(id: string): ModelProgress | null {
    return _downloading[id] ?? null;
  },
  /// 0..1 fraction for a model's download, or null when idle/unknown.
  fraction(id: string): number | null {
    const p = _downloading[id];
    if (!p || p.total_bytes === 0) return null;
    return p.downloaded_bytes / p.total_bytes;
  },
  error(id: string): string | null {
    return _errors[id] ?? null;
  },

  async refresh() {
    if (!isTauri()) return;
    try {
      _statuses = await invoke<ModelStatus[]>("asr_models_status");
    } catch (e) {
      console.error("asr_models_status failed:", e);
    }
  },

  async download(id: string) {
    if (!isTauri() || id in _downloading) return;
    delete _errors[id];
    _downloading[id] = null;
    try {
      // Resolves only when every file is fetched. The promise is held at
      // module scope, so it survives the SettingsForm unmounting.
      await invoke("download_asr_model", { modelId: id });
    } catch (e) {
      _errors[id] = errorMessage(e);
    } finally {
      delete _downloading[id];
      await this.refresh();
    }
  },

  async remove(id: string) {
    if (!isTauri()) return;
    delete _errors[id];
    try {
      await invoke("delete_asr_model", { modelId: id });
    } catch (e) {
      _errors[id] = errorMessage(e);
    }
    await this.refresh();
  },

  // --- Called from the global event listeners in events.ts ---
  _onProgress(p: ModelProgress) {
    _downloading[p.model_id] = p;
  },
  _onComplete() {
    void this.refresh();
  },
};
