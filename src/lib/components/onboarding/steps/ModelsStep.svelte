<script lang="ts">
    // "Recommended for this computer": system_specs → recommend.ts → two
    // switch rows (language, accuracy) over a checkbox list of download
    // units. What's checked is what downloads; features follow
    // which models end up present ([shell.md](../../../../../docs/shell.md),
    // [transcription.md](../../../../../docs/transcription.md)).
    import { onMount } from "svelte";
    import { invoke } from "@tauri-apps/api/core";
    import { Check } from "lucide-svelte";
    import { Button } from "$lib/components/ui/button";
    import { modelsStore } from "$lib/stores/models.svelte";
    import { formatBytes } from "$lib/utils/bytes";
    import { copy } from "$lib/copy";
    import type { TranscriptionLanguage } from "$lib/types";
    import type { OnboardingDraft } from "../types";
    import Segmented from "../Segmented.svelte";
    import {
        ASR_ACCURATE,
        ASR_BALANCED,
        ASR_FAST,
        ASR_MULTILINGUAL,
        EMBEDDING,
        LLM_RUNTIME,
        LLM_WEIGHTS,
        PUNCTUATION,
        SPEAKER_ID,
        diskWarning,
        missingBytes,
        recommend,
        recommendedLanguage,
        type SystemSpecs,
    } from "../recommend";

    let { draft }: { draft: OnboardingDraft } = $props();

    const t = $derived(copy.onboarding.models);

    let specs = $state<SystemSpecs | null>(null);
    // Per-unit selection; ASR/punctuation/speakers/search default in, the
    // summaries pair follows the RAM recommendation (seeded on mount).
    let wantAsr = $state(true);
    let wantPunct = $state(true);
    let wantSummaries = $state(false);
    let wantSpeakers = $state(true);
    let wantSearch = $state(true);

    const recommendedLang = recommendedLanguage(navigator.language);

    onMount(async () => {
        void modelsStore.refresh();
        try {
            specs = await invoke<SystemSpecs>("system_specs");
            // The recommendation is written to the draft immediately: continuing
            // without touching anything keeps the recommended setup.
            const rec = recommend(specs);
            draft.local_asr_model = rec.asr;
            draft.transcription_language = recommendedLang;
            wantSummaries = rec.llm;
        } catch (e) {
            console.error("system_specs failed:", e);
        }
    });

    let rec = $derived(specs ? recommend(specs) : null);
    let multilingual = $derived(draft.transcription_language === "multilingual");
    let chosenAsr = $derived(
        multilingual ? ASR_MULTILINGUAL : draft.local_asr_model,
    );
    // Zipformers need the punctuation pass; the parakeets punctuate
    // natively, so the row disappears under them.
    let needsPunct = $derived.by(() => {
        const m = modelsStore.status(chosenAsr);
        return m ? !m.native_punctuation : false;
    });

    interface Unit {
        key: string;
        title: string;
        why: string;
        ids: string[];
        present: boolean;
        downloading: boolean;
        checked: boolean;
        set: (v: boolean) => void;
    }

    let units = $derived.by((): Unit[] => {
        const mk = (
            key: string,
            title: string,
            why: string,
            ids: string[],
            checked: boolean,
            set: (v: boolean) => void,
        ): Unit | null => {
            const statuses = ids.map((id) => modelsStore.status(id));
            if (statuses.some((s) => !s)) return null;
            return {
                key,
                title,
                why,
                ids,
                present: statuses.every((s) => s!.present),
                downloading: ids.some((id) => modelsStore.isDownloading(id)),
                checked,
                set,
            };
        };
        const asrStatus = modelsStore.status(chosenAsr);
        return [
            mk(
                "asr",
                asrStatus?.display_name ?? t.units.asrTitle,
                multilingual ? t.units.asrWhyMultilingual : t.units.asrWhy,
                [chosenAsr],
                wantAsr,
                (v) => (wantAsr = v),
            ),
            ...(needsPunct
                ? [
                      mk(
                          "punct",
                          modelsStore.status(PUNCTUATION)?.display_name ??
                              t.units.punctTitle,
                          t.units.punctWhy,
                          [PUNCTUATION],
                          wantPunct,
                          (v) => (wantPunct = v),
                      ),
                  ]
                : []),
            mk(
                "summaries",
                t.units.summariesTitle,
                t.units.summariesWhy,
                [LLM_RUNTIME, LLM_WEIGHTS],
                wantSummaries,
                (v) => (wantSummaries = v),
            ),
            mk(
                "speakers",
                modelsStore.status(SPEAKER_ID)?.display_name ??
                    t.units.speakersTitle,
                t.units.speakersWhy,
                [SPEAKER_ID],
                wantSpeakers,
                (v) => (wantSpeakers = v),
            ),
            mk(
                "search",
                modelsStore.status(EMBEDDING)?.display_name ?? t.units.searchTitle,
                t.units.searchWhy,
                [EMBEDDING],
                wantSearch,
                (v) => (wantSearch = v),
            ),
        ].filter((u): u is Unit => u !== null);
    });

    // Button state is computed over the units that still have something to
    // download; present units are checked-and-locked decoration.
    let downloadable = $derived(units.filter((u) => !u.present));
    let checkedUnits = $derived(downloadable.filter((u) => u.checked));
    let selectedIds = $derived(checkedUnits.flatMap((u) => u.ids));
    let toDownload = $derived(
        selectedIds.filter((id) => {
            const m = modelsStore.status(id);
            return m && !m.present && !modelsStore.isDownloading(id);
        }),
    );
    let downloadBytes = $derived(missingBytes(selectedIds, modelsStore.statuses));
    let allPresent = $derived(units.length > 0 && downloadable.length === 0);
    let anyDownloading = $derived(units.some((u) => u.downloading));
    let warnDisk = $derived(specs ? diskWarning(specs, downloadBytes) : false);
    let buttonLabel = $derived(
        checkedUnits.length === 0
            ? t.downloadNone
            : checkedUnits.length === downloadable.length
              ? t.downloadAll(formatBytes(downloadBytes))
              : t.downloadSelected(formatBytes(downloadBytes)),
    );

    function downloadSelected() {
        for (const id of toDownload) void modelsStore.download(id);
    }

    /** Combined percent across a unit's models, weighted by size. */
    function unitPct(ids: string[]): number {
        let done = 0;
        let total = 0;
        for (const id of ids) {
            const m = modelsStore.status(id);
            if (!m) continue;
            total += m.total_bytes;
            done += m.present
                ? m.total_bytes
                : (modelsStore.fraction(id) ?? 0) * m.total_bytes;
        }
        return total > 0 ? Math.round((done / total) * 100) : 0;
    }

    /** A unit's still-to-download size. */
    function unitBytes(ids: string[]): number {
        return missingBytes(ids, modelsStore.statuses);
    }

    let languageOptions = $derived([
        { value: "english", label: t.languageOptions.english },
        { value: "multilingual", label: t.languageOptions.multilingual },
    ]);
    let tierOptions = $derived([
        { value: ASR_FAST, label: t.tierOptions.fast },
        { value: ASR_BALANCED, label: t.tierOptions.balanced },
        { value: ASR_ACCURATE, label: t.tierOptions.accurate },
    ]);
