import { beforeEach, describe, expect, it } from 'vitest';
import { ListSelection } from './listSelection.svelte';

/**
 * The selection model behind both lists (meetings and profiles). It is the
 * branchiest logic on the frontend and the easiest to get subtly wrong: a
 * shift-range that creeps, a primary that points at a deleted row.
 *
 * (This file is also why the vitest config carries the SvelteKit plugin:
 * `listSelection.svelte.ts` is a runes module, and `$state` only exists after
 * the Svelte compiler has seen it.)
 */
const ORDER = ['a', 'b', 'c', 'd', 'e'];
const plain = { shiftKey: false, ctrlKey: false, metaKey: false };
const ctrl = { shiftKey: false, ctrlKey: true, metaKey: false };
const shift = { shiftKey: true, ctrlKey: false, metaKey: false };

describe('ListSelection', () => {
  let sel: ListSelection;
  beforeEach(() => {
    sel = new ListSelection();
  });

  it('starts empty', () => {
    expect(sel.count).toBe(0);
    expect(sel.primary).toBeNull();
  });

  it('replaces the selection on a plain click', () => {
    sel.click('a', plain, ORDER);
    sel.click('c', plain, ORDER);
    expect(sel.ids).toEqual(['c']);
    expect(sel.primary).toBe('c');
  });

  it('adds and removes with ctrl', () => {
    sel.click('a', plain, ORDER);
    sel.click('c', ctrl, ORDER);
    expect(sel.ids).toEqual(['a', 'c']);
    expect(sel.primary).toBe('c');

    sel.click('c', ctrl, ORDER);
    expect(sel.ids).toEqual(['a']);
    // The primary followed the row that left; it must land on something that is
    // still selected, or the detail pane renders a row nobody picked.
    expect(sel.primary).toBe('a');
  });

  it('leaves no primary when ctrl removes the last row', () => {
    sel.click('a', plain, ORDER);
    sel.click('a', ctrl, ORDER);
    expect(sel.count).toBe(0);
    expect(sel.primary).toBeNull();
  });

  it('takes a range with shift, in either direction', () => {
    sel.click('b', plain, ORDER);
    sel.click('d', shift, ORDER);
    expect(sel.ids).toEqual(['b', 'c', 'd']);
    // The primary is the row actually clicked, not the end of the array: a
    // range dragged upwards ends on its first id.
    expect(sel.primary).toBe('d');

    sel.click('d', plain, ORDER);
    sel.click('b', shift, ORDER);
    expect(sel.ids).toEqual(['b', 'c', 'd']);
    expect(sel.primary).toBe('b');
  });

  it('keeps the shift anchor where it was', () => {
    // Re-ranging must measure from the original anchor, not creep along behind
    // the pointer; otherwise a second shift-click grows the selection instead
    // of redrawing it.
    sel.click('b', plain, ORDER);
    sel.click('d', shift, ORDER);
    sel.click('c', shift, ORDER);
    expect(sel.ids).toEqual(['b', 'c']);

    sel.click('e', shift, ORDER);
    expect(sel.ids).toEqual(['b', 'c', 'd', 'e']);
  });

  it('moves the anchor on a ctrl click', () => {
    sel.click('a', plain, ORDER);
    sel.click('d', ctrl, ORDER);
    sel.click('e', shift, ORDER);
    // The range runs from the ctrl-clicked row, which is where the user last
    // put their finger.
    expect(sel.ids).toEqual(['d', 'e']);
  });

  it('selects a single row when shift is pressed with no anchor', () => {
    sel.click('c', shift, ORDER);
    expect(sel.ids).toEqual(['c']);
  });

  it('drops rows that no longer exist, and repairs the primary', () => {
    sel.click('b', plain, ORDER);
    sel.click('d', shift, ORDER); // b, c, d — primary d
    sel.retain(['a', 'b', 'c']); // d was deleted
    expect(sel.ids).toEqual(['b', 'c']);
    expect(sel.primary).toBe('c');
  });

  it('leaves no primary when everything selected is gone', () => {
    sel.click('b', plain, ORDER);
    sel.retain(['a', 'c']);
    expect(sel.count).toBe(0);
    expect(sel.primary).toBeNull();
  });

  it('repairs the anchor when it is deleted, so a later shift-click is sane', () => {
    sel.click('b', plain, ORDER); // anchor b
    sel.click('c', ctrl, ORDER); // anchor c, ids b,c
    sel.retain(['a', 'b', 'e']); // c is gone — anchor and primary must recover
    expect(sel.ids).toEqual(['b']);
    expect(sel.primary).toBe('b');

    // The anchor fell back to the surviving primary, so this ranges b→e rather
    // than measuring from a row that no longer exists.
    sel.click('e', shift, ['a', 'b', 'e']);
    expect(sel.ids).toEqual(['b', 'e']);
  });
});
