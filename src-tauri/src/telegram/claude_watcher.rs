// Claude Code JSONL session-file watcher.
// Polls Claude Code's append-only structured session log for new assistant
// messages and sends them to Telegram, bypassing the PTY-based pipeline.
//
// Shared scaffold (find_latest_jsonl, read_new_lines, polling/rotation
// constants) lives in `jsonl_kernel.rs` — see commit 1 for the extraction.

use std::io::{Read as IoRead, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use tauri::Emitter;
use tokio::sync::mpsc::UnboundedSender;
use tokio::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

use crate::capture::key::{ReaderAttachment, ReaderObservations};
use crate::capture::record::{CaptureProvider, CapturedRecord, RecordOrigin};
use crate::capture::state::head_from_lines;
use crate::network::OutboundNetwork;
use crate::telegram::jsonl_kernel::{
    find_latest_jsonl, read_new_lines_with_starts, read_preamble_for_race, POLL_INTERVAL_MS,
    PREAMBLE_MAX_BYTES, RACE_GRACE_SECS, ROTATION_STALE_SECS,
};
use crate::telegram::output::{flush_buffer, BridgeLogger, DiagLogger};

const FLUSH_DELAY_MS: u64 = 500;

/// Spawn a JSONL file watcher task that polls for new assistant messages
/// and sends them to Telegram via the shared buffer/send pipeline.
///
/// `project_dir` must be the already-resolved Claude `projects/<mangled-cwd>`
/// directory (callers resolve via `commands::session::resolve_claude_projects_dir`
/// so wrapper-driven `CLAUDE_CONFIG_DIR` overrides like `claude-mb` are honored).
///
/// `dest` carries the Telegram destination, consulted **only** when it changes
/// — at attach and at detach (#2232 phase 4 section 6). `None` is a room-only
/// reader: no logger is built, nothing is sent, and the records still reach
/// `sink`. `reanchor` is bumped by the supervisor on a session restart; the
/// demand set is untouched and only the transcript **file** is re-resolved
/// (section 5.2).
#[allow(clippy::too_many_arguments)]
pub fn spawn_watch_task<R: tauri::Runtime>(
    project_dir: PathBuf,
    network: OutboundNetwork,
    dest: tokio::sync::watch::Receiver<Option<BotTarget>>,
    reanchor: tokio::sync::watch::Receiver<u64>,
    session_id: String,
    cancel: CancellationToken,
    app: tauri::AppHandle<R>,
    sink: Option<UnboundedSender<Arc<CapturedRecord>>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        watch_loop(
            project_dir,
            network,
            dest,
            reanchor,
            session_id.clone(),
            cancel,
            app.clone(),
            sink,
        )
        .await;
        log::info!("[JSONL_EXIT] Watcher task ended for session {}", session_id);
    })
}

/// Where a reader sends Telegram messages (#2232 phase 4 section 6).
///
/// Deliberately **not** shared with `codex_watcher`, which declares its own:
/// section 10 forbids either watcher gaining a reference the other does not
/// already have, and `claude_watcher_layering` equality-pins this module's
/// dependency set. Two three-line structs are cheaper than an arc between the
/// watchers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BotTarget {
    pub token: String,
    pub chat_id: i64,
}