</script>

<h1 class="font-display text-2xl tracking-tight">{t.title}</h1>

{#if specs && rec}
    <p class="mt-3 text-sm text-muted-foreground">
        {t.intro}
    </p>

    <div class="mt-5 space-y-2.5">
        <div class="flex items-center justify-between gap-3">
            <p class="text-sm font-medium">{t.language}</p>
            <Segmented
                options={languageOptions}
                value={draft.transcription_language}
                recommended={recommendedLang}
                onSelect={(v) =>
                    (draft.transcription_language =
                        v as TranscriptionLanguage)}
            />
        </div>
        {#if !multilingual}
            <div class="flex items-center justify-between gap-3">
                <p class="text-sm font-medium">{t.accuracy}</p>
                <Segmented
                    options={tierOptions}
                    value={draft.local_asr_model}
                    recommended={rec.asr}
                    onSelect={(v) => (draft.local_asr_model = v)}
                />
            </div>
        {/if}
    </div>

    {#if units.length > 0}
    <div class="mt-5 rounded-lg border border-border bg-card">
        <div class="divide-y divide-border">
            {#each units as u (u.key)}
                <label
                    class="flex cursor-pointer items-center gap-3 px-4 py-2.5"
                >
                    <input
                        type="checkbox"
                        class="accent-primary"
                        checked={u.present || u.checked}
                        disabled={u.present}
                        onchange={(e) => u.set(e.currentTarget.checked)}
                    />
                    <div class="min-w-0 flex-1">
                        <p class="text-sm font-medium">{u.title}</p>
                        <p class="text-xs text-muted-foreground">{u.why}</p>
                    </div>
                    <span class="shrink-0 text-xs text-muted-foreground">
                        {#if u.present}
                            <Check size={14} class="text-primary" />
                        {:else if u.downloading}
                            {unitPct(u.ids)}%
                        {:else}
                            {formatBytes(unitBytes(u.ids))}
                        {/if}
                    </span>
                </label>
            {/each}
        </div>
        <div class="border-t border-border p-3">
            {#if allPresent}
                <p class="flex items-center gap-1.5 text-sm text-primary">
                    <Check size={15} /> {t.ready}
                </p>
            {:else if anyDownloading && toDownload.length === 0}
                <p class="text-sm text-muted-foreground">
                    {t.downloadingBackground}
                </p>
            {:else}
                <Button
                    size="sm"
                    disabled={checkedUnits.length === 0}
                    onclick={downloadSelected}
                >
                    {buttonLabel}
                </Button>
            {/if}
            {#if warnDisk}
                <p class="mt-2 text-xs text-destructive">
                    {t.lowSpace}
                </p>
            {/if}
        </div>
    </div>
    {/if}
{:else}
    <p class="mt-3 text-sm text-muted-foreground">{t.checking}</p>
{/if}
