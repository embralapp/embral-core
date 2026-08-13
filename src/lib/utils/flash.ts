/**
 * Briefly mark the thing a search result landed on.
 *
 * Scrolling a paragraph into the middle of a long document is only half an
 * answer: the user still has to work out which paragraph they were sent
 * to. A short highlight says it without adding any chrome that has to be
 * dismissed: it fades on its own and leaves the document alone.
 */

export const FLASH_CLASS = 'embral-flash';

/** Long enough to catch the eye after a scroll, short enough not to linger
 * as a thing the user wonders how to clear. Matches the fade in layout.css. */
export const FLASH_MS = 1400;

/** Per-element timer, so re-landing on the same paragraph restarts the
 * highlight instead of having the first run's timeout cut the second short. */
const running = new WeakMap<Element, ReturnType<typeof setTimeout>>();

/**
 * Run `then` once a smooth scroll to `target` has arrived.
 *
 * A highlight that starts with the scroll is half over by the time the
 * reader's eye gets there, and worse the further it travels, which is
 * exactly when the mark is most needed. So the mark waits for the journey
 * to end.
 *
 * Whether a journey is happening is decided from where the target is,
 * not by watching `scrollTop`: a smooth scroll has not moved a pixel by the
 * next frame, so sampling the position just says "no scroll" and marks
 * immediately, which is the bug this exists to fix.
 *
 * Call it before starting the scroll, so the measurement is of the
 * position being left rather than the one being sought.
 */
export function afterScrollTo(scroller: Element, target: Element, then: () => void): void {
  const box = scroller.getBoundingClientRect();
  const row = target.getBoundingClientRect();
  if (row.top >= box.top && row.bottom <= box.bottom) {
    // Already on screen: nothing to wait for, and waiting would only make
    // the mark look late.
    then();
    return;
  }
  let done = false;
  const finish = () => {
    if (done) return;
    done = true;
    clearTimeout(timer);
    scroller.removeEventListener('scrollend', finish);
    then();
  };
  // `scrollend` is the precise signal; the timer is the backstop for a
  // scroll that gets interrupted, or a platform that never fires it.
  const timer = setTimeout(finish, 1200);
  scroller.addEventListener('scrollend', finish);
}

export function flash(el: Element | null | undefined, durationMs = FLASH_MS): void {
  if (!el) return;
  const previous = running.get(el);
  if (previous) {
    clearTimeout(previous);
    el.classList.remove(FLASH_CLASS);
    // Force a reflow so re-adding the class restarts the animation rather
    // than continuing the one already in flight.
    void (el as HTMLElement).offsetWidth;
  }
  el.classList.add(FLASH_CLASS);
  running.set(
    el,
    setTimeout(() => {
      el.classList.remove(FLASH_CLASS);
      running.delete(el);
    }, durationMs)
  );
}
