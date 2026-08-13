<script lang="ts">
  import { LoaderCircle, Star } from 'lucide-svelte';
  import type { PendingMeeting } from '$lib/stores/app-state.svelte';
  import { nameClass } from '$lib/utils/speakerColors';
  import { formatDuration, formatTime } from '$lib/utils/meetingFormat';
  import AudioPlayer from './AudioPlayer.svelte';
  import EditorView from './EditorView.svelte';
  import { copy } from '$lib/copy';

  const t = $derived(copy.meetings.pending);

  let { pending }: { pending: PendingMeeting } = $props();

  type PendingTab = 'notes' | 'transcript';
  let activeTab = $state<PendingTab>('transcript');
  let player = $state<AudioPlayer | null>(null);

  const labels = $derived.by(() => {
    const seen: string[] = [];
    for (const s of pending.segments) {
      if (s.speaker && !seen.includes(s.speaker)) seen.push(s.speaker);
    }
    return seen;
  });

  // Star markers mapped to the first segment at or after each star.
  const starMarkers = $derived.by(() => {
    const map = new Map<number, number[]>();
    const stars = pending.stars.map((s) => s.seconds).sort((a, b) => a - b);
    let si = 0;
    for (let i = 0; i < pending.segments.length; i++) {
      const before: number[] = [];
      while (si < stars.length && stars[si] <= pending.segments[i].start) {
        before.push(stars[si++]);
      }
      if (before.length) map.set(i, before);
    }
    if (si < stars.length) map.set(pending.segments.length, stars.slice(si));
    return map;
  });

  // Same-speaker sentences read as one turn, broken at starred moments:
  // the same grouping the saved transcript editor uses, minus the editing.
  interface Turn {
    speaker: string | null;
    start: number;
    first: number;
    texts: string[];
  }
  const turns = $derived.by(() => {
    const out: Turn[] = [];
    for (let i = 0; i < pending.segments.length; i++) {
      const seg = pending.segments[i];
      const last = out[out.length - 1];
      if (!last || (seg.speaker ?? null) !== last.speaker || starMarkers.has(i)) {
        out.push({ speaker: seg.speaker ?? null, start: seg.start, first: i, texts: [] });
      }
      out[out.length - 1].texts.push(seg.text);
    }
    return out;
  });
</script>

<div class="flex min-h-0 min-w-0 flex-1 flex-col bg-background">
  <div class="shrink-0 border-b border-border px-3 py-3">
    <h2 class="font-display text-lg leading-snug">{pending.title}</h2>
    <div class="mt-1 flex items-baseline gap-x-2 text-xs text-muted-foreground">
      <span>{t.justNow}</span>
      <span class="tabular-nums">{formatDuration(pending.durationSeconds)}</span>
    </div>
  </div>

  <!-- What's still being generated: the summary. Notes, transcript, and (as
       soon as it's encoded) the audio are already real below. -->
  <div class="flex shrink-0 items-center gap-2 border-b border-border px-3 py-2.5">
    {#if pending.error}
      <p class="text-xs text-destructive">{pending.error}</p>
    {:else}
      <LoaderCircle size={13} class="animate-spin text-muted-foreground" />
      <p class="text-xs text-muted-foreground">{t.finalizing}</p>
    {/if}
  </div>

  <div class="flex shrink-0 items-center gap-5 border-b border-border px-4">
    {#each [['notes', t.tabs.notes], ['transcript', t.tabs.transcript]] as [key, label] (key)}
      <button
        onclick={() => (activeTab = key as PendingTab)}
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
    {#if activeTab === 'notes'}
      <EditorView
        value={pending.userNotes}
        readonly
        placeholder={copy.meetings.notes.emptyView}
        stars={pending.stars}
        onStarClick={pending.audioPath
          ? (star) => player?.seekTo(star.seconds)
          : undefined}
      />
    {:else}
      <div class="min-h-0 flex-1 space-y-1 overflow-y-auto px-3 py-3">
        {#if pending.segments.length === 0}
          <p class="text-sm text-muted-foreground">{t.noSpeech}</p>
        {:else}
          {#each turns as turn (turn.first)}
            {#each starMarkers.get(turn.first) ?? [] as star (star)}
              {@render starRow(star)}
            {/each}
            <div class="rounded-md px-2 py-1.5">
              <div class="flex items-center gap-2">
                <span
                  class="w-9 shrink-0 font-mono text-[10px] tabular-nums text-muted-foreground"
                >
                  {formatTime(turn.start)}
                </span>
                {#if turn.speaker}
                  <span
                    class="shrink-0 text-[11px] font-medium {nameClass(
                      turn.speaker,
                      labels
                    )}"
                  >
                    {turn.speaker}
                  </span>
                {/if}
              </div>
              <p class="mt-0.5 pl-11 text-[15px] leading-relaxed">{turn.texts.join(' ')}</p>
            </div>
          {/each}
          {#each starMarkers.get(pending.segments.length) ?? [] as star (star)}
            {@render starRow(star)}
          {/each}
        {/if}
      </div>
    {/if}
  </div>

  {#if pending.audioPath}
    <AudioPlayer bind:this={player} audioPath={pending.audioPath} stars={pending.stars} />
  {/if}
</div>

{#snippet starRow(star: number)}
  <div class="flex items-center gap-2 px-2 py-1 text-[11px] text-muted-foreground">
    <Star size={11} fill="currentColor" class="shrink-0" />
    <span class="tabular-nums">{formatTime(star)}</span>
    <span class="h-px min-w-0 flex-1 bg-border"></span>
  </div>
{/snippet}
