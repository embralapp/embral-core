<script lang="ts">
    import { onMount, tick } from "svelte";
    import { invoke } from "@tauri-apps/api/core";
    import { CloudOff, X } from "lucide-svelte";
    import { configStore } from "$lib/stores/config.svelte";
    import { storageRoot } from "$lib/stores/storageRoot.svelte";
    import { appState } from "$lib/stores/app-state.svelte";
    import { displayAppName } from "$lib/utils/detectedApp";
    import { themeStore } from "$lib/stores/theme.svelte";
    import { setupEventListeners } from "$lib/events";
    import { applyRecordingFixture, loadFixture } from "$lib/fixture";
    import TitleBar from "$lib/components/shell/TitleBar.svelte";
    import Sidebar from "$lib/components/shell/Sidebar.svelte";
    import SearchPalette from "$lib/components/shell/SearchPalette.svelte";
    import Onboarding from "$lib/components/onboarding/Onboarding.svelte";
    import MeetingsHome from "$lib/components/MeetingsHome.svelte";
    import RecordingControls from "$lib/components/RecordingControls.svelte";
    import NotesEditor from "$lib/components/NotesEditor.svelte";
    import LiveTranscript from "$lib/components/LiveTranscript.svelte";
    import ProcessingSteps from "$lib/components/ProcessingSteps.svelte";
    import ResizableSplit from "$lib/components/ResizableSplit.svelte";
    import SettingsLayout from "$lib/components/settings/SettingsLayout.svelte";
    import SpeakersPage from "$lib/components/speakers/SpeakersPage.svelte";
    import DictationHome from "$lib/components/dictation/DictationHome.svelte";
    import { copy } from "$lib/copy";
    import { errorMessage } from "$lib/copy/errors";

    const banner = $derived(copy.shell.detectionBanner);
    const silenceBanner = $derived(copy.meetings.silence);

    let userNotes = $state("");
    let meetingTitle = $state("");
    let paletteOpen = $state(false);
    let notesEditorRef = $state<NotesEditor | null>(null);

    // Star the current moment: the backend splits the in-flight utterance
    // and returns the star's timestamp on the segment timeline (so the
    // marker orders correctly against the words spoken before and after);
    // then the star lands in the store and the notes gutter. On a line
    // that already carries a star, this is a toggle: the existing star is
    // removed everywhere instead.
    let starring = false;
    async function starMoment() {
        if (!appState.isRecording || starring) return;
        const existing = notesEditorRef?.starAtCursor();
        if (existing !== null && existing !== undefined) {
            unstarMoment(existing);
            return;
        }
        starring = true;
        try {
            const seconds = await invoke<number>("star_moment", {
                seconds: appState.elapsedSeconds(),
            });
            const star = appState.addStar(seconds);
            notesEditorRef?.addStar(star.id);
        } catch (e) {
            console.error("star_moment failed:", e);
        } finally {
            starring = false;
        }
    }

    // Shadow mode borrows the transcript pane's collapse ([shell.md]
    // §Recording): the pane renders shut while shadowed, without touching
    // its persisted layout. Reopening it at the divider sticks for the
    // rest of the recording; the flag clears when shadow mode ends (toggle
    // or stop — appState.shadowMode already gates on isRecording).
    let shadowReopened = $state(false);
    $effect(() => {
        if (!appState.shadowMode) shadowReopened = false;
    });

    // A fresh recording gets a fresh notes draft (stops from the hotkey or
    // tray never pass through the stop button's clearing).
    let lastRecordingStart: number | null = null;
    $effect(() => {
        const startedAt = appState.recordingStartedAt;
        if (startedAt !== null && startedAt !== lastRecordingStart) {
            lastRecordingStart = startedAt;
            userNotes = "";
            meetingTitle = "";
        }
    });

    // Clicking a gutter star removes the moment everywhere: the notes
    // widget, the live transcript marker, and the backend accumulator.
    function unstarMoment(id: number) {
        const star = appState.removeStar(id);
        notesEditorRef?.removeStar(id);
        if (star) {
            invoke("unstar_moment", { seconds: star.seconds }).catch((e) =>
                console.error("unstar_moment failed:", e),
            );
        }
    }


    let needsOnboarding = $derived(
        configStore.config !== null && !configStore.config.onboarding_completed,
    );

    function onGlobalKeydown(e: KeyboardEvent) {
        if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
            e.preventDefault();
            paletteOpen = !paletteOpen;
            return;
        }
        // Ctrl+S while recording: star this moment (works mid-typing in the
        // notes — the keydown bubbles up from the editor).
        if (
            (e.ctrlKey || e.metaKey) &&
            e.key.toLowerCase() === "s" &&
            appState.isRecording
        ) {
            e.preventDefault();
            starMoment();
            return;
        }
        // Ctrl+1/2/3: jump between the main pages (the recording view is its
        // own destination, reached via the sidebar record button).
        if (e.ctrlKey || e.metaKey) {
            const view =
                e.key === "1"
                    ? ("idle" as const)
                    : e.key === "2"
                      ? ("speakers" as const)
                      : e.key === "3"
                        ? ("dictation" as const)
                        : null;
            if (view) {
                e.preventDefault();
                appState.setView(view);
            }
        }
    }

    async function keepRecordingThroughSilence() {
        try {
            await invoke("silence_keep_recording");
        } catch (e) {
            appState.setError(errorMessage(e));
        }
    }

    async function stopFromSilence() {
        // The same stop the header button performs: drafts travel with it
        // (strings always, empty included — a null arg means "use the
        // backend's mirror" and is reserved for the handshake fallback).
        appState.setPendingTitleHint(meetingTitle);
        try {
            await invoke("stop_recording", {
                userNotes,
                meetingTitle,
            });
        } catch (e) {
            appState.setError(errorMessage(e));
        }
    }

    async function acceptDetectedMeeting() {
        appState.setDetectedApp(null);
        try {
            await invoke("accept_detected_meeting");
        } catch (e) {
            appState.setError(errorMessage(e));
        }
    }

    async function dismissDetectedMeeting() {
        appState.setDetectedApp(null);
        try {
            await invoke("dismiss_detected_meeting");
        } catch {
            // Suppression is best-effort; the banner is already gone.
        }
    }

    onMount(async () => {
        // The stop path reads the notes text and star anchors before the
        // recording view unmounts; this page owns both.
        appState.setRecordingSnapshotProvider(() => {
            // A destroyed editor must not take the stop path down with it —
            // the notes text still delivers even if the anchors don't.
            let starBlocks = new Map<number, number>();
            try {
                starBlocks = notesEditorRef?.getStarBlocks() ?? starBlocks;
            } catch {
                // Anchors lost; the stars keep their timestamps.
            }
            return { notes: userNotes, title: meetingTitle, starBlocks };
        });
        await configStore.load();
        // Needed before any document with an image renders — it is what
        // turns a stored `assets/…` link into something the webview loads.
        await storageRoot.load();
        setupEventListeners();
        // First-run goes through onboarding instead of being dropped into
        // Settings; afterwards, an unconfigured provider still lands there.
        if (
            configStore.config?.onboarding_completed &&
            !configStore.isConfigured
        ) {
            appState.setView("settings");
        }
        // Staged screenshot moment (dev sandboxes only — $lib/fixture).
        // The drafts land after a tick, past the fresh-recording clear.
        const fixture = await loadFixture();
        if (fixture?.recording) {
            applyRecordingFixture(fixture.recording);
            await tick();
            userNotes = fixture.recording.notes_markdown ?? "";
            meetingTitle = fixture.recording.title ?? "";
        }
    });

    // Re-apply whenever the configured theme changes (settings, sidebar cycle).
    $effect(() => {
        themeStore.apply(configStore.config?.theme ?? "system");
    });

    // Mirror the drafts into the backend while recording (debounced), so a
    // stop the frontend never answers still saves the human's words.
    let draftSyncTimer: ReturnType<typeof setTimeout> | null = null;
    $effect(() => {
        const notes = userNotes;
        const title = meetingTitle;
        if (!appState.isRecording) return;
        if (draftSyncTimer) clearTimeout(draftSyncTimer);
        draftSyncTimer = setTimeout(() => {
            void invoke("sync_recording_drafts", {
                notes,
                meetingTitle: title,
            }).catch(() => {});
        }, 1000);
    });
