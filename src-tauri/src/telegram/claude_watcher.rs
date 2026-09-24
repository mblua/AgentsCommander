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
/// `sink`.
#[allow(clippy::too_many_arguments)]
pub fn spawn_watch_task<R: tauri::Runtime>(
    project_dir: PathBuf,
    network: OutboundNetwork,
    dest: tokio::sync::watch::Receiver<Option<BotTarget>>,
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

/// The §6 attach/detach transition: **discard the pending buffer, switch the
/// destination, run the preamble**, in that order.
///
/// Deliberately **not** `async`. The order is correctness, not style, and there
/// must be no `await` between the three: yielding would flush the pending
/// buffer to the NEW destination, which is exactly what this contract
/// prevents, and the preamble could use a different offset than the reader had
/// when it started. A synchronous function makes that structural rather than a
/// convention — an `await` cannot be introduced here without changing the
/// signature.
///
/// Returns the preamble bodies to send **to Telegram only**. There is no sink
/// parameter by construction: a live-attach preamble must never reach the sink,
/// because those lines were already delivered as live records and re-delivering
/// them with the same key could consume or overwrite a pending live candidate.
/// The cold-attach preamble, which the sink does need, runs on the §J path in
/// the poll loop instead.
fn apply_destination_change(
    buffer: &mut String,
    current_dest: &mut Option<BotTarget>,
    new_dest: Option<BotTarget>,
    current_file: Option<&Path>,
    file_offset: u64,
    switch_time: DateTime<Utc>,
) -> Vec<String> {
    // 1. discard the pending buffer. §7: the chat no longer receives up to 2 s
    //    of pre-attach text; those records already reached the sink on their own.
    buffer.clear();

    // 2. switch the destination.
    let attaching = new_dest.is_some();
    *current_dest = new_dest;

    // 3. run the preamble, for a live attach only. A cold attach — the reader
    //    has not bound a file yet — keeps the §J scan in the poll loop, which
    //    also marks the sink. A live attach **does not assign `file_offset`**:
    //    the reader stays where it is and only the lines strictly below that
    //    offset are emitted, so the chat sees no duplicates.
    let Some(path) = current_file else {
        return Vec::new();
    };
    if !attaching {
        return Vec::new();
    }
    match read_preamble_with_starts(path, switch_time) {
        Ok(lines) => lines
            .into_iter()
            .filter(|(start, _)| *start < file_offset)
            .map(|(_, body)| body)
            .collect(),
        Err(e) => {
            log::warn!("[JSONL_ERR] live-attach preamble read failed: {}", e);
            Vec::new()
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn watch_loop<R: tauri::Runtime>(
    project_dir: PathBuf,
    network: OutboundNetwork,
    dest: tokio::sync::watch::Receiver<Option<BotTarget>>,
    session_id: String,
    cancel: CancellationToken,
    app: tauri::AppHandle<R>,
    sink: Option<UnboundedSender<Arc<CapturedRecord>>>,
) {
    let mut dest_rx = dest;
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
        // `biased`: the destination change is processed before the next file
        // poll, never after it (section 6).
        tokio::select! {
            biased;
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
                // The loggers are born at the moment of a hot attach, which is
                // when they are truncated on attach today, so what is
                // observable does not change (§8).
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
                // The moment of the destination switch serves as `attach_time`.
                let preamble = apply_destination_change(
                    &mut buffer,
                    &mut current_dest,
                    new_dest,
                    current_file.as_deref(),
                    file_offset,
                    Utc::now(),
                );
                for body in preamble {
                    bridge_log!("JSONL_PREAMBLE", &body);
                    buffer.push_str(&body);
                    buffer.push('\n');
                    last_buffer_add = Instant::now();
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

    use std::io::Write;

    /// A Claude fixture line with a timestamp, as the preamble scan needs.
    fn stamped_line(text: &str, ts: DateTime<Utc>) -> String {
        serde_json::json!({
            "type": "assistant",
            "timestamp": ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "message": {"content": [{"type": "text", "text": text}]}
        })
        .to_string()
    }

    struct Transcript {
        _dir: tempfile::TempDir,
        path: PathBuf,
        /// Absolute start offset of each written line, in order.
        starts: Vec<u64>,
        bodies: Vec<String>,
    }

    /// Five recent assistant turns, one with a multi-byte character and one
    /// terminated by CRLF, so the byte arithmetic is exercised (test 4).
    fn transcript(now: DateTime<Utc>) -> Transcript {
        let dir = tempfile::tempdir().expect("fixture dir");
        let path = dir.path().join("session.jsonl");
        let bodies = vec![
            "first".to_string(),
            "caf\u{e9} \u{2705} second".to_string(),
            "third".to_string(),
            "fourth".to_string(),
            "fifth".to_string(),
        ];
        let mut file = std::fs::File::create(&path).expect("fixture file");
        let mut starts = Vec::new();
        let mut offset = 0u64;
        for (i, body) in bodies.iter().enumerate() {
            let line = stamped_line(body, now - chrono::Duration::milliseconds(100));
            // Row 2 ends with CRLF; the rest with LF.
            let raw = if i == 1 {
                format!("{line}\r\n")
            } else {
                format!("{line}\n")
            };
            starts.push(offset);
            offset += raw.len() as u64;
            file.write_all(raw.as_bytes()).expect("write fixture line");
        }
        file.sync_all().expect("sync fixture");
        Transcript {
            _dir: dir,
            path,
            starts,
            bodies,
        }
    }

    /// What the normal poll loop sends to Telegram from `offset` onward.
    fn loop_bodies_from(path: &Path, offset: u64) -> Vec<String> {
        let mut cursor = offset;
        let mut remainder = String::new();
        read_new_lines_with_starts(path, &mut cursor, &mut remainder)
            .expect("read fixture")
            .into_iter()
            .filter_map(|(_, line)| extract_assistant_text(&line))
            .collect()
    }

    /// Test 11 (Claude half; the Codex half lives in `codex_watcher`): with no
    /// Bot demand no `JSONL_EXTRACT` line is written and **no global log file
    /// is truncated**. Each file's size is asserted unchanged, not merely that
    /// no line matched — `BridgeLogger::new` truncates on construction
    /// (`telegram/output.rs:140`), so "not constructed" is the only safe state.
    #[tokio::test]
    async fn a_room_only_claude_reader_truncates_no_global_log_and_still_emits() {
        // Serialized against the Codex hot-attach test, which constructs the
        // loggers and writes to the same global files.
        let _logs = crate::telegram::codex_watcher::lock_global_diagnostic_files().await;
        let dir = tempfile::tempdir().expect("projects dir");
        let now = Utc::now();
        std::fs::write(
            dir.path().join("session.jsonl"),
            format!("{}\n", stamped_line("room-only body", now)),
        )
        .expect("write transcript");

        let before: Vec<(PathBuf, u64)> = match crate::config::config_dir() {
            Some(config) => ["telegram-bridge.log", "diag-raw.log", "diag-sent.log"]
                .into_iter()
                .map(|name| {
                    let path = config.join(name);
                    let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    (path, len)
                })
                .collect(),
            None => Vec::new(),
        };

        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build claude watcher test app");
        let network = crate::network::OutboundNetwork::new_for_tests(1);
        let cancel = CancellationToken::new();
        let (dest_tx, dest_rx) = tokio::sync::watch::channel(None);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        let task = spawn_watch_task(
            dir.path().to_path_buf(),
            None,
            network.clone(),
            dest_rx,
            "room-only-claude".to_string(),
            cancel.clone(),
            app.handle().clone(),
            Some(tx),
        );

        let record = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("a room-only reader must still deliver records")
            .expect("the sink stays open while the reader runs");
        assert_eq!(record.text, "room-only body");

        cancel.cancel();
        drop(dest_tx);
        let _ = tokio::time::timeout(Duration::from_secs(5), task).await;

        assert!(
            network.acquired_labels_for_tests().is_empty(),
            "no HTTP request may be attempted without a bot demand"
        );
        for (path, len) in before {
            let after = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            assert_eq!(
                after,
                len,
                "{} must not be truncated by a room-only reader",
                path.display()
            );
        }
    }

    /// Test 4: preamble line starts index back to the exact raw bytes,
    /// including a line containing a multi-byte character and a line ending in
    /// CRLF. Starts are computed on the raw bytes plus the window offset, never
    /// on the decoded text.
    #[test]
    fn preamble_line_starts_index_back_to_the_exact_raw_bytes() {
        let now = Utc::now();
        let fixture = transcript(now);
        let raw = std::fs::read(&fixture.path).expect("read fixture bytes");

        let lines = read_preamble_with_starts(&fixture.path, now).expect("preamble scan");

        assert_eq!(lines.len(), fixture.bodies.len());
        for ((start, body), (expected_start, expected_body)) in lines
            .iter()
            .zip(fixture.starts.iter().zip(fixture.bodies.iter()))
        {
            assert_eq!(start, expected_start, "body={body}");
            // The start walks back to the exact bytes in the file.
            let tail = &raw[*start as usize..];
            let end = tail
                .iter()
                .position(|&b| b == b'\n')
                .expect("every fixture line is terminated");
            let re_read = String::from_utf8(tail[..end].to_vec())
                .expect("fixture lines are valid UTF-8")
                .trim_end_matches('\r')
                .to_string();
            assert_eq!(
                extract_assistant_text(&re_read).as_deref(),
                Some(expected_body.as_str())
            );
            assert_eq!(body, expected_body);
        }
    }

    /// Tests 1 and 2: attaching over a **live** reader and a **cold** attach
    /// produce the same chat content over the same fixture transcript, and no
    /// duplicate line reaches Telegram when the preamble runs over a live
    /// reader. This is the parity hypothesis of `epic.md` 9.6.
    #[test]
    fn a_live_attach_and_a_cold_attach_send_the_same_chat_content_exactly_once() {
        let now = Utc::now();
        let fixture = transcript(now);

        // Cold attach: the §J scan emits the whole recent tail, then the reader
        // continues from EOF, which has nothing more to send.
        let (cold_preamble, _ids, cold_offset) =
            read_preamble_for_race(&fixture.path, now, claude_preamble_extractor)
                .expect("cold preamble");
        let cold_chat: Vec<String> = cold_preamble
            .into_iter()
            .chain(loop_bodies_from(&fixture.path, cold_offset))
            .collect();

        // Live attach: the reader is already running and has consumed the first
        // three turns, so it sits at the start of the fourth.
        let reader_offset = fixture.starts[3];
        let mut buffer = String::new();
        let mut current_dest = None;
        let live_preamble = apply_destination_change(
            &mut buffer,
            &mut current_dest,
            Some(BotTarget {
                token: "token".into(),
                chat_id: 7,
            }),
            Some(&fixture.path),
            reader_offset,
            now,
        );
        let live_chat: Vec<String> = live_preamble
            .clone()
            .into_iter()
            .chain(loop_bodies_from(&fixture.path, reader_offset))
            .collect();

        assert_eq!(cold_chat, fixture.bodies, "the fixture pins the cold case");
        assert_eq!(live_chat, cold_chat, "a live attach must match a cold one");

        // No duplicate: the preamble stops exactly where the reader stands.
        assert_eq!(live_preamble, fixture.bodies[..3].to_vec());
        let mut seen = live_chat.clone();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), live_chat.len(), "no line is sent twice");
    }

    /// Test 3: the pending buffer is **not** flushed to the new destination
    /// when the destination switches — it is discarded first, before the
    /// destination is switched and before the preamble runs.
    #[test]
    fn the_pending_buffer_is_discarded_and_never_reaches_the_new_destination() {
        let now = Utc::now();
        let fixture = transcript(now);
        let mut buffer = String::from("pre-attach text that must never be sent\n");
        let mut current_dest = None;

        let preamble = apply_destination_change(
            &mut buffer,
            &mut current_dest,
            Some(BotTarget {
                token: "token".into(),
                chat_id: 7,
            }),
            Some(&fixture.path),
            fixture.starts[3],
            now,
        );

        assert!(buffer.is_empty(), "the pending buffer must be discarded");
        assert!(
            !preamble.iter().any(|body| body.contains("pre-attach")),
            "pre-attach text must not reappear through the preamble"
        );
        assert_eq!(current_dest.map(|d| d.chat_id), Some(7));
    }

    /// Test 13: a live-attach preamble delivers **zero** records to the sink; a
    /// cold-attach preamble delivers them marked `Preamble`.
    #[test]
    fn a_live_attach_preamble_reaches_telegram_only_and_a_cold_one_marks_the_sink() {
        let now = Utc::now();
        let fixture = transcript(now);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        // Live attach: the transition function has no sink parameter at all, so
        // no record can reach one.
        let mut buffer = String::new();
        let mut current_dest = None;
        let live = apply_destination_change(
            &mut buffer,
            &mut current_dest,
            Some(BotTarget {
                token: "token".into(),
                chat_id: 7,
            }),
            Some(&fixture.path),
            fixture.starts[3],
            now,
        );
        assert!(!live.is_empty(), "the live attach does emit to Telegram");
        assert!(
            rx.try_recv().is_err(),
            "a live-attach preamble delivers zero records to the sink"
        );

        // Cold attach: the §J bodies are delivered, marked `Preamble`.
        let (cold_bodies, _ids, _len) =
            read_preamble_for_race(&fixture.path, now, claude_preamble_extractor)
                .expect("cold preamble");
        let mut reader_seq = 0u64;
        let records = capture_preamble_bodies(
            cold_bodies,
            "session",
            &fixture.path,
            &mut reader_seq,
            Some(&tx),
            &ReaderAttachment::default(),
            RecordOrigin::Preamble,
        );
        assert_eq!(records.len(), fixture.bodies.len());
        for _ in 0..records.len() {
            let record = rx.try_recv().expect("cold preamble reaches the sink");
            assert_eq!(record.origin, RecordOrigin::Preamble);
        }
    }

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

    // ── #2454: the pinned first attach ──

    const PIN_ID: &str = "16045f91-7d59-4c75-acdb-11ebfcb9aa68";

    fn inputs<'a>(
        transcript_id: Option<&'a str>,
        pinned: Option<&'a Path>,
        newest: Option<&'a Path>,
        first_poll: bool,
        deadline_expired: bool,
    ) -> AttachInputs<'a> {
        AttachInputs {
            transcript_id,
            pinned,
            newest,
            first_poll,
            deadline_expired,
        }
    }

    // Test 10.
    #[test]
    fn decide_attach_waits_when_the_pinned_file_is_absent_at_the_first_poll() {
        let old = Path::new("old.jsonl");
        assert_eq!(
            decide_attach(inputs(Some(PIN_ID), None, Some(old), true, false)),
            AttachDecision::Wait
        );
        assert_eq!(
            decide_attach(inputs(Some(PIN_ID), None, Some(old), false, false)),
            AttachDecision::Wait
        );
    }

    // Test 11.
    #[test]
    fn decide_attach_pins_when_the_awaited_file_appears() {
        let pinned = PathBuf::from(format!("{PIN_ID}.jsonl"));
        for newest in [Some(pinned.as_path()), Some(Path::new("old.jsonl"))] {
            assert_eq!(
                decide_attach(inputs(Some(PIN_ID), Some(&pinned), newest, false, false)),
                AttachDecision::Pinned(pinned.clone())
            );
        }
    }

    // Test 12: late or resumed history, the pinned file already newest.
    #[test]
    fn decide_attach_takes_mtime_when_the_pinned_file_already_exists_and_is_newest() {
        let pinned = PathBuf::from(format!("{PIN_ID}.jsonl"));
        assert_eq!(
            decide_attach(inputs(
                Some(PIN_ID),
                Some(&pinned),
                Some(&pinned),
                true,
                false
            )),
            AttachDecision::Mtime(pinned.clone())
        );
    }

    // Test 13: the post-`/clear` late attach, the pinned file not newest.
    #[test]
    fn decide_attach_takes_mtime_when_the_pinned_file_already_exists_and_is_not_newest() {
        let pinned = PathBuf::from(format!("{PIN_ID}.jsonl"));
        let newer = Path::new("after-clear.jsonl");
        assert_eq!(
            decide_attach(inputs(
                Some(PIN_ID),
                Some(&pinned),
                Some(newer),
                true,
                false
            )),
            AttachDecision::Mtime(newer.to_path_buf())
        );
    }

    // Test 14.
    #[test]
    fn decide_attach_without_an_id_is_mtime_for_every_input() {
        let pinned = PathBuf::from(format!("{PIN_ID}.jsonl"));
        let newest = Path::new("newest.jsonl");
        for pinned in [None, Some(pinned.as_path())] {
            for first_poll in [true, false] {
                for deadline_expired in [true, false] {
                    assert_eq!(
                        decide_attach(inputs(
                            None,
                            pinned,
                            Some(newest),
                            first_poll,
                            deadline_expired
                        )),
                        AttachDecision::Mtime(newest.to_path_buf())
                    );
                    assert_eq!(
                        decide_attach(inputs(None, pinned, None, first_poll, deadline_expired)),
                        AttachDecision::Nothing
                    );
                }
            }
        }
    }

    // Test 15.
    #[test]
    fn decide_attach_falls_back_to_mtime_when_the_deadline_expires() {
        let old = Path::new("old.jsonl");
        assert_eq!(
            decide_attach(inputs(Some(PIN_ID), None, Some(old), false, true)),
            AttachDecision::MtimeFallback(old.to_path_buf())
        );
        assert_eq!(
            decide_attach(inputs(Some(PIN_ID), None, None, false, true)),
            AttachDecision::Nothing
        );
    }

    /// A reader over a fixture projects dir, with a live sink.
    struct Reader {
        _app: tauri::App<tauri::test::MockRuntime>,
        _dest_tx: tokio::sync::watch::Sender<Option<BotTarget>>,
        cancel: CancellationToken,
        task: tokio::task::JoinHandle<()>,
        rx: tokio::sync::mpsc::UnboundedReceiver<Arc<CapturedRecord>>,
    }

    impl Reader {
        fn start(project_dir: &Path, transcript_id: Option<String>) -> Self {
            let app = tauri::test::mock_builder()
                .build(tauri::test::mock_context(tauri::test::noop_assets()))
                .expect("build claude watcher test app");
            let (dest_tx, dest_rx) = tokio::sync::watch::channel(None);
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            let cancel = CancellationToken::new();
            let task = spawn_watch_task(
                project_dir.to_path_buf(),
                transcript_id,
                crate::network::OutboundNetwork::new_for_tests(1),
                dest_rx,
                "pin-session".to_string(),
                cancel.clone(),
                app.handle().clone(),
                Some(tx),
            );
            Self {
                _app: app,
                _dest_tx: dest_tx,
                cancel,
                task,
                rx,
            }
        }

        async fn next(&mut self, within: Duration) -> Option<Arc<CapturedRecord>> {
            tokio::time::timeout(within, self.rx.recv())
                .await
                .ok()
                .flatten()
        }

        async fn stop(self) {
            self.cancel.cancel();
            let _ = tokio::time::timeout(Duration::from_secs(5), self.task).await;
        }
    }

    /// Write `content` to `path` whole, through a rename, so the reader never
    /// sees a half-written first line.
    fn write_atomically(path: &Path, content: &str) {
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, content).expect("write fixture");
        std::fs::rename(&tmp, path).expect("publish fixture");
    }

    fn backdate(path: &Path, secs: u64) {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("open fixture");
        file.set_modified(SystemTime::now() - std::time::Duration::from_secs(secs))
            .expect("backdate fixture");
    }

    /// Commit `record` as an `Automatic` effect and return `routable`, the
    /// routing answer of `capture::state::commit_effect`.
    fn routable(room: &Path, record: &Arc<CapturedRecord>) -> bool {
        use crate::capture::key::ConsumptionKey;
        use crate::capture::state::{commit_effect, EffectKind, EffectPreconditions};
        let slot = crate::capture::sink::CaptureSlot::new();
        slot.offer(Arc::clone(record), 0);
        let key = ConsumptionKey::from_record(record, 0);
        let pre = EffectPreconditions {
            session_alive: true,
            session_id: record.session_id.clone(),
            anchor: "agent".to_owned(),
            provider: CaptureProvider::Claude,
            unique_live_session_for_cwd: true,
            no_pending_user_input: true,
            effective_ready: true,
            observed_at: std::time::Instant::now(),
            kind: EffectKind::Automatic,
        };
        commit_effect(room, &slot, slot.seq(), &key, &pre)
            .expect("the commit takes the record")
            .routable
    }

    fn room(temp: &tempfile::TempDir) -> PathBuf {
        let room = temp.path().join("room-1-dev-team");
        std::fs::create_dir_all(&room).expect("room dir");
        crate::capture::state::forget_room_for_tests(&room);
        room
    }

    /// Test 16, the reported defect: `<id>.jsonl` is absent at the first poll,
    /// so the reader waits; when it appears its first reply is `Live` and is
    /// actually routed.
    #[tokio::test]
    async fn a_fresh_spawn_first_reply_is_live_and_routed() {
        let temp = tempfile::tempdir().expect("temp");
        let projects = temp.path().join("projects");
        std::fs::create_dir_all(&projects).expect("projects dir");
        let old = projects.join("old-session.jsonl");
        std::fs::write(&old, format!("{}\n", stamped_line("old reply", Utc::now())))
            .expect("old transcript");

        let mut reader = Reader::start(&projects, Some(PIN_ID.to_string()));
        // The reader waits: nothing from the old transcript reaches the sink.
        assert!(reader.next(Duration::from_millis(1200)).await.is_none());

        let pinned = projects.join(format!("{PIN_ID}.jsonl"));
        write_atomically(
            &pinned,
            &format!("{}\n", stamped_line("first reply", Utc::now())),
        );
        let record = reader
            .next(Duration::from_secs(10))
            .await
            .expect("the first reply reaches the sink");
        reader.stop().await;

        assert_eq!(record.text, "first reply");
        assert_eq!(record.file, pinned);
        assert_eq!(record.origin, RecordOrigin::Live);
        assert!(
            !crate::capture::state::is_baseline(&record),
            "supporting evidence only"
        );
        assert!(
            routable(&room(&temp), &record),
            "the first reply must be routed"
        );
    }

    /// Test 16b (guard): `<id>.jsonl` already exists and is the newest, and
    /// ends with a reply inside the 5 s window that a previous reader already
    /// consumed. A new reader stamps it `Preamble` and never routes it again.
    #[tokio::test]
    async fn an_existing_pinned_file_reply_is_not_routed_twice() {
        let temp = tempfile::tempdir().expect("temp");
        let room = room(&temp);
        let projects = temp.path().join("projects");
        std::fs::create_dir_all(&projects).expect("projects dir");
        let old = projects.join("old-session.jsonl");
        std::fs::write(&old, format!("{}\n", stamped_line("old reply", Utc::now())))
            .expect("old transcript");
        backdate(&old, 60);
        let pinned = projects.join(format!("{PIN_ID}.jsonl"));
        let line = stamped_line("consumed reply", Utc::now());
        std::fs::write(&pinned, format!("{line}\n")).expect("pinned transcript");

        // A previous reader consumed the reply through an incremental read.
        let mut reader_seq = 0u64;
        let earlier = capture_live_lines(
            vec![(0, line)],
            "pin-session",
            &pinned,
            &mut reader_seq,
            None,
            RecordOrigin::Live,
            &ReaderAttachment::default(),
        );
        assert!(routable(&room, &earlier[0]), "the first delivery is routed");

        let mut reader = Reader::start(&projects, Some(PIN_ID.to_string()));
        let record = reader
            .next(Duration::from_secs(10))
            .await
            .expect("the preamble reaches the sink");
        reader.stop().await;

        assert_eq!(record.text, "consumed reply");
        assert_eq!(record.file, pinned);
        assert_eq!(record.origin, RecordOrigin::Preamble);
        assert!(crate::capture::state::is_baseline(&record));
        assert!(
            !routable(&room, &record),
            "a consumed reply must not route twice"
        );
    }

    /// Test 17 (guard): an attach by mtime over genuine history still stamps
    /// `Preamble` and is still suppressed.
    #[tokio::test]
    async fn an_mtime_first_attach_over_history_is_still_suppressed() {
        let temp = tempfile::tempdir().expect("temp");
        let projects = temp.path().join("projects");
        std::fs::create_dir_all(&projects).expect("projects dir");
        let history = projects.join("history.jsonl");
        std::fs::write(
            &history,
            format!("{}\n", stamped_line("history reply", Utc::now())),
        )
        .expect("history transcript");

        let mut reader = Reader::start(&projects, None);
        let record = reader
            .next(Duration::from_secs(10))
            .await
            .expect("the preamble reaches the sink");
        reader.stop().await;

        assert_eq!(record.file, history);
        assert_eq!(record.origin, RecordOrigin::Preamble);
        assert!(
            !routable(&room(&temp), &record),
            "history must stay suppressed"
        );
    }

    /// Test 18 (guard): an unextractable id changes nothing. Asserted as
    /// absolute values for each shape: the newest file by mtime, `Preamble`
    /// stamps, and the offset at that file's length.
    #[tokio::test]
    async fn an_unextractable_id_attaches_by_mtime_and_reads_to_the_end() {
        for shape in [vec!["--continue".to_string()], Vec::new()] {
            let transcript_id = crate::commands::session::transcript_id_from_args(&shape);
            let temp = tempfile::tempdir().expect("temp");
            let projects = temp.path().join("projects");
            std::fs::create_dir_all(&projects).expect("projects dir");
            let older = projects.join("older.jsonl");
            std::fs::write(
                &older,
                format!("{}\n", stamped_line("older reply", Utc::now())),
            )
            .expect("older transcript");
            backdate(&older, 60);
            let newest = projects.join("newest.jsonl");
            std::fs::write(
                &newest,
                format!(
                    "{}\n{}\n",
                    stamped_line("newest one", Utc::now()),
                    stamped_line("newest two", Utc::now())
                ),
            )
            .expect("newest transcript");
            let newest_len = std::fs::metadata(&newest).expect("newest len").len();

            let mut reader = Reader::start(&projects, transcript_id);
            let mut records = Vec::new();
            while records.len() < 2 {
                match reader.next(Duration::from_secs(10)).await {
                    Some(record) => records.push(record),
                    None => break,
                }
            }
            reader.stop().await;

            assert_eq!(records.len(), 2, "shape={shape:?}");
            for record in records {
                assert_eq!(record.file, newest, "shape={shape:?}");
                assert_eq!(record.origin, RecordOrigin::Preamble, "shape={shape:?}");
                assert_eq!(record.observed_len, newest_len, "shape={shape:?}");
            }
        }
    }

    /// Test 19 (guard): after the pinned first attach, a `/clear` rotation is
    /// still picked up by newest mtime.
    #[tokio::test]
    async fn a_clear_after_the_pinned_attach_is_still_a_rotation() {
        let temp = tempfile::tempdir().expect("temp");
        let projects = temp.path().join("projects");
        std::fs::create_dir_all(&projects).expect("projects dir");

        let mut reader = Reader::start(&projects, Some(PIN_ID.to_string()));
        tokio::time::sleep(Duration::from_millis(700)).await;
        let pinned = projects.join(format!("{PIN_ID}.jsonl"));
        write_atomically(
            &pinned,
            &format!("{}\n", stamped_line("first reply", Utc::now())),
        );
        let first = reader
            .next(Duration::from_secs(10))
            .await
            .expect("the pinned reply reaches the sink");
        assert_eq!(first.file, pinned);

        // The rotation stale guard reads wall-clock mtimes.
        backdate(&pinned, 60);
        tokio::time::sleep(Duration::from_millis(700)).await;
        let cleared = projects.join("after-clear.jsonl");
        write_atomically(
            &cleared,
            &format!("{}\n", stamped_line("after clear", Utc::now())),
        );
        let rotated = reader
            .next(Duration::from_secs(10))
            .await
            .expect("the rotated transcript is read");
        reader.stop().await;

        assert_eq!(rotated.file, cleared);
        assert_eq!(rotated.origin, RecordOrigin::RotationBackfill);
    }

    // Test 20: the fallback fires only AFTER the deadline. Same inputs, the
    // deadline live then expired.
    #[test]
    fn decide_attach_falls_back_only_after_the_deadline() {
        let old = Path::new("old.jsonl");
        assert_eq!(
            decide_attach(inputs(Some(PIN_ID), None, Some(old), false, false)),
            AttachDecision::Wait
        );
        assert_eq!(
            decide_attach(inputs(Some(PIN_ID), None, Some(old), false, true)),
            AttachDecision::MtimeFallback(old.to_path_buf())
        );
    }
}
