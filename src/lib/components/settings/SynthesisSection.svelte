<script lang="ts">
    import { onMount } from "svelte";
    import type { AppConfig } from "$lib/types";
    import ModelCard from "./ModelCard.svelte";
    import SettingsGroup from "./SettingsGroup.svelte";
    import SettingRow from "./SettingRow.svelte";
    import { Input } from "$lib/components/ui/input";
    import { Switch } from "$lib/components/ui/switch";
    import { modelsStore } from "$lib/stores/models.svelte";
    import { usesLocalLlm } from "$lib/utils/llmUsage";
    import { copy } from "$lib/copy";

    let { draft }: { draft: AppConfig } = $props();

    const t = $derived(copy.settings.synthesis);

    onMount(() => {
        modelsStore.refresh();
    });

    // This page manages the on-device model's files and runtime behavior.
    // Choosing an engine lives with its feature: summaries on Meetings,
    // cleanup on Dictation.
    let llmModels = $derived(modelsStore.statuses.filter((m) => m.kind === "llm"));
    // The semantic-search model sits here with the other text-understanding
    // models; the speech page keeps only things that hear.
    let searchModels = $derived(
        modelsStore.statuses.filter((m) => m.kind === "embedding"),
    );

    // The performance knobs only mean something while a summary or cleanup
    // engine actually lives on the device; the backend stops honoring
    // keep-warm at the same boundary (llm::uses_local_llm).
    let localLlmInUse = $derived(usesLocalLlm(draft));
</script>

<div class="space-y-6">
    <div>
        <p
            class="pb-2 text-xs font-medium tracking-wider text-muted-foreground uppercase"
        >
            {t.localLlm}
        </p>
        <div class="grid grid-cols-1 gap-3 sm:grid-cols-2">
            {#each llmModels as model (model.id)}
                <ModelCard {model} />
            {/each}
        </div>
    </div>

    <div>
        <p
            class="pb-2 text-xs font-medium tracking-wider text-muted-foreground uppercase"
        >
            {t.search}
        </p>
        <div class="grid grid-cols-1 gap-3 sm:grid-cols-2">
            {#each searchModels as model (model.id)}
                <ModelCard {model} />
            {/each}
        </div>
    </div>

    {#if localLlmInUse}
        <SettingsGroup label={t.performance._group}>
            <SettingRow
                title={t.performance.keepWarm.label}
                description={t.performance.keepWarm.sub}
            >
                <Switch bind:checked={draft.llm_keep_warm} />
            </SettingRow>
            <SettingRow title={t.performance.unloadIdle.label}>
                <div class="flex items-center gap-2">
                    <Input
                        type="number"
                        min="1"
                        bind:value={draft.llm_idle_minutes}
                        disabled={draft.llm_keep_warm}
                        class="w-16 text-right"
                    />
                    <span class="text-xs text-muted-foreground"
                        >{t.performance.unloadIdle.unit}</span
                    >
                </div>
            </SettingRow>
        </SettingsGroup>
    {/if}
</div>
