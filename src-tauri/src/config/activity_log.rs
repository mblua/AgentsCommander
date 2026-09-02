//! #1149 - append-only per-agent activity log: `<config_dir>/activity.jsonl`.
//!
//! One JSON object per line, schema-versioned (`v`), self-contained, written at
//! the instant of the event it describes. Nothing is ever buffered in memory
//! waiting for a later event, so a hard kill loses only records that were never
//! written.
//!
//! # Time zone and the two clock domains
//!
//! Every visible timestamp is **UTC without exception**: RFC3339 with
//! milliseconds and a `Z` suffix, byte-identical to
//! [`crate::phone::types::canonical_pty_timestamp`], so this file correlates
//! exactly with `api-audit.log`, `coordinator_clocks.json` and the outbox
//! envelopes. It **does NOT match `app.log`**, which uses `chrono::Local::now()`
//! (`logging.rs`): correlating the two needs an explicit local-to-UTC
//! conversion, DST included.
//!
//! The file carries **two clock domains**. Every `at` comes from `Utc::now()`,
//! while `continuesBlock` and `gapMs` derive from a monotonic `Instant`. A human
//! reader can therefore see two records whose `at` values differ by minutes yet
//! carry `continuesBlock: true`, if the wall clock jumped between them. Both the
//! `Instant` delta and the `Utc` delta must be under the 30 s window for a
//! continuation, so a clock jump or a suspend can only ever *suppress* a
//! continuation, never manufacture one.
//!
//! # Blocks are annotated, never buffered
//!
//! An `idle` record is written the instant the edge happens. The next `busy` for
//! that session looks back at the previous `idle` and stamps
//! `continuesBlock: true` plus `gapMs` when the gap is under the window. A block
//! starts at a `busy` with `continuesBlock: false` and continues through every
//! subsequent `busy` with `continuesBlock: true`. Block boundaries are stated by
//! the emitter; no consumer-side window arithmetic exists.
//!
//! # Supported metrics
//!
//! **Supported:** `totalBusyMs` and `totalBusyRawMs`, summed over closed
//! intervals keyed `(runId, sessionId)`:
//!
//! ```text
//! totalBusyRawMs = sum over closed intervals of (close - open)
//! totalBusyMs    = sum over closed intervals of max(0, (close - open) - idleThresholdMs_of_the_closing_record)
//! ```
//!
//! `idleThresholdMs` is present only on `reason: "mark_idle"` records, which are
//! the only threshold-delayed closes; synthetic closes omit it and nothing is
//! subtracted. `totalBusyMs` is the supported figure and its bias is downward,
//! from three causes that must be stated wherever it is presented: the
//! `max(0, ..)` clamp (unreachable through the production emission path, its one
//! real trigger being the `mark_busy`/`mark_idle` spawn-order race), silent tool
//! calls being invisible because the PTY carries bytes and not semantics, and
//! the correction itself, whose size is the sum of the subtracted thresholds.
//!
//! **Unsupported:** `intervalCount` and `blockCount`. Both are countable from
//! the file but neither is robust: the interval count varies with the coalescing
//! window and rotation can evict an opening `busy`. Every `app_start` declares
//! this in the data itself, under `metrics`.
//!
//! # Closing rule for a consumer
//!
//! For each open `busy` keyed `(runId, sessionId)`, close it at the earliest of:
//! the `at` of the next `idle` for that key; the `at` of that run's `app_stop`;
//! `previousRun.lastRecordAt` from an `app_start` naming that `runId` with
//! `clean: false` and no `concurrentInstancePid` (`anchorSeen` is never
//! consulted); and, only when all three are absent, the `at` of the last
//! `app_alive` of that run whose `workingSessionIds` contains that `sessionId`.
//! An interval no rule closes is **discarded, not extended**. A `busy` after its
//! run's `app_stop`, and an `idle` with no open `busy`, are each discarded and
//! counted rather than faulted: rotation and two-instance interleaving make both
//! legitimately reachable.
//!
//! `at` is authoritative for arithmetic and file position is the tiebreak for
//! equal `at`; records reach the file in lock-acquisition order, not edge order.
//! Lines that do not parse, or that parse but lack `v`, `at`, `runId` or
//! `event`, are skipped and counted.
//!
//! # Failure behavior
//!
//! Telemetry never degrades AgentsCommander. No public function here returns an
//! error or panics: I/O failures warn and are swallowed, a record that fails to
//! serialize is skipped while the rest of its batch continues, and a poisoned
//! mutex is recovered with `into_inner()`. `append` and `append_batch` are inert
//! no-ops until [`init_run`] stores a sink, which happens only in `lib.rs::run()`
//! - so unit tests and every CLI subcommand write nothing, by construction.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::session::profile::{idle_tuning_for, CodingAgentKind};
use crate::session::session::Session;

/// Schema version stamped on every record as `v`.
const SCHEMA_VERSION: u32 = 1;

/// Live-file size at or above which an append rotates first. With the heartbeat
/// volume this is roughly three weeks to four months of retained history across
/// the four kept generations.
const ACTIVITY_MAX_BYTES: u64 = 16 * 1024 * 1024;

/// Retained rotated generations: `activity.jsonl.1` through `.4`.
const ACTIVITY_KEEP: u32 = 4;

/// Bytes read back from the end of a file during the startup scan. Sized well
/// above the post-`app_stop` burst ceiling of `2 * session_count + 2` records.
const TAIL_READ_BYTES: u64 = 256 * 1024;

/// Coalescing window. Governs only the `continuesBlock` annotation, never the
/// recorded busy time, which is summed from raw pairs.
const BLOCK_WINDOW: Duration = Duration::from_secs(30);

const FILE_NAME: &str = crate::config::instance_artifacts::ACTIVITY_LOG_FILE_NAME;
const EVENT_APP_START: &str = "app_start";
const EVENT_APP_STOP: &str = "app_stop";

/// One line of `activity.jsonl`.
///
/// `Debug` is required because this type travels inside
/// `session::manager::CommitResult`, which derives it.
#[derive(Debug, Clone, Serialize)]
pub struct ActivityRecord {
    pub v: u32,
    pub at: String,
    #[serde(rename = "runId")]
    pub run_id: String,
    #[serde(flatten)]
    pub payload: ActivityPayload,
}

/// Event-specific body. Internally tagged by `event`, so each variant carries
/// exactly the fields its event is documented to carry and no others.
#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "event",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ActivityPayload {
    /// A session's working-state edge from false to true.
    Busy {
        session_id: Uuid,
        name: String,
        cwd: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        agent_kind: Option<CodingAgentKind>,
        reason: BusyReason,
        continues_block: bool,
        /// Present exactly when `continues_block` is true. There is no third case.
        #[serde(skip_serializing_if = "Option::is_none")]
        gap_ms: Option<u64>,
    },
    /// A session's working-state edge from true to false.
    Idle {
        session_id: Uuid,
        name: String,
        cwd: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        agent_kind: Option<CodingAgentKind>,
        reason: IdleReason,
        /// Marker for "this close waited out the idle threshold". Present only
        /// on `reason: "mark_idle"`; its absence is what tells the consumer not
        /// to subtract anything.
        #[serde(skip_serializing_if = "Option::is_none")]
        idle_threshold_ms: Option<u64>,
    },
    AppStart {
        pid: u32,
        app_version: String,
        previous_run_scan: PreviousRunScan,
        /// `null` when the scan reported `empty` or `unreadable`.
        previous_run: Option<PreviousRun>,
        /// Present only when another live process holds the previous
        /// `daemon.pid`. Its presence forbids recovery-based closing.
        #[serde(skip_serializing_if = "Option::is_none")]
        concurrent_instance_pid: Option<u32>,
        metrics: MetricsDeclaration,
    },
    /// Heartbeat. The detector's view of which sessions are working, which is
    /// also the universal backstop for closing edges no mutation site emits.
    AppAlive { working_session_ids: Vec<Uuid> },
    AppStop {
        open_sessions_enumerated: bool,
        open_session_count: usize,
    },
    /// Re-declares the metrics contract as the first record of a freshly rotated
    /// file, so the self-describing contract survives eviction of every
    /// `app_start`. Deliberately NOT a second `app_start`: it must not become an
    /// anchor for its run, and it carries no fact it cannot back.
    Metrics { metrics: MetricsDeclaration },
}

/// Why a `busy` record was emitted. Fixed per call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BusyReason {
    /// The opening edge at session birth, from `commit_selection_transition`.
    SessionStart,
    MarkBusy,
    PtyInputBoundary,
}

/// Why an `idle` record was emitted. Fixed per call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IdleReason {
    /// The only close that waited out the idle threshold.
    MarkIdle,
    /// A synthetic close written during shutdown. Waits out nothing.
    AppStop,
}

/// How the startup scan for the previous run turned out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviousRunScan {
    /// The live region answered.
    Ok,
    /// The answer came from `activity.jsonl.1`.
    RecoveredFromRotated,
    /// Neither region yielded a record.
    Empty,
    /// I/O error.
    Unreadable,
}

/// What the previous run left behind, as read from the scanned region.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviousRun {
    pub run_id: String,
    pub last_record_at: String,
    pub last_event: String,
    /// An `app_stop` for this run appeared anywhere in the scanned region. NOT
    /// "the last record is `app_stop`": threads that can append after
    /// `app_stop` are still running during teardown.
    pub clean: bool,
    /// An anchor (`app_start` or `app_stop`) for this run appeared in the
    /// scanned region. **Diagnostic only.** On a long-running crashed instance
    /// the `app_start` can be hours outside the region, so gating recovery on
    /// this would disable recovery for exactly the runs that need it.
    pub anchor_seen: bool,
}

/// The normative statement of which metrics this file supports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MetricsDeclaration {
    pub supported: Vec<&'static str>,
    pub unsupported: Vec<&'static str>,
}

/// The four fields an `idle` record needs, sampled from a live session without
/// holding any lock afterwards. Produced by
/// `SessionManager::try_snapshot_working_sessions` for the shutdown path.
///
/// Deliberately carries nothing else: no `token`, no `last_prompt`, no
/// `shell_args` and no `effective_shell_args`, so no shutdown record can leak a
/// secret or a payload.
#[derive(Debug, Clone)]
pub struct WorkingSessionSnapshot {
    pub id: Uuid,
    pub name: String,
    pub cwd: String,
    pub agent_kind: Option<CodingAgentKind>,
}

