// Claude Code JSONL session-file watcher.
// Polls Claude Code's append-only structured session log for new assistant
// messages and sends them to Telegram, bypassing the PTY-based pipeline.
//
// Shared scaffold (find_latest_jsonl, read_new_lines, polling/rotation
// constants) lives in `jsonl_kernel.rs` — see commit 1 for the extraction.

use std::path::PathBuf;
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use tauri::Emitter;
use tokio::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

use crate::network::OutboundNetwork;
use crate::telegram::output::{prepare_output_chunks, TelegramErrKind};

struct WatcherLogger {
    file: Option<std::fs::File>,
}

impl WatcherLogger {
    fn new(session_id: &str) -> Self {
        let file = crate::config::config_dir().and_then(|dir| {
            std::fs::create_dir_all(&dir).ok()?;
            let path = dir.join("telegram-bridge.log");
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .ok()
        });

        if let Some(ref file) = file {
            let metadata = file.metadata().ok();
            log::info!(
                "Bridge logger active for session {} ({} bytes)",
                session_id,
                metadata.map(|metadata| metadata.len()).unwrap_or(0)
            );
        }

        Self { file }
    }

    fn log(&mut self, direction: &str, session_id: &str, text: &str) {
        if let Some(ref mut file) = self.file {
            let now = chrono::Utc::now().format("%H:%M:%S%.3f");
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
            let _ = std::io::Write::write_fmt(
                file,
                format_args!("[{}] {} sid={} | {}\n", now, direction, session_id, preview),
            );
            let _ = std::io::Write::flush(file);
        }
    }
}

struct WatcherDiagLogger {
    #[allow(dead_code)]
    raw_file: Option<std::fs::File>,
    sent_file: Option<std::fs::File>,
}

impl WatcherDiagLogger {
    fn new() -> Self {
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

    #[allow(dead_code)]
    fn log_raw(&mut self, text: &str) {
        if let Some(ref mut file) = self.raw_file {
            let now = chrono::Utc::now().format("%H:%M:%S%.3f");
            let text = crate::telegram::redact::redact(text);
            let _ = std::io::Write::write_fmt(file, format_args!("--- [{}] ---\n", now));
            let _ = std::io::Write::write_fmt(file, format_args!("{}\n", text));
            let _ = std::io::Write::flush(file);
        }
    }

    fn log_sent(&mut self, text: &str) {
        if let Some(ref mut file) = self.sent_file {
            let now = chrono::Utc::now().format("%H:%M:%S%.3f");
            let text = crate::telegram::redact::redact(text);
            let _ = std::io::Write::write_fmt(file, format_args!("--- [{}] ---\n", now));
            let _ = std::io::Write::write_fmt(file, format_args!("{}\n", text));
            let _ = std::io::Write::flush(file);
        }
    }
}

#[cfg(test)]
mod watcher_output_ownership_tests {
    use super::{WatcherDiagLogger, WatcherLogger};

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

    #[test]
    fn watcherlogger_log_redacts_telegram_token_in_text() {
        let temp_dir = tempfile::tempdir().expect("tmp");
        let path = temp_dir.path().join("telegram-bridge.log");
        let file = open_temp_log(temp_dir.path(), "telegram-bridge.log");
        let mut logger = WatcherLogger { file: Some(file) };
        let leak_shaped = format!(
            "error sending request for url (https://api.telegram.org/bot{}/getUpdates?offset=0)",
            FAKE_TG_TOKEN
        );
        logger.log("ERR", "sid-test", &leak_shaped);
        drop(logger);

        let content = std::fs::read_to_string(&path).expect("read log back");
        assert!(content.contains("/bot***/getUpdates"));
        assert!(!content.contains(FAKE_TG_TOKEN));
    }

