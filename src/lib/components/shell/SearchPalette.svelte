<script lang="ts">
	import { errorMessage } from '$lib/copy/errors';
    import { invoke } from "@tauri-apps/api/core";
    import {
        BookUser,
        FileText,
        FileUp,
        Mic,
        NotebookPen,
        Settings,
        Square,
        Type,
        UserPlus,
    } from "lucide-svelte";
    import { importRecording } from "$lib/utils/importRecording";
    import type { LibraryMeetingHit, LibrarySearchResults } from "$lib/types";
    import { appState } from "$lib/stores/app-state.svelte";
    import { meetingsStore } from "$lib/stores/meetings.svelte";
    import { configStore } from "$lib/stores/config.svelte";
    import { dictationStore } from "$lib/stores/dictation.svelte";
    import { copy } from "$lib/copy";
    import * as Command from "$lib/components/ui/command";

    let { open = $bindable(false) }: { open?: boolean } = $props();

    const t = $derived(copy.shell.palette);
    const navCopy = $derived(copy.shell.sidebar.nav);
    const sectionNames = $derived(copy.settings.nav.sections);

    const noResults: LibrarySearchResults = { meetings: [], dictations: [] };

    let query = $state("");
    let results = $state<LibrarySearchResults>(noResults);
    let searching = $state(false);
    let searchTimer: ReturnType<typeof setTimeout> | null = null;
    /** Which search is the current one. A slower earlier query must not
     * overwrite a newer one's results: the palette would show answers to a
     * question the user has already moved on from. */
    let generation = 0;

    // Debounced hybrid query (meetings + dictations in one call); results
    // are already ranked by the backend, so the Command list renders them
    // as-is (shouldFilter=false; all filtering here is manual).
    $effect(() => {
        const q = query.trim();
        if (searchTimer) clearTimeout(searchTimer);
        if (!open || q.length < 2) {
            results = noResults;
            searching = false;
            generation++;
            return;
        }
        searching = true;
        searchTimer = setTimeout(async () => {
            const mine = ++generation;
            try {
                const found = await invoke<LibrarySearchResults>("search_library", {
                    query: q,
                    limit: 12,
                });
                if (mine !== generation) return;
                results = found;
            } catch {
                if (mine !== generation) return;
                results = noResults;
            } finally {
                if (mine === generation) searching = false;
            }
        }, 150);
    });

    $effect(() => {
        if (!open) {
            query = "";
            results = noResults;
        }
    });

    /// Split an FTS snippet on its [match] markers for highlighting.
    function snippetParts(
        snippet: string,
    ): { text: string; match: boolean }[] {
        const parts: { text: string; match: boolean }[] = [];
        const re = /\[([^\]]*)\]/g;
        let last = 0;
        for (const m of snippet.matchAll(re)) {
            if (m.index > last) {
                parts.push({ text: snippet.slice(last, m.index), match: false });
            }
            parts.push({ text: m[1], match: true });
            last = m.index + m[0].length;
        }
        if (last < snippet.length) {
            parts.push({ text: snippet.slice(last), match: false });
        }
        return parts;
    }

    function formatDate(iso: string): string {
        return new Date(iso).toLocaleDateString(undefined, {
            month: "short",
            day: "numeric",
            year: "numeric",
        });
    }

    function close(fn: () => void) {
        open = false;
        fn();
    }

    // Opening a result carries the passage it matched, so the detail pane
    // opens on that sentence rather than at the top of the meeting. The
    // search already knew where it was; making the user find it again was
    // the whole gap.
    async function openMeeting(hit: LibraryMeetingHit) {
        const asked = query.trim();
        open = false;
        appState.setView("idle");
        await meetingsStore.select(hit.id, {
            source: hit.source,
            start_secs: hit.start_secs,
            end_secs: hit.end_secs,
            lead: hit.lead,
            image: hit.image,
            query: asked,
        });
    }

    async function startRecording() {
        open = false;
        try {
            await invoke("start_recording");
        } catch (e) {
            appState.setError(errorMessage(e));
        }
    }

    async function stopRecording() {
        open = false;
        try {
            await invoke("stop_recording", { userNotes: null, meetingTitle: null });
        } catch (e) {
            appState.setError(errorMessage(e));
        }
    }

    // Navigation entries, filtered manually against the query. The main
    // pages always show; settings pages surface as you type. The order and the
    // destinations are this component's; the names come from the catalog.
    // These must stay the same words the sidebar and the settings rail use.
    const pages: {
        key: keyof typeof copy.shell.sidebar.nav;
        icon: typeof Mic;
        go: () => void;
    }[] = [
        { key: "meetings", icon: NotebookPen, go: () => appState.setView("idle") },
        { key: "speakers", icon: BookUser, go: () => appState.setView("speakers") },
        { key: "dictation", icon: Type, go: () => appState.setView("dictation") },
    ];
    // Ids follow SettingsLayout's SectionId values. Account is deliberately
    // absent: it has no deep link.
    const settingsIds = [
        "general",
        "meetings",
        "dictation",
        "about",
        "markdown",
        "webhooks",
        "mcp",
        "transcription",
        "synthesis",
    ] as const;
    let settingsMatches = $derived.by(() => {
        const q = query.trim().toLowerCase();
        if (q.length < 2) return [];
        const settingsWord = t.settings.toLowerCase();
        return settingsIds
            .map((id) => ({ id, label: sectionNames[id] }))
            .filter(
                (p) =>
                    p.label.toLowerCase().includes(q) ||
                    settingsWord.includes(q) ||
                    `${settingsWord} ${p.label}`.toLowerCase().includes(q),
            );
    });
    let pageMatches = $derived.by(() => {
        const q = query.trim().toLowerCase();
        const labelled = pages.map((p) => ({ ...p, label: navCopy[p.key] }));
        if (!q) return labelled;
        return labelled.filter((p) => p.label.toLowerCase().includes(q));
    });
