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
//! totalBusyMs    = sum over closed intervals of
//!                  max(0, (close - open) - idleThresholdMs_of_the_closing_record)
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
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::filesystem::activity_log as activity_io;

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
pub struct ActivityCoalescer {
    last_idle_at: OnceLock<Mutex<LastIdleAt>>,
}

#[allow(clippy::new_without_default)]
impl ActivityCoalescer {
    pub fn new() -> Self {
        Self {
            last_idle_at: OnceLock::new(),
        }
    }
}

fn writer_lock() -> &'static Mutex<()> {
    WRITER.get_or_init(|| Mutex::new(()))
}

fn coalescer(owner: &ActivityCoalescer) -> &Mutex<LastIdleAt> {
    owner
        .last_idle_at
        .get_or_init(|| Mutex::new(HashMap::new()))
}

/// Locks the coalescer, recovering from poison, and drops entries older than the
/// window on the way in so the map stays bounded even if pruning never had
/// another reason to run.
fn locked_coalescer(now: Instant, owner: &ActivityCoalescer) -> MutexGuard<'_, LastIdleAt> {
    let mut guard = coalescer(owner)
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
pub fn build_busy(
    id: Uuid,
    session: &Session,
    reason: BusyReason,
    owner: &ActivityCoalescer,
) -> ActivityRecord {
    let now_utc = Utc::now();
    let now_instant = Instant::now();
    let gap_ms = {
        let previous = locked_coalescer(now_instant, owner);
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
pub fn build_idle(
    id: Uuid,
    session: &Session,
    reason: IdleReason,
    owner: &ActivityCoalescer,
) -> ActivityRecord {
    let now_utc = Utc::now();
    let now_instant = Instant::now();
    locked_coalescer(now_instant, owner).insert(id, (now_instant, now_utc));
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
pub(crate) mod capture;

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
    activity_io::append_activity(path, buffer)
}

/// Shift `activity.jsonl.<i>` up one slot and move the live file into `.1`.
/// Returns whether the live file was actually rotated away.
///
/// Best-effort throughout: on failure the live file keeps growing past the cap
/// until a later rotation succeeds.
fn rotate_if_needed(path: &Path) -> bool {
    let size = match activity_io::activity_metadata(path) {
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
        if !activity_io::activity_exists(&from) {
            continue;
        }
        let to = parent.join(format!("{name}.{}", index + 1));
        if let Err(error) = activity_io::rename_activity(&from, &to) {
            log::warn!(
                "[activity] rotation could not shift {} to {} (continuing): {}",
                from.display(),
                to.display(),
                error
            );
        }
    }
    let first = parent.join(format!("{name}.1"));
    if let Err(error) = activity_io::rename_activity(path, &first) {
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
    let mut file = match activity_io::open_activity_tail(path) {
        Ok(file) => file,
        // A missing file recorded nothing; that is empty, not broken.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return RegionRead::Empty,
        Err(_) => return RegionRead::Unreadable,
    };
    let len = match activity_io::opened_activity_metadata(&file) {
        Ok(metadata) => metadata.len(),
        Err(_) => return RegionRead::Unreadable,
    };
    if len == 0 {
        return RegionRead::Empty;
    }
    // Clamped: a bare `End(-TAIL_READ_BYTES)` on a short file is an error on
    // Windows.
    let window = len.min(TAIL_READ_BYTES);
    if activity_io::seek_activity_tail(&mut file, SeekFrom::End(-(window as i64))).is_err() {
        return RegionRead::Unreadable;
    }
    let mut bytes = Vec::with_capacity(window as usize);
    if activity_io::read_activity_tail(&mut file, &mut bytes).is_err() {
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
mod tests;
