<script lang="ts">
  // What this recording is capturing, changeable mid-meeting
  // ([recording.md] §Dual-stream capture). Two sections: the apps playing
  // audio, and the microphones. Everything starts checked except extra
  // mics; unchecking an app narrows the capture to the ones left, which
  // is the only way to keep a music player out of a meeting.
  //
  // The meter beside this button is the feedback: uncheck an app and watch
  // the faint bars drop.
  import { onDestroy } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { Check, SlidersHorizontal } from 'lucide-svelte';
  import { appState } from '$lib/stores/app-state.svelte';
  import { groupAudioApps, type AppGroup } from '$lib/utils/detectedApp';
  import { tip } from '$lib/tip.svelte';
  import { copy } from '$lib/copy';
  import { cn } from '$lib/utils';

  const t = $derived(copy.meetings.sources);

  type AudioApp = { pid: number; name: string };
  type Sources = { apps: AudioApp[]; mics: string[]; primary_mic: string };

  let open = $state(false);
  let sources = $state<Sources>({ apps: [], mics: [], primary_mic: '' });
  /** Checked apps; null means everything (including apps yet to appear). */
  let selectedApps = $state<number[] | null>(null);
  let extraMics = $state<string[]>([]);
  let poll: ReturnType<typeof setInterval> | null = null;

  let trigger = $state<HTMLButtonElement | null>(null);
  let panel = $state({ left: 0, top: 0, width: 0 });

  /** The panel's own width when the window has room for it (`w-72`). */
  const PANEL_WIDTH = 288;
  /** Breathing room kept at either window edge. */
  const EDGE_GAP = 12;

  // Centered under the button, but never past a window edge. Device names
  // run long ("Microphone Array (Intel® Smart Sound Technology…)") and the
  // window is often narrow; anchoring the panel's left edge to the button
  // ran it straight off the right side of the app.
  function place() {
    if (!trigger) return;
    const r = trigger.getBoundingClientRect();
    const width = Math.min(PANEL_WIDTH, window.innerWidth - EDGE_GAP * 2);
    const centered = r.left + r.width / 2 - width / 2;
    const maxLeft = window.innerWidth - width - EDGE_GAP;
    panel = { left: Math.min(Math.max(EDGE_GAP, centered), maxLeft), top: r.bottom + 6, width };
  }

  function toggleOpen() {
    open = !open;
    if (open) {
      place();
      void refresh();
      // Apps come and go mid-call; keep the list honest while it is up.
      poll = setInterval(() => void refresh(), 3000);
    } else {
      stopPoll();
    }
  }

  async function refresh() {
    sources = await invoke<Sources>('list_audio_sources').catch(() => sources);
  }

  function stopPoll() {
    if (poll) {
      clearInterval(poll);
      poll = null;
    }
  }

  /** One row per app, however many audio sessions it happens to hold. */
  const appGroups = $derived(groupAudioApps(sources.apps));

  const groupChecked = (group: AppGroup) =>
    selectedApps === null || group.pids.some((p) => selectedApps?.includes(p));

  async function toggleGroup(group: AppGroup) {
    const current = selectedApps ?? sources.apps.map((a) => a.pid);
    // The row is the app, so every session behind it moves together.
    const next = groupChecked(group)
      ? current.filter((p) => !group.pids.includes(p))
      : [...current, ...group.pids.filter((p) => !current.includes(p))];
    // Back to everything: also picks up apps that start playing later.
    const everything = sources.apps.every((a) => next.includes(a.pid));
    selectedApps = everything ? null : next;
    await invoke('set_system_audio_sources', { apps: selectedApps }).catch(() => {});
  }

  async function toggleMic(name: string) {
    if (name === sources.primary_mic) return; // the master clock stays
    extraMics = extraMics.includes(name)
      ? extraMics.filter((m) => m !== name)
      : [...extraMics, name];
    await invoke('set_extra_mics', { devices: extraMics }).catch(() => {});
  }

  // A new recording resets the picker to its defaults.
  $effect(() => {
    if (!appState.isRecording) {
      selectedApps = null;
      extraMics = [];
      open = false;
      stopPoll();
    }
  });

  function close() {
    open = false;
    stopPoll();
  }

  onDestroy(stopPoll);
</script>

<svelte:window
  onkeydown={(e) => {
    if (open && e.key === 'Escape') close();
  }}
  onresize={() => open && place()}
/>

<div class="shrink-0">
  <button
    bind:this={trigger}
    use:tip={t.aria}
    class={cn(
      'rounded-md p-2 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground',
      open && 'bg-accent text-foreground'
    )}
    aria-label={t.aria}
    aria-expanded={open}
    onclick={toggleOpen}
  >
    <SlidersHorizontal size={16} />
  </button>

  {#if open}
    <!-- Click-away closes it; the panel itself swallows clicks. -->
    <button
      class="fixed inset-0 z-40 cursor-default"
      aria-label={copy.common.close}
      onclick={close}
    ></button>
    <div
      class="fixed z-50 rounded-xl border border-border bg-popover p-3 shadow-2xl"
      style="left: {panel.left}px; top: {panel.top}px; width: {panel.width}px;"
    >
      <p class="text-[11px] font-medium tracking-wide text-muted-foreground uppercase">
        {t.systemAudio}
      </p>
      <div class="mt-1.5 space-y-1">
        {#each appGroups as group (group.label)}
          {@const checked = groupChecked(group)}
          <button
            type="button"
            class="flex w-full items-center gap-2 rounded-md px-1.5 py-1 text-left text-xs transition-colors hover:bg-accent/60"
            role="checkbox"
            aria-checked={checked}
            onclick={() => void toggleGroup(group)}
          >
            <span
              class={cn(
                'flex size-3.5 shrink-0 items-center justify-center rounded-sm border',
                checked ? 'border-primary bg-primary text-primary-foreground' : 'border-input'
              )}
            >
              {#if checked}<Check size={10} strokeWidth={3} />{/if}
            </span>
            <span class="min-w-0 truncate">{group.label}</span>
          </button>
        {:else}
          <p class="px-1.5 py-1 text-xs text-muted-foreground">{t.nothingPlaying}</p>
        {/each}
      </div>

      <p class="mt-3 text-[11px] font-medium tracking-wide text-muted-foreground uppercase">
        {t.microphones}
      </p>
      <div class="mt-1.5 space-y-1">
        {#each sources.mics as mic (mic)}
          {@const primary = mic === sources.primary_mic}
          {@const checked = primary || extraMics.includes(mic)}
          <button
            type="button"
            class={cn(
              'flex w-full items-center gap-2 rounded-md px-1.5 py-1 text-left text-xs transition-colors',
              primary ? 'cursor-default' : 'hover:bg-accent/60'
            )}
            role="checkbox"
            aria-checked={checked}
            aria-disabled={primary}
            onclick={() => void toggleMic(mic)}
          >
            <span
              class={cn(
                'flex size-3.5 shrink-0 items-center justify-center rounded-sm border',
                checked ? 'border-primary bg-primary text-primary-foreground' : 'border-input',
                primary && 'opacity-60'
              )}
            >
              {#if checked}<Check size={10} strokeWidth={3} />{/if}
            </span>
            <span class="min-w-0 truncate">{mic}</span>
            {#if primary}
              <span class="ml-auto shrink-0 text-[10px] text-muted-foreground">
                {t.primary}
              </span>
            {/if}
          </button>
        {/each}
      </div>
    </div>
  {/if}
</div>