/// Outcome of a startup scan: the label plus the previous run it identified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviousRunReport {
    pub scan: PreviousRunScan,
    pub previous_run: Option<PreviousRun>,
}

struct Sink {
    path: PathBuf,
    run_id: String,
}

/// Sink identity. Set once, only by [`init_run`]. While unset, every append is a
/// no-op, which is what keeps unit tests and CLI subcommands out of a live log.
static SINK: OnceLock<Sink> = OnceLock::new();

/// Serializes the rotate-check-plus-write critical section. Always the innermost
/// lock: it is taken only inside the append path, which is called with no
/// manager or detector lock held.
static WRITER: OnceLock<Mutex<()>> = OnceLock::new();

/// When each session last went idle, in both clock domains.
type LastIdleAt = HashMap<Uuid, (Instant, DateTime<Utc>)>;

/// `last_idle_at` per session, the module's only emitter state. Every value
/// duplicates a fact already durably on disk, so losing it costs exactly one
/// annotation and cannot lose, invent, shorten or lengthen a millisecond of
/// busy time. Taken only while the manager's write guard is held, so at most one
/// thread is ever inside it.
static COALESCER: OnceLock<Mutex<LastIdleAt>> = OnceLock::new();

fn writer_lock() -> &'static Mutex<()> {
    WRITER.get_or_init(|| Mutex::new(()))
}

fn coalescer() -> &'static Mutex<LastIdleAt> {
    COALESCER.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Locks the coalescer, recovering from poison, and drops entries older than the
/// window on the way in so the map stays bounded even if pruning never had
/// another reason to run.
fn locked_coalescer(now: Instant) -> MutexGuard<'static, LastIdleAt> {
    let mut guard = coalescer()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    guard.retain(|_, (stamped, _)| {
        now.checked_duration_since(*stamped)
            .is_some_and(|delta| delta <= BLOCK_WINDOW)
    });
    guard
}

fn run_id() -> String {
    SINK.get()
        .map(|sink| sink.run_id.clone())
        .unwrap_or_default()
}

fn sink_path() -> Option<PathBuf> {
    SINK.get().map(|sink| sink.path.clone())
}

/// The canonical `at` encoding. Byte-identical to
/// `phone::types::canonical_pty_timestamp`, pinned by
/// `timestamp_format_is_byte_identical_to_canonical_pty_timestamp`.
fn stamp(now: DateTime<Utc>) -> String {
    now.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn metrics_declaration() -> MetricsDeclaration {
    MetricsDeclaration {
        supported: vec!["totalBusyMs", "totalBusyRawMs"],
        unsupported: vec!["intervalCount", "blockCount"],
    }
}

/// Build the opening edge for `session`, stamping `at` and deciding the block
/// annotation in the same breath.
///
/// Contains no `log::*` call, deliberately: every production caller runs inside
/// `SessionManager::state`'s write guard, and `log::*` reaches `logging.rs`,
/// whose rotation holds a `Mutex<File>` and can rename a 50 MiB file. The caller
/// logs after releasing.
pub fn build_busy(id: Uuid, session: &Session, reason: BusyReason) -> ActivityRecord {
    let now_utc = Utc::now();
    let now_instant = Instant::now();
    let gap_ms = {
        let previous = locked_coalescer(now_instant);
        previous.get(&id).and_then(|(last_instant, last_utc)| {
            // Both clock domains must agree the gap is inside the window, so a
            // wall-clock jump or a suspend can only suppress a continuation.
            let instant_gap = now_instant.checked_duration_since(*last_instant)?;
            let utc_gap_ms = now_utc.signed_duration_since(*last_utc).num_milliseconds();
            let window_ms = BLOCK_WINDOW.as_millis() as i64;
            (instant_gap < BLOCK_WINDOW && (0..window_ms).contains(&utc_gap_ms))
                .then_some(instant_gap.as_millis() as u64)
        })
    };
    ActivityRecord {
        v: SCHEMA_VERSION,
        at: stamp(now_utc),
        run_id: run_id(),
        payload: ActivityPayload::Busy {
            session_id: id,
            name: session.name.clone(),
            cwd: session.working_directory.clone(),
            agent_kind: session.agent_kind,
            reason,
            continues_block: gap_ms.is_some(),
            gap_ms,
        },
    }
}

/// Build the closing edge for `session` and record `last_idle_at` so the next
/// opening edge can annotate its block.
///
/// Contains no `log::*` call; see [`build_busy`].
pub fn build_idle(id: Uuid, session: &Session, reason: IdleReason) -> ActivityRecord {
    let now_utc = Utc::now();
    let now_instant = Instant::now();
    locked_coalescer(now_instant).insert(id, (now_instant, now_utc));
    ActivityRecord {
        v: SCHEMA_VERSION,
        at: stamp(now_utc),
        run_id: run_id(),
        payload: ActivityPayload::Idle {
            session_id: id,
            name: session.name.clone(),
            cwd: session.working_directory.clone(),
            agent_kind: session.agent_kind,
            reason,
            idle_threshold_ms: match reason {
                IdleReason::MarkIdle => Some(
                    idle_tuning_for(session.agent_kind)
                        .idle_threshold
                        .as_millis() as u64,
                ),
                IdleReason::AppStop => None,
            },
        },
    }
}

/// Build a synthetic closing edge from a shutdown snapshot.
///
/// **Touches no coalescer state**, which is what keeps "the coalescer is only
/// ever taken under the manager's write guard" true: this runs on the main
/// thread inside `RunEvent::Exit` with no such guard held. Writing
/// `last_idle_at` in a dying process would be pointless anyway.
///
/// Carries no `idleThresholdMs`: nothing waited out a threshold here.
///
/// Contains no `log::*` call; see [`build_busy`].
pub fn build_idle_from_snapshot(
    snapshot: &WorkingSessionSnapshot,
    reason: IdleReason,
) -> ActivityRecord {
    ActivityRecord {
        v: SCHEMA_VERSION,
        at: stamp(Utc::now()),
        run_id: run_id(),
        payload: ActivityPayload::Idle {
            session_id: snapshot.id,
            name: snapshot.name.clone(),
            cwd: snapshot.cwd.clone(),
            agent_kind: snapshot.agent_kind,
            reason,
            idle_threshold_ms: None,
        },
    }
}

/// Build the heartbeat record from the detector's view of who is working.
pub fn build_app_alive(working_session_ids: Vec<Uuid>) -> ActivityRecord {
    ActivityRecord {
        v: SCHEMA_VERSION,
        at: stamp(Utc::now()),
        run_id: run_id(),
        payload: ActivityPayload::AppAlive {
            working_session_ids,
        },
    }
}

/// Build the shutdown marker. `open_sessions_enumerated: false` is a designed
/// outcome, not a failure: the consumer's closing rule closes every open
/// interval at this record's `at` regardless, so the enumerated `idle` records
/// are pure precision.
pub fn build_app_stop(open_sessions_enumerated: bool, open_session_count: usize) -> ActivityRecord {
    ActivityRecord {
        v: SCHEMA_VERSION,
        at: stamp(Utc::now()),
        run_id: run_id(),
        payload: ActivityPayload::AppStop {
            open_sessions_enumerated,
            open_session_count,
        },
    }
}

fn metrics_record(run_id: String) -> ActivityRecord {
    ActivityRecord {
        v: SCHEMA_VERSION,
        at: stamp(Utc::now()),
        run_id,
        payload: ActivityPayload::Metrics {
            metrics: metrics_declaration(),
        },
    }
}

/// Append one record. A pure serialize-and-write: the record already carries its
/// own `at`, `runId` and block annotation, all stamped by the builder under
/// whatever guard the call site held.
///
/// Inert until [`init_run`] has stored a sink.
pub fn append(record: ActivityRecord) {
    #[cfg(test)]
    capture::note(std::slice::from_ref(&record));
    let Some(path) = sink_path() else { return };
    append_at(&path, std::slice::from_ref(&record));
}

/// Append several records in one open-write-close.
///
/// Inert until [`init_run`] has stored a sink.
pub fn append_batch(records: &[ActivityRecord]) {
    #[cfg(test)]
    capture::note(records);
    if records.is_empty() {
        return;
    }
    let Some(path) = sink_path() else { return };
    append_at(&path, records);
}

/// Test-only, in-memory mirror of the emission calls.
///
/// No sink is ever configured in a test process, which is the property that
/// keeps unit tests out of a live log. The same property makes the record an
/// emission site produced invisible to a test, because the mutation sites hold
/// it in a local and hand it straight to [`append`]. Mirroring the call here is
/// what lets `session/manager.rs` assert "exactly one record" without a file.
///
/// This is NOT a sink: nothing is written, nothing is read back by production
/// code, and each `#[test]` owns its own thread-local buffer.
#[cfg(test)]
pub(crate) mod capture {
    use super::ActivityRecord;
    use std::cell::RefCell;

    thread_local! {
        static CAPTURED: RefCell<Vec<ActivityRecord>> = const { RefCell::new(Vec::new()) };
    }

    pub(crate) fn note(records: &[ActivityRecord]) {
        CAPTURED.with(|captured| captured.borrow_mut().extend(records.iter().cloned()));
    }

    /// Take everything emitted on this thread since the last call.
    pub(crate) fn drain() -> Vec<ActivityRecord> {
        CAPTURED.with(|captured| std::mem::take(&mut *captured.borrow_mut()))
    }
}

/// Path-parameterized append core, and the test seam for every append.
///
/// Holds the writer mutex across the rotation check and the write, so within one
/// process a line can never interleave. Cross-process atomicity is NOT claimed:
/// `FILE_APPEND_DATA` guarantees an atomic seek-to-end but carries no documented
/// atomicity for an arbitrary-length `WriteFile`, which is why the consumer
/// contract specifies skip-and-count for torn lines.
fn append_at(path: &Path, records: &[ActivityRecord]) {
    if records.is_empty() {
        return;
    }
    let _writer = writer_lock()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let mut buffer = String::new();
    if rotate_if_needed(path) {
        // The fresh file's first line re-declares the metrics contract, for the
        // same run as the record that triggered the rotation.
        let run_id = records
            .first()
            .map(|record| record.run_id.clone())
            .unwrap_or_default();
        push_line(&mut buffer, &metrics_record(run_id));
    }
    for record in records {
        push_line(&mut buffer, record);
    }
    if buffer.is_empty() {
        return;
    }
    if let Err(error) = write_append(path, &buffer) {
        log::warn!(
            "[activity] append to {} failed (continuing): {}",
            path.display(),
            error
        );
    }
}

/// Serialize one record onto `buffer`. A record that fails to serialize is
/// skipped; the rest of the batch still lands.
fn push_line(buffer: &mut String, record: &ActivityRecord) {
    match serde_json::to_string(record) {
        Ok(line) => {
            buffer.push_str(&line);
            buffer.push('\n');
        }
        Err(error) => log::warn!("[activity] record skipped, serialization failed: {}", error),
    }
}

/// Opened per call, so no handle is held across a rename and rotation cannot
/// fail on Windows for sharing reasons.
fn write_append(path: &Path, buffer: &str) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(buffer.as_bytes())
}

