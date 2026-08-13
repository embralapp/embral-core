/**
 * Turning a keypress into a hotkey string the Rust side can actually register.
 *
 * The trap here is `KeyboardEvent.key`: it is the *character produced*, not the
 * key pressed. Hold Shift and hit the `]` key and the browser says `"}"`. The
 * `global-hotkey` crate parses combos against a fixed table of physical keys
 * (`BracketRight`, `KeyA`, `Digit1`) and their unshifted glyphs (`]`, `a`, `1`);
 * `}` is not in it, so the combo fails to parse and the hotkey silently never
 * fires. Same for `Shift+2` → `@`, and for any layout where a key's character
 * differs from its position.
 *
 * So we read `KeyboardEvent.code` (the physical key), which happens to use the
 * exact names that table accepts. `KEYS` is that table's frontend half: a key
 * we can't name in it is a key we refuse to save, rather than one we save and
 * discover is dead. The values are what the user sees.
 */

import { isMac } from '../platform';
const KEYS: Record<string, string> = {
  Backquote: '`',
  Backslash: '\\',
  BracketLeft: '[',
  BracketRight: ']',
  Comma: ',',
  Equal: '=',
  Minus: '-',
  Period: '.',
  Quote: "'",
  Semicolon: ';',
  Slash: '/',
  Space: 'Space',
  Enter: 'Enter',
  Tab: 'Tab',
  Backspace: 'Backspace',
  Delete: 'Delete',
  Insert: 'Insert',
  Home: 'Home',
  End: 'End',
  PageUp: 'PageUp',
  PageDown: 'PageDown',
  ArrowUp: '↑',
  ArrowDown: '↓',
  ArrowLeft: '←',
  ArrowRight: '→',
  CapsLock: 'CapsLock',
  NumLock: 'NumLock',
  ScrollLock: 'ScrollLock',
  PrintScreen: 'PrintScreen',
  Pause: 'Pause',
  NumpadAdd: 'Numpad +',
  NumpadSubtract: 'Numpad -',
  NumpadMultiply: 'Numpad *',
  NumpadDivide: 'Numpad /',
  NumpadDecimal: 'Numpad .',
  NumpadEnter: 'Numpad Enter'
};
for (const c of 'ABCDEFGHIJKLMNOPQRSTUVWXYZ') KEYS[`Key${c}`] = c;
for (let d = 0; d <= 9; d++) {
  KEYS[`Digit${d}`] = String(d);
  KEYS[`Numpad${d}`] = `Numpad ${d}`;
}
for (let f = 1; f <= 24; f++) KEYS[`F${f}`] = `F${f}`;

/** Modifier keys held down; a combo of nothing but these isn't a hotkey. */
const MODIFIER_CODES = /^(Control|Shift|Alt|Meta)(Left|Right)$/;

/**
 * The combo for a keypress, or null if it isn't one yet (a modifier on its own)
 * or never will be (a key the Rust parser has no name for). Modifier order
 * matches what the parser expects: modifiers first, one real key last.
 */
export function comboFromEvent(e: KeyboardEvent): string | null {
  if (MODIFIER_CODES.test(e.code)) return null;
  const key = KEYS[e.code];
  if (!key) return null;

  const parts: string[] = [];
  if (e.ctrlKey) parts.push('Ctrl');
  if (e.altKey) parts.push('Alt');
  if (e.shiftKey) parts.push('Shift');
  if (e.metaKey) parts.push('Super');
  parts.push(e.code);
  return parts.join('+');
}

/** The macOS modifier glyphs, in the order Apple writes them. The stored
 * combo keeps the portable `Ctrl+Alt+Shift+Super` vocabulary either way;
 * only the display changes. */
const MAC_GLYPHS: Record<string, string> = {
  Ctrl: '⌃',
  Alt: '⌥',
  Shift: '⇧',
  Super: '⌘'
};

/**
 * A saved combo, written the way a person would read it on this OS:
 * `Shift + Alt + Super + ]` on Windows, `⌥⇧⌘]` on macOS (glyphs, no
 * separators, Apple's convention). Anything it doesn't recognize passes
 * through as-is, so a combo saved by an older build still displays.
 */
export function formatCombo(combo: string, mac: boolean = isMac): string {
  if (!combo) return '';
  const tokens = combo.split('+');
  if (mac) {
    return tokens.map((token) => MAC_GLYPHS[token] ?? KEYS[token] ?? token).join('');
  }
  return tokens.map((token) => KEYS[token] ?? token).join(' + ');
}
