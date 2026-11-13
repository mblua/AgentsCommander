use super::*;

// Fake-but-real-shape Telegram token. NEVER use the real leaked token
// in tests, even though it has been rotated. The shape (digits ":" 35+
// base64url chars) is what the redactor matches against.
const FAKE_TG_TOKEN: &str = "123456789:FAKE_TOKEN_FOR_TESTING_xxxxxxxxxxxxxxx";
const FAKE_GEMINI_KEY: &str = "AIzaSyFakeKeyForTesting1234567890";

#[test]
fn redacts_full_telegram_url() {
    let url =
        format!("url (https://api.telegram.org/bot{FAKE_TG_TOKEN}/getUpdates?offset=0&timeout=5)");
    let got = redact(&url);
    assert!(
        got.contains("/bot***/getUpdates?offset=0&timeout=5"),
        "got: {got}"
    );
    assert!(!got.contains(FAKE_TG_TOKEN), "token leaked: {got}");
}

#[test]
fn redacts_sendmessage_url() {
    let s = format!("POST https://api.telegram.org/bot{FAKE_TG_TOKEN}/sendMessage");
    let got = redact(&s);
    assert!(got.contains("/bot***/sendMessage"));
    assert!(!got.contains(FAKE_TG_TOKEN));
}

#[test]
fn redacts_file_download_url() {
    let s = format!("GET https://api.telegram.org/file/bot{FAKE_TG_TOKEN}/voice/file_0.oga");
    let got = redact(&s);
    assert!(got.contains("/file/bot***/voice/file_0.oga"));
    assert!(!got.contains(FAKE_TG_TOKEN));
}

#[test]
fn redacts_gemini_query_key() {
    let s = include_str!("tests/fixtures/gemini-error.txt");
    let got = redact(s);
    assert!(got.contains("?key=***"), "got: {got}");
    assert!(!got.contains(FAKE_GEMINI_KEY), "key leaked: {got}");
}

#[test]
fn redacts_gemini_ampersand_key() {
    let s = format!("...?alt=json&key={FAKE_GEMINI_KEY}&foo=bar");
    let got = redact(&s);
    assert!(got.contains("&key=***"));
    assert!(!got.contains(FAKE_GEMINI_KEY));
    // Preserve surrounding query params.
    assert!(got.contains("?alt=json"));
    assert!(got.contains("foo=bar"));
}

#[test]
fn preserves_non_secret_text() {
    assert_eq!(
        redact("Telegram error: Unauthorized"),
        "Telegram error: Unauthorized"
    );
    assert_eq!(redact("Conflict"), "Conflict");
    assert_eq!(
        redact("HTTP 429 Too Many Requests"),
        "HTTP 429 Too Many Requests"
    );
}

#[test]
fn preserves_empty_string() {
    assert_eq!(redact(""), "");
}

#[test]
fn handles_multiple_tokens_in_one_string() {
    let s = format!("first /bot{FAKE_TG_TOKEN}/x and second /bot{FAKE_TG_TOKEN}/y");
    let got = redact(&s);
    assert!(!got.contains(FAKE_TG_TOKEN));
    assert_eq!(got.matches("/bot***").count(), 2);
}

