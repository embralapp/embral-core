<script lang="ts">
    import { onMount } from "svelte";
    import { Copy, Mic, Speech, Square, Trash2 } from "lucide-svelte";
    import { dictationStore } from "$lib/stores/dictation.svelte";
    import { configStore } from "$lib/stores/config.svelte";
    import { appState } from "$lib/stores/app-state.svelte";
    import { Button } from "$lib/components/ui/button";
    import Tip from "$lib/components/Tip.svelte";
    import { formatCombo } from "$lib/utils/hotkey";
    import { copy as catalog } from "$lib/copy";
    import { cn } from "$lib/utils";
    import { flash } from "$lib/utils/flash";

    const t = $derived(catalog.dictation.home);

    onMount(() => {
        void dictationStore.refresh();
    });

    // Which rows show the raw transcript instead of the cleaned text.
    let showRaw = $state<Record<number, boolean>>({});
    let copiedId = $state<number | null>(null);
    let rowEls = $state<Record<number, HTMLElement>>({});

    /** A search result opened this page for one entry: scroll to it and mark
     * it, once the history it lives in has actually loaded. */
    $effect(() => {
        const id = dictationStore.pendingLanding;
        if (id === null) return;
        const el = rowEls[id];
        if (!el) return;
        dictationStore.takeLanding();
        el.scrollIntoView({ block: "center", behavior: "smooth" });
        flash(el);
    });

    let hotkey = $derived(configStore.config?.dictation_hotkey ?? "");

    function textOf(d: { id: number; raw_text: string; cleaned_text: string | null }): string {
        return showRaw[d.id] || !d.cleaned_text ? d.raw_text : d.cleaned_text;
    }

    async function copy(d: { id: number; raw_text: string; cleaned_text: string | null }) {
        await navigator.clipboard.writeText(textOf(d));
        copiedId = d.id;
        setTimeout(() => (copiedId = null), 1200);
    }

    function formatWhen(iso: string): string {
        const date = new Date(iso);
        return date.toLocaleString(undefined, {
            month: "short",
            day: "numeric",
            hour: "numeric",
            minute: "2-digit",
        });
    }
</script>

<div class="flex min-h-0 flex-1 flex-col">
    <div class="flex shrink-0 items-center justify-between border-b border-border px-4 py-3">
        <h2 class="font-display text-base tracking-tight">{t.title}</h2>

        <!-- The hotkey is shown, not set: clicking it opens the dictation
             settings page where the capture control lives. -->
        <div class="flex shrink-0 items-center gap-2">
            <Tip text={t.hotkeyTip}>
                {#snippet children({ props })}
                    <button
                        {...props}
                        class="min-w-28 rounded-md border border-input px-3 py-1.5 font-mono text-xs text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
                        aria-label={t.hotkeyAria}
                        onclick={() => appState.openSettings("dictation")}
                    >
                        {formatCombo(hotkey) || t.setHotkey}
                    </button>
                {/snippet}
            </Tip>
            <Button
                size="sm"
                variant={dictationStore.active ? "destructive" : "default"}
                onclick={() =>
                    dictationStore.active ? dictationStore.stop() : dictationStore.start()}
            >
                {#if dictationStore.active}
                    <Square size={13} class="mr-1" /> {t.stop}
                {:else}
                    <Mic size={13} class="mr-1" /> {t.dictate}
                {/if}
            </Button>
        </div>
    </div>

    {#if dictationStore.error}
        <p class="shrink-0 border-b border-border bg-destructive/5 px-4 py-2 text-xs text-destructive">
            {dictationStore.error}
        </p>
    {/if}

    <div class="min-h-0 flex-1 overflow-y-auto p-3">
        {#if dictationStore.history.length === 0}
            <div class="flex h-full flex-col items-center justify-center gap-3 text-center">
                <Speech size={26} class="text-muted-foreground/60" />
                <div class="max-w-sm space-y-1">
                    <p class="text-sm font-medium">{t.emptyTitle}</p>
                    <p class="text-xs leading-relaxed text-muted-foreground">
                        {t.emptyBody}
                    </p>
                </div>
                <button
                    class="text-xs text-primary underline-offset-2 hover:underline"
                    onclick={() => appState.openSettings("dictation")}
                >
                    {t.openSettings}
                </button>
            </div>
        {:else}
            <div class="mx-auto max-w-2xl space-y-2">
                {#each dictationStore.history as d (d.id)}
                    <div
                        bind:this={rowEls[d.id]}
                        class="group rounded-lg border border-border p-3"
                    >
                        <div class="flex items-center gap-2 text-[11px] text-muted-foreground">
                            <span>{formatWhen(d.created_at)}</span>
                            {#if d.app}
                                <span class="rounded bg-muted px-1.5 py-0.5">{d.app}</span>
                            {/if}
                            {#if d.cleaned_text}
                                <button
                                    class={cn(
                                        "rounded px-1.5 py-0.5 transition-colors",
                                        showRaw[d.id]
                                            ? "bg-muted"
                                            : "bg-primary/10 text-primary",
                                    )}
                                    onclick={() => (showRaw[d.id] = !showRaw[d.id])}
                                >
                                    {showRaw[d.id] ? t.raw : t.cleaned}
                                </button>
                            {/if}
                            <span class="flex-1"></span>
                            <div
                                class="flex items-center gap-0.5 opacity-0 transition-opacity group-hover:opacity-100"
                            >
                                <Tip text={t.copy}>
                                    {#snippet children({ props })}
                                        <button
                                            {...props}
                                            class="rounded p-1 text-muted-foreground hover:bg-accent hover:text-foreground"
                                            aria-label={t.copy}
                                            onclick={() => copy(d)}
                                        >
                                            <Copy size={12} />
                                        </button>
                                    {/snippet}
                                </Tip>
                                <Tip text={t.delete}>
                                    {#snippet children({ props })}
                                        <button
                                            {...props}
                                            class="rounded p-1 text-muted-foreground hover:bg-destructive/10 hover:text-destructive"
                                            aria-label={t.delete}
                                            onclick={() => dictationStore.remove(d.id)}
                                        >
                                            <Trash2 size={12} />
                                        </button>
                                    {/snippet}
                                </Tip>
                            </div>
                        </div>
                        <p class="mt-1.5 text-sm leading-relaxed">
                            {textOf(d)}
                        </p>
                        {#if copiedId === d.id}
                            <p class="mt-1 text-[10px] text-primary">{t.copied}</p>
                        {/if}
                    </div>
                {/each}
            </div>
        {/if}
    </div>
</div>
