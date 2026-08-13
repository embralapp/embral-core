// Linux wording, overlaid onto the shared English catalog at the swap
// point in ../index.ts (docs/copy.md). Only keys whose Windows wording is
// wrong on Linux appear here; the base file stays the single readable
// document per surface, and this file reads as the diff.
//
// Much shorter than the macOS overlay on purpose. Linux shares Windows'
// modifier dialect (Ctrl+K, Ctrl+S) and its window chrome, and "Close to
// tray" is already the right words, so only two keys actually differ.

import { en } from './index';
import type { Overlay } from '../types';

export const linux = {
  settings: {
    general: {
      appearance: {
        indicator: {
          // "Windows accent" names the wrong OS; the value comes from the
          // XDG settings portal here, which every desktop answers.
          accent: 'System accent'
        }
      }
    },
    mcp: {
      // No `.exe` on this platform.
      missingServer:
        'A part of embral this feature needs is missing (embral-mcp). Reinstalling the app should fix this.'
    }
  }
} satisfies Overlay<typeof en>;
