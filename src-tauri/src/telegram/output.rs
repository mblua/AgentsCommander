use tokio::time::Duration;

use crate::config::instance_artifacts::{
    TELEGRAM_BRIDGE_LOG_FILE_NAME, TELEGRAM_DIAG_RAW_LOG_FILE_NAME,
    TELEGRAM_DIAG_SENT_LOG_FILE_NAME,
};
use crate::filesystem::telegram_logs::{
    ensure_telegram_log_directory, flush_telegram_log, open_telegram_bridge_log,
    open_telegram_diagnostic_log, telegram_log_metadata, write_telegram_log, TelegramLogFile,
};
use crate::network::OutboundNetwork;
use crate::telegram::api;

pub(crate) trait TelegramEvents: Send + Sync {
    fn bridge_error(&self, session_id: &str, error: &str, kind: Option<&str>);
}

fn report_send_error(
    events: &dyn TelegramEvents,
    session_id: &str,
    msg: &str,
    kind: TelegramErrKind,
) {
    events.bridge_error(session_id, msg, Some(kind.as_str()));
}

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
    file: Option<TelegramLogFile>,
}

impl BridgeLogger {
    pub(crate) fn new(session_id: &str) -> Self {
        let file = crate::config::config_dir().and_then(|dir| {
            ensure_telegram_log_directory(&dir).ok()?;
            let path = dir.join(TELEGRAM_BRIDGE_LOG_FILE_NAME);
            open_telegram_bridge_log(&path).ok()
        });

        if let Some(ref f) = file {
            let path = telegram_log_metadata(f).ok();
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
            let _ = write_telegram_log(
                f,
                format_args!("[{}] {} sid={} | {}\n", now, direction, session_id, preview),
            );
            let _ = flush_telegram_log(f);
        }
    }
}

// ── Diagnostic logger (full capture, no truncation) ─────────

pub(crate) struct DiagLogger {
    raw_file: Option<TelegramLogFile>,
    sent_file: Option<TelegramLogFile>,
}

impl DiagLogger {
    pub(crate) fn new() -> Self {
        let dir = crate::config::config_dir();

        let open = |name: &str| -> Option<TelegramLogFile> {
            let dir = dir.as_ref()?;
            ensure_telegram_log_directory(dir).ok()?;
            let path = dir.join(name);
            open_telegram_diagnostic_log(&path).ok()
        };

        let raw_file = open(TELEGRAM_DIAG_RAW_LOG_FILE_NAME);
        let sent_file = open(TELEGRAM_DIAG_SENT_LOG_FILE_NAME);

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
            let _ = write_telegram_log(f, format_args!("--- [{}] ---\n", now));
            let _ = write_telegram_log(f, format_args!("{}\n", text));
            let _ = flush_telegram_log(f);
        }
    }

    /// Log what actually gets sent to Telegram
    pub(crate) fn log_sent(&mut self, text: &str) {
        if let Some(ref mut f) = self.sent_file {
            let now = chrono::Utc::now().format("%H:%M:%S%.3f");
            // #280 G-MED-3 — same scrub as log_raw; diag-sent.log bypasses
            // env_logger.
            let text = crate::telegram::redact::redact(text);
            let _ = write_telegram_log(f, format_args!("--- [{}] ---\n", now));
            let _ = write_telegram_log(f, format_args!("{}\n", text));
            let _ = flush_telegram_log(f);
        }
    }
}

// ── Flush to Telegram ────────────────────────────────────────

// Threads through the full bridge state on each flush; collapsing into a
// struct would only move the fields, not reduce them.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn flush_buffer(
    buffer: &mut String,
    network: &OutboundNetwork,
    token: &str,
    chat_id: i64,
    session_id: &str,
    events: &dyn TelegramEvents,
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
            report_send_error(events, session_id, &msg, kind);
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
mod tests;
