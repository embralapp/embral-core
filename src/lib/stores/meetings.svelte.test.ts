// @vitest-environment happy-dom
//
// The list pages through the library instead of stopping at a fixed number
// of meetings (regression 260813: `load(limit = 100)` with no pagination
// anywhere, so meeting 101 could only be reached through search).
//
// The tauri bridge is mocked with a backend that pages the way the Rust
// command does: rows strictly older than the cursor row, newest first.

import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { MeetingRecord } from '$lib/types';
import { groupByDate } from '$lib/utils/meetingFormat';

const PAGE = 100;

const backend = vi.hoisted(() => ({
  /** The whole library, newest first. */
  library: [] as { id: string; date: string }[],
  /** Every list request, so a test can count the fetches. */
  calls: [] as { limit: number; before: { date: string; id: string } | null }[]
}));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: async (command: string, args: Record<string, unknown>) => {
    if (command === 'get_meeting_records') {
      const before = (args.before ?? null) as { date: string; id: string } | null;
      backend.calls.push({ limit: args.limit as number, before });
      const from = before ? backend.library.findIndex((m) => m.id === before.id) + 1 : 0;
      return backend.library.slice(from, from + (args.limit as number)).map(record);
    }
    if (command === 'get_meeting_detail') {
      return { record: record(backend.library[0]), notes: '', segments: [], stars: [] };
    }
    if (command === 'delete_meetings') {
      const ids = new Set(args.ids as string[]);
      backend.library = backend.library.filter((m) => !ids.has(m.id));
      return null;
    }
    return null;
  }
}));

function record(row: { id: string; date: string }): MeetingRecord {
  return {
    id: row.id,
    title: row.id,
    date: row.date,
    duration_seconds: 600,
    chunks: 1,
    audio_path: ''
  };
}

/** A library of `count` meetings, one an hour apart, newest first. Mid-June,
 * so 250 of them still sit inside the one month whatever the timezone. */
function library(count: number) {
  const start = Date.UTC(2026, 5, 20, 12, 0, 0);
  return Array.from({ length: count }, (_, i) => ({
    id: `m${i}`,
    date: new Date(start - i * 3_600_000).toISOString().replace('.000', '')
  }));
}

/** Long enough after the library that every meeting groups under its month. */
const LATER = new Date('2026-08-13T12:00:00Z');

/** The store is a module singleton with no reset, so each test takes a fresh
 * copy of the module. */
async function freshStore() {
  vi.resetModules();
  return (await import('./meetings.svelte')).meetingsStore;
}

const PLAIN = { shiftKey: false, ctrlKey: false, metaKey: false };
const SHIFT = { shiftKey: true, ctrlKey: false, metaKey: false };

beforeEach(() => {
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
  backend.library = library(250);
  backend.calls = [];
});

describe('paging the meeting list', () => {
  it('reaches every meeting, not just the first page', async () => {
    const store = await freshStore();
    await store.load();
    expect(store.records.length).toBe(PAGE);
    expect(store.hasMore).toBe(true);

    // Exactly what scrolling to the bottom does, and it has to terminate.
    let guard = 0;
    while (store.hasMore && guard++ < 10) await store.loadMore();

    expect(store.records.map((m) => m.id)).toEqual(backend.library.map((m) => m.id));
    expect(store.hasMore).toBe(false);
  });

  it('asks for the rows after the last one it has, never an offset', async () => {
    const store = await freshStore();
    await store.load();
    await store.loadMore();

    const last = backend.calls.at(-1);
    expect(last?.before).toEqual({ date: backend.library[PAGE - 1].date, id: `m${PAGE - 1}` });
    // No row skipped and none repeated across the boundary.
    expect(new Set(store.records.map((m) => m.id)).size).toBe(2 * PAGE);
  });

  it('fetches one page at a time however often it is asked', async () => {
    const store = await freshStore();
    await store.load();
    backend.calls = [];

    // The scroll observer fires repeatedly while the bottom row is in view.
    await Promise.all([store.loadMore(), store.loadMore(), store.loadMore()]);

    expect(backend.calls.length).toBe(1);
    expect(store.records.length).toBe(2 * PAGE);
  });

  it('stops asking once the library runs out', async () => {
    backend.library = library(12);
    const store = await freshStore();
    await store.load();
    expect(store.hasMore).toBe(false);

    await store.loadMore();
    expect(backend.calls.length).toBe(1);
  });

  it('leaves a date group split by a page boundary as one header', async () => {
    const store = await freshStore();
    await store.load();
    await store.loadMore();

    // Both pages fall under one month heading. The list groups the loaded
    // rows as a single run, so the group has to come out once; grouping the
    // pages separately is what would print the header twice.
    const date = (m: MeetingRecord) => m.date;
    expect(groupByDate(store.records, date, LATER).length).toBe(1);
    expect(
      groupByDate(store.records.slice(0, PAGE), date, LATER).length +
        groupByDate(store.records.slice(PAGE), date, LATER).length
    ).toBe(2);
  });

  it('keeps a multi-selection when the next page arrives', async () => {
    const store = await freshStore();
    await store.load();
    const order = store.records.map((m) => m.id);
    await store.clickRow(order[2], PLAIN, order);
    await store.clickRow(order[5], SHIFT, order);
    expect(store.selection.count).toBe(4);

    await store.loadMore();

    // Appending rows drops nothing: a selection made higher up the list is
    // not something a scroll should clear.
    expect(store.selection.count).toBe(4);
    expect(store.selectedId).toBe(order[5]);
  });

  it('keeps the rows the user scrolled to across a refresh', async () => {
    const store = await freshStore();
    await store.load();
    await store.loadMore();
    expect(store.records.length).toBe(2 * PAGE);

    // A refresh after a delete or an import must not snap the list back to
    // one page under someone who had scrolled past it.
    await store.load();
    expect(store.records.length).toBe(2 * PAGE);
    expect(store.hasMore).toBe(true);
  });

  it('refills the list when every loaded meeting is deleted', async () => {
    const store = await freshStore();
    await store.load();
    const order = store.records.map((m) => m.id);
    await store.clickRow(order[0], PLAIN, order);
    await store.clickRow(order[order.length - 1], SHIFT, order);
    expect(store.selection.count).toBe(PAGE);

    await store.deleteSelected();

    // With nothing loaded there is no row to page from, so the list starts
    // again from the top rather than reading as an empty library.
    expect(backend.library.length).toBe(150);
    expect(store.records.length).toBe(PAGE);
  });
});
