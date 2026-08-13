import { copy } from '$lib/copy';

/** Time-of-day only ("3:42 PM"), for list rows already sitting under a
 * date group header. */
export function formatMeetingTime(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return new Intl.DateTimeFormat(undefined, {
    hour: 'numeric',
    minute: '2-digit'
  }).format(date);
}

const startOfDay = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();

/** Midnight on the Sunday that opens `d`'s week. */
const startOfWeek = (d: Date) => startOfDay(d) - d.getDay() * 86_400_000;

/**
 * The list's date group. Calendar-aligned, Sunday-opening weeks, and the order
 * of the tests is the design; first match wins:
 *
 *   Today · Yesterday · a weekday name for the rest of this week (Monday) ·
 *   Last week (the preceding Sun–Sat) · Last month (the preceding calendar
 *   month) · "June 2026"
 *
 * Because the weekday rule only covers the current week, "Last week" is exactly
 * the previous Sun–Sat block, and "Last month" is the previous calendar month
 * with whatever last week already claimed cut away, so a week that straddles
 * the turn of the month falls under Last week, not Last month.
 *
 * A date earlier in the current month than last week (the 5th, seen on the
 * 30th) falls through to "July 2026": the current month as a heading, which is
 * unambiguous.
 */
export function dateGroupLabel(value: string, now: Date = new Date()): string {
  const groups = copy.meetings.dateGroups;
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return groups.earlier;

  const day = startOfDay(date);
  const today = startOfDay(now);
  if (day >= today) return groups.today;
  if (day === today - 86_400_000) return groups.yesterday;

  const thisWeek = startOfWeek(now);
  if (day >= thisWeek) {
    return new Intl.DateTimeFormat(undefined, { weekday: 'long' }).format(date);
  }
  if (day >= thisWeek - 7 * 86_400_000) return groups.lastWeek;

  // The previous calendar month, whatever its length.
  const prevMonth = new Date(now.getFullYear(), now.getMonth() - 1, 1);
  if (
    date.getFullYear() === prevMonth.getFullYear() &&
    date.getMonth() === prevMonth.getMonth()
  ) {
    return groups.lastMonth;
  }

  return new Intl.DateTimeFormat(undefined, { month: 'long', year: 'numeric' }).format(date);
}

const WEEKDAY_LABELS = new Set(
  // Jan 7 2024 was a Sunday. Built in local time on purpose: a UTC date can
  // format as the neighbouring weekday west of Greenwich.
  Array.from({ length: 7 }, (_, i) =>
    new Intl.DateTimeFormat(undefined, { weekday: 'long' }).format(new Date(2024, 0, 7 + i))
  )
);

/** Whether a group header already pins the day, so its rows need only a time.
 * True for Today, Yesterday, and the weekday names; false for the ranges
 * (Last week, Last month, "June 2026"), where a row must say which day it was. */
export function isSingleDayGroup(label: string): boolean {
  const groups = copy.meetings.dateGroups;
  return label === groups.today || label === groups.yesterday || WEEKDAY_LABELS.has(label);
}

/**
 * Group already-sorted items under their date headers. Both lists sort
 * newest-first, so equal labels are contiguous and a running comparison is
 * enough; no map, no re-sort (a sort here would mask an ordering bug in the
 * store).
 */
export function groupByDate<T>(
  items: T[],
  getDate: (item: T) => string,
  now: Date = new Date()
): { label: string; items: T[] }[] {
  const groups: { label: string; items: T[] }[] = [];
  for (const item of items) {
    const label = dateGroupLabel(getDate(item), now);
    const last = groups[groups.length - 1];
    if (last && last.label === label) {
      last.items.push(item);
    } else {
      groups.push({ label, items: [item] });
    }
  }
  return groups;
}

export function formatMeetingDate(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return new Intl.DateTimeFormat(undefined, {
    month: 'short',
    day: 'numeric',
    hour: 'numeric',
    minute: '2-digit'
  }).format(date);
}

/**
 * A position inside a recording, like "5:03". The player's readout, the live
 * elapsed timer, the transcript's timecode gutter, the starred moments.
 *
 * This existed five times over, privately, in five components, and had already
 * drifted: the live timer padded its minutes ("05:03") while every other
 * timecode in the app did not. Truncating, not rounding: a moment at 5:03.9 is
 * still 5:03, and rounding it up would name a second that has not happened yet.
 */
export function formatTime(seconds: number): string {
  const safeSeconds = Math.max(0, Math.floor(seconds));
  const minutes = Math.floor(safeSeconds / 60);
  const remainder = safeSeconds % 60;
  return `${minutes}:${remainder.toString().padStart(2, '0')}`;
}

/**
 * How long something lasted: "5:03", or "1:22:33" once it runs past an
 * hour (hours appear only when needed). A duration rounds (a 90.6-second
 * meeting lasted 91 seconds); a position ([`formatTime`]) truncates. The
 * two are different questions.
 */
export function formatDuration(totalSeconds: number): string {
  const safeSeconds = Math.max(0, Math.round(totalSeconds));
  const minutes = Math.floor(safeSeconds / 60);
  const seconds = safeSeconds % 60;
  if (minutes >= 60) {
    const hours = Math.floor(minutes / 60);
    const remainingMinutes = minutes % 60;
    return `${hours}:${remainingMinutes.toString().padStart(2, '0')}:${seconds
      .toString()
      .padStart(2, '0')}`;
  }
  return `${minutes}:${seconds.toString().padStart(2, '0')}`;
}