/// Shift `activity.jsonl.<i>` up one slot and move the live file into `.1`.
/// Returns whether the live file was actually rotated away.
///
/// Best-effort throughout: on failure the live file keeps growing past the cap
/// until a later rotation succeeds.
fn rotate_if_needed(path: &Path) -> bool {
    let size = match std::fs::metadata(path) {
        Ok(metadata) => metadata.len(),
        // No file yet, or unreadable: nothing to rotate.
        Err(_) => return false,
    };
    if size < ACTIVITY_MAX_BYTES {
        return false;
    }
    let Some(parent) = path.parent() else {
        return false;
    };
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    // Descending, so each rename atomically replaces its destination and the old
    // `.ACTIVITY_KEEP` is evicted by the first iteration.
    for index in (1..ACTIVITY_KEEP).rev() {
        let from = parent.join(format!("{name}.{index}"));
        if !from.exists() {
            continue;
        }
        let to = parent.join(format!("{name}.{}", index + 1));
        if let Err(error) = std::fs::rename(&from, &to) {
            log::warn!(
                "[activity] rotation could not shift {} to {} (continuing): {}",
                from.display(),
                to.display(),
                error
            );
        }
    }
    let first = parent.join(format!("{name}.1"));
    if let Err(error) = std::fs::rename(path, &first) {
        log::warn!(
            "[activity] rotation of {} failed, leaving the active file in place: {}",
            path.display(),
            error
        );
        return false;
    }
    true
}

/// Start this process's run: store the sink, scan for the previous run, and
/// append `app_start`.
///
/// Called exactly once, from `lib.rs::run()`, before the Tauri builder exists.
/// Every append before this call is a no-op.
pub fn init_run(dir: &Path, run_id: &str, enabled: bool) {
    if !enabled {
        log::info!(
            "[activity] recording disabled by settings (activityLogEnabled=false); \
             activity.jsonl will not be written this run"
        );
        return;
    }
    // First-wins: a second call would be a programming error, and silently
    // keeping the first sink is the fail-soft direction.
    let _ = SINK.set(Sink {
        path: dir.join(FILE_NAME),
        run_id: run_id.to_string(),
    });
    init_run_at(dir, run_id);
}

/// Path-parameterized startup work, and the test seam for it. Writes only under
/// `dir`, so a test can never touch the live binary's `activity.jsonl` or read
/// the developer's real `daemon.pid`.
fn init_run_at(dir: &Path, run_id: &str) {
    let report = scan_previous_run_at(dir);
    let scan = report.scan;
    // Path-parameterized on purpose: at this point `daemon.pid` still holds the
    // PREVIOUS writer's PID, and the argument-free detector would resolve the
    // live config dir instead of `dir`.
    let concurrent_instance_pid =
        match crate::config::daemon_pid::detect_daemon_state_at(&dir.join("daemon.pid")) {
            crate::config::daemon_pid::DaemonState::Running { pid }
                if pid != std::process::id() =>
            {
                Some(pid)
            }
            _ => None,
        };
    let record = ActivityRecord {
        v: SCHEMA_VERSION,
        at: stamp(Utc::now()),
        run_id: run_id.to_string(),
        payload: ActivityPayload::AppStart {
            pid: std::process::id(),
            // `init_run` runs before the Tauri builder, so `app.package_info()`
            // is unavailable.
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            previous_run_scan: scan,
            previous_run: report.previous_run,
            concurrent_instance_pid,
            metrics: metrics_declaration(),
        },
    };
    // After the scan, so a rotation triggered by this very append cannot
    // invalidate what the scan just read.
    append_at(&dir.join(FILE_NAME), std::slice::from_ref(&record));
    log::info!(
        "[activity] run {} started, previous-run scan {:?}, concurrent instance {:?}",
        run_id,
        scan,
        concurrent_instance_pid
    );
}

/// Scan the live config directory for the previous run. Counterpart to
/// [`scan_previous_run_at`], mirroring `daemon_pid::detect_daemon_state`.
pub fn scan_previous_run() -> PreviousRunReport {
    match crate::config::config_dir() {
        Some(dir) => scan_previous_run_at(&dir),
        None => PreviousRunReport {
            scan: PreviousRunScan::Empty,
            previous_run: None,
        },
    }
}

/// One record as the scan needs it: the four fields every valid line carries.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ScannedRecord {
    run_id: String,
    event: String,
    at: String,
}

enum RegionRead {
    Records(Vec<ScannedRecord>),
    /// No file, no bytes, or no parseable record.
    Empty,
    Unreadable,
}

/// Path-parameterized previous-run scan.
fn scan_previous_run_at(dir: &Path) -> PreviousRunReport {
    let live_path = dir.join(FILE_NAME);
    let rotated_path = dir.join(format!("{FILE_NAME}.1"));

    let live = match read_tail_records(&live_path) {
        RegionRead::Records(records) => records,
        RegionRead::Empty => Vec::new(),
        RegionRead::Unreadable => {
            return PreviousRunReport {
                scan: PreviousRunScan::Unreadable,
                previous_run: None,
            }
        }
    };

    // Fallback clause 1: the live region yields no parseable record at all. That
    // is one truncated line from a crash mid-write, and therefore exactly the
    // case this feature exists to detect. It is also sticky without the
    // fallback: the condition would persist until the next rotation.
    let Some(candidate) = live.last().cloned() else {
        let rotated = read_rotated_records(&rotated_path);
        let Some(rotated_candidate) = rotated.last().cloned() else {
            return PreviousRunReport {
                scan: PreviousRunScan::Empty,
                previous_run: None,
            };
        };
        return PreviousRunReport {
            scan: PreviousRunScan::RecoveredFromRotated,
            previous_run: Some(summarize(&rotated, &rotated_candidate)),
        };
    };

    if has_anchor(&live, &candidate.run_id) {
        // The live region answered; `.1` is never opened.
        return PreviousRunReport {
            scan: PreviousRunScan::Ok,
            previous_run: Some(summarize(&live, &candidate)),
        };
    }

    // Fallback clause 2: the rotation boundary. A run appends `app_stop`, a
    // teardown `busy` pushes the file over the cap, and that `busy` lands alone
    // in a fresh live file. The live tail then does yield a parseable record, so
    // clause 1 never fires, yet a genuinely clean exit would read unclean.
    let mut merged = read_rotated_records(&rotated_path);
    let recovered = has_anchor(&merged, &candidate.run_id);
    merged.extend(live);
    PreviousRunReport {
        scan: if recovered {
            PreviousRunScan::RecoveredFromRotated
        } else {
            PreviousRunScan::Ok
        },
        previous_run: Some(summarize(&merged, &candidate)),
    }
}

fn summarize(region: &[ScannedRecord], candidate: &ScannedRecord) -> PreviousRun {
    PreviousRun {
        run_id: candidate.run_id.clone(),
        last_record_at: candidate.at.clone(),
        last_event: candidate.event.clone(),
        clean: region
            .iter()
            .any(|record| record.run_id == candidate.run_id && record.event == EVENT_APP_STOP),
        anchor_seen: has_anchor(region, &candidate.run_id),
    }
}

/// An anchor is an `app_start` OR an `app_stop` for that run.
fn has_anchor(region: &[ScannedRecord], run_id: &str) -> bool {
    region.iter().any(|record| {
        record.run_id == run_id
            && (record.event == EVENT_APP_START || record.event == EVENT_APP_STOP)
    })
}

fn read_rotated_records(path: &Path) -> Vec<ScannedRecord> {
    #[cfg(test)]
    tests::note_rotated_read();
    match read_tail_records(path) {
        RegionRead::Records(records) => records,
        RegionRead::Empty | RegionRead::Unreadable => Vec::new(),
    }
}

/// Read the last [`TAIL_READ_BYTES`] of `path` and return every valid record in
/// file order.
fn read_tail_records(path: &Path) -> RegionRead {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        // A missing file recorded nothing; that is empty, not broken.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return RegionRead::Empty,
        Err(_) => return RegionRead::Unreadable,
    };
    let len = match file.metadata() {
        Ok(metadata) => metadata.len(),
        Err(_) => return RegionRead::Unreadable,
    };
    if len == 0 {
        return RegionRead::Empty;
    }
    // Clamped: a bare `End(-TAIL_READ_BYTES)` on a short file is an error on
    // Windows.
    let window = len.min(TAIL_READ_BYTES);
    if file.seek(SeekFrom::End(-(window as i64))).is_err() {
        return RegionRead::Unreadable;
    }
    let mut bytes = Vec::with_capacity(window as usize);
    if file.read_to_end(&mut bytes).is_err() {
        return RegionRead::Unreadable;
    }
    // The seek point can land mid-codepoint and `name`/`cwd` come from
    // user-chosen paths, so decode lossily rather than degrading the whole scan
    // to "unreadable". Everything below slices a validated `str`, never the raw
    // byte buffer.
    let text = String::from_utf8_lossy(&bytes);
    let body = if len > window {
        // The read started past byte 0, so the first line is a fragment. Only
        // then: applied unconditionally this would drop the first line of every
        // file smaller than the window.
        match text.find('\n') {
            Some(index) => &text[index + 1..],
            None => "",
        }
    } else {
        text.as_ref()
    };
    let records: Vec<ScannedRecord> = body.lines().filter_map(parse_record).collect();
    if records.is_empty() {
        RegionRead::Empty
    } else {
        RegionRead::Records(records)
    }
}

