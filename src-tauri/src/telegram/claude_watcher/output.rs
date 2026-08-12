use std::io::Write as IoWrite;

use tauri::Emitter;
use tokio::time::Duration;

use crate::network::OutboundNetwork;
use crate::telegram::api;

// ── #280 §3.2 — error classification + throttling ─────────────

/// Coarse classification of Telegram API failure modes. Used to tag log
/// lines so an operator can grep by kind and to drive the throttle below
/// (`Network` errors burst; the rare kinds emit unthrottled).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TelegramErrKind {
    Network,
    Unauthorized,
    Conflict,
    RateLimited,
    Other,
}

impl TelegramErrKind {
    /// Substring-match classification against `AppError::Telegram` strings
    /// (already redacted by api.rs §1.3 — token shape never affects
    /// classification). Order matters: precise API-side errors must be
    /// tested before the generic transport-error catchall.
    pub(crate) fn classify(msg: &str) -> Self {
        let lc = msg.to_lowercase();
        if lc.contains("unauthorized") {
            Self::Unauthorized
        } else if lc.contains("conflict") {
            Self::Conflict
        } else if lc.contains("too many requests") || lc.contains("429") {
            Self::RateLimited
        } else if lc.contains("error sending request") || lc.contains("timed out") {
            Self::Network
        } else {
            Self::Other
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::Unauthorized => "unauthorized",
            Self::Conflict => "conflict",
            Self::RateLimited => "rate_limited",
            Self::Other => "other",
        }
    }
}

// ── File logger ──────────────────────────────────────────────

pub(crate) struct BridgeLogger {
    file: Option<std::fs::File>,
}

impl BridgeLogger {
    pub(crate) fn new(session_id: &str) -> Self {
        let file = crate::config::config_dir().and_then(|dir| {
            std::fs::create_dir_all(&dir).ok()?;
            let path = dir.join("telegram-bridge.log");
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .ok()
        });

        if let Some(ref f) = file {
            let path = f.metadata().ok();
            log::info!(
                "Bridge logger active for session {} ({} bytes)",
                session_id,
                path.map(|m| m.len()).unwrap_or(0)
            );
        }

        Self { file }
    }

    pub(crate) fn log(&mut self, direction: &str, session_id: &str, text: &str) {
        if let Some(ref mut f) = self.file {
            let now = chrono::Utc::now().format("%H:%M:%S%.3f");
            // #280 G-MED-3 / LOW-1 — telegram-bridge.log is outside
            // env_logger's sink scrub. Redact BEFORE truncating so the 500-
            // byte cut cannot land inside a token tail and leak a partial
            // secret (the redactor's 10-char floor would refuse to match
            // the post-truncate stub). Redaction only shrinks the string
            // (replaces secrets with `***`), so truncate-after-redact never
            // grows past 500 bytes.
            let text_redacted = crate::telegram::redact::redact(text);
            let preview = if text_redacted.len() > 500 {
                let mut end = 500;
                while !text_redacted.is_char_boundary(end) {
                    end -= 1;
                }
                format!(
                    "{}...[{}b total]",
                    &text_redacted[..end],
                    text_redacted.len()
                )
            } else {
                text_redacted
            };
            let _ = writeln!(
                f,
                "[{}] {} sid={} | {}",
                now, direction, session_id, preview
            );
            let _ = f.flush();
        }
    }
}

// ── Diagnostic logger (full capture, no truncation) ─────────

pub(crate) struct DiagLogger {
    raw_file: Option<std::fs::File>,
    sent_file: Option<std::fs::File>,
}

impl DiagLogger {
    pub(crate) fn new() -> Self {
        let dir = crate::config::config_dir();

        let open = |name: &str| -> Option<std::fs::File> {
            let dir = dir.as_ref()?;
            std::fs::create_dir_all(dir).ok()?;
            let path = dir.join(name);
            std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&path)
                .ok()
        };

        let raw_file = open("diag-raw.log");
        let sent_file = open("diag-sent.log");

        if raw_file.is_some() && sent_file.is_some() {
            log::info!("Diagnostic logger active: diag-raw.log + diag-sent.log");
        }

