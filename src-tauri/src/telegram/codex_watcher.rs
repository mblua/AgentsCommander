// Codex CLI session-file watcher.
//
// Polls `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` files for
// `response_item` records with `payload.type == "message"`,
// `payload.role == "assistant"` and `payload.phase == "final_answer"`, joining
// their `output_text` content blocks into the assistant prose sent to Telegram.
// Historical `event_msg + agent_message` records are deliberately ignored.
// Uses Kernel A (offset-based append-only JSONL) from `jsonl_kernel.rs` once
// `find_session_file` selects the right rollout.

use std::io::Read as IoRead;
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
use crate::commands::codex_resolver::canonicalize_cwd_for_codex;
use crate::network::OutboundNetwork;
use crate::telegram::jsonl_kernel::{
    read_new_lines_with_starts, read_preamble_for_race, POLL_INTERVAL_MS, ROTATION_STALE_SECS,
};
use crate::telegram::output::{flush_buffer, BridgeLogger, DiagLogger};

/// Buffer thresholds for Codex final answers: the extracted prose of a turn is
/// coalesced into one Telegram message once it settles for `FLUSH_DELAY_MS` or
/// grows past `FLUSH_BYTES`.
const FLUSH_DELAY_MS: u64 = 1500;
const FLUSH_BYTES: usize = 3000;

/// Grace window for the M6 day-walk: files older than this aren't considered.
const FILE_MTIME_GRACE_SECS: i64 = 5 * 60;

/// M6 empirical (Codex 0.130.0): `codex resume --last` APPENDS to the prior
/// rollout file. The walk therefore covers today + last 7 UTC days to catch
/// resumes from week-old sessions. See plan §15 §A for the empirical record.
const DAY_WALK_DEPTH: i64 = 7;

/// Where a reader sends Telegram messages (#2232 phase 4 section 4.1).
///
/// One `Option<BotTarget>` and not two separate `Option`s: a single `Option`
/// makes a half-configured send **unrepresentable**, which two `Option`s would
/// only make a convention.
///
/// The struct is declared here, beside the spawn function, and deliberately
/// **not** shared with `claude_watcher`: section 10 forbids either watcher
/// gaining a reference the other does not already have, and
/// `claude_watcher_layering` equality-pins the Claude watcher's dependency set.
/// Two three-line structs are cheaper than an arc between the watchers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BotTarget {
    pub token: String,
    pub chat_id: i64,
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_watch_task<R: tauri::Runtime>(
    search_root: PathBuf,
    expected_cwd: String,
    attach_time: DateTime<Utc>,
    network: OutboundNetwork,
    bot_target: Option<BotTarget>,
    session_id: String,
    cancel: CancellationToken,
    app: tauri::AppHandle<R>,
    sink: Option<UnboundedSender<Arc<CapturedRecord>>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        watch_loop(
            search_root,
            expected_cwd,
            attach_time,
            network,
            bot_target,
            session_id.clone(),
            cancel,
            app.clone(),
            sink,
        )
        .await;
        log::info!("[CODEX_EXIT] Watcher task ended for session {}", session_id);
    })
}

/// Extractor for `read_preamble_for_race`: pairs each emitted body with the
/// line's top-level `timestamp` field so the kernel can apply its grace-window
/// filter. Codex final-answer records have no stable per-line id this watcher
/// dedups on, so the id slot is always `None`.
fn codex_preamble_extractor(line: &str) -> Option<(DateTime<Utc>, Option<String>, String)> {
    let body = extract_assistant_final(line)?;
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let ts_str = v.get("timestamp")?.as_str()?;
    let ts = DateTime::parse_from_rfc3339(ts_str)
        .ok()?
        .with_timezone(&Utc);
    Some((ts, None, body))
}

/// Parse a single Codex rollout JSONL line and extract the current-format
/// final assistant answer body together with its turn id, if any.
///
/// Accepts exactly `type=response_item`, `payload.type=message`,
/// `payload.role=assistant` and `payload.phase=final_answer` (all
/// case-sensitive) with an array `payload.content`. The string `text` values of
/// its `output_text` blocks are joined in array order with a single newline and
/// the result is trimmed of its outer whitespace; `None` is returned when no
/// usable text remains. Individually malformed content blocks and non-string
/// `text` values are skipped without discarding the other valid blocks. Every
/// other record shape — including all `event_msg` records — malformed JSON and
/// missing or wrong fields fail closed to `None`, with no panic, fallback,
/// dedup or per-line diagnostics. The body is never rendered with internal
/// metadata; the only metadata read is `turn_id`.
///
/// The turn id lives at
/// `payload.internal_chat_message_metadata_passthrough.turn_id` (the real
/// fixture below shows the nested location). The flat `turn_id` on
/// `task_complete` is a different location and belongs to phase 5.
fn extract_assistant_final_with_turn(line: &str) -> Option<(String, Option<String>)> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("type")?.as_str()? != "response_item" {
        return None;
    }
    let payload = v.get("payload")?;
    if payload.get("type")?.as_str()? != "message" {
        return None;
    }
    if payload.get("role")?.as_str()? != "assistant" {
        return None;
    }
    if payload.get("phase")?.as_str()? != "final_answer" {
        return None;
    }
    let content = payload.get("content")?.as_array()?;

    let mut parts: Vec<&str> = Vec::new();
    for block in content {
        let Some(obj) = block.as_object() else {
            continue; // non-object block
        };
        if obj.get("type").and_then(|t| t.as_str()) != Some("output_text") {
            continue; // non-output_text block
        }
        if let Some(text) = obj.get("text").and_then(|t| t.as_str()) {
            parts.push(text);
        }
    }
    if parts.is_empty() {
        return None;
    }

    let joined = parts.join("\n");
    let trimmed = joined.trim();
    if trimmed.is_empty() {
        return None;
    }

    let turn_id = payload
        .get("internal_chat_message_metadata_passthrough")
        .and_then(|metadata| metadata.get("turn_id"))
        .and_then(|turn_id| turn_id.as_str())
        .map(str::to_owned);
    Some((trimmed.to_string(), turn_id))
}

/// Text-only view of [`extract_assistant_final_with_turn`].
fn extract_assistant_final(line: &str) -> Option<String> {
    extract_assistant_final_with_turn(line).map(|(text, _)| text)
}

/// Build one [`CapturedRecord`] for an accepted final-answer record.
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
        provider: CaptureProvider::Codex,
        // Codex carries its provider's own `final_answer` marker, so every
        // accepted record is final; only a `turn_id` makes it groupable.
        provider_final: true,
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
        let Some((text, turn_id)) = extract_assistant_final_with_turn(&line) else {
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

/// Open `path`, read up to 64 KiB from the start, return the first complete
/// JSON line. Used by `find_session_file` to inspect each candidate's
/// `session_meta` header without slurping the full rollout (a 2 MB file would
/// be ~30 ms of blocking I/O per candidate otherwise).
fn read_first_line(path: &Path) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; 64 * 1024];
    let n = f.read(&mut buf).ok()?;
    let bytes = &buf[..n];
    let end = bytes
        .iter()
        .position(|&b| b == b'\n')
        .unwrap_or(bytes.len());
    let line = String::from_utf8_lossy(&bytes[..end]).into_owned();
    Some(line)
}

