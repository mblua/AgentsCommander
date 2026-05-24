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
            if digits_end > after_prefix
                && digits_end < bytes.len()
                && bytes[digits_end] == b':'
            {
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
mod tests {
    use super::*;

    // Fake-but-real-shape Telegram token. NEVER use the real leaked token
    // in tests, even though it has been rotated. The shape (digits ":" 35+
    // base64url chars) is what the redactor matches against.
    const FAKE_TG_TOKEN: &str = "123456789:FAKE_TOKEN_FOR_TESTING_xxxxxxxxxxxxxxx";
    const FAKE_GEMINI_KEY: &str = "AIzaSyFakeKeyForTesting1234567890";

    #[test]
    fn redacts_full_telegram_url() {
        let url = format!(
            "url (https://api.telegram.org/bot{}/getUpdates?offset=0&timeout=5)",
            FAKE_TG_TOKEN
        );
        let got = redact(&url);
        assert!(
            got.contains("/bot***/getUpdates?offset=0&timeout=5"),
            "got: {got}"
        );
        assert!(!got.contains(FAKE_TG_TOKEN), "token leaked: {got}");
    }

    #[test]
    fn redacts_sendmessage_url() {
        let s = format!("POST https://api.telegram.org/bot{}/sendMessage", FAKE_TG_TOKEN);
        let got = redact(&s);
        assert!(got.contains("/bot***/sendMessage"));
        assert!(!got.contains(FAKE_TG_TOKEN));
    }

    #[test]
    fn redacts_file_download_url() {
        let s = format!(
            "GET https://api.telegram.org/file/bot{}/voice/file_0.oga",
            FAKE_TG_TOKEN
        );
        let got = redact(&s);
        assert!(got.contains("/file/bot***/voice/file_0.oga"));
        assert!(!got.contains(FAKE_TG_TOKEN));
    }

    #[test]
    fn redacts_gemini_query_key() {
        let s = format!(
            "Gemini API request failed: error sending request for url \
             (https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:generateContent?key={})",
            FAKE_GEMINI_KEY
        );
        let got = redact(&s);
        assert!(got.contains("?key=***"), "got: {got}");
        assert!(!got.contains(FAKE_GEMINI_KEY), "key leaked: {got}");
    }

    #[test]
    fn redacts_gemini_ampersand_key() {
        let s = format!("...?alt=json&key={}&foo=bar", FAKE_GEMINI_KEY);
        let got = redact(&s);
        assert!(got.contains("&key=***"));
        assert!(!got.contains(FAKE_GEMINI_KEY));
        // Preserve surrounding query params.
        assert!(got.contains("?alt=json"));
        assert!(got.contains("foo=bar"));
    }

    #[test]
    fn preserves_non_secret_text() {
        assert_eq!(redact("Telegram error: Unauthorized"), "Telegram error: Unauthorized");
        assert_eq!(redact("Conflict"), "Conflict");
        assert_eq!(redact("HTTP 429 Too Many Requests"), "HTTP 429 Too Many Requests");
    }

    #[test]
    fn preserves_empty_string() {
        assert_eq!(redact(""), "");
    }

    #[test]
    fn handles_multiple_tokens_in_one_string() {
        let s = format!(
            "first /bot{}/x and second /bot{}/y",
            FAKE_TG_TOKEN, FAKE_TG_TOKEN
        );
        let got = redact(&s);
        assert!(!got.contains(FAKE_TG_TOKEN));
        assert_eq!(got.matches("/bot***").count(), 2);
    }

    #[test]
    fn handles_telegram_and_gemini_in_one_string() {
        let s = format!(
            "tg=/bot{}/x gemini=?key={}",
            FAKE_TG_TOKEN, FAKE_GEMINI_KEY
        );
        let got = redact(&s);
        assert!(!got.contains(FAKE_TG_TOKEN));
        assert!(!got.contains(FAKE_GEMINI_KEY));
        assert!(got.contains("/bot***/x"));
        assert!(got.contains("?key=***"));
    }

    #[test]
    fn does_not_match_short_telegram_paths() {
        // "/bot1:x" is below the 10-char token-tail floor; must NOT match.
        let s = "/bot1:x and /bot22:abc";
        let got = redact(s);
        assert_eq!(got, s);
    }

    #[test]
    fn does_not_match_empty_query_value() {
        // `?key=` with no value following must not produce a phantom "***".
        let s = "/api?key=&other=1";
        let got = redact(s);
        assert_eq!(got, s);
    }

    #[test]
    fn snapshot_real_shape_url() {
        let input = format!(
            "[ERROR] Telegram poll error: error sending request for url \
             (https://api.telegram.org/bot{}/getUpdates?offset=0&timeout=5)",
            FAKE_TG_TOKEN
        );
        let expected = "[ERROR] Telegram poll error: error sending request for url \
                        (https://api.telegram.org/bot***/getUpdates?offset=0&timeout=5)";
        assert_eq!(redact(&input), expected);
    }

    #[test]
    fn does_not_match_bot_without_digits() {
        // The literal "/bot" without a numeric prefix should not be touched.
        let s = "see /botanical-garden in docs";
        assert_eq!(redact(s), s);
    }

    #[test]
    fn does_not_match_key_eq_alone() {
        // A `key=` without the leading `?` or `&` (e.g., form field) must not
        // be redacted. This avoids accidentally redacting arbitrary text.
        let s = "some_field key=hello world";
        assert_eq!(redact(s), s);
    }

    /// #280 /feature-dev review G-HIGH — the byte-walking fallback used
    /// `out.push(bytes[i] as char)` which corrupts UTF-8 continuation
    /// bytes whenever the scanner runs (i.e., when `/bot` or `key=` is
    /// present somewhere in the input). DiagLogger pipes raw agent PTY
    /// output through `redact()`, so any Spanish/Italian/CJK message
    /// containing one of those markers would have been corrupted.
    #[test]
    fn preserves_non_ascii_chars_when_scanner_runs() {
        // `/bot` substring triggers the scanner; the `/bot1:short` match
        // attempt below the 10-char floor falls through to the no-match
        // copy. Multi-byte characters must round-trip verbatim.
        let s = "café and /bot1:x is short — also résumé and 中文";
        assert_eq!(redact(s), s);
    }

    #[test]
    fn redacts_telegram_token_surrounded_by_non_ascii() {
        let s = format!(
            "résumé /bot{}/sendMessage 中文 ñ",
            FAKE_TG_TOKEN
        );
        let got = redact(&s);
        assert!(got.contains("/bot***/sendMessage"), "got: {got}");
        assert!(!got.contains(FAKE_TG_TOKEN), "token leaked: {got}");
        // Non-ASCII surrounding text preserved verbatim.
        assert!(got.contains("résumé"), "lost résumé: {got}");
        assert!(got.contains("中文"), "lost 中文: {got}");
        assert!(got.contains("ñ"), "lost ñ: {got}");
    }
}
