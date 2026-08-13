<script lang="ts">
    /**
     * "These are the same person": pick the name that stays; the others
     * merge into it. Hand-rolled like ConfirmDialog and for the same reason:
     * a small two-button question, not a modal surface.
     */
    import type { SpeakerProfile } from "$lib/types";
    import { copy } from "$lib/copy";

    const t = $derived(copy.speakers.merge);
    const tc = $derived(copy.common);

    let {
        open = $bindable(false),
        profiles,
        busy = false,
        onConfirm,
    }: {
        open?: boolean;
        profiles: SpeakerProfile[];
        busy?: boolean;
        onConfirm: (targetId: string) => void;
    } = $props();

    let targetId = $state<string | null>(null);

    // Default survivor: whoever was in a meeting most recently; the name
    // in active use is the one worth keeping.
    $effect(() => {
        if (!open) {
            targetId = null;
            return;
        }
        if (targetId && profiles.some((p) => p.id === targetId)) return;
        const byActivity = [...profiles].sort((a, b) =>
            (b.last_seen ?? b.created_at).localeCompare(a.last_seen ?? a.created_at),
        );
        targetId = byActivity[0]?.id ?? null;
    });

    function onKeydown(e: KeyboardEvent) {
        if (!open) return;
        if (e.key === "Escape") {
            e.preventDefault();
            open = false;
        }
    }
</script>

<svelte:window onkeydown={onKeydown} />

{#if open}
    <div class="fixed inset-0 z-50 flex items-center justify-center bg-black/35 px-4">
        <div class="w-full max-w-sm rounded-md border border-border bg-background p-4 shadow-lg">
            <h3 class="text-sm font-semibold">{t.title(profiles.length)}</h3>
            <p class="mt-2 text-sm text-muted-foreground">{t.keep}</p>
            <div class="mt-2 space-y-0.5">
                {#each profiles as p (p.id)}
                    <label
                        class="flex cursor-pointer items-center gap-2 rounded-md px-2 py-1.5 text-sm transition-colors hover:bg-accent/40"
                    >
                        <input
                            type="radio"
                            name="merge-target"
                            value={p.id}
                            checked={targetId === p.id}
                            onchange={() => (targetId = p.id)}
                            class="accent-primary"
                        />
                        <span class="min-w-0 truncate">{p.name}</span>
                    </label>
                {/each}
            </div>
            <p class="mt-3 text-xs text-muted-foreground">{t.body}</p>
            <div class="mt-4 flex justify-end gap-2">
                <button
                    onclick={() => (open = false)}
                    class="h-9 rounded-md border border-border px-3 text-sm font-medium transition-colors hover:bg-accent"
                    disabled={busy}
                >
                    {tc.cancel}
                </button>
                <button
                    onclick={() => targetId && onConfirm(targetId)}
                    class="h-9 rounded-md bg-primary px-3 text-sm font-medium text-primary-foreground transition-colors hover:bg-primary/90 disabled:opacity-50"
                    disabled={busy || !targetId}
                >
                    {t.confirm}
                </button>
            </div>
        </div>
    </div>
{/if}