        Self {
            raw_file,
            sent_file,
        }
    }

    /// Log stabilized rows (post-stabilization, pre-agent-filter)
    pub(crate) fn log_raw(&mut self, text: &str) {
        if let Some(ref mut f) = self.raw_file {
            let now = chrono::Utc::now().format("%H:%M:%S%.3f");
            // #280 G-MED-3 — diag-raw.log lives outside env_logger's sink
            // scrub. Scrub here so secret-bearing content (agent output
            // containing a Telegram URL, or any future call site that
            // bypassed api.rs) cannot land in the diag log.
            let text = crate::telegram::redact::redact(text);
            let _ = writeln!(f, "--- [{}] ---", now);
            let _ = writeln!(f, "{}", text);
            let _ = f.flush();
        }
    }

    /// Log what actually gets sent to Telegram
    pub(crate) fn log_sent(&mut self, text: &str) {
        if let Some(ref mut f) = self.sent_file {
            let now = chrono::Utc::now().format("%H:%M:%S%.3f");
            // #280 G-MED-3 — same scrub as log_raw; diag-sent.log bypasses
            // env_logger.
            let text = crate::telegram::redact::redact(text);
            let _ = writeln!(f, "--- [{}] ---", now);
            let _ = writeln!(f, "{}", text);
            let _ = f.flush();
        }
    }
}

// ── Flush to Telegram ────────────────────────────────────────

// Threads through the full bridge state on each flush; collapsing into a
// struct would only move the fields, not reduce them.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn flush_buffer<R: tauri::Runtime>(
    buffer: &mut String,
    network: &OutboundNetwork,
    token: &str,
    chat_id: i64,
    session_id: &str,
    app: &tauri::AppHandle<R>,
    logger: &mut BridgeLogger,
    diag: &mut DiagLogger,
    skip_dedup: bool,
) {
    let text = std::mem::take(buffer);
    // Deduplicate consecutive identical lines (PTY mode only — screen redraws cause duplicates).
    // JSONL mode skips dedup because content is clean and legitimate repeated lines are valid.
    let text = if skip_dedup {
        text
    } else {
        let mut lines: Vec<&str> = Vec::new();
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if lines.last().map(|l: &&str| l.trim()) != Some(trimmed) {
                lines.push(line);
            }
        }
        lines.join("\n")
    };
    let text = text.trim().to_string();
    if text.is_empty() {
        return;
    }

    for chunk in chunk_text(&text, 4000) {
        logger.log("SEND_TG", session_id, &chunk);
        diag.log_sent(&chunk);

        if let Err(e) = api::send_message(network, token, chat_id, &chunk).await {
            // #280 — defense-in-depth scrub before `msg` reaches the
            // `telegram_bridge_error` Tauri event payload (which bypasses
            // the env_logger format closure that protects stderr/app.log).
            // The reqwest error sites inside api.rs already wrap with
            // `redact`, but body.description / future call sites could
            // construct an `AppError::Telegram` carrying an unscrubbed
            // URL. Idempotent on already-redacted strings; near-zero cost
            // when no secret marker is present.
            let msg = crate::telegram::redact::redact(&e.to_string());
            let kind = TelegramErrKind::classify(&msg);
            let pid = std::process::id();
            // `token_prefix` is the numeric bot user id (e.g. "8336197840"),
            // not the secret. Telegram bot user ids are public via
            // @BotInfoBot — they're identifiers, useful for correlating
            // multi-bot deployments. The secret is the `:AAGB…` tail.
            let token_prefix = token.split(':').next().unwrap_or("?");
            logger.log("SEND_ERR", session_id, &msg);
            log::error!(
                "[bridge] Telegram send failed — kind={} session_id={} pid={} bot_id={} err={}",
                kind.as_str(),
                session_id,
                pid,
                token_prefix,
                msg
            );
            let _ = app.emit(
                "telegram_bridge_error",
                serde_json::json!({
                    "sessionId": session_id,
                    "error": msg,
                    "kind": kind.as_str(),
                }),
            );
        }
        // Rate limit: 35ms between sends
        tokio::time::sleep(Duration::from_millis(35)).await;
    }
}

fn chunk_text(text: &str, max_len: usize) -> Vec<String> {
    if text.len() <= max_len {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let mut end = (start + max_len).min(text.len());
        // Snap backward to nearest char boundary to avoid UTF-8 slicing panic
        while end > start && !text.is_char_boundary(end) {
            end -= 1;
        }
        let actual_end = if end < text.len() {
            text[start..end]
                .rfind('\n')
                .map(|i| start + i + 1)
                .unwrap_or(end)
        } else {
            end
        };
        chunks.push(text[start..actual_end].to_string());
        start = actual_end;
    }
    chunks
}

#[cfg(test)]
mod tests {
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
            "Gemini call: https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:generateContent?key={}",
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
}
