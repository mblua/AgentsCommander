// JSONL file watcher for Claude Code sessions.
// Polls Claude Code's structured session log files for new assistant messages
// and sends them to Telegram, bypassing the PTY-based pipeline entirely.

use std::io::{Read as IoRead, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use tauri::Emitter;
use tokio::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

use crate::session::session::mangle_cwd_for_claude;
use crate::telegram::bridge::{flush_buffer, BridgeLogger, DiagLogger};

const POLL_INTERVAL_MS: u64 = 500;
const FLUSH_DELAY_MS: u64 = 500;
/// Duration a tracked file must be stale before switching to a newer one (file rotation guard)
const ROTATION_STALE_SECS: u64 = 3;

/// Spawn a JSONL file watcher task that polls for new assistant messages
/// and sends them to Telegram via the shared buffer/send pipeline.
pub fn spawn_watch_task(
    cwd: String,
    bot_token: String,
    chat_id: i64,
    session_id: String,
    cancel: CancellationToken,
    app: tauri::AppHandle,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        watch_loop(cwd, bot_token, chat_id, session_id.clone(), cancel, app.clone()).await;
        log::info!("[JSONL_EXIT] Watcher task ended for session {}", session_id);
    })
}

/// Find the most recently modified .jsonl file in the project directory.
fn find_latest_jsonl(project_dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(project_dir).ok()?;
    let mut best: Option<(PathBuf, SystemTime)> = None;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        if let Ok(meta) = entry.metadata() {
            if let Ok(modified) = meta.modified() {
                match &best {
                    Some((_, best_time)) if modified > *best_time => {
                        best = Some((path, modified));
                    }
                    None => {
                        best = Some((path, modified));
                    }
                    _ => {}
                }
            }
        }
    }

    best.map(|(p, _)| p)
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
            if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
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
            if texts.is_empty() { None } else { Some(texts.join("\n")) }
        }
        _ => None,
    }
}

/// Read new bytes from a file starting at the given byte offset.
/// Returns parsed complete lines and updates the offset by actual bytes read.
/// Partial lines are accumulated in `remainder` for the next poll.
fn read_new_lines(
    path: &Path,
    offset: &mut u64,
    remainder: &mut String,
) -> std::io::Result<Vec<String>> {
    let mut file = std::fs::File::open(path)?;
    // Use metadata on the open handle (avoids TOCTOU with path-based metadata)
    let file_len = file.metadata()?.len();

    // G3: File truncation/shrink detection — reset to beginning
    if file_len < *offset {
        log::warn!(
            "[JSONL_TRUNCATE] File shrank ({} < {}), resetting offset",
            file_len,
            *offset
        );
        *offset = 0;
        remainder.clear();
    }

    if file_len <= *offset {
        return Ok(vec![]);
    }

    file.seek(SeekFrom::Start(*offset))?;
    let mut buf = String::new();
    file.read_to_string(&mut buf)?;

    // G2: Track offset by actual bytes read, not reported file length
    *offset += buf.len() as u64;

    // Prepend any partial line from previous read
    if !remainder.is_empty() {
        let mut combined = std::mem::take(remainder);
        combined.push_str(&buf);
        buf = combined;
    }

    let mut lines = Vec::new();
    let mut last_newline = 0;

    for (i, ch) in buf.char_indices() {
        if ch == '\n' {
            let line = &buf[last_newline..i];
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                lines.push(trimmed.to_string());
            }
            last_newline = i + 1;
        }
    }

    // Keep unterminated tail in remainder for next poll
    if last_newline < buf.len() {
        *remainder = buf[last_newline..].to_string();
    }

    Ok(lines)
}

async fn watch_loop(
    cwd: String,
    token: String,
    chat_id: i64,
    session_id: String,
    cancel: CancellationToken,
    app: tauri::AppHandle,
) {
    let project_dir = match dirs::home_dir() {
        Some(home) => home.join(".claude").join("projects").join(mangle_cwd_for_claude(&cwd)),
        None => {
            log::error!("[JSONL_ERR] Cannot resolve home directory — JSONL watcher dormant");
            // Stay alive but dormant until cancelled
            cancel.cancelled().await;
            return;
        }
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_default();

    let mut logger = BridgeLogger::new(&session_id);
    let mut diag = DiagLogger::new();
    let mut buffer = String::new();
    let mut last_buffer_add = Instant::now();
    let flush_delay = Duration::from_millis(FLUSH_DELAY_MS);

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
                            // First attach: skip existing content (seek to end)
                            file_offset = latest.as_ref()
                                .and_then(|p| std::fs::metadata(p).ok())
                                .map(|m| m.len())
                                .unwrap_or(0);
                            logger.log("JSONL_FILE", &session_id,
                                &format!("initial file, skipping to offset {}", file_offset));
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
                        flush_buffer(
                            &mut buffer, &client, &token, chat_id,
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
        flush_buffer(
            &mut buffer, &client, &token, chat_id,
            &session_id, &app, &mut logger, &mut diag,
            true,
        )
        .await;
    }
}
