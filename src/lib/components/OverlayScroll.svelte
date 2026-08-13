<script lang="ts">
  import type { Snippet } from 'svelte';

  /**
   * A scroll container whose scrollbar takes no layout space: the native bar is
   * hidden and a thumb floats over the content instead.
   *
   * The lists need this. A native scrollbar (and the global
   * `scrollbar-gutter: stable`, which the settings pages depend on) reserves a
   * strip the rows cannot paint into, so a selected row's fill stopped short of
   * the pane edge and read as a gap wherever the thumb wasn't beside it. Here
   * the rows run edge to edge and the thumb sits on top of them.
   *
   * The thumb shows while scrolling and while the pointer is over the pane, and
   * doesn't exist at all when there is nothing to scroll.
   */
  let {
    children,
    class: className = ''
  }: { children: Snippet; class?: string } = $props();

  let viewport = $state<HTMLDivElement | null>(null);
  let scrollTop = $state(0);
  let scrollHeight = $state(0);
  let clientHeight = $state(0);
  let scrolling = $state(false);
  let dragging = $state(false);
  let scrollTimer: ReturnType<typeof setTimeout> | undefined;

  const MIN_THUMB = 28;

  const scrollable = $derived(scrollHeight > clientHeight + 1);
  const thumbHeight = $derived(
    Math.max(MIN_THUMB, (clientHeight / Math.max(scrollHeight, 1)) * clientHeight)
  );
  // The thumb travels the track, not the content: its top is a fraction of the
  // room left over once the thumb itself is accounted for.
  const thumbTop = $derived.by(() => {
    const distance = scrollHeight - clientHeight;
    if (distance <= 0) return 0;
    return (scrollTop / distance) * (clientHeight - thumbHeight);
  });
  // Hover is CSS (`group-hover`), not state: a pointerenter handler on a plain
  // div is a static-element interaction, and the thumb is not interactive chrome
  // a screen reader should hear about.
  const active = $derived(scrolling || dragging);

  function measure() {
    if (!viewport) return;
    scrollTop = viewport.scrollTop;
    scrollHeight = viewport.scrollHeight;
    clientHeight = viewport.clientHeight;
  }

  function onScroll() {
    measure();
    scrolling = true;
    clearTimeout(scrollTimer);
    scrollTimer = setTimeout(() => (scrolling = false), 700);
  }

  // Content grows and shrinks under us (a meeting finishes, a filter changes),
  // so the thumb is sized from an observer rather than from a mount-time read.
  $effect(() => {
    if (!viewport) return;
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(viewport);
    for (const child of Array.from(viewport.children)) observer.observe(child);
    return () => {
      observer.disconnect();
      clearTimeout(scrollTimer);
    };
  });

  function onThumbPointerDown(e: PointerEvent) {
    if (!viewport) return;
    dragging = true;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    const startY = e.clientY;
    const startScroll = viewport.scrollTop;
    const track = clientHeight - thumbHeight;
    const distance = scrollHeight - clientHeight;

    const onMove = (move: PointerEvent) => {
      if (!viewport || track <= 0) return;
      const delta = ((move.clientY - startY) / track) * distance;
      viewport.scrollTop = startScroll + delta;
    };
    const onUp = () => {
      dragging = false;
      window.removeEventListener('pointermove', onMove);
      window.removeEventListener('pointerup', onUp);
    };
    window.addEventListener('pointermove', onMove);
    window.addEventListener('pointerup', onUp);
  }
</script>

<div class="group relative min-h-0 flex-1 {className}">
  <div bind:this={viewport} onscroll={onScroll} class="no-native-scrollbar h-full overflow-y-auto">
    {@render children()}
  </div>

  {#if scrollable}
    <div
      class="absolute top-0 right-0.5 w-1.5 rounded-full bg-muted-foreground/35 transition-opacity duration-150 hover:bg-muted-foreground/55 {active
        ? 'opacity-100'
        : 'opacity-0 group-hover:opacity-100'}"
      style="height: {thumbHeight}px; transform: translateY({thumbTop}px)"
      onpointerdown={onThumbPointerDown}
      aria-hidden="true"
    ></div>
  {/if}
</div>

<style>
  /* No gutter, no track: the scrollbar must not take a strip the rows cannot
     paint into. (Scoped here rather than in layout.css; the settings pages
     still want their stable gutter.) */
  .no-native-scrollbar {
    scrollbar-width: none;
  }
  .no-native-scrollbar::-webkit-scrollbar {
    display: none;
  }
</style>
