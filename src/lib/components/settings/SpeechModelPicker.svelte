<script lang="ts">
    import { appState } from "$lib/stores/app-state.svelte";
    import { modelsStore } from "$lib/stores/models.svelte";
    import {
        ASR_ACCURATE,
        ASR_BALANCED,
        ASR_FAST,
        MULTILINGUAL_ASR_MODEL,
    } from "$lib/utils/asrModel";
    import { formatBytes } from "$lib/utils/bytes";
    import type { TranscriptionLanguage } from "$lib/types";
    import SettingRow from "./SettingRow.svelte";
    import { Button } from "$lib/components/ui/button";
    import { copy } from "$lib/copy";

    const t = $derived(copy.settings.transcription.picker);

    /** The on-device accuracy choice, projected onto the catalog's model ids.
     * The language lives one level up (it governs cloud transcription too), so
     * this component only answers "how accurate, on this machine". */
    let {
        value,
        onChange,
        language,
    }: {
        /** The configured English model id. */
        value: string;
        onChange: (id: string) => void;
        language: TranscriptionLanguage;
    } = $props();

    // The order is fixed; the labels come from the catalog by key.
    const tierKeys = ["fast", "balanced", "accurate"] as const;

    const tier = $derived.by(() => {
        if (value === ASR_FAST) return 0;
        if (value === ASR_ACCURATE) return 2;
        return 1; // the balanced model and anything unmapped
    });

    // There is exactly one multilingual model in the catalog, so there is no
    // tier to choose between; the language decides the model outright.
    const modelId = $derived(
        language === "multilingual" ? MULTILINGUAL_ASR_MODEL : value,
    );

    function modelForTier(t: number): string {
        return t <= 0 ? ASR_FAST : t === 1 ? ASR_BALANCED : ASR_ACCURATE;
    }

    function setTier(t: number) {
        onChange(modelForTier(t));
    }

    const status = $derived(modelsStore.status(modelId));
    const downloading = $derived(modelsStore.isDownloading(modelId));
    const pct = $derived(Math.round((modelsStore.fraction(modelId) ?? 0) * 100));

</script>

<SettingRow title={t.label}>
    {#snippet descriptionExtra()}
        <button
            class="text-xs text-muted-foreground underline-offset-2 hover:text-foreground hover:underline"
            onclick={() => appState.openSettings("transcription")}
        >
            {t.manageModels}
        </button>
    {/snippet}
    <div class="tier flex w-56 flex-col gap-1.5">
        <input
            type="range"
            min="0"
            max="2"
            step="1"
            value={tier}
            oninput={(e) => setTier(Number(e.currentTarget.value))}
            class="tier-slider"
            aria-label={t.accuracyAria}
        />
        <!-- Fast hugs the bar's left edge, Accurate its right, Balanced sits
             centered within it, anchored to the bar, not the thumb stops. -->
        <div class="relative flex items-baseline justify-between text-[11px]">
            {#each tierKeys as key, i (key)}
                <button
                    class="whitespace-nowrap transition-colors {i === 1
                        ? 'absolute left-1/2 -translate-x-1/2'
                        : ''} {tier === i
                        ? 'font-medium text-foreground'
                        : 'text-muted-foreground hover:text-foreground'}"
                    onclick={() => setTier(i)}
                >
                    {t.tiers[key]}
                </button>
            {/each}
        </div>
    </div>
</SettingRow>

{#if status && !status.present}
    <div class="flex items-center justify-between gap-3 px-4 py-3">
        <p class="text-xs text-muted-foreground">
            {#if downloading}
                {t.downloading(pct)}
            {:else}
                {t.needsDownload}
            {/if}
        </p>
        {#if !downloading}
            <Button size="sm" onclick={() => modelsStore.download(modelId)}>
                {t.download(formatBytes(status.total_bytes))}
            </Button>
        {/if}
    </div>
    {#if downloading}
        <div class="px-4 pb-3">
            <div class="h-1.5 w-full overflow-hidden rounded-full bg-muted">
                <div
                    class="h-full bg-primary transition-all duration-300"
                    style="width: {pct}%"
                ></div>
            </div>
        </div>
    {/if}
    {#if modelsStore.error(modelId)}
        <p class="px-4 pb-3 text-xs text-destructive">{modelsStore.error(modelId)}</p>
    {/if}
{/if}

<style>
    /* The thumb width, shared by the track and the label positions: a native
       range thumb's centre never reaches the ends, it insets by half itself. */
    .tier {
        --thumb: 14px;
    }
    .tier-slider {
        appearance: none;
        -webkit-appearance: none;
        height: 4px;
        border-radius: 9999px;
        background: var(--muted);
        outline: none;
        cursor: pointer;
    }
    .tier-slider::-webkit-slider-thumb {
        appearance: none;
        -webkit-appearance: none;
        height: var(--thumb);
        width: var(--thumb);
        border-radius: 9999px;
        background: var(--foreground);
        border: none;
        transition: transform 150ms ease-out;
    }
    .tier-slider::-webkit-slider-thumb:hover {
        transform: scale(1.15);
    }
</style>
