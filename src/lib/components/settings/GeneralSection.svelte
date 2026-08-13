<script lang="ts">
    import { onMount } from "svelte";
    import type { Component } from "svelte";
    import { invoke } from "@tauri-apps/api/core";
    import { open } from "@tauri-apps/plugin-dialog";
    import { RefreshCw } from "lucide-svelte";
    import type { AppConfig, AudioDevices, Theme } from "$lib/types";
    import SettingsGroup from "./SettingsGroup.svelte";
    import MicAccess from "./MicAccess.svelte";
    import SettingRow from "./SettingRow.svelte";
    import * as Select from "$lib/components/ui/select";
    import { Switch } from "$lib/components/ui/switch";
    import { Input } from "$lib/components/ui/input";
    import { Button } from "$lib/components/ui/button";
    import { CLOUD_ENABLED, loadTelemetrySetting } from "$lib/cloud";
    import { copy } from "$lib/copy";

    let { draft }: { draft: AppConfig } = $props();

    const t = $derived(copy.settings.general);

    // The Privacy group (telemetry toggle) is a cloud-edition component
    // ([telemetry.md]); the open-core build has no telemetry.
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    let TelemetrySetting = $state<Component<any> | null>(null);
    onMount(async () => {
        TelemetrySetting = await loadTelemetrySetting();
    });

    // Recording indicator: the Windows accent by default, or a preset.
    // Sentinel because the stored "follow the accent" value is "".
    const ACCENT = "__accent__";
    // The palette is fixed; the names come from the catalog by key.
    const indicatorColors: {
        value: string;
        key: keyof typeof copy.settings.general.appearance.indicator.colors;
    }[] = [
        { value: "#b91c1c", key: "red" },
        { value: "#c2410c", key: "orange" },
        { value: "#15803d", key: "green" },
        { value: "#1d4ed8", key: "blue" },
        { value: "#6d28d9", key: "purple" },
        { value: "#be185d", key: "pink" },
    ];
    let indicatorLabel = $derived.by(() => {
        const found = indicatorColors.find(
            (c) => c.value === draft.tray_recording_color,
        );
        return found
            ? t.appearance.indicator.colors[found.key]
            : t.appearance.indicator.accent;
    });
    // The live accent, for the "Windows accent" swatch; stock blue until read.
    let accentColor = $state("#0078d4");
    let swatchColor = $derived(draft.tray_recording_color || accentColor);

    // --- Audio devices (moved from the former Audio page) ---
    // Sentinel for "system default" — an empty string is stored in config.
    const DEFAULT = "__default__";
    let devices = $state<AudioDevices>({ inputs: [], outputs: [] });
    let loading = $state(false);

    async function refresh() {
        loading = true;
        try {
            devices = await invoke<AudioDevices>("list_audio_devices");
        } catch (e) {
            console.error("list_audio_devices failed:", e);
        } finally {
            loading = false;
        }
    }

    onMount(() => {
        void refresh();
        invoke<string>("system_accent_color")
            .then((c) => (accentColor = c))
            .catch(() => {});
    });

    function deviceLabel(configured: string): string {
        return configured === "" ? t.audio.systemDefault : configured;
    }

    async function browseStorageDir() {
        const dir = await open({ directory: true });
        if (typeof dir === "string") {
            draft.storage_dir = dir;
        }
    }
</script>

