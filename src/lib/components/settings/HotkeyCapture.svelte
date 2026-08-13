<script lang="ts">
  import { Button } from "$lib/components/ui/button";
  import { comboFromEvent, formatCombo } from "$lib/utils/hotkey";
  import { copy } from "$lib/copy";

  const t = $derived(copy.common.hotkey);

  /**
   * Press-the-combo hotkey capture: click the chip, press a combo, done. Esc
   * cancels; Clear unsets.
   *
   * This existed twice, byte for byte, in the Meetings and Dictation settings
   * sections (same comment, same body, different config field), and the
   * Dictation page needed a third. It takes `onChange` rather than `bind:` so
   * the settings pages can keep writing straight into their shared debounced
   * draft while the Dictation page writes through `configStore.save`.
   */
  let {
    value,
    onChange,
    ariaLabel = t.defaultAria,
  }: {
    value: string;
    onChange: (combo: string) => void;
    ariaLabel?: string;
  } = $props();

  let capturing = $state(false);

  function capture(e: KeyboardEvent) {
    if (!capturing) return;
    e.preventDefault();
    e.stopPropagation();
    if (e.code === "Escape") {
      capturing = false;
      return;
    }
    // Null while only modifiers are down, and for keys we can't name; either
    // way, keep listening rather than saving a combo that won't register.
    const combo = comboFromEvent(e);
    if (!combo) return;

    onChange(combo);
    capturing = false;
  }
</script>

<div class="flex items-center gap-2">
  <button
    class="min-w-28 rounded-md border border-input px-3 py-1.5 font-mono text-xs transition-colors hover:bg-accent {capturing
      ? 'ring-1 ring-ring'
      : ''}"
    aria-label={ariaLabel}
    onclick={() => (capturing = true)}
    onkeydown={capture}
  >
    {capturing ? t.pressCombo : formatCombo(value) || t.notSet}
  </button>
  {#if value}
    <Button variant="ghost" size="sm" onclick={() => onChange("")}>{t.clear}</Button>
  {/if}
</div>
