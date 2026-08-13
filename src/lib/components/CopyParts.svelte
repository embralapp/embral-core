<script lang="ts">
  // Renders a sentence that is interrupted by an inline element: a link, a
  // code span, a differently-weighted run (docs/copy.md). The catalog holds
  // the sentence as an ordered `Part[]`; plain strings render in the parent's
  // own style, and each `{ slot, text }` is handed to the caller's `part`
  // snippet, which renders the element for that slot name. A translator can
  // reorder the array, so the interrupted element can move within the
  // sentence, which fixed before/after fragments could never allow.
  import type { Snippet } from 'svelte';
  import type { Part } from '$lib/copy/types';

  let {
    parts,
    part
  }: {
    parts: readonly Part[];
    /** Renders one slotted part: (slotName, text). */
    part: Snippet<[string, string]>;
  } = $props();
</script>

{#each parts as p, i (i)}{#if typeof p === 'string'}{p}{:else}{@render part(p.slot, p.text)}{/if}{/each}
