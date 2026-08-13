<script lang="ts">

	import { errorMessage } from '$lib/copy/errors';
    import { onMount } from "svelte";
    import { invoke } from "@tauri-apps/api/core";
    import { getVersion } from "@tauri-apps/api/app";
    import { FolderOpen, Loader2, RotateCcw } from "lucide-svelte";
    import { openUrl as openUrlExternal } from "@tauri-apps/plugin-opener";
    import * as Dialog from "$lib/components/ui/dialog";
    import SettingsGroup from "./SettingsGroup.svelte";
    import SettingRow from "./SettingRow.svelte";
    import { Button } from "$lib/components/ui/button";
    import { Switch } from "$lib/components/ui/switch";
    import EmbralIcon from "$lib/components/EmbralIcon.svelte";
    import { openNotesFolder } from "$lib/utils/openNotesFolder";
    import { configStore } from "$lib/stores/config.svelte";
    import { appState } from "$lib/stores/app-state.svelte";
    import { meetingsStore } from "$lib/stores/meetings.svelte";
    import { dictationStore } from "$lib/stores/dictation.svelte";
    import { modelsStore } from "$lib/stores/models.svelte";
    import { updaterStore } from "$lib/stores/updater.svelte";
    import { CLOUD_ENABLED } from "$lib/cloud";
    import { copy } from "$lib/copy";

    const t = $derived(copy.settings.about);

    let version = $state("");
    let resetOpen = $state(false);
    let resetting = $state(false);
    let resetError = $state("");

    // Settings is the old one-button reset, so it starts on.
    // The scope keys and order are this component's; labels come from the
    // catalog.
    const scopeKeys = [
        "settings",
        "meetings",
        "profiles",
        "dictations",
        "models",
    ] as const;
    type ScopeKey = (typeof scopeKeys)[number];
    let scopes = $state<Record<ScopeKey, boolean>>({
        settings: true,
        meetings: false,
        profiles: false,
        dictations: false,
        models: false,
    });
    let anyScope = $derived(scopeKeys.some((k) => scopes[k]));

    async function resetApp() {
        resetting = true;
        resetError = "";
        try {
            await invoke("reset_app_data", { scopes: { ...scopes } });
            if (scopes.meetings) await meetingsStore.load();
            if (scopes.dictations) await dictationStore.refresh();
            if (scopes.models) await modelsStore.refresh();
            resetOpen = false;
            if (scopes.settings) {
                // Reloading picks up onboarding_completed=false, so the
                // onboarding gate takes over the window.
                await configStore.load();
                appState.setView("idle");
            }
        } catch (e) {
            resetError = errorMessage(e);
        } finally {
            resetting = false;
        }
    }

    // Whether installing an update runs the package manager, which means an
    // authentication dialog the user should see coming. Backend-resolved: only
    // it can tell a .deb/.rpm install from an AppImage.
    let needsPassword = $state(false);

    onMount(async () => {
        try {
            version = await getVersion();
        } catch {
            version = "dev";
        }
        try {
            needsPassword = await invoke<boolean>("update_needs_authentication");
        } catch {
            needsPassword = false;
        }
    });

    async function openLogs() {
        try {
            await invoke("open_logs_folder");
        } catch (e) {
            console.error("open_logs_folder failed:", e);
        }
    }

    async function openUrl(url: string) {
        try {
            await openUrlExternal(url);
        } catch (e) {
            console.error("open url failed:", e);
        }
    }

    // Names, licenses, and URLs are data; the "what" description comes from
    // the catalog by key.
    const credits: {
        name: string;
        key: keyof typeof copy.settings.about.credits.what;
        license: string;
        url: string;
    }[] = [
        {
            name: "sherpa-onnx (k2-fsa)",
            key: "sherpa",
            license: "Apache-2.0",
            url: "https://github.com/k2-fsa/sherpa-onnx",
        },
        {
            name: "NVIDIA Parakeet & TitaNet",
            key: "parakeet",
            license: "CC-BY-4.0",
            url: "https://huggingface.co/nvidia",
        },
        {
            name: "icefall Zipformer",
            key: "zipformer",
            license: "Apache-2.0",
            url: "https://github.com/k2-fsa/icefall",
        },
        {
            name: "pyannote segmentation",
            key: "pyannote",
            license: "MIT",
            url: "https://github.com/pyannote/pyannote-audio",
        },
        {
            name: "NVIDIA TitaNet",
            key: "titanet",
            license: "CC-BY-4.0",
            url: "https://catalog.ngc.nvidia.com/orgs/nvidia/teams/nemo/models/titanet_small",
        },
        {
            name: "Silero VAD",
            key: "silero",
            license: "MIT",
            url: "https://github.com/snakers4/silero-vad",
        },
        {
            name: "Qwen3 (Alibaba)",
            key: "qwen",
            license: "Apache-2.0",
            url: "https://github.com/QwenLM/Qwen3",
        },
        {
            name: "llama.cpp",
            key: "llama",
            license: "MIT",
            url: "https://github.com/ggml-org/llama.cpp",
        },
    ];
