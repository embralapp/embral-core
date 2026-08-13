import { describe, expect, it } from 'vitest';
import { plural, type PluralForms } from './plural';

const meetings: PluralForms = {
  one: 'Delete meeting?',
  other: 'Delete meetings?'
};

describe('plural', () => {
  it('picks English one/other, and zero counts as other', () => {
    // English has no `zero` category: 0 selects `other`, which is why
    // "0 meetings" reads correctly without a special case.
    expect(plural('en', 1, meetings)).toBe('Delete meeting?');
    expect(plural('en', 2, meetings)).toBe('Delete meetings?');
    expect(plural('en', 0, meetings)).toBe('Delete meetings?');
  });

  it('ignores categories English never selects', () => {
    // A translator may leave `few` in place when adapting a form from another
    // locale. Under `en` it must never win.
    const withFew: PluralForms = { ...meetings, few: 'WRONG', zero: 'WRONG' };
    expect(plural('en', 0, withFew)).toBe('Delete meetings?');
    expect(plural('en', 3, withFew)).toBe('Delete meetings?');
  });

  it('selects Polish few and many', () => {
    // The reason this helper exists rather than `n === 1 ? a : b` at each call
    // site. Polish selects `few` at 2-4 and `many` at 5+; an English-shaped
    // schema cannot express either.
    const pl: PluralForms = {
      one: 'spotkanie',
      few: 'spotkania',
      many: 'spotkań',
      other: 'spotkania'
    };
    expect(plural('pl', 1, pl)).toBe('spotkanie');
    expect(plural('pl', 2, pl)).toBe('spotkania');
    expect(plural('pl', 5, pl)).toBe('spotkań');
  });

  it('falls back to other when the selected category is missing', () => {
    // An incomplete translation should render an imperfect sentence, not throw
    // and take down the screen containing it.
    const incomplete: PluralForms = { one: 'spotkanie', other: 'spotkania' };
    expect(plural('pl', 5, incomplete)).toBe('spotkania');
  });
});
