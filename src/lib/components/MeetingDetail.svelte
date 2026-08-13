<script lang="ts">
  import { onDestroy, onMount, tick } from 'svelte';
  import { ChevronLeft, Trash2 } from 'lucide-svelte';
  import { meetingsStore, PENDING_MEETING_ID } from '$lib/stores/meetings.svelte';
  import { appState } from '$lib/stores/app-state.svelte';
  import { configStore } from '$lib/stores/config.svelte';
  import type { MeetingDetail } from '$lib/types';
  import AudioPlayer from './AudioPlayer.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import Tip from './Tip.svelte';
  import PendingMeetingDetail from './PendingMeetingDetail.svelte';
  import TranscriptSegments from './TranscriptSegments.svelte';
  import EditorView from './EditorView.svelte';
  import type { ChunkSource, MeetingStar, PassageLanding } from '$lib/types';
  import { findMatchIndex } from '$lib/editor/locate';
  import { formatDuration, formatMeetingDate } from '$lib/utils/meetingFormat';
  import { copy } from '$lib/copy';

  const t = $derived(copy.meetings.detail);
  const deleteConfirm = $derived(copy.meetings.deleteConfirm);

  type DetailTab = 'summary' | 'notes' | 'transcript';
  type SaveState = 'idle' | 'saving' | 'saved' | 'error';

  let {
    showBack = false,
    onBack
  }: { showBack?: boolean; onBack?: () => void } = $props();

  let activeTab = $state<DetailTab>('summary');
  let titleDraft = $state('');
  let summaryDraft = $state('');
  let notesDraft = $state('');
  let transcriptDraft = $state('');
  let summaryFrontmatter = $state('');
  let transcriptFrontmatter = $state('');
  let loadedDraftId = $state<string | null>(null);
  let saveState = $state<SaveState>('idle');
  let saveError = $state<string | null>(null);
  let confirmDelete = $state(false);
  let isDeleting = $state(false);
  let player = $state<AudioPlayer | null>(null);
  // Playback state mirrored out of the player; it drives the transcript
  // tab's current-segment highlight and auto-follow.
  let playbackTime = $state(0);
  let playbackActive = $state(false);
  let notesViewRef = $state<EditorView | null>(null);
  let summaryViewRef = $state<EditorView | null>(null);
  let transcriptViewRef = $state<EditorView | null>(null);
  let transcriptRef = $state<TranscriptSegments | null>(null);
  /** Where a search result asked us to land, held until the tab it names has
   * actually mounted: the `{#key}` blocks below remount per meeting, so the
   * view refs are null for a beat after the detail arrives. */
  let landing = $state<PassageLanding | null>(null);
  let landingAttempts = $state(0);
  /** Mount, then the draft arriving, is two. The rest is slack for a tab
   * whose content settles in stages; past this the text is genuinely not
   * there and the user stays where they are. */
  const MAX_LANDING_ATTEMPTS = 6;

  /** Which tab holds a passage. Image text has no document of its own: it
   * was read out of a picture, and the picture is in the notes. */
  function tabForSource(source: ChunkSource): DetailTab {
    if (source === 'summary') return 'summary';
    if (source === 'transcript') return 'transcript';
    return 'notes';
  }

  /** A star tick/chip on the player was clicked: the player already
   * seeked; scroll the active tab to that star's place. */
  function onStarActivate(star: MeetingStar, index: number) {
    if (activeTab === 'notes' && star.note_block !== null) {
      notesViewRef?.scrollToStar(index);
    } else if (activeTab === 'transcript') {
      transcriptRef?.followPlayhead();
    }
  }

  // A debounced save is a promise to write, so it is never dropped: it
  // is either fired by its timer or flushed early. Keeping the work itself
  // (not just the timer) is what makes flushing possible: switching
  // meetings used to clear the timers, so typing and clicking away inside
  // the debounce window silently lost the edit. Each closure captured its
  // own meeting id, so a flush after the selection moved still writes to
  // the meeting the text belongs to.
  type SaveKey = 'title' | 'summary' | 'notes' | 'transcript';
  const timers: Record<SaveKey, ReturnType<typeof setTimeout> | null> = {
    title: null,
    summary: null,
    notes: null,
    transcript: null
  };
  const pending: Record<SaveKey, (() => Promise<unknown>) | null> = {
    title: null,
    summary: null,
    notes: null,
    transcript: null
  };
  let saveRevision = 0;

  const detail = $derived(meetingsStore.selectedDetail);
  const isLoading = $derived(
    meetingsStore.selectedId !== null &&
      meetingsStore.detailLoadingId === meetingsStore.selectedId
  );

  /** A meeting recorded with summaries off has no summary document: it is its
   * notes and its transcript, so it shows no Summary tab rather than an empty
   * one. Meetings summarized before the setting changed keep theirs.
   *
   * Keyed off the saved meeting, not the draft: from the draft, the tab would
   * vanish out from under a user who selected all and hit delete. */
  const hasSummary = $derived((detail?.summary ?? '').trim().length > 0);
  const tabs = $derived(
    (hasSummary
      ? [
          ['summary', t.tabs.summary],
          ['notes', t.tabs.notes],
          ['transcript', t.tabs.transcript]
        ]
      : [
          ['notes', t.tabs.notes],
          ['transcript', t.tabs.transcript]
        ]) as [DetailTab, string][]
  );
  const statusText = $derived.by(() => {
    if (saveState === 'saving') return t.status.saving;
    if (saveState === 'saved') return t.status.saved;
    if (saveState === 'error') return saveError ?? t.status.failed;
    return '';
  });
  // Read-only: speakers are edited through the transcript pills, and this
  // line reflects them (frontmatter attendees for legacy meetings).
  const attendeeLine = $derived.by(() => {
    if (detail && detail.segments.length > 0) {
      const seen: string[] = [];
      for (const s of detail.segments) {
        if (s.speaker && !seen.includes(s.speaker)) seen.push(s.speaker);
      }
      if (seen.length > 0) return seen.join(', ');
    }
    return detail?.attendees.join(', ') ?? '';
  });

  function stripFirstHeading(markdown: string): string {
    const lines = markdown.split(/\r?\n/);
    const headingIndex = lines.findIndex((line) => line.startsWith('# '));
    if (headingIndex < 0) return markdown;

    lines.splice(headingIndex, 1);
    while (headingIndex < lines.length && lines[headingIndex]?.trim() === '') {
      lines.splice(headingIndex, 1);
    }
    return lines.join('\n').trimStart();
  }

  function splitEditableMarkdown(markdown: string): { frontmatter: string; body: string } {
    if (!markdown.startsWith('---')) {
      return { frontmatter: '', body: stripFirstHeading(markdown) };
    }
    const end = markdown.indexOf('\n---', 3);
    if (end === -1) {
      return { frontmatter: '', body: stripFirstHeading(markdown) };
    }
    const closingEnd = markdown.indexOf('\n', end + 4);
    if (closingEnd === -1) {
      return { frontmatter: markdown, body: '' };
    }
    return {
      frontmatter: markdown.slice(0, closingEnd).trimEnd(),
      body: stripFirstHeading(markdown.slice(closingEnd + 1).trimStart())
    };
  }

  function joinEditableMarkdown(frontmatter: string, heading: string, body: string): string {
    const trimmedFrontmatter = frontmatter.trim();
    const trimmedBody = body.trimStart();
    const markdown = `# ${heading.trim()}\n\n${trimmedBody}`.trimEnd();
    if (!trimmedFrontmatter) return markdown;
    return `${trimmedFrontmatter}\n\n${markdown}`;
  }

  $effect(() => {
    const selectedId = meetingsStore.selectedId;
    const selectedDetail = detail;
    if (!selectedId) {
      void flushSaves();
      loadedDraftId = null;
      titleDraft = '';
      summaryDraft = '';
      notesDraft = '';
      transcriptDraft = '';
      summaryFrontmatter = '';
      transcriptFrontmatter = '';
      return;
    }
    if (selectedDetail && loadedDraftId !== selectedId) {
      // The outgoing meeting's pending edits go to disk before this pane
      // starts showing a different meeting's text.
      void flushSaves();
      const notesParts = splitEditableMarkdown(selectedDetail.summary);
      const transcriptParts = splitEditableMarkdown(selectedDetail.transcript);
      loadedDraftId = selectedId;
      titleDraft = selectedDetail.record.title;
      summaryDraft = notesParts.body;
      // The Notes tab needs a draft of its own for the same reason the
      // Summary tab does: it used to render `detail.notes` straight, and
      // the fresh detail returned by a save would push text back into the
      // live editor, wiping the star attributes and jumping the caret.
      notesDraft = selectedDetail.notes;
      transcriptDraft = transcriptParts.body;
      summaryFrontmatter = notesParts.frontmatter;
      transcriptFrontmatter = transcriptParts.frontmatter;
      saveState = 'idle';
      saveError = null;
      confirmDelete = false;
      // The config names one of the detail's three tabs directly; a meeting
      // with no summary has no Summary tab to open on, so that one degrades
      // to Notes. A search result overrides this; see below.
      const preferred: DetailTab = configStore.config?.open_meeting_tab ?? 'summary';
      activeTab =
        preferred === 'summary' && !notesParts.body.trim() ? 'notes' : preferred;
    }
  });

  /** A search result names the tab its passage lives in, and beats the
   * config default: the user asked for that sentence, not for their usual
   * landing page.
   *
   * Deliberately its own effect rather than part of the block above, which
   * only runs when the meeting changes: searching for a passage in the
   * meeting already on screen is an ordinary thing to do, and it used to do
   * nothing at all. Waiting on `loadedDraftId` keeps the order fixed: the
   * drafts are in place, so this is never overwritten by the config default
   * a moment later. */
  $effect(() => {
    const pending = meetingsStore.pendingLanding;
    if (!pending || loadedDraftId !== meetingsStore.selectedId) return;
    meetingsStore.takeLanding();
    landing = pending;
    landingAttempts = 0;
    const wanted = tabForSource(pending.source);
    activeTab = wanted === 'summary' && !hasSummary ? 'notes' : wanted;
  });

  /** Land on the passage a search result matched, once the tab holding it
   * has a document to scroll.
   *
   * Two separate waits, and missing the second one is what made this look
   * broken from a different meeting while working on the one already
   * open. The `{#key}` blocks remount per meeting, so first the view ref
   * has to exist, and then the document inside it, which arrives with the
   * draft, a beat later still. Landing at the first of those searched an
   * editor still holding the previous meeting's text, found nothing, and
   * cleared the landing anyway, so nothing retried.
   *
   * So: keep the landing until an attempt succeeds, re-run when a draft
   * changes, and do the work after `tick()`: this effect runs before the
   * editor's own, parent before child, so without the wait it would read
   * the document one update too early.
   *
   * A passage that cannot be found once the document is there leaves the
   * user on the right tab with nothing marked, which is honest: the text
   * may have been edited away since it was indexed. */
  $effect(() => {
    const target = landing;
    if (!target) return;
    // Named so the effect re-runs when what it needs finally appears: the
    // view ref, and the draft whose arrival is what fills the document.
    const waitingOn = {
      notesViewRef,
      summaryViewRef,
      transcriptViewRef,
      transcriptRef,
      summaryDraft,
      notesDraft,
      transcriptDraft
    };
    void waitingOn;
    // After the DOM settles: the editor applies a new draft in its own
    // effect, and this one runs first (parent before child). Without the
    // wait we search a document that has not been filled in yet.
    void tick().then(() => attemptLanding(target));
  });

  function attemptLanding(target: PassageLanding) {
    // A newer search has already replaced this one.
    if (landing !== target) return;
    landingAttempts += 1;
    const givingUp = landingAttempts >= MAX_LANDING_ATTEMPTS;

    if (target.source === 'transcript') {
      const segments = detail?.segments ?? [];
      if (transcriptRef && segments.length > 0) {
        // The line the user searched for, not the passage it sits in: a
        // passage is packed paragraphs, so its start is routinely minutes
        // early. The search is bounded to the passage, because a common
        // word ("north") occurs all over a long transcript and only the
        // occurrence the result showed is the right one. `start_secs`
        // stands in for a semantic hit, where the words are nowhere.
        const within = segments
          .map((segment, index) => ({ segment, index }))
          .filter(
            ({ segment }) =>
              target.start_secs === null ||
              target.end_secs === null ||
              (segment.start >= target.start_secs - 0.01 &&
                segment.start <= target.end_secs + 0.01)
          );
        const pool = within.length > 0 ? within : segments.map((segment, index) => ({ segment, index }));
        const found = findMatchIndex(pool.map(({ segment }) => segment.text), target.query);
        const match = found !== null ? pool[found] : null;
        const seconds = match ? match.segment.start : target.start_secs;
        if (seconds !== null) {
          landing = null;
          // A little way into the line, not at its edge: a media element
          // snaps a seek to the nearest frame it can decode, and landing
          // even milliseconds short leaves the highlight on the line
          // before. A quarter-second is inaudible inside a spoken line and
          // comfortably past that.
          const inside = seconds + 0.25;
          void player?.seekTo(inside);
          // Drives the current-segment highlight even with no audio to
          // play: `activeIndex` derives from this, and a meeting recorded
          // with `retain_audio` off has no player at all.
          playbackTime = inside;
          if (match) {
            void tick().then(() => transcriptRef?.revealIndex(match.index));
          } else {
            void tick().then(() => transcriptRef?.followPlayhead());
          }
          return;
        }
      } else if (transcriptViewRef?.isReady()) {
        // A legacy import: no segments, so the transcript is a document.
        if (transcriptViewRef.scrollToPassage(target.query, target.lead)) {
          landing = null;
          return;
        }
      }
      if (givingUp) landing = null;
      return;
    }

    const view = target.source === 'summary' ? summaryViewRef : notesViewRef;
    const landed =
      view?.isReady() &&
      (target.source === 'image_text'
        ? // The matched text is inside the picture, so there is nothing in
          // the document to search for: the image itself is the target.
          view.scrollToImage(target.image ?? '')
        : view.scrollToPassage(target.query, target.lead));

    // A failed attempt is usually "the document is not here yet", not "the
    // text is gone": the editor mounts holding the previous meeting's draft
    // and is refilled a beat later, which re-runs the effect above. The
    // bound is what stops a landing nobody can satisfy from firing much
    // later, on an unrelated edit, and scrolling out of nowhere.
    if (landed || givingUp) landing = null;
  }

  /** Space toggles playback, ←/→ skip ±10s, unless focus is in an editor
   * or other input, which owns its keys. */
  function onDetailKeydown(e: KeyboardEvent) {
    if (!detail?.audio_exists || !player) return;
    const target = e.target as HTMLElement | null;
    if (
      target &&
      (target.tagName === 'INPUT' ||
        target.tagName === 'TEXTAREA' ||
        target.isContentEditable)
    ) {
      return;
    }
    if (e.key === ' ') {
      e.preventDefault();
      void player.toggle();
    } else if (e.key === 'ArrowLeft') {
      e.preventDefault();
      player.skip(-10);
    } else if (e.key === 'ArrowRight') {
      e.preventDefault();
      player.skip(10);
    }
  }

  function schedule(key: SaveKey, delayMs: number, run: () => Promise<unknown>) {
    const timer = timers[key];
    if (timer) clearTimeout(timer);
    pending[key] = run;
    timers[key] = setTimeout(() => void runPending(key), delayMs);
  }

  async function runPending(key: SaveKey) {
    const run = pending[key];
    const timer = timers[key];
    if (timer) clearTimeout(timer);
    timers[key] = null;
    pending[key] = null;
    if (!run) return;
    const revision = ++saveRevision;
    markSaving();
    try {
      await run();
      markSaved(revision);
    } catch (e) {
      markError(e, revision);
    }
  }

  /** Write anything still sitting in a debounce, now. Called before the
   * selection moves, when the tab changes, when the pane goes away, and
   * when the window is hidden: every point where the user is done with
   * this text even though the timer hasn't fired. */
  function flushSaves(): Promise<unknown> {
    return Promise.all(
      (['title', 'summary', 'notes', 'transcript'] as SaveKey[]).map(runPending)
    );
  }

  function markSaving() {
    saveState = 'saving';
    saveError = null;
  }

  function markSaved(revision: number) {
    if (revision === saveRevision) {
      saveState = 'saved';
      saveError = null;
    }
  }

  function markError(error: unknown, revision: number) {
    if (revision === saveRevision) {
      saveState = 'error';
      saveError = error instanceof Error ? error.message : String(error);
    }
  }

  function scheduleTitleSave() {
    const id = meetingsStore.selectedId;
    const title = titleDraft.trim();
    if (!id) return;
    if (!title) {
      saveState = 'error';
      saveError = t.titleRequired;
      return;
    }
    schedule('title', 500, () => meetingsStore.updateTitle(id, title));
  }

  function scheduleSummarySave(value: string) {
    summaryDraft = value;
    const id = meetingsStore.selectedId;
    if (!id) return;
    const markdown = joinEditableMarkdown(summaryFrontmatter, titleDraft, value);
    schedule('summary', 900, () => meetingsStore.updateSummary(id, markdown));
  }

  function scheduleNotesSave(value: string) {
    notesDraft = value;
    const id = meetingsStore.selectedId;
    if (!id) return;
    // Read the star ordinals at save time, not at edit time: the editor is
    // the source of truth for where they sit now.
    schedule('notes', 900, () =>
      meetingsStore.updateNotes(id, value, notesViewRef?.currentStars() ?? [])
    );
  }

  function scheduleTranscriptSave(value: string) {
    transcriptDraft = value;
    const id = meetingsStore.selectedId;
    if (!id) return;
    const markdown = joinEditableMarkdown(
      transcriptFrontmatter,
      t.transcriptHeading(titleDraft),
      value
    );
    schedule('transcript', 900, () => meetingsStore.updateTranscript(id, markdown));
  }

  /// Segment edits regenerate the transcript document (and can rename
  /// attendees) backend-side, so pull those fields back into the drafts.
  function syncFromDetail(updated: MeetingDetail) {
    const transcriptParts = splitEditableMarkdown(updated.transcript);
    transcriptDraft = transcriptParts.body;
    transcriptFrontmatter = transcriptParts.frontmatter;
  }

  async function deleteSelectedMeeting() {
    const id = meetingsStore.selectedId;
    if (!id) return;
    isDeleting = true;
    try {
      await meetingsStore.deleteMeeting(id);
      confirmDelete = false;
      onBack?.();
    } finally {
      isDeleting = false;
    }
  }

  // The pane going away, the window being hidden, or the app quitting are
  // all "the user is done with this text", so write it rather than let the
  // debounce die with the listener. `visibilitychange` fires with time to
  // spare, unlike `beforeunload`, where an async save would not finish.
  function flushOnHide() {
    if (document.visibilityState === 'hidden') void flushSaves();
  }

  onMount(() => {
    document.addEventListener('visibilitychange', flushOnHide);
    window.addEventListener('blur', flushSaves);
  });

  onDestroy(() => {
    document.removeEventListener('visibilitychange', flushOnHide);
    window.removeEventListener('blur', flushSaves);
    void flushSaves();
  });
