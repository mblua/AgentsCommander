// Codex CLI session-file watcher.
//
// Polls `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` files for
// `response_item` records with `payload.type == "message"`,
// `payload.role == "assistant"` and `payload.phase == "final_answer"`, joining
// their `output_text` content blocks into the assistant prose sent to Telegram.
// Historical `event_msg + agent_message` records are deliberately ignored.
// Uses Kernel A (offset-based append-only JSONL) from `jsonl_kernel.rs` once
// `find_session_file` selects the right rollout.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc::UnboundedSender;
use tokio::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

use crate::capture::key::{ReaderAttachment, ReaderObservations};
use crate::capture::record::{CaptureProvider, CapturedRecord, RecordOrigin};
use crate::capture::state::head_from_lines;
use crate::capture::turn_codex::{TurnAccumulator, TurnEvent, TurnFragment};
use crate::commands::codex_resolver::canonicalize_cwd_for_codex;
use crate::network::OutboundNetwork;
use crate::telegram::jsonl_kernel::{
    self, read_new_lines_with_starts, read_preamble_for_race, POLL_INTERVAL_MS, PREAMBLE_MAX_BYTES,
    RACE_GRACE_SECS, ROTATION_STALE_SECS,
};
use crate::telegram::output::{flush_buffer, BridgeLogger, DiagLogger, TelegramEvents};

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
pub fn spawn_watch_task(
    search_root: PathBuf,
    expected_cwd: String,
    attach_time: DateTime<Utc>,
    network: OutboundNetwork,
    dest: tokio::sync::watch::Receiver<Option<BotTarget>>,
    session_id: String,
    cancel: CancellationToken,
    event_port: Arc<dyn TelegramEvents>,
    sink: Option<UnboundedSender<Arc<CapturedRecord>>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        watch_loop(
            search_root,
            expected_cwd,
            attach_time,
            network,
            dest,
            session_id.clone(),
            cancel,
            event_port.clone(),
            sink,
        )
        .await;
        log::info!("[CODEX_EXIT] Watcher task ended for session {}", session_id);
    })
}