/// Walk today + last 7 UTC days of `search_root`, open each candidate
/// `rollout-*.jsonl` whose mtime is recent enough, parse its first
/// `session_meta` line, and return the path whose `payload.cwd` (after
/// canonicalization) equals the canonicalized `expected_cwd`, preferring the
/// candidate with the newest **file mtime**.
///
/// **mtime, not `payload.timestamp`**: `session_meta.payload.timestamp` is the
/// session CREATION time and is NEVER updated on resume. M6 empirical (Codex
/// 0.130.0) shows `codex resume --last` appends to the original file from the
/// resumed session's creation day — so a 4-day-old file's `payload.timestamp`
/// is 4 days old even when it was just appended to. mtime tracks the
/// most-recently-written file, which is the live one Codex is appending to.
///
/// Returns `None` if no candidate matches; the watcher polls again next tick.
fn find_session_file(
    search_root: &Path,
    expected_cwd: &str,
    attach_time: DateTime<Utc>,
) -> Option<PathBuf> {
    let normalized_expected = canonicalize_cwd_for_codex(expected_cwd);
    let mtime_cutoff = attach_time - chrono::Duration::seconds(FILE_MTIME_GRACE_SECS);
    let mtime_cutoff_st: SystemTime = mtime_cutoff.into();

    let mut best: Option<(PathBuf, SystemTime)> = None;

    for offset in 0..=DAY_WALK_DEPTH {
        let day = attach_time.date_naive() - chrono::Duration::days(offset);
        let dir = search_root
            .join(format!("{:04}", day.format("%Y")))
            .join(format!("{:02}", day.format("%m")))
            .join(format!("{:02}", day.format("%d")));
        let read = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue, // partition missing — normal for days with no codex usage
        };
        for entry in read.flatten() {
            let path = entry.path();
            let fname = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n,
                None => continue,
            };
            if !fname.starts_with("rollout-") || !fname.ends_with(".jsonl") {
                continue;
            }
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            let mtime = match meta.modified() {
                Ok(m) => m,
                Err(_) => continue, // can't compare without mtime
            };
            // Skip files modified more than 5 min before attach (stale).
            if mtime < mtime_cutoff_st {
                continue;
            }
            let first = match read_first_line(&path) {
                Some(l) => l,
                None => continue,
            };
            let v: serde_json::Value = match serde_json::from_str(&first) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let payload = match v.get("payload") {
                Some(p) => p,
                None => continue,
            };
            let candidate_cwd = match payload.get("cwd").and_then(|c| c.as_str()) {
                Some(c) => c,
                None => continue,
            };
            if canonicalize_cwd_for_codex(candidate_cwd) != normalized_expected {
                continue;
            }
            match &best {
                Some((_, best_mtime)) if mtime > *best_mtime => best = Some((path, mtime)),
                None => best = Some((path, mtime)),
                _ => {}
            }
        }
    }

    best.map(|(p, _)| p)
}

