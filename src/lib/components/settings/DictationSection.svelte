<script lang="ts">
    import { onMount } from "svelte";
    import { Info } from "lucide-svelte";
    import type { AppConfig, DictationCleanup } from "$lib/types";
    import TranscriptionBlock from "./TranscriptionBlock.svelte";
    import HotkeyCapture from "./HotkeyCapture.svelte";
    import SettingsGroup from "./SettingsGroup.svelte";
    import SettingRow from "./SettingRow.svelte";
    import * as Dialog from "$lib/components/ui/dialog";
    import * as Select from "$lib/components/ui/select";
    import { Input } from "$lib/components/ui/input";
    import { Switch } from "$lib/components/ui/switch";
    import { modelsStore } from "$lib/stores/models.svelte";
    import { CLOUD_ENABLED } from "$lib/cloud";
    import { cloudAuth } from "$lib/stores/cloudAuth.svelte";
    import { copy } from "$lib/copy";
    import AccessibilityAccess from "./AccessibilityAccess.svelte";

    let { draft }: { draft: AppConfig } = $props();

    const t = $derived(copy.settings.dictation);
    const providers = $derived(copy.common.providers);

    onMount(() => {
        modelsStore.refresh();
    });

    // "" historically meant "same as meetings"; the UI now always shows a
    // concrete model (the backend still falls back while the value is "").
    let effectiveDictationModel = $derived(
        draft.dictation_asr_model || draft.local_asr_model,
    );

    // cloud / on_device share the app-wide provider labels; "off" is this
    // page's own word.
    let cleanupLabels: Record<DictationCleanup, string> = $derived({
        cloud: providers.cloud,
        on_device: providers.localModel,
        off: t.output.cleanup.off,
    });
    let cleanupInfoOpen = $state(false);
</script>

