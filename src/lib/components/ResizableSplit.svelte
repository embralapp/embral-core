<script lang="ts">
  import { onMount, type Snippet } from 'svelte';
  import { copy } from '$lib/copy';

  const t = $derived(copy.common);

  /** Two panes around a draggable divider. One side keeps a fixed pixel
   * width (persisted per machine in localStorage); the other flexes. Pane
   * classes are overridable so callers can add responsive behavior (the
   * meetings page collapses to a single panel on narrow windows).
   *
   * With `collapsible`, dragging the fixed pane well under its minimum
   * shuts it instead of stopping at the floor (animated, so it reads as
   * the pane closing rather than the drag breaking). The divider is then
   * the way back: click it, or drag it out again. */
  let {
    left,
    right,
    fixedSide,
    storageKey,
    defaultSize = 340,
    minFixed = 260,
    minFlex = 320,
    collapsible = false,
    collapsed = $bindable(false),
    forceCollapsed = false,
    onForceReopen,
    fixedClass = 'flex w-(--split-size) shrink-0',
    flexClass = 'flex min-w-0 flex-1',
    dividerClass = ''
  }: {
    left: Snippet;
    right: Snippet;
    fixedSide: 'left' | 'right';
    storageKey: string;
    defaultSize?: number;
    minFixed?: number;
    minFlex?: number;
    /** Allow the fixed pane to shut entirely. */
    collapsible?: boolean;
    /** Two-way, for a caller that needs to know the pane shut. */
    collapsed?: boolean;
    /** Display-only shut: the pane renders closed without touching the
     * persisted `collapsed` state, so the caller can borrow the collapse
     * (shadow mode does) and give the layout back untouched. */
    forceCollapsed?: boolean;
    /** The divider defeated `forceCollapsed`; the caller drops the force
     * so the reopen sticks. */
    onForceReopen?: () => void;
    fixedClass?: string;
    flexClass?: string;
    dividerClass?: string;
  } = $props();

  /** How far past the minimum the drag must go before the pane shuts.
   * Wide enough that hitting the floor doesn't slam it closed by accident. */
  const COLLAPSE_SLACK = 80;

  // Snapshot on mount is intentional: defaultSize is a static per-usage
  // constant, and the saved width overrides it right away anyway.
  // svelte-ignore state_referenced_locally
  let size = $state(defaultSize);
  let containerEl = $state<HTMLElement | null>(null);
  let dragging = $state(false);
  /** Whether this drag ever moved: a press that didn't is a click, which
   * is how a shut pane is reopened. */
  let moved = false;

  const collapsedKey = $derived(`${storageKey}:collapsed`);

  /** What actually renders shut: the user's own collapse or a caller's
   * display-only force. Only `collapsed` ever persists. */
  const shut = $derived(collapsed || forceCollapsed);

  /** Set once the saved state has been read, so the persisting effect
   * below doesn't write the default back over it on mount. */
  let restored = $state(false);

  onMount(() => {
    const saved = Number(localStorage.getItem(storageKey));
    if (Number.isFinite(saved) && saved >= minFixed) {
      size = saved;
    }
    if (collapsible && localStorage.getItem(collapsedKey) === '1') {
      collapsed = true;
    }
    restored = true;
  });

  // Persist wherever the change came from: the divider, or a caller
  // flipping the bound value.
  $effect(() => {
    if (!collapsible || !restored) return;
    localStorage.setItem(collapsedKey, collapsed ? '1' : '0');
  });

  function setCollapsed(next: boolean) {
    collapsed = next;
  }

  function onPointerDown(e: PointerEvent) {
    dragging = true;
    moved = false;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }

  function onPointerMove(e: PointerEvent) {
    if (!dragging || !containerEl) return;
    moved = true;
    const rect = containerEl.getBoundingClientRect();
    const max = Math.max(minFixed, rect.width - minFlex);
    const raw =
      fixedSide === 'right' ? rect.right - e.clientX : e.clientX - rect.left;
    if (collapsible && raw < minFixed - COLLAPSE_SLACK) {
      if (!collapsed) setCollapsed(true);
      return;
    }
    if (forceCollapsed) onForceReopen?.();
    if (collapsed) setCollapsed(false);
    size = Math.min(max, Math.max(minFixed, raw));
  }

  function onPointerUp(e: PointerEvent) {
    if (!dragging) return;
    dragging = false;
    (e.currentTarget as HTMLElement).releasePointerCapture(e.pointerId);
    // A press with no drag on a shut pane opens it: the divider is the
    // only affordance left once the pane has no width.
    if (collapsible && shut && !moved) {
      if (forceCollapsed) onForceReopen?.();
      setCollapsed(false);
      return;
    }
    localStorage.setItem(storageKey, String(size));
  }
</script>

<div
  bind:this={containerEl}
  class="flex min-h-0 min-w-0 flex-1"
  style="--split-size: {shut ? 0 : size}px"
>
  <div
    class="min-h-0 flex-col {fixedSide === 'left'
      ? fixedClass
      : flexClass} {fixedSide === 'left' && shut
      ? 'overflow-hidden'
      : ''} {fixedSide === 'left' && !dragging
      ? 'transition-[width] duration-200 ease-out'
      : ''}"
    inert={fixedSide === 'left' && shut}
  >
    {@render left()}
  </div>
  <div
    role="separator"
    aria-orientation="vertical"
    aria-label={shut ? t.reopenPane : t.resizePanels}
    class="shrink-0 cursor-col-resize transition-colors {shut
      ? 'w-[5px] bg-border hover:bg-muted-foreground/60'
      : 'w-[3px]'} {dragging
      ? 'bg-muted-foreground/50'
      : 'bg-border hover:bg-muted-foreground/40'} {dividerClass}"
    onpointerdown={onPointerDown}
    onpointermove={onPointerMove}
    onpointerup={onPointerUp}
  ></div>
  <div
    class="min-h-0 flex-col {fixedSide === 'right'
      ? fixedClass
      : flexClass} {fixedSide === 'right' && shut
      ? 'overflow-hidden'
      : ''} {fixedSide === 'right' && !dragging
      ? 'transition-[width] duration-200 ease-out'
      : ''}"
    inert={fixedSide === 'right' && shut}
  >
    {@render right()}
  </div>
</div>
