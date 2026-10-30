use super::*;

#[test]
fn classify_unauthorized() {
    assert_eq!(
        TelegramErrKind::classify("Telegram error: Unauthorized"),
        TelegramErrKind::Unauthorized
    );
}

#[test]
fn classify_conflict() {
    assert_eq!(
        TelegramErrKind::classify(
            "Telegram error: Conflict: terminated by other getUpdates request"
        ),
        TelegramErrKind::Conflict
    );
}

#[test]
fn classify_rate_limited() {
    assert_eq!(
        TelegramErrKind::classify("Telegram error: Too Many Requests: retry after 5"),
        TelegramErrKind::RateLimited
    );
    assert_eq!(
        TelegramErrKind::classify("HTTP 429 Too Many Requests"),
        TelegramErrKind::RateLimited
    );
}

#[test]
fn classify_network() {
    assert_eq!(
        TelegramErrKind::classify(
            "error sending request for url (https://api.telegram.org/bot***/getUpdates)"
        ),
        TelegramErrKind::Network
    );
    assert_eq!(
        TelegramErrKind::classify("operation timed out"),
        TelegramErrKind::Network
    );
}

#[test]
fn classify_other_falls_through() {
    assert_eq!(
        TelegramErrKind::classify("something unexpected happened"),
        TelegramErrKind::Other
    );
}

// ── #280 G-MED-3 / LOW-3 — call-site redaction tests ────────
//
// The redact() helper has its own thorough tests (15+ cases in
// telegram::redact::tests). These tests cover the INVOCATIONS at the
// BridgeLogger / DiagLogger call sites so a future refactor that
// removes the scrub wrap is caught by unit tests rather than only by
// a live smoke test (which team agents cannot run).

// Fake-but-real-shape token. NEVER use a real token in tests.
const FAKE_TG_TOKEN: &str = "987654321:FAKE_TOKEN_FOR_TESTING_xxxxxxxxxxxxxxx";
const FAKE_GEMINI_KEY: &str = "AIzaSyFakeKeyForTesting1234567890";

fn open_temp_log(dir: &std::path::Path, name: &str) -> std::fs::File {
    let path = dir.join(name);
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .expect("open temp log")
}

/// LOW-3 — `BridgeLogger::log` must redact any Telegram token in the
/// `text` payload before writing to `telegram-bridge.log` (the log
/// bypasses env_logger's sink scrub). Guards against a future caller
/// passing an unscrubbed reqwest error string.
#[test]
fn bridgelogger_log_redacts_telegram_token_in_text() {
    let tmp = tempfile::tempdir().expect("tmp");
    let path = tmp.path().join("telegram-bridge.log");
    let f = open_temp_log(tmp.path(), "telegram-bridge.log");
    let mut logger = BridgeLogger { file: Some(f) };

    let leak_shaped = format!(
        "error sending request for url (https://api.telegram.org/bot{}/getUpdates?offset=0)",
        FAKE_TG_TOKEN
    );
    logger.log("ERR", "sid-test", &leak_shaped);
    drop(logger); // close handle before read

    let content = std::fs::read_to_string(&path).expect("read log back");
    assert!(
        content.contains("/bot***/getUpdates"),
        "expected redacted token; got: {content}"
    );
    assert!(
        !content.contains(FAKE_TG_TOKEN),
        "token leaked to bridge log: {content}"
    );
}

