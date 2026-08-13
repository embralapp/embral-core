<script lang="ts">
  import { Upload, LoaderCircle, Trash2 } from 'lucide-svelte';
  import { meetingsStore, PENDING_MEETING_ID } from '$lib/stores/meetings.svelte';
  import { appState } from '$lib/stores/app-state.svelte';
  import type { MeetingRecord } from '$lib/types';
  import {
    formatDuration,
    formatMeetingDate,
    formatMeetingTime,
    groupByDate,
    isSingleDayGroup
  } from '$lib/utils/meetingFormat';
  import { importRecording } from '$lib/utils/importRecording';
  import OverlayScroll from '$lib/components/OverlayScroll.svelte';
  import Tip from '$lib/components/Tip.svelte';
  import * as ContextMenu from '$lib/components/ui/context-menu';
  import { copy } from '$lib/copy';

  const t = $derived(copy.meetings.list);

  let { onSelect, onDelete }: { onSelect?: () => void; onDelete?: () => void } = $props();

  const selection = $derived(meetingsStore.selection);

  /** How many meetings the row menu's Delete would take (the pending
   * meeting has no row to delete). */
  const menuCount = $derived(
    selection.ids.filter((id) => id !== PENDING_MEETING_ID).length
  );

  /** The menu acts on the selection, so a right-click outside it moves the
   * selection first, exactly what a plain click would have done. */
  function onRowContextMenu(id: string, event: MouseEvent) {
    if (!selection.has(id)) void meetingsStore.clickRow(id, event, visibleOrder);
  }

  /** A row is either a saved meeting or the one still being processed. */
  type Row = { kind: 'pending' } | { kind: 'record'; record: MeetingRecord };

  // Rows under date headers, preserving the store's newest-first order. The
  // pending meeting joins Today: it is grouped by now rather than by when
  // it started, because "Finishing up…" is a statement about the present (and
  // that also settles the meeting that ended before midnight and is still
  // processing this morning).
  let groups = $derived.by(() => {
    const now = new Date();
    const groups: { label: string; rows: Row[] }[] = groupByDate(
      meetingsStore.records,
      (record) => record.date,
      now
    ).map((group) => ({
      label: group.label,
      rows: group.items.map((record): Row => ({ kind: 'record', record }))
    }));

    if (appState.pendingMeeting) {
      const todayLabel = copy.meetings.dateGroups.today;
      if (groups[0]?.label === todayLabel) {
        groups[0].rows.unshift({ kind: 'pending' });
      } else {
        groups.unshift({ label: todayLabel, rows: [{ kind: 'pending' }] });
      }
    }
    return groups;
  });

  /** The rows as they appear on screen, headers ignored: a Shift-range is
   * measured over this, so it crosses date groups the way the eye expects. */
  let visibleOrder = $derived(
    groups.flatMap((group) =>
      group.rows.map((row) => (row.kind === 'pending' ? PENDING_MEETING_ID : row.record.id))
    )
  );

  function onRowClick(id: string, event: MouseEvent) {
    void meetingsStore.clickRow(id, event, visibleOrder);
    // A modified click is building a selection, not opening a meeting: on a
    // narrow window it must not swap the pane out from under the user.
    if (!event.shiftKey && !event.ctrlKey && !event.metaKey) onSelect?.();
  }

  /** Under a header that already names the day, rows show the time; the range
   * headers (Last week, June 2026) leave the day to the row. */
  function rowDate(record: MeetingRecord, groupLabel: string): string {
    return isSingleDayGroup(groupLabel)
      ? formatMeetingTime(record.date)
      : formatMeetingDate(record.date);
  }

  /** The row under the oldest loaded meeting. Scrolling it into view is what
   * asks for the next page, so the list has no bottom until the library
   * does. */
  let sentinel = $state<HTMLElement | null>(null);
  const loaded = $derived(meetingsStore.records.length);

  /** The element that actually scrolls, which is inside OverlayScroll, not
   * the window. An observer rooted anywhere else would treat rows clipped by
   * that box as visible and fetch the whole library at once. */
  function scrollBox(node: HTMLElement): HTMLElement | null {
    for (let el = node.parentElement; el; el = el.parentElement) {
      const overflow = getComputedStyle(el).overflowY;
      if (overflow === 'auto' || overflow === 'scroll') return el;
    }
    return null;
  }

  // Rebuilt whenever rows arrive, and that is the point: an observer reports
  // changes, so a page that did not push the sentinel back out of view would
  // leave it sitting there "already visible" and the page after it would
  // never be asked for. Observing afresh re-reports where things stand.
  $effect(() => {
    const node = sentinel;
    // Read so the effect re-runs on a new page; without rows there is
    // nothing to continue from anyway.
    if (!node || loaded === 0) return;

    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) void meetingsStore.loadMore();
      },
      // A screenful of margin, so the rows are already there by the time the
      // user scrolls onto them.
      { root: scrollBox(node), rootMargin: '600px' }
    );
    observer.observe(node);
    return () => observer.disconnect();
  });

  /** The primary row (the one the detail pane shows) carries the accent edge;
   * the other rows in a multi-selection get the wash alone, so it is always
   * clear which one you are reading. */
  const rowClass = (id: string) => {
    const primary = meetingsStore.selectedId === id;
    const selected = selection.has(id);
    return `w-full border-l-2 px-3 py-2 text-left transition-colors duration-150 ${
      primary
        ? 'border-l-foreground/60 bg-accent/50'
        : selected
          ? 'border-l-transparent bg-accent/40'
          : 'border-l-transparent hover:bg-accent/40'
    }`;
  };
