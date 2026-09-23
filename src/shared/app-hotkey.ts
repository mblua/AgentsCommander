/**
 * #2236 phase 5 — pure parse and match functions for the configurable sidebar
 * compact toggle shortcut. The signal and the default live in
 * `./sidebar-compact` (D16a); this module declares neither.
 *
 * The accepted set mirrors `validate_sidebar_compact_hotkey` in
 * `src-tauri/src/config/settings.rs`, which is the authority: `Ctrl+Shift+<A-Z>`
 * minus the shipped `W/R/C/V`, never a digit (D8).
 */
export const RESERVED_HOTKEY_LETTERS = ["w", "r", "c", "v"] as const;

function isReserved(letter: string): boolean {
  return (RESERVED_HOTKEY_LETTERS as readonly string[]).includes(letter.toLowerCase());
}

export function parseAppHotkey(value: string): { letter: string } | null {
  const parts = value.trim().split("+").map((p) => p.trim());
  if (parts.length !== 3) return null;
  const [ctrl, shift, letter] = parts;
  if (!/^(ctrl|control)$/i.test(ctrl) || !/^shift$/i.test(shift)) return null;
  // /^[A-Za-z]$/, never toLowerCase(): "K".toLowerCase() === "k" would
  // accept the Kelvin sign that Rust's is_ascii_alphabetic rejects.
  if (!/^[A-Za-z]$/.test(letter)) return null;
  if (isReserved(letter)) return null;
  return { letter: letter.toLowerCase() };
}

/** "e" -> "KeyE" */
export function hotkeyEventCode(parsed: { letter: string }): string {
  return "Key" + parsed.letter.toUpperCase();
}

/** Matches on the physical key (`e.code`), so every layout works. */
export function matchesHotkeyEvent(e: KeyboardEvent, value: string): boolean {
  if (e.repeat || e.isComposing) return false;
  if (!e.ctrlKey || !e.shiftKey || e.altKey || e.metaKey) return false;
  // D8b: a key that reports a reserved letter belongs to the shipped binding.
  if (isReserved(e.key)) return false;
  const parsed = parseAppHotkey(value);
  return parsed !== null && e.code === hotkeyEventCode(parsed);
}

export function letterFromCaptureEvent(
  e: KeyboardEvent,
): { letter: string } | { error: string } | null {
  if (!e.ctrlKey || !e.shiftKey || e.altKey || e.metaKey) return null;
  const match = /^Key([A-Z])$/.exec(e.code);
  if (!match) return null;
  const letter = match[1];
  if (isReserved(letter) || isReserved(e.key)) {
    return {
      error: `Ctrl+Shift+${letter} conflicts with a built-in shortcut (Ctrl+Shift+W/R/C/V).`,
    };
  }
  return { letter };
}
