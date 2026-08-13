// Walks the whole catalog and asserts the invariants that catch the realistic
// mechanical errors (docs/copy.md). It is not a baseline-equality test: a
// committed snapshot of every string would just be a second copy to maintain;
// these invariants hold permanently instead.
//
// The English catalog is the schema, so `copy` typing already forbids missing
// keys and call-site typos. This adds the value-shape checks the type can't:
// stray whitespace from pasting out of indented markup, a template that should
// have been a function, and a function that returns `undefined`/`NaN`.

import { describe, expect, it } from 'vitest';
import { copy } from './index';
import { cloudCopy } from '$lib/cloud/copy';

// A string sample: catalog functions only interpolate their args (never do
// arithmetic), and some pass an arg straight through as a Part's text, so a
// string keeps that text a string, while numeric interpolation coerces fine.
const SAMPLE = 'x';

type Leaf = { path: string; value: unknown };

function leaves(node: unknown, path: string, acc: Leaf[]): void {
  // Arrays and functions are catalog leaves in their own right (a static or
  // computed Part[], or an interpolating function); checkLeaf handles both.
  if (node === null || typeof node !== 'object' || Array.isArray(node)) {
    acc.push({ path, value: node });
    return;
  }
  for (const [k, v] of Object.entries(node)) leaves(v, `${path}.${k}`, acc);
}

/** Validate a Part[]: each piece a plain string or { slot, text }; fragments
 * are glued, so only the well-formed check applies. */
function checkParts(path: string, parts: unknown[]): void {
  for (const part of parts) {
    if (typeof part === 'string') checkWellFormed(`${path} fragment`, part);
    else {
      const p = part as { slot: string; text: string };
      expect(p.slot, `${path} part missing slot`).toBeTruthy();
      checkWellFormed(`${path} part`, p.text);
    }
  }
}

/** A rendered string carries no stray template braces and no failed
 * interpolation. */
function checkWellFormed(path: string, s: string): void {
  expect(s, `${path} contains a stray brace (a template that should be a function?)`).not.toMatch(
    /[{}]/
  );
  expect(s, `${path} interpolated a bad value`).not.toMatch(/undefined|NaN|\[object Object\]/);
}

/** A standalone leaf is also non-empty and free of edge whitespace (the #1
 * paste-from-markup error). Part fragments are glued together, so their
 * spacing is meaningful and this stricter check does not apply to them. */
function checkString(path: string, s: string): void {
  expect(s, `${path} is empty`).not.toBe('');
  expect(s, `${path} has edge whitespace`).toBe(s.trim());
  checkWellFormed(path, s);
}

function checkLeaf({ path, value }: Leaf): void {
  if (typeof value === 'function') {
    const args = Array.from({ length: value.length }, () => SAMPLE);
    const out = (value as (...a: unknown[]) => unknown)(...args);
    if (Array.isArray(out)) checkParts(`${path}()`, out);
    else {
      expect(typeof out, `${path}() did not return a string`).toBe('string');
      checkString(`${path}()`, out as string);
    }
    return;
  }
  if (Array.isArray(value)) {
    // A static Part[] (the interrupted-sentence primitive).
    checkParts(path, value);
    return;
  }
  expect(typeof value, `${path} is not a string`).toBe('string');
  checkString(path, value as string);
}

describe('copy catalog corpus', () => {
  it('every English catalog leaf is a well-formed string', () => {
    const acc: Leaf[] = [];
    leaves(copy, 'copy', acc);
    expect(acc.length).toBeGreaterThan(200);
    for (const leaf of acc) checkLeaf(leaf);
  });

  it('every cloud catalog leaf is a well-formed string', () => {
    const acc: Leaf[] = [];
    leaves(cloudCopy, 'cloudCopy', acc);
    expect(acc.length).toBeGreaterThan(30);
    for (const leaf of acc) checkLeaf(leaf);
  });
});
