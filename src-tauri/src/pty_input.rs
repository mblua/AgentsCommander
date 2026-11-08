//! Terminal input scalar policy: the one forbidden-scalar predicate shared by
//! PTY input validation and injected-message sanitization. It holds no PTY
//! transport, validation errors, length checks, rendering, normalization or
//! storage, and imports nothing from the crate.

/// #1157 - the forbidden-scalar set of `crate::pty::inject::validate_pty_input_text`,
/// shared so the injected-message sanitizer
/// (`crate::config::injected_messages::render::sanitize`) strips exactly what
/// that validator rejects. Deriving both from one predicate is what keeps the
/// two from drifting; there must be no second copy of this list.
///
/// Covers C0 except `\t` and `\n`, DEL and C1, and the whole bidi and separator
/// class (U+061C, U+200E, U+200F, U+2028, U+2029, U+202A-U+202E, U+2066-U+2069).
pub(crate) fn is_forbidden_pty_scalar(ch: char) -> bool {
    matches!(
        ch,
        '\u{0000}'..='\u{0008}'
            | '\u{000b}'
            | '\u{000c}'
            | '\u{000d}'
            | '\u{000e}'..='\u{001f}'
            | '\u{007f}'..='\u{009f}'
            | '\u{061c}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
    )
}
