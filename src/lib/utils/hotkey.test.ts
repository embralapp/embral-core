import { describe, expect, it } from 'vitest';
import { comboFromEvent, formatCombo } from './hotkey';

/**
 * These guard one contract: every combo we save has to parse in
 * `global-hotkey`'s `parse_key` (src-tauri/src/hotkey.rs calls
 * `Shortcut::from_str`). That table knows physical key names and unshifted
 * glyphs only, and a combo that misses it fails at registration time (in a
 * log line, not in the UI), so the user sees a hotkey that doesn't work.
 * `KeyboardEvent.key` walks straight into that: it reports the character,
 * so Shift rewrites it.
 */

/** The fields `comboFromEvent` reads. `code` is the physical key. */
function press(code: string, mods: Partial<Record<'ctrlKey' | 'altKey' | 'shiftKey' | 'metaKey', boolean>> = {}) {
  return { code, ctrlKey: false, altKey: false, shiftKey: false, metaKey: false, ...mods } as KeyboardEvent;
}

describe('comboFromEvent', () => {
  it('names the key pressed, not the character Shift turns it into', () => {
    // The bug: this combo used to save as "Shift+Alt+Super+}", which the Rust
    // parser rejects; `}` is a shifted glyph and is not in its table.
    const combo = comboFromEvent(press('BracketRight', { shiftKey: true, altKey: true, metaKey: true }));
    expect(combo).toBe('Alt+Shift+Super+BracketRight');
    expect(combo).not.toContain('}');
  });

  it('survives Shift on a number key', () => {
    // Used to save as "Ctrl+Shift+@".
    expect(comboFromEvent(press('Digit2', { ctrlKey: true, shiftKey: true }))).toBe('Ctrl+Shift+Digit2');
  });

  it('orders modifiers ahead of the key, as the parser requires', () => {
    expect(comboFromEvent(press('KeyR', { ctrlKey: true, altKey: true, shiftKey: true, metaKey: true }))).toBe(
      'Ctrl+Alt+Shift+Super+KeyR'
    );
  });

  it('keeps listening while only modifiers are down', () => {
    expect(comboFromEvent(press('ShiftLeft', { shiftKey: true }))).toBeNull();
    expect(comboFromEvent(press('MetaRight', { metaKey: true }))).toBeNull();
  });

  it('refuses a key the parser has no name for, rather than saving a dead combo', () => {
    // Present on many non-US layouts; `global-hotkey` has no `Code` for it.
    expect(comboFromEvent(press('IntlBackslash', { ctrlKey: true }))).toBeNull();
  });
});

// The `mac` flag is always passed explicitly here: its default reads the
// host platform, and these assertions must not change meaning between a
// Windows and a macOS checkout.
describe('formatCombo', () => {
  it('shows the glyph, not the code name', () => {
    expect(formatCombo('Alt+Shift+Super+BracketRight', false)).toBe('Alt + Shift + Super + ]');
    expect(formatCombo('Ctrl+KeyR', false)).toBe('Ctrl + R');
  });

  it('speaks mac glyphs on macos, in stored order, without separators', () => {
    expect(formatCombo('Alt+Shift+Super+BracketRight', true)).toBe('⌥⇧⌘]');
    expect(formatCombo('Ctrl+KeyR', true)).toBe('⌃R');
    // Unknown tokens still pass through.
    expect(formatCombo('Ctrl+Mystery', true)).toBe('⌃Mystery');
  });

  it('passes through combos saved by older builds', () => {
    expect(formatCombo('Ctrl+Shift+R', false)).toBe('Ctrl + Shift + R');
    expect(formatCombo('', false)).toBe('');
    expect(formatCombo('', true)).toBe('');
  });
});
