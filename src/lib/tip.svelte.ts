// The shared hover label for hot-path lists: one floating element for the
// whole app instead of a mounted tooltip tree per row (`Tip.svelte` stays
// the tool for one-off chrome; this action exists because a thousand
// transcript rows cannot afford a thousand tooltip components).
//
// Same contract as Tip: the label is presentation only. `aria-label`
// stays on the element, and a disabled button emits no pointer events so
// it never opens.

type ActiveTip = { text: string; x: number; y: number };

/** Matches the app-wide Tooltip.Provider delay (+layout.svelte). */
const DELAY_MS = 400;

let current = $state<ActiveTip | null>(null);
let timer: ReturnType<typeof setTimeout> | null = null;

export const hoverTip = {
  get current() {
    return current;
  }
};

export function hideTip() {
  if (timer) {
    clearTimeout(timer);
    timer = null;
  }
  current = null;
}

/** `use:tip={label}` shows the shared label centered above the element. */
export function tip(node: HTMLElement, text: string) {
  let label = text;

  const show = () => {
    const r = node.getBoundingClientRect();
    current = { text: label, x: r.left + r.width / 2, y: r.top };
  };
  const schedule = () => {
    hideTip();
    timer = setTimeout(show, DELAY_MS);
  };

  node.addEventListener('pointerenter', schedule);
  node.addEventListener('pointerleave', hideTip);
  node.addEventListener('pointerdown', hideTip);
  node.addEventListener('focusin', schedule);
  node.addEventListener('focusout', hideTip);

  return {
    update(next: string) {
      label = next;
    },
    destroy() {
      hideTip();
      node.removeEventListener('pointerenter', schedule);
      node.removeEventListener('pointerleave', hideTip);
      node.removeEventListener('pointerdown', hideTip);
      node.removeEventListener('focusin', schedule);
      node.removeEventListener('focusout', hideTip);
    }
  };
}