/// The §6 live-attach preamble window of `path`, as `(start, body)` pairs.
///
/// The Codex-local variant of the Claude helper: starts are computed on the
/// **raw bytes plus the window offset**, never on the decoded text, so
/// `raw[start..]` walks back to the exact file bytes. The seek point is
/// resynced past a partial line unless the window starts at offset 0.
fn read_preamble_with_starts(
    path: &Path,
    attach_time: DateTime<Utc>,
) -> std::io::Result<Vec<(u64, String)>> {
    let (buf, window_start) = jsonl_kernel::read_live_preamble_window(path, PREAMBLE_MAX_BYTES)?;

    // Drop everything before the first `\n` unless the window starts at 0: the
    // seek point almost certainly lands mid-line. `base` is the absolute file
    // offset of the first byte of `bytes`.
    let (bytes, base): (&[u8], u64) = if window_start == 0 {
        (&buf, 0)
    } else {
        match buf.iter().position(|&b| b == b'\n') {
            Some(i) => (&buf[i + 1..], window_start + i as u64 + 1),
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
        if let Some((ts, _id, body)) = codex_preamble_extractor(line) {
            if ts >= cutoff {
                out.push((line_start, body));
            }
        }
    }
    Ok(out)
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
/// #2232 phase 5: an accepted `final_answer` with a turn id is handed to
/// `turns` and stays held until its `task_complete` closes the turn; only then
/// does one assembled record reach the sink, so the slot never holds a
/// fragment. A `final_answer` with no turn id has nothing to group with and is
/// emitted immediately, exactly as phase 1 does. Every closure is consumed
/// here too, so the two record types the extractor still rejects are no longer
/// discarded.
///
/// The caller appends `record.text` and a newline to the Telegram buffer,
/// exactly as the pre-#2232 code appended the extractor text.
#[allow(clippy::too_many_arguments)] // phase 1's seven plus the turn accumulator.
fn capture_live_lines(
    new_lines: Vec<(u64, String)>,
    session_id: &str,
    file: &Path,
    reader_seq: &mut u64,
    sender: Option<&UnboundedSender<Arc<CapturedRecord>>>,
    origin: RecordOrigin,
    attach: &ReaderAttachment,
    turns: &mut TurnAccumulator,
) -> Vec<Arc<CapturedRecord>> {
    let mut records = Vec::new();
    for (record_start, line) in new_lines {
        if let Some((text, turn_id)) = extract_assistant_final_with_turn(&line) {
            match turn_id {
                // No identifier: nothing to group with, so it is its own
                // candidate right now (plan section 5.2).
                None => {
                    let record = capture_record(
                        text,
                        None,
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
                // A multi-record turn is assembled before the slot: hold the
                // fragment until the turn's `task_complete` arrives.
                Some(turn_id) => {
                    for event in turns.on_final_answer(
                        &turn_id,
                        TurnFragment {
                            text,
                            record_start: Some(record_start),
                        },
                    ) {
                        log_turn_event(turns, &event);
                    }
                }
            }
            continue;
        }
        for event in turns.on_line(&line) {
            match event {
                TurnEvent::Emit(assembled) => {
                    let record = capture_record(
                        assembled.text,
                        Some(assembled.turn_id),
                        assembled.first_record_start,
                        origin,
                        session_id,
                        file,
                        reader_seq,
                        attach,
                    );
                    deliver_capture_record(sender, &record);
                    records.push(record);
                }
                other => log_turn_event(turns, &other),
            }
        }
    }
    records
}

/// Render one non-emission turn event as a visible, counted log line.
///
/// The accumulator owns the counters; rendering them here is what makes a
/// superseded or evicted turn (and an unparseable closure) observable without
/// ever turning into a routed candidate.
fn log_turn_event(turns: &TurnAccumulator, event: &TurnEvent) {
    let counters = turns.counters();
    match event {
        TurnEvent::Emit(_) => {}
        TurnEvent::Abstain { turn_id, reason } => log::info!(
            "[CODEX_TURN] abstained turn={} reason={} (abstained={})",
            turn_id,
            reason.as_str(),
            counters.abstained
        ),
        TurnEvent::Superseded { turn_id } => log::info!(
            "[CODEX_TURN] superseded open turn {} (superseded={})",
            turn_id,
            counters.superseded
        ),
        TurnEvent::Evicted { turn_id } => log::info!(
            "[CODEX_TURN] evicted oldest open turn {} (evicted={})",
            turn_id,
            counters.evicted
        ),
        TurnEvent::LateAfterRoute { turn_id } => log::info!(
            "[CODEX_TURN] ignored late final_answer for routed turn {} (late_after_route={})",
            turn_id,
            counters.late_after_route
        ),
        TurnEvent::LateAfterSuperseded { turn_id } => log::info!(
            "[CODEX_TURN] ignored late final_answer for superseded turn {} \
             (late_after_superseded={})",
            turn_id,
            counters.late_after_superseded
        ),
        TurnEvent::MalformedClosure => log::info!(
            "[CODEX_TURN] skipped unparseable closure record (malformed_closures={})",
            counters.malformed_closures
        ),
    }
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
    jsonl_kernel::read_transcript_header(path)
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
        jsonl_kernel::visit_codex_rollout_candidates(&dir, |path, mtime| {
            // Skip files modified more than 5 min before attach (stale).
            if mtime < mtime_cutoff_st {
                return;
            }
            let first = match read_first_line(&path) {
                Some(l) => l,
                None => return,
            };
            let v: serde_json::Value = match serde_json::from_str(&first) {
                Ok(v) => v,
                Err(_) => return,
            };
            let payload = match v.get("payload") {
                Some(p) => p,
                None => return,
            };
            let candidate_cwd = match payload.get("cwd").and_then(|c| c.as_str()) {
                Some(c) => c,
                None => return,
            };
            if canonicalize_cwd_for_codex(candidate_cwd) != normalized_expected {
                return;
            }
            match &best {
                Some((_, best_mtime)) if mtime > *best_mtime => best = Some((path, mtime)),
                None => best = Some((path, mtime)),
                _ => {}
            }
        });
    }

    best.map(|(p, _)| p)
}

/// Process-wide lock for tests that read or write the three global diagnostic
/// files (`telegram-bridge.log`, `diag-raw.log`, `diag-sent.log`).
///
/// `telegram::output` is preserved byte-for-byte this phase, and cargo runs
/// every lib test in one process, so a logger-constructing test would race the
/// room-only tests that assert those files are untouched. The lock lives here,
/// a `#[cfg(test)]` item outside the tests module, so the Claude watcher can
/// share it without a production arc.
#[cfg(test)]
pub(crate) async fn lock_global_diagnostic_files() -> tokio::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await
}

macro_rules! bridge_log {
    ($state:ident, $tag:expr, $msg:expr) => {
        if let Some(bridge_logger) = $state.logger.as_mut() {
            bridge_logger.log($tag, &$state.session_id, $msg);
        }
    };
}
struct CodexWatchState {
    search_root: PathBuf,
    expected_cwd: String,
    attach_time: DateTime<Utc>,
    network: OutboundNetwork,
    session_id: String,
    event_port: Arc<dyn TelegramEvents>,
    bot_target: Option<BotTarget>,
    logger: Option<BridgeLogger>,
    diag: Option<DiagLogger>,
    buffer: String,
    last_buffer_add: Instant,
    capture_tx: Option<UnboundedSender<Arc<CapturedRecord>>>,
    reader_seq: u64,
    turns: TurnAccumulator,
    observer: ReaderObservations,
    current_file: Option<PathBuf>,
    current_file_mtime: Option<SystemTime>,
    last_mtime_advance: Instant,
    file_offset: u64,
    line_remainder: String,
    search_warned: bool,
}
#[allow(clippy::too_many_arguments)]
async fn watch_loop(
    search_root: PathBuf,
    expected_cwd: String,
    attach_time: DateTime<Utc>,
    network: OutboundNetwork,
    dest: tokio::sync::watch::Receiver<Option<BotTarget>>,
    session_id: String,
    cancel: CancellationToken,
    event_port: Arc<dyn TelegramEvents>,
    sink: Option<UnboundedSender<Arc<CapturedRecord>>>,
) {
    let mut dest_rx = dest;
    let bot_target = dest_rx.borrow_and_update().clone();
    let logger = bot_target.as_ref().map(|_| BridgeLogger::new(&session_id));
    let diag = bot_target.as_ref().map(|_| DiagLogger::new());
    let buffer = String::new();
    let last_buffer_add = Instant::now();
    let flush_delay = Duration::from_millis(FLUSH_DELAY_MS);
    let mut state = CodexWatchState {
        search_root,
        expected_cwd,
        attach_time,
        network,
        session_id,
        event_port,
        bot_target,
        logger,
        diag,
        buffer,
        last_buffer_add,
        capture_tx: sink,
        reader_seq: 0,
        turns: TurnAccumulator::new(),
        observer: ReaderObservations::default(),
        current_file: None,
        current_file_mtime: None,
        last_mtime_advance: Instant::now(),
        file_offset: 0,
        line_remainder: String::new(),
        search_warned: false,
    };
    bridge_log!(
        state,
        "CODEX_INIT",
        &format!(
            "search_root={} expected_cwd={}",
            state.search_root.display(),
            state.expected_cwd
        )
    );
    let mut poll_interval = tokio::time::interval(Duration::from_millis(POLL_INTERVAL_MS));
    poll_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            changed = dest_rx.changed() => {
                if changed.is_err() {
                    break;
                }
                let new_target = dest_rx.borrow_and_update().clone();
                state.destination_changed(new_target);
            }
            _ = poll_interval.tick() => {
                state.rescan();
                state.read_current();
                state.flush_due(flush_delay).await;
            }
        }
    }
    state.finish().await;
}
impl CodexWatchState {
    fn destination_changed(&mut self, new_target: Option<BotTarget>) {
        let attaching = new_target.is_some();
        if attaching {
            if self.logger.is_none() {
                self.logger = Some(BridgeLogger::new(&self.session_id));
            }
            if self.diag.is_none() {
                self.diag = Some(DiagLogger::new());
            }
        } else {
            self.logger = None;
            self.diag = None;
        }
        self.buffer.clear();
        self.bot_target = new_target;
        if attaching {
            if let Some(ref path) = self.current_file {
                match read_preamble_with_starts(path, Utc::now()) {
                    Ok(lines) => {
                        for (start, body) in lines {
                            if start < self.file_offset {
                                bridge_log!(self, "CODEX_PREAMBLE", &body);
                                self.buffer.push_str(&body);
                                self.buffer.push('\n');
                                self.last_buffer_add = Instant::now();
                            }
                        }
                    }
                    Err(e) => {
                        log::warn!("[CODEX_ERR] live-attach preamble read failed: {}", e);
                    }
                }
            }
        }
    }
    fn rescan(&mut self) {
        let need_rescan = match &self.current_file {
            None => true,
            Some(p) if !jsonl_kernel::transcript_exists(p) => true,
            Some(_) => self.last_mtime_advance.elapsed().as_secs() >= ROTATION_STALE_SECS,
        };
        if need_rescan {
            if let Some(found) =
                find_session_file(&self.search_root, &self.expected_cwd, self.attach_time)
            {
                let same_as_current = self.current_file.as_ref() == Some(&found);
                if !same_as_current {
                    self.bind_file(&found);
                    self.current_file = Some(found);
                }
                let new_mtime = self
                    .current_file
                    .as_ref()
                    .and_then(|p| jsonl_kernel::transcript_metadata(p).ok())
                    .and_then(|m| m.modified().ok());
                if new_mtime != self.current_file_mtime {
                    self.last_mtime_advance = Instant::now();
                }
                self.current_file_mtime = new_mtime;
            } else if !self.search_warned {
                bridge_log!(self, "CODEX_WAIT", "no rollout matching cwd found yet");
                self.search_warned = true;
            }
        }
    }
    fn bind_file(&mut self, found: &Path) {
        let first_bind = self.current_file.is_none();
        if first_bind {
            self.line_remainder.clear();
            match read_preamble_for_race(found, self.attach_time, codex_preamble_extractor) {
                Ok((bodies, _ids, file_len)) => {
                    let attach = if self.capture_tx.is_some() {
                        self.observer.observe(found, file_len, Vec::new())
                    } else {
                        ReaderAttachment::default()
                    };
                    for record in capture_preamble_bodies(
                        bodies,
                        &self.session_id,
                        found,
                        &mut self.reader_seq,
                        self.capture_tx.as_ref(),
                        &attach,
                    ) {
                        bridge_log!(self, "CODEX_PREAMBLE", &record.text);
                        if self.bot_target.is_some() {
                            self.buffer.push_str(&record.text);
                            self.buffer.push('\n');
                            self.last_buffer_add = Instant::now();
                        }
                    }
                    self.file_offset = file_len;
                    bridge_log!(
                        self,
                        "CODEX_FILE",
                        &format!(
                            "bound to {}, preamble done, offset={}",
                            found.display(),
                            self.file_offset
                        )
                    );
                }
                Err(e) => {
                    bridge_log!(self, "CODEX_ERR", &format!("preamble scan failed: {}", e));
                    self.file_offset = jsonl_kernel::transcript_metadata(found)
                        .ok()
                        .map(|m| m.len())
                        .unwrap_or(0);
                }
            }
        } else {
            self.line_remainder.clear();
            self.file_offset = jsonl_kernel::transcript_metadata(found)
                .ok()
                .map(|m| m.len())
                .unwrap_or(0);
            bridge_log!(
                self,
                "CODEX_ROTATE",
                &format!(
                    "rotated to {}, offset={}",
                    found.display(),
                    self.file_offset
                )
            );
        }
    }
    fn read_current(&mut self) {
        if let Some(ref path) = self.current_file {
            let read_start = self.file_offset;
            match read_new_lines_with_starts(path, &mut self.file_offset, &mut self.line_remainder)
            {
                Ok(new_lines) => {
                    let attach = if self.capture_tx.is_some() {
                        let head = head_from_lines(&new_lines, read_start);
                        self.observer.observe(path, self.file_offset, head)
                    } else {
                        ReaderAttachment::default()
                    };
                    for record in capture_live_lines(
                        new_lines,
                        &self.session_id,
                        path,
                        &mut self.reader_seq,
                        self.capture_tx.as_ref(),
                        RecordOrigin::Live,
                        &attach,
                        &mut self.turns,
                    ) {
                        bridge_log!(self, "CODEX_EXTRACT", &record.text);
                        if self.bot_target.is_some() {
                            self.buffer.push_str(&record.text);
                            self.buffer.push('\n');
                            self.last_buffer_add = Instant::now();
                        }
                    }
                    let new_mtime = jsonl_kernel::transcript_metadata(path)
                        .ok()
                        .and_then(|m| m.modified().ok());
                    if new_mtime != self.current_file_mtime {
                        self.last_mtime_advance = Instant::now();
                    }
                    self.current_file_mtime = new_mtime;
                }
                Err(e) => {
                    bridge_log!(self, "CODEX_ERR", &e.to_string());
                    log::error!(
                        "[CODEX_ERR] Read error for session {}: {}",
                        self.session_id,
                        e
                    );
                    self.event_port.bridge_error(
                        &self.session_id,
                        &format!("Codex JSONL read error: {}", e),
                        None,
                    );
                }
            }
        }
    }
    async fn flush_due(&mut self, flush_delay: Duration) {
        if !self.buffer.is_empty() {
            let elapsed = self.last_buffer_add.elapsed();
            if elapsed >= flush_delay || self.buffer.len() > FLUSH_BYTES {
                if let (Some(target), Some(bridge_logger), Some(diag_logger)) = (
                    self.bot_target.as_ref(),
                    self.logger.as_mut(),
                    self.diag.as_mut(),
                ) {
                    flush_buffer(
                        &mut self.buffer,
                        &self.network,
                        &target.token,
                        target.chat_id,
                        &self.session_id,
                        self.event_port.as_ref(),
                        bridge_logger,
                        diag_logger,
                        true,
                    )
                    .await;
                } else {
                    self.buffer.clear();
                }
            }
        }
    }
    async fn finish(&mut self) {
        if let Some(ref path) = self.current_file {
            let read_start = self.file_offset;
            if let Ok(new_lines) =
                read_new_lines_with_starts(path, &mut self.file_offset, &mut self.line_remainder)
            {
                let attach = if self.capture_tx.is_some() {
                    let head = head_from_lines(&new_lines, read_start);
                    self.observer.observe(path, self.file_offset, head)
                } else {
                    ReaderAttachment::default()
                };
                for record in capture_live_lines(
                    new_lines,
                    &self.session_id,
                    path,
                    &mut self.reader_seq,
                    self.capture_tx.as_ref(),
                    RecordOrigin::Live,
                    &attach,
                    &mut self.turns,
                ) {
                    if self.bot_target.is_some() {
                        self.buffer.push_str(&record.text);
                        self.buffer.push('\n');
                    }
                }
            }
        }
        if !self.buffer.is_empty() {
            if let (Some(target), Some(bridge_logger), Some(diag_logger)) = (
                self.bot_target.as_ref(),
                self.logger.as_mut(),
                self.diag.as_mut(),
            ) {
                flush_buffer(
                    &mut self.buffer,
                    &self.network,
                    &target.token,
                    target.chat_id,
                    &self.session_id,
                    self.event_port.as_ref(),
                    bridge_logger,
                    diag_logger,
                    true,
                )
                .await;
            }
        }
    }
}

#[cfg(test)]
mod tests;