<div class="space-y-6">
    <SettingsGroup label={t.appearance._group}>
        <SettingRow title={t.appearance.theme.label}>
            <Select.Root
                type="single"
                value={draft.theme}
                onValueChange={(v) => (draft.theme = (v ?? "system") as Theme)}
            >
                <Select.Trigger class="w-56"
                    >{t.appearance.theme.options[draft.theme]}</Select.Trigger
                >
                <Select.Content>
                    <Select.Item
                        value="system"
                        label={t.appearance.theme.options.system}
                    />
                    <Select.Item
                        value="light"
                        label={t.appearance.theme.options.light}
                    />
                    <Select.Item
                        value="dark"
                        label={t.appearance.theme.options.dark}
                    />
                </Select.Content>
            </Select.Root>
        </SettingRow>

        <SettingRow title={t.appearance.indicator.label}>
            <Select.Root
                type="single"
                value={draft.tray_recording_color === ""
                    ? ACCENT
                    : draft.tray_recording_color}
                onValueChange={(v) =>
                    (draft.tray_recording_color =
                        !v || v === ACCENT ? "" : v)}
            >
                <Select.Trigger class="w-56">
                    <span class="flex min-w-0 items-center gap-2">
                        <span
                            class="size-3 shrink-0 rounded-full"
                            style="background: {swatchColor}"
                        ></span>
                        <span class="truncate">{indicatorLabel}</span>
                    </span>
                </Select.Trigger>
                <Select.Content>
                    <Select.Item
                        value={ACCENT}
                        label={t.appearance.indicator.accent}
                    >
                        <span class="flex items-center gap-2">
                            <span
                                class="size-3 shrink-0 rounded-full"
                                style="background: {accentColor}"
                            ></span>
                            {t.appearance.indicator.accent}
                        </span>
                    </Select.Item>
                    {#each indicatorColors as c (c.value)}
                        <Select.Item
                            value={c.value}
                            label={t.appearance.indicator.colors[c.key]}
                        >
                            <span class="flex items-center gap-2">
                                <span
                                    class="size-3 shrink-0 rounded-full"
                                    style="background: {c.value}"
                                ></span>
                                {t.appearance.indicator.colors[c.key]}
                            </span>
                        </Select.Item>
                    {/each}
                </Select.Content>
            </Select.Root>
        </SettingRow>
    </SettingsGroup>

    <SettingsGroup label={t.storage._group}>
        <SettingRow title={t.storage.folder.label} vertical>
            <div class="flex w-full gap-2">
                <Input bind:value={draft.storage_dir} class="flex-1" />
                <Button variant="outline" size="sm" onclick={browseStorageDir}
                    >{t.storage.browse}</Button
                >
            </div>
        </SettingRow>
    </SettingsGroup>

    <SettingsGroup label={t.audio._group}>
        <MicAccess />
        <SettingRow title={t.audio.mic.label}>
            <Select.Root
                type="single"
                value={draft.mic_device === "" ? DEFAULT : draft.mic_device}
                onValueChange={(v) =>
                    (draft.mic_device = !v || v === DEFAULT ? "" : v)}
            >
                <Select.Trigger class="w-56"
                    ><span class="truncate min-w-0"
                        >{deviceLabel(draft.mic_device)}</span
                    ></Select.Trigger
                >
                <Select.Content>
                    <Select.Item value={DEFAULT} label={t.audio.systemDefault} />
                    {#each devices.inputs as name (name)}
                        <Select.Item value={name} label={name} />
                    {/each}
                </Select.Content>
            </Select.Root>
        </SettingRow>

        <SettingRow
            title={t.audio.systemAudio.label}
            description={t.audio.systemAudio.sub}
        >
            <Select.Root
                type="single"
                value={draft.output_device === ""
                    ? DEFAULT
                    : draft.output_device}
                onValueChange={(v) =>
                    (draft.output_device = !v || v === DEFAULT ? "" : v)}
            >
                <Select.Trigger class="w-56"
                    ><span class="truncate min-w-0"
                        >{deviceLabel(draft.output_device)}</span
                    ></Select.Trigger
                >
                <Select.Content>
                    <Select.Item value={DEFAULT} label={t.audio.systemDefault} />
                    {#each devices.outputs as name (name)}
                        <Select.Item value={name} label={name} />
                    {/each}
                </Select.Content>
            </Select.Root>
        </SettingRow>

        <SettingRow title={t.audio.refresh.label}>
            <Button
                variant="outline"
                size="sm"
                disabled={loading}
                onclick={refresh}
            >
                <RefreshCw size={14} class={loading ? "animate-spin" : ""} />
                {t.audio.refresh.button}
            </Button>
        </SettingRow>
    </SettingsGroup>

    <SettingsGroup label={t.notifications._group}>
        <SettingRow title={t.notifications.summaryReady.label}>
            <Switch bind:checked={draft.notify_summary_ready} />
        </SettingRow>
        <SettingRow title={t.notifications.recordingStarted.label}>
            <Switch bind:checked={draft.notify_recording_started} />
        </SettingRow>
        <SettingRow
            title={t.notifications.callDetected.label}
            description={t.notifications.callDetected.sub}
        >
            <Switch bind:checked={draft.notify_call_detected} />
        </SettingRow>
        <!-- Self-updating is cloud-edition-only (cloud-seam.md); without
             an updater there is no update-ready moment to announce. -->
        {#if CLOUD_ENABLED}
            <SettingRow title={t.notifications.updateReady.label}>
                <Switch bind:checked={draft.notify_update_available} />
            </SettingRow>
        {/if}
    </SettingsGroup>

    {#if TelemetrySetting}
        <TelemetrySetting {draft} />
    {/if}
</div>