</script>

<svelte:window onkeydown={onGlobalKeydown} />

<div class="flex h-screen flex-col overflow-hidden bg-background text-foreground">
    <TitleBar onSearch={() => (paletteOpen = true)} />

    <div class="flex min-h-0 flex-1">
        <Sidebar />

        <main class="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
            {#if appState.detectedApp && !appState.isRecording}
                <div
                    class="flex shrink-0 items-center justify-between gap-3 border-b border-border bg-primary/5 px-4 py-2"
                >
                    <p class="min-w-0 truncate text-sm">
                        <span class="font-medium">{banner.label}</span>
                        <span class="text-muted-foreground">
                            {banner.detail(displayAppName(appState.detectedApp))}</span
                        >
                    </p>
                    <div class="flex shrink-0 items-center gap-2">
                        <button
                            class="rounded-md bg-primary px-3 py-1 text-xs font-medium text-primary-foreground transition-colors hover:bg-primary/90"
                            onclick={acceptDetectedMeeting}
                        >
                            {banner.record}
                        </button>
                        <button
                            class="rounded-md px-2 py-1 text-xs text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
                            onclick={dismissDetectedMeeting}
                        >
                            {banner.dismiss}
                        </button>
                    </div>
                </div>
            {/if}
            {#if appState.view === "idle"}
                <MeetingsHome />
            {:else if appState.view === "recording"}
                <div class="flex min-h-0 flex-1 flex-col overflow-hidden">
                    <RecordingControls
                        bind:userNotes
                        bind:meetingTitle
                        onStar={starMoment}
                    />
                    {#if appState.silenceNoticeMinutes !== null}
                        <div
                            class="flex shrink-0 items-center justify-between gap-3 border-b border-border bg-primary/5 px-4 py-2"
                        >
                            <p class="min-w-0 truncate text-sm">
                                <span class="font-medium">{silenceBanner.label}</span>
                                <span class="text-muted-foreground">
                                    {silenceBanner.detail(appState.silenceNoticeMinutes)}</span
                                >
                            </p>
                            <div class="flex shrink-0 items-center gap-2">
                                <button
                                    class="rounded-md bg-primary px-3 py-1 text-xs font-medium text-primary-foreground transition-colors hover:bg-primary/90"
                                    onclick={keepRecordingThroughSilence}
                                >
                                    {silenceBanner.keep}
                                </button>
                                <button
                                    class="rounded-md px-2 py-1 text-xs text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
                                    onclick={stopFromSilence}
                                >
                                    {silenceBanner.stop}
                                </button>
                            </div>
                        </div>
                    {/if}
                    {#if appState.fallbackNotice}
                        <div
                            class="flex shrink-0 items-center gap-2 border-b border-border bg-amber-500/10 px-4 py-1.5 text-xs text-amber-700 dark:text-amber-400"
                        >
                            <CloudOff size={12} class="shrink-0" />
                            <span class="min-w-0 flex-1 truncate">{appState.fallbackNotice}</span>
                            <!-- Read once and it has done its job; the switch
                                 already happened and nothing here is actionable. -->
                            <button
                                class="-mr-1 shrink-0 rounded p-1 transition-colors hover:bg-amber-500/20"
                                aria-label={copy.common.dismiss}
                                onclick={() => appState.setFallbackNotice(null)}
                            >
                                <X size={12} />
                            </button>
                        </div>
                    {/if}
                    <ResizableSplit
                        fixedSide="right"
                        storageKey="embral:recording-transcript-width"
                        defaultSize={340}
                        minFixed={260}
                        minFlex={320}
                        collapsible
                        forceCollapsed={appState.shadowMode &&
                            !shadowReopened}
                        onForceReopen={() => (shadowReopened = true)}
                    >
                        {#snippet left()}
                            <NotesEditor
                                bind:this={notesEditorRef}
                                bind:value={userNotes}
                                onStarClick={unstarMoment}
                                onPasteError={(message) =>
                                    appState.setError(message)}
                            />
                        {/snippet}
                        {#snippet right()}
                            <LiveTranscript />
                        {/snippet}
                    </ResizableSplit>
                </div>
            {:else if appState.view === "processing"}
                <ProcessingSteps />
            {:else if appState.view === "speakers"}
                <SpeakersPage />
            {:else if appState.view === "dictation"}
                <DictationHome />
            {:else if appState.view === "settings"}
                <SettingsLayout />
            {/if}
        </main>
    </div>
</div>

<SearchPalette bind:open={paletteOpen} />

{#if needsOnboarding}
    <Onboarding />
{/if}