/// LOW-1 — BridgeLogger truncates at 500 bytes. The redact step must
/// run BEFORE truncation, otherwise a token positioned so the cut
/// lands inside its tail leaves only a partial fragment (sub
/// 10-char floor) that the redactor refuses to match — leaking the
/// secret prefix. This test pre-computes a string where the pre-fix
/// code would have leaked, and asserts the post-fix code does not.
#[test]
fn bridgelogger_log_redacts_before_truncating_at_500_bytes() {
    let tmp = tempfile::tempdir().expect("tmp");
    let path = tmp.path().join("telegram-bridge.log");
    let f = open_temp_log(tmp.path(), "telegram-bridge.log");
    let mut logger = BridgeLogger { file: Some(f) };

    // Layout (byte-exact, so the 500-byte cut lands inside the token
    // tail at offset 5):
    //   481 bytes of "a" padding (`a` chosen so it is NOT a member of
    //   the token-char set we later grep for)
    // + "/bot987654321:"        (14 bytes — `/bot` + 9 digits + `:`)
    // +  5 bytes of token-chars (the secret prefix "FAKE_")
    // = 500 bytes — the truncation boundary.
    // Total text length: 481 + 14 + 37 + 11 = 543 bytes
    //   (token chars after `:` = 37; "/getUpdates" = 11).
    //
    // Pre-fix flow (truncate → redact):
    //   - Truncate keeps the first 500 bytes, dropping all but
    //     "FAKE_" from the token tail.
    //   - Redact scans the survivor; 5 < 10-char floor → NO match.
    //   - "FAKE_" leaks to the log file.
    //
    // Post-fix flow (redact → truncate):
    //   - Redact replaces `/bot<digits>:<tail>` with `/bot***`; the
    //     redacted string is 481 + 7 + 11 = 499 bytes — already
    //     under 500, so the truncation branch never fires.
    //   - No fragment of the token can survive.
    let padding = "a".repeat(481);
    let text = format!("{padding}/bot{FAKE_TG_TOKEN}/getUpdates");
    assert!(text.len() > 500, "test setup: text must exceed 500 bytes");

    logger.log("ERR", "sid-test", &text);
    drop(logger);

    let content = std::fs::read_to_string(&path).expect("read log back");
    assert!(
        content.contains("/bot***"),
        "redact marker missing — scrub did not run: {content}"
    );
    assert!(
        !content.contains(FAKE_TG_TOKEN),
        "full token leaked: {content}"
    );
    // The 5-char "FAKE_" prefix is what the pre-fix code leaked.
    // Stronger: also assert the recognizable token middle never
    // surfaces under any future regression that re-orders truncate
    // back ahead of redact.
    assert!(
        !content.contains("FAKE_"),
        "partial token prefix leaked: {content}"
    );
    assert!(
        !content.contains("FAKE_TOKEN_FOR_TESTING"),
        "token middle leaked: {content}"
    );
}

/// LOW-3 — `DiagLogger::log_raw` writes the full stabilized PTY row
/// to `diag-raw.log` with no truncation. If a row contains a Telegram
/// URL (e.g. the agent pasted one into its terminal), the scrub at
/// bridge.rs:207 must mask it before the row reaches disk.
#[test]
fn diaglogger_log_raw_redacts_telegram_token() {
    let tmp = tempfile::tempdir().expect("tmp");
    let raw_path = tmp.path().join("diag-raw.log");
    let raw = open_temp_log(tmp.path(), "diag-raw.log");
    let mut logger = DiagLogger {
        raw_file: Some(raw),
        sent_file: None,
    };

    let row = format!(
        "POST https://api.telegram.org/bot{}/sendMessage failed",
        FAKE_TG_TOKEN
    );
    logger.log_raw(&row);
    drop(logger);

    let content = std::fs::read_to_string(&raw_path).expect("read raw log");
    assert!(
        content.contains("/bot***/sendMessage"),
        "expected redacted token; got: {content}"
    );
    assert!(
        !content.contains(FAKE_TG_TOKEN),
        "token leaked to diag-raw: {content}"
    );
}

/// LOW-3 — `DiagLogger::log_sent` mirrors `log_raw` for the
/// post-filter outbound payload. Same scrub contract: any embedded
/// `[?&]key=<value>` or Telegram token must be masked.
#[test]
fn diaglogger_log_sent_redacts_gemini_key() {
    let tmp = tempfile::tempdir().expect("tmp");
    let sent_path = tmp.path().join("diag-sent.log");
    let sent = open_temp_log(tmp.path(), "diag-sent.log");
    let mut logger = DiagLogger {
        raw_file: None,
        sent_file: Some(sent),
    };

    let row = format!(
        "Gemini call: https://generativelanguage.googleapis.com/v1beta/models/\
         gemini-2.5-flash:generateContent?key={}",
        FAKE_GEMINI_KEY
    );
    logger.log_sent(&row);
    drop(logger);

    let content = std::fs::read_to_string(&sent_path).expect("read sent log");
    assert!(
        content.contains("?key=***"),
        "expected redacted key; got: {content}"
    );
    assert!(
        !content.contains(FAKE_GEMINI_KEY),
        "key leaked to diag-sent: {content}"
    );
}

#[test]
fn report_send_error_preserves_redacted_classified_error_once() {
    let events = RecordingEvents::default();
    let raw = format!(
        "ñ: error sending request for url (https://api.telegram.org/bot{FAKE_TG_TOKEN}/sendMessage)"
    );
    let msg = crate::telegram::redact::redact(&raw);
    let kind = TelegramErrKind::classify(&msg);
    report_send_error(&events, "sesión-ñ", &msg, kind);

    let calls = events.0.lock().unwrap();
    assert_eq!(
        *calls,
        vec![(
            "sesión-ñ".to_owned(),
            "ñ: error sending request for url (https://api.telegram.org/bot***/sendMessage)"
                .to_owned(),
            Some("network".to_owned()),
        )]
    );
    assert!(!calls[0].1.contains(FAKE_TG_TOKEN));
}
