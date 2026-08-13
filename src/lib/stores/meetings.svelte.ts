import { errorMessage } from '$lib/copy/errors';
import { invoke } from '@tauri-apps/api/core';
import type {
  MeetingDetail,
  MeetingRecord,
  MeetingStar,
  PassageLanding,
  SegmentEdit
} from '$lib/types';
import { ListSelection } from '$lib/utils/listSelection.svelte';

function isTauri() {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

/** Selection sentinel for the not-yet-persisted meeting finalizing in the
 * background (`appState.pendingMeeting`); it has no database row yet. */
export const PENDING_MEETING_ID = '__pending__';

/** How many meetings a page of the list is. The whole library is browsable;
 * this is only how much of it arrives at once. Each row costs a full
 * database row, the summary and transcript text along with it, which is why
 * the list takes them a page at a time. 100 is what the first load already
 * fetched back when that number was a ceiling instead of a page. */
const PAGE_SIZE = 100;

let _records = $state<MeetingRecord[]>([]);
let _details = $state<Record<string, MeetingDetail>>({});
let _selectedId = $state<string | null>(null);
let _isLoading = $state(false);
let _isLoadingMore = $state(false);
/** Whether the library has meetings older than the ones loaded. False once a
 * page comes back short, which is the only way to know there is no more. */
let _hasMore = $state(true);
let _detailLoadingId = $state<string | null>(null);
let _error = $state<string | null>(null);

/** Bumped by every full refresh. A page that was in flight when the list was
 * reloaded belongs to a list that no longer exists, so it is dropped rather
 * than appended to rows it does not follow. */
let _generation = 0;

/** Multi-selection for the list. `selectedId` stays the primary row (the one
 * the detail pane shows), so everything that opens a single meeting (the
 * palette, the pending sentinel, a fresh load) is untouched by it. */
const _selection = new ListSelection();

async function loadDetail(id: string) {
  if (!isTauri() || _details[id]) return;
  _detailLoadingId = id;
  _error = null;
  try {
    const detail = await invoke<MeetingDetail>('get_meeting_detail', { id });
    _details = { ..._details, [id]: detail };
  } catch (e) {
    _error = errorMessage(e);
  } finally {
    _detailLoadingId = null;
  }
}

/** Where the loaded rows end, for asking the backend what comes after them. */
function cursor(): { date: string; id: string } | null {
  const last = _records.at(-1);
  return last ? { date: last.date, id: last.id } : null;
}

/**
 * Reload the list from the top: what a delete, an import, or a finished
 * meeting leaves behind.
 *
 * It refetches as many rows as were on screen rather than just the first
 * page, so a refresh does not throw away the user's scrolling and leave them
 * looking at a list that suddenly stops.
 */
async function load() {
  if (!isTauri()) return;
  const generation = ++_generation;
  const wanted = Math.max(PAGE_SIZE, _records.length);
  _isLoading = true;
  _error = null;
  try {
    const page = await invoke<MeetingRecord[]>('get_meeting_records', {
      limit: wanted,
      since: null,
      before: null
    });
    if (generation !== _generation) return;
    _records = page;
    // A short page is the end of the library; a full one means there is at
    // least possibly more, and the next scroll finds out.
    _hasMore = page.length === wanted;
    // Rows can vanish under a selection (a delete elsewhere, a janitor prune).
    _selection.retain(_records.map((record) => record.id));
    // The pending sentinel is a valid selection even though no row backs it.
    if (
      !_selectedId ||
      (_selectedId !== PENDING_MEETING_ID &&
        !_records.some((record) => record.id === _selectedId))
    ) {
      const next = _records[0]?.id ?? null;
      _selectedId = next;
      if (next) _selection.select(next);
    }
    if (_selectedId && _selectedId !== PENDING_MEETING_ID) {
      await loadDetail(_selectedId);
    }
  } catch (e) {
    _error = errorMessage(e);
  } finally {
    // Only the newest refresh clears the flag; an older one finishing must
    // not report the list as settled while its replacement is still coming.
    if (generation === _generation) _isLoading = false;
  }
}

/**
 * Append the next page of older meetings: what the list asks for as the user
 * scrolls towards the bottom.
 *
 * Nothing is dropped or re-selected here. This only ever adds rows below the
 * ones already loaded, so a multi-selection made higher up survives, and the
 * date headers stay right because the new rows continue the same
 * newest-first order the groups are built from.
 */
async function loadMore() {
  if (!isTauri() || _isLoading || _isLoadingMore || !_hasMore) return;
  const before = cursor();
  // No rows to continue from, but the library has more: deleting every
  // loaded meeting at once leaves the list here. "More" then means the
  // first page, which is `load`'s job.
  if (!before) {
    await load();
    return;
  }

  const generation = _generation;
  _isLoadingMore = true;
  _error = null;
  try {
    const page = await invoke<MeetingRecord[]>('get_meeting_records', {
      limit: PAGE_SIZE,
      since: null,
      before
    });
    if (generation !== _generation) return;
    _hasMore = page.length === PAGE_SIZE;
    // The cursor cannot repeat a row, but a meeting edited into view while
    // the page was in flight could already be here; a duplicate id would
    // break the keyed list.
    const known = new Set(_records.map((record) => record.id));
    _records = [..._records, ...page.filter((record) => !known.has(record.id))];
  } catch (e) {
    _error = errorMessage(e);
    // Stop asking. Something is wrong with the query, and an observer that
    // fires on every scroll would otherwise retry it forever.
    _hasMore = false;
  } finally {
    _isLoadingMore = false;
  }
}

/** Where a search result wanted to land, waiting for the detail pane to
 * mount and take it. Held here rather than passed as a prop because the
 * pane is not a child of whatever opened the meeting; the palette is a
 * dialog somewhere else entirely. */
let _pendingLanding = $state<PassageLanding | null>(null);

async function select(id: string, landing?: PassageLanding) {
  // Set before the await: the detail pane reacts to the selection, and a
  // landing that arrived after it had already chosen a tab would be a frame
  // too late.
  _pendingLanding = landing ?? null;
  _selectedId = id;
  _selection.select(id);
  if (id !== PENDING_MEETING_ID) {
    await loadDetail(id);
  }
}

/** Called after rows are dropped locally. Deleting every loaded meeting at
 * once leaves the list with nothing to page from while the library still has
 * older ones, so it starts again from the top. */
async function refillIfEmptied() {
  if (_records.length === 0 && _hasMore) await load();
}

/** A click in the list: plain, Ctrl (add/remove) or Shift (range). `order` is
 * the rows as they appear on screen, so a range crosses date headers. */
async function clickRow(
  id: string,
  event: { shiftKey: boolean; ctrlKey: boolean; metaKey: boolean },
  order: string[]
) {
  _selection.click(id, event, order);
  _selectedId = _selection.primary;
  if (_selectedId && _selectedId !== PENDING_MEETING_ID) {
    await loadDetail(_selectedId);
  }
}

/** Delete every selected meeting. The pending sentinel is skipped: it has no
 * row to delete, and it is about to become one. */
async function deleteSelected() {
  const ids = _selection.ids.filter((id) => id !== PENDING_MEETING_ID);
  if (ids.length === 0) return;
  _error = null;
  try {
    await invoke('delete_meetings', { ids });
    _records = _records.filter((record) => !ids.includes(record.id));
    const details = { ..._details };
    for (const id of ids) delete details[id];
    _details = details;

    _selection.retain(_records.map((record) => record.id));
    if (_selection.count === 0) {
      const next = _records[0]?.id ?? null;
      if (next) _selection.select(next);
      _selectedId = next;
    } else {
      _selectedId = _selection.primary;
    }
    if (_selectedId && _selectedId !== PENDING_MEETING_ID) await loadDetail(_selectedId);
    await refillIfEmptied();
  } catch (e) {
    _error = errorMessage(e);
    throw e;
  }
}

function upsertDetail(detail: MeetingDetail) {
  _details = { ..._details, [detail.record.id]: detail };
  _records = _records
    .map((record) => (record.id === detail.record.id ? detail.record : record))
    .sort((a, b) => new Date(b.date).getTime() - new Date(a.date).getTime());
}

export const meetingsStore = {
  get records() {
    return _records;
  },
  get selectedId() {
    return _selectedId;
  },
  get selectedRecord() {
    return _records.find((record) => record.id === _selectedId) ?? null;
  },
  get selectedDetail() {
    return _selectedId ? (_details[_selectedId] ?? null) : null;
  },
  get isLoading() {
    return _isLoading;
  },
  /** Whether the library has meetings older than the loaded ones. The list
   * keeps a row at the bottom while this holds, and that row is what asks
   * for them. */
  get hasMore() {
    return _hasMore;
  },
  get detailLoadingId() {
    return _detailLoadingId;
  },
  get error() {
    return _error;
  },
  /** The multi-selection. `selectedId` is its primary: what the detail shows. */
  get selection() {
    return _selection;
  },
  /** Where a search result asked the detail pane to land. */
  get pendingLanding() {
    return _pendingLanding;
  },
  /** Taken once, by whoever acts on it: a landing must not fire again when
   * the user comes back to the same meeting by an ordinary click. */
  takeLanding(): PassageLanding | null {
    const landing = _pendingLanding;
    _pendingLanding = null;
    return landing;
  },

  load,
  loadMore,
  clickRow,
  deleteSelected,

  async refreshAndSelect(id?: string) {
    await load();
    const nextId = id ?? _records[0]?.id ?? null;
    if (nextId) {
      await select(nextId);
    }
  },

  select,

  loadDetail,

  async updateTitle(id: string, title: string) {
    if (!isTauri()) return;
    _error = null;
    try {
      const detail = await invoke<MeetingDetail>('update_meeting_title', { id, title });
      upsertDetail(detail);
      return detail;
    } catch (e) {
      _error = errorMessage(e);
      throw e;
    }
  },

  /** Save the user's own notes. `stars` carries where each star sits now:
   * they anchor into the notes by block ordinal, so an edit moves them and
   * a save that omitted them would leave the anchors drifting. */
  async updateNotes(id: string, markdown: string, stars: MeetingStar[]) {
    if (!isTauri()) return;
    _error = null;
    try {
      const detail = await invoke<MeetingDetail>('update_meeting_notes', {
        id,
        markdown,
        stars
      });
      upsertDetail(detail);
      return detail;
    } catch (e) {
      _error = errorMessage(e);
      throw e;
    }
  },

  async updateSummary(id: string, markdown: string) {
    if (!isTauri()) return;
    _error = null;
    try {
      const detail = await invoke<MeetingDetail>('update_meeting_summary', {
        id,
        markdown
      });
      upsertDetail(detail);
      return detail;
    } catch (e) {
      _error = errorMessage(e);
      throw e;
    }
  },

  async updateTranscript(id: string, markdown: string) {
    if (!isTauri()) return;
    _error = null;
    try {
      const detail = await invoke<MeetingDetail>('update_meeting_transcript', {
        id,
        markdown
      });
      upsertDetail(detail);
      return detail;
    } catch (e) {
      _error = errorMessage(e);
      throw e;
    }
  },

  async editSegments(id: string, edit: SegmentEdit) {
    if (!isTauri()) return;
    _error = null;
    try {
      const detail = await invoke<MeetingDetail>('edit_segments', {
        meetingId: id,
        edit
      });
      upsertDetail(detail);
      return detail;
    } catch (e) {
      _error = errorMessage(e);
      throw e;
    }
  },

  /** Apply a notes-derived name suggestion ("Speaker N is this person"). */
  async confirmNameSuggestion(id: string, label: string, name: string) {
    if (!isTauri()) return;
    _error = null;
    try {
      const detail = await invoke<MeetingDetail>('confirm_name_suggestion', {
        meetingId: id,
        label,
        name
      });
      upsertDetail(detail);
      return detail;
    } catch (e) {
      _error = errorMessage(e);
      throw e;
    }
  },

  async dismissNameSuggestion(id: string, label: string) {
    if (!isTauri()) return;
    _error = null;
    try {
      const detail = await invoke<MeetingDetail>('dismiss_name_suggestion', {
        meetingId: id,
        label
      });
      upsertDetail(detail);
      return detail;
    } catch (e) {
      _error = errorMessage(e);
      throw e;
    }
  },

  async deleteMeeting(id: string) {
    if (!isTauri()) return;
    _error = null;
    try {
      await invoke('delete_meeting', { id });
      _records = _records.filter((record) => record.id !== id);
      const { [id]: _removed, ...remaining } = _details;
      _details = remaining;
      if (_selectedId === id) {
        _selectedId = _records[0]?.id ?? null;
        if (_selectedId) {
          await loadDetail(_selectedId);
        }
      }
      await refillIfEmptied();
    } catch (e) {
      _error = errorMessage(e);
      throw e;
    }
  }
};