</script>

<div class="space-y-6">
    <div class="flex items-center gap-4 rounded-lg border border-border p-5">
        <EmbralIcon size={36} />
        <div class="min-w-0">
            <p class="text-base font-semibold tracking-tight">embral</p>
            <p class="mt-0.5 text-xs text-muted-foreground">
                {t.tagline}
            </p>
            <p class="mt-1 font-mono text-[11px] text-muted-foreground/80">
                {t.version(version)}
            </p>
        </div>
    </div>

    <!-- Self-updating is cloud-edition-only: the release channel serves
         cloud installers; source builds update via git (cloud-seam.md). -->
    {#if CLOUD_ENABLED}
    <SettingsGroup label={t.updates._group}>
        <SettingRow
            title={updaterStore.available
                ? t.updates.ready(updaterStore.available.version)
                : t.updates.upToDate}
            description={updaterStore.blocked
                ? t.updates.blocked(updaterStore.blocked)
                : updaterStore.available && needsPassword
                  ? t.updates.needsPassword
                  : ""}
        >
            {#if updaterStore.available}
                <Button
                    size="sm"
                    onclick={() => updaterStore.install()}
                    disabled={updaterStore.installing}
                >
                    {#if updaterStore.installing}
                        <Loader2 size={13} class="animate-spin" />
                        {t.updates.installing}
                    {:else}
                        <RotateCcw size={13} />
                        {t.updates.restartAndUpdate}
                    {/if}
                </Button>
            {:else}
                <Button
                    variant="outline"
                    size="sm"
                    onclick={() => updaterStore.checkNow()}
                    disabled={updaterStore.checking}
                >
                    {#if updaterStore.checking}
                        <Loader2 size={13} class="animate-spin" />
                    {/if}
                    {t.updates.checkForUpdates}
                </Button>
            {/if}
        </SettingRow>
        {#if updaterStore.error}
            <p class="px-4 pb-3 text-xs text-destructive">{updaterStore.error}</p>
        {/if}
    </SettingsGroup>
    {/if}

    <SettingsGroup label={t.diagnostics._group}>
        <SettingRow
            title={t.diagnostics.logs.label}
            description={t.diagnostics.logs.sub}
        >
            <Button variant="outline" size="sm" onclick={openLogs}>
                <FolderOpen size={14} />
                {t.diagnostics.logs.button}
            </Button>
        </SettingRow>
        <SettingRow title={t.diagnostics.notesFolder.label}>
            <Button variant="outline" size="sm" onclick={openNotesFolder}>
                <FolderOpen size={14} />
                {t.diagnostics.notesFolder.button}
            </Button>
        </SettingRow>
        <SettingRow title={t.diagnostics.reset.label}>
            <Button
                variant="outline"
                size="sm"
                class="text-destructive hover:text-destructive"
                disabled={resetting}
                onclick={() => (resetOpen = true)}
            >
                <RotateCcw size={14} />
                {t.diagnostics.reset.button}
            </Button>
        </SettingRow>
    </SettingsGroup>

    <SettingsGroup label={t.credits._group}>
        <div class="px-4 py-3">
            <div class="space-y-1.5">
                {#each credits as c (c.name)}
                    <div class="flex items-baseline justify-between gap-3 text-xs">
                        <button
                            class="min-w-0 truncate text-left text-foreground underline-offset-2 hover:underline"
                            onclick={() => openUrl(c.url)}
                        >
                            {c.name}
                        </button>
                        <span class="shrink-0 text-muted-foreground"
                            >{t.credits.what[c.key]} · {c.license}</span
                        >
                    </div>
                {/each}
            </div>
        </div>
    </SettingsGroup>
</div>

<Dialog.Root bind:open={resetOpen}>
    <Dialog.Content class="sm:max-w-md">
        <Dialog.Header>
            <Dialog.Title>{t.resetDialog.title}</Dialog.Title>
            <Dialog.Description>
                {t.resetDialog.description}
            </Dialog.Description>
        </Dialog.Header>
        <div>
            {#each scopeKeys as key (key)}
                <div class="flex items-center justify-between py-2">
                    <span class="text-sm">{t.resetDialog.scopes[key]}</span>
                    <Switch bind:checked={scopes[key]} />
                </div>
            {/each}
        </div>
        {#if resetError}
            <p class="text-xs text-destructive">{resetError}</p>
        {/if}
        <Dialog.Footer>
            <Button variant="ghost" size="sm" onclick={() => (resetOpen = false)}>
                {t.resetDialog.cancel}
            </Button>
            <Button
                variant="destructive"
                size="sm"
                disabled={!anyScope || resetting}
                onclick={resetApp}
            >
                {resetting ? t.resetDialog.resetting : t.resetDialog.reset}
            </Button>
        </Dialog.Footer>
    </Dialog.Content>
</Dialog.Root>
