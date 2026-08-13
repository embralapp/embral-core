<script lang="ts">
  import type { Snippet } from 'svelte';
  import * as Tooltip from '$lib/components/ui/tooltip';

  /**
   * A themed hover label: the app's replacement for `title=""`.
   *
   * The native tooltip is an OS control: a hard-outlined box in Windows' own
   * colours that ignores the app's theme entirely. This renders the vendored
   * tooltip instead, which the whole app now has a provider for
   * (`+layout.svelte`).
   *
   * The trigger is your element, not a wrapper: the child snippet hands you
   * the trigger props to spread, so the button keeps its own classes, layout and
   * handlers and nothing new is added to the DOM. Spread `props` first, so your
   * own handlers win any collision.
   *
   * `aria-label` stays on the element. A tooltip is not an accessible name; a
   * screen reader must not depend on hover.
   */
  let {
    text,
    side = 'top',
    sideOffset = 6,
    children
  }: {
    text: string;
    side?: 'top' | 'right' | 'bottom' | 'left';
    sideOffset?: number;
    children: Snippet<[{ props: Record<string, unknown> }]>;
  } = $props();
</script>

<Tooltip.Root>
  <Tooltip.Trigger>
    {#snippet child({ props })}
      {@render children({ props })}
    {/snippet}
  </Tooltip.Trigger>
  <Tooltip.Content {side} {sideOffset}>{text}</Tooltip.Content>
</Tooltip.Root>