<div class="space-y-6">
    <SettingsGroup label={t.start._group}>
        <SettingRow title={t.start.hotkey.label}>
            <HotkeyCapture
                value={draft.dictation_hotkey}
                ariaLabel={t.start.hotkey.aria}
                onChange={(combo) => (draft.dictation_hotkey = combo)}
            />
        </SettingRow>
    </SettingsGroup>

    <!-- Dictation's own transcription tree — independent of the Meetings
         one, because cloud meetings with on-device dictation is legitimate. -->
    <SettingsGroup label={t.transcription._group}>
        <TranscriptionBlock
            providerLabel={t.transcription.providerLabel}
            provider={draft.dictation_provider}
            onProviderChange={(v) => (draft.dictation_provider = v)}
            outOfHours={draft.dictation_out_of_hours}
            onOutOfHoursChange={(v) => (draft.dictation_out_of_hours = v)}
            language={draft.dictation_language}
            onLanguageChange={(v) => (draft.dictation_language = v)}
            accuracyModel={effectiveDictationModel}
            onAccuracyChange={(id) => (draft.dictation_asr_model = id)}
        />
    </SettingsGroup>

    <SettingsGroup label={t.output._group}>
        <SettingRow title={t.output.copyClipboard.label}>
            <Switch bind:checked={draft.dictation_copy_clipboard} />
        </SettingRow>
        <SettingRow title={t.output.autoPaste.label}>
            <Switch bind:checked={draft.dictation_auto_paste} />
        </SettingRow>
        <AccessibilityAccess enabled={draft.dictation_auto_paste} />
        <SettingRow title={t.output.cleanup.label}>
            {#snippet titleExtra()}
                <button
                    class="text-muted-foreground/60 transition-colors hover:text-foreground"
                    aria-label={t.output.cleanup.infoAria}
                    onclick={() => (cleanupInfoOpen = true)}
                >
                    <Info size={13} />
                </button>
            {/snippet}
            <Select.Root
                type="single"
                value={draft.dictation_cleanup}
                onValueChange={(v) => {
                    if (!v) return;
                    // Cloud cleanup needs an account; refuse and prompt when
                    // signed out, leaving cleanup on its current value.
                    if (v === "cloud" && !cloudAuth.requireSignedIn()) return;
                    draft.dictation_cleanup = v as DictationCleanup;
                }}
            >
                <Select.Trigger class="w-56"
                    >{cleanupLabels[draft.dictation_cleanup]}</Select.Trigger
                >
                <Select.Content>
                    {#if CLOUD_ENABLED}
                        <Select.Item value="cloud" label={providers.cloud} />
                    {/if}
                    <Select.Item value="on_device" label={providers.localModel} />
                    <Select.Item value="off" label={t.output.cleanup.off} />
                </Select.Content>
            </Select.Root>
        </SettingRow>
    </SettingsGroup>

    <SettingsGroup label={t.history._group}>
        <SettingRow
            title={t.history.autoDelete.label}
            description={draft.dictation_auto_delete
                ? t.history.autoDelete.sub
                : ""}
        >
            <Switch bind:checked={draft.dictation_auto_delete} />
        </SettingRow>
        {#if draft.dictation_auto_delete}
            <!-- value + clamp, not bind:value: an emptied number input binds
                 null, and one bad field fails the whole config autosave
                 against the Rust u32. -->
            <SettingRow title={t.history.deleteAfter.label}>
                <div class="flex items-center gap-2">
                    <Input
                        type="number"
                        min="0"
                        value={String(draft.dictation_retention_days)}
                        oninput={(e) =>
                            (draft.dictation_retention_days = Math.max(
                                0,
                                Math.floor(Number(e.currentTarget.value) || 0),
                            ))}
                        class="w-16 text-right"
                    />
                    <span class="text-xs text-muted-foreground"
                        >{t.history.deleteAfter.unit}</span
                    >
                </div>
            </SettingRow>
            <SettingRow title={t.history.keepLast.label}>
                <div class="flex items-center gap-2">
                    <Input
                        type="number"
                        min="0"
                        value={String(draft.dictation_retention_count)}
                        oninput={(e) =>
                            (draft.dictation_retention_count = Math.max(
                                0,
                                Math.floor(Number(e.currentTarget.value) || 0),
                            ))}
                        class="w-16 text-right"
                    />
                    <span class="text-xs text-muted-foreground"
                        >{t.history.keepLast.unit}</span
                    >
                </div>
            </SettingRow>
        {/if}
    </SettingsGroup>
</div>

<Dialog.Root bind:open={cleanupInfoOpen}>
    <Dialog.Content class="sm:max-w-lg">
        <Dialog.Header>
            <Dialog.Title>{t.cleanupDialog.title}</Dialog.Title>
            <Dialog.Description>
                {t.cleanupDialog.description}
            </Dialog.Description>
        </Dialog.Header>
        <div class="space-y-4 text-sm">
            <div>
                <p class="font-medium">{t.cleanupDialog.fillers.heading}</p>
                <p class="mt-1 text-xs text-muted-foreground">
                    "{t.cleanupDialog.fillers.input}"
                </p>
                <p class="mt-0.5 text-xs">
                    {t.cleanupDialog.fillers.output}
                </p>
            </div>
            <div>
                <p class="font-medium">{t.cleanupDialog.formatting.heading}</p>
                <p class="mt-1 text-xs text-muted-foreground">
                    "{t.cleanupDialog.formatting.input}"
                </p>
                <p class="mt-0.5 text-xs whitespace-pre-line">{t.cleanupDialog
                        .formatting.output}</p>
            </div>
            <div>
                <p class="font-medium">{t.cleanupDialog.instruction.heading}</p>
                <p class="mt-1 text-xs text-muted-foreground">
                    {t.cleanupDialog.instruction.input}
                </p>
                <p class="mt-0.5 text-xs">{t.cleanupDialog.instruction.output}</p>
            </div>
        </div>
    </Dialog.Content>
</Dialog.Root>
