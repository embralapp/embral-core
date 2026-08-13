/**
 * Selection for a flat list of rows: plain click, Ctrl-click to add or remove,
 * Shift-click to take a range. Shared by the meetings and profiles lists, which
 * are the same object with different contents.
 *
 * `primary` is the row the detail pane shows: the last one the user actually
 * clicked, which is not always the last id in the list (a Shift-range downwards
 * ends on its first id).
 */
export class ListSelection {
  ids = $state<string[]>([]);
  primary = $state<string | null>(null);
  /** Where a Shift-range measures from: the last plain or Ctrl click. */
  #anchor: string | null = null;

  get count(): number {
    return this.ids.length;
  }

  has(id: string): boolean {
    return this.ids.includes(id);
  }

  /** Select exactly this row (what a plain click and a programmatic open do). */
  select(id: string): void {
    this.ids = [id];
    this.primary = id;
    this.#anchor = id;
  }

  clear(): void {
    this.ids = [];
    this.primary = null;
    this.#anchor = null;
  }

  /**
   * Apply a click. `order` is the ids in the order they appear on screen (the
   * flattened rows, headers ignored), so a Shift-range crosses date groups the
   * way the eye expects.
   */
  click(id: string, event: { shiftKey: boolean; ctrlKey: boolean; metaKey: boolean }, order: string[]): void {
    if (event.shiftKey && this.#anchor !== null && order.includes(this.#anchor)) {
      const from = order.indexOf(this.#anchor);
      const to = order.indexOf(id);
      if (to !== -1) {
        const [lo, hi] = from <= to ? [from, to] : [to, from];
        this.ids = order.slice(lo, hi + 1);
        this.primary = id;
        // The anchor stays put: shift-clicking again re-ranges from the same
        // origin rather than creeping along behind the pointer.
        return;
      }
    }

    if (event.ctrlKey || event.metaKey) {
      const removing = this.has(id);
      this.ids = removing ? this.ids.filter((x) => x !== id) : [...this.ids, id];
      this.primary = removing ? (this.ids.at(-1) ?? null) : id;
      this.#anchor = id;
      return;
    }

    this.select(id);
  }

  /** Drop ids that no longer exist: after a delete, or a refresh that lost rows. */
  retain(existing: string[]): void {
    this.ids = this.ids.filter((id) => existing.includes(id));
    if (this.primary !== null && !existing.includes(this.primary)) {
      this.primary = this.ids.at(-1) ?? null;
    }
    if (this.#anchor !== null && !existing.includes(this.#anchor)) {
      this.#anchor = this.primary;
    }
  }
}