</script>

<div class="flex flex-col min-w-0 min-h-0 bg-background flex-1">
  {#if meetingsStore.selectedId === PENDING_MEETING_ID && appState.pendingMeeting}
    <PendingMeetingDetail pending={appState.pendingMeeting} />
  {:else if !meetingsStore.selectedRecord}
    <div class="flex flex-1 items-center justify-center p-6 text-center">
      <p class="text-sm text-muted-foreground">{t.selectPrompt}</p>
    </div>
  {:else}
    <div class="px-3 py-3 border-b border-border shrink-0">
      {#if showBack}
        <button
          onclick={onBack}
          aria-label={t.backAria}
          class="mb-2 inline-flex items-center gap-1.5 text-xs font-medium text-muted-foreground hover:text-foreground transition-colors min-[960px]:hidden"
        >
          <ChevronLeft size={14} />
          {t.back}
        </button>
      {/if}

      <div class="flex items-start gap-2">
        <!-- Borderless like the writing surface: editing the title should
             feel like editing text, not a form field. -->
        <input
          bind:value={titleDraft}
          oninput={scheduleTitleSave}
          class="font-display min-w-0 flex-1 bg-transparent px-0 py-0.5 text-lg leading-snug outline-none
            placeholder:text-muted-foreground/70"
          aria-label={t.titleAria}
        />
        <Tip text={t.deleteMeeting}>
          {#snippet children({ props })}
            <button
              {...props}
              onclick={() => (confirmDelete = true)}
              class="p-1.5 rounded-md text-muted-foreground hover:text-destructive hover:bg-destructive/10 transition-colors shrink-0"
              aria-label={t.deleteMeeting}
            >
              <Trash2 size={15} />
            </button>
          {/snippet}
        </Tip>
      </div>

      <!-- One muted metadata line: date · duration · speakers (read-only;
           the transcript pills are where speakers get edited), with the
           save status trailing quietly. -->
      <div class="mt-1 flex items-baseline gap-x-2 text-xs text-muted-foreground">
        <span class="shrink-0">{formatMeetingDate(meetingsStore.selectedRecord.date)}</span>
        <span class="shrink-0 tabular-nums"
          >{formatDuration(meetingsStore.selectedRecord.duration_seconds)}</span
        >
        <span class="min-w-0 flex-1 truncate py-0.5">{attendeeLine}</span>
        {#if statusText}
          <span
            class="shrink-0 {saveState === 'error' ? 'text-destructive' : 'text-muted-foreground/70'}"
          >
            {statusText}
          </span>
        {/if}
      </div>
    </div>

    <!-- Quiet underline tabs: text, a hairline, and an accent edge on the
         active one; no boxed segmented control. -->
    <div class="flex shrink-0 items-center gap-5 border-b border-border px-4">
      {#each tabs as [key, label] (key)}
        <button
          onclick={() => {
            // Leaving a document is finishing with it: don't make the edit
            // wait out its debounce in a tab nobody is looking at.
            void flushSaves();
            activeTab = key as DetailTab;
          }}
          class="-mb-px border-b-2 px-0.5 py-2 text-sm font-medium transition-colors
            {activeTab === key
            ? 'border-foreground text-foreground'
            : 'border-transparent text-muted-foreground hover:text-foreground'}"
        >
          {label}
        </button>
      {/each}
    </div>

    <div class="flex min-h-0 flex-1 flex-col">
      {#if isLoading}
        <p class="p-3 text-sm text-muted-foreground">{t.loading}</p>
      {:else if meetingsStore.error}
        <p class="p-3 text-sm text-destructive">{meetingsStore.error}</p>
      {:else if detail}
        {#if activeTab === 'summary'}
          {#key `${detail.record.id}:summary`}
            <EditorView
              bind:this={summaryViewRef}
              value={summaryDraft}
              placeholder={t.summaryPlaceholder}
              onChange={scheduleSummarySave}
              pasteMeetingId={() => detail.record.id}
              onPasteError={(message) => {
                saveState = 'error';
                saveError = message;
              }}
            />
          {/key}
        {:else if activeTab === 'notes'}
          {#key `${detail.record.id}:usernotes`}
            <EditorView
              bind:this={notesViewRef}
              value={notesDraft}
              onChange={scheduleNotesSave}
              placeholder={copy.meetings.notes.placeholder}
              stars={detail.stars}
              onStarClick={detail.audio_exists
                ? (star) => player?.seekTo(star.seconds)
                : undefined}
              pasteMeetingId={() => detail.record.id}
              onPasteError={(message) => {
                saveState = 'error';
                saveError = message;
              }}
            />
          {/key}
        {:else if detail.segments.length > 0}
          <div class="min-h-0 flex-1 px-3 py-3">
            {#key `${detail.record.id}:segments`}
              <TranscriptSegments
                bind:this={transcriptRef}
                {detail}
                onDetailChange={syncFromDetail}
                currentTime={playbackTime}
                playing={playbackActive}
                onSeek={detail.audio_exists ? (secs) => player?.seekTo(secs) : undefined}
              />
            {/key}
          </div>
        {:else}
          {#key `${detail.record.id}:transcript`}
            <EditorView
              bind:this={transcriptViewRef}
              value={transcriptDraft}
              placeholder={t.transcriptPlaceholder}
              onChange={scheduleTranscriptSave}
            />
          {/key}
        {/if}
      {/if}
    </div>

    {#if detail || !isLoading}
      <AudioPlayer
        bind:this={player}
        bind:currentTime={playbackTime}
        bind:playing={playbackActive}
        audioPath={detail?.audio_path ?? null}
        stars={detail?.stars ?? []}
        onStarActivate={onStarActivate}
      />
    {/if}
  {/if}
</div>

<svelte:window onkeydown={onDetailKeydown} />

<ConfirmDialog
  bind:open={confirmDelete}
  title={deleteConfirm.title(1)}
  body={deleteConfirm.body(1)}
  busy={isDeleting}
  onConfirm={deleteSelectedMeeting}
/>
