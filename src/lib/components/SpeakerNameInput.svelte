<script lang="ts">
  /** Inline speaker-name editor with an app-themed suggestion popover;
   * replaces the native datalist (which renders as the browser's autofill
   * dropdown). Suggestions come from the profile registry plus the
   * meeting's other labels (for merges); arrows + Enter pick, Escape
   * cancels, blur commits.
   *
   * Borderless and transparent, like the meeting title: a speaker's name
   * is a piece of text you edit in place, not a form field you click
   * into. Callers pass the same classes the name carries when it is not
   * being edited (`nameClass` and its size), so the text keeps its
   * identity (colour, weight, position) while the caret is in it. */

  import { copy } from '$lib/copy';

  const t = $derived(copy.speakers.nameInput);

  let {
    value = $bindable(''),
    suggestions = [],
    class: className = '',
    onCommit,
    onCancel
  }: {
    value?: string;
    suggestions?: string[];
    /** The text styling this name has when it is not being edited. */
    class?: string;
    onCommit: () => void;
    onCancel: () => void;
  } = $props();

  let inputEl: HTMLInputElement | undefined = $state();
  let highlighted = $state(-1);

  const matches = $derived.by(() => {
    const q = value.trim().toLowerCase();
    const seen = new Set<string>();
    const out: string[] = [];
    for (const name of suggestions) {
      const lower = name.toLowerCase();
      if (seen.has(lower) || lower === q) continue;
      if (!q || lower.includes(q)) {
        seen.add(lower);
        out.push(name);
      }
      if (out.length >= 6) break;
    }
    return out;
  });

  $effect(() => {
    inputEl?.focus();
    inputEl?.select();
  });

  function pick(name: string) {
    value = name;
    onCommit();
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      highlighted = Math.min(highlighted + 1, matches.length - 1);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      highlighted = Math.max(highlighted - 1, -1);
    } else if (e.key === 'Enter') {
      e.preventDefault();
      if (highlighted >= 0 && matches[highlighted]) {
        pick(matches[highlighted]);
      } else {
        onCommit();
      }
    } else if (e.key === 'Escape') {
      onCancel();
    } else {
      highlighted = -1;
    }
  }
</script>

<!-- The name's classes sit on the wrapper, not only on the mirror: a
     box's line height can never be shorter than its own font's, and this
     wrapper otherwise inherits the row's larger font, which made the
     editing box taller than the idle name and shifted the row on click. -->
<span class="relative inline-block {className}">
  <!-- The invisible mirror is what sizes the field: same font (inherited
       from the wrapper), same text, so the box is exactly as wide as its
       content and the name's glyphs hold still when the caret arrives.
       The input overlays it absolutely; in flow it would contribute its
       own intrinsic width and stretch the box. (`size`-based widths are
       character approximations and shifted the text; CSS field-sizing is
       absent from WKWebView.) -->
  <span aria-hidden="true" class="invisible whitespace-pre">{value || ' '}</span>
  <input
    bind:this={inputEl}
    bind:value
    class="absolute inset-0 bg-transparent p-0 outline-none {className}"
    onkeydown={onKeydown}
    onblur={onCommit}
    aria-label={t.aria}
  />
  {#if matches.length > 0}
    <div
      class="absolute top-full left-0 z-50 mt-1 min-w-36 overflow-hidden rounded-md border border-border bg-popover py-1 shadow-md"
    >
      {#each matches as name, i (name)}
        <!-- pointerdown beats the input's blur, so picking works. -->
        <button
          class="block w-full truncate px-2.5 py-1 text-left text-xs transition-colors {i ===
          highlighted
            ? 'bg-accent text-accent-foreground'
            : 'text-popover-foreground hover:bg-accent hover:text-accent-foreground'}"
          onpointerdown={(e) => {
            e.preventDefault();
            pick(name);
          }}
        >
          {name}
        </button>
      {/each}
    </div>
  {/if}
</span>
