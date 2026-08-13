<script lang="ts">
    import { getCurrentWindow } from "@tauri-apps/api/window";
    import { Minus, Search, Square, X } from "lucide-svelte";
    import EmbralIcon from "$lib/components/EmbralIcon.svelte";
    import { copy } from "$lib/copy";
    import { isMac } from "$lib/platform";

    let { onSearch }: { onSearch: () => void } = $props();

    const t = $derived(copy.shell.titleBar);

    const win = getCurrentWindow();
</script>

<!-- On macOS the native traffic lights render top-left (titleBarStyle
     Overlay), so the brand block shifts right of them and the custom
     window buttons don't exist. -->
<header
    data-tauri-drag-region
    class={[
        "grid h-11 shrink-0 select-none grid-cols-[1fr_minmax(0,26rem)_1fr] items-center border-b border-border bg-sidebar",
        isMac ? "pl-20" : "pl-3",
    ]}
>
    <div data-tauri-drag-region class="flex items-center gap-2">
        <EmbralIcon size={16} />
        <span data-tauri-drag-region class="text-sm font-semibold tracking-tight"
            >embral</span
        >
    </div>

    <!-- The command bar: a quiet, always-visible entry point to the palette
         (the palette itself stays the Ctrl+K overlay). Not a drag region. -->
    <button
        class="mx-4 flex h-7 min-w-0 items-center gap-2 rounded-md border border-border/60 bg-background/40 px-2.5 text-xs text-muted-foreground transition-colors hover:border-border hover:bg-background/70 hover:text-foreground"
        onclick={onSearch}
    >
        <Search size={12} class="shrink-0 opacity-70" />
        <span class="min-w-0 truncate">{t.commandBar.placeholder}</span>
        <kbd
            class="ml-auto shrink-0 rounded border border-border/60 bg-muted/60 px-1 font-mono text-[10px] leading-4"
            >{t.commandBar.shortcut}</kbd
        >
    </button>

    <!-- The right column: its empty space (left of the window controls) must
         drag the window too; without the attribute here it fell on a plain
         div and stayed dead. The buttons carry no drag attribute, so they
         still click. -->
    <div data-tauri-drag-region class="flex h-full items-center justify-end">
        {#if !isMac}
            <!-- Window controls: fixed-size hit targets, no drag region.
                 macOS uses the native traffic lights instead. -->
            <div class="flex h-full items-stretch">
                <button
                    class="flex w-11 items-center justify-center text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
                    aria-label={t.minimize}
                    onclick={() => win.minimize()}
                >
                    <Minus size={15} />
                </button>
                <button
                    class="flex w-11 items-center justify-center text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
                    aria-label={t.maximize}
                    onclick={() => win.toggleMaximize()}
                >
                    <Square size={12} />
                </button>
                <!-- No hover label, unlike most icon buttons: a window's ✕
                     needs no explaining, and a tooltip on the one control
                     the mouse crosses on its way out of the window fires
                     constantly. Its siblings carry none either; the
                     aria-label stays, since that is the name, not a tip. -->
                <button
                    class="flex w-11 items-center justify-center text-muted-foreground transition-colors hover:bg-destructive hover:text-white"
                    aria-label={t.close}
                    onclick={() => win.close()}
                >
                    <X size={15} />
                </button>
            </div>
        {/if}
    </div>
</header>
