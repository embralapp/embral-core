<script lang="ts">
    // Dictation's day-one decisions: the hotkey that makes it exist, the
    // cleanup mode, and where the text goes. The provider/language tree
    // keeps its defaults; Settings has the rest.
    import * as Select from "$lib/components/ui/select";
    import { Switch } from "$lib/components/ui/switch";
    import SettingRow from "$lib/components/settings/SettingRow.svelte";
    import SettingsGroup from "$lib/components/settings/SettingsGroup.svelte";
    import HotkeyCapture from "$lib/components/settings/HotkeyCapture.svelte";
    import { cloudAuth } from "$lib/stores/cloudAuth.svelte";
    import { CLOUD_ENABLED } from "$lib/cloud";
    import type { DictationCleanup } from "$lib/types";
    import { copy } from "$lib/copy";
    import AccessibilityAccess from "$lib/components/settings/AccessibilityAccess.svelte";
    import type { OnboardingDraft } from "../types";

    let { draft }: { draft: OnboardingDraft } = $props();

    const t = $derived(copy.onboarding.dictation);
    const providers = $derived(copy.common.providers);

    let cleanupLabels: Record<string, string> = $derived({
        cloud: providers.cloud,
        on_device: providers.localModel,
        off: t.cleanupOff,
    });
</script>

<h1 class="font-display text-2xl tracking-tight">{t.title}</h1>
<p class="mt-3 text-sm text-muted-foreground">
    {t.intro}
</p>

<div class="mt-6 space-y-4">
    <SettingsGroup>
        <SettingRow title={t.hotkey}>
            <HotkeyCapture
                value={draft.dictation_hotkey}
                ariaLabel={t.hotkeyAria}
                onChange={(combo) => (draft.dictation_hotkey = combo)}
            />
        </SettingRow>

        <SettingRow title={t.cleanup} description={t.cleanupSub}>
            <Select.Root
                type="single"
                value={draft.dictation_cleanup}
                onValueChange={(v) => {
                    if (!v) return;
                    if (v === "cloud" && !cloudAuth.requireSignedIn()) return;
                    draft.dictation_cleanup = v as DictationCleanup;
                }}
            >
                <Select.Trigger class="w-44"
                    >{cleanupLabels[draft.dictation_cleanup]}</Select.Trigger
                >
                <Select.Content>
                    {#if CLOUD_ENABLED}
                        <Select.Item value="cloud" label={providers.cloud} />
                    {/if}
                    <Select.Item value="on_device" label={providers.localModel} />
                    <Select.Item value="off" label={t.cleanupOff} />
                </Select.Content>
            </Select.Root>
        </SettingRow>

        <SettingRow title={t.copyClipboard}>
            <Switch bind:checked={draft.dictation_copy_clipboard} />
        </SettingRow>

        <SettingRow title={t.autoPaste}>
            <Switch bind:checked={draft.dictation_auto_paste} />
        </SettingRow>
        <AccessibilityAccess enabled={draft.dictation_auto_paste} />
    </SettingsGroup>
</div>
