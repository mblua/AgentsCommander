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
//! * **AgentsCommander credentials** (#4319):
//!   - container API tokens `ac-container-<uuid>-<uuid>` are always masked
//!     (`pty/container_tokens.rs` mints them with that prefix);
//!   - a bare UUID is masked only right after a secret context:
//!     `AGENTSCOMMANDER_TOKEN=`, `AGENTSCOMMANDER_API_TOKEN=`,
//!     `--token <x>`, `--token=<x>` or `Bearer <x>`. Session, message,
//!     client and op ids are UUIDs too and stay visible elsewhere.
//!   - Known gap: a secret UUID logged with no context is not masked.
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
    let spans = secret_spans(input);
    if spans.is_empty() {
        return input.to_string();
    }
    let mut out = String::with_capacity(input.len());
    let mut last = 0;
    for (start, end) in spans {
        out.push_str(&input[last..start]);
        out.push_str("***");
        last = end;
    }
    out.push_str(&input[last..]);
    out
}

/// [`redact`] plus every occurrence of the exact `needles`, in ONE pass over
/// the original text: collect both sets of spans, merge the ones that overlap
/// or touch, then render once. Two sequential passes let one pass break the
/// other's match and leave a secret tail in clear (#4319). A merged span that
/// holds a needle prints `<redacted>`; one made only of redactor spans `***`.
pub(crate) fn redact_with_needles(text: &str, needles: &[String]) -> String {
    let mut spans: Vec<(usize, usize, bool)> = secret_spans(text)
        .into_iter()
        .map(|(start, end)| (start, end, false))
        .collect();
    for needle in needles.iter().filter(|s| !s.is_empty()) {
        spans.extend(
            text.match_indices(needle.as_str())
                .map(|(at, s)| (at, at + s.len(), true)),
        );
    }
    if spans.is_empty() {
        return text.to_string();
    }
    spans.sort_unstable();
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut iter = spans.into_iter().peekable();
    while let Some((start, mut end, mut needle)) = iter.next() {
        while let Some(&(next_start, next_end, next_needle)) = iter.peek() {
            if next_start > end {
                break;
            }
            end = end.max(next_end);
            needle |= next_needle;
            iter.next();
        }
        out.push_str(&text[last..start]);
        out.push_str(if needle { "<redacted>" } else { "***" });
        last = end;
    }
    out.push_str(&text[last..]);
    out
}

/// Byte ranges of `input` that [`redact`] masks, in order and disjoint.
/// Each range starts and ends on a char boundary (the scanners only walk
/// ASCII bytes).
fn secret_spans(input: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    // Fast path: no secret-bearing substrings present.
    let has_bot = input.contains("/bot");
    let has_key = input.contains("key=");
    let has_ac = input.contains(CONTAINER_PREFIX)
        || input.contains("TOKEN=")
        || input.contains("--token")
        || input.contains("Bearer ");
    if !has_bot && !has_key && !has_ac {
        return spans;
    }

    let bytes = input.as_bytes();
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
                    // Match. Keep `/bot`, mask the `<digits>:<chars>` span.
                    spans.push((after_prefix, chars_end));
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
                    // Match. Keep the `?key=` / `&key=` prefix, mask value.
                    spans.push((val_start, val_end));
                    i = val_end;
                    continue;
                }
            }
        }

        // AgentsCommander credentials (#4319). Keep the context prefix,
        // mask only the secret.
        if has_ac {
            if let Some((keep, end)) = match_ac_credential(bytes, i) {
                spans.push((keep, end));
                i = end;
                continue;
            }
        }

        // No match at this position — skip a full UTF-8 char. `i` is always
        // at a char boundary here: i=0 starts at one, the match-branch
        // advances (`chars_end`, `val_end`, `end`) only walk ASCII-byte
        // predicates, and this branch advances by `len_utf8()` of the
        // current char. That also keeps every span on char boundaries.
        let ch = input[i..]
            .chars()
            .next()
            .expect("non-empty bytes must yield a char at boundary i");
        i += ch.len_utf8();
    }
    spans
}

const CONTAINER_PREFIX: &str = "ac-container-";

/// Contexts after which a UUID is a secret. The bool says whether one or
/// more spaces must separate the context from the value.
const SECRET_CONTEXTS: [(&str, bool); 5] = [
    ("AGENTSCOMMANDER_TOKEN=", false),
    ("AGENTSCOMMANDER_API_TOKEN=", false),
    ("--token=", false),
    ("--token", true),
    ("Bearer", true),
];

/// If an AgentsCommander credential starts at `i`, return
/// `(keep_end, secret_end)`: bytes `i..keep_end` stay verbatim and
/// `keep_end..secret_end` is the secret to mask.
fn match_ac_credential(bytes: &[u8], i: usize) -> Option<(usize, usize)> {
    let prefix = CONTAINER_PREFIX.as_bytes();
    if bytes[i..].starts_with(prefix) {
        let first = i + prefix.len();
        if let Some(mid) = uuid_end(bytes, first) {
            if mid < bytes.len() && bytes[mid] == b'-' {
                if let Some(end) = uuid_end(bytes, mid + 1) {
                    return Some((first, end));
                }
            }
        }
    }
    for (ctx, spaced) in SECRET_CONTEXTS {
        if !bytes[i..].starts_with(ctx.as_bytes()) {
            continue;
        }
        let mut start = i + ctx.len();
        if spaced {
            let after = scan_while(bytes, start, |b| b == b' ');
            if after == start {
                continue;
            }
            start = after;
        }
        // Optional opening quote, kept verbatim.
        if start < bytes.len() && (bytes[start] == b'"' || bytes[start] == b'\'') {
            start += 1;
        }
        if let Some(end) = uuid_end(bytes, start) {
            return Some((start, end));
        }
    }
    None
}

/// End of a canonical hyphenated UUID starting at `start`, if one is there
/// and is not glued to more alphanumeric or `_` chars.
fn uuid_end(bytes: &[u8], start: usize) -> Option<usize> {
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    let mut i = start;
    for (n, len) in GROUPS.into_iter().enumerate() {
        if n > 0 {
            if i >= bytes.len() || bytes[i] != b'-' {
                return None;
            }
            i += 1;
        }
        let end = i + len;
        if end > bytes.len() || !bytes[i..end].iter().all(u8::is_ascii_hexdigit) {
            return None;
        }
        i = end;
    }
    if i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
        return None;
    }
    Some(i)
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
