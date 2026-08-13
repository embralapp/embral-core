// Settings → MCP. Connecting AI assistants to the local notes server.

import type { Part } from '../../types';

export const mcp = {
  intro: 'Give your AI assistants read and search access to your meeting notes',
  missingServer:
    'A part of embral this feature needs is missing (embral-mcp.exe). Reinstalling the app should fix this.',

  // A sentence interrupted by the "Semantic search" link (CopyParts).
  semanticHint: [
    'Assistants search by keywords only until the ',
    { slot: 'link', text: 'Semantic search' },
    ' model is downloaded; then, they search by meaning too.'
  ] as Part[],

  clients: {
    claudeDesktop: {
      title: 'Claude Desktop',
      // Interrupted by the config path <code> (CopyParts).
      restart: (path: string): Part[] => [
        'Add this to ',
        { slot: 'code', text: path },
        ', then restart Claude Desktop:'
      ]
    },
    claudeCode: {
      title: 'Claude Code',
      instruction: 'Run this once in your terminal:'
    },
    codex: {
      // One card for the unified OpenAI app: ChatGPT desktop, the Codex
      // CLI, and the IDE extension share ~/.codex/config.toml.
      title: 'ChatGPT & Codex',
      instruction: 'Run this once in your terminal:',
      orConfig:
        'Or add this to ~/.codex/config.toml (shared by ChatGPT desktop, the Codex CLI, and the IDE extension):'
    },
    other: {
      title: 'Other clients',
      subtitle: 'Connect any client by hand',
      pointAt: 'Point the client at this server:',
      orConfig: 'Or add this configuration:'
    }
  },

  // The shared ClientCard: status line, the register/remove action, and the
  // manual-setup disclosure.
  card: {
    manualSetup: 'Manual setup',
    registered: 'Registered',
    installed: 'Installed',
    notInstalled: 'Not installed',
    checking: 'Checking...',
    working: 'Working...',
    remove: 'Remove',
    register: 'Register'
  }
};
