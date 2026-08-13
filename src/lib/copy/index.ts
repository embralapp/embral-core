// The copy catalog: every user-facing string in the frontend (docs/copy.md).
//
// Read a surface file top to bottom and you read that screen's copy in the
// order a user meets it. Components never hold display strings.
//
// This is a plain .ts module, not .svelte.ts, on purpose: plain consumers
// (events.ts, bytes.ts, importRecording.ts) and any node-side tooling that
// walks the catalog must be able to import it without the runes compiler.
// Adding a second locale converts this one file to `.svelte.ts` holding a
// `$state.raw` catalog behind per-surface getters; nothing else changes.

import { platform } from '../platform';
import { en } from './en';
import { linux } from './en/linux';
import { mac } from './en/mac';
import { overlay } from './overlay';
import type { Widen } from './types';

/** The catalog's shape with literals widened, so another locale can be typed
 * against it and checked for completeness at build time. */
export type Copy = Widen<typeof en>;

// The completeness gate. `npm run check` fails here if `en` grows a value the
// schema can't widen; caught while it is being written rather than when a
// second locale is added.
const _schema: Copy = en;
void _schema;

// The platform overlay applies at the same swap point a second locale will:
// wording that is Windows-specific ("Ctrl+K", "Windows accent") reads
// correctly on macOS and Linux without forking the catalog. `en` is the
// Windows shape, so that platform needs no overlay at all.
export const copy =
  platform === 'macos' ? overlay(en, mac) : platform === 'linux' ? overlay(en, linux) : en;