    #[test]
    fn watcherlogger_log_redacts_before_truncating_at_500_bytes() {
        let temp_dir = tempfile::tempdir().expect("tmp");
        let path = temp_dir.path().join("telegram-bridge.log");
        let file = open_temp_log(temp_dir.path(), "telegram-bridge.log");
        let mut logger = WatcherLogger { file: Some(file) };
        let padding = "a".repeat(481);
        let text = format!("{padding}/bot{FAKE_TG_TOKEN}/getUpdates");
        assert!(text.len() > 500);

        logger.log("ERR", "sid-test", &text);
        drop(logger);

        let content = std::fs::read_to_string(&path).expect("read log back");
        assert!(content.contains("/bot***"));
        assert!(!content.contains(FAKE_TG_TOKEN));
        assert!(!content.contains("FAKE_"));
        assert!(!content.contains("FAKE_TOKEN_FOR_TESTING"));
    }

    #[test]
    fn watcherdiaglogger_log_raw_redacts_telegram_token() {
        let temp_dir = tempfile::tempdir().expect("tmp");
        let raw_path = temp_dir.path().join("diag-raw.log");
        let raw = open_temp_log(temp_dir.path(), "diag-raw.log");
        let mut logger = WatcherDiagLogger {
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
        assert!(content.contains("/bot***/sendMessage"));
        assert!(!content.contains(FAKE_TG_TOKEN));
    }

    #[test]
    fn watcherdiaglogger_log_sent_redacts_gemini_key() {
        let temp_dir = tempfile::tempdir().expect("tmp");
        let sent_path = temp_dir.path().join("diag-sent.log");
        let sent = open_temp_log(temp_dir.path(), "diag-sent.log");
        let mut logger = WatcherDiagLogger {
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
        assert!(content.contains("?key=***"));
        assert!(!content.contains(FAKE_GEMINI_KEY));
    }
}

#[allow(clippy::too_many_arguments)]
async fn flush_watcher_buffer<R: tauri::Runtime>(
    buffer: &mut String,
    network: &OutboundNetwork,
    token: &str,
    chat_id: i64,
    session_id: &str,
    app: &tauri::AppHandle<R>,
    logger: &mut WatcherLogger,
    diag: &mut WatcherDiagLogger,
    skip_dedup: bool,
) {
    for chunk in prepare_output_chunks(buffer, skip_dedup) {
        logger.log("SEND_TG", session_id, &chunk);
        diag.log_sent(&chunk);

        if let Err(error) = crate::telegram::api::send_message(network, token, chat_id, &chunk).await {
            let message = crate::telegram::redact::redact(&error.to_string());
            let kind = TelegramErrKind::classify(&message);
            let process_id = std::process::id();
            let token_prefix = token.split(':').next().unwrap_or("?");
            logger.log("SEND_ERR", session_id, &message);
            log::error!(
                "[bridge] Telegram send failed - kind={} session_id={} pid={} bot_id={} err={}",
                kind.as_str(),
                session_id,
                process_id,
                token_prefix,
                message
            );
            let _ = tauri::Emitter::emit(
                app,
                "telegram_bridge_error",
                serde_json::json!({
                    "sessionId": session_id,
                    "error": message,
                    "kind": kind.as_str(),
                }),
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(35)).await;
    }
}
use crate::telegram::jsonl_kernel::{
    find_latest_jsonl, read_new_lines, read_preamble_for_race, POLL_INTERVAL_MS,
    ROTATION_STALE_SECS,
};

const FLUSH_DELAY_MS: u64 = 500;

/// Spawn a JSONL file watcher task that polls for new assistant messages
/// and sends them to Telegram via the shared buffer/send pipeline.
///
/// `project_dir` must be the already-resolved Claude `projects/<mangled-cwd>`
/// directory (callers resolve via `commands::session::resolve_claude_projects_dir`
/// so wrapper-driven `CLAUDE_CONFIG_DIR` overrides like `claude-mb` are honored).
pub fn spawn_watch_task<R: tauri::Runtime>(
    project_dir: PathBuf,
    network: OutboundNetwork,
    bot_token: String,
    chat_id: i64,
    session_id: String,
    cancel: CancellationToken,
    app: tauri::AppHandle<R>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        watch_loop(
            project_dir,
            network,
            bot_token,
            chat_id,
            session_id.clone(),
            cancel,
            app.clone(),
        )
        .await;
        log::info!("[JSONL_EXIT] Watcher task ended for session {}", session_id);
    })
}

/// Extractor for `read_preamble_for_race`: pairs each emitted body with the
/// line's `timestamp` field so the kernel can apply its grace-window filter.
/// Claude does not dedupe by id (idempotent assistant turns are absent in the
/// JSONL format), so the id slot is always `None`.
fn claude_preamble_extractor(line: &str) -> Option<(DateTime<Utc>, Option<String>, String)> {
    let body = extract_assistant_text(line)?;
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let ts_str = v.get("timestamp")?.as_str()?;
    let ts = DateTime::parse_from_rfc3339(ts_str)
        .ok()?
        .with_timezone(&Utc);
    Some((ts, None, body))
}

/// Parse a single JSONL line and extract assistant text content.
/// Returns None for non-assistant messages, tool_use blocks, thinking blocks, etc.
fn extract_assistant_text(line: &str) -> Option<String> {
    // G6 fast-path: skip lines that can't be assistant messages (avoids multi-MB JSON parses)
    if !line.contains("\"type\":\"assistant\"") && !line.contains("\"type\": \"assistant\"") {
        return None;
    }

    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("type")?.as_str()? != "assistant" {
        return None;
    }

    let content = v.get("message")?.get("content")?;

    match content {
        serde_json::Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        serde_json::Value::Array(arr) => {
            let mut texts = Vec::new();
            for block in arr {
                // G4: whitelist "text" only — filters tool_use, tool_result, thinking, and future types
                if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                    if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                        let trimmed = text.trim();
                        if !trimmed.is_empty() {
                            texts.push(trimmed.to_string());
                        }
                    }
                }
            }
            if texts.is_empty() {
                None
            } else {
                Some(texts.join("\n"))
            }
        }
        _ => None,
    }
}

