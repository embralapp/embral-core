<script lang="ts">
    import { onDestroy, onMount } from "svelte";
    import { listen, type UnlistenFn } from "@tauri-apps/api/event";
    import { configStore } from "$lib/stores/config.svelte";
    import { themeStore } from "$lib/stores/theme.svelte";
    import { loadFixture } from "$lib/fixture";
    import { copy } from "$lib/copy";

    const t = $derived(copy.dictation.overlay);

    // The dictation overlay window: never focused, always on top, shown by
    // the backend while a session runs. A mic spectrum + status row over the
    // words as they arrive: the same live-transcript feel as meetings,
    // shrunk to a pill ([dictation.md](../../../docs/dictation.md)).

    type Phase = "listening" | "finishing";
    let phase = $state<Phase>("listening");
    /** Committed words (finalized + the interim's stable part). */
    let text = $state("");
    /** The unstable trailing hypothesis: rendered dim, may change. */
    let tentative = $state("");
    /** Mic band magnitudes from the backend's LevelTap (~10 Hz). */
    let bands = $state<number[]>([]);

    function reset() {
        phase = "listening";
        text = "";
        tentative = "";
        bands = [];
    }

    // The words viewport, snapped to whole lines: the window is a fixed
    // logical size but scaling varies per monitor, so the largest multiple
    // of the line height that fits is measured, not hard-coded; otherwise
    // the descenders of the clipped line above (y, g, j) peek through at
    // the top.
    let areaHeight = $state(0);
    let paragraph = $state<HTMLParagraphElement | null>(null);
    let lineHeight = $state(0);
    $effect(() => {
        if (paragraph) {
            lineHeight = parseFloat(getComputedStyle(paragraph).lineHeight);
        }
    });
    let clipHeight = $derived(
        lineHeight > 0 && areaHeight > 0
            ? Math.floor(areaHeight / lineHeight) * lineHeight
            : areaHeight,
    );

    // The meeting meter's rendering rule (LevelRibbon.svelte), mic-only:
    // square-rooted with a visual gain so quiet speech still reads.
    const GAIN = 40;
    function height(v: number): number {
        return Math.min(1, Math.sqrt(v * GAIN)) * 100;
    }

    let unlisteners: UnlistenFn[] = [];

    onMount(async () => {
        await configStore.load();
        themeStore.apply(configStore.config?.theme ?? "system");
        // Staged screenshot moment (dev sandboxes only; $lib/fixture):
        // render the overlay mid-dictation without a session.
        const fixture = await loadFixture();
        if (fixture?.overlay) {
            phase = fixture.overlay.phase ?? "listening";
            text = fixture.overlay.text ?? "";
            tentative = fixture.overlay.tentative ?? "";
            bands = fixture.overlay.bands ?? [];
            // Deterministic hydration marker for the capture tooling.
            document.documentElement.dataset.fixture = "overlay";
        }
        unlisteners = await Promise.all([
            listen("dictation-started", reset),
            // Clear immediately when a session ends (delivered or cancelled):
            // the window is hidden and reused, and showing it again must
            // never flash the previous dictation's words.
            listen("dictation-complete", reset),
            listen("dictation-finishing", () => {
                phase = "finishing";
                bands = [];
            }),
            listen<{ text: string; tentative: string }>(
                "dictation-text",
                (e) => {
                    text = e.payload.text;
                    tentative = e.payload.tentative;
                },
            ),
            listen<number[]>("dictation-level", (e) => {
                bands = e.payload;
            }),
        ]);
    });

    onDestroy(() => {
        for (const fn of unlisteners) fn();
    });
</script>

<div
    class="flex h-screen w-screen flex-col overflow-hidden rounded-xl border border-border bg-background/95 px-4 pb-3 pt-3 text-foreground shadow-2xl"
>
    <!-- The spectrum hugs its bars on the left (not full-width; it is a
         glanceable signal, not the centerpiece); the status sits right. -->
    <div class="flex shrink-0 items-center justify-between gap-4">
        <div class="flex h-4 items-end gap-px" aria-hidden="true">
            {#each Array(24) as _, i (i)}
                <div class="flex h-full w-[3px] flex-col justify-end">
                    <div
                        class="w-full rounded-[1px] bg-foreground/70 transition-[height] duration-100 ease-linear"
                        style="height: {height(bands[i] ?? 0)}%"
                    ></div>
                </div>
            {/each}
        </div>
        <span class="shrink-0 text-xs font-medium text-muted-foreground">
            {phase === "listening" ? t.listening : t.finishing}
        </span>
    </div>

    <!-- Bottom-anchored and overflow-hidden: the newest words stay visible
         and older lines clip away at the top, tail-following without any
         scrollbar. The top margin keeps a full-height text block from
         touching the status row; the inner box is snapped to whole lines
         so a clipped line disappears entirely instead of leaving its
         descenders visible. -->
    <div
        class="mt-2 flex min-h-0 flex-1 flex-col justify-end"
        bind:clientHeight={areaHeight}
    >
        <div
            class="flex flex-col justify-end overflow-hidden"
            style="height: {clipHeight}px"
        >
            <!-- No whitespace between text and the tentative span: the
                 tail's own leading space (or its absence, mid-word) is the
                 word boundary; see the interim contract in types.ts. -->
            <p bind:this={paragraph} class="text-sm leading-snug">
                {text}{#if tentative}<span class="text-muted-foreground"
                        >{tentative}</span
                    >{/if}
            </p>
        </div>
    </div>
</div>
