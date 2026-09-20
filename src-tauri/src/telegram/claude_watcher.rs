// Claude Code JSONL session-file watcher.
// Polls Claude Code's append-only structured session log for new assistant
// messages and sends them to Telegram, bypassing the PTY-based pipeline.
//
// Shared scaffold (find_latest_jsonl, read_new_lines, polling/rotation
// constants) lives in `jsonl_kernel.rs` — see commit 1 for the extraction.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use tauri::Emitter;
use tokio::sync::mpsc::UnboundedSender;
use tokio::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

use crate::capture::key::{classify_observation, extend_observation, FileObservation};
use crate::capture::record::{CaptureProvider, CapturedRecord, RecordOrigin};
use crate::capture::state::{observation_prefix, OBSERVED_PREFIX_CAP};
use crate::network::OutboundNetwork;
use crate::telegram::jsonl_kernel::{
    find_latest_jsonl, read_new_lines_with_starts, read_preamble_for_race, POLL_INTERVAL_MS,
    ROTATION_STALE_SECS,
};
use crate::telegram::output::{flush_buffer, BridgeLogger, DiagLogger};

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

/// #2232 phase 3: what the reader knows about the file it just read, carried on
/// every record it emits.
///
/// The watcher takes **no lock, performs no disk write and reads no**
/// `state.json`: the observation is built from bytes it has already read, and
/// `capture::state` evaluates the persisted epoch predicate off this thread. A
/// watcher whose sender is `None` computes none of it, so a room without the
/// flag performs zero additional syscalls per poll and the Telegram cadence is
/// unchanged.
#[derive(Clone, Debug, Default)]
struct CaptureAttachment {
    epoch: u64,
    observed_path: PathBuf,
    observed_len: u64,
    observed_prefix: Vec<u8>,
}

/// The reader's own per-file epoch, kept in memory for this reader's lifetime.
///
/// Returning to a file read earlier recovers its epoch, so moving between files
/// never advances one. The authoritative, persisted epoch is
/// [`crate::capture::state::observe`], evaluated by the supervisor from the
/// three observation fields this attaches; this local view exists so the record
/// carries a real epoch instead of phase 1's hardcoded `0`.
///
/// The cut of section 7 is **not** applied here: it is a property of a consumer
/// demand, so the sink applies it (`capture::sink::CaptureSlot::offer`) and the
/// supervisor supersedes its sequence tie-break at reader start and at every
/// re-anchor.
#[derive(Debug, Default)]
struct CaptureObserver {
    files: std::collections::HashMap<PathBuf, (u64, FileObservation)>,
}

impl CaptureObserver {
    /// Record one observation and return the attachment for the records it
    /// produced. `head` carries the file's first bytes only when the read
    /// actually covered offset 0; otherwise there is no prefix evidence and
    /// length alone decides, which is exactly what `classify_observation`
    /// specifies for a zero-length shared range.
    fn observe(&mut self, path: &Path, len: u64, head: &[u8]) -> CaptureAttachment {
        let current = FileObservation::new(len, observation_prefix(head));
        let entry = self
            .files
            .entry(path.to_path_buf())
            .or_insert_with(|| (0, current.clone()));
        if classify_observation(&entry.1, &current).advances_epoch() {
            entry.0 += 1;
            entry.1 = current;
        } else {
            extend_observation(&mut entry.1, &current);
        }
        CaptureAttachment {
            epoch: entry.0,
            observed_path: path.to_path_buf(),
            observed_len: entry.1.len,
            observed_prefix: entry.1.prefix.clone(),
        }
    }
}

/// Rebuild the head of the file from lines the reader already holds, capped at
/// [`OBSERVED_PREFIX_CAP`].
///
/// Only a read that started at offset 0 carries head evidence, and only the
/// lines that begin inside the cap contribute. This is a fingerprint built from
/// bytes already in hand; it never reads the file a second time.
fn head_from_lines(lines: &[(u64, String)], read_start: u64) -> Vec<u8> {
    if read_start > 0 {
        return Vec::new();
    }
    let mut head = Vec::new();
    for (start, line) in lines {
        if usize::try_from(*start).unwrap_or(usize::MAX) >= OBSERVED_PREFIX_CAP {
            break;
        }
        head.extend_from_slice(line.as_bytes());
        head.push(b'\n');
    }
    observation_prefix(&head)
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
    attach: &CaptureAttachment,
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
    attach: &CaptureAttachment,
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
    attach: &CaptureAttachment,
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

async fn watch_loop<R: tauri::Runtime>(
    project_dir: PathBuf,
    network: OutboundNetwork,
    token: String,
    chat_id: i64,
    session_id: String,
    cancel: CancellationToken,
    app: tauri::AppHandle<R>,
) {
    let mut logger = BridgeLogger::new(&session_id);
    let mut diag = DiagLogger::new();
    let mut buffer = String::new();
    let mut last_buffer_add = Instant::now();
    let flush_delay = Duration::from_millis(FLUSH_DELAY_MS);

    // #2232 phase 1: records are built and delivered per accepted JSONL record,
    // but no sink is attached yet — phase 4 passes a real sender into this
    // watcher. With `None` the emit is a no-op and the Telegram path stays
    // byte-identical.
    let capture_tx: Option<UnboundedSender<Arc<CapturedRecord>>> = None;
    let mut reader_seq: u64 = 0;
    // #2232 phase 3: the reader's own epoch and file observation. Both are
    // computed only when a sink is attached, so a room without the flag keeps
    // the pre-#2232 syscall count.
    let mut observer = CaptureObserver::default();
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
                                        // The §J scan reads the tail, so it
                                        // carries no head evidence: length
                                        // alone decides the epoch here.
                                        let attach = if capture_tx.is_some() {
                                            observer.observe(p, file_len, &[])
                                        } else {
                                            CaptureAttachment::default()
                                        };
                                        for record in capture_preamble_bodies(
                                            bodies,
                                            &session_id,
                                            p,
                                            &mut reader_seq,
                                            capture_tx.as_ref(),
                                            &attach,
                                        ) {
                                            logger.log("JSONL_PREAMBLE", &session_id, &record.text);
                                            buffer.push_str(&record.text);
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
                            rotation_backfill_pending = true;
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
                    let read_start = file_offset;
                    match read_new_lines_with_starts(path, &mut file_offset, &mut line_remainder) {
                        Ok(new_lines) => {
                            let attach = if capture_tx.is_some() {
                                let head = head_from_lines(&new_lines, read_start);
                                observer.observe(path, file_offset, &head)
                            } else {
                                CaptureAttachment::default()
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
                                logger.log("JSONL_EXTRACT", &session_id, &record.text);
                                buffer.push_str(&record.text);
                                buffer.push('\n');
                                last_buffer_add = Instant::now();
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
        let read_start = file_offset;
        if let Ok(new_lines) =
            read_new_lines_with_starts(path, &mut file_offset, &mut line_remainder)
        {
            let attach = if capture_tx.is_some() {
                let head = head_from_lines(&new_lines, read_start);
                observer.observe(path, file_offset, &head)
            } else {
                CaptureAttachment::default()
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
                buffer.push_str(&record.text);
                buffer.push('\n');
            }
        }
    }
    if !buffer.is_empty() {
        flush_buffer(
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
            &CaptureAttachment::default(),
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
