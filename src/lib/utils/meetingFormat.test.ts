import { describe, expect, it } from 'vitest';
import {
  dateGroupLabel,
  formatDuration,
  formatTime,
  groupByDate,
  isSingleDayGroup
} from './meetingFormat';

/**
 * The date buckets. These were verified once with a throwaway script that no
 * longer exists, which is what this whole phase is about. The awkward cases are
 * the point: the ordering of the rules is the design, and a plausible-looking
 * reshuffle silently breaks the week that straddles the turn of the month.
 *
 * Locale discipline: Today / Yesterday / Last week / Last month are literals and
 * safe to assert. The weekday and month-year branches go through `Intl`, so they
 * are asserted against `Intl` itself; hard-coding "Monday" would pass here and
 * fail on a machine with a different locale.
 */
describe('dateGroupLabel', () => {
  const weekdayOf = (iso: string) =>
    new Intl.DateTimeFormat(undefined, { weekday: 'long' }).format(new Date(iso));
  const monthYearOf = (iso: string) =>
    new Intl.DateTimeFormat(undefined, { month: 'long', year: 'numeric' }).format(
      new Date(iso)
    );

  // A Wednesday. Its week opens on Sunday the 12th.
  const wednesday = new Date('2026-07-15T09:00');

  it('names today and yesterday', () => {
    expect(dateGroupLabel('2026-07-15T08:00', wednesday)).toBe('Today');
    expect(dateGroupLabel('2026-07-14T08:00', wednesday)).toBe('Yesterday');
  });

  it('names the remaining days of this week by weekday', () => {
    expect(dateGroupLabel('2026-07-13T08:00', wednesday)).toBe(weekdayOf('2026-07-13T08:00'));
    // Sunday opens the week, so it is still "this week".
    expect(dateGroupLabel('2026-07-12T08:00', wednesday)).toBe(weekdayOf('2026-07-12T08:00'));
  });

  it('closes the week at Saturday', () => {
    // The day before this week's Sunday belongs to last week, not to a weekday.
    expect(dateGroupLabel('2026-07-11T08:00', wednesday)).toBe('Last week');
    expect(dateGroupLabel('2026-07-05T08:00', wednesday)).toBe('Last week');
  });

  it('falls through to the month once a date is older than last week', () => {
    // Still July, but older than last week: it gets the month heading, not a
    // relative one. Unambiguous, if a little odd-looking mid-month.
    expect(dateGroupLabel('2026-07-04T08:00', wednesday)).toBe(monthYearOf('2026-07-04T08:00'));
  });

  it('names the previous calendar month', () => {
    expect(dateGroupLabel('2026-06-28T08:00', wednesday)).toBe('Last month');
    expect(dateGroupLabel('2026-05-20T08:00', wednesday)).toBe(monthYearOf('2026-05-20T08:00'));
  });

  it('lets last week win over last month when a week straddles the turn of the month', () => {
    // The central case. Now is Wed 8 July; last week is Sun 28 June – Sat 4
    // July, which is mostly June. A June date inside that week is Last week,
    // not Last month; the rules are ordered, and reordering them breaks
    // exactly this.
    const straddle = new Date('2026-07-08T09:00');
    expect(dateGroupLabel('2026-06-30T08:00', straddle)).toBe('Last week');
    // A June date outside it falls through to Last month as usual.
    expect(dateGroupLabel('2026-06-27T08:00', straddle)).toBe('Last month');
  });

  it('lets yesterday win over the week rules', () => {
    // On a Sunday, yesterday is last week's Saturday.
    const sunday = new Date('2026-07-12T09:00');
    expect(dateGroupLabel('2026-07-11T08:00', sunday)).toBe('Yesterday');
    expect(dateGroupLabel('2026-07-10T08:00', sunday)).toBe('Last week');

    // On a Monday, yesterday is this week's Sunday; Yesterday still wins over
    // the weekday name.
    const monday = new Date('2026-07-13T09:00');
    expect(dateGroupLabel('2026-07-12T08:00', monday)).toBe('Yesterday');
    expect(dateGroupLabel('2026-07-11T08:00', monday)).toBe('Last week');
  });

  it('crosses the year for last month', () => {
    const january = new Date('2026-01-20T09:00');
    expect(dateGroupLabel('2025-12-15T08:00', january)).toBe('Last month');
    expect(dateGroupLabel('2025-11-15T08:00', january)).toBe(monthYearOf('2025-11-15T08:00'));
  });

  it('does not throw on a date it cannot read', () => {
    expect(dateGroupLabel('not a date', wednesday)).toBe('Earlier');
  });
});

