<script lang="ts">
	import { errorMessage } from '$lib/copy/errors';
  import { onMount, tick } from 'svelte';
  import { Play, Scissors, Star, Trash2, UserPen, X } from 'lucide-svelte';
  import type { MeetingDetail, TranscriptionSegment } from '$lib/types';
  import { meetingsStore } from '$lib/stores/meetings.svelte';
  import { speakersStore } from '$lib/stores/speakers.svelte';
  import { nameClass } from '$lib/utils/speakerColors';
  import { formatTime } from '$lib/utils/meetingFormat';
  import { charLen, startsNewParagraph } from '$lib/utils/transcriptBreaks';
  import { cn } from '$lib/utils';
  import SpeakerNameInput from './SpeakerNameInput.svelte';
  import * as ContextMenu from '$lib/components/ui/context-menu';
  import { tip } from '$lib/tip.svelte';
  import CopyParts from './CopyParts.svelte';
  import { copy } from '$lib/copy';

  const t = $derived(copy.meetings.transcript);

  let {
    detail,
    onDetailChange,
    currentTime = 0,
    playing = false,
    onSeek
  }: {
    detail: MeetingDetail;
    onDetailChange?: (updated: MeetingDetail) => void;
    /** Playback position (seconds); highlights the current segment. */
    currentTime?: number;
    playing?: boolean;
    /** Seek-and-play from a segment's timestamp; absent = no audio. */
    onSeek?: (seconds: number) => void;
  } = $props();

  let busy = $state(false);
  let error = $state<string | null>(null);
  // Sentence whose speaker is being edited (via the context menu), and its
  // draft value. The sentence lifts out as its own turn while the caret is
  // in it (see the grouping rule below).
  let editingRow = $state<number | null>(null);
  let rowSpeakerDraft = $state('');
  // Turn whose speaker is being edited via its header name.
  let editingTurn = $state<number | null>(null);
  let turnDraft = $state('');
  // Label being renamed via its header name.
  let editingLabel = $state<string | null>(null);
  let labelDraft = $state('');
  // Turn armed for click-to-split.
  let splittingTurn = $state<number | null>(null);
  // The turn under the pointer (or holding focus). Turns mount their action
  // cluster and play affordance only while hovered: a thousand rows of
  // always-mounted icons and tooltips is what made this tab slow.
  let hoverTurn = $state<number | null>(null);
  // The sentence a right-click landed on: the context menu's target.
  let menuSeg = $state<number | null>(null);

  onMount(() => {
    if (!speakersStore.loaded) void speakersStore.refresh();
  });

  const meetingId = $derived(detail.record.id);
  const segments = $derived(detail.segments);
  // Starred moments, mapped to the first segment starting at or after each
  // star (index == segments.length collects trailing stars).
  const starMarkers = $derived.by(() => {
    const map = new Map<number, number[]>();
    const stars = (detail.stars ?? []).map((s) => s.seconds).sort((a, b) => a - b);
    let si = 0;
    for (let i = 0; i < segments.length; i++) {
      const before: number[] = [];
      while (si < stars.length && stars[si] <= segments[i].start) {
        before.push(stars[si++]);
      }
      if (before.length) map.set(i, before);
    }
    if (si < stars.length) map.set(segments.length, stars.slice(si));
    return map;
  });
  const labels = $derived.by(() => {
    const seen: string[] = [];
    for (const s of segments) {
      if (s.speaker && !seen.includes(s.speaker)) seen.push(s.speaker);
    }
    return seen;
  });

  // --- Turns: the list reads as one block per speaker turn (consecutive
  // sentences from the same label flow together as a paragraph) because a
  // row per sentence made long transcripts a wall to scroll. The sentences
  // and their timings are untouched underneath: each is still its own
  // clickable span (seek, split target, context-menu target). A turn also
  // breaks at a starred moment (its marker row sits between turns) and at
  // a sentence whose speaker is being edited, which lifts out on its own
  // while the caret is in it.
  interface TurnItem {
    seg: TranscriptionSegment;
    index: number;
  }
  interface Turn {
    speaker: string | null;
    start: number;
    /** Index of the turn's first segment: the stable render key. */
    first: number;
    items: TurnItem[];
  }
  const turns = $derived.by(() => {
    const out: Turn[] = [];
    // The shared paragraph rules (gaps, sentence breaks, running length;
    // the same ones the stored markdown uses), plus this surface's own
    // breaks: a starred moment, and the row under edit. Without the
    // shared rules a speakerless meeting rendered as one turn.
    let runningLen = 0;
    for (let i = 0; i < segments.length; i++) {
      const seg = segments[i];
      const last = out[out.length - 1];
      if (
        !last ||
        startsNewParagraph(segments[i - 1], seg, runningLen) ||
        starMarkers.has(i) ||
        editingRow === i ||
        editingRow === i - 1
      ) {
        out.push({ speaker: seg.speaker ?? null, start: seg.start, first: i, items: [] });
        runningLen = charLen(seg.text);
      } else {
        runningLen += charLen(seg.text) + 1;
      }
      out[out.length - 1].items.push({ seg, index: i });
    }
    return out;
  });
  /** Segment index → its turn's ordinal, for scroll and render targets. */
  const turnOf = $derived.by(() => {
    const map: number[] = [];
    turns.forEach((turn, g) => turn.items.forEach(({ index }) => (map[index] = g)));
    return map;
  });

  // --- Progressive render: an instant first screen, the rest mounted in
  // idle time. Long transcripts made the tab switch stall for seconds when
  // every row mounted synchronously. Counted in turns: the worst case
  // (every sentence its own turn) is the old per-row behavior.
  const INITIAL_TURNS = 200;
  const RENDER_BATCH = 400;
  let renderCount = $state(INITIAL_TURNS);
  const fullyRendered = $derived(renderCount >= turns.length);

  $effect(() => {
    if (fullyRendered) return;
    const grow = () =>
      (renderCount = Math.min(turns.length, renderCount + RENDER_BATCH));
    if (typeof requestIdleCallback === 'function') {
      const id = requestIdleCallback(grow);
      return () => cancelIdleCallback(id);
    }
    const timeout = setTimeout(grow, 0);
    return () => clearTimeout(timeout);
  });

  /** Mount up to a target segment's turn now: scroll targets can sit past
   * the rendered window while the idle growth is still catching up. */
  async function ensureRendered(index: number) {
    const g = turnOf[index] ?? turns.length - 1;
    if (g < renderCount) return;
    renderCount = Math.min(turns.length, g + 30);
    await tick();
  }

  // --- Playback sync: highlight the segment under the playhead and follow
  // it while playing; scrolling by hand unfollows until the "Jump to
  // current" pill re-pins.
  const activeIndex = $derived.by(() => {
    if (!playing && currentTime <= 0) return -1;
    for (let i = segments.length - 1; i >= 0; i--) {
      if (currentTime >= segments[i].start) {
        return currentTime < segments[i].end + 0.75 ? i : -1;
      }
    }
    return -1;
  });

  // Sentence spans by segment index: scroll targets for playback follow
  // and search landings stay sentence-precise inside the grouped turns.
  let sentenceEls: (HTMLElement | null)[] = [];
  let following = $state(true);

  // Unfollow only on the user's own scrolling: wheel, touch, or grabbing
  // the scrollbar (a pointerdown whose target is the scroller itself).
  // Watching the scroll event needed an "is this scroll ours" flag whose
  // timer raced the smooth-scroll animation, and a follow scroll that
  // outlasted it read as the user scrolling off.
  function unfollowIfScrollbar(e: PointerEvent) {
    if (e.target === e.currentTarget) following = false;
  }

  function jumpToCurrent() {
    following = true;
    void scrollActiveIntoView('smooth');
  }

  /** Re-pin to the playhead (player star clicks) so the seek's landing
   * segment scrolls into view. */
  export function followPlayhead() {
    following = true;
  }

  /** Bring one line to the middle: how a search result arrives.
   *
   * Takes the index rather than reading the playhead: the row list is
   * virtualized and the playhead's own segment is derived from a time the
   * media element is still settling on, so "scroll to whatever is active"
   * raced both. The caller knows exactly which line it matched.
   *
   * `center` where playback following uses `nearest`: following moves as
   * little as possible while audio runs, but arriving from somewhere else
   * wants the line in the middle, with its lead-up visible above it. */
  export async function revealIndex(index: number) {
    if (index < 0 || index >= segments.length) return;
    following = true;
    await ensureRendered(index);
    // Centre by arithmetic rather than `scrollIntoView`: the list is
    // virtualized, so rows keep rendering after the scroll starts and the
    // target drifts under it; a smooth `scrollIntoView` reliably finished
    // a screen short. Set the position, then correct once more after the
    // layout has settled.
    centreRow(index);
    requestAnimationFrame(() => centreRow(index));
    setTimeout(() => centreRow(index), 200);
  }

  function centreRow(index: number, behavior: ScrollBehavior = 'auto') {
    const el = sentenceEls[index];
    const scroller = el?.closest<HTMLElement>('.overflow-y-auto');
    if (!el || !scroller) return;
    const row = el.getBoundingClientRect();
    const box = scroller.getBoundingClientRect();
    scroller.scrollTo({
      top: scroller.scrollTop + row.top - box.top - (box.height - row.height) / 2,
      behavior
    });
  }

  /** Playback follow keeps the current sentence in the middle of the
   * viewport: riding the bottom edge left no read-ahead below the line. */
  async function scrollActiveIntoView(behavior: ScrollBehavior) {
    if (activeIndex < 0) return;
    await ensureRendered(activeIndex);
    centreRow(activeIndex, behavior);
  }

  $effect(() => {
    activeIndex;
    if (playing && following && activeIndex >= 0) {
      void scrollActiveIntoView('smooth');
    }
  });

  /// Suggestion pool for a name editor: registry profiles plus the
  /// meeting's other labels (typing another label merges into it).
  function nameSuggestions(current: string | null): string[] {
    return [
      ...speakersStore.speakers.map((p) => p.name),
      ...labels.filter((l) => l !== current)
    ];
  }

  /// The registry id for a person whose name matches, so manual renames to a
  /// known person link the segments too.
  function registryIdFor(name: string): string | null {
    const person = speakersStore.speakers.find(
      (p) => p.name.toLowerCase() === name.trim().toLowerCase()
    );
    return person?.id ?? null;
  }

  async function apply(fn: () => Promise<MeetingDetail | undefined>) {
    if (busy) return;
    busy = true;
    error = null;
    try {
      const updated = await fn();
      if (updated) onDetailChange?.(updated);
      // An edit can prune a newly-orphaned profile (a corrected typo);
      // refresh so it leaves the suggestion lists too.
      void speakersStore.refresh();
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = false;
    }
  }

  function startRowEdit(index: number) {
    if (busy) return;
    editingRow = index;
    rowSpeakerDraft = segments[index]?.speaker ?? '';
    editingTurn = null;
    splittingTurn = null;
  }

  async function commitRowEdit(index: number) {
    const speaker = rowSpeakerDraft.trim();
    editingRow = null;
    if (!speaker || speaker === segments[index]?.speaker) return;
    await apply(() =>
      meetingsStore.editSegments(meetingId, {
        kind: 'reassign',
        index,
        speaker,
        speaker_id: registryIdFor(speaker)
      })
    );
  }

  function startTurnEdit(g: number) {
    if (busy) return;
    editingTurn = g;
    turnDraft = turns[g]?.speaker ?? '';
    editingRow = null;
    splittingTurn = null;
  }

  /** Rename a whole turn in one edit: the turn's rows are contiguous by
   * construction, so a single index-range reassign covers it (one
   * document regeneration however long the turn), which is what makes
   * naming a speakerless meeting practical ([speakers.md]). */
  async function commitTurnEdit(g: number) {
    const turn = turns[g];
    const speaker = turnDraft.trim();
    editingTurn = null;
    if (!turn || !speaker || speaker === turn.speaker) return;
    const speaker_id = registryIdFor(speaker);
    await apply(() =>
      meetingsStore.editSegments(meetingId, {
        kind: 'reassign_range',
        from_index: turn.items[0].index,
        to_index: turn.items[turn.items.length - 1].index,
        speaker,
        speaker_id
      })
    );
  }

  async function deleteRow(index: number) {
    await apply(() => meetingsStore.editSegments(meetingId, { kind: 'delete', index }));
  }

  function armSplit(g: number) {
    splittingTurn = splittingTurn === g ? null : g;
    editingRow = null;
    editingTurn = null;
  }

  async function splitAtSelection(index: number) {
    const selection = window.getSelection();
    if (!selection || selection.rangeCount === 0) return;
    const offset = selection.getRangeAt(0).startOffset;
    splittingTurn = null;
    if (offset <= 0 || offset >= (segments[index]?.text.length ?? 0)) return;
    await apply(() =>
      meetingsStore.editSegments(meetingId, {
        kind: 'split',
        index,
        char_offset: offset
      })
    );
  }

  /** A sentence click: the split target while its turn is armed, ignored
   * mid-text-selection (copying, not seeking), otherwise a seek, which
   * also re-pins the follow: "go here" is the opposite of scrolling off. */
  function onSentenceClick(g: number, index: number) {
    if (splittingTurn === g) {
      void splitAtSelection(index);
      return;
    }
    const selection = window.getSelection();
    if (selection && !selection.isCollapsed) return;
    if (onSeek) following = true;
    onSeek?.(segments[index].start);
  }

  function startLabelEdit(label: string) {
    editingLabel = label;
    labelDraft = label;
  }

  /** "Speaker N" is a machine label, never worth a profile. */
  function isGenericLabel(label: string): boolean {
    return /^Speaker \d+$/.test(label);
  }

  async function commitLabelEdit() {
    const from = editingLabel;
    const to = labelDraft.trim();
    editingLabel = null;
    if (!from || !to || to === from) return;
    // A real name that isn't in the registry yet becomes a profile on the
    // spot, so the rename also links the segments.
    let speakerId = registryIdFor(to);
    if (!speakerId && !isGenericLabel(to)) {
      const created = await speakersStore.save({
        name: to,
        notes: ''
      });
      speakerId = created?.id ?? null;
    }
    await apply(() =>
      meetingsStore.editSegments(meetingId, {
        kind: 'relabel_all',
        from,
        to,
        speaker_id: speakerId
      })
    );
  }

  /// Remove a label from the transcript: its segments become unattributed.
  async function clearLabel(label: string) {
    await apply(() => meetingsStore.editSegments(meetingId, { kind: 'clear_label', label }));
  }

  async function confirmName(label: string, name: string) {
    await apply(() => meetingsStore.confirmNameSuggestion(meetingId, label, name));
  }

  async function dismissName(label: string) {
    await apply(() => meetingsStore.dismissNameSuggestion(meetingId, label));
  }
