<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { Eye, EyeOff, Pause, Play, Square, Star } from 'lucide-svelte';
  import { appState } from '$lib/stores/app-state.svelte';
  import { errorMessage } from '$lib/copy/errors';
  import { formatTime } from '$lib/utils/meetingFormat';
  import LevelRibbon from './LevelRibbon.svelte';
  import SourcePicker from './SourcePicker.svelte';
  import Tip from './Tip.svelte';
  import { copy } from '$lib/copy';

  const t = $derived(copy.meetings.recording);

  let {
    userNotes = $bindable(''),
    meetingTitle = $bindable(''),
    onStar
  }: { userNotes: string; meetingTitle: string; onStar?: () => void } = $props();

  // Elapsed time derives from the store's recording clock (backend start
  // instant, paused spans excluded); the interval only refreshes `now`, so
  // the timer survives leaving and re-entering this view.
  let now = $state(Date.now());
  let elapsed = $derived(appState.elapsedSeconds(now));

  $effect(() => {
    if (appState.isRecording && !appState.isPaused) {
      now = Date.now();
      const interval = setInterval(() => {
        now = Date.now();
      }, 1000);
      return () => clearInterval(interval);
    }
  });


  async function togglePause() {
    try {
      if (appState.isPaused) {
        await invoke('resume_recording');
        appState.setPaused(false);
      } else {
        await invoke('pause_recording');
        appState.setPaused(true);
      }
    } catch (e) {
      appState.setError(errorMessage(e));
    }
  }

  async function stop() {
    // The pending meeting on the Meetings page carries this title until the
    // persisted record replaces it. The notes/title drafts are not cleared
    // here: the pending view still reads them; a new recording clears them.
    // Strings always, empty included: a null arg means "use the backend's
    // mirror" and is reserved for the handshake fallback.
    appState.setPendingTitleHint(meetingTitle);
    try {
      await invoke('stop_recording', {
        userNotes,
        meetingTitle
      });
    } catch (e) {
      appState.setError(errorMessage(e));
    }
  }
</script>

<div class="flex h-12 shrink-0 items-center gap-3 border-b border-border px-4">
  <!-- The sidebar's record button already carries the recording status;
       the header leads with the star action and the timer. -->
  <div class="flex shrink-0 items-center gap-1.5">
    <Tip text={t.star}>
      {#snippet children({ props })}
        <button
          {...props}
          onclick={() => onStar?.()}
          class="rounded-md p-2 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
          aria-label={t.starAria}
        >
          <Star size={16} />
        </button>
      {/snippet}
    </Tip>
    <!-- Shadow mode ([shell.md] §Recording): the timer is the loudest
         "this is being recorded" on the page after the meter. Starring
         stays: it is an action, not a tell. -->
    {#if !appState.shadowMode}
      <span class="text-sm tabular-nums">{formatTime(elapsed)}</span>
    {/if}
  </div>

  <!-- Borderless: editing the title feels like editing text, not a field. -->
  <input
    bind:value={meetingTitle}
    class="font-display h-8 min-w-0 flex-1 bg-transparent px-2 text-base outline-none
      placeholder:text-muted-foreground/70"
    placeholder={t.titlePlaceholder}
    aria-label={t.titleAria}
  />

  <SourcePicker />
  {#if !appState.shadowMode}
    <LevelRibbon />
  {/if}

  <div class="flex shrink-0 items-center gap-1.5">
    <!-- Shadow mode ([shell.md] §Recording): its own control, deliberately
         not tied to the transcript being shut. Collapsing a pane to get
         room is not the same request as asking the screen to stop
         announcing the recording. Stays visible while active; it is the
         way back. The name holds still across the toggle; `aria-pressed`
         and the icon carry the state. While shadow is on, the hover tip
         also names where Stop went: hover never shows on the shared
         screen, and the moment someone wonders is the moment they hover
         the one control left. -->
    <Tip
      text={appState.shadowMode
        ? t.shadowStopHint(copy.shell.titleBar.commandBar.shortcut)
        : t.shadowMode}
    >
      {#snippet children({ props })}
        <button
          {...props}
          onclick={() => appState.setShadowMode(!appState.shadowMode)}
          class="rounded-md p-2 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
          aria-pressed={appState.shadowMode}
          aria-label={t.shadowMode}
        >
          {#if appState.shadowMode}
            <EyeOff size={16} />
          {:else}
            <Eye size={16} />
          {/if}
        </button>
      {/snippet}
    </Tip>
    <!-- Pause and the red stop square are tells after all: together they
         read as recording controls on a shared screen, which is the one
         thing shadow mode exists to prevent. The command palette keeps
         "Stop recording" as the way out ([shell.md] §Recording). -->
    {#if !appState.shadowMode}
      <Tip text={appState.isPaused ? t.resume : t.pause}>
        {#snippet children({ props })}
          <button
            {...props}
            onclick={togglePause}
            class="rounded-md p-2 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
            aria-label={appState.isPaused ? t.resumeAria : t.pauseAria}
          >
            {#if appState.isPaused}
              <Play size={16} />
            {:else}
              <Pause size={16} />
            {/if}
          </button>
        {/snippet}
      </Tip>
      <Tip text={t.stop}>
        {#snippet children({ props })}
          <button
            {...props}
            onclick={stop}
            class="rounded-md p-2 text-destructive transition-colors hover:bg-destructive/10"
            aria-label={t.stop}
          >
            <Square size={16} fill="currentColor" />
          </button>
        {/snippet}
      </Tip>
    {/if}
  </div>
</div>
