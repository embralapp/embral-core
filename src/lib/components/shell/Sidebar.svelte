<script lang="ts">
	import { errorMessage } from '$lib/copy/errors';
    import { invoke } from "@tauri-apps/api/core";
    import {
        Circle,
        Mic,
        Menu,
        NotebookPen,
        BookUser,
        Speech,
        Settings,
    } from "lucide-svelte";
    import { appState, type AppView } from "$lib/stores/app-state.svelte";
    import { configStore } from "$lib/stores/config.svelte";
    import Tip from "$lib/components/Tip.svelte";
    import { CLOUD_ENABLED, loadHoursRing } from "$lib/cloud";
    import { copy } from "$lib/copy";
    import { cn } from "$lib/utils";

    const t = $derived(copy.shell.sidebar);

    // The rail expands/collapses via the hamburger at the bottom (persisted
    // as config.sidebar_expanded).
    let starting = $state(false);

    let expanded = $derived(configStore.config?.sidebar_expanded ?? false);

    // The recording view is its own destination (the record button); the
    // Meetings item covers the library and the import-processing view, and
    // stays reachable while a recording runs in the background.
    const meetingViews = ["idle", "processing"];

    // The order is this component's; the names are the catalog's.
    type NavKey = keyof typeof copy.shell.sidebar.nav;
    const navItems: { view: AppView; key: NavKey; icon: typeof Mic }[] = [
        { view: "idle", key: "meetings", icon: NotebookPen },
        { view: "speakers", key: "speakers", icon: BookUser },
        { view: "dictation", key: "dictation", icon: Speech },
    ];

    function isActive(item: (typeof navItems)[number]): boolean {
        if (item.view === "idle") return meetingViews.includes(appState.view);
        return appState.view === item.view;
    }

    function navigate(item: (typeof navItems)[number]) {
        appState.setView(item.view);
    }

    async function onRecordClick() {
        if (appState.isRecording) {
            // Never stop from here: one stray click must not end a meeting.
            appState.setView("recording");
            return;
        }
        if (starting || !configStore.isConfigured) return;
        starting = true;
        try {
            await invoke("start_recording");
        } catch (e) {
            appState.setError(errorMessage(e));
        } finally {
            starting = false;
        }
    }

    async function toggleExpanded() {
        const cfg = configStore.config;
        if (!cfg) return;
        await configStore.save({ ...cfg, sidebar_expanded: !expanded });
    }

    const rowClass =
        "flex h-10 w-full items-center gap-3 overflow-hidden rounded-lg px-[11px] text-sidebar-foreground/60 transition-colors hover:bg-sidebar-accent hover:text-sidebar-foreground";
    const activeClass = "bg-sidebar-accent text-sidebar-foreground";
    const labelClass = (show: boolean) =>
        cn(
            "min-w-0 truncate text-sm transition-[opacity,transform] duration-200",
            show ? "opacity-100" : "-translate-x-1 opacity-0",
        );
</script>

<nav
    class={cn(
        "flex shrink-0 flex-col justify-between overflow-hidden border-r border-sidebar-border bg-sidebar px-2 py-2 transition-[width] duration-200 ease-out",
        expanded ? "w-52" : "w-[54px]",
    )}
>
    <div class="flex flex-col gap-1">
        <!-- Record: the rail's headline action, and the recording view's own
             nav item while a recording runs (the rest of the app stays
             browsable; this button is the way back to the live view). -->
        <Tip
            side="right"
            text={appState.isRecording
                ? t.recordTip.recording
                : configStore.isConfigured
                  ? t.recordTip.ready
                  : t.recordTip.notConfigured}
        >
            {#snippet children({ props })}
                <button
                    {...props}
                    class={cn(
                        rowClass,
                        appState.shadowMode
                            ? "text-muted-foreground hover:text-muted-foreground"
                            : appState.isRecording
                              ? "text-destructive hover:text-destructive"
                              : "bg-primary text-primary-foreground hover:bg-primary/90 hover:text-primary-foreground",
                        appState.isRecording &&
                            appState.view === "recording" &&
                            "bg-sidebar-accent",
                        (!configStore.isConfigured || starting) &&
                            !appState.isRecording &&
                            "cursor-default opacity-50 hover:bg-primary",
                    )}
                    onclick={onRecordClick}
                >
                    <!-- Shadow mode ([shell.md] §Recording): the rail keeps
                         its slot and its click target, but a small neutral
                         dot where the red microphone was. It reads as a UI
                         mark rather than an alarm: the point is that a
                         glance at the screen does not announce a meeting is
                         being recorded. Hovering still tells the truth. -->
                    {#if appState.shadowMode}
                        <span
                            class="flex size-[18px] shrink-0 items-center justify-center"
                        >
                            <Circle size={9} fill="currentColor" />
                        </span>
                    {:else}
                        <Mic size={18} class="shrink-0" />
                    {/if}
                    <span class={labelClass(expanded)}>
                        {appState.shadowMode
                            ? t.recordLabel.shadow
                            : appState.isRecording
                              ? t.recordLabel.recording
                              : t.recordLabel.idle}
                    </span>
                </button>
            {/snippet}
        </Tip>

        <div class="mx-1 my-1 border-t border-sidebar-border"></div>

        {#each navItems as item (item.view)}
            <Tip side="right" text={t.nav[item.key]}>
                {#snippet children({ props })}
                    <button
                        {...props}
                        class={cn(rowClass, isActive(item) && activeClass)}
                        aria-label={t.nav[item.key]}
                        onclick={() => navigate(item)}
                    >
                        <item.icon size={18} class="shrink-0" />
                        <span class={labelClass(expanded)}>{t.nav[item.key]}</span>
                    </button>
                {/snippet}
            </Tip>
        {/each}
    </div>

    <div class="flex flex-col gap-1">
        <!-- The cloud hours meter, in cloud builds only. It loads the
             cloud-only code the way everything else does: through $lib/cloud,
             never by naming the directory (cloud-seam.md). It renders nothing
             when there is no account, so the rail is unchanged for offline
             users. -->
        {#if CLOUD_ENABLED}
            {#await loadHoursRing() then Ring}
                {#if Ring}
                    <Ring
                        rowClass={cn(rowClass, "hover:text-sidebar-foreground")}
                        labelClass={labelClass(expanded)}
                    />
                {/if}
            {/await}
        {/if}
        <Tip side="right" text={t.settings}>
            {#snippet children({ props })}
                <button
                    {...props}
                    class={cn(rowClass, appState.view === "settings" && activeClass)}
                    aria-label={t.settings}
                    onclick={() => appState.setView("settings")}
                >
                    <Settings size={18} class="shrink-0" />
                    <span class={labelClass(expanded)}>{t.settings}</span>
                </button>
            {/snippet}
        </Tip>
        <Tip
            side="right"
            text={expanded ? t.collapseTip.expanded : t.collapseTip.collapsed}
        >
            {#snippet children({ props })}
                <button
                    {...props}
                    class={rowClass}
                    aria-label={expanded
                        ? t.collapseTip.expanded
                        : t.collapseTip.collapsed}
                    aria-expanded={expanded}
                    onclick={toggleExpanded}
                >
                    <Menu size={18} class="shrink-0" />
                    <span class={labelClass(expanded)}>{t.collapseLabel}</span>
                </button>
            {/snippet}
        </Tip>
    </div>
</nav>