</script>

<!-- A dialog normally hands focus back to whatever opened it. This one
     navigates: the row that had focus a moment ago is usually not even the
     meeting now on screen, so restoring it leaves a focus ring sitting on
     the wrong thing, reading as a selection that isn't one. -->
<Command.Dialog
    bind:open
    shouldFilter={false}
    onCloseAutoFocus={(event) => event.preventDefault()}
    title={t.dialogTitle}
    description={t.dialogDescription}
    class="sm:max-w-2xl"
>
    <Command.Input placeholder={t.placeholder} bind:value={query} />
    <Command.List>
        <!-- "No results" is only true once the search has actually finished.
             Saying it while a query is in flight tells the user their meeting
             isn't there, a moment before it appears. -->
        {#if query.trim().length >= 2 && !searching && results.meetings.length === 0 && results.dictations.length === 0}
            <Command.Empty>{t.empty}</Command.Empty>
        {/if}

        {#if searching && results.meetings.length === 0}
            <Command.Loading>
                <p class="px-2 py-3 text-center text-sm text-muted-foreground">
                    {t.searching}
                </p>
            </Command.Loading>
        {/if}

        {#if results.meetings.length > 0}
            <Command.Group heading={t.groups.meetings}>
                {#each results.meetings as hit (hit.id)}
                    <Command.Item
                        value={hit.id}
                        onSelect={() => openMeeting(hit)}
                        class="flex-col items-start gap-0.5"
                    >
                        <div class="flex w-full items-center gap-2">
                            <FileText size={14} class="shrink-0 text-muted-foreground" />
                            <span class="truncate font-medium">{hit.title}</span>
                            <span class="ml-auto shrink-0 text-xs text-muted-foreground"
                                >{formatDate(hit.started_at)}</span
                            >
                        </div>
                        <p class="line-clamp-1 pl-6 text-xs text-muted-foreground">
                            {#each snippetParts(hit.snippet) as part, i (i)}
                                {#if part.match}<mark
                                        class="rounded-sm bg-primary/15 px-0.5 text-foreground"
                                        >{part.text}</mark
                                    >{:else}{part.text}{/if}
                            {/each}
                        </p>
                    </Command.Item>
                {/each}
            </Command.Group>
        {/if}

        {#if results.dictations.length > 0}
            <Command.Group heading={t.groups.dictations}>
                {#each results.dictations as d (d.id)}
                    <Command.Item
                        value={`dictation-${d.id}`}
                        onSelect={() =>
                            close(() => {
                                dictationStore.landOn(d.id);
                                appState.setView("dictation");
                            })}
                        class="flex-col items-start gap-0.5"
                    >
                        <div class="flex w-full items-center gap-2">
                            <Type size={14} class="shrink-0 text-muted-foreground" />
                            <span class="line-clamp-1 min-w-0 text-sm">
                                {#each snippetParts(d.snippet) as part, i (i)}
                                    {#if part.match}<mark
                                            class="rounded-sm bg-primary/15 px-0.5 text-foreground"
                                            >{part.text}</mark
                                        >{:else}{part.text}{/if}
                                {/each}
                            </span>
                            <span class="ml-auto shrink-0 text-xs text-muted-foreground"
                                >{formatDate(d.created_at)}</span
                            >
                        </div>
                    </Command.Item>
                {/each}
            </Command.Group>
        {/if}

        <Command.Group heading={t.groups.actions}>
            {#if appState.isRecording}
                <Command.Item value="action-stop" onSelect={stopRecording}>
                    <Square size={14} />
                    {t.actions.stopRecording}
                </Command.Item>
            {:else if configStore.isConfigured}
                <Command.Item value="action-record" onSelect={startRecording}>
                    <Mic size={14} />
                    {t.actions.startRecording}
                </Command.Item>
                <Command.Item
                    value="action-dictate"
                    onSelect={() => close(() => void dictationStore.start())}
                >
                    <Type size={14} />
                    {t.actions.startDictation}
                </Command.Item>
            {/if}
            <Command.Item
                value="action-import"
                onSelect={() => close(() => void importRecording())}
            >
                <FileUp size={14} />
                {t.actions.importRecording}
            </Command.Item>
            <Command.Item
                value="action-new-profile"
                onSelect={() => close(() => appState.openProfilesCreate())}
            >
                <UserPlus size={14} />
                {t.actions.newProfile}
            </Command.Item>
        </Command.Group>

        <Command.Group heading={t.groups.goTo}>
            {#each pageMatches as p (p.key)}
                <Command.Item value={`nav-${p.key}`} onSelect={() => close(p.go)}>
                    <p.icon size={14} />
                    {p.label}
                </Command.Item>
            {/each}
            {#if settingsMatches.length === 0}
                <Command.Item
                    value="nav-settings"
                    onSelect={() => close(() => appState.openSettings())}
                >
                    <Settings size={14} />
                    {t.settings}
                </Command.Item>
            {/if}
            {#each settingsMatches as s (s.id)}
                <Command.Item
                    value={`nav-settings-${s.id}`}
                    onSelect={() => close(() => appState.openSettings(s.id))}
                >
                    <Settings size={14} />
                    {t.settingsPage(s.label)}
                </Command.Item>
            {/each}
        </Command.Group>
    </Command.List>
</Command.Dialog>
