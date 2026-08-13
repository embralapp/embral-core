<script lang="ts">
    // Meetings, distilled to the three decisions that matter on day one:
    // when to start, whether to summarize (and with what), and the hotkey.
    // Everything else keeps its default; Settings remains the full surface.
    import { LLM_RUNTIME, LLM_WEIGHTS } from "$lib/utils/asrModel";
    import * as Select from "$lib/components/ui/select";
    import { Switch } from "$lib/components/ui/switch";
    import { Button } from "$lib/components/ui/button";
    import SettingRow from "$lib/components/settings/SettingRow.svelte";
    import SettingsGroup from "$lib/components/settings/SettingsGroup.svelte";
    import MicAccess from "$lib/components/settings/MicAccess.svelte";
    import HotkeyCapture from "$lib/components/settings/HotkeyCapture.svelte";
    import { modelsStore } from "$lib/stores/models.svelte";
    import { cloudAuth } from "$lib/stores/cloudAuth.svelte";
    import { CLOUD_ENABLED } from "$lib/cloud";
    import type { AutoStartPolicy } from "$lib/types";
    import { BUILTIN_PROFILE_ID, CLOUD_PROFILE_ID } from "$lib/types";
    import { copy } from "$lib/copy";
    import type { OnboardingDraft } from "../types";

    let { draft }: { draft: OnboardingDraft } = $props();

    const t = $derived(copy.onboarding.meetings);
    const providers = $derived(copy.common.providers);

    let policyOptions: { value: AutoStartPolicy; label: string }[] = $derived([
        { value: "always", label: t.autoStartOptions.always },
        { value: "prompt", label: t.autoStartOptions.prompt },
        { value: "manual", label: t.autoStartOptions.manual },
    ]);
    let policyLabel = $derived(
        policyOptions.find((o) => o.value === draft.auto_start_policy)?.label ??
            t.autoStartOptions.prompt,
    );

    let engineLabel = $derived(
        draft.summaries_profile_id === CLOUD_PROFILE_ID
            ? providers.cloud
            : providers.localModel,
    );

    // The summaries-on/no-model dead end: engine is on-device but the LLM
    // pack was declined at the models step. One tap fixes it here.
    let llmMissing = $derived.by(() => {
        const runtime = modelsStore.status(LLM_RUNTIME);
        const weights = modelsStore.status(LLM_WEIGHTS);
        if (!runtime || !weights) return false;
        const present = runtime.present && weights.present;
        const downloading =
            modelsStore.isDownloading(LLM_RUNTIME) ||
            modelsStore.isDownloading(LLM_WEIGHTS);
        return !present && !downloading;
    });
    let needsLlmNudge = $derived(
        draft.summaries_enabled &&
            draft.summaries_profile_id !== CLOUD_PROFILE_ID &&
            llmMissing,
    );
</script>

<h1 class="font-display text-2xl tracking-tight">{t.title}</h1>
<p class="mt-3 text-sm text-muted-foreground">
    {t.intro}
</p>

<div class="mt-6 space-y-4">
    <MicAccess />
    <SettingsGroup>
        <SettingRow title={t.autoStart}>
            <Select.Root
                type="single"
                value={draft.auto_start_policy}
                onValueChange={(v) =>
                    (draft.auto_start_policy = (v ?? "prompt") as AutoStartPolicy)}
            >
                <Select.Trigger class="w-44">{policyLabel}</Select.Trigger>
                <Select.Content>
                    {#each policyOptions as o (o.value)}
                        <Select.Item value={o.value} label={o.label} />
                    {/each}
                </Select.Content>
            </Select.Root>
        </SettingRow>

        <SettingRow title={t.summarize}>
            <Switch bind:checked={draft.summaries_enabled} />
        </SettingRow>

        {#if draft.summaries_enabled}
            <SettingRow title={t.engine}>
                <Select.Root
                    type="single"
                    value={draft.summaries_profile_id}
                    onValueChange={(v) => {
                        if (!v) return;
                        if (v === CLOUD_PROFILE_ID && !cloudAuth.requireSignedIn())
                            return;
                        draft.summaries_profile_id = v;
                    }}
                >
                    <Select.Trigger class="w-44">{engineLabel}</Select.Trigger>
                    <Select.Content>
                        {#if CLOUD_ENABLED}
                            <Select.Item value={CLOUD_PROFILE_ID} label={providers.cloud} />
                        {/if}
                        <Select.Item value={BUILTIN_PROFILE_ID} label={providers.localModel} />
                    </Select.Content>
                </Select.Root>
            </SettingRow>
            {#if needsLlmNudge}
                <div class="flex items-center justify-between gap-3 px-4 py-3">
                    <p class="text-xs text-muted-foreground">
                        {t.llmNudge}
                    </p>
                    <Button
                        variant="outline"
                        size="sm"
                        onclick={() => {
                            void modelsStore.download(LLM_RUNTIME);
                            void modelsStore.download(LLM_WEIGHTS);
                        }}
                    >
                        {t.download}
                    </Button>
                </div>
            {/if}
        {/if}

        <SettingRow title={t.hotkey} description={t.hotkeySub}>
            <HotkeyCapture
                value={draft.record_hotkey}
                ariaLabel={t.hotkeyAria}
                onChange={(combo) => (draft.record_hotkey = combo)}
            />
        </SettingRow>
    </SettingsGroup>
</div>
