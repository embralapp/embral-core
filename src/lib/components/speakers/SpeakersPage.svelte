<script lang="ts">
    import { onMount } from "svelte";
    import { Plus, Trash2, Users, Merge } from "lucide-svelte";
    import type { SpeakerProfile } from "$lib/types";
    import { speakersStore } from "$lib/stores/speakers.svelte";
    import { appState } from "$lib/stores/app-state.svelte";
    import SpeakerProfilePane from "./SpeakerProfilePane.svelte";
    import MergeProfilesDialog from "./MergeProfilesDialog.svelte";
    import * as ContextMenu from "$lib/components/ui/context-menu";
    import ResizableSplit from "$lib/components/ResizableSplit.svelte";
    import OverlayScroll from "$lib/components/OverlayScroll.svelte";
    import ConfirmDialog from "$lib/components/ConfirmDialog.svelte";
    import Tip from "$lib/components/Tip.svelte";
    import { Button } from "$lib/components/ui/button";
    import { groupByDate } from "$lib/utils/meetingFormat";
    import { ListSelection } from "$lib/utils/listSelection.svelte";
    import { copy } from "$lib/copy";
    import { cn } from "$lib/utils";

    const t = $derived(copy.speakers);

    let selectedId = $state<string | null>(null);
    let creating = $state(false);
    let confirmDelete = $state(false);
    let deleting = $state(false);
    let confirmMerge = $state(false);
    let merging = $state(false);

    // The same selection model as the meetings list; the two lists are one
    // object with different contents.
    const selection = new ListSelection();

    onMount(() => {
        void speakersStore.refresh();
    });

    let speakers = $derived(speakersStore.speakers);

    // Grouped under the same date headers as the meetings list, by when each
    // person was last in a meeting, so the headings read "who you met with
    // today". Someone never seen in one falls back to when they were added,
    // which keeps a just-created profile at the top where the user is looking.
    // The backend already returns them in this order.
    let groups = $derived(
        groupByDate(speakers, (s) => s.last_seen ?? s.created_at),
    );
    let selected = $derived(
        selectedId ? (speakersStore.byId(selectedId) ?? null) : null,
    );

    let visibleOrder = $derived(groups.flatMap((g) => g.items.map((s) => s.id)));
    const multi = $derived(selection.count > 1);

    $effect(() => {
        if (!creating && !selected && speakers.length > 0) {
            selectedId = speakers[0].id;
            selection.select(speakers[0].id);
        }
    });

    function onRowClick(id: string, event: MouseEvent) {
        creating = false;
        selection.click(id, event, visibleOrder);
        selectedId = selection.primary;
    }

    /** The menu acts on the selection, so a right-click outside it moves
     * the selection first, exactly what a plain click would have done. */
    function onRowContextMenu(id: string, event: MouseEvent) {
        if (!selection.has(id)) onRowClick(id, event);
    }

    /** The selected people, resolved to profiles for the merge dialog. */
    let mergeCandidates = $derived(
        selection.ids
            .map((id) => speakersStore.byId(id))
            .filter((p): p is SpeakerProfile => p !== undefined),
    );

    async function mergeSelected(targetId: string) {
        merging = true;
        try {
            const sourceIds = selection.ids.filter((id) => id !== targetId);
            const merged = await speakersStore.merge(targetId, sourceIds);
            if (merged) {
                confirmMerge = false;
                selection.select(targetId);
                selectedId = targetId;
            }
        } finally {
            merging = false;
        }
    }

    function onKeydown(e: KeyboardEvent) {
        if (e.key !== "Delete" || confirmDelete || selection.count === 0) return;
        const target = e.target as HTMLElement | null;
        if (
            target &&
            (target.tagName === "INPUT" ||
                target.tagName === "TEXTAREA" ||
                target.isContentEditable)
        ) {
            return;
        }
        e.preventDefault();
        confirmDelete = true;
    }

    async function deleteSelected() {
        deleting = true;
        try {
            await speakersStore.removeMany(selection.ids);
            selection.clear();
            selectedId = null;
            confirmDelete = false;
        } finally {
            deleting = false;
        }
    }

    // Palette "New profile" opens this page in create mode.
    $effect(() => {
        if (appState.profilesCreateRequest) {
            appState.clearProfilesCreateRequest();
            startCreate();
        }
    });

    function startCreate() {
        creating = true;
        selectedId = null;
    }

    function onSaved(id: string) {
        creating = false;
        selectedId = id;
    }
</script>

