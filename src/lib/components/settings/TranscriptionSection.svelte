<script lang="ts">
    import { onMount } from "svelte";
    import { X } from "lucide-svelte";
    import type { AppConfig } from "$lib/types";
    import { modelsStore } from "$lib/stores/models.svelte";
    import ModelCard from "./ModelCard.svelte";
    import SettingsGroup from "./SettingsGroup.svelte";
    import SettingRow from "./SettingRow.svelte";
    import { Input } from "$lib/components/ui/input";
    import { copy } from "$lib/copy";

    let { draft }: { draft: AppConfig } = $props();

    const t = $derived(copy.settings.transcription);

    let vocabularyInput = $state("");

    onMount(() => {
        modelsStore.refresh();
    });

    let selectedModel = $derived(modelsStore.status(draft.local_asr_model));
    // The manageable library, as card grids by what the model does.
    // Selection happens on the Meetings and Dictation pages; this page
    // manages the files.
    let speechModels = $derived(
        modelsStore.statuses.filter(
            (m) => m.kind === "streaming_asr" || m.kind === "offline_asr",
        ),
    );
    let supportingModels = $derived(
        modelsStore.statuses.filter(
            (m) => m.kind === "punctuation" || m.kind === "speaker_id",
        ),
    );

    function addVocabularyWord() {
        const word = vocabularyInput.trim();
        if (!word) return;
        if (!draft.vocabulary.includes(word)) {
            draft.vocabulary = [...draft.vocabulary, word];
        }
        vocabularyInput = "";
    }

    function removeVocabularyWord(word: string) {
        draft.vocabulary = draft.vocabulary.filter((w) => w !== word);
    }
</script>

<div class="space-y-6">
    <div>
        <p
            class="pb-2 text-xs font-medium tracking-wider text-muted-foreground uppercase"
        >
            {t.speechRecognition}
        </p>
        <div class="grid grid-cols-1 gap-3 sm:grid-cols-2">
            {#each speechModels as model (model.id)}
                <ModelCard {model} />
            {/each}
        </div>
    </div>

    <div>
        <p
            class="pb-2 text-xs font-medium tracking-wider text-muted-foreground uppercase"
        >
            {t.supportingModels}
        </p>
        <div class="grid grid-cols-1 gap-3 sm:grid-cols-2">
            {#each supportingModels as model (model.id)}
                <ModelCard {model} />
            {/each}
        </div>
    </div>

    <!-- Hidden entirely for models the runtime can't boost: a control
         that can't do anything is noise, and the list is kept in config
         for models that can. -->
    {#if selectedModel?.supports_hotwords !== false}
        <SettingsGroup label={t.vocabulary._group}>
            <SettingRow
                title={t.vocabulary.custom.label}
                description={t.vocabulary.custom.sub}
                vertical
            >
                <Input
                    bind:value={vocabularyInput}
                    onkeydown={(e) => {
                        if (e.key === "Enter") {
                            e.preventDefault();
                            addVocabularyWord();
                        }
                    }}
                    placeholder={t.vocabulary.custom.placeholder}
                    class="max-w-sm"
                />
                {#if draft.vocabulary.length > 0}
                    <div class="mt-3 flex max-w-xl flex-wrap gap-2">
                        {#each draft.vocabulary as word (word)}
                            <span
                                class="inline-flex items-center gap-1.5 rounded-full bg-muted py-1 pr-1.5 pl-2.5 text-xs"
                            >
                                {word}
                                <button
                                    type="button"
                                    class="rounded-full p-0.5 text-muted-foreground transition-colors hover:bg-background hover:text-foreground"
                                    aria-label={t.vocabulary.remove(word)}
                                    onclick={() => removeVocabularyWord(word)}
                                >
                                    <X size={11} />
                                </button>
                            </span>
                        {/each}
                    </div>
                {/if}
            </SettingRow>
        </SettingsGroup>
    {/if}
</div>
