//! #2774 Hotkey key-char to global-shortcut `Code` mapping (a leaf, moved out of
//! `native.rs` for its line budget).

use tauri_plugin_global_shortcut::Code;

/// Map a normalized alphanumeric key char to a `Code` via its `PascalCase` name
/// (`Key{A..Z}` / `Digit{0..9}`), the string form `keyboard_types::Code`
/// round-trips through `FromStr`.
pub(super) fn key_char_to_code(c: char) -> Option<Code> {
    let up = c.to_ascii_uppercase();
    let name = if up.is_ascii_alphabetic() {
        format!("Key{up}")
    } else if up.is_ascii_digit() {
        format!("Digit{up}")
    } else {
        return None;
    };
    name.parse::<Code>().ok()
}