<div class="flex min-h-0 flex-1">
    <ResizableSplit
        fixedSide="left"
        storageKey="embral:profiles-list-width"
        defaultSize={280}
        minFixed={220}
        minFlex={420}
    >
        {#snippet left()}
            <!-- People list: no header, just the scrollable list with the
                 floating add button. The same object as the meetings pane,
                 down to the row treatment and the date headers. -->
            <div
                class="relative flex min-h-0 w-full flex-1 flex-col border-r border-border bg-muted/20"
            >
                <OverlayScroll>
                    <div class="pb-16">
                        {#each groups as group (group.label)}
                            <p
                                class="px-3 pt-3 pb-1 text-[11px] font-medium tracking-wide text-muted-foreground/80 uppercase"
                            >
                                {group.label}
                            </p>
                            {#each group.items as s (s.id)}
                                <ContextMenu.Root>
                                    <ContextMenu.Trigger>
                                        {#snippet child({ props })}
                                            <button
                                                {...props}
                                                class={cn(
                                                    "w-full border-l-2 px-3 py-2 text-left transition-colors duration-150",
                                                    selectedId === s.id && !creating
                                                        ? "border-l-foreground/60 bg-accent/50"
                                                        : selection.has(s.id) && !creating
                                                          ? "border-l-transparent bg-accent/40"
                                                          : "border-l-transparent hover:bg-accent/40",
                                                )}
                                                onclick={(e) => onRowClick(s.id, e)}
                                                oncontextmenu={(e) => {
                                                    onRowContextMenu(s.id, e);
                                                    (props as {
                                                        oncontextmenu?: (ev: MouseEvent) => void;
                                                    }).oncontextmenu?.(e);
                                                }}
                                            >
                                                <span
                                                    class="font-display block min-w-0 truncate text-sm"
                                                    >{s.name}</span
                                                >
                                            </button>
                                        {/snippet}
                                    </ContextMenu.Trigger>
                                    <ContextMenu.Content>
                                        {#if selection.count > 1}
                                            <ContextMenu.Item
                                                onSelect={() => (confirmMerge = true)}
                                            >
                                                <Merge />
                                                {t.menu.merge(selection.count)}
                                            </ContextMenu.Item>
                                            <ContextMenu.Separator />
                                        {/if}
                                        <ContextMenu.Item
                                            variant="destructive"
                                            onSelect={() => (confirmDelete = true)}
                                        >
                                            <Trash2 />
                                            {t.menu.delete(selection.count)}
                                        </ContextMenu.Item>
                                    </ContextMenu.Content>
                                </ContextMenu.Root>
                            {/each}
                        {/each}
                        {#if speakers.length === 0 && speakersStore.loaded && !creating}
                            <!-- Same treatment as the meeting list's empty
                                 state; the two pages are the same object. -->
                            <p class="px-3 py-4 text-sm text-muted-foreground">
                                {t.empty}
                            </p>
                        {/if}
                    </div>
                </OverlayScroll>

                <Tip side="left" text={t.add}>
                    {#snippet children({ props })}
                        <button
                            {...props}
                            onclick={startCreate}
                            class="absolute right-3 bottom-3 flex h-10 w-10 items-center justify-center rounded-2xl bg-primary text-primary-foreground shadow-md transition-colors hover:bg-primary/90"
                            aria-label={t.add}
                        >
                            <Plus size={18} />
                        </button>
                    {/snippet}
                </Tip>
            </div>
        {/snippet}
        {#snippet right()}
            <!-- Profile / empty state -->
            <div class="min-h-0 min-w-0 flex-1 overflow-y-auto">
                {#if multi && !creating}
                    <!-- Several people picked: no single profile to show. -->
                    <div
                        class="flex h-full flex-col items-center justify-center gap-4 p-8"
                    >
                        <p class="text-sm text-muted-foreground">
                            {t.multiSelect.selected(selection.count)}
                        </p>
                        <div class="flex items-center gap-2">
                            <button
                                onclick={() => (confirmMerge = true)}
                                class="inline-flex h-9 items-center gap-2 rounded-md border border-border px-3 text-sm font-medium transition-colors hover:bg-accent"
                            >
                                <Merge size={15} />
                                {t.multiSelect.merge(selection.count)}
                            </button>
                            <button
                                onclick={() => (confirmDelete = true)}
                                class="inline-flex h-9 items-center gap-2 rounded-md border border-border px-3 text-sm font-medium transition-colors hover:bg-destructive hover:text-white"
                            >
                                <Trash2 size={15} />
                                {t.multiSelect.delete(selection.count)}
                            </button>
                        </div>
                        <p class="text-xs text-muted-foreground">{t.multiSelect.hint}</p>
                    </div>
                {:else if creating}
                    <SpeakerProfilePane speaker={null} onSaved={onSaved} />
                {:else if selected}
                    <SpeakerProfilePane
                        speaker={selected}
                        onSaved={onSaved}
                        onDeleted={() => (selectedId = null)}
                    />
                {:else}
                    <div
                        class="flex h-full flex-col items-center justify-center gap-3 p-8 text-center"
                    >
                        <Users size={28} class="text-muted-foreground/60" />
                        <div class="max-w-md space-y-1.5">
                            <p class="text-sm font-medium">{t.intro.title}</p>
                            <p class="text-xs leading-relaxed text-muted-foreground">
                                {t.intro.body}
                            </p>
                        </div>
                        <Button size="sm" onclick={startCreate}>
                            <Plus size={15} class="mr-1" /> {t.add}
                        </Button>
                    </div>
                {/if}
            </div>
        {/snippet}
    </ResizableSplit>
</div>

<svelte:window onkeydown={onKeydown} />

<ConfirmDialog
    bind:open={confirmDelete}
    title={t.deleteConfirm.title(selection.count)}
    body={t.deleteConfirm.body}
    confirmLabel={t.deleteConfirm.confirm(selection.count)}
    busy={deleting}
    onConfirm={deleteSelected}
/>

<MergeProfilesDialog
    bind:open={confirmMerge}
    profiles={mergeCandidates}
    busy={merging}
    onConfirm={mergeSelected}
/>