/// A line is a record only if it parses as a JSON object carrying `v`, `at`,
/// `runId` and `event`. A partially written final line is the expected state
/// after a hard kill, so this is a filter and never an error.
fn parse_record(line: &str) -> Option<ScannedRecord> {
    let value: Value = serde_json::from_str(line).ok()?;
    let object = value.as_object()?;
    object.get("v")?;
    Some(ScannedRecord {
        at: object.get("at")?.as_str()?.to_string(),
        run_id: object.get("runId")?.as_str()?.to_string(),
        event: object.get("event")?.as_str()?.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pty::backend::SessionBackendKind;
    use crate::session::session::SessionStatus;
    use serde_json::json;
    use std::cell::Cell;
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    // ---------------------------------------------------------------------
    // Probes. `read_rotated_records` is the single place that opens the
    // rotated file, so a per-thread counter proves whether the scan touched
    // it. Each `#[test]` owns its thread, so the count needs no
    // synchronization and cannot be perturbed by a parallel test.
    // ---------------------------------------------------------------------

    thread_local! {
        static ROTATED_READS: Cell<usize> = const { Cell::new(0) };
    }

    pub(super) fn note_rotated_read() {
        ROTATED_READS.with(|count| count.set(count.get() + 1));
    }

    fn reset_rotated_reads() {
        ROTATED_READS.with(|count| count.set(0));
    }

    fn rotated_reads() -> usize {
        ROTATED_READS.with(Cell::get)
    }

    /// Insert straight into the coalescer, bypassing the pruning that
    /// [`locked_coalescer`] applies, so a test can plant an entry the emitter
    /// would otherwise have dropped.
    fn seed_coalescer(id: Uuid, stamped: Instant, wall: DateTime<Utc>) {
        coalescer()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert(id, (stamped, wall));
    }

    fn coalescer_entry(id: Uuid) -> Option<(Instant, DateTime<Utc>)> {
        coalescer()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(&id)
            .copied()
    }

    fn ago(seconds: u64) -> Instant {
        let now = Instant::now();
        now.checked_sub(Duration::from_secs(seconds)).unwrap_or(now)
    }

    // ---------------------------------------------------------------------
    // Fixtures
    // ---------------------------------------------------------------------

    fn sample_session(id: Uuid, agent_kind: Option<CodingAgentKind>) -> Session {
        Session {
            id,
            name: "wg-14-dev-team/dev-rust".to_string(),
            shell: "claude".to_string(),
            shell_args: vec!["--dangerously-skip-permissions".to_string()],
            backend_kind: SessionBackendKind::LocalProcess,
            effective_shell_args: None,
            created_at: Utc::now(),
            working_directory: "C:\\repos\\ac\\.ac\\wg-14-dev-team\\__agent_dev-rust".to_string(),
            status: SessionStatus::Running,
            waiting_for_input: false,
            communication: None,
            pending_review: false,
            last_prompt: None,
            agent_id: None,
            agent_label: None,
            git_repos: Vec::new(),
            is_coordinator: false,
            is_root_agent: false,
            git_repos_gen: 0,
            agent_turn_armed: false,
            token: Uuid::new_v4(),
            agent_kind,
            requested_profile: None,
            effective_profile: None,
            profile_fallback_chain: Vec::new(),
            profile_fallback_applied: false,
            effective_codex_home: None,
            resolved_claude_projects_dir: None,
            profile_content_hash: None,
            trusted_configured_spawn: false,
            telegram_bot_id: None,
            was_detached: false,
            detached_geometry: None,
            start_fresh_on_restore: false,
            context_percent: None,
        }
    }

    fn sample_snapshot(id: Uuid) -> WorkingSessionSnapshot {
        WorkingSessionSnapshot {
            id,
            name: "wg-14-dev-team/tech-lead".to_string(),
            cwd: "C:\\repos\\ac\\.ac\\wg-14-dev-team\\__agent_tech-lead".to_string(),
            agent_kind: Some(CodingAgentKind::Claude),
        }
    }

    fn json_of(record: &ActivityRecord) -> Value {
        serde_json::to_value(record).expect("record serializes")
    }

    fn lines_of(path: &Path) -> Vec<String> {
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn parsed_lines(path: &Path) -> Vec<Value> {
        lines_of(path)
            .iter()
            .map(|line| serde_json::from_str::<Value>(line).expect("line parses as JSON"))
            .collect()
    }

    /// A file at exactly [`ACTIVITY_MAX_BYTES`], made of valid records plus a
    /// trailing fragment so the byte count lands on the cap.
    fn write_file_at_cap(path: &Path, run_id: &str) {
        let record = json!({
            "v": 1,
            "at": at_offset(0),
            "runId": run_id,
            "event": "app_alive",
            "workingSessionIds": [],
        });
        let line = format!("{record}\n");
        let cap = ACTIVITY_MAX_BYTES as usize;
        let mut content = String::with_capacity(cap + line.len());
        while content.len() + line.len() <= cap {
            content.push_str(&line);
        }
        while content.len() < cap {
            content.push('x');
        }
        std::fs::write(path, &content).expect("write a file at the cap");
        assert_eq!(
            std::fs::metadata(path).expect("metadata").len(),
            ACTIVITY_MAX_BYTES
        );
    }

    const BASE_AT: &str = "2026-07-26T00:00:00.000Z";

    fn base_time() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(BASE_AT)
            .expect("base timestamp")
            .with_timezone(&Utc)
    }

    fn at_offset(millis: i64) -> String {
        stamp(base_time() + chrono::Duration::milliseconds(millis))
    }

    // ---------------------------------------------------------------------
    // §10.1 - schema, sink, rotation
    // ---------------------------------------------------------------------

    #[test]
    fn append_at_writes_exactly_one_line_per_record() {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join(FILE_NAME);
        let session = sample_session(Uuid::new_v4(), None);
        append_at(
            &path,
            &[build_busy(session.id, &session, BusyReason::MarkBusy)],
        );
        append_at(
            &path,
            &[build_idle(session.id, &session, IdleReason::MarkIdle)],
        );
        assert_eq!(lines_of(&path).len(), 2);
    }

    #[test]
    fn append_batch_writes_all_lines_in_one_open() {
        // `append_batch` delegates to `append_at`, which opens the file once
        // before its write loop; the observable property is that every record
        // of the batch lands, in order.
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join(FILE_NAME);
        let first = sample_session(Uuid::new_v4(), None);
        let second = sample_session(Uuid::new_v4(), None);
        let batch = vec![
            build_idle(first.id, &first, IdleReason::AppStop),
            build_idle(second.id, &second, IdleReason::AppStop),
            build_app_stop(true, 2),
        ];
        append_at(&path, &batch);
        let events: Vec<String> = parsed_lines(&path)
            .iter()
            .map(|value| value["event"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(events, vec!["idle", "idle", "app_stop"]);
    }

    #[test]
    fn every_written_line_parses_as_json_and_carries_v_at_run_id_event() {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join(FILE_NAME);
        let session = sample_session(Uuid::new_v4(), Some(CodingAgentKind::Claude));
        append_at(
            &path,
            &[
                build_busy(session.id, &session, BusyReason::SessionStart),
                build_idle(session.id, &session, IdleReason::MarkIdle),
                build_app_alive(vec![session.id]),
                build_app_stop(false, 0),
            ],
        );
        for line in lines_of(&path) {
            let record = parse_record(&line);
            assert!(record.is_some(), "line lacks a required field: {line}");
        }
    }

    #[test]
    fn timestamp_format_is_byte_identical_to_canonical_pty_timestamp() {
        for offset in [0_i64, 1, 999, 1_000, 86_399_999] {
            let now = base_time() + chrono::Duration::milliseconds(offset);
            assert_eq!(
                stamp(now),
                crate::phone::types::canonical_pty_timestamp(now)
            );
        }
        assert_eq!(
            stamp(base_time()),
            crate::phone::types::canonical_pty_timestamp(base_time())
        );
    }

    #[test]
    fn rotation_shifts_suffixes_and_evicts_the_oldest() {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join(FILE_NAME);
        write_file_at_cap(&path, "live-run");
        for index in 1..=ACTIVITY_KEEP {
            std::fs::write(
                dir.path().join(format!("{FILE_NAME}.{index}")),
                format!("generation-{index}\n"),
            )
            .expect("seed a rotated generation");
        }

        assert!(rotate_if_needed(&path), "a file at the cap must rotate");

        let generation = |index: u32| {
            std::fs::read_to_string(dir.path().join(format!("{FILE_NAME}.{index}"))).ok()
        };
        assert!(!path.exists(), "the live file moved into the .1 slot");
        assert_eq!(generation(2).as_deref(), Some("generation-1\n"));
        assert_eq!(generation(3).as_deref(), Some("generation-2\n"));
        assert_eq!(generation(4).as_deref(), Some("generation-3\n"));
        assert_eq!(
            generation(ACTIVITY_KEEP + 1),
            None,
            "retention must not grow past ACTIVITY_KEEP"
        );
        let rotated = generation(1).expect(".1 holds the former live file");
        assert!(
            !rotated.starts_with("generation-"),
            ".1 must hold the former live file, not a marker"
        );
    }

    #[test]
    fn rotation_is_skipped_below_the_cap() {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join(FILE_NAME);
        std::fs::write(&path, "well below the cap\n").expect("seed a small file");
        assert!(!rotate_if_needed(&path));
        assert!(path.exists());
        assert!(!dir.path().join(format!("{FILE_NAME}.1")).exists());
    }

    #[test]
    fn metrics_is_re_emitted_as_the_first_record_of_a_freshly_rotated_file() {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join(FILE_NAME);
        write_file_at_cap(&path, "previous-run");
        let session = sample_session(Uuid::new_v4(), None);
        append_at(
            &path,
            &[build_busy(session.id, &session, BusyReason::MarkBusy)],
        );

        let records = parsed_lines(&path);
        assert_eq!(
            records.len(),
            2,
            "the fresh file holds metrics plus the record"
        );
        assert_eq!(records[0]["event"], "metrics");
        assert_eq!(
            records[0]["metrics"]["unsupported"],
            json!(["intervalCount", "blockCount"])
        );
        assert_eq!(records[0]["v"], json!(SCHEMA_VERSION));
        assert_eq!(records[1]["event"], "busy");
    }

    // ---------------------------------------------------------------------
    // §10.1 - the coalescing annotation
    // ---------------------------------------------------------------------

    #[test]
    fn first_busy_for_a_session_has_continues_block_false_and_no_gap_ms() {
        let session = sample_session(Uuid::new_v4(), None);
        let record = json_of(&build_busy(session.id, &session, BusyReason::SessionStart));
        assert_eq!(record["continuesBlock"], json!(false));
        assert!(
            record.get("gapMs").is_none(),
            "gapMs is present exactly when continuesBlock is true"
        );
    }

    #[test]
    fn busy_within_the_window_has_continues_block_true_and_reports_gap_ms() {
        let session = sample_session(Uuid::new_v4(), None);
        seed_coalescer(
            session.id,
            ago(8),
            Utc::now() - chrono::Duration::seconds(8),
        );
        let record = json_of(&build_busy(session.id, &session, BusyReason::MarkBusy));
        assert_eq!(record["continuesBlock"], json!(true));
        let gap = record["gapMs"]
            .as_u64()
            .expect("gapMs accompanies a continuation");
        assert!(
            (7_500..=9_000).contains(&gap),
            "gapMs must report the Instant delta, got {gap}"
        );
    }

    #[test]
    fn busy_after_the_window_has_continues_block_false_and_no_gap_ms() {
        let session = sample_session(Uuid::new_v4(), None);
        seed_coalescer(
            session.id,
            ago(45),
            Utc::now() - chrono::Duration::seconds(45),
        );
        let record = json_of(&build_busy(session.id, &session, BusyReason::MarkBusy));
        assert_eq!(record["continuesBlock"], json!(false));
        assert!(record.get("gapMs").is_none());
    }

    #[test]
    fn a_backwards_utc_jump_suppresses_the_continuation_even_when_instant_is_under_the_window() {
        let session = sample_session(Uuid::new_v4(), None);
        // The Instant delta is a few seconds, but the wall clock moved
        // backwards after the idle was stamped, so the Utc delta is negative.
        seed_coalescer(
            session.id,
            ago(5),
            Utc::now() + chrono::Duration::seconds(3_600),
        );
        let record = json_of(&build_busy(session.id, &session, BusyReason::MarkBusy));
        assert_eq!(
            record["continuesBlock"],
            json!(false),
            "a clock jump may only suppress a continuation"
        );
        assert!(record.get("gapMs").is_none());
    }

    #[test]
    fn coalescer_state_prunes_entries_older_than_the_window() {
        let stale = Uuid::new_v4();
        seed_coalescer(stale, ago(120), Utc::now() - chrono::Duration::seconds(120));
        assert!(coalescer_entry(stale).is_some(), "seeded");

        let other = sample_session(Uuid::new_v4(), None);
        let _ = build_idle(other.id, &other, IdleReason::MarkIdle);

        assert!(
            coalescer_entry(stale).is_none(),
            "an entry past the window is dropped on the next write"
        );
    }

    #[test]
    fn build_idle_from_snapshot_does_not_read_or_write_the_coalescer() {
        let id = Uuid::new_v4();
        let stamped = ago(3);
        let wall = Utc::now() - chrono::Duration::seconds(3);
        seed_coalescer(id, stamped, wall);

        let record = json_of(&build_idle_from_snapshot(
            &sample_snapshot(id),
            IdleReason::AppStop,
        ));

        assert_eq!(
            coalescer_entry(id),
            Some((stamped, wall)),
            "the shutdown constructor must not write last_idle_at"
        );
        assert!(record.get("continuesBlock").is_none());
        assert!(record.get("gapMs").is_none());
    }

    #[test]
    fn build_idle_from_snapshot_omits_idle_threshold_ms() {
        let record = json_of(&build_idle_from_snapshot(
            &sample_snapshot(Uuid::new_v4()),
            IdleReason::AppStop,
        ));
        assert_eq!(record["reason"], "app_stop");
        assert!(
            record.get("idleThresholdMs").is_none(),
            "nothing waited out a threshold on the app_stop path"
        );
    }

    // ---------------------------------------------------------------------
    // §10.1 - fail-soft behavior
    // ---------------------------------------------------------------------

    #[test]
    fn append_with_no_sink_configured_is_a_no_op() {
        // No test ever calls `init_run`, so the sink is never configured in a
        // test process and this holds for every unit test in the crate.
        let dir = TempDir::new().expect("temp dir");
        let session = sample_session(Uuid::new_v4(), None);
        append(build_busy(session.id, &session, BusyReason::MarkBusy));
        append_batch(&[build_idle(session.id, &session, IdleReason::MarkIdle)]);
        assert!(
            !dir.path().join(FILE_NAME).exists(),
            "an unconfigured sink must write nothing"
        );
    }

    #[test]
    fn init_run_disabled_stores_no_sink_and_writes_nothing() {
        let dir = TempDir::new().expect("temp dir");
        init_run(dir.path(), "run-disabled", false);
        init_run(dir.path(), "run-disabled", false);
        assert!(sink_path().is_none());
        assert!(run_id().is_empty());
        assert!(!dir.path().join(FILE_NAME).exists());
        append(build_app_alive(vec![]));
        append_batch(&[build_app_stop(true, 0)]);
        assert!(
            !dir.path().join(FILE_NAME).exists(),
            "a disabled run must never create the activity file"
        );
    }

    #[test]
    fn append_at_an_unwritable_path_does_not_panic_and_returns() {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("does-not-exist").join(FILE_NAME);
        let session = sample_session(Uuid::new_v4(), None);
        append_at(
            &path,
            &[build_busy(session.id, &session, BusyReason::MarkBusy)],
        );
        assert!(!path.exists());
    }

    #[test]
    fn a_poisoned_writer_mutex_does_not_panic_a_later_append() {
        let handle = std::thread::spawn(|| {
            let _guard = writer_lock()
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            panic!("poison the writer mutex on purpose");
        });
        assert!(
            handle.join().is_err(),
            "the helper thread must have panicked"
        );

        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join(FILE_NAME);
        let session = sample_session(Uuid::new_v4(), None);
        append_at(
            &path,
            &[build_busy(session.id, &session, BusyReason::MarkBusy)],
        );
        assert_eq!(lines_of(&path).len(), 1);
    }

    #[test]
    fn a_poisoned_coalescer_mutex_does_not_panic_a_later_build() {
        let handle = std::thread::spawn(|| {
            let _guard = coalescer()
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            panic!("poison the coalescer mutex on purpose");
        });
        assert!(
            handle.join().is_err(),
            "the helper thread must have panicked"
        );

        let session = sample_session(Uuid::new_v4(), None);
        let idle = json_of(&build_idle(session.id, &session, IdleReason::MarkIdle));
        assert_eq!(idle["event"], "idle");
        let busy = json_of(&build_busy(session.id, &session, BusyReason::MarkBusy));
        assert_eq!(busy["continuesBlock"], json!(true));
    }

    #[test]
    fn concurrent_appends_never_interleave_a_line() {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join(FILE_NAME);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let path = path.as_path();
                scope.spawn(move || {
                    for _ in 0..100 {
                        let session = sample_session(Uuid::new_v4(), None);
                        append_at(
                            path,
                            &[build_busy(session.id, &session, BusyReason::MarkBusy)],
                        );
                    }
                });
            }
        });
        let lines = lines_of(&path);
        assert_eq!(lines.len(), 800);
        for line in lines {
            assert!(
                parse_record(&line).is_some(),
                "a line was torn by a concurrent append: {line}"
            );
        }
    }

    // ---------------------------------------------------------------------
    // §10.1 - the startup scan
    // ---------------------------------------------------------------------

    fn record_line(run_id: &str, event: &str, millis: i64) -> String {
        format!(
            "{}\n",
            json!({ "v": 1, "at": at_offset(millis), "runId": run_id, "event": event })
        )
    }

    fn seed_live(dir: &TempDir, content: &str) {
        std::fs::write(dir.path().join(FILE_NAME), content).expect("seed the live file");
    }

    fn seed_rotated(dir: &TempDir, content: &str) {
        std::fs::write(dir.path().join(format!("{FILE_NAME}.1")), content)
            .expect("seed the rotated file");
    }

    fn scan(dir: &TempDir) -> PreviousRunReport {
        reset_rotated_reads();
        scan_previous_run_at(dir.path())
    }

    #[test]
    fn scan_returns_clean_when_the_last_record_is_app_stop() {
        let dir = TempDir::new().expect("temp dir");
        seed_live(
            &dir,
            &format!(
                "{}{}",
                record_line("run-a", "app_start", 0),
                record_line("run-a", "app_stop", 5_000)
            ),
        );
        let report = scan(&dir);
        assert_eq!(report.scan, PreviousRunScan::Ok);
        let previous = report.previous_run.expect("a previous run");
        assert_eq!(previous.run_id, "run-a");
        assert!(previous.clean);
        assert!(previous.anchor_seen);
        assert_eq!(previous.last_event, "app_stop");
        assert_eq!(previous.last_record_at, at_offset(5_000));
    }

    #[test]
    fn scan_returns_clean_when_app_stop_is_followed_by_a_stray_busy() {
        let dir = TempDir::new().expect("temp dir");
        seed_live(
            &dir,
            &format!(
                "{}{}{}",
                record_line("run-a", "app_start", 0),
                record_line("run-a", "app_stop", 5_000),
                record_line("run-a", "busy", 5_200)
            ),
        );
        let previous = scan(&dir).previous_run.expect("a previous run");
        assert!(
            previous.clean,
            "an app_stop anywhere in the region means clean"
        );
        assert_eq!(previous.last_event, "busy");
    }

    #[test]
    fn scan_returns_clean_when_app_stop_is_followed_by_app_alive() {
        let dir = TempDir::new().expect("temp dir");
        seed_live(
            &dir,
            &format!(
                "{}{}{}",
                record_line("run-a", "app_start", 0),
                record_line("run-a", "app_stop", 5_000),
                record_line("run-a", "app_alive", 5_400)
            ),
        );
        let previous = scan(&dir).previous_run.expect("a previous run");
        assert!(previous.clean);
        assert_eq!(previous.last_event, "app_alive");
    }

    #[test]
    fn scan_returns_unclean_when_no_app_stop_appears_for_that_run() {
        let dir = TempDir::new().expect("temp dir");
        seed_live(
            &dir,
            &format!(
                "{}{}",
                record_line("run-a", "app_start", 0),
                record_line("run-a", "app_alive", 60_000)
            ),
        );
        let report = scan(&dir);
        assert_eq!(report.scan, PreviousRunScan::Ok);
        let previous = report.previous_run.expect("a previous run");
        assert!(!previous.clean);
        assert!(previous.anchor_seen, "the app_start is the anchor");
        assert_eq!(previous.last_record_at, at_offset(60_000));
    }

    #[test]
    fn scan_sets_anchor_seen_false_when_neither_file_holds_an_anchor() {
        let dir = TempDir::new().expect("temp dir");
        seed_live(&dir, &record_line("run-a", "app_alive", 60_000));
        seed_rotated(&dir, &record_line("run-a", "busy", 30_000));
        let report = scan(&dir);
        let previous = report.previous_run.expect("a previous run");
        assert!(!previous.anchor_seen);
        assert!(!previous.clean);
        assert_eq!(
            report.scan,
            PreviousRunScan::Ok,
            "the rotated file supplied no answer"
        );
    }

    #[test]
    fn scan_keeps_the_first_line_when_the_file_is_smaller_than_the_tail_window() {
        let dir = TempDir::new().expect("temp dir");
        // A single record: discarding the leading line unconditionally would
        // leave nothing to find.
        seed_live(&dir, &record_line("run-a", "app_stop", 0));
        let previous = scan(&dir).previous_run.expect("a previous run");
        assert_eq!(previous.run_id, "run-a");
        assert!(previous.clean);
    }

    #[test]
    fn scan_survives_a_multibyte_character_at_the_tail_boundary() {
        let dir = TempDir::new().expect("temp dir");
        let tail = format!(
            "{}{}",
            record_line("run-a", "app_start", 0),
            record_line("run-a", "app_stop", 1_000)
        );
        // Grow a padding line of multibyte characters until the tail window
        // starts inside one of them.
        let mut padding = String::new();
        let content = loop {
            let filler = format!("{{\"pad\":\"{}\"}}\n", "é".repeat(2_000));
            while padding.len() < TAIL_READ_BYTES as usize {
                padding.push_str(&filler);
            }
            let content = format!("{padding}{tail}");
            let offset = content.len() - TAIL_READ_BYTES as usize;
            if content.as_bytes()[offset] & 0xC0 == 0x80 {
                break content;
            }
            padding.push('x');
        };
        assert!(content.len() > TAIL_READ_BYTES as usize);
        seed_live(&dir, &content);

        let previous = scan(&dir).previous_run.expect("a previous run");
        assert_eq!(previous.run_id, "run-a");
        assert!(previous.clean, "a split codepoint must not break the scan");
    }

    #[test]
    fn scan_takes_the_last_parseable_record_not_the_last_line() {
        let dir = TempDir::new().expect("temp dir");
        seed_live(
            &dir,
            &format!(
                "{}{}{{\"v\":1,\"at\":\"2026-07-26T00:00:0",
                record_line("run-a", "app_start", 0),
                record_line("run-a", "app_alive", 2_000)
            ),
        );
        let previous = scan(&dir).previous_run.expect("a previous run");
        assert_eq!(previous.last_event, "app_alive");
        assert_eq!(previous.last_record_at, at_offset(2_000));
    }

    #[test]
    fn scan_falls_back_to_the_rotated_file_when_the_live_tail_has_no_parseable_record() {
        let dir = TempDir::new().expect("temp dir");
        seed_live(&dir, "{\"v\":1,\"at\":\"2026-07-26T00");
        seed_rotated(
            &dir,
            &format!(
                "{}{}",
                record_line("run-a", "app_start", 0),
                record_line("run-a", "app_alive", 9_000)
            ),
        );
        let report = scan(&dir);
        assert_eq!(report.scan, PreviousRunScan::RecoveredFromRotated);
        assert_eq!(rotated_reads(), 1);
        let previous = report.previous_run.expect("a previous run");
        assert_eq!(previous.last_event, "app_alive");
        assert!(!previous.clean);
        assert!(previous.anchor_seen);
    }

    #[test]
    fn scan_falls_back_to_the_rotated_file_when_the_live_file_is_empty() {
        let dir = TempDir::new().expect("temp dir");
        seed_live(&dir, "");
        seed_rotated(&dir, &record_line("run-a", "app_stop", 0));
        let report = scan(&dir);
        assert_eq!(report.scan, PreviousRunScan::RecoveredFromRotated);
        assert_eq!(rotated_reads(), 1);
        assert!(report.previous_run.expect("a previous run").clean);
    }

    #[test]
    fn scan_falls_back_to_rotated_when_the_live_region_has_records_but_no_anchor_for_that_run() {
        let dir = TempDir::new().expect("temp dir");
        seed_live(&dir, &record_line("run-a", "busy", 9_000));
        seed_rotated(&dir, &record_line("run-a", "app_start", 0));
        let report = scan(&dir);
        assert_eq!(rotated_reads(), 1, "the anchor clause must consult .1");
        assert_eq!(report.scan, PreviousRunScan::RecoveredFromRotated);
        assert!(report.previous_run.expect("a previous run").anchor_seen);
    }

    #[test]
    fn scan_reports_clean_when_app_stop_is_in_the_rotated_file_and_a_teardown_busy_is_in_the_live_file(
    ) {
        let dir = TempDir::new().expect("temp dir");
        seed_live(&dir, &record_line("run-a", "busy", 5_100));
        seed_rotated(
            &dir,
            &format!(
                "{}{}",
                record_line("run-a", "app_start", 0),
                record_line("run-a", "app_stop", 5_000)
            ),
        );
        let previous = scan(&dir).previous_run.expect("a previous run");
        assert!(
            previous.clean,
            "a rotation boundary must not turn a clean exit unclean"
        );
        assert_eq!(previous.last_event, "busy");
    }

    #[test]
    fn scan_does_not_read_the_rotated_file_when_the_live_region_holds_the_app_stop() {
        let dir = TempDir::new().expect("temp dir");
        seed_live(
            &dir,
            &format!(
                "{}{}{}",
                record_line("run-a", "app_start", 0),
                record_line("run-a", "app_stop", 5_000),
                record_line("run-a", "app_alive", 5_500)
            ),
        );
        seed_rotated(&dir, &record_line("run-a", "app_start", -60_000));
        let report = scan(&dir);
        assert_eq!(
            rotated_reads(),
            0,
            "the clean case must short-circuit before opening .1"
        );
        assert!(report.previous_run.expect("a previous run").clean);
    }

    #[test]
    fn scan_on_two_empty_files_reports_empty_and_null_previous_run() {
        let dir = TempDir::new().expect("temp dir");
        let report = scan(&dir);
        assert_eq!(report.scan, PreviousRunScan::Empty);
        assert_eq!(report.previous_run, None);

        seed_live(&dir, "");
        seed_rotated(&dir, "");
        let report = scan(&dir);
        assert_eq!(report.scan, PreviousRunScan::Empty);
        assert_eq!(report.previous_run, None);
    }

    // ---------------------------------------------------------------------
    // §10.1 - init_run_at
    // ---------------------------------------------------------------------

    fn app_start_of(dir: &TempDir) -> Value {
        parsed_lines(&dir.path().join(FILE_NAME))
            .into_iter()
            .find(|value| value["event"] == "app_start")
            .expect("an app_start record")
    }

    #[test]
    fn init_run_at_on_a_file_smaller_than_the_tail_window() {
        let dir = TempDir::new().expect("temp dir");
        seed_live(&dir, &record_line("run-a", "app_stop", 0));
        init_run_at(dir.path(), "run-b");

        let start = app_start_of(&dir);
        assert_eq!(start["runId"], "run-b");
        assert_eq!(start["previousRunScan"], "ok");
        assert_eq!(start["previousRun"]["runId"], "run-a");
        assert_eq!(start["previousRun"]["clean"], json!(true));
        assert_eq!(start["appVersion"], env!("CARGO_PKG_VERSION"));
        assert_eq!(start["pid"], json!(std::process::id()));
    }

    #[test]
    fn init_run_at_on_a_tail_split_mid_utf8_character() {
        let dir = TempDir::new().expect("temp dir");
        let mut content = String::new();
        let filler = format!("{{\"pad\":\"{}\"}}\n", "ü".repeat(2_000));
        while content.len() < TAIL_READ_BYTES as usize + 1_024 {
            content.push_str(&filler);
        }
        content.push_str(&record_line("run-a", "app_stop", 0));
        seed_live(&dir, &content);

        init_run_at(dir.path(), "run-b");
        let start = app_start_of(&dir);
        assert_eq!(start["previousRun"]["runId"], "run-a");
        assert_eq!(start["previousRun"]["clean"], json!(true));
    }

    #[test]
    fn init_run_at_rotates_when_the_live_file_is_exactly_at_the_cap() {
        let dir = TempDir::new().expect("temp dir");
        write_file_at_cap(&dir.path().join(FILE_NAME), "run-a");

        init_run_at(dir.path(), "run-b");

        assert!(dir.path().join(format!("{FILE_NAME}.1")).exists());
        let records = parsed_lines(&dir.path().join(FILE_NAME));
        assert_eq!(records.len(), 2);
        assert_eq!(records[0]["event"], "metrics");
        assert_eq!(records[1]["event"], "app_start");
        assert_eq!(
            records[1]["previousRun"]["runId"], "run-a",
            "the scan runs before the append, so the rotation cannot invalidate it"
        );
    }

    #[test]
    fn init_run_at_on_a_read_only_directory_does_not_panic() {
        // Windows ignores the read-only attribute on directories, so the
        // portable stand-ins for "this sink cannot be written" are a directory
        // that does not exist and a sink path that is itself a directory.
        let dir = TempDir::new().expect("temp dir");
        init_run_at(&dir.path().join("absent"), "run-b");
        assert!(!dir.path().join("absent").exists());

        std::fs::create_dir(dir.path().join(FILE_NAME)).expect("occupy the sink path");
        init_run_at(dir.path(), "run-c");
        assert!(dir.path().join(FILE_NAME).is_dir());
    }

    #[test]
    fn init_run_at_reads_daemon_pid_from_its_argument_directory_not_the_live_config_dir() {
        let dir = TempDir::new().expect("temp dir");
        // PID 4 is the Windows System process: always present, and reported
        // alive even when `OpenProcess` is denied. The developer's real
        // `daemon.pid` never holds it, so observing it proves the argument
        // directory was the one read.
        std::fs::write(dir.path().join("daemon.pid"), "4").expect("seed daemon.pid");
        init_run_at(dir.path(), "run-b");
        assert_eq!(app_start_of(&dir)["concurrentInstancePid"], json!(4));

        let own = TempDir::new().expect("temp dir");
        std::fs::write(
            own.path().join("daemon.pid"),
            std::process::id().to_string(),
        )
        .expect("seed daemon.pid");
        init_run_at(own.path(), "run-c");
        assert!(
            app_start_of(&own).get("concurrentInstancePid").is_none(),
            "our own PID is not a concurrent instance"
        );
    }

    #[test]
    fn app_start_always_declares_interval_count_unsupported() {
        let dir = TempDir::new().expect("temp dir");
        init_run_at(dir.path(), "run-a");
        let metrics = &app_start_of(&dir)["metrics"];
        assert_eq!(
            metrics["supported"],
            json!(["totalBusyMs", "totalBusyRawMs"])
        );
        assert_eq!(
            metrics["unsupported"],
            json!(["intervalCount", "blockCount"])
        );
    }

    // ---------------------------------------------------------------------
    // §10.3 - the reference reducer
    //
    // A test-only implementation of the consumer contract, which is what makes
    // "total busy time is derivable from the file alone" objectively
    // verifiable without shipping a consumer surface.
    //
    // It walks records in FILE order and uses `at` only for arithmetic. That
    // ordering is what makes an inverted pair from the spawn-order race
    // observable at all; sorting the stream by `at` would turn every such pair
    // into an orphan plus an unclosed interval and the clamp could never fire.
    // ---------------------------------------------------------------------

    #[derive(Debug, Default, PartialEq, Eq)]
    struct ReducerReport {
        total_busy_ms: i64,
        total_busy_raw_ms: i64,
        per_cwd_busy_ms: BTreeMap<String, i64>,
        unparseable_lines: usize,
        orphan_idles: usize,
        busy_after_app_stop: usize,
        discarded_unclosed: usize,
    }

    type IntervalKey = (String, String);

    fn field<'a>(value: &'a Value, name: &str) -> Option<&'a str> {
        value.get(name)?.as_str()
    }

    fn moment(value: &Value) -> Option<DateTime<Utc>> {
        DateTime::parse_from_rfc3339(field(value, "at")?)
            .ok()
            .map(|parsed| parsed.with_timezone(&Utc))
    }

    fn key_of(value: &Value) -> Option<IntervalKey> {
        Some((
            field(value, "runId")?.to_string(),
            field(value, "sessionId")?.to_string(),
        ))
    }

    fn reduce(stream: &str) -> ReducerReport {
        let mut report = ReducerReport::default();
        let mut records: Vec<Value> = Vec::new();
        for line in stream.lines() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Value>(line) {
                Ok(value)
                    if value.get("v").is_some()
                        && field(&value, "runId").is_some()
                        && field(&value, "event").is_some()
                        && moment(&value).is_some() =>
                {
                    records.push(value)
                }
                _ => report.unparseable_lines += 1,
            }
        }

        // Rule 3 and rule 4 candidates, collected first because both are stated
        // by records that can appear anywhere in the stream.
        let mut recovery: HashMap<String, DateTime<Utc>> = HashMap::new();
        let mut heartbeat: HashMap<IntervalKey, DateTime<Utc>> = HashMap::new();
        for value in &records {
            match field(value, "event").unwrap_or_default() {
                EVENT_APP_START => {
                    let Some(previous) = value.get("previousRun") else {
                        continue;
                    };
                    // `anchorSeen` is deliberately not consulted.
                    if previous.get("clean").and_then(Value::as_bool) != Some(false)
                        || value.get("concurrentInstancePid").is_some()
                    {
                        continue;
                    }
                    let Some(run) = field(previous, "runId") else {
                        continue;
                    };
                    let Some(last) = field(previous, "lastRecordAt")
                        .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
                    else {
                        continue;
                    };
                    recovery.insert(run.to_string(), last.with_timezone(&Utc));
                }
                "app_alive" => {
                    let (Some(run), Some(at)) = (field(value, "runId"), moment(value)) else {
                        continue;
                    };
                    let listed = value
                        .get("workingSessionIds")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    for id in listed.iter().filter_map(Value::as_str) {
                        heartbeat.insert((run.to_string(), id.to_string()), at);
                    }
                }
                _ => {}
            }
        }

        let mut open: HashMap<IntervalKey, DateTime<Utc>> = HashMap::new();
        let mut cwd_of: HashMap<IntervalKey, String> = HashMap::new();
        let mut stopped: HashMap<String, DateTime<Utc>> = HashMap::new();
        for value in &records {
            let Some(at) = moment(value) else { continue };
            let run = field(value, "runId").unwrap_or_default().to_string();
            match field(value, "event").unwrap_or_default() {
                "busy" => {
                    let Some(key) = key_of(value) else { continue };
                    if stopped.contains_key(&run) {
                        // No close candidate can be later than this open.
                        report.busy_after_app_stop += 1;
                        continue;
                    }
                    if let Some(cwd) = field(value, "cwd") {
                        cwd_of.insert(key.clone(), cwd.to_string());
                    }
                    open.entry(key).or_insert(at);
                }
                "idle" => {
                    let Some(key) = key_of(value) else { continue };
                    match open.remove(&key) {
                        Some(opened) => {
                            let threshold = value
                                .get("idleThresholdMs")
                                .and_then(Value::as_i64)
                                .unwrap_or_default();
                            close(&mut report, &cwd_of, &key, opened, at, threshold);
                        }
                        None => report.orphan_idles += 1,
                    }
                }
                EVENT_APP_STOP => {
                    stopped.insert(run.clone(), at);
                    let keys: Vec<IntervalKey> = open
                        .keys()
                        .filter(|(candidate, _)| *candidate == run)
                        .cloned()
                        .collect();
                    for key in keys {
                        if let Some(opened) = open.remove(&key) {
                            close(&mut report, &cwd_of, &key, opened, at, 0);
                        }
                    }
                }
                _ => {}
            }
        }

        let mut leftovers: Vec<(IntervalKey, DateTime<Utc>)> = open.into_iter().collect();
        leftovers.sort();
        for (key, opened) in leftovers {
            let candidate = recovery
                .get(&key.0)
                .copied()
                .or_else(|| heartbeat.get(&key).copied())
                .filter(|close_at| *close_at >= opened);
            match candidate {
                Some(close_at) => close(&mut report, &cwd_of, &key, opened, close_at, 0),
                // Discarded, not extended.
                None => report.discarded_unclosed += 1,
            }
        }
        report
    }

    fn close(
        report: &mut ReducerReport,
        cwd_of: &HashMap<IntervalKey, String>,
        key: &IntervalKey,
        opened: DateTime<Utc>,
        closed: DateTime<Utc>,
        threshold_ms: i64,
    ) {
        let raw = closed.signed_duration_since(opened).num_milliseconds();
        let corrected = (raw - threshold_ms).max(0);
        report.total_busy_raw_ms += raw;
        report.total_busy_ms += corrected;
        if let Some(cwd) = cwd_of.get(key) {
            *report.per_cwd_busy_ms.entry(cwd.clone()).or_default() += corrected;
        }
    }

    // Stream builders for the reducer tests.

    fn busy_line(
        run: &str,
        session: &str,
        millis: i64,
        continues: bool,
        gap: Option<u64>,
    ) -> String {
        let mut record = json!({
            "v": 1,
            "at": at_offset(millis),
            "runId": run,
            "event": "busy",
            "sessionId": session,
            "name": "wg-14-dev-team/dev-rust",
            "cwd": "C:\\replica",
            "reason": "mark_busy",
            "continuesBlock": continues,
        });
        if let Some(gap) = gap {
            record["gapMs"] = json!(gap);
        }
        format!("{record}\n")
    }

    fn idle_line(run: &str, session: &str, millis: i64, threshold: Option<u64>) -> String {
        let mut record = json!({
            "v": 1,
            "at": at_offset(millis),
            "runId": run,
            "event": "idle",
            "sessionId": session,
            "name": "wg-14-dev-team/dev-rust",
            "cwd": "C:\\replica",
            "reason": "mark_idle",
        });
        match threshold {
            Some(threshold) => record["idleThresholdMs"] = json!(threshold),
            None => record["reason"] = json!("app_stop"),
        }
        format!("{record}\n")
    }

    fn app_stop_line(run: &str, millis: i64, enumerated: bool, count: usize) -> String {
        format!(
            "{}\n",
            json!({
                "v": 1,
                "at": at_offset(millis),
                "runId": run,
                "event": "app_stop",
                "openSessionsEnumerated": enumerated,
                "openSessionCount": count,
            })
        )
    }

    fn app_alive_line(run: &str, millis: i64, ids: &[&str]) -> String {
        format!(
            "{}\n",
            json!({
                "v": 1,
                "at": at_offset(millis),
                "runId": run,
                "event": "app_alive",
                "workingSessionIds": ids,
            })
        )
    }

    fn app_start_line(
        run: &str,
        millis: i64,
        previous: Option<(&str, i64, bool, bool)>,
        concurrent: Option<u32>,
    ) -> String {
        let mut record = json!({
            "v": 1,
            "at": at_offset(millis),
            "runId": run,
            "event": "app_start",
            "pid": 1234,
            "appVersion": "0.20.0",
            "previousRunScan": "ok",
            "previousRun": Value::Null,
            "metrics": { "supported": ["totalBusyMs", "totalBusyRawMs"], "unsupported": ["intervalCount", "blockCount"] },
        });
        if let Some((previous_run, last_at, clean, anchor_seen)) = previous {
            record["previousRun"] = json!({
                "runId": previous_run,
                "lastRecordAt": at_offset(last_at),
                "lastEvent": "app_alive",
                "clean": clean,
                "anchorSeen": anchor_seen,
            });
        }
        if let Some(pid) = concurrent {
            record["concurrentInstancePid"] = json!(pid);
        }
        format!("{record}\n")
    }

    const SESSION_A: &str = "3f51d2df-0000-0000-0000-000000000001";
    const SESSION_B: &str = "8a5cc5f1-0000-0000-0000-000000000002";

    #[test]
    fn a_normal_busy_idle_pair_yields_its_span_minus_the_idle_threshold() {
        let stream = format!(
            "{}{}",
            busy_line("run-a", SESSION_A, 0, false, None),
            idle_line("run-a", SESSION_A, 10_000, Some(2_500))
        );
        let report = reduce(&stream);
        assert_eq!(report.total_busy_raw_ms, 10_000);
        assert_eq!(report.total_busy_ms, 7_500);
        assert_eq!(report.per_cwd_busy_ms.get("C:\\replica"), Some(&7_500));
        assert_eq!(report.orphan_idles, 0);
        assert_eq!(report.discarded_unclosed, 0);
    }

    #[test]
    fn raw_and_corrected_totals_differ_by_exactly_the_sum_of_subtracted_thresholds() {
        let stream = format!(
            "{}{}{}{}",
            busy_line("run-a", SESSION_A, 0, false, None),
            idle_line("run-a", SESSION_A, 10_000, Some(2_500)),
            busy_line("run-a", SESSION_A, 20_000, true, Some(10_000)),
            idle_line("run-a", SESSION_A, 45_000, Some(2_500))
        );
        let report = reduce(&stream);
        assert_eq!(report.total_busy_raw_ms, 35_000);
        assert_eq!(report.total_busy_ms, 35_000 - 5_000);
    }

    #[test]
    fn an_inverted_pair_from_the_reordering_race_clamps_to_zero() {
        // Only a synthetic stream reaches this: the edge guard means a starved
        // `mark_busy` task cannot produce a recorded pair at all.
        let stream = format!(
            "{}{}",
            busy_line("run-a", SESSION_A, 1_000, false, None),
            idle_line("run-a", SESSION_A, 800, Some(2_500))
        );
        let report = reduce(&stream);
        assert_eq!(report.total_busy_ms, 0, "the clamp holds the total at zero");
        assert_eq!(report.orphan_idles, 0, "the pair was paired, not orphaned");
        assert_eq!(report.discarded_unclosed, 0);
    }

    #[test]
    fn a_run_closed_by_app_stop_leaves_no_open_interval() {
        let stream = format!(
            "{}{}{}",
            busy_line("run-a", SESSION_A, 0, false, None),
            idle_line("run-a", SESSION_A, 4_000, None),
            app_stop_line("run-a", 4_000, true, 1)
        );
        let report = reduce(&stream);
        assert_eq!(report.discarded_unclosed, 0);
        assert_eq!(report.total_busy_raw_ms, 4_000);
        assert_eq!(
            report.total_busy_ms, 4_000,
            "a synthetic close subtracts nothing"
        );
    }

    #[test]
    fn app_stop_closes_intervals_even_when_open_sessions_enumerated_is_false() {
        let stream = format!(
            "{}{}{}",
            busy_line("run-a", SESSION_A, 0, false, None),
            busy_line("run-a", SESSION_B, 1_000, false, None),
            app_stop_line("run-a", 6_000, false, 0)
        );
        let report = reduce(&stream);
        assert_eq!(report.discarded_unclosed, 0);
        assert_eq!(report.total_busy_raw_ms, 6_000 + 5_000);
    }

    #[test]
    fn an_unclean_run_is_closed_at_previous_run_last_record_at() {
        let stream = format!(
            "{}{}",
            busy_line("run-a", SESSION_A, 0, false, None),
            app_start_line("run-b", 90_000, Some(("run-a", 30_000, false, true)), None)
        );
        let report = reduce(&stream);
        assert_eq!(report.total_busy_raw_ms, 30_000);
        assert_eq!(report.discarded_unclosed, 0);
    }

    #[test]
    fn an_unclean_run_with_a_concurrent_instance_pid_is_not_closed_by_recovery() {
        let stream = format!(
            "{}{}",
            busy_line("run-a", SESSION_A, 0, false, None),
            app_start_line(
                "run-b",
                90_000,
                Some(("run-a", 30_000, false, true)),
                Some(41_256)
            )
        );
        let report = reduce(&stream);
        assert_eq!(report.total_busy_raw_ms, 0);
        assert_eq!(report.discarded_unclosed, 1);
    }

    #[test]
    fn recovery_ignores_anchor_seen() {
        let with_anchor = format!(
            "{}{}",
            busy_line("run-a", SESSION_A, 0, false, None),
            app_start_line("run-b", 90_000, Some(("run-a", 30_000, false, true)), None)
        );
        let without_anchor = format!(
            "{}{}",
            busy_line("run-a", SESSION_A, 0, false, None),
            app_start_line("run-b", 90_000, Some(("run-a", 30_000, false, false)), None)
        );
        assert_eq!(reduce(&with_anchor), reduce(&without_anchor));
        assert_eq!(reduce(&without_anchor).total_busy_raw_ms, 30_000);
    }

    #[test]
    fn an_interval_with_no_closing_record_is_closed_at_the_last_heartbeat_that_listed_it() {
        let stream = format!(
            "{}{}{}{}",
            busy_line("run-a", SESSION_A, 0, false, None),
            app_alive_line("run-a", 60_000, &[SESSION_A]),
            app_alive_line("run-a", 120_000, &[SESSION_A]),
            app_alive_line("run-a", 180_000, &[SESSION_B])
        );
        let report = reduce(&stream);
        assert_eq!(
            report.total_busy_raw_ms, 120_000,
            "the last heartbeat that listed the session closes it"
        );
        assert_eq!(report.discarded_unclosed, 0);
    }

    #[test]
    fn an_interval_no_rule_can_close_is_discarded_rather_than_extended() {
        let stream = format!(
            "{}{}",
            busy_line("run-a", SESSION_A, 0, false, None),
            app_alive_line("run-a", 60_000, &[SESSION_B])
        );
        let report = reduce(&stream);
        assert_eq!(report.total_busy_raw_ms, 0);
        assert_eq!(report.total_busy_ms, 0);
        assert_eq!(report.discarded_unclosed, 1);
    }

    #[test]
    fn a_busy_after_app_stop_is_discarded_and_counted_not_clamped() {
        let stream = format!(
            "{}{}{}{}",
            busy_line("run-a", SESSION_A, 0, false, None),
            idle_line("run-a", SESSION_A, 4_000, None),
            app_stop_line("run-a", 4_000, true, 1),
            busy_line("run-a", SESSION_B, 4_200, false, None)
        );
        let report = reduce(&stream);
        assert_eq!(report.busy_after_app_stop, 1);
        assert_eq!(
            report.discarded_unclosed, 0,
            "it never became an open interval"
        );
        assert_eq!(report.total_busy_raw_ms, 4_000);
    }

    #[test]
    fn an_orphan_idle_is_discarded_and_counted_not_faulted() {
        let stream = idle_line("run-a", SESSION_A, 4_000, Some(2_500));
        let report = reduce(&stream);
        assert_eq!(report.orphan_idles, 1);
        assert_eq!(report.total_busy_raw_ms, 0);
        assert_eq!(report.total_busy_ms, 0);
    }

    #[test]
    fn unparseable_lines_are_skipped_and_counted() {
        let stream = format!(
            "not json at all\n{}{{\"v\":1,\"at\":\"2026-07-26T00:00:00.000Z\",\"runId\":\"run-a\"}}\n{}",
            busy_line("run-a", SESSION_A, 0, false, None),
            idle_line("run-a", SESSION_A, 3_000, Some(2_500))
        );
        let report = reduce(&stream);
        assert_eq!(
            report.unparseable_lines, 2,
            "one non-JSON line and one lacking `event`"
        );
        assert_eq!(report.total_busy_raw_ms, 3_000);
        assert_eq!(report.total_busy_ms, 500);
    }

    #[test]
    fn a_coalesced_block_reports_the_same_total_busy_ms_as_the_raw_pairs() {
        // Three pairs separated by sub-window gaps, so the second and third
        // opening edges are annotated as continuations.
        let annotated = format!(
            "{}{}{}{}{}{}",
            busy_line("run-a", SESSION_A, 0, false, None),
            idle_line("run-a", SESSION_A, 20_000, Some(2_500)),
            busy_line("run-a", SESSION_A, 28_000, true, Some(8_000)),
            idle_line("run-a", SESSION_A, 50_000, Some(2_500)),
            busy_line("run-a", SESSION_A, 55_000, true, Some(5_000)),
            idle_line("run-a", SESSION_A, 70_000, Some(2_500))
        );
        // The same edges with the annotation stripped to `false`.
        let unannotated = format!(
            "{}{}{}{}{}{}",
            busy_line("run-a", SESSION_A, 0, false, None),
            idle_line("run-a", SESSION_A, 20_000, Some(2_500)),
            busy_line("run-a", SESSION_A, 28_000, false, None),
            idle_line("run-a", SESSION_A, 50_000, Some(2_500)),
            busy_line("run-a", SESSION_A, 55_000, false, None),
            idle_line("run-a", SESSION_A, 70_000, Some(2_500))
        );
        let report = reduce(&annotated);
        assert_eq!(report.total_busy_raw_ms, 20_000 + 22_000 + 15_000);
        assert_eq!(report.total_busy_ms, 57_000 - 7_500);
        assert_eq!(
            report,
            reduce(&unannotated),
            "the block annotation must cost nothing in either total"
        );
    }

    #[test]
    fn records_from_two_interleaved_run_ids_are_reduced_independently() {
        // The same session id under two runs, interleaved, plus an app_stop for
        // one run that must not close the other run's interval.
        let stream = format!(
            "{}{}{}{}{}",
            busy_line("run-a", SESSION_A, 0, false, None),
            busy_line("run-b", SESSION_A, 1_000, false, None),
            idle_line("run-a", SESSION_A, 5_000, Some(2_500)),
            app_stop_line("run-a", 5_000, true, 0),
            idle_line("run-b", SESSION_A, 9_000, Some(2_500))
        );
        let report = reduce(&stream);
        assert_eq!(report.total_busy_raw_ms, 5_000 + 8_000);
        assert_eq!(report.total_busy_ms, 2_500 + 5_500);
        assert_eq!(report.orphan_idles, 0);
        assert_eq!(report.discarded_unclosed, 0);
    }

    #[test]
    fn a_session_start_busy_opens_an_interval_that_the_first_idle_closes() {
        let mut opening = busy_line("run-a", SESSION_A, 0, false, None);
        opening = opening.replace("\"mark_busy\"", "\"session_start\"");
        let stream = format!(
            "{opening}{}",
            idle_line("run-a", SESSION_A, 6_000, Some(2_500))
        );
        let report = reduce(&stream);
        assert!(stream.contains("\"reason\":\"session_start\""));
        assert_eq!(report.total_busy_raw_ms, 6_000);
        assert_eq!(report.total_busy_ms, 3_500);
        assert_eq!(report.orphan_idles, 0);
    }
}