async fn watch_loop<R: tauri::Runtime>(
    project_dir: PathBuf,
    network: OutboundNetwork,
    token: String,
    chat_id: i64,
    session_id: String,
    cancel: CancellationToken,
    app: tauri::AppHandle<R>,
) {
    let mut logger = WatcherLogger::new(&session_id);
    let mut diag = WatcherDiagLogger::new();
    let mut buffer = String::new();
    let mut last_buffer_add = Instant::now();
    let flush_delay = Duration::from_millis(FLUSH_DELAY_MS);

    let attach_time: DateTime<Utc> = Utc::now();
    let mut current_file: Option<PathBuf> = None;
    let mut current_file_mtime: Option<SystemTime> = None;
    let mut file_offset: u64 = 0;
    let mut line_remainder = String::new();
    let mut dir_warned = false;

    logger.log(
        "JSONL_INIT",
        &session_id,
        &format!("project_dir={}", project_dir.display()),
    );

    let mut poll_interval = tokio::time::interval(Duration::from_millis(POLL_INTERVAL_MS));
    poll_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = poll_interval.tick() => {
                // Check if project directory exists yet
                if !project_dir.is_dir() {
                    if !dir_warned {
                        logger.log("JSONL_WAIT", &session_id, "project directory does not exist yet");
                        dir_warned = true;
                    }
                    continue;
                }
                if dir_warned {
                    logger.log("JSONL_INIT", &session_id, "project directory appeared");
                    dir_warned = false;
                }

                let latest = find_latest_jsonl(&project_dir);

                // Handle file rotation with flicker guard
                if latest != current_file {
                    let should_switch = match (&current_file, &current_file_mtime) {
                        (Some(_), Some(mtime)) => {
                            // Only switch if current file is stale
                            mtime.elapsed()
                                .map(|d| d.as_secs() >= ROTATION_STALE_SECS)
                                .unwrap_or(true)
                        }
                        _ => true, // No current file — always accept
                    };

                    if should_switch {
                        if current_file.is_none() {
                            // First attach (§J preamble scan): emit recent
                            // lines from the file's tail, then set offset = file_len.
                            if let Some(ref p) = latest {
                                match read_preamble_for_race(p, attach_time, claude_preamble_extractor) {
                                    Ok((bodies, _ids, file_len)) => {
                                        for text in bodies {
                                            logger.log("JSONL_PREAMBLE", &session_id, &text);
                                            buffer.push_str(&text);
                                            buffer.push('\n');
                                            last_buffer_add = Instant::now();
                                        }
                                        file_offset = file_len;
                                        logger.log("JSONL_FILE", &session_id,
                                            &format!("initial file, preamble scan done, offset={}", file_offset));
                                    }
                                    Err(e) => {
                                        logger.log("JSONL_ERR", &session_id,
                                            &format!("preamble scan failed: {}", e));
                                        file_offset = std::fs::metadata(p).ok()
                                            .map(|m| m.len())
                                            .unwrap_or(0);
                                    }
                                }
                            } else {
                                file_offset = 0;
                            }
                        } else {
                            // File rotation (new Claude session): read from start
                            file_offset = 0;
                            logger.log("JSONL_ROTATE", &session_id,
                                &format!("new file: {:?}", latest));
                        }
                        current_file = latest;
                        current_file_mtime = current_file.as_ref()
                            .and_then(|p| std::fs::metadata(p).ok())
                            .and_then(|m| m.modified().ok());
                        line_remainder.clear();
                    }
                }

                if let Some(ref path) = current_file {
                    match read_new_lines(path, &mut file_offset, &mut line_remainder) {
                        Ok(new_lines) => {
                            for line in new_lines {
                                if let Some(text) = extract_assistant_text(&line) {
                                    logger.log("JSONL_EXTRACT", &session_id, &text);
                                    buffer.push_str(&text);
                                    buffer.push('\n');
                                    last_buffer_add = Instant::now();
                                }
                            }

                            // Update mtime for rotation flicker guard
                            current_file_mtime = std::fs::metadata(path).ok()
                                .and_then(|m| m.modified().ok());
                        }
                        Err(e) => {
                            // G5: Emit bridge error event for file I/O failures
                            logger.log("JSONL_ERR", &session_id, &e.to_string());
                            log::error!("[JSONL_ERR] Read error for session {}: {}", session_id, e);
                            let _ = app.emit(
                                "telegram_bridge_error",
                                serde_json::json!({
                                    "sessionId": session_id,
                                    "error": format!("JSONL read error: {}", e),
                                }),
                            );
                        }
                    }
                }

                // Flush buffer if enough time has passed since last addition
                if !buffer.is_empty() {
                    let elapsed = last_buffer_add.elapsed();
                    if elapsed >= flush_delay || buffer.len() > 2000 {
                        flush_watcher_buffer(
                            &mut buffer, &network, &token, chat_id,
                            &session_id, &app, &mut logger, &mut diag,
                            true, // skip_dedup: JSONL text is clean, repeated lines are legitimate
                        ).await;
                    }
                }
            }
        }
    }

    // G1: Final poll + flush after cancel (don't lose buffered content)
    if let Some(ref path) = current_file {
        if let Ok(new_lines) = read_new_lines(path, &mut file_offset, &mut line_remainder) {
            for line in new_lines {
                if let Some(text) = extract_assistant_text(&line) {
                    buffer.push_str(&text);
                    buffer.push('\n');
                }
            }
        }
    }
    if !buffer.is_empty() {
        flush_watcher_buffer(
            &mut buffer,
            &network,
            &token,
            chat_id,
            &session_id,
            &app,
            &mut logger,
            &mut diag,
            true,
        )
        .await;
    }
}
