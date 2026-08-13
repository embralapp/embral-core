<script lang="ts">
  import { appState } from '$lib/stores/app-state.svelte';

  /** A live spectrum: one stationary bar per frequency band (low → high),
   * moving up and down with the current audio. Each bar stacks the
   * microphone's share (solid tone, at the bottom) under the system
   * audio's share (muted tone); sources, never identity. Quiet by
   * design: no numbers, no color coding beyond the two tones. */
  const BAND_COUNT = 24;
  /** Rough visual gain: speech band magnitudes sit well below full scale. */
  const GAIN = 40;

  type Bar = { mic: number; system: number };

  let bars = $derived.by(() => {
    const { mic, system } = appState.levelBands;
    const out: Bar[] = [];
    for (let i = 0; i < BAND_COUNT; i++) {
      out.push({ mic: mic[i] ?? 0, system: system[i] ?? 0 });
    }
    return out;
  });

  /** Per-bar stacked heights in %, square-rooted so quiet speech still
   * reads. */
  function heights(b: Bar): { mic: number; system: number } {
    const sum = b.mic + b.system;
    if (sum <= 0) return { mic: 0, system: 0 };
    const total = Math.min(1, Math.sqrt(sum * GAIN)) * 100;
    return { mic: (total * b.mic) / sum, system: (total * b.system) / sum };
  }
</script>

<div
  class="flex h-6 shrink-0 items-end gap-px transition-opacity duration-250 {appState.isPaused
    ? 'opacity-25'
    : 'opacity-70'}"
  aria-hidden="true"
>
  {#each bars as b, i (i)}
    {@const h = heights(b)}
    <div class="flex h-full w-[3px] flex-col justify-end">
      <div
        class="w-full rounded-[1px] bg-muted-foreground/40 transition-[height] duration-100 ease-linear"
        style="height: {h.system}%"
      ></div>
      <div
        class="w-full rounded-[1px] bg-foreground/70 transition-[height] duration-100 ease-linear"
        style="height: {h.mic}%"
      ></div>
    </div>
  {/each}
</div>
