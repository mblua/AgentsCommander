//! Redaction helpers for secret-bearing URLs and error messages.
//!
//! `reqwest::Error::Display` prints the full request URL when an HTTP
//! request fails, and several APIs we call encode credentials directly in
//! the URL — without scrubbing, those credentials reach stderr, `app.log`,
//! the error modal, and (for the Telegram bridge) remote chat users.
//!
//! Coverage:
//!
//! * **Telegram bot tokens** — `/bot<digits>:<base64url-chars>` in both
//!   `https://api.telegram.org/bot<TOKEN>/<method>` and
//!   `https://api.telegram.org/file/bot<TOKEN>/<path>` URL shapes. The
//!   token segment is identical in both; the single scanner handles both.
//!   See issue #280 §1.
//!
//! * **Gemini API keys** — `?key=<value>` or `&key=<value>` in
//!   `https://generativelanguage.googleapis.com/v1beta/...?key=<API_KEY>`.
//!   The structurally identical leak shape was surfaced by the round-1
//!   adversarial review (G-HIGH-2). The Gemini key leak is arguably worse
//!   than the Telegram one: error strings flow through `bridge.rs` to a
//!   remote chat user, not just the local log file.
//!   - **Policy:** any non-empty `[?&]key=<value>` is masked, with no
//!     minimum length floor (the plan §0 spec listed `{20,}` for parity
//!     with real Gemini key lengths; the impl is deliberately stronger as
//!     defense-in-depth). Real keys are always scrubbed; the trade-off is
//!     that an unrelated `?key=ok` style debug log is also masked. See
//!     `redact()`'s inline note and the
//!     `redacts_short_query_key_values_intentionally` test.
//!
//! Implementation notes:
//!
//! * Regex-free, allocation-light, single-pass scan.
//! * Safe to call from inside `logging.rs`'s format closure: no `log::*`
//!   macros, no panics.
//! * Returns the input unchanged (cloned) when no match is found.

/// Scrub Telegram bot tokens and Gemini API keys from an arbitrary string.
///
/// Replaces the matched secret span with `***` while preserving the
/// surrounding URL shape so operators can still tell whether the failure
/// was on `/getUpdates`, `/sendMessage`, `/file/...`, or
/// `:generateContent`. Safe to call on strings that contain no secret —
/// returns the input unchanged.
///
/// Examples:
///
/// ```text
/// "url (https://api.telegram.org/bot123456:AAGBp_x-yzzzzzzzzzzz/getUpdates?o=1)"
///   -> "url (https://api.telegram.org/bot***/getUpdates?o=1)"
///
/// "url (https://generativelanguage.googleapis.com/...?key=AIzaSyAbc123)"
///   -> "url (https://generativelanguage.googleapis.com/...?key=***)"
///
/// "Telegram error: Unauthorized"
///   -> "Telegram error: Unauthorized"  (no match, no change)
/// ```
pub fn redact(input: &str) -> String {
    // Fast path: no secret-bearing substrings present.
    let has_bot = input.contains("/bot");
    let has_key = input.contains("key=");
    if !has_bot && !has_key {
        return input.to_string();
    }

    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        // Telegram bot token: `/bot<digits>:<base64url-chars>`
        if has_bot && i + 4 <= bytes.len() && &bytes[i..i + 4] == b"/bot" {
            // Look for `<digits>:<chars>` immediately after `/bot`.
            let after_prefix = i + 4;
            let digits_end = scan_while(bytes, after_prefix, is_ascii_digit);
            if digits_end > after_prefix && digits_end < bytes.len() && bytes[digits_end] == b':' {
                let chars_start = digits_end + 1;
                let chars_end = scan_while(bytes, chars_start, is_token_char);
                if chars_end - chars_start >= 10 {
                    // Match. Keep `/bot`, replace the `<digits>:<chars>` span.
                    out.push_str("/bot***");
                    i = chars_end;
                    continue;
                }
            }
        }

        // Gemini API key: `?key=<value>` or `&key=<value>`
        // The leading byte (`?` or `&`) is consumed verbatim, then we replace
        // the value after `key=`.
        //
        // #280 LOW-2 — the plan §0 specified a 20-char floor (matching the
        // 32-char real Gemini key length minus slack). We deliberately
        // accept ANY non-empty value here as defense-in-depth: real keys
        // are still scrubbed, and a future debug log printing
        // `?key=short_test_value` is also masked. The cost is cosmetic
        // (operator surprise when a non-secret short value is masked).
        // See `redacts_short_query_key_values_intentionally` test for the
        // contract.
        if has_key && i + 5 <= bytes.len() {
            let starts_query = bytes[i] == b'?' || bytes[i] == b'&';
            if starts_query && &bytes[i + 1..i + 5] == b"key=" {
                let val_start = i + 5;
                let val_end = scan_while(bytes, val_start, is_token_char);
                if val_end > val_start {
                    // Match. Keep the `?key=` / `&key=` prefix, replace value.
                    out.push(bytes[i] as char);
                    out.push_str("key=***");
                    i = val_end;
                    continue;
                }
            }
        }

        // No match at this position — copy a full UTF-8 char. Casting
        // `bytes[i] as char` is UNSAFE for multi-byte chars: a continuation
        // byte (e.g. 0xC3 0xA9 for "é") would each map to a different
        // Unicode scalar (U+00C3 U+00A9 = "Ã©"). `i` is always at a char
        // boundary here: i=0 starts at one, the match-branch advances
        // (`chars_end`, `val_end`) only walk ASCII-byte predicates, and
        // this branch advances by `len_utf8()` of the current char.
        let ch = input[i..]
            .chars()
            .next()
            .expect("non-empty bytes must yield a char at boundary i");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

#[inline]
fn is_ascii_digit(b: u8) -> bool {
    b.is_ascii_digit()
}

#[inline]
fn is_token_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

#[inline]
fn scan_while(bytes: &[u8], start: usize, pred: fn(u8) -> bool) -> usize {
    let mut i = start;
    while i < bytes.len() && pred(bytes[i]) {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests;
