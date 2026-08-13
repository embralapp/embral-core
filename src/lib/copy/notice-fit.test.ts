// The notice window is one fixed 360x56 row ([shell.md] §Notices): logo,
// one line of title, the answer buttons, a quiet ✕. Nothing in that row
// wraps or scrolls, so a label that grows past the budget ships as a
// truncated title — which is issue #10's bug. This test recomputes every
// composed notice row from the catalog and fails when one stops fitting.
//
// The width model is CALIBRATED, not measured: vitest runs in node with no
// layout engine, so glyph widths come from a per-class table checked
// against live CDP getBoundingClientRect readings on Windows (2026-08-08:
// "Still recording?" 94.5px vs 101.9 modeled -> TITLE_CAL 0.93; "Continue"
// 73.5 vs 72.8 modeled and "Stop" 40.2 vs 41.8 at text-xs -> buttons
// uncalibrated; the whole silence row measured ~7px of real slack).
// Linux's default sans (DejaVu/Cantarell) runs ~8% wider; the silence
// row's "Continue" label spends most of that margin by owner's choice, so
// PLATFORM_MARGIN stays at 1.0 and the Segoe table is the contract. If a
// row here starts failing, shorten the label — do not loosen the table.
//
// Adding a notice? Add its row to ROWS below.

import { describe, expect, it } from 'vitest';
import { copy } from './index';

// --- Row chrome, from src/routes/notice/+page.svelte ------------------
const WINDOW_W = 360; // NOTICE_SIZE, src-tauri/src/notice.rs
const BUDGET = WINDOW_W - 2 * 1 - 2 * 16; // border + px-4 => 326
const GAP = 10; // gap-2.5 between flex children
const ICON = 18; // EmbralIcon size
const CLOSE = 14 + 2 * 4; // X icon + p-1
const PRIMARY_PAD = 2 * 12; // px-3
const SECONDARY_PAD = 2 * 8; // px-2

// --- Glyph widths at 14px (text-sm), Segoe UI -------------------------
const NARROW = new Set([...`iljI.,':;!| `]);
const SEMI = new Set([...`ftr()[]"-`]);
const WIDE_LOWER = new Set([...'mw']);
const WIDE_UPPER = new Set([...'MW']);

function charWidth(ch: string): number {
  if (NARROW.has(ch)) return 4.5;
  if (SEMI.has(ch)) return 5.6;
  if (WIDE_LOWER.has(ch)) return 11.0;
  if (WIDE_UPPER.has(ch)) return 12.3;
  if (ch >= '0' && ch <= '9') return 7.8;
  if (ch >= 'A' && ch <= 'Z') return 9.2;
  return 7.2; // remaining lowercase and punctuation
}

const MEDIUM = 1.03; // font-medium tracks slightly wider
// The table over-charges text-sm titles by ~7% against measured Segoe
// rendering while matching text-xs buttons — see the header. Applied to
// titles only, so button estimates stay on their measured values.
const TITLE_CAL = 0.93;

/** Estimated rendered width of `s` at the given font size. */
function textWidth(s: string, px: number, medium = false): number {
  const at14 = [...s].reduce((w, ch) => w + charWidth(ch), 0);
  return at14 * (px / 14) * (medium ? MEDIUM : 1);
}

const PLATFORM_MARGIN = 1.0; // see header comment

// --- The composed rows, mirroring every notify() call site ------------
// events.ts: recording_started, silence, start_failed, switched_to_local,
// notes_ready, call_detected, webhook_failed; updater.svelte.ts:
// update_ready. The call-accept "starting" state drops the buttons and
// the ✕, so it can only be narrower than the row it replaces.
type Action = { label: string; primary: boolean };
// `chip` reserves room in the title cell for the countdown ("120s" is the
// widest it gets — the grace is a 120s constant).
type Row = { kind: string; title: string; actions: Action[]; chip?: boolean };

const os = copy.notifications.os;
const appNames = Object.values(copy.settings.meetings.autoStart.apps.names);
const widestApp = appNames.reduce((a, b) =>
  textWidth(a, 14) >= textWidth(b, 14) ? a : b
);

const ROWS: Row[] = [
  { kind: 'recording_started', title: os.recordingStarted.title, actions: [] },
  {
    kind: 'silence',
    title: os.stillRecording.title,
    actions: [
      { label: os.stillRecording.keep, primary: true },
      { label: os.stillRecording.stop, primary: false }
    ],
    chip: true
  },
  { kind: 'start_failed', title: os.startFailed.title, actions: [] },
  { kind: 'switched_to_local', title: os.switchedToLocal.title, actions: [] },
  { kind: 'notes_ready', title: os.notesReady.title, actions: [] },
  {
    // Unknown apps degrade to an arbitrary cleaned process stem, which is
    // unbounded by design and may ellipsize; the catalog names must not.
    kind: 'call_detected',
    title: os.callDetected.title(widestApp),
    actions: [{ label: copy.shell.detectionBanner.record, primary: true }]
  },
  { kind: 'update_ready', title: os.updateReady.title, actions: [] },
  { kind: 'webhook_failed', title: os.webhookFailed.title, actions: [] }
];

function rowWidth(row: Row): number {
  // icon | title-cell | ...actions | ✕ — GAP between each adjacent pair.
  // The countdown chip lives inside the title cell (gap-1.5 = 6px), so it
  // widens that cell rather than adding a flex child.
  const chip = row.chip
    ? 6 + textWidth(copy.notifications.os.countdown(120), 12)
    : 0;
  const title = textWidth(row.title, 14, true) * TITLE_CAL + chip;
  const buttons = row.actions.map(
    (a) =>
      textWidth(a.label, 12, true) + (a.primary ? PRIMARY_PAD : SECONDARY_PAD)
  );
  const children = [ICON, title, ...buttons, CLOSE];
  const gaps = GAP * (children.length - 1);
  return (children.reduce((a, b) => a + b, 0) + gaps) * PLATFORM_MARGIN;
}

describe('notice rows fit the fixed 360px window', () => {
  for (const row of ROWS) {
    it(`${row.kind} fits untruncated`, () => {
      const width = rowWidth(row);
      expect(
        width,
        `${row.kind} needs ~${Math.round(width)}px of the ${BUDGET}px row — shorten a label`
      ).toBeLessThanOrEqual(BUDGET);
    });
  }
});