</script>

{#snippet starRow(star: number)}
  <!-- A starred moment at its place in the transcript; clicking plays it. The
       tooltip follows the enabled state for free: a disabled button emits no
       pointer events, so it never opens without a player to seek. -->
  <button
    use:tip={t.playFromHere}
    class="flex w-full items-center gap-2 px-2 py-1 text-[11px] text-muted-foreground transition-colors {onSeek
      ? 'hover:text-foreground'
      : 'cursor-default'}"
    disabled={!onSeek}
    onclick={() => onSeek?.(star)}
  >
    <Star size={11} fill="currentColor" class="shrink-0" />
    <span class="tabular-nums">{formatTime(star)}</span>
    <span class="h-px min-w-0 flex-1 bg-border"></span>
  </button>
{/snippet}

<div class="flex h-full min-h-0 flex-col">
  {#if detail.name_suggestions.length > 0}
    <div class="mb-2 shrink-0 space-y-1.5">
      {#each detail.name_suggestions as sug (sug.label)}
        <div
          class="flex items-center justify-between gap-3 rounded-lg border border-primary/30 bg-primary/5 px-3 py-2"
        >
          <p class="min-w-0 truncate text-xs">
            <CopyParts parts={t.suggestion(sug.label, sug.name)}>
              {#snippet part(slot, text)}
                {#if slot === 'strong'}<span class="font-medium">{text}</span
                  >{:else if slot === 'muted'}<span class="text-muted-foreground"
                    >{text}</span
                  >{/if}
              {/snippet}
            </CopyParts>
          </p>
          <div class="flex shrink-0 items-center gap-1">
            <button
              class="inline-flex items-center gap-1 rounded-md bg-primary px-2 py-1 text-[11px] font-medium text-primary-foreground transition-colors hover:bg-primary/90"
              disabled={busy}
              onclick={() => confirmName(sug.label, sug.name)}
            >
              {t.suggestionApply}
            </button>
            <button
              class="rounded-md px-2 py-1 text-[11px] text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
              disabled={busy}
              onclick={() => dismissName(sug.label)}
            >
              {t.suggestionDismiss}
            </button>
          </div>
        </div>
      {/each}
    </div>
  {/if}

  {#if labels.length > 0}
    <div class="mb-2 flex shrink-0 flex-wrap items-center gap-1.5">
      {#each labels as label (label)}
        {#if editingLabel === label}
          <SpeakerNameInput
            bind:value={labelDraft}
            suggestions={nameSuggestions(label)}
            class={cn('text-[11px] font-medium', nameClass(label, labels))}
            onCommit={commitLabelEdit}
            onCancel={() => (editingLabel = null)}
          />
        {:else}
          <button
            use:tip={t.renameSpeaker}
            class={cn(
              'text-[11px] font-medium underline-offset-4 transition-opacity hover:underline hover:opacity-75',
              nameClass(label, labels)
            )}
            onclick={() => startLabelEdit(label)}
            oncontextmenu={(e) => {
              e.preventDefault();
              void clearLabel(label);
            }}
          >
            {label}
          </button>
        {/if}
      {/each}
    </div>
  {/if}

  {#if error}
    <p class="mb-2 shrink-0 text-xs text-destructive">{error}</p>
  {/if}

  <div class="relative min-h-0 flex-1">
  <!-- svelte-ignore a11y_no_static_element_interactions -- the handlers
       only detect the user scrolling off; scrolling stays native. -->
  <div
    onwheel={() => (following = false)}
    ontouchmove={() => (following = false)}
    onpointerdown={unfollowIfScrollbar}
    class="h-full space-y-1 overflow-y-auto pr-1"
  >
    {#each turns.slice(0, renderCount) as turn, g (turn.first)}
      {#each starMarkers.get(turn.first) ?? [] as star (star)}
        {@render starRow(star)}
      {/each}
      <ContextMenu.Root>
        <!-- svelte-ignore a11y_no_static_element_interactions -- hover
             tracking only; the turn's controls carry their own semantics. -->
        <div
          onpointerenter={() => (hoverTurn = g)}
          onpointerleave={() => (hoverTurn = null)}
          onfocusin={() => (hoverTurn = g)}
          class={cn(
            'rounded-md border-l-2 border-transparent px-2 py-1.5 transition-colors duration-150 hover:bg-accent/40',
            activeIndex >= 0 && turnOf[activeIndex] === g && 'border-l-foreground/60',
            splittingTurn === g && 'bg-primary/5 ring-1 ring-primary/40'
          )}
        >
          <div class="flex items-center gap-2">
            {#if onSeek}
              <button
                use:tip={t.playFromHere}
                class="relative w-9 shrink-0 text-left font-mono text-[10px] tabular-nums text-muted-foreground transition-colors hover:text-foreground"
                aria-label={t.playFrom(formatTime(turn.start))}
                onclick={() => {
                  following = true;
                  onSeek?.(turn.start);
                }}
              >
                <span class={hoverTurn === g ? 'opacity-0' : ''}>{formatTime(turn.start)}</span>
                {#if hoverTurn === g}
                  <span class="absolute inset-y-0 left-0 flex items-center">
                    <Play size={11} fill="currentColor" />
                  </span>
                {/if}
              </button>
            {:else}
              <span class="w-9 shrink-0 font-mono text-[10px] tabular-nums text-muted-foreground">
                {formatTime(turn.start)}
              </span>
            {/if}
            {#if editingTurn === g}
              <SpeakerNameInput
                bind:value={turnDraft}
                suggestions={nameSuggestions(turn.speaker)}
                class={cn('text-[11px] font-medium', nameClass(turn.speaker ?? '', labels))}
                onCommit={() => commitTurnEdit(g)}
                onCancel={() => (editingTurn = null)}
              />
            {:else if editingRow === turn.first && turn.items.length === 1}
              <SpeakerNameInput
                bind:value={rowSpeakerDraft}
                suggestions={nameSuggestions(turn.speaker)}
                class={cn('text-[11px] font-medium', nameClass(turn.speaker ?? '', labels))}
                onCommit={() => commitRowEdit(turn.first)}
                onCancel={() => (editingRow = null)}
              />
            {:else if turn.speaker}
              <button
                use:tip={t.changeTurnSpeaker}
                class={cn(
                  'shrink-0 text-[11px] font-medium underline-offset-4 transition-opacity hover:underline hover:opacity-75',
                  nameClass(turn.speaker, labels)
                )}
                disabled={busy}
                onclick={() => startTurnEdit(g)}
              >
                {turn.speaker}
              </button>
            {/if}
            <span class="min-w-0 flex-1"></span>
            {#if hoverTurn === g || splittingTurn === g}
              <!-- Mounted, not revealed: these existed opacity-0 on every row
                   and their tooltip trees dominated the tab's render cost. -->
              <div class="flex h-5 shrink-0 items-center gap-0.5">
                {#if !turn.speaker}
                  <!-- Labeled turns edit by clicking the name itself; this is
                       the affordance for turns that have no name to click. -->
                  <button
                    use:tip={t.assignSpeaker}
                    class="rounded p-1 text-muted-foreground hover:bg-accent hover:text-foreground"
                    aria-label={t.assignSpeaker}
                    disabled={busy}
                    onclick={() => startTurnEdit(g)}
                  >
                    <UserPen size={12} />
                  </button>
                {/if}
                <button
                  use:tip={t.splitSegment}
                  class={cn(
                    'rounded p-1 hover:bg-accent',
                    splittingTurn === g
                      ? 'text-primary'
                      : 'text-muted-foreground hover:text-foreground'
                  )}
                  aria-label={t.splitSegment}
                  disabled={busy}
                  onclick={() => armSplit(g)}
                >
                  {#if splittingTurn === g}<X size={12} />{:else}<Scissors size={12} />{/if}
                </button>
              </div>
            {:else}
              <!-- Height placeholder so hovering a turn never reflows it. -->
              <div class="h-5 shrink-0"></div>
            {/if}
          </div>
          {#if splittingTurn === g}
            <p class="mt-0.5 pl-11 text-[10px] text-primary">
              {t.splitHint}
            </p>
          {/if}
          <ContextMenu.Trigger>
            {#snippet child({ props })}
              <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
              <p
                {...props}
                oncontextmenu={(e: MouseEvent) => {
                  const el = (e.target as HTMLElement).closest<HTMLElement>('[data-seg]');
                  menuSeg = el ? Number(el.dataset.seg) : null;
                  if (menuSeg === null) return;
                  (props as { oncontextmenu?: (ev: MouseEvent) => void }).oncontextmenu?.(e);
                }}
                class="mt-0.5 pl-11 text-[15px] leading-relaxed"
              >
                {#each turn.items as item (item.index)}
                  <span
                    bind:this={sentenceEls[item.index]}
                    data-seg={item.index}
                    class={cn(
                      'rounded-sm transition-colors duration-150',
                      splittingTurn === g
                        ? 'cursor-text select-text hover:bg-primary/10'
                        : onSeek && 'cursor-pointer hover:bg-accent/70',
                      activeIndex === item.index && 'bg-accent/80'
                    )}
                    onclick={() => onSentenceClick(g, item.index)}
                  >{item.seg.text}</span>{' '}
                {/each}
              </p>
            {/snippet}
          </ContextMenu.Trigger>
        </div>
        <ContextMenu.Content>
          {#if onSeek}
            <ContextMenu.Item
              onSelect={() => menuSeg !== null && onSeek?.(segments[menuSeg]?.start ?? 0)}
            >
              <Play />
              {t.playFrom(formatTime(menuSeg !== null ? (segments[menuSeg]?.start ?? 0) : 0))}
            </ContextMenu.Item>
          {/if}
          <ContextMenu.Item onSelect={() => menuSeg !== null && startRowEdit(menuSeg)}>
            <UserPen />
            {t.changeSpeaker}
          </ContextMenu.Item>
          <ContextMenu.Item
            variant="destructive"
            onSelect={() => menuSeg !== null && deleteRow(menuSeg)}
          >
            <Trash2 />
            {t.deleteSegment}
          </ContextMenu.Item>
        </ContextMenu.Content>
      </ContextMenu.Root>
    {/each}
    {#if fullyRendered}
      {#each starMarkers.get(segments.length) ?? [] as star (star)}
        {@render starRow(star)}
      {/each}
    {/if}
  </div>

  {#if playing && !following && activeIndex >= 0}
    <button
      onclick={jumpToCurrent}
      class="absolute bottom-3 left-1/2 inline-flex -translate-x-1/2 items-center rounded-full border border-border bg-background/95 px-3 py-1 text-[11px] font-medium text-muted-foreground shadow-sm transition-colors hover:text-foreground"
    >
      {t.jumpToCurrent}
    </button>
  {/if}
  </div>
</div>