#[test]
fn handles_telegram_and_gemini_in_one_string() {
    let s = format!("tg=/bot{FAKE_TG_TOKEN}/x gemini=?key={FAKE_GEMINI_KEY}");
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
         (https://api.telegram.org/bot{FAKE_TG_TOKEN}/getUpdates?offset=0&timeout=5)"
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
/// present somewhere in the input). `DiagLogger` pipes raw agent PTY
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

/// #280 LOW-2 — the implementation deliberately deviates from the
/// plan's 20-char floor for Gemini `?key=<value>` matches: any
/// non-empty value is masked. This is safer than the spec (real keys
/// are still scrubbed), at the cost of masking unrelated short query
/// values in debug logs. Locked in so a future refactor that
/// re-introduces the floor must update this test (and the docstring)
/// rather than silently weaken the contract.
#[test]
fn redacts_short_query_key_values_intentionally() {
    // 2-char value, well below the spec's 20-char floor.
    assert_eq!(redact("?key=ok"), "?key=***");
    // Ampersand variant.
    assert_eq!(redact("&key=ab"), "&key=***");
    // Inside a longer URL; surrounding text preserved.
    let got = redact("https://example.test/v1/x?alt=json&key=hi&foo=bar");
    assert!(got.contains("&key=***"), "got: {got}");
    assert!(got.contains("?alt=json"), "lost prefix: {got}");
    assert!(got.contains("foo=bar"), "lost suffix: {got}");
    // Empty-value contract (`?key=` followed by `&` / end) is the
    // only case that escapes — guarded separately by
    // `does_not_match_empty_query_value`.
}

#[test]
fn redacts_telegram_token_surrounded_by_non_ascii() {
    let s = format!("résumé /bot{FAKE_TG_TOKEN}/sendMessage 中文 ñ");
    let got = redact(&s);
    assert!(got.contains("/bot***/sendMessage"), "got: {got}");
    assert!(!got.contains(FAKE_TG_TOKEN), "token leaked: {got}");
    // Non-ASCII surrounding text preserved verbatim.
    assert!(got.contains("résumé"), "lost résumé: {got}");
    assert!(got.contains("中文"), "lost 中文: {got}");
    assert!(got.contains("ñ"), "lost ñ: {got}");
}

// #4319 — AgentsCommander credentials. `logging.rs` passes every log line
// and every error-modal message through `redact()`, so testing `redact()`
// covers both sinks.
const FAKE_UUID: &str = "1b4e28ba-2fa1-41d2-883f-0016d3cca427";

/// #4319 — container API tokens are masked wherever they appear.
#[test]
fn redacts_container_api_token() {
    let msg = redact(&format!(
        "auth failed for ac-container-{FAKE_UUID}-9f0c2a7e-5d1b-4c3a-8e6f-2b7d9a1c4e50 (expired)"
    ));
    assert_eq!(msg, "auth failed for ac-container-*** (expired)");
}

/// #4319 — a UUID right after each secret context is masked; the context
/// itself stays so the operator can see which credential was involved.
#[test]
fn redacts_uuid_after_secret_contexts() {
    for (raw, want) in [
        (
            format!("env AGENTSCOMMANDER_TOKEN={FAKE_UUID} set"),
            "env AGENTSCOMMANDER_TOKEN=*** set",
        ),
        (
            format!("env AGENTSCOMMANDER_API_TOKEN=\"{FAKE_UUID}\""),
            "env AGENTSCOMMANDER_API_TOKEN=\"***\"",
        ),
        (
            format!("ran send --token {FAKE_UUID} --root x"),
            "ran send --token *** --root x",
        ),
        (
            format!("ran send --token={FAKE_UUID}"),
            "ran send --token=***",
        ),
        (
            format!("header Authorization: Bearer {FAKE_UUID}"),
            "header Authorization: Bearer ***",
        ),
    ] {
        let msg = redact(&raw);
        assert_eq!(msg, want, "input: {raw}");
        assert!(!msg.contains(FAKE_UUID), "leaked: {msg}");
    }
}

/// #4319 — bare ids (session/message/client/op) are UUIDs too and must
/// stay visible: only a secret context marks a UUID as a secret.
#[test]
fn keeps_bare_uuid_without_secret_context() {
    for raw in [
        format!("session {FAKE_UUID} closed"),
        format!("queued message {FAKE_UUID} to peer"),
        format!("client_id=container-{FAKE_UUID}-x"),
        format!("--token-file {FAKE_UUID}"),
        format!("Bearer-ish {FAKE_UUID}"),
    ] {
        assert_eq!(redact(&raw), raw);
    }
}

/// #4319 — `{:?}` of a Telegram bot config never shows the token, while
/// serialization still writes it.
#[test]
fn telegram_bot_config_debug_redacts_token() {
    let bot = crate::telegram::types::TelegramBotConfig {
        id: "bot-1".to_string(),
        label: "Main".to_string(),
        token: "987654321:FAKE_TOKEN_FOR_TESTING_xxxxxxxxxxxxxxx".to_string(),
        chat_id: 42,
        color: "blue".to_string(),
    };
    let debug = format!("{bot:?}");
    assert!(!debug.contains("FAKE_TOKEN_FOR_TESTING"), "leaked: {debug}");
    assert!(debug.contains("token: \"[REDACTED]\""), "got: {debug}");
    assert!(debug.contains("chat_id: 42"), "got: {debug}");
    let json = serde_json::to_string(&bot).expect("serialize");
    assert!(
        json.contains("FAKE_TOKEN_FOR_TESTING"),
        "serialization changed: {json}"
    );
}