#[allow(clippy::too_many_arguments)]
async fn watch_loop<R: tauri::Runtime>(
    search_root: PathBuf,
    expected_cwd: String,
    attach_time: DateTime<Utc>,
    network: OutboundNetwork,
    bot_target: Option<BotTarget>,
    session_id: String,
    cancel: CancellationToken,
    app: tauri::AppHandle<R>,
    sink: Option<UnboundedSender<Arc<CapturedRecord>>>,
) {
    // #2232 phase 4 section 8: with no bot demand no diagnostic is built, so
    // `BridgeLogger::new` — which truncates the **global** diagnostic files
    // (`telegram/output.rs:140`) — is never called for a room-only reader.
    let (token, chat_id) = match &bot_target {
        Some(target) => (target.token.clone(), target.chat_id),
        None => (String::new(), 0),
    };
    let mut logger = bot_target.as_ref().map(|_| BridgeLogger::new(&session_id));
    let mut diag = bot_target.as_ref().map(|_| DiagLogger::new());
    // Log through the bridge logger only when one exists. With no bot demand
    // there is no logger, so `CODEX_EXTRACT` is not written and no global log
    // file is touched (#2232 phase 4 section 8).
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

    // #2232 phase 1: records are built and delivered per accepted JSONL record,
    // but no sink is attached yet — phase 4 passes a real sender into this
    // watcher. With `None` the emit is a no-op and the Telegram path stays
    // byte-identical.
    // #2232 phase 4 section 4.2: the supervisor passes the live sender in, so a
    // room-only reader emits into `CaptureRegistry` with no bot anywhere.
    let capture_tx: Option<UnboundedSender<Arc<CapturedRecord>>> = sink;
    let mut reader_seq: u64 = 0;
    // #2232 phase 3: the reader's own epoch and file observation, computed only
    // when a sink is attached. Codex re-anchors at the new file's EOF on
    // rotation and therefore never replays, so it has no rotation backfill to
    // mark; the Claude reader, which reads a rotated file from zero, does.
    let mut observer = ReaderObservations::default();

    let mut current_file: Option<PathBuf> = None;
    let mut current_file_mtime: Option<SystemTime> = None;
    let mut last_mtime_advance: Instant = Instant::now();
    let mut file_offset: u64 = 0;
    let mut line_remainder = String::new();
    let mut search_warned = false;

    bridge_log!(
        "CODEX_INIT",
        &format!(
            "search_root={} expected_cwd={}",
            search_root.display(),
            expected_cwd
        )
    );

    let mut poll_interval = tokio::time::interval(Duration::from_millis(POLL_INTERVAL_MS));
    poll_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = poll_interval.tick() => {
                // M5: re-scan only when we don't have a current file, the tracked
                // file has been unlinked, or the file's mtime has not advanced
                // for ROTATION_STALE_SECS wall-clock seconds (i.e. it might have
                // been rotated out from under us). `last_mtime_advance` is
                // updated below when we observe the file's current mtime grow.
                let need_rescan = match &current_file {
                    None => true,
                    Some(p) if !p.exists() => true,
                    Some(_) => last_mtime_advance.elapsed().as_secs() >= ROTATION_STALE_SECS,
                };

                if need_rescan {
                    if let Some(found) = find_session_file(&search_root, &expected_cwd, attach_time) {
                        if Some(&found) != current_file.as_ref() {
                            // First bind OR rotation. On first bind, run the §J
                            // preamble scan to emit any final assistant answer from
                            // the file's tail with timestamp >= attach_time - 5s.
                            // Then set offset = file_len.
                            let first_bind = current_file.is_none();
                            line_remainder.clear();
                            if first_bind {
                                match read_preamble_for_race(&found, attach_time, codex_preamble_extractor) {
                                    Ok((bodies, _ids, file_len)) => {
                                        // The §J scan reads the tail, so it
                                        // carries no head evidence: length
                                        // alone decides the epoch here.
                                        let attach = if capture_tx.is_some() {
                                            observer.observe(&found, file_len, Vec::new())
                                        } else {
                                            ReaderAttachment::default()
                                        };
                                        for record in capture_preamble_bodies(
                                            bodies,
                                            &session_id,
                                            &found,
                                            &mut reader_seq,
                                            capture_tx.as_ref(),
                                            &attach,
                                        ) {
                                            bridge_log!("CODEX_PREAMBLE", &record.text);
                                            if bot_target.is_some() {
                                                buffer.push_str(&record.text);
                                                buffer.push('\n');
                                                last_buffer_add = Instant::now();
                                            }
                                        }
                                        file_offset = file_len;
                                        bridge_log!("CODEX_FILE", &format!("bound to {}, preamble done, offset={}", found.display(), file_offset));
                                    }
                                    Err(e) => {
                                        bridge_log!("CODEX_ERR", &format!("preamble scan failed: {}", e));
                                        file_offset = std::fs::metadata(&found).ok().map(|m| m.len()).unwrap_or(0);
                                    }
                                }
                            } else {
                                // Rotation. Re-anchor at the new file's current EOF.
                                file_offset = std::fs::metadata(&found).ok().map(|m| m.len()).unwrap_or(0);
                                bridge_log!("CODEX_ROTATE", &format!("rotated to {}, offset={}", found.display(), file_offset));
                            }
                            current_file = Some(found);
                        }
                        let new_mtime = current_file.as_ref()
                            .and_then(|p| std::fs::metadata(p).ok())
                            .and_then(|m| m.modified().ok());
                        if new_mtime != current_file_mtime {
                            last_mtime_advance = Instant::now();
                        }
                        current_file_mtime = new_mtime;
                    } else if !search_warned {
                        bridge_log!("CODEX_WAIT", "no rollout matching cwd found yet");
                        search_warned = true;
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
                            for record in capture_live_lines(
                                new_lines,
                                &session_id,
                                path,
                                &mut reader_seq,
                                capture_tx.as_ref(),
                                RecordOrigin::Live,
                                &attach,
                            ) {
                                bridge_log!("CODEX_EXTRACT", &record.text);
                                if bot_target.is_some() {
                                    buffer.push_str(&record.text);
                                    buffer.push('\n');
                                    last_buffer_add = Instant::now();
                                }
                            }
                            let new_mtime = std::fs::metadata(path).ok()
                                .and_then(|m| m.modified().ok());
                            if new_mtime != current_file_mtime {
                                last_mtime_advance = Instant::now();
                            }
                            current_file_mtime = new_mtime;
                        }
                        Err(e) => {
                            bridge_log!("CODEX_ERR", &e.to_string());
                            log::error!("[CODEX_ERR] Read error for session {}: {}", session_id, e);
                            let _ = app.emit(
                                "telegram_bridge_error",
                                serde_json::json!({
                                    "sessionId": session_id,
                                    "error": format!("Codex JSONL read error: {}", e),
                                }),
                            );
                        }
                    }
                }

                if !buffer.is_empty() {
                    let elapsed = last_buffer_add.elapsed();
                    if elapsed >= flush_delay || buffer.len() > FLUSH_BYTES {
                        if let (Some(bridge_logger), Some(diag_logger)) =
                            (logger.as_mut(), diag.as_mut())
                        {
                            flush_buffer(
                                &mut buffer, &network, &token, chat_id,
                                &session_id, &app, bridge_logger, diag_logger,
                                true,
                            ).await;
                        }
                    }
                }
            }
        }
    }

    // Final poll + flush after cancel.
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
            for record in capture_live_lines(
                new_lines,
                &session_id,
                path,
                &mut reader_seq,
                capture_tx.as_ref(),
                RecordOrigin::Live,
                &attach,
            ) {
                if bot_target.is_some() {
                    buffer.push_str(&record.text);
                    buffer.push('\n');
                }
            }
        }
    }
    if !buffer.is_empty() {
        if let (Some(bridge_logger), Some(diag_logger)) = (logger.as_mut(), diag.as_mut()) {
            flush_buffer(
                &mut buffer,
                &network,
                &token,
                chat_id,
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
    // The extractor tests drive the kernel directly; production now reads
    // through `read_new_lines_with_starts` (`#2232` phase 1), so the wrapper
    // is imported here instead of by the module's production import list.
    use crate::telegram::jsonl_kernel::read_new_lines;
    use std::fs;
    use std::io::Write;

    const SESSION_META_TEMPLATE: &str = r#"{"timestamp":"{ts}","type":"session_meta","payload":{"id":"{id}","cwd":"{cwd}","timestamp":"{ts}"}}"#;

    fn write_rollout(dir: &Path, name: &str, cwd: &str, ts: &str) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        let line = SESSION_META_TEMPLATE
            .replace("{ts}", ts)
            .replace("{cwd}", cwd)
            .replace("{id}", "test-uuid");
        let mut f = fs::File::create(&path).unwrap();
        writeln!(f, "{}", line).unwrap();
        path
    }

    /// Byte-exact sanitized copy of the real Codex 0.154.0 `final_answer`
    /// record captured at rollout byte offset 87513 (room-shared
    /// `codex-real-assistant-sanitized.jsonl`, SHA256
    /// 76d5a755f02e62242c6b8ce7804a5ebf9322e66d14785d0109dba45dd6f70498).
    const REAL_CURRENT_CODEX_FINAL: &str = r#"{"timestamp":"2026-09-13T18:28:25.938Z","ordinal":12,"type":"response_item","payload":{"type":"message","id":"msg_07770767cb7cb624016aa6eb4a72d487d2bcf644a579754670","role":"assistant","content":[{"type":"output_text","text":"sanitized assistant reply"}],"phase":"final_answer","internal_chat_message_metadata_passthrough":{"turn_id":"01a09c07-0ad4-7710-9c1d-ce3f8dd0aaac","create_time":1789324104.823481,"content_item_kinds":["unknown"]}}}"#;

    /// Payload of a valid current-format record.
    fn valid_final_payload() -> serde_json::Value {
        serde_json::json!({
            "type": "message",
            "role": "assistant",
            "phase": "final_answer",
            "content": [{"type": "output_text", "text": "body"}]
        })
    }

    /// A valid current-format record with every field the extractor requires.
    fn valid_final_value() -> serde_json::Value {
        serde_json::json!({
            "type": "response_item",
            "payload": valid_final_payload()
        })
    }

    /// Synthetic current-format record carrying the given `output_text` texts.
    fn current_final_record(texts: &[&str]) -> String {
        let content: Vec<serde_json::Value> = texts
            .iter()
            .map(|t| serde_json::json!({"type": "output_text", "text": t}))
            .collect();
        let mut v = valid_final_value();
        v["payload"]["content"] = serde_json::Value::Array(content);
        v.to_string()
    }

    /// Synthetic current-format record with an explicit top-level `timestamp`;
    /// `None` omits the field.
    fn current_final_record_with_ts(text: &str, ts: Option<&str>) -> String {
        let mut v = valid_final_value();
        v["payload"]["content"] = serde_json::json!([{"type": "output_text", "text": text}]);
        if let Some(ts) = ts {
            v["timestamp"] = serde_json::json!(ts);
        }
        v.to_string()
    }

    /// Extract the assistant bodies of already-read JSONL lines, exactly as
    /// the watcher's incremental and final-drain call sites do.
    fn extract_bodies(lines: &[String]) -> Vec<String> {
        lines
            .iter()
            .filter_map(|l| extract_assistant_final(l.as_str()))
            .collect()
    }

    // ── extract_assistant_final: accepted current format ──────────────────

    #[test]
    fn real_current_codex_final() {
        assert_eq!(
            extract_assistant_final(REAL_CURRENT_CODEX_FINAL),
            Some("sanitized assistant reply".to_string())
        );
    }

    #[test]
    fn extract_assistant_final_accepts_synthetic_positive_control() {
        // Independent synthetic record (not the real fixture) exercising the
        // accepted path: response_item / message / assistant / final_answer.
        let line = current_final_record(&["Synthetic final answer."]);
        assert_eq!(
            extract_assistant_final(&line),
            Some("Synthetic final answer.".to_string())
        );
    }

    #[test]
    fn extract_assistant_final_rejects_task_and_tool_records() {
        // Negative controls: task lifecycle and tool traffic must stay silent.
        let task_started = serde_json::json!({
            "timestamp": "2026-09-13T18:28:09.828Z",
            "type": "event_msg",
            "payload": {"type": "task_started"}
        })
        .to_string();
        let task_complete = serde_json::json!({
            "type": "event_msg",
            "payload": {"type": "task_complete", "last_agent_message": "x"}
        })
        .to_string();
        let tool_call = serde_json::json!({
            "type": "response_item",
            "payload": {"type": "function_call", "name": "shell", "arguments": "{}"}
        })
        .to_string();
        let tool_output = serde_json::json!({
            "type": "response_item",
            "payload": {"type": "function_call_output", "call_id": "c1", "output": "ok"}
        })
        .to_string();
        for (label, line) in [
            ("task_started", task_started),
            ("task_complete", task_complete),
            ("function_call", tool_call),
            ("function_call_output", tool_output),
        ] {
            assert_eq!(extract_assistant_final(&line), None, "{}", label);
        }
    }

    #[test]
    fn extract_assistant_final_handles_compact_and_spaced_json() {
        let compact = current_final_record(&["body"]);
        let spaced = r#"{
            "type" : "response_item",
            "payload" : {
                "type" : "message",
                "role" : "assistant",
                "phase" : "final_answer",
                "content" : [ { "type" : "output_text", "text" : "body" } ]
            }
        }"#;
        assert_eq!(extract_assistant_final(&compact), Some("body".to_string()));
        assert_eq!(extract_assistant_final(spaced), Some("body".to_string()));
    }

    #[test]
    fn extract_assistant_final_joins_text_blocks_in_array_order() {
        let line = current_final_record(&["first block", "second block", "third block"]);
        assert_eq!(
            extract_assistant_final(&line),
            Some("first block\nsecond block\nthird block".to_string())
        );
        // Only the outer whitespace of the joined result is trimmed.
        let line = current_final_record(&["  padded  "]);
        assert_eq!(extract_assistant_final(&line), Some("padded".to_string()));
    }

    #[test]
    fn extract_assistant_final_skips_malformed_blocks_but_keeps_valid_ones() {
        let mut v = valid_final_value();
        v["payload"]["content"] = serde_json::json!([
            {"type": "output_text", "text": "kept one"},
            null,
            42,
            "a bare string",
            {"type": "input_text", "text": "not output_text"},
            {"type": 7, "text": "non-string type"},
            {"type": "output_text", "text": 99},
            {"type": "output_text"},
            {"type": "output_text", "text": "kept two"},
        ]);
        assert_eq!(
            extract_assistant_final(&v.to_string()),
            Some("kept one\nkept two".to_string())
        );
    }

    #[test]
    fn extract_assistant_final_returns_none_for_empty_or_all_invalid_content() {
        let cases: Vec<serde_json::Value> = vec![
            serde_json::json!([]),
            serde_json::json!([null, 1, "text"]),
            serde_json::json!([{"type": "input_text", "text": "x"}]),
            serde_json::json!([{"type": "output_text", "text": ""}]),
            serde_json::json!([{"type": "output_text", "text": "   \n\t "}]),
        ];
        for content in cases {
            let mut v = valid_final_value();
            v["payload"]["content"] = content;
            assert_eq!(
                extract_assistant_final(&v.to_string()),
                None,
                "content={}",
                v["payload"]["content"]
            );
        }
    }

    #[test]
    fn extract_assistant_final_returns_none_for_malformed_json() {
        for line in [
            "",
            "not json at all",
            "{\"type\":\"response_item\"",
            "null",
            "[1,2,3]",
            "{ this is not JSON }",
        ] {
            assert_eq!(extract_assistant_final(line), None, "line={:?}", line);
        }
    }

    #[test]
    fn extract_assistant_final_rejects_every_other_type_role_and_phase() {
        // Top-level types other than `response_item` (event_msg included).
        for kind in [
            "session_meta",
            "event_msg",
            "turn_context",
            "world_state",
            "token_usage_record",
            "compacted",
            "Response_Item",
        ] {
            let line =
                serde_json::json!({"type": kind, "payload": valid_final_payload()}).to_string();
            assert_eq!(extract_assistant_final(&line), None, "type={}", kind);
        }

        // Payload types other than `message`.
        for ptype in [
            "reasoning",
            "function_call",
            "function_call_output",
            "input_text",
            "output_text",
            "message_summary",
            "Message",
        ] {
            let line = serde_json::json!({
                "type": "response_item",
                "payload": {"type": ptype, "role": "assistant", "phase": "final_answer",
                            "content": [{"type": "output_text", "text": "body"}]}
            })
            .to_string();
            assert_eq!(
                extract_assistant_final(&line),
                None,
                "payload.type={}",
                ptype
            );
        }

        // Roles other than `assistant`.
        for role in ["user", "system", "developer", "tool", "Assistant", ""] {
            let mut payload = valid_final_payload();
            payload["role"] = serde_json::json!(role);
            let line = serde_json::json!({"type": "response_item", "payload": payload}).to_string();
            assert_eq!(extract_assistant_final(&line), None, "role={}", role);
        }

        // Phases other than `final_answer`.
        for phase in [
            "commentary",
            "analysis",
            "final",
            "Final_Answer",
            "final_answer ",
            "",
        ] {
            let mut payload = valid_final_payload();
            payload["phase"] = serde_json::json!(phase);
            let line = serde_json::json!({"type": "response_item", "payload": payload}).to_string();
            assert_eq!(extract_assistant_final(&line), None, "phase={}", phase);
        }
        // Missing phase entirely.
        let mut payload = valid_final_payload();
        payload.as_object_mut().unwrap().remove("phase");
        let line = serde_json::json!({"type": "response_item", "payload": payload}).to_string();
        assert_eq!(extract_assistant_final(&line), None, "phase absent");
    }

    #[test]
    fn extract_assistant_final_rejects_absent_or_non_string_fields() {
        // Missing / non-string top-level type.
        let mut v = valid_final_value();
        v.as_object_mut().unwrap().remove("type");
        assert_eq!(extract_assistant_final(&v.to_string()), None, "type absent");
        let mut v = valid_final_value();
        v["type"] = serde_json::json!(7);
        assert_eq!(
            extract_assistant_final(&v.to_string()),
            None,
            "type non-string"
        );

        // Missing / non-object payload.
        let mut v = valid_final_value();
        v.as_object_mut().unwrap().remove("payload");
        assert_eq!(
            extract_assistant_final(&v.to_string()),
            None,
            "payload absent"
        );
        let mut v = valid_final_value();
        v["payload"] = serde_json::json!("not an object");
        assert_eq!(
            extract_assistant_final(&v.to_string()),
            None,
            "payload non-object"
        );

        // Missing payload fields.
        for field in ["type", "role", "phase", "content"] {
            let mut payload = valid_final_payload();
            payload.as_object_mut().unwrap().remove(field);
            let line = serde_json::json!({"type": "response_item", "payload": payload}).to_string();
            assert_eq!(
                extract_assistant_final(&line),
                None,
                "payload.{} absent",
                field
            );
        }

        // Non-string payload fields.
        for field in ["type", "role", "phase"] {
            let mut payload = valid_final_payload();
            payload[field] = serde_json::json!(["not", "a", "string"]);
            let line = serde_json::json!({"type": "response_item", "payload": payload}).to_string();
            assert_eq!(
                extract_assistant_final(&line),
                None,
                "payload.{} non-string",
                field
            );
        }

        // Content not an array.
        for content in [
            serde_json::json!("body"),
            serde_json::json!({"type": "output_text"}),
            serde_json::json!(3),
        ] {
            let mut payload = valid_final_payload();
            payload["content"] = content;
            let line = serde_json::json!({"type": "response_item", "payload": payload}).to_string();
            assert_eq!(extract_assistant_final(&line), None, "content non-array");
        }
    }

    #[test]
    fn extract_assistant_final_rejects_legacy_event_msg_records() {
        // Historical Codex format: event_msg / agent_message. Rejected even
        // when its phase string matches the current vocabulary.
        for phase in ["commentary", "final", "final_answer"] {
            let line = serde_json::json!({
                "timestamp": "2026-05-19T05:00:00Z",
                "type": "event_msg",
                "payload": {"type": "agent_message", "message": "  legacy prose  ", "phase": phase}
            })
            .to_string();
            assert_eq!(
                extract_assistant_final(&line),
                None,
                "legacy phase={}",
                phase
            );
        }
        // Legacy envelope carrying response-shaped payload fields.
        let line = serde_json::json!({
            "type": "event_msg",
            "payload": {"type": "agent_message", "role": "assistant", "phase": "final_answer",
                        "content": [{"type": "output_text", "text": "legacy-shaped"}]}
        })
        .to_string();
        assert_eq!(
            extract_assistant_final(&line),
            None,
            "event_msg with response fields"
        );
    }

    #[test]
    fn extraction_is_independent_of_session_metadata_variants() {
        let base = current_final_record(&["stable body"]);
        let mut with_meta = serde_json::from_str::<serde_json::Value>(&base).unwrap();
        with_meta["cli_version"] = serde_json::json!("0.154.0");
        with_meta["originator"] = serde_json::json!("codex-tui");
        let mut changed_meta = with_meta.clone();
        changed_meta["cli_version"] = serde_json::json!("9.9.9");
        changed_meta["originator"] = serde_json::json!("other-originator");
        for line in [base, with_meta.to_string(), changed_meta.to_string()] {
            assert_eq!(
                extract_assistant_final(&line),
                Some("stable body".to_string()),
                "line={}",
                line
            );
        }
    }

    // ── extract_assistant_final through the kernel watch paths ────────────

    #[test]
    fn preamble_emits_fresh_final_answer_and_incremental_read_does_not_repeat() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rollout-preamble.jsonl");
        let now = Utc::now();
        // Real fixture bytes with the timestamp rewritten into the grace
        // window; the JSON shape and sanitized text are preserved.
        let fresh_ts = (now - chrono::Duration::seconds(1)).to_rfc3339();
        let fresh = REAL_CURRENT_CODEX_FINAL.replace("2026-09-13T18:28:25.938Z", &fresh_ts);
        assert!(
            fresh.contains(&fresh_ts),
            "fixture timestamp must be rewritten"
        );
        let mut f = fs::File::create(&path).unwrap();
        writeln!(f, "{}", fresh).unwrap();
        drop(f);

        let (bodies, ids, file_len) =
            read_preamble_for_race(&path, now, codex_preamble_extractor).unwrap();
        assert_eq!(bodies, vec!["sanitized assistant reply".to_string()]);
        assert_eq!(ids, vec![None]);
        assert_eq!(file_len, fs::metadata(&path).unwrap().len());

        // The first bind advances the consumer to EOF: the next incremental
        // read must not repeat the consumed record.
        let mut offset = file_len;
        let mut remainder = String::new();
        let lines = read_new_lines(&path, &mut offset, &mut remainder).unwrap();
        assert!(lines.is_empty(), "consumed preamble must not replay");
    }

    #[test]
    fn preamble_rejects_missing_invalid_and_out_of_grace_timestamps() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rollout-preamble-reject.jsonl");
        let now = Utc::now();

        let fresh_ts = (now - chrono::Duration::seconds(1)).to_rfc3339();
        let stale_ts = (now - chrono::Duration::seconds(30)).to_rfc3339();
        let fresh = current_final_record_with_ts("fresh body", Some(&fresh_ts));
        let stale = current_final_record_with_ts("stale body", Some(&stale_ts));
        let invalid = current_final_record_with_ts("invalid body", Some("not-a-timestamp"));
        let missing = current_final_record_with_ts("missing body", None);

        let mut f = fs::File::create(&path).unwrap();
        for line in [&stale, &invalid, &missing, &fresh] {
            writeln!(f, "{}", line).unwrap();
        }
        drop(f);

        let (bodies, _ids, file_len) =
            read_preamble_for_race(&path, now, codex_preamble_extractor).unwrap();
        assert_eq!(
            bodies,
            vec!["fresh body".to_string()],
            "only the in-grace record may be emitted"
        );
        assert_eq!(file_len, fs::metadata(&path).unwrap().len());

        // Offset moves to EOF, so the rejected records are not replayed later
        // either.
        let mut offset = file_len;
        let mut remainder = String::new();
        let lines = read_new_lines(&path, &mut offset, &mut remainder).unwrap();
        assert!(
            lines.is_empty(),
            "rejected preamble records must not replay"
        );
    }

    #[test]
    fn incremental_read_completes_a_split_json_line_once() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rollout-split.jsonl");
        let line = current_final_record(&["split body"]);
        let (head, tail) = line.split_at(line.len() / 2);

        let mut f = fs::File::create(&path).unwrap();
        f.write_all(head.as_bytes()).unwrap();
        drop(f);

        let mut offset = 0u64;
        let mut remainder = String::new();
        let lines = read_new_lines(&path, &mut offset, &mut remainder).unwrap();
        assert!(lines.is_empty(), "partial JSON must not produce a record");
        assert_eq!(offset, head.len() as u64);

        // Complete the line; the record is extracted exactly once.
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(tail.as_bytes()).unwrap();
        f.write_all(b"\n").unwrap();
        drop(f);

        let lines = read_new_lines(&path, &mut offset, &mut remainder).unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(extract_bodies(&lines), vec!["split body".to_string()]);
        assert_eq!(offset, fs::metadata(&path).unwrap().len());

        // A further poll (the same path the final drain uses) finds nothing.
        let lines = read_new_lines(&path, &mut offset, &mut remainder).unwrap();
        assert!(lines.is_empty(), "completed record must not repeat");
    }

    #[test]
    fn final_drain_extracts_complete_appended_records_once() {
        // Mirrors the cancel path: the final poll drains the complete records
        // appended since the last incremental read.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rollout-drain.jsonl");
        let first = current_final_record(&["drain one"]);
        let second = current_final_record(&["drain two"]);

        let mut offset = 0u64;
        let mut remainder = String::new();
        let mut f = fs::File::create(&path).unwrap();
        write!(f, "{}\n{}\n", first, second).unwrap();
        drop(f);

        let lines = read_new_lines(&path, &mut offset, &mut remainder).unwrap();
        assert_eq!(
            extract_bodies(&lines),
            vec!["drain one".to_string(), "drain two".to_string()]
        );
        assert_eq!(offset, fs::metadata(&path).unwrap().len());

        let lines = read_new_lines(&path, &mut offset, &mut remainder).unwrap();
        assert!(lines.is_empty(), "drained records must not repeat");
    }

    #[test]
    fn coexisting_legacy_and_current_records_emit_only_the_response_body() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rollout-coexist.jsonl");
        let legacy = |msg: &str| {
            serde_json::json!({
                "type": "event_msg",
                "payload": {"type": "agent_message", "message": msg, "phase": "final"}
            })
            .to_string()
        };
        let current = current_final_record(&["current body"]);
        let mut offset = 0u64;
        let mut remainder = String::new();

        // Order 1: legacy before current in the same read.
        let mut f = fs::File::create(&path).unwrap();
        write!(f, "{}\n{}\n", legacy("legacy first"), current).unwrap();
        drop(f);
        let mut buffer =
            extract_bodies(&read_new_lines(&path, &mut offset, &mut remainder).unwrap());
        assert_eq!(buffer, vec!["current body".to_string()]);

        // Simulated flush: the Telegram buffer was cleared, then more records
        // arrive. Nothing already consumed may be re-emitted.
        buffer.clear();

        // Order 2: current before legacy, appended after the flush.
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        write!(f, "{}\n{}\n", current, legacy("legacy second")).unwrap();
        drop(f);
        buffer.extend(extract_bodies(
            &read_new_lines(&path, &mut offset, &mut remainder).unwrap(),
        ));
        assert_eq!(buffer, vec!["current body".to_string()]);
    }

    #[test]
    fn two_identical_final_records_remain_two_bodies() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rollout-dupe.jsonl");
        let record = current_final_record(&["identical body"]);
        let mut offset = 0u64;
        let mut remainder = String::new();

        let mut f = fs::File::create(&path).unwrap();
        write!(f, "{}\n{}\n", record, record).unwrap();
        drop(f);

        let lines = read_new_lines(&path, &mut offset, &mut remainder).unwrap();
        assert_eq!(
            extract_bodies(&lines),
            vec!["identical body".to_string(), "identical body".to_string()],
            "no text dedup may collapse two distinct records"
        );
    }

    /// Appends synthetic, never-valid filler records until the file reaches
    /// `target_len` bytes, always ending on a line boundary.
    fn write_synthetic_filler(f: &mut fs::File, target_len: u64) {
        const LINE: &str = "synthetic filler for issue 1997 geometry (not a codex record)\n";
        let mut written = f.metadata().unwrap().len();
        while written + LINE.len() as u64 <= target_len {
            f.write_all(LINE.as_bytes()).unwrap();
            written += LINE.len() as u64;
        }
        if written < target_len {
            let rem = (target_len - written) as usize;
            let mut last = "x".repeat(rem - 1);
            last.push('\n');
            f.write_all(last.as_bytes()).unwrap();
            written += rem as u64;
        }
        assert_eq!(written, target_len);
        f.flush().unwrap();
    }

    #[test]
    fn replay_prefix_geometry_extracts_the_real_fixture_once() {
        // Real rollout geometry: the watcher binds at byte 22352 and the real
        // final answer lands at byte 87513. The filler is synthetic; the
        // record is the byte-exact real fixture.
        const BIND_OFFSET: u64 = 22_352;
        const FIXTURE_OFFSET: u64 = 87_513;

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rollout-geometry.jsonl");
        let mut f = fs::File::create(&path).unwrap();
        write_synthetic_filler(&mut f, BIND_OFFSET);

        // First bind: the preamble scan leaves the consumer at EOF.
        let mut offset = f.metadata().unwrap().len();
        assert_eq!(offset, BIND_OFFSET);
        let mut remainder = String::new();

        // Filler arrives up to the byte where the real answer starts.
        write_synthetic_filler(&mut f, FIXTURE_OFFSET);
        let lines = read_new_lines(&path, &mut offset, &mut remainder).unwrap();
        assert!(
            extract_bodies(&lines).is_empty(),
            "synthetic filler must never be extracted"
        );
        assert_eq!(offset, FIXTURE_OFFSET, "offset advances with the filler");

        // The real final answer arrives.
        f.write_all(REAL_CURRENT_CODEX_FINAL.as_bytes()).unwrap();
        f.write_all(b"\n").unwrap();
        f.flush().unwrap();

        let lines = read_new_lines(&path, &mut offset, &mut remainder).unwrap();
        assert_eq!(
            extract_bodies(&lines),
            vec!["sanitized assistant reply".to_string()]
        );
        assert_eq!(
            offset,
            FIXTURE_OFFSET + REAL_CURRENT_CODEX_FINAL.len() as u64 + 1,
            "offset advances past the consumed record"
        );

        // Next poll: nothing new and nothing replayed.
        let lines = read_new_lines(&path, &mut offset, &mut remainder).unwrap();
        assert!(
            extract_bodies(&lines).is_empty(),
            "record must not replay on the next poll"
        );
    }

    #[test]
    fn rotation_reanchor_and_truncation_do_not_replay_consumed_data() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rollout-rotation.jsonl");
        let mut offset = 0u64;
        let mut remainder = String::new();

        let mut f = fs::File::create(&path).unwrap();
        writeln!(f, "{}", REAL_CURRENT_CODEX_FINAL).unwrap();
        drop(f);
        let lines = read_new_lines(&path, &mut offset, &mut remainder).unwrap();
        assert_eq!(
            extract_bodies(&lines),
            vec!["sanitized assistant reply".to_string()]
        );

        // Rotation: the watcher re-anchors at the new file's EOF instead of
        // replaying it.
        let mut rotated_offset = fs::metadata(&path).unwrap().len();
        let lines = read_new_lines(&path, &mut rotated_offset, &mut remainder).unwrap();
        assert!(lines.is_empty(), "rotation re-anchor must not replay");

        // Truncation: the kernel re-anchors to the new EOF, skipping replay.
        let mut f = fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        writeln!(f, "{{ truncated }}").unwrap();
        drop(f);
        let new_len = fs::metadata(&path).unwrap().len();
        assert!(new_len < offset, "truncation must shrink the file");
        let lines = read_new_lines(&path, &mut offset, &mut remainder).unwrap();
        assert!(
            lines.is_empty(),
            "truncation must not replay consumed records"
        );
        assert_eq!(offset, new_len, "offset re-anchors to the new EOF");

        // A record appended after truncation is extracted normally.
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(f, "{}", REAL_CURRENT_CODEX_FINAL).unwrap();
        drop(f);
        let lines = read_new_lines(&path, &mut offset, &mut remainder).unwrap();
        assert_eq!(
            extract_bodies(&lines),
            vec!["sanitized assistant reply".to_string()]
        );
    }

    // ── find_session_file ─────────────────────────────────────────────────

    fn day_dir(root: &Path, day: chrono::NaiveDate) -> PathBuf {
        root.join(format!("{:04}", day.format("%Y")))
            .join(format!("{:02}", day.format("%m")))
            .join(format!("{:02}", day.format("%d")))
    }

    #[test]
    fn find_session_file_matches_by_cwd_canonicalization() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let now = Utc::now();
        let today = day_dir(root, now.date_naive());

        let cwd_a = r"C:\Users\foo\bar";
        let cwd_b = r"C:\Users\foo\baz";

        let expected_a = write_rollout(
            &today,
            "rollout-a.jsonl",
            &cwd_a.replace('\\', "\\\\"),
            &now.to_rfc3339(),
        );
        let _b = write_rollout(
            &today,
            "rollout-b.jsonl",
            &cwd_b.replace('\\', "\\\\"),
            &now.to_rfc3339(),
        );

        let found = find_session_file(root, "c:\\users\\foo\\bar", now);
        assert_eq!(found, Some(expected_a));
    }

    #[test]
    fn find_session_file_matches_forward_slash_ac_input_against_backslash_codex_input() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let now = Utc::now();
        let today = day_dir(root, now.date_naive());

        // Codex writes backslashes in cwd; AC passes forward slashes.
        let codex_cwd = r"C:\Users\foo\bar";
        let ac_cwd = "C:/Users/foo/bar";

        let expected = write_rollout(
            &today,
            "rollout-fwd.jsonl",
            &codex_cwd.replace('\\', "\\\\"),
            &now.to_rfc3339(),
        );

        let found = find_session_file(root, ac_cwd, now);
        assert_eq!(found, Some(expected));
    }

    #[test]
    fn find_session_file_picks_newest_mtime_when_multiple_match() {
        // M6: tiebreaker is file mtime, not payload.timestamp — see
        // find_session_file doc comment for why.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let now = Utc::now();
        let today = day_dir(root, now.date_naive());

        let cwd = r"C:\Users\foo\bar";
        // Both files reference the same cwd; payload.timestamp values are not
        // used for tie-breaking. Sleep 150 ms between writes so the two
        // candidates have definitely-distinct mtimes on Windows NTFS.
        let ts = now.to_rfc3339();
        let _old = write_rollout(&today, "rollout-old.jsonl", &cwd.replace('\\', "\\\\"), &ts);
        std::thread::sleep(std::time::Duration::from_millis(150));
        let expected_new =
            write_rollout(&today, "rollout-new.jsonl", &cwd.replace('\\', "\\\\"), &ts);

        let found = find_session_file(root, "c:\\users\\foo\\bar", now);
        assert_eq!(found, Some(expected_new));
    }

    #[test]
    fn find_session_file_picks_resumed_file_even_with_old_payload_timestamp() {
        // M6 use case: codex exec resume --last appends to a 4-day-old file.
        // The candidate has payload.timestamp = 4 days ago but mtime = now.
        // Another (older session, abandoned) candidate exists with both
        // payload.timestamp and mtime "today (earlier)".
        // The mtime tiebreaker should pick the resumed file (current mtime),
        // NOT the abandoned same-day file.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let now = Utc::now();
        let cwd = r"C:\Users\foo\bar";

        // Abandoned same-day session — its payload.timestamp is "today" but
        // it was last touched a couple of minutes ago.
        let today = day_dir(root, now.date_naive());
        let abandoned_today = write_rollout(
            &today,
            "rollout-abandoned.jsonl",
            &cwd.replace('\\', "\\\\"),
            &(now - chrono::Duration::minutes(2)).to_rfc3339(),
        );
        std::thread::sleep(std::time::Duration::from_millis(150));

        // Resumed file lives 4 days back — its payload.timestamp is 4 days ago
        // but its mtime is "now" (we just wrote it = the resume just appended).
        let four_days_ago = now - chrono::Duration::days(4);
        let dir4 = day_dir(root, four_days_ago.date_naive());
        let resumed = write_rollout(
            &dir4,
            "rollout-resumed-4d.jsonl",
            &cwd.replace('\\', "\\\\"),
            &four_days_ago.to_rfc3339(),
        );

        let found = find_session_file(root, "c:\\users\\foo\\bar", now);
        assert_eq!(found, Some(resumed));
        // The abandoned file should NOT have been chosen.
        assert_ne!(found, Some(abandoned_today));
    }

    #[test]
    fn find_session_file_accepts_fresh_mtime_files() {
        // A file created "now" must NOT be skipped by the mtime grace window.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let now = Utc::now();
        let today = day_dir(root, now.date_naive());

        let cwd = r"C:\Users\foo\bar";
        let expected = write_rollout(
            &today,
            "rollout-fresh.jsonl",
            &cwd.replace('\\', "\\\\"),
            &now.to_rfc3339(),
        );
        let found = find_session_file(root, "c:\\users\\foo\\bar", now);
        assert_eq!(found, Some(expected));
    }

    #[test]
    fn find_session_file_skips_files_without_rollout_prefix() {
        // Files whose name doesn't match `rollout-*.jsonl` must be ignored
        // (defensive against stray files in the partition).
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let now = Utc::now();
        let today = day_dir(root, now.date_naive());

        let cwd = r"C:\Users\foo\bar";
        // No `rollout-` prefix.
        let _stray = write_rollout(
            &today,
            "session.jsonl",
            &cwd.replace('\\', "\\\\"),
            &now.to_rfc3339(),
        );
        let found = find_session_file(root, "c:\\users\\foo\\bar", now);
        assert!(found.is_none());
    }

    #[test]
    fn find_session_file_returns_none_when_no_match() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let now = Utc::now();
        let today = day_dir(root, now.date_naive());

        let other = r"C:\Other\Path";
        let _file = write_rollout(
            &today,
            "rollout.jsonl",
            &other.replace('\\', "\\\\"),
            &now.to_rfc3339(),
        );

        let found = find_session_file(root, "c:\\users\\foo\\bar", now);
        assert!(found.is_none());
    }

    #[test]
    fn find_session_file_handles_missing_date_partition() {
        // Empty root — no partitions at all. Function returns None without panic.
        let tmp = tempfile::tempdir().unwrap();
        let now = Utc::now();
        let found = find_session_file(tmp.path(), "c:\\users\\foo\\bar", now);
        assert!(found.is_none());
    }

    #[test]
    fn find_session_file_skips_unparseable_first_line() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let now = Utc::now();
        let today = day_dir(root, now.date_naive());
        fs::create_dir_all(&today).unwrap();

        let bad = today.join("rollout-bad.jsonl");
        let mut f = fs::File::create(&bad).unwrap();
        writeln!(f, "{{ this is not valid JSON").unwrap();

        let found = find_session_file(root, "c:\\users\\foo\\bar", now);
        assert!(found.is_none());
    }

    #[test]
    fn find_session_file_walks_past_seven_days() {
        // M6: the day-walk covers today + last 7 UTC days. A file 4 days
        // ago should be found.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let now = Utc::now();
        let four_days_ago = now - chrono::Duration::days(4);
        let dir = day_dir(root, four_days_ago.date_naive());

        let cwd = r"C:\Users\foo\bar";
        // The mtime of the freshly-written file will be now (well within the
        // 5 min mtime grace), simulating an APPEND on a resumed session.
        let expected = write_rollout(
            &dir,
            "rollout-old-session.jsonl",
            &cwd.replace('\\', "\\\\"),
            &four_days_ago.to_rfc3339(),
        );

        let found = find_session_file(root, "c:\\users\\foo\\bar", now);
        assert_eq!(found, Some(expected));
    }

    // ── #2232 phase 1: capture records ────────────────────────────────────

    /// Capture one synthetic line with no sink attached.
    fn capture_one(line: &str) -> Vec<Arc<CapturedRecord>> {
        let mut reader_seq = 0u64;
        capture_live_lines(
            vec![(0, line.to_string())],
            "codex-session",
            Path::new("rollout.jsonl"),
            &mut reader_seq,
            None,
            RecordOrigin::Live,
            &ReaderAttachment::default(),
        )
    }

    #[test]
    fn real_fixture_carries_the_nested_turn_id_and_final_bits() {
        // Test 3: the real fixture's turn id lives at
        // `payload.internal_chat_message_metadata_passthrough.turn_id`.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut reader_seq = 0u64;
        let records = capture_live_lines(
            vec![(17, REAL_CURRENT_CODEX_FINAL.to_string())],
            "codex-session",
            Path::new("rollout-real.jsonl"),
            &mut reader_seq,
            Some(&tx),
            RecordOrigin::Live,
            &ReaderAttachment::default(),
        );
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(
            record.turn_id.as_deref(),
            Some("01a09c07-0ad4-7710-9c1d-ce3f8dd0aaac")
        );
        assert!(record.provider_final);
        assert!(record.turn_identified);
        assert_eq!(record.text, "sanitized assistant reply");
        assert_eq!(record.provider, CaptureProvider::Codex);
        assert_eq!(record.origin, RecordOrigin::Live);
        assert_eq!(record.record_start, Some(17));
        assert_eq!(record.reader_seq, 0);
        // The record reached the sink, exactly once.
        assert_eq!(rx.try_recv().unwrap().text, "sanitized assistant reply");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn final_answer_without_turn_metadata_is_final_but_not_identified() {
        // Test 4: metadata object absent → `provider_final == true`,
        // `turn_identified == false`, `turn_id == None`.
        let records = capture_one(&current_final_record(&["no metadata"]));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].turn_id, None);
        assert!(records[0].provider_final);
        assert!(!records[0].turn_identified);
    }

    #[test]
    fn task_complete_emits_no_capture_record() {
        // Test 5: the extractor still rejects `task_complete`; the flat
        // `turn_id` on that record belongs to phase 5.
        let line = serde_json::json!({
            "type": "event_msg",
            "payload": {"type": "task_complete", "turn_id": "01a09c07-0ad4-7710-9c1d-ce3f8dd0aaac"}
        })
        .to_string();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut reader_seq = 0u64;
        let records = capture_live_lines(
            vec![(0, line)],
            "codex-session",
            Path::new("rollout.jsonl"),
            &mut reader_seq,
            Some(&tx),
            RecordOrigin::Live,
            &ReaderAttachment::default(),
        );
        assert!(records.is_empty());
        assert!(rx.try_recv().is_err(), "nothing may reach the sink");
        assert_eq!(reader_seq, 0, "a rejected line must not advance reader_seq");
    }

    #[test]
    fn capture_without_a_sender_leaves_the_telegram_bytes_unchanged() {
        // Test 8: a full watcher read pass with the sender left `None` appends
        // exactly the bytes the pre-change code appended.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rollout-parity.jsonl");
        let mut f = fs::File::create(&path).unwrap();
        writeln!(f, "{}", current_final_record(&["first body"])).unwrap();
        writeln!(
            f,
            r#"{{"type":"event_msg","payload":{{"type":"task_complete"}}}}"#
        )
        .unwrap();
        writeln!(f, "{}", current_final_record(&["second body"])).unwrap();
        drop(f);

        let mut offset = 0u64;
        let mut remainder = String::new();
        let new_lines = read_new_lines_with_starts(&path, &mut offset, &mut remainder).unwrap();

        // Pre-change append path: extractor text, one trailing newline each.
        let mut expected = String::new();
        for (_, line) in &new_lines {
            if let Some(text) = extract_assistant_final(line) {
                expected.push_str(&text);
                expected.push('\n');
            }
        }

        // Watcher path: capture with no sink attached, then buffer the text.
        let mut reader_seq = 0u64;
        let mut observed = String::new();
        for record in capture_live_lines(
            new_lines,
            "codex-session",
            &path,
            &mut reader_seq,
            None,
            RecordOrigin::Live,
            &ReaderAttachment::default(),
        ) {
            observed.push_str(&record.text);
            observed.push('\n');
        }

        assert_eq!(expected, observed);
        assert_eq!(reader_seq, 2);
    }
}
