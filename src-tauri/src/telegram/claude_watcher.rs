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

use crate::capture::key::{ReaderAttachment, ReaderObservations};
use crate::capture::record::{CaptureProvider, CapturedRecord, RecordOrigin};
use crate::capture::state::head_from_lines;
use crate::network::OutboundNetwork;
use crate::telegram::jsonl_kernel::{
    self, find_latest_jsonl, find_pinned_jsonl, read_new_lines_with_starts, read_preamble_for_race,
    PINNED_ATTACH_WAIT_SECS, POLL_INTERVAL_MS, PREAMBLE_MAX_BYTES, RACE_GRACE_SECS,
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
///
/// `transcript_id` is the transcript id AC minted at spawn (#2454). When it is
/// known and `<id>.jsonl` is absent at the first poll, the first attach waits
/// for that file instead of taking the newest one by mtime.
///
/// `dest` carries the Telegram destination, consulted **only** when it changes
/// — at attach and at detach (#2232 phase 4 section 6). `None` is a room-only
/// reader: no logger is built, nothing is sent, and the records still reach
/// `sink`.
#[allow(clippy::too_many_arguments)]
pub fn spawn_watch_task<R: tauri::Runtime>(
    project_dir: PathBuf,
    transcript_id: Option<String>,
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
            transcript_id,
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

/// The first-attach decision of the Claude reader (#2454).
#[derive(Clone, Debug, PartialEq, Eq)]
enum AttachDecision {
    /// The minted transcript was absent at the first poll and still is, and
    /// the deadline is live: attach to nothing.
    Wait,
    /// The reader was waiting and the minted transcript has appeared. The
    /// fresh-spawn case, and only it.
    Pinned(PathBuf),
    /// The newest `.jsonl` by mtime, exactly as before #2454.
    Mtime(PathBuf),
    /// The reader was waiting, the deadline expired and the minted transcript
    /// is still absent: attach exactly like `Mtime`. Its own variant so the
    /// fallback `warn!` has one call site.
    MtimeFallback(PathBuf),
    /// No `.jsonl` at all.
    Nothing,
}

/// What [`decide_attach`] reads. Whether the pinned file is the newest is
/// `pinned == newest`; requirement 4 of #2454 makes the answer irrelevant.
#[derive(Clone, Copy, Debug)]
struct AttachInputs<'a> {
    transcript_id: Option<&'a str>,
    /// `<id>.jsonl` when it exists.
    pinned: Option<&'a Path>,
    newest: Option<&'a Path>,
    first_poll: bool,
    deadline_expired: bool,
}

/// Decide the reader's first attach. Pure, so tests observe the decision
/// directly.
///
/// `Pinned` is taken only when the reader was waiting: the minted file did not
/// exist at the first poll, so no reader can have consumed any of it. A file
/// that already existed at the first poll is late or resumed history and takes
/// `Mtime`, whether or not it is the newest.
fn decide_attach(inputs: AttachInputs<'_>) -> AttachDecision {
    let by_mtime = || match inputs.newest {
        Some(path) => AttachDecision::Mtime(path.to_path_buf()),
        None => AttachDecision::Nothing,
    };
    if inputs.transcript_id.is_none() {
        return by_mtime();
    }
    match (inputs.first_poll, inputs.pinned) {
        (true, Some(_)) => by_mtime(),
        (true, None) => AttachDecision::Wait,
        (false, Some(path)) => AttachDecision::Pinned(path.to_path_buf()),
        (false, None) if inputs.deadline_expired => match inputs.newest {
            Some(path) => AttachDecision::MtimeFallback(path.to_path_buf()),
            None => AttachDecision::Nothing,
        },
        (false, None) => AttachDecision::Wait,
    }
}

/// Whether this poll consults [`decide_attach`] (#2454 p3b requirement 4).
///
/// Only until the reader's first attach: after it, a vanished transcript
/// leaves `current_file` empty again, and re-running the decision would pin a
/// restored file whose content this reader already consumed.
fn runs_first_attach_decision(first_attach_done: bool, has_current_file: bool) -> bool {
    !first_attach_done && !has_current_file
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
    let (buf, window_start) = jsonl_kernel::read_live_preamble_window(path, PREAMBLE_MAX_BYTES)?;

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
                // G4: whitelist "text" only — filters tool_use, tool_result, thinking,
                // and future types
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
/// lines are not tracked by the kernel, so `record_start` is `None`.
///
/// `origin` is [`RecordOrigin::Preamble`] for an attach by mtime and
/// [`RecordOrigin::Live`] for a pinned attach (#2454): a pinned file did not
/// exist when the reader started, so its content is this spawn's own output.
/// The `None` start is kept on purpose; the scan reads a capped tail window, so
/// any offset it produced would be a guess.
fn capture_preamble_bodies(
    bodies: Vec<String>,
    session_id: &str,
    file: &Path,
    reader_seq: &mut u64,
    sender: Option<&UnboundedSender<Arc<CapturedRecord>>>,
    attach: &ReaderAttachment,
    origin: RecordOrigin,
) -> Vec<Arc<CapturedRecord>> {
    let mut records = Vec::new();
    for text in bodies {
        let record = capture_record(
            text, None, None, origin, session_id, file, reader_seq, attach,
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

/// The mutable state of one Claude reader, and the poll-tick blocks of
/// [`watch_loop`] as methods over it.
struct ClaudeReader<R: tauri::Runtime> {
    project_dir: PathBuf,
    network: OutboundNetwork,
    session_id: String,
    app: tauri::AppHandle<R>,
    current_dest: Option<BotTarget>,
    logger: Option<BridgeLogger>,
    diag: Option<DiagLogger>,
    buffer: String,
    last_buffer_add: Instant,
    flush_delay: Duration,
    capture_tx: Option<UnboundedSender<Arc<CapturedRecord>>>,
    reader_seq: u64,
    observer: ReaderObservations,
    rotation_backfill_pending: bool,
    attach_time: DateTime<Utc>,
    current_file: Option<PathBuf>,
    current_file_mtime: Option<SystemTime>,
    file_offset: u64,
    line_remainder: String,
    dir_warned: bool,
    pin_id: Option<String>,
    first_poll: bool,
    first_attach_done: bool,
    wait_deadline: Option<Instant>,
}

impl<R: tauri::Runtime> ClaudeReader<R> {
    fn new(
        project_dir: PathBuf,
        transcript_id: Option<String>,
        network: OutboundNetwork,
        current_dest: Option<BotTarget>,
        session_id: String,
        app: tauri::AppHandle<R>,
        sink: Option<UnboundedSender<Arc<CapturedRecord>>>,
    ) -> Self {
        // #2232 phase 4 section 8: with no bot demand no diagnostic is built, so
        // `BridgeLogger::new` — which truncates the **global** diagnostic files
        // (`telegram/output.rs:140`) — is never called for a room-only reader. On a
        // hot attach the loggers are born at that moment, which is when they are
        // truncated on attach today.
        let logger = current_dest
            .as_ref()
            .map(|_| BridgeLogger::new(&session_id));
        let diag = current_dest.as_ref().map(|_| DiagLogger::new());
        Self {
            project_dir,
            network,
            session_id,
            app,
            current_dest,
            logger,
            diag,
            buffer: String::new(),
            last_buffer_add: Instant::now(),
            flush_delay: Duration::from_millis(FLUSH_DELAY_MS),
            // #2232 phase 4 section 4.2: the supervisor passes the live sender in, so
            // the records of a room-only reader reach `CaptureRegistry` with no bot
            // anywhere. With `None` the emit is a no-op.
            capture_tx: sink,
            reader_seq: 0,
            // #2232 phase 3: the reader's own epoch and file observation. Both are
            // computed only when a sink is attached, so a room without the flag keeps
            // the pre-#2232 syscall count.
            observer: ReaderObservations::default(),
            // The first sweep of a rotated transcript is a backfill, not a live turn
            // (section 8.3). Declared divergence: Telegram does send that content
            // today, so a legitimate first turn can be suppressed downstream.
            rotation_backfill_pending: false,
            attach_time: Utc::now(),
            current_file: None,
            current_file_mtime: None,
            file_offset: 0,
            line_remainder: String::new(),
            dir_warned: false,
            // #2454: the minted transcript id, consulted until the first attach.
            pin_id: transcript_id,
            first_poll: true,
            first_attach_done: false,
            wait_deadline: None,
        }
    }

    /// Log through the bridge logger only when one exists, so `JSONL_EXTRACT`
    /// is not written for a room-only reader (section 8).
    fn bridge_log(&mut self, tag: &str, msg: &str) {
        if let Some(bridge_logger) = self.logger.as_mut() {
            bridge_logger.log(tag, &self.session_id, msg);
        }
    }

    /// Queue a record's text for Telegram when a destination is set.
    fn buffer_record(&mut self, text: &str) {
        if self.current_dest.is_some() {
            self.buffer.push_str(text);
            self.buffer.push('\n');
            self.last_buffer_add = Instant::now();
        }
    }

    // §6: the destination is consulted only when it changes, and the
    // three transitions run **inside the reader task**. There is no
    // `await` between them: yielding would flush the pending buffer to
    // the NEW destination, which is exactly what this contract
    // prevents, and the preamble could use a different offset than the
    // reader had when it started.
    fn on_dest_change(&mut self, new_dest: Option<BotTarget>) {
        let attaching = new_dest.is_some();
        // The loggers are born at the moment of a hot attach, which is
        // when they are truncated on attach today, so what is
        // observable does not change (§8).
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
        // The moment of the destination switch serves as `attach_time`.
        let preamble = apply_destination_change(
            &mut self.buffer,
            &mut self.current_dest,
            new_dest,
            self.current_file.as_deref(),
            self.file_offset,
            Utc::now(),
        );
        for body in preamble {
            self.bridge_log("JSONL_PREAMBLE", &body);
            self.buffer.push_str(&body);
            self.buffer.push('\n');
            self.last_buffer_add = Instant::now();
        }
    }

    /// One poll tick.
    async fn poll(&mut self) {
        if !self.project_dir_ready() {
            return;
        }
        let newest = find_latest_jsonl(&self.project_dir);
        let (latest, pinned_attach) = self.attach_target(newest);
        self.switch_file(latest, pinned_attach);
        self.read_new_lines();
        self.flush_if_due().await;
    }

    /// Check if project directory exists yet
    fn project_dir_ready(&mut self) -> bool {
        if !jsonl_kernel::transcript_is_dir(&self.project_dir) {
            if !self.dir_warned {
                self.bridge_log("JSONL_WAIT", "project directory does not exist yet");
                self.dir_warned = true;
            }
            return false;
        }
        if self.dir_warned {
            self.bridge_log("JSONL_INIT", "project directory appeared");
            self.dir_warned = false;
        }
        true
    }

    /// The file this poll should read, and whether it is a pinned attach.
    fn attach_target(&mut self, newest: Option<PathBuf>) -> (Option<PathBuf>, bool) {
        // #2454: the first attach goes through `decide_attach`; after
        // it, rotation follows the newest mtime exactly as before.
        let mut pinned_attach = false;
        let latest =
            if !runs_first_attach_decision(self.first_attach_done, self.current_file.is_some()) {
                newest
            } else {
                let pinned = self
                    .pin_id
                    .as_deref()
                    .and_then(|id| find_pinned_jsonl(&self.project_dir, id));
                let decision = decide_attach(AttachInputs {
                    transcript_id: self.pin_id.as_deref(),
                    pinned: pinned.as_deref(),
                    newest: newest.as_deref(),
                    first_poll: self.first_poll,
                    deadline_expired: self.wait_deadline.is_some_and(|d| Instant::now() >= d),
                });
                self.first_poll = false;
                let waiting = matches!(decision, AttachDecision::Wait);
                if waiting && self.wait_deadline.is_none() {
                    self.wait_deadline =
                        Some(Instant::now() + Duration::from_secs(PINNED_ATTACH_WAIT_SECS));
                    log::info!(
                        "[JSONL_ATTACH] session {} waiting up to {}s for {}",
                        self.session_id,
                        PINNED_ATTACH_WAIT_SECS,
                        self.project_dir
                            .join(format!(
                                "{}.jsonl",
                                self.pin_id.as_deref().unwrap_or_default()
                            ))
                            .display()
                    );
                }
                match decision {
                    AttachDecision::Pinned(path) => {
                        pinned_attach = true;
                        Some(path)
                    }
                    AttachDecision::Mtime(path) => Some(path),
                    // Runs once: this attach ends the decision loop.
                    AttachDecision::MtimeFallback(path) => {
                        log::warn!(
                            "[JSONL_ATTACH] session {} pinned transcript absent after {}s; \
                         falling back to newest by mtime",
                            self.session_id,
                            PINNED_ATTACH_WAIT_SECS
                        );
                        Some(path)
                    }
                    AttachDecision::Wait | AttachDecision::Nothing => None,
                }
            };
        (latest, pinned_attach)
    }

    /// Handle file rotation with flicker guard
    fn switch_file(&mut self, latest: Option<PathBuf>, pinned_attach: bool) {
        if latest == self.current_file || !self.should_switch() {
            return;
        }
        if self.current_file.is_none() {
            // First attach (§J preamble scan): emit recent
            // lines from the file's tail, then set offset = file_len.
            if let Some(ref p) = latest {
                self.first_attach_scan(p, pinned_attach);
            } else {
                self.file_offset = 0;
            }
        } else {
            // File rotation (new Claude session): read from start
            self.file_offset = 0;
            self.rotation_backfill_pending = true;
            self.bridge_log("JSONL_ROTATE", &format!("new file: {:?}", latest));
            log::info!(
                "[JSONL_ROTATE] session {} old={} new={} offset={}",
                self.session_id,
                self.current_file
                    .as_deref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
                latest
                    .as_deref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
                self.file_offset
            );
        }
        self.current_file = latest;
        self.first_attach_done |= self.current_file.is_some();
        self.current_file_mtime = self
            .current_file
            .as_ref()
            .and_then(|p| jsonl_kernel::transcript_metadata(p).ok())
            .and_then(|m| m.modified().ok());
        self.line_remainder.clear();
    }

    fn should_switch(&self) -> bool {
        match (&self.current_file, &self.current_file_mtime) {
            (Some(_), Some(mtime)) => {
                // Only switch if current file is stale
                mtime
                    .elapsed()
                    .map(|d| d.as_secs() >= ROTATION_STALE_SECS)
                    .unwrap_or(true)
            }
            _ => true, // No current file — always accept
        }
    }

    /// The first attach's §J preamble scan over `p`.
    fn first_attach_scan(&mut self, p: &Path, pinned_attach: bool) {
        // #2454 requirement 5: a pinned attach is this
        // spawn's own output, so it is `Live`.
        let origin = if pinned_attach {
            RecordOrigin::Live
        } else {
            RecordOrigin::Preamble
        };
        log::info!(
            "[JSONL_ATTACH] session {} first attach {} path={}",
            self.session_id,
            if pinned_attach { "pinned" } else { "by mtime" },
            p.display()
        );
        match read_preamble_for_race(p, self.attach_time, claude_preamble_extractor) {
            Ok((bodies, _ids, file_len)) => {
                // The §J scan reads the tail, so it
                // carries no head evidence: length
                // alone decides the epoch here.
                let attach = if self.capture_tx.is_some() {
                    self.observer.observe(p, file_len, Vec::new())
                } else {
                    ReaderAttachment::default()
                };
                for record in capture_preamble_bodies(
                    bodies,
                    &self.session_id,
                    p,
                    &mut self.reader_seq,
                    self.capture_tx.as_ref(),
                    &attach,
                    origin,
                ) {
                    self.bridge_log("JSONL_PREAMBLE", &record.text);
                    self.buffer_record(&record.text);
                }
                self.file_offset = file_len;
                self.bridge_log(
                    "JSONL_FILE",
                    &format!(
                        "initial file, preamble scan done, offset={}",
                        self.file_offset
                    ),
                );
            }
            Err(e) => {
                self.bridge_log("JSONL_ERR", &format!("preamble scan failed: {}", e));
                self.file_offset = jsonl_kernel::transcript_metadata(p)
                    .ok()
                    .map(|m| m.len())
                    .unwrap_or(0);
            }
        }
    }

    /// Read the lines appended to the current file since the last poll.
    fn read_new_lines(&mut self) {
        let Some(path) = self.current_file.clone() else {
            return;
        };
        let read_start = self.file_offset;
        match read_new_lines_with_starts(&path, &mut self.file_offset, &mut self.line_remainder) {
            Ok(new_lines) => {
                let attach = if self.capture_tx.is_some() {
                    let head = head_from_lines(&new_lines, read_start);
                    self.observer.observe(&path, self.file_offset, head)
                } else {
                    ReaderAttachment::default()
                };
                let origin = if self.rotation_backfill_pending && !new_lines.is_empty() {
                    self.rotation_backfill_pending = false;
                    RecordOrigin::RotationBackfill
                } else {
                    RecordOrigin::Live
                };
                for record in capture_live_lines(
                    new_lines,
                    &self.session_id,
                    &path,
                    &mut self.reader_seq,
                    self.capture_tx.as_ref(),
                    origin,
                    &attach,
                ) {
                    self.bridge_log("JSONL_EXTRACT", &record.text);
                    self.buffer_record(&record.text);
                }

                // Update mtime for rotation flicker guard
                self.current_file_mtime = jsonl_kernel::transcript_metadata(&path)
                    .ok()
                    .and_then(|m| m.modified().ok());
            }
            Err(e) => {
                // G5: Emit bridge error event for file I/O failures
                self.bridge_log("JSONL_ERR", &e.to_string());
                log::error!(
                    "[JSONL_ERR] Read error for session {}: {}",
                    self.session_id,
                    e
                );
                let _ = self.app.emit(
                    "telegram_bridge_error",
                    serde_json::json!({
                        "sessionId": self.session_id,
                        "error": format!("JSONL read error: {}", e),
                    }),
                );
            }
        }
    }

    /// Flush buffer if enough time has passed since last addition
    async fn flush_if_due(&mut self) {
        if self.buffer.is_empty() {
            return;
        }
        let elapsed = self.last_buffer_add.elapsed();
        if elapsed >= self.flush_delay || self.buffer.len() > 2000 {
            if let (Some(target), Some(bridge_logger), Some(diag_logger)) = (
                self.current_dest.as_ref(),
                self.logger.as_mut(),
                self.diag.as_mut(),
            ) {
                flush_buffer(
                    &mut self.buffer,
                    &self.network,
                    &target.token,
                    target.chat_id,
                    &self.session_id,
                    &self.app,
                    bridge_logger,
                    diag_logger,
                    true, // skip_dedup: JSONL text is clean, repeated lines are legitimate
                )
                .await;
            } else {
                self.buffer.clear();
            }
        }
    }

    /// G1: Final poll + flush after cancel (don't lose buffered content)
    async fn drain(&mut self) {
        if let Some(path) = self.current_file.clone() {
            let read_start = self.file_offset;
            if let Ok(new_lines) =
                read_new_lines_with_starts(&path, &mut self.file_offset, &mut self.line_remainder)
            {
                let attach = if self.capture_tx.is_some() {
                    let head = head_from_lines(&new_lines, read_start);
                    self.observer.observe(&path, self.file_offset, head)
                } else {
                    ReaderAttachment::default()
                };
                let origin = if self.rotation_backfill_pending && !new_lines.is_empty() {
                    RecordOrigin::RotationBackfill
                } else {
                    RecordOrigin::Live
                };
                for record in capture_live_lines(
                    new_lines,
                    &self.session_id,
                    &path,
                    &mut self.reader_seq,
                    self.capture_tx.as_ref(),
                    origin,
                    &attach,
                ) {
                    if self.current_dest.is_some() {
                        self.buffer.push_str(&record.text);
                        self.buffer.push('\n');
                    }
                }
            }
        }
        if !self.buffer.is_empty() {
            if let (Some(target), Some(bridge_logger), Some(diag_logger)) = (
                self.current_dest.as_ref(),
                self.logger.as_mut(),
                self.diag.as_mut(),
            ) {
                flush_buffer(
                    &mut self.buffer,
                    &self.network,
                    &target.token,
                    target.chat_id,
                    &self.session_id,
                    &self.app,
                    bridge_logger,
                    diag_logger,
                    true,
                )
                .await;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn watch_loop<R: tauri::Runtime>(
    project_dir: PathBuf,
    transcript_id: Option<String>,
    network: OutboundNetwork,
    dest: tokio::sync::watch::Receiver<Option<BotTarget>>,
    session_id: String,
    cancel: CancellationToken,
    app: tauri::AppHandle<R>,
    sink: Option<UnboundedSender<Arc<CapturedRecord>>>,
) {
    let mut dest_rx = dest;
    let current_dest: Option<BotTarget> = dest_rx.borrow_and_update().clone();
    let mut reader = ClaudeReader::new(
        project_dir,
        transcript_id,
        network,
        current_dest,
        session_id,
        app,
        sink,
    );

    // Emitted once, before the loop, so it cannot flood.
    log::info!(
        "[JSONL_ATTACH] session {} transcript_id={}",
        reader.session_id,
        reader.pin_id.as_deref().unwrap_or("none")
    );

    let init = format!("project_dir={}", reader.project_dir.display());
    reader.bridge_log("JSONL_INIT", &init);

    let mut poll_interval = tokio::time::interval(Duration::from_millis(POLL_INTERVAL_MS));
    poll_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        // `biased`: the destination change is processed before the next file
        // poll, never after it (section 6).
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,

            changed = dest_rx.changed() => {
                if changed.is_err() {
                    // The supervisor dropped the sender: the reader is going away.
                    break;
                }
                let new_dest = dest_rx.borrow_and_update().clone();
                reader.on_dest_change(new_dest);
            }

            _ = poll_interval.tick() => reader.poll().await,
        }
    }

    reader.drain().await;
}

#[cfg(test)]
mod tests;
