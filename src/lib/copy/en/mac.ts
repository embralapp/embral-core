// macOS wording, overlaid onto the shared English catalog at the swap
// point in ../index.ts (docs/copy.md). Only keys whose Windows wording is
// wrong on a Mac appear here; the base file stays the single readable
// document per surface, and this file reads as the diff.

import { en } from './index';
import type { Overlay } from '../types';

export const mac = {
  shell: {
    titleBar: {
      commandBar: { shortcut: '⌘K' },
      close: 'Close to menu bar'
    }
  },
  meetings: {
    recording: {
      star: 'Star this moment (⌘S)'
    }
  },
  settings: {
    general: {
      appearance: {
        indicator: {
          // The default: follow the system accent color.
          accent: 'System accent'
        }
      }
    },
    mcp: {
      missingServer:
        'A part of embral this feature needs is missing (embral-mcp). Reinstalling the app should fix this.'
    }
  }
} satisfies Overlay<typeof en>;