</script>

<!-- No search bar here; the titlebar command bar owns meeting search. -->
<div
  class="relative flex flex-col flex-1 w-full min-w-0 min-h-0 bg-muted/20 min-[960px]:border-r min-[960px]:border-border"
>
  <OverlayScroll>
    <div class="pb-16">
    {#if meetingsStore.isLoading}
      <p class="px-3 py-4 text-sm text-muted-foreground">{t.loading}</p>
    {:else if groups.length === 0}
      <p class="px-3 py-4 text-sm text-muted-foreground">
        {t.empty}
      </p>
    {:else}
      {#each groups as group (group.label)}
        <p
          class="px-3 pt-3 pb-1 text-[11px] font-medium tracking-wide text-muted-foreground/80 uppercase"
        >
          {group.label}
        </p>
        {#each group.rows as row (row.kind === 'pending' ? PENDING_MEETING_ID : row.record.id)}
          {#if row.kind === 'pending'}
            {@const pending = appState.pendingMeeting}
            {#if pending}
              <button
                onclick={(e) => onRowClick(PENDING_MEETING_ID, e)}
                class={rowClass(PENDING_MEETING_ID)}
              >
                <h3 class="font-display line-clamp-2 text-sm leading-snug">{pending.title}</h3>
                <div
                  class="mt-0.5 flex items-center justify-between gap-2 text-[11px] text-muted-foreground"
                >
                  <span class="inline-flex items-center gap-1.5 truncate">
                    <LoaderCircle size={11} class="shrink-0 animate-spin" />
                    {t.finishingUp}
                  </span>
                  <span class="shrink-0 tabular-nums">
                    {formatDuration(pending.durationSeconds)}
                  </span>
                </div>
              </button>
            {/if}
          {:else}
            {@const record = row.record}
            <ContextMenu.Root>
              <ContextMenu.Trigger>
                {#snippet child({ props })}
                  <button
                    {...props}
                    onclick={(e) => onRowClick(record.id, e)}
                    oncontextmenu={(e) => {
                      onRowContextMenu(record.id, e);
                      (props as { oncontextmenu?: (ev: MouseEvent) => void }).oncontextmenu?.(e);
                    }}
                    class={rowClass(record.id)}
                  >
                    <h3 class="font-display line-clamp-2 text-sm leading-snug">{record.title}</h3>
                    <div
                      class="mt-0.5 flex items-center justify-between gap-2 text-[11px] text-muted-foreground"
                    >
                      <span class="truncate">{rowDate(record, group.label)}</span>
                      <span class="shrink-0 tabular-nums">
                        {formatDuration(record.duration_seconds)}
                      </span>
                    </div>
                  </button>
                {/snippet}
              </ContextMenu.Trigger>
              <ContextMenu.Content>
                <ContextMenu.Item variant="destructive" onSelect={() => onDelete?.()}>
                  <Trash2 />
                  {t.menuDelete(menuCount)}
                </ContextMenu.Item>
              </ContextMenu.Content>
            </ContextMenu.Root>
          {/if}
        {/each}
      {/each}
      {#if meetingsStore.hasMore}
        <p bind:this={sentinel} class="px-3 py-4 text-sm text-muted-foreground">
          {t.loadingMore}
        </p>
      {/if}
    {/if}
    </div>
  </OverlayScroll>

  <Tip side="left" text={t.import}>
    {#snippet children({ props })}
      <button
        {...props}
        onclick={importRecording}
        class="absolute right-3 bottom-3 flex h-10 w-10 items-center justify-center rounded-2xl bg-primary text-primary-foreground shadow-md transition-colors hover:bg-primary/90"
        aria-label={t.import}
      >
        <Upload size={17} />
      </button>
    {/snippet}
  </Tip>
</div>