describe('isSingleDayGroup', () => {
  const wednesday = new Date('2026-07-15T09:00');

  it('is true exactly for the headers that already name a day', () => {
    // Asserted by round-trip, so it holds in any locale: a header that names one
    // day lets its rows show a time; a range header must not.
    for (const iso of ['2026-07-15T08:00', '2026-07-14T08:00', '2026-07-13T08:00']) {
      expect(isSingleDayGroup(dateGroupLabel(iso, wednesday))).toBe(true);
    }
    for (const iso of ['2026-07-05T08:00', '2026-06-28T08:00', '2026-05-20T08:00']) {
      expect(isSingleDayGroup(dateGroupLabel(iso, wednesday))).toBe(false);
    }
  });
});

describe('groupByDate', () => {
  const now = new Date('2026-07-15T09:00');
  const at = (date: string) => ({ date });

  it('collects contiguous runs under one header', () => {
    const groups = groupByDate(
      [at('2026-07-15T10:00'), at('2026-07-15T08:00'), at('2026-07-14T08:00')],
      (item) => item.date,
      now
    );
    expect(groups.map((g) => g.label)).toEqual(['Today', 'Yesterday']);
    expect(groups[0].items).toHaveLength(2);
  });

  it('does not re-sort', () => {
    // The store owns the order (newest first). Sorting here would paper over an
    // ordering bug in the store instead of exposing it, so an out-of-order
    // input must produce a repeated header, visibly wrong.
    const groups = groupByDate(
      [at('2026-07-15T08:00'), at('2026-07-14T08:00'), at('2026-07-15T10:00')],
      (item) => item.date,
      now
    );
    expect(groups.map((g) => g.label)).toEqual(['Today', 'Yesterday', 'Today']);
  });

  it('has nothing to group when there is nothing', () => {
    expect(groupByDate([], (item: { date: string }) => item.date, now)).toEqual([]);
  });
});

describe('formatTime', () => {
  it('reads as a position in the recording', () => {
    expect(formatTime(0)).toBe('0:00');
    expect(formatTime(63)).toBe('1:03');
    expect(formatTime(600)).toBe('10:00');
  });

  it('truncates rather than rounding', () => {
    // A moment at 5:03.9 is still 5:03: rounding up would name a second that
    // has not happened yet, and seeking there would end up past the mark.
    expect(formatTime(303.9)).toBe('5:03');
  });

  it('keeps counting past an hour rather than wrapping', () => {
    expect(formatTime(3600)).toBe('60:00');
    expect(formatTime(7325)).toBe('122:05');
  });

  it('clamps a negative to zero', () => {
    expect(formatTime(-5)).toBe('0:00');
  });
});

describe('formatDuration', () => {
  it('reads as a length', () => {
    expect(formatDuration(0)).toBe('0:00');
    expect(formatDuration(63)).toBe('1:03');
  });

  it('rounds, unlike formatTime', () => {
    // A duration is a measurement: 90.6 seconds lasted 91 seconds. A position
    // truncates. The two really are different questions.
    expect(formatDuration(90.6)).toBe('1:31');
    expect(formatTime(90.6)).toBe('1:30');
  });

  it('grows an hours digit once it runs long, keeping the seconds', () => {
    expect(formatDuration(3600)).toBe('1:00:00');
    expect(formatDuration(4830)).toBe('1:20:30');
    expect(formatDuration(4953)).toBe('1:22:33');
  });

  it('clamps a negative to zero', () => {
    expect(formatDuration(-10)).toBe('0:00');
  });
});
