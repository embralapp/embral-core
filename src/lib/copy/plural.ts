// The one plural primitive (see docs/copy.md).
//
// English needs two forms; Polish needs four. Writing `n === 1 ? a : b` at the
// call site bakes the English shape into the catalog's schema, which is the
// expensive thing to undo later, so the primitive is plural-rule-aware from
// the start even though only `en` exists today.

/** The forms a string takes across plural categories.
 *
 * `one` and `other` are required so an English catalog can't under-specify;
 * the rest are optional so a Polish one can be complete. Values are complete
 * sentences, never a noun to splice into a template; the real cases in this
 * app vary well beyond the noun ("its notes, transcript, and audio" becomes
 * "their notes, transcripts, and audio").
 */
export type PluralForms = { one: string; other: string } &
  Partial<Record<'zero' | 'two' | 'few' | 'many', string>>;

// Intl.PluralRules construction is not free and the same handful of locales
// repeat for the life of the process.
const rules = new Map<string, Intl.PluralRules>();

function rulesFor(locale: string): Intl.PluralRules {
  let r = rules.get(locale);
  if (!r) {
    r = new Intl.PluralRules(locale);
    rules.set(locale, r);
  }
  return r;
}

/** Picks the form matching `n` under `locale`'s plural rules.
 *
 * `locale` is explicit and never defaults: `undefined` would mean the system
 * locale, which is exactly the divergence between UI language and OS language
 * that this catalog exists to keep straight. Each locale directory exports its
 * own constant.
 *
 * A category the caller didn't supply falls back to `other` rather than
 * throwing: a missing Polish `few` should render an imperfect sentence, not
 * take down the screen that contains it.
 */
export function plural(locale: string, n: number, forms: PluralForms): string {
  const category = rulesFor(locale).select(n);
  return forms[category] ?? forms.other;
}
