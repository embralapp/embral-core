<script lang="ts">
  import { appState } from '$lib/stores/app-state.svelte';
  import { openNotesFolder } from '$lib/utils/openNotesFolder';
  import { copy } from '$lib/copy';

  const t = $derived(copy.meetings.processing);

  // Imports only: a stopped recording goes straight back to the Meetings
  // page as a pending entry instead of a processing screen. The step ids are
  // matched against appState.processingStep (data); labels come from the
  // catalog by key.
  const steps = [
    { id: 'transcribing-import', key: 'transcribing' as const },
    { id: 'finalizing-transcript', key: 'finalizing' as const },
    { id: 'generating-notes', key: 'generating' as const }
  ];

  function getStatus(stepId: string): 'complete' | 'active' | 'pending' {
    const current = appState.processingStep;
    if (!current) return 'pending';
    const order = steps.map((s) => s.id);
    const currentIdx = order.indexOf(current);
    const stepIdx = order.indexOf(stepId);
    if (stepIdx < currentIdx) return 'complete';
    if (stepIdx === currentIdx) return 'active';
    return 'pending';
  }

  function continueInBackground() {
    appState.resetToIdle();
  }
</script>

<div class="flex min-h-0 flex-1 flex-col">
  <div class="flex h-12 shrink-0 items-center gap-3 border-b border-border px-4">
    <span class="text-sm text-muted-foreground">{t.importing}</span>
  </div>

  <div class="flex flex-1 flex-col items-center justify-center gap-6 px-8">
    <div class="w-full max-w-xs space-y-3">
      {#each steps as step (step.id)}
        {@const status = getStatus(step.id)}
        <div class="flex items-center gap-3">
          {#if status === 'complete'}
            <div
              class="flex h-5 w-5 shrink-0 items-center justify-center rounded-full bg-green-500/20"
            >
              <svg
                class="h-3 w-3 text-green-500"
                fill="none"
                viewBox="0 0 24 24"
                stroke="currentColor"
              >
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2.5" d="M5 13l4 4L19 7" />
              </svg>
            </div>
          {:else if status === 'active'}
            <div class="flex h-5 w-5 shrink-0 items-center justify-center">
              <div
                class="h-4 w-4 animate-spin rounded-full border-2 border-primary border-t-transparent"
              ></div>
            </div>
          {:else}
            <div class="h-5 w-5 shrink-0 rounded-full border-2 border-border"></div>
          {/if}
          <span
            class="text-sm {status === 'active'
              ? 'font-medium text-foreground'
              : 'text-muted-foreground'}"
          >
            {t.steps[step.key]}
            {#if step.id === 'transcribing-import' && status === 'active' && appState.importFraction != null}
              <span class="tabular-nums text-muted-foreground">
                {t.percent(Math.round(appState.importFraction * 100))}
              </span>
            {/if}
          </span>
        </div>
      {/each}
    </div>

    {#if appState.error}
      <p class="text-center text-xs text-destructive">{appState.error}</p>
    {/if}

    <div class="flex w-full max-w-xs flex-col gap-2">
      <button
        onclick={continueInBackground}
        class="w-full rounded-md bg-primary px-4 py-2 text-sm font-medium
          text-primary-foreground transition-colors hover:bg-primary/90"
      >
        {appState.error ? t.backToMeetings : t.continueBackground}
      </button>
      {#if appState.error}
        <button
          onclick={openNotesFolder}
          class="w-full rounded-md border border-border px-4 py-2 text-sm
            font-medium transition-colors hover:bg-accent"
        >
          {t.openNotesFolder}
        </button>
      {/if}
    </div>
  </div>
</div>