/// The §6 live-attach preamble window of `path`, as `(start, body)` pairs.
///
/// **Starts are computed on the raw bytes plus the window offset**, never on
/// the decoded text: lossy decoding changes lengths and the line iterator
/// already drops the carriage return. `raw[start..]` therefore walks back to
/// the exact bytes in the file, which is what lets the caller drop every line
/// at or above the reader's current offset — those the normal loop will send.
fn read_preamble_with_starts(
    path: &Path,
    attach_time: DateTime<Utc>,
) -> std::io::Result<Vec<(u64, String)>> {
    let initial_len = std::fs::metadata(path)?.len();
    let window_start = initial_len.saturating_sub(PREAMBLE_MAX_BYTES);
    let mut file = std::fs::File::open(path)?;
    file.seek(SeekFrom::Start(window_start))?;
    let mut buf: Vec<u8> = Vec::new();
    file.read_to_end(&mut buf)?;

    // Drop everything before the first `\n` unless the window starts at 0: the
    // seek point almost certainly lands mid-line. `base` is the absolute file
    // offset of the first byte of `bytes`.
    let (bytes, base): (&[u8], u64) = if window_start == 0 {
        (&buf, 0)
    } else {
        match buf.iter().position(|&b| b == b'\n') {
            Some(i) => (&buf[i + 1..], window_start + i as u64 + 1),
            // A single line larger than the window: nothing can be anchored.
            None => return Ok(Vec::new()),
        }
    };

    let cutoff = attach_time - chrono::Duration::seconds(RACE_GRACE_SECS);
    let mut out = Vec::new();
    let mut cursor: usize = 0;
    for raw in bytes.split_inclusive(|&b| b == b'\n') {
        let line_start = base + cursor as u64;
        cursor += raw.len();
        let decoded = String::from_utf8_lossy(raw);
        let line = decoded.trim_end_matches('\n').trim_end_matches('\r');
        if let Some((ts, _id, body)) = claude_preamble_extractor(line) {
            if ts >= cutoff {
                out.push((line_start, body));
            }
        }
    }
    Ok(out)
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

/// Parse a single JSONL line and extract the assistant text plus its turn id.
///
/// Claude's JSONL format carries no turn id (`claude_watcher.rs:58` of the
/// planning base documents exactly this), so the id slot is `None` by
/// construction. Returns None for non-assistant messages, tool_use blocks,
/// thinking blocks, etc.
fn extract_assistant_text_with_turn(line: &str) -> Option<(String, Option<String>)> {
    // G6 fast-path: skip lines that can't be assistant messages (avoids multi-MB JSON parses)
    if !line.contains("\"type\":\"assistant\"") && !line.contains("\"type\": \"assistant\"") {
        return None;
    }

    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("type")?.as_str()? != "assistant" {
        return None;
    }

    let content = v.get("message")?.get("content")?;

    let text = match content {
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
    }?;

    Some((text, None))
}

/// Text-only view of [`extract_assistant_text_with_turn`].
fn extract_assistant_text(line: &str) -> Option<String> {
    extract_assistant_text_with_turn(line).map(|(text, _)| text)
}

/// Build one [`CapturedRecord`] for an accepted assistant record.
///
/// `reader_seq` is a per-reader counter starting at 0: the record keeps the
/// value it was built with and the counter advances once per produced record.
/// `attach` carries the reader's epoch and its observation of the file
/// (#2232 phase 3); phase 1 hardcoded `epoch: 0` and had nowhere to put the
/// observation.
// Eight parameters: phase 1's seven plus the attachment. Grouping them into a
// builder would hide which of them the record copies verbatim, which is the one
// thing the phase-1 tests read this function for.
#[allow(clippy::too_many_arguments)]
fn capture_record(
    text: String,
    turn_id: Option<String>,
    record_start: Option<u64>,
    origin: RecordOrigin,
    session_id: &str,
    file: &Path,
    reader_seq: &mut u64,
    attach: &ReaderAttachment,
) -> Arc<CapturedRecord> {
    let turn_identified = turn_id.is_some();
    let text_sha256: [u8; 32] = Sha256::digest(text.as_bytes()).into();
    let record = Arc::new(CapturedRecord {
        session_id: session_id.to_owned(),
        text,
        file: file.to_path_buf(),
        epoch: attach.epoch,
        record_start,
        reader_seq: *reader_seq,
        text_sha256,
        turn_id,
        provider: CaptureProvider::Claude,
        // Claude's bits are the permanent shape (epic.md 3.2/3.3): the provider
        // carries no final marker and the `Stop` hook is out of scope.
        provider_final: false,
        turn_identified,
        origin,
        observed_path: attach.observed_path.clone(),
        observed_len: attach.observed_len,
        observed_prefix: attach.observed_prefix.clone(),
    });
    *reader_seq += 1;
    record
}

/// Deliver `record` to the capture sink when one is attached.
///
/// The send result is deliberately discarded: a closed or absent receiver must
/// never fail the watcher loop, and a slow consumer must never stall Telegram
/// (the channel is unbounded). Every caller leaves `sender` `None` in this
/// phase.
fn deliver_capture_record(
    sender: Option<&UnboundedSender<Arc<CapturedRecord>>>,
    record: &Arc<CapturedRecord>,
) {
    if let Some(sender) = sender {
        let _ = sender.send(Arc::clone(record));
    }
}

/// Capture and deliver every accepted record among `new_lines`, in order.
///
/// The caller appends `record.text` and a newline to the Telegram buffer,
/// exactly as the pre-#2232 code appended the extractor text.
fn capture_live_lines(
    new_lines: Vec<(u64, String)>,
    session_id: &str,
    file: &Path,
    reader_seq: &mut u64,
    sender: Option<&UnboundedSender<Arc<CapturedRecord>>>,
    origin: RecordOrigin,
    attach: &ReaderAttachment,
) -> Vec<Arc<CapturedRecord>> {
    let mut records = Vec::new();
    for (record_start, line) in new_lines {
        let Some((text, turn_id)) = extract_assistant_text_with_turn(&line) else {
            continue;
        };
        let record = capture_record(
            text,
            turn_id,
            Some(record_start),
            origin,
            session_id,
            file,
            reader_seq,
            attach,
        );
        deliver_capture_record(sender, &record);
        records.push(record);
    }
    records
}

/// Capture and deliver the bodies of a §J first-attach preamble scan. Those
/// lines are not tracked by the kernel, so `record_start` is `None` and the
/// origin is [`RecordOrigin::Preamble`].
fn capture_preamble_bodies(
    bodies: Vec<String>,
    session_id: &str,
    file: &Path,
    reader_seq: &mut u64,
    sender: Option<&UnboundedSender<Arc<CapturedRecord>>>,
    attach: &ReaderAttachment,
) -> Vec<Arc<CapturedRecord>> {
    let mut records = Vec::new();
    for text in bodies {
        let record = capture_record(
            text,
            None,
            None,
            RecordOrigin::Preamble,
            session_id,
            file,
            reader_seq,
            attach,
        );
        deliver_capture_record(sender, &record);
        records.push(record);
    }
    records
}

#[allow(clippy::too_many_arguments)]
async fn watch_loop<R: tauri::Runtime>(
    project_dir: PathBuf,
    network: OutboundNetwork,
    dest: tokio::sync::watch::Receiver<Option<BotTarget>>,
    reanchor: tokio::sync::watch::Receiver<u64>,
    session_id: String,
    cancel: CancellationToken,
    app: tauri::AppHandle<R>,
    sink: Option<UnboundedSender<Arc<CapturedRecord>>>,
) {
    let mut dest_rx = dest;
    let mut reanchor_rx = reanchor;
    let mut current_dest: Option<BotTarget> = dest_rx.borrow_and_update().clone();

    // #2232 phase 4 section 8: with no bot demand no diagnostic is built, so
    // `BridgeLogger::new` — which truncates the **global** diagnostic files
    // (`telegram/output.rs:140`) — is never called for a room-only reader. On a
    // hot attach the loggers are born at that moment, which is when they are
    // truncated on attach today.
    let mut logger = current_dest
        .as_ref()
        .map(|_| BridgeLogger::new(&session_id));
    let mut diag = current_dest.as_ref().map(|_| DiagLogger::new());
    // Log through the bridge logger only when one exists, so `JSONL_EXTRACT`
    // is not written for a room-only reader (section 8).
    macro_rules! bridge_log {
        ($tag:expr, $msg:expr) => {
            if let Some(bridge_logger) = logger.as_mut() {
                bridge_logger.log($tag, &session_id, $msg);
            }
        };
    }
    let mut buffer = String::new();
    let mut last_buffer_add = Instant::now();
    let flush_delay = Duration::from_millis(FLUSH_DELAY_MS);

    // #2232 phase 4 section 4.2: the supervisor passes the live sender in, so
    // the records of a room-only reader reach `CaptureRegistry` with no bot
    // anywhere. With `None` the emit is a no-op.
    let capture_tx: Option<UnboundedSender<Arc<CapturedRecord>>> = sink;
    let mut reader_seq: u64 = 0;
    // #2232 phase 3: the reader's own epoch and file observation. Both are
    // computed only when a sink is attached, so a room without the flag keeps
    // the pre-#2232 syscall count.
    let mut observer = ReaderObservations::default();
    // The first sweep of a rotated transcript is a backfill, not a live turn
    // (section 8.3). Declared divergence: Telegram does send that content
    // today, so a legitimate first turn can be suppressed downstream.
    let mut rotation_backfill_pending = false;

    let attach_time: DateTime<Utc> = Utc::now();
    let mut current_file: Option<PathBuf> = None;
    let mut current_file_mtime: Option<SystemTime> = None;
    let mut file_offset: u64 = 0;
    let mut line_remainder = String::new();
    let mut dir_warned = false;

    bridge_log!(
        "JSONL_INIT",
        &format!("project_dir={}", project_dir.display())
    );

    let mut poll_interval = tokio::time::interval(Duration::from_millis(POLL_INTERVAL_MS));
    poll_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,

            // §6: the destination is consulted only when it changes, and the
            // three transitions run **inside the reader task**. There is no
            // `await` between them: yielding would flush the pending buffer to
            // the NEW destination, which is exactly what this contract
            // prevents, and the preamble could use a different offset than the
            // reader had when it started.
            changed = dest_rx.changed() => {
                if changed.is_err() {
                    // The supervisor dropped the sender: the reader is going away.
                    break;
                }
                let new_dest = dest_rx.borrow_and_update().clone();
                let attaching = new_dest.is_some();
                let over_live_reader = current_file.is_some();

                // 1. discard the pending buffer. §7: the chat no longer
                //    receives up to 2 s of pre-attach text; those records
                //    already reached the sink on their own.
                buffer.clear();

                // 2. switch the destination.
                current_dest = new_dest;
                if attaching {
                    if logger.is_none() {
                        logger = Some(BridgeLogger::new(&session_id));
                    }
                    if diag.is_none() {
                        diag = Some(DiagLogger::new());
                    }
                } else {
                    logger = None;
                    diag = None;
                }

                // 3. run the preamble, for a live attach only. A cold attach —
                //    the reader has not bound a file yet — keeps the §J scan
                //    below, which also marks the sink. A live attach **does not
                //    assign `file_offset`**: the reader stays where it is, and
                //    only the lines strictly below that offset are emitted, so
                //    the chat sees no duplicates. The moment of the switch is
                //    the attach time.
                if attaching && over_live_reader {
                    let switch_time: DateTime<Utc> = Utc::now();
                    let mut preamble: Vec<String> = Vec::new();
                    if let Some(ref path) = current_file {
                        match read_preamble_with_starts(path, switch_time) {
                            Ok(lines) => {
                                for (line_start, body) in lines {
                                    if line_start >= file_offset {
                                        // Already ahead of the reader: the
                                        // normal loop sends this one.
                                        continue;
                                    }
                                    preamble.push(body);
                                }
                            }
                            Err(e) => {
                                log::warn!(
                                    "[JSONL_ERR] live-attach preamble failed for session {}: {}",
                                    session_id, e
                                );
                            }
                        }
                    }
                    // A live-attach preamble goes to Telegram **only**, never
                    // to the sink: those lines were already delivered as live
                    // records, and re-delivering them with the same key could
                    // consume or overwrite a pending live candidate.
                    for body in preamble {
                        bridge_log!("JSONL_PREAMBLE", &body);
                        buffer.push_str(&body);
                        buffer.push('\n');
                        last_buffer_add = Instant::now();
                    }
                }
            }

            // §5.2: a restart keeps the demand set untouched and re-anchors the
            // transcript **file**. Dropping the binding makes the next poll
            // resolve the path again and open the epoch entry for whatever that
            // resolution now returns; the supervisor supersedes the cut's
            // sequence part and clears the slot.
            changed = reanchor_rx.changed() => {
                if changed.is_ok() {
                    let _ = *reanchor_rx.borrow_and_update();
                    current_file = None;
                    current_file_mtime = None;
                    file_offset = 0;
                    line_remainder.clear();
                    rotation_backfill_pending = false;
                    bridge_log!("JSONL_REANCHOR", "restart: re-resolving transcript file");
                }
            }

            _ = poll_interval.tick() => {
                // Check if project directory exists yet
                if !project_dir.is_dir() {
                    if !dir_warned {
                        bridge_log!("JSONL_WAIT", "project directory does not exist yet");
                        dir_warned = true;
                    }
                    continue;
                }
                if dir_warned {
                    bridge_log!("JSONL_INIT", "project directory appeared");
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
                                        // The §J scan reads the tail, so it
                                        // carries no head evidence: length
                                        // alone decides the epoch here.
                                        let attach = if capture_tx.is_some() {
                                            observer.observe(p, file_len, Vec::new())
                                        } else {
                                            ReaderAttachment::default()
                                        };
                                        for record in capture_preamble_bodies(
                                            bodies,
                                            &session_id,
                                            p,
                                            &mut reader_seq,
                                            capture_tx.as_ref(),
                                            &attach,
                                        ) {
                                            bridge_log!("JSONL_PREAMBLE", &record.text);
                                            if current_dest.is_some() {
                                                buffer.push_str(&record.text);
                                                buffer.push('\n');
                                                last_buffer_add = Instant::now();
                                            }
                                        }
                                        file_offset = file_len;
                                        bridge_log!("JSONL_FILE",
                                            &format!("initial file, preamble scan done, offset={}", file_offset));
                                    }
                                    Err(e) => {
                                        bridge_log!("JSONL_ERR",
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
                            rotation_backfill_pending = true;
                            bridge_log!("JSONL_ROTATE", &format!("new file: {:?}", latest));
                        }
                        current_file = latest;
                        current_file_mtime = current_file.as_ref()
                            .and_then(|p| std::fs::metadata(p).ok())
                            .and_then(|m| m.modified().ok());
                        line_remainder.clear();
                    }
                }

                if let Some(ref path) = current_file {
                    let read_start = file_offset;
                    match read_new_lines_with_starts(path, &mut file_offset, &mut line_remainder) {
                        Ok(new_lines) => {
                            let attach = if capture_tx.is_some() {
                                let head = head_from_lines(&new_lines, read_start);
                                observer.observe(path, file_offset, head)
                            } else {
                                ReaderAttachment::default()
                            };
                            let origin = if rotation_backfill_pending && !new_lines.is_empty() {
                                rotation_backfill_pending = false;
                                RecordOrigin::RotationBackfill
                            } else {
                                RecordOrigin::Live
                            };
                            for record in capture_live_lines(
                                new_lines,
                                &session_id,
                                path,
                                &mut reader_seq,
                                capture_tx.as_ref(),
                                origin,
                                &attach,
                            ) {
                                bridge_log!("JSONL_EXTRACT", &record.text);
                                if current_dest.is_some() {
                                    buffer.push_str(&record.text);
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
                            bridge_log!("JSONL_ERR", &e.to_string());
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
                        if let (Some(target), Some(bridge_logger), Some(diag_logger)) =
                            (current_dest.as_ref(), logger.as_mut(), diag.as_mut())
                        {
                            flush_buffer(
                                &mut buffer, &network, &target.token, target.chat_id,
                                &session_id, &app, bridge_logger, diag_logger,
                                true, // skip_dedup: JSONL text is clean, repeated lines are legitimate
                            ).await;
                        } else {
                            buffer.clear();
                        }
                    }
                }
            }
        }
    }

    // G1: Final poll + flush after cancel (don't lose buffered content)
    if let Some(ref path) = current_file {
        let read_start = file_offset;
        if let Ok(new_lines) =
            read_new_lines_with_starts(path, &mut file_offset, &mut line_remainder)
        {
            let attach = if capture_tx.is_some() {
                let head = head_from_lines(&new_lines, read_start);
                observer.observe(path, file_offset, head)
            } else {
                ReaderAttachment::default()
            };
            let origin = if rotation_backfill_pending && !new_lines.is_empty() {
                RecordOrigin::RotationBackfill
            } else {
                RecordOrigin::Live
            };
            for record in capture_live_lines(
                new_lines,
                &session_id,
                path,
                &mut reader_seq,
                capture_tx.as_ref(),
                origin,
                &attach,
            ) {
                if current_dest.is_some() {
                    buffer.push_str(&record.text);
                    buffer.push('\n');
                }
            }
        }
    }
    if !buffer.is_empty() {
        if let (Some(target), Some(bridge_logger), Some(diag_logger)) =
            (current_dest.as_ref(), logger.as_mut(), diag.as_mut())
        {
            flush_buffer(
                &mut buffer,
                &network,
                &target.token,
                target.chat_id,
                &session_id,
                &app,
                bridge_logger,
                diag_logger,
                true,
            )
            .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Claude fixture line carrying one assistant `text` block.
    fn assistant_line(text: &str) -> String {
        serde_json::json!({
            "type": "assistant",
            "message": {"content": [{"type": "text", "text": text}]}
        })
        .to_string()
    }

    fn capture_one(line: &str) -> Vec<Arc<CapturedRecord>> {
        let mut reader_seq = 0u64;
        capture_live_lines(
            vec![(0, line.to_string())],
            "claude-session",
            Path::new("session.jsonl"),
            &mut reader_seq,
            None,
            RecordOrigin::Live,
            &ReaderAttachment::default(),
        )
    }

    #[test]
    fn assistant_records_carry_claudes_permanent_bits() {
        // Test 6: Claude yields `provider_final == false`, `turn_identified ==
        // false`, `turn_id == None`. Those bits are the permanent shape
        // (`epic.md` 3.2/3.3); no phase raises them.
        let records = capture_one(&assistant_line("hello"));
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record.text, "hello");
        assert_eq!(record.turn_id, None);
        assert!(!record.provider_final);
        assert!(!record.turn_identified);
        assert_eq!(record.provider, CaptureProvider::Claude);
        assert_eq!(record.origin, RecordOrigin::Live);
        assert_eq!(record.record_start, Some(0));
        assert_eq!(record.reader_seq, 0);
        assert_eq!(record.epoch, 0);
        assert_eq!(record.file, PathBuf::from("session.jsonl"));
        assert_eq!(record.session_id, "claude-session");
    }

    #[test]
    fn tool_use_and_thinking_blocks_still_yield_nothing() {
        // Test 6: the extractor's whitelist is unchanged by the capture layer.
        let tool_use = serde_json::json!({
            "type": "assistant",
            "message": {"content": [{"type": "tool_use", "name": "Bash", "input": {}}]}
        })
        .to_string();
        let thinking = serde_json::json!({
            "type": "assistant",
            "message": {"content": [{"type": "thinking", "thinking": "why"}]}
        })
        .to_string();
        for line in [tool_use, thinking] {
            assert!(capture_one(&line).is_empty(), "line={line}");
        }
    }

    #[test]
    fn text_sha256_covers_the_utf8_bytes() {
        // Test 7: the digest is over `text.as_bytes()`, not over characters.
        let text = "café ✅";
        let records = capture_one(&assistant_line(text));
        assert_eq!(records.len(), 1);
        let expected: [u8; 32] = Sha256::digest(text.as_bytes()).into();
        assert_eq!(records[0].text_sha256, expected);
    }

    #[test]
    fn sidechain_assistant_records_are_captured_today() {
        // Test 9 — sub-agent residual, pinned not fixed (`epic.md` 9.7):
        // `claude_watcher` has no `isSidechain` filter (0 hits) and follows
        // `find_latest_jsonl`, so a sidechain assistant record is captured.
        // Narrowing this is not in #2264 scope; this test makes a later
        // narrowing a deliberate, visible change.
        let line = serde_json::json!({
            "type": "assistant",
            "isSidechain": true,
            "message": {"content": [{"type": "text", "text": "sidechain body"}]}
        })
        .to_string();
        let records = capture_one(&line);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].text, "sidechain body");
    }
}
