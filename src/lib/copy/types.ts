// Types shared by the copy catalog (see docs/copy.md).
//
// This file imports nothing from the catalog itself, so `en/*` may import
// `Part` without a cycle. The `Copy` type (which does need `typeof en`)
// lives in index.ts for the same reason.

/** Widens a catalog's literal types so another locale can be checked against
 * it: every string becomes `string`, functions keep their exact signature, and
 * objects widen recursively.
 *
 * The function arm must precede the object fallback: a function is an
 * object, and the fallback would map over its properties and produce garbage.
 * The array arm is a safety net; the catalog holds keyed records, not arrays
 * (ordered sets are a code concern, so the component owns the array of ids).
 */
export type Widen<T> = T extends string
  ? string
  : T extends (...args: infer A) => infer R
    ? (...args: A) => R
    : T extends readonly (infer E)[]
      ? readonly Widen<E>[]
      : { [K in keyof T]: Widen<T[K]> };

/** A sparse view of a catalog for per-platform wording overlays: every key
 * optional, recursing through objects, with strings, functions, and `Part[]`
 * arrays as replace-whole leaves. Typing an overlay against the catalog is
 * the staleness guard: renaming a base key breaks the overlay at check time.
 */
export type Overlay<T> = T extends string
  ? T
  : T extends (...args: infer A) => infer R
    ? (...args: A) => R
    : T extends readonly unknown[]
      ? T
      : { [K in keyof T]?: Overlay<T[K]> };

/** One piece of a sentence that is interrupted by an interactive or styled
 * element: a link, a button, a code span. Rendered by CopyParts.svelte.
 *
 * A whole sentence is an ordered array of these, so a translator can move the
 * interrupted element anywhere in the sentence. Splitting the same sentence
 * into `before`/`after` keys would fix the word order in English forever.
 */
export type Part = string | { slot: string; text: string };
