//! #1652 Backend IPC observer: the process-side view of the frontend -> backend
//! IPC stream, plus the intake for the renderer black boxes phase 2 writes.
//!
//! Every `invoke()` that reaches the app handler is stamped by `note_invoke`,
//! which is the only place in the process that sees that direction as a STREAM
//! rather than as one command. A 1 Hz loop watches the stream: once it has been
//! silent for `SILENCE_THRESHOLD` the loop declares an episode, writes a
//! `[ipc-observer] SILENCE` block into `app.log` from a process that is provably
//! still alive, drops a durable marker file next to the config, and starts
//! emitting `ipc_silence_probe` events at the renderer. The probe is the
//! falsifiable half: a renderer that records one was alive and receiving during
//! the episode, and a renderer that kept writing its black box and recorded none
//! proves the channel was dead in both directions.
//!
//! The marker is process-global and a black-box record is one window's, so every
//! arm of `classify` that reads the marker as a statement about a particular
//! window is guarded by evidence that window produced. `epic.md` decision 14
//! carries that rule and its four consequences.
//!
//! Phase 1 ships the writer, the marker and the intake. With phase 1 alone
//! `ipc_blackbox_report` is never called, so the marker is written and never read
//! back; the intake, the verdict and the marker's deletion all first run when the
//! renderer black box (phase 2) lands.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{Local, TimeZone};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

use crate::config::config_dir;
use crate::shutdown::ShutdownSignal;

/// Loop cadence. One tick is the resolution of every bound below.
const TICK_INTERVAL: Duration = Duration::from_secs(1);

/// How long the invoke stream must be silent before an episode is declared.
///
/// 90 s sits ABOVE the ~60 s throttled worst case that `REPORT_STALENESS_CEILING`
/// (180 s, `non_stop_watchdog.rs:42`) is itself sized against, and BELOW that
/// 180 s disarm, so the silence block always lands in `app.log` before the
/// `[non-stop] ... frontend gone` line it explains.
const SILENCE_THRESHOLD: u64 = 90_000;

/// Spacing between `ipc_silence_probe` emissions inside one episode.
const PROBE_INTERVAL: u64 = 10_000;

/// Probe budget for one episode. Past this the loop stays quiet.
const MAX_PROBES_PER_EPISODE: u32 = 30;

/// How many distinct commands the `recent:` line of the silence block names.
const COMMANDS_LOGGED: usize = 12;

/// Ceiling on the per-command table, so a pathological command namespace cannot
/// grow the observer without bound.
const MAX_TRACKED_COMMANDS: usize = 256;

/// Ceiling on the records one `ipc_blackbox_report` call may carry.
const MAX_RECORDS_PER_REPORT: usize = 16;

/// Ceiling on one record's serialized size.
const MAX_RECORD_BYTES: usize = 64 * 1024;

/// The age of the last animation frame on a VISIBLE window past which the
/// renderer's paint loop is called stalled. Two throttled tick periods, so a
/// backgrounded-then-restored window cannot forge a frame stall. Used ONLY in
/// the no-episode branch.
const RAF_STALE_MS: u64 = 120_000;

/// The ceiling BELOW which the renderer's task loop is proved to have stopped
/// before the silence onset, and it is used only in that direction:
/// `outlived_ms < LOOP_OUTLIVED_MS` proves the loop was already dead before the
/// first probe was emitted. The forward reading ("`>=` proves liveness") is its
/// converse and is NOT what this bound licenses.
const LOOP_OUTLIVED_MS: u64 = 30_000;

/// The durable marker, written next to the config so it survives a force-kill.
const MARKER_FILE: &str = "ipc-freeze-marker.json";

/// Longest silence-block line before the `recent:` list is wrapped.
const MAX_LOG_LINE: usize = 512;

/// Fixed text repeated on every record block. It is what stops a record with an
/// empty `pending` list from reading as exonerating when the hung call was a
/// `plugin:window|*` one; see `epic.md` decision 2.
const COVERAGE_LINE: &str = "[ipc-blackbox]   coverage: app commands only. Tauri plugin IPC (plugin:window|*, plugin:webview|*, plugin:dialog|open, plugin:event|*) enters neither registry, so an empty pending list does not exonerate a hung window call.";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// The process-global record of one silence episode.
///
/// `Clone` is load-bearing, not decoration: `snapshot_for_tick` returns this by
/// value, which is what lets the tick own its copy and leave the critical
/// section before any I/O.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FreezeMarker {
    pub backend_started_at_ms: u64,
    pub silence_started_at_ms: u64,
    pub last_invoke_at_ms: u64,
    pub last_invoke_cmd: String,
    pub probes_emitted: u32,
    pub silence_ended_at_ms: Option<u64>,
    /// `"traffic"` or `"shutdown"`. `classify` keys on this and NOT on
    /// `silence_ended_at_ms`, which both writers stamp.
    pub ended_by: Option<String>,
}

/// One entry of the renderer's `localStorage` black box, as handed over by
/// `ipc_blackbox_report`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredRecord {
    pub key: String,
    pub json: String,
}

/// One in-flight invoke as the renderer recorded it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PendingEntry {
    pub id: u64,
    pub cmd: String,
    pub age_ms: u64,
    pub overdue: bool,
}

/// One window's black box. Deliberately NO `deny_unknown_fields`: a record from
/// an older or newer renderer must still parse.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BlackBoxRecord {
    /// Recorded and logged, never branched on in this phase.
    pub v: u32,
    pub label: String,
    pub window_type: String,
    pub started_at_ms: u64,
    pub written_at_ms: u64,
    pub tick_seq: u64,
    pub raf_seq: u64,
    pub last_raf_at_ms: u64,
    pub visible: bool,
    /// Stamped `true` from the window's `pagehide` handler. It means exactly one
    /// thing: this window's JS task loop was alive at teardown and the window was
    /// closed rather than killed. Defaults to `false`, which is what a
    /// force-killed window carries and what an older renderer's record parses as.
    pub closed_cleanly: bool,
    pub last_pointer_at_ms: u64,
    pub last_event_at_ms: u64,
    pub last_event_name: String,
    pub probe_seq: Option<u64>,
    pub probe_at_ms: Option<u64>,
    pub sent: u64,
    pub settled: u64,
    pub last_settled_at_ms: u64,
    /// Stamped in `noteInvokeStart` BEFORE the call is handed to the transport,
    /// so it is set whether the send path then hangs or throws. It means exactly
    /// one thing: this window handed a call to the transport at that instant.
    /// Defaults to 0, so a record from an older renderer, or from a window that
    /// never invoked anything, can never reach the `b/send-path-broken` arm.
    pub last_sent_at_ms: u64,
    pub pending_total: u32,
    /// The UNCAPPED count of overdue in-flight calls. `pending` is capped at
    /// phase 2's `MAX_PENDING_RECORDED` (32), so this is not recoverable from the
    /// `pending` array once the cap bites. `classify` does not read it; it stays
    /// on the record and on the log block because that uncapped figure is what a
    /// reader needs to tell a window holding two stuck calls from one holding
    /// forty. Do not drop it as unused.
    pub overdue_total: u32,
    pub pending: Vec<PendingEntry>,
    /// `[sent, settled]` per command name.
    pub per_command: HashMap<String, [u64; 2]>,
}

/// What the intake returns to `ingest_records`, and what the tests assert on.
#[derive(Debug, Default)]
pub struct IngestOutcome {
    pub delete_keys: Vec<String>,
    pub lines: Vec<String>,
    pub marker_reported: bool,
}

// ---------------------------------------------------------------------------
// Verdict and classify
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Healthy,
    FrameStall,
    TaskLoopStopped,
    SendPathBroken,
    IpcDeadBothWays,
    Bystander,
    Inconclusive,
}

impl Verdict {
    /// Short tag, as it appears after `VERDICT` and in the `SUMMARY` line.
    pub fn tag(self) -> &'static str {
        match self {
            Verdict::Healthy => "healthy",
            Verdict::FrameStall => "d/frame-stall",
            Verdict::TaskLoopStopped => "a/task-loop-stopped",
            Verdict::SendPathBroken => "b/send-path-broken",
            Verdict::IpcDeadBothWays => "c/ipc-dead-both-ways",
            Verdict::Bystander => "bystander",
            Verdict::Inconclusive => "inconclusive",
        }
    }

    /// Fixed one-sentence explanation. The explanations carry NO runtime value;
    /// the measured figures go on the record block's `timing:` line instead,
    /// which is what keeps the logged wording testable by string equality.
    pub fn explanation(self) -> &'static str {
        match self {
            Verdict::Healthy => {
                "this window's black box shows nothing this instrument can call a freeze"
            }
            Verdict::FrameStall => {
                "the window was visible and painted no animation frame for at least RAF_STALE_MS (120000), so the renderer's paint loop stalled while the IPC channel kept working"
            }
            Verdict::TaskLoopStopped => {
                "the renderer stopped writing its black box within LOOP_OUTLIVED_MS (30000) of the last invoke the backend saw, so its JS task loop was already dead before the silence onset and it never had a probe to miss"
            }
            Verdict::SendPathBroken => {
                "this window received a silence probe and went on handing calls to the transport during the episode, yet the backend saw none of them arrive and nothing this window sent came back, so the frontend -> backend send path is broken"
            }
            Verdict::IpcDeadBothWays => {
                "the renderer went on writing its black box at or after the silence onset and recorded no probe, so neither direction of the IPC channel was working"
            }
            Verdict::Bystander => {
                "this window received a silence probe, so its JS task loop and the backend -> renderer direction were both alive during the episode, and it issued no invoke at or after the silence onset, so its own send path was never exercised: the episode is not this window's to explain"
            }
            Verdict::Inconclusive => {
                "the renderer's last record predates the silence onset, so it may or may not have been running when the first probe went out; it outlived the last invoke by at least LOOP_OUTLIVED_MS (30000), so it cannot be called a/task-loop-stopped either"
            }
        }
    }
}

/// The no-episode reading, shared by "no marker at all" and "the silence ended
/// by resumed traffic". It uses nothing from the marker, which is the point.
fn no_episode(record: &BlackBoxRecord) -> Verdict {
    let raf_floor = record.last_raf_at_ms.max(record.started_at_ms);
    if record.visible && record.written_at_ms.saturating_sub(raf_floor) >= RAF_STALE_MS {
        Verdict::FrameStall
    } else {
        Verdict::Healthy
    }
}

pub fn classify(record: &BlackBoxRecord, marker: Option<&FreezeMarker>) -> Verdict {
    // let-else on in-tree precedent alone, as used throughout `src-tauri/src/` (e.g.
    // `agent_update.rs:279,585`). NOT a lint requirement: `clippy::manual_let_else` is
    // pedantic, this repo enables no pedantic group, and an equivalent `match` with
    // `None => return no_episode(record)` is green under `cargo clippy -- -D warnings`.
    let Some(m) = marker else {
        return no_episode(record);
    };

    // An episode that ended by RESUMED TRAFFIC is evidence that the PROCESS-WIDE
    // silence ended. `ended_by == Some("shutdown")` is the opposite fact - the
    // silence NEVER ended and the process was told to quit while still silent -
    // so it must still be classified. Do NOT key this on `silence_ended_at_ms`:
    // that is set by both, and it lets the way the user terminated the app decide
    // the verdict. The marker is rewritten and never deleted on close, so a
    // traffic-closed marker outlives its episode on disk.
    let recovered = m.ended_by.as_deref() == Some("traffic");
    // A probe stamped at or after the onset proves the JS loop ran, received a
    // backend event and persisted it DURING the episode. Probes exist only from
    // the onset onwards and step 3 rebuilds the marker on every open, so this can
    // be satisfied by exactly one thing: a probe from THIS episode.
    let probed = record
        .probe_at_ms
        .is_some_and(|p| p >= m.silence_started_at_ms);
    // Send-side evidence, and the reason the 23rd field exists. Stamped in
    // `noteInvokeStart` BEFORE the call reaches the transport, so it is true for a
    // send path that hangs and for one that throws alike. Defaults to 0.
    let sent_in_episode = record.last_sent_at_ms >= m.silence_started_at_ms;

    // (b) ALWAYS requires send-side evidence. A probe alone proves that RECEIVE
    // worked and the loop was alive; it says NOTHING about the send path, and the
    // probe is emitted UNSCOPED to every window. Only a window that was trying to
    // send while the backend saw nothing can be said to have lost the send path.
    //
    // The extra condition on a recovered episode: `last_sent_at_ms` is monotone,
    // so on an episode that ended it lands after the onset for EVERY window that
    // outlived it - including the window whose successful send is what ENDED the
    // silence. There the send test cannot localise to the episode, so the arm
    // needs a corroborator that the EPISODE constrains - not merely one that
    // happens to be true at the end of the run.
    //
    // `overdue_total` is NOT that, and neither is `overdue_total > 0` paired with
    // `stopped_settling`. The count is a SNAPSHOT recomputed by each tick from the
    // live pending map and harvested at the record's LAST WRITE, so on a recovered
    // episode it answers "did this window hold a non-allowlisted call at least 5 s
    // old when the RUN ended". Pairing it with `stopped_settling` does not repair
    // that: `sent_in_episode` is monotone and unbounded above, so BOTH halves are
    // satisfied by a window that was untouched across the whole episode and then
    // made ONE slow call hours after it closed - a lid-close, a wake, and a
    // `kill_group` or `check_workgroup_repos_dirty` at 15:22 (neither is in
    // NEVER_OVERDUE, neither is timed). Nothing in that pair is bounded by the
    // episode's END.
    //
    // The corroborator has to be a quantity the EPISODE bounds. `pending` is
    // emitted OLDEST-ID FIRST and the cap keeps the oldest (phase 2), so the first
    // entry with `overdue` true is the OLDEST non-allowlisted call still
    // outstanding at the last write, and `written_at_ms - age_ms` is the instant it
    // was issued. Requiring that instant to be at or before `silence_ended_at_ms`
    // says: THIS WINDOW STILL HOLDS A CALL THAT WAS ALREADY IN FLIGHT WHEN THE
    // SILENCE ENDED. That is a snapshot the episode constrains - the second clause
    // of decision 14's rule - and it IMPLIES `overdue_total > 0`, which is why the
    // count is no longer a conjunct of its own. The bound is the END and not the
    // onset because the frozen window need not be the one that went silent first:
    // its first stuck call can post-date the onset by seconds. A record with no
    // recorded overdue entry - it holds none, or its oldest 32 are all
    // NEVER_OVERDUE - reads as NOT corroborated, the false-negative direction.
    //
    // `last_settled_at_ms < silence_started_at_ms` is the arm's other claim:
    // NOTHING this window sent has come back since the onset. It is a monotone
    // stamp read in the NEGATIVE direction, which is why no window that kept
    // working can satisfy it. Require BOTH. `epic.md` decision 14, consequence 4.
    let stopped_settling = record.last_settled_at_ms < m.silence_started_at_ms;
    // `silence_ended_at_ms` is `Some` whenever `ended_by` is, because step 5 stamps
    // the two together; `_ => false` is totality, not a case the writer produces.
    // `checked_sub`, NOT `saturating_sub`: saturation floors the issue instant to 0 and
    // `0 <= ended` holds, so it would fail TOWARD corroboration, while `None` fails away
    // from it. Neither is reachable from a well-formed record - it needs
    // `age_ms > written_at_ms`, and a backwards wall-clock step makes `ageMs` negative,
    // which fails `u64` deserialization at intake instead - so this removes an unsafe
    // direction rather than closing a defect. Build the wrapped shape verbatim: it is
    // what rustfmt emits at the default `max_width` of 100, the one-line form being 105
    // columns. Measured, not assumed; `-D warnings` is clean on it.
    let oldest_overdue = record.pending.iter().find(|e| e.overdue);
    let in_flight_since_episode = match (oldest_overdue, m.silence_ended_at_ms) {
        (Some(p), Some(ended)) => record
            .written_at_ms
            .checked_sub(p.age_ms)
            .is_some_and(|t| t <= ended),
        _ => false,
    };
    let corroborated = in_flight_since_episode && stopped_settling;
    if probed && sent_in_episode && (!recovered || corroborated) {
        return Verdict::SendPathBroken;
    }
    if recovered {
        // Nothing else a resumed-traffic episode carries is per-window evidence: a
        // record written long after it trivially clears the (c) boundary, and by
        // saturation forges (a). Evaluate as if no episode had happened.
        return no_episode(record);
    }
    if probed {
        // Received a probe and issued no invoke at or after the onset: alive,
        // receiving, and never obliged to send, so the episode is not this
        // window's to explain. Reachable on an OPEN episode only - the recovered
        // arm above returns first, so a probe-without-send on a recovered episode
        // reports `healthy`, the same scope statement at lower resolution.
        // Returned HERE, above (c): a recorded probe falsifies "recorded
        // none" for this window, so it must not fall into the (c) arm.
        return Verdict::Bystander;
    }
    // (c) is bounded by the silence ONSET, not by a constant: probes exist
    // only from the onset onwards, so only a record still being written at or
    // after it can be said to have missed one.
    if record.written_at_ms >= m.silence_started_at_ms {
        return Verdict::IpcDeadBothWays;
    }
    // `pagehide` ran, so this window's loop was ALIVE at teardown: TaskLoopStopped
    // is false for it by construction. Without this the arm below is reached by
    // SATURATION - a window closed cleanly at 10:00 has `written_at_ms` hours
    // BEFORE an 18:00 `last_invoke_at_ms`, so `outlived_ms` saturates to 0 and
    // forges (a). Placed AFTER the probe and (c) arms on purpose: a window that
    // really froze and was then closed still reports (b) or (c) on its own evidence.
    if record.closed_cleanly {
        return Verdict::Healthy;
    }
    let outlived_ms = record.written_at_ms.saturating_sub(m.last_invoke_at_ms);
    if outlived_ms < LOOP_OUTLIVED_MS {
        Verdict::TaskLoopStopped // proved dead before the onset
    } else {
        Verdict::Inconclusive // may or may not have survived to probe 1
    }
}

// ---------------------------------------------------------------------------
// Observer state
// ---------------------------------------------------------------------------

struct CommandStat {
    count: u64,
    last_at_ms: u64,
}

#[derive(Default)]
struct Inner {
    last_invoke_cmd: String,
    per_command: HashMap<String, CommandStat>,
    episode: Option<FreezeMarker>,
    next_probe_at_ms: u64,
}

/// Everything the tick's DECISION half needs, all owned, taken in one
/// acquisition so no formatting or I/O happens under the lock.
struct TickSnapshot {
    open_episode: Option<FreezeMarker>,
    last_invoke_cmd: String,
    /// `(name, count, last_at_ms)`, most recent first, `COMMANDS_LOGGED` long.
    recent: Vec<(String, u64, u64)>,
    tracked: usize,
    probe_due: bool,
}

/// An already-decided mutation, applied by the tick's MUTATION half.
enum TickChange {
    Open {
        marker: FreezeMarker,
        next_probe_at_ms: u64,
    },
    Probe {
        marker: FreezeMarker,
        next_probe_at_ms: u64,
    },
    Close,
    Shutdown,
}

pub struct IpcObserver {
    backend_started_at_ms: u64,
    total_invokes: AtomicU64,
    last_invoke_at_ms: AtomicU64,
    inner: Mutex<Inner>,
}

impl IpcObserver {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            backend_started_at_ms: now_ms(),
            total_invokes: AtomicU64::new(0),
            last_invoke_at_ms: AtomicU64::new(0),
            inner: Mutex::new(Inner::default()),
        })
    }

    /// The invoke hot path. Runs on the Tauri event-loop thread for EVERY
    /// invoke including `pty_write`, so it must not allocate once a command is
    /// known, must do no I/O and must not log. A poisoned lock is skipped,
    /// never unwrapped.
    pub fn note_invoke(&self, cmd: &str) {
        let at = now_ms();
        self.total_invokes.fetch_add(1, Ordering::Relaxed);
        self.last_invoke_at_ms.store(at, Ordering::Relaxed);
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        if let Some(stat) = inner.per_command.get_mut(cmd) {
            stat.count += 1;
            stat.last_at_ms = at;
        } else if inner.per_command.len() < MAX_TRACKED_COMMANDS {
            inner.per_command.insert(
                cmd.to_string(),
                CommandStat {
                    count: 1,
                    last_at_ms: at,
                },
            );
        }
        if inner.last_invoke_cmd != cmd {
            inner.last_invoke_cmd.clear();
            inner.last_invoke_cmd.push_str(cmd);
        }
    }

    /// The ONLY locking method the tick's decision half calls.
    fn snapshot_for_tick(&self, now: u64) -> TickSnapshot {
        let Ok(inner) = self.inner.lock() else {
            return TickSnapshot {
                open_episode: None,
                last_invoke_cmd: String::new(),
                recent: Vec::new(),
                tracked: 0,
                probe_due: false,
            };
        };
        let mut recent: Vec<(String, u64, u64)> = inner
            .per_command
            .iter()
            .map(|(name, stat)| (name.clone(), stat.count, stat.last_at_ms))
            .collect();
        recent.sort_by_key(|(_, _, at)| std::cmp::Reverse(*at));
        recent.truncate(COMMANDS_LOGGED);
        TickSnapshot {
            open_episode: inner.episode.clone(),
            last_invoke_cmd: inner.last_invoke_cmd.clone(),
            recent,
            tracked: inner.per_command.len(),
            probe_due: now >= inner.next_probe_at_ms,
        }
    }

    /// The ONLY locking method the tick's mutation half calls. Takes an
    /// already-decided change and mutates in-memory state only.
    ///
    /// Accepted residual: this and `snapshot_for_tick` are two separate
    /// acquisitions with the tick's I/O between them, so an invoke arriving in
    /// that gap can open an episode that step 5 then closes by traffic on the
    /// next tick. The cost is one spurious `SILENCE` block and a traffic-closed
    /// marker. That is the correct price for never holding a lock across I/O in
    /// a diagnostic built for a UI freeze; closing it would mean doing the thing
    /// this discipline forbids.
    fn apply_tick(&self, change: TickChange) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        match change {
            TickChange::Open {
                marker,
                next_probe_at_ms,
            }
            | TickChange::Probe {
                marker,
                next_probe_at_ms,
            } => {
                inner.episode = Some(marker);
                inner.next_probe_at_ms = next_probe_at_ms;
            }
            TickChange::Close | TickChange::Shutdown => {
                inner.episode = None;
            }
        }
    }

    /// `(total, last_at_ms, cmd)`. Exists for intake; the tick never calls it.
    pub fn last_invoke_snapshot(&self) -> (u64, u64, String) {
        let total = self.total_invokes.load(Ordering::Relaxed);
        let at = self.last_invoke_at_ms.load(Ordering::Relaxed);
        let Ok(inner) = self.inner.lock() else {
            return (total, at, String::new());
        };
        (total, at, inner.last_invoke_cmd.clone())
    }

    /// Harvest the previous run's renderer black boxes into `app.log` and return
    /// the `localStorage` keys the caller must delete.
    pub fn ingest_records(&self, records: Vec<StoredRecord>) -> Vec<String> {
        let marker = read_marker();
        let outcome = self.ingest_with_marker(records, marker.as_ref());
        for line in &outcome.lines {
            log::warn!("{}", line);
        }
        if outcome.marker_reported {
            delete_marker();
        }
        outcome.delete_keys
    }

    /// The pure half `ingest_records` calls after reading the marker file, so
    /// tests never touch `config_dir()`. It RETURNS the lines rather than
    /// logging them, which is what makes the wording testable.
    pub fn ingest_with_marker(
        &self,
        records: Vec<StoredRecord>,
        marker: Option<&FreezeMarker>,
    ) -> IngestOutcome {
        let mut out = IngestOutcome::default();
        // A marker written by THIS run is never reported and never deleted.
        let marker_is_previous_run =
            marker.is_some_and(|m| m.backend_started_at_ms < self.backend_started_at_ms);

        let total = records.len();
        let mut verdicts: Vec<(String, &'static str)> = Vec::new();
        for stored in records.into_iter().take(MAX_RECORDS_PER_REPORT) {
            if stored.json.len() > MAX_RECORD_BYTES {
                out.lines.push(format!(
                    "[ipc-blackbox] dropped oversized record key={} bytes={}",
                    stored.key,
                    stored.json.len()
                ));
                out.delete_keys.push(stored.key);
                continue;
            }
            let record: BlackBoxRecord = match serde_json::from_str(&stored.json) {
                Ok(record) => record,
                Err(err) => {
                    let head: String = stored.json.chars().take(200).collect();
                    out.lines.push(format!(
                        "[ipc-blackbox] unparseable record key={} err={} head={}",
                        stored.key, err, head
                    ));
                    out.delete_keys.push(stored.key);
                    continue;
                }
            };
            if record.started_at_ms >= self.backend_started_at_ms {
                // Current run: a sibling window is still writing this one.
                continue;
            }
            let carried = marker.filter(|m| {
                marker_is_previous_run && record.started_at_ms >= m.backend_started_at_ms
            });
            let verdict = classify(&record, carried);
            out.lines.extend(record_block(&record, carried, verdict));
            if carried.is_some() {
                out.marker_reported = true;
            }
            verdicts.push((record.label.clone(), verdict.tag()));
            out.delete_keys.push(stored.key);
        }
        if total > MAX_RECORDS_PER_REPORT {
            out.lines.push(format!(
                "[ipc-blackbox] dropped {} record(s) past MAX_RECORDS_PER_REPORT={}",
                total - MAX_RECORDS_PER_REPORT,
                MAX_RECORDS_PER_REPORT
            ));
        }

        // Marker-only branch: without it a run whose `localStorage` did not
        // survive produces ZERO of a, b, c, d after the restart.
        match marker {
            Some(m) if !out.marker_reported && marker_is_previous_run => {
                out.lines.extend(marker_only_block(m));
                out.marker_reported = true;
            }
            _ => {}
        }

        if verdicts.len() >= 2 {
            out.lines.push(summary_line(&verdicts));
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Block builders (pure)
// ---------------------------------------------------------------------------

fn record_block(
    record: &BlackBoxRecord,
    marker: Option<&FreezeMarker>,
    verdict: Verdict,
) -> Vec<String> {
    let mut out = Vec::new();
    out.push(format!(
        "[ipc-blackbox] previous-run window label={} type={} v={} visible={} closed_cleanly={}",
        record.label, record.window_type, record.v, record.visible, record.closed_cleanly
    ));
    out.push(format!(
        "[ipc-blackbox]   started={} last_tick={} ticks={} raf={} last_raf={}",
        stamp(record.started_at_ms),
        stamp(record.written_at_ms),
        record.tick_seq,
        record.raf_seq,
        stamp(record.last_raf_at_ms)
    ));
    let probe = match record.probe_at_ms {
        Some(at) => match record.probe_seq {
            Some(seq) => format!("{}@{}", seq, stamp(at)),
            None => format!("?@{}", stamp(at)),
        },
        None => "none".to_string(),
    };
    out.push(format!(
        "[ipc-blackbox]   last_pointer={} last_event={} ({}) probe={}",
        stamp(record.last_pointer_at_ms),
        stamp(record.last_event_at_ms),
        record.last_event_name,
        probe
    ));
    let last_sent = if record.last_sent_at_ms == 0 {
        "never".to_string()
    } else {
        stamp(record.last_sent_at_ms)
    };
    out.push(format!(
        "[ipc-blackbox]   invokes sent={} settled={} last_settled={} last_sent={} pending={} overdue={}",
        record.sent,
        record.settled,
        stamp(record.last_settled_at_ms),
        last_sent,
        record.pending_total,
        record.overdue_total
    ));
    // One line per entry IN ARRAY ORDER, up to phase 2's MAX_PENDING_RECORDED
    // (32). Not a formatting preference: the (b) arm's corroborator is read off
    // the OLDEST `overdue=true` entry, and the array's first entries can be
    // `overdue=false` dialogs, so an emitter that printed only the first entry,
    // or that reordered them, would put the verdict beyond hand-derivation.
    for entry in record.pending.iter().take(32) {
        out.push(format!(
            "[ipc-blackbox]   pending id={} cmd={} age_ms={} overdue={}",
            entry.id, entry.cmd, entry.age_ms, entry.overdue
        ));
    }
    let raf_age_ms = record
        .written_at_ms
        .saturating_sub(record.last_raf_at_ms.max(record.started_at_ms));
    match marker {
        Some(m) => {
            out.push(format!(
                "[ipc-blackbox]   marker: last_invoke={}@{} silence_declared={} probes={} ended={}",
                m.last_invoke_cmd,
                stamp(m.last_invoke_at_ms),
                stamp(m.silence_started_at_ms),
                m.probes_emitted,
                ended_text(m)
            ));
            let outlived_ms = record.written_at_ms.saturating_sub(m.last_invoke_at_ms);
            let onset_offset = m.silence_started_at_ms.saturating_sub(m.last_invoke_at_ms);
            out.push(format!(
                "[ipc-blackbox]   timing: outlived_ms={} (bounds: a<{}, c>=onset at {}ms); raf_age_ms={} visible={}",
                outlived_ms, LOOP_OUTLIVED_MS, onset_offset, raf_age_ms, record.visible
            ));
        }
        None => {
            out.push(format!(
                "[ipc-blackbox]   timing: no episode; raf_age_ms={} visible={}",
                raf_age_ms, record.visible
            ));
        }
    }
    out.push(COVERAGE_LINE.to_string());
    out.push(format!(
        "[ipc-blackbox] VERDICT {}: {}",
        verdict.tag(),
        verdict.explanation()
    ));
    out
}

fn marker_only_block(marker: &FreezeMarker) -> Vec<String> {
    vec![
        format!(
            "[ipc-blackbox] marker only: silence declared {}; last was {} at {}; probes={}; ended={}",
            stamp(marker.silence_started_at_ms),
            marker.last_invoke_cmd,
            stamp(marker.last_invoke_at_ms),
            marker.probes_emitted,
            ended_text(marker)
        ),
        "[ipc-blackbox] NO VERDICT: no renderer black box survived that run, so a/b/c/d cannot be separated. Either localStorage did not survive the kill or no window ever wrote a record. The `[ipc-observer] SILENCE` block from that run is what the instrument produced.".to_string(),
    ]
}

fn summary_line(entries: &[(String, &'static str)]) -> String {
    let body = entries
        .iter()
        .map(|(label, tag)| format!("{}={}", label, tag))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "[ipc-blackbox] SUMMARY {} previous-run windows: {}",
        entries.len(),
        body
    )
}

fn ended_text(marker: &FreezeMarker) -> String {
    match (marker.ended_by.as_deref(), marker.silence_ended_at_ms) {
        (Some(reason), Some(at)) => format!("{}@{}", reason, stamp(at)),
        (Some(reason), None) => reason.to_string(),
        _ => "never".to_string(),
    }
}

/// The silence block. `recent:` is wrapped so no line exceeds `MAX_LOG_LINE`.
fn silence_block(
    silent_for: u64,
    last_invoke_at_ms: u64,
    snapshot: &TickSnapshot,
    total_invokes: u64,
) -> Vec<String> {
    let mut out = vec![
        format!(
            "[ipc-observer] SILENCE: no invoke for {}ms; last was {} at {}",
            silent_for,
            snapshot.last_invoke_cmd,
            stamp(last_invoke_at_ms)
        ),
        format!(
            "[ipc-observer]   total invokes this run={}; tracked commands={}",
            total_invokes, snapshot.tracked
        ),
    ];
    const RECENT_PREFIX: &str = "[ipc-observer]   recent: ";
    let mut line = String::from(RECENT_PREFIX);
    let mut first_on_line = true;
    for (name, count, at) in &snapshot.recent {
        let piece = format!("{} x{} last {}", name, count, stamp(*at));
        let extra = if first_on_line { 0 } else { 3 };
        if !first_on_line && line.len() + extra + piece.len() > MAX_LOG_LINE {
            out.push(line);
            line = String::from(RECENT_PREFIX);
            first_on_line = true;
        }
        if !first_on_line {
            line.push_str(" | ");
        }
        line.push_str(&piece);
        first_on_line = false;
    }
    out.push(line);
    out.push(
        "[ipc-observer]   probing the renderer on `ipc_silence_probe`; the next start reports what it saw"
            .to_string(),
    );
    out
}

// ---------------------------------------------------------------------------
// Clock, marker I/O and the probe
// ---------------------------------------------------------------------------

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

/// `chrono::Local` `%H:%M:%S%.3f`, to line up with the surrounding `app.log`
/// lines.
fn stamp(ms: u64) -> String {
    match Local.timestamp_millis_opt(ms as i64) {
        chrono::LocalResult::Single(dt) => dt.format("%H:%M:%S%.3f").to_string(),
        _ => format!("+{}ms", ms),
    }
}

fn marker_path() -> Option<std::path::PathBuf> {
    config_dir().map(|dir| dir.join(MARKER_FILE))
}

/// `None` from `config_dir()` is degraded mode: skip the write, one `warn!` per
/// episode, every other leg keeps working. A write error is likewise logged once
/// per episode and never retried in a loop.
fn write_marker(marker: &FreezeMarker, warned: &mut bool) {
    let Some(path) = marker_path() else {
        if !*warned {
            log::warn!(
                "[ipc-observer] no config dir; the freeze marker cannot be written this episode"
            );
            *warned = true;
        }
        return;
    };
    let bytes = match serde_json::to_vec_pretty(marker) {
        Ok(bytes) => bytes,
        Err(err) => {
            if !*warned {
                log::warn!("[ipc-observer] freeze marker could not be serialized: {err}");
                *warned = true;
            }
            return;
        }
    };
    if let Err(err) = std::fs::write(&path, bytes) {
        if !*warned {
            log::warn!(
                "[ipc-observer] freeze marker could not be written to {}: {err}",
                path.display()
            );
            *warned = true;
        }
    }
}

fn read_marker() -> Option<FreezeMarker> {
    let bytes = std::fs::read(marker_path()?).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn delete_marker() {
    if let Some(path) = marker_path() {
        let _ = std::fs::remove_file(path);
    }
}

/// Unscoped, so every window receives it. The result is discarded: a failed emit
/// is itself evidence and must not abort the loop.
fn emit_probe(app: &AppHandle, seq: u32, now: u64) {
    let _ = app.emit(
        "ipc_silence_probe",
        serde_json::json!({ "seq": seq, "backendNowMs": now }),
    );
}

// ---------------------------------------------------------------------------
// The loop
// ---------------------------------------------------------------------------

pub fn start(app: AppHandle, observer: Arc<IpcObserver>, shutdown: ShutdownSignal) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(TICK_INTERVAL);
        let mut marker_warned = false;
        loop {
            tokio::select! {
                _ = shutdown.token().cancelled() => {
                    on_shutdown(&observer, &mut marker_warned);
                    break;
                }
                _ = interval.tick() => tick(&app, &observer, &mut marker_warned),
            }
        }
    });
}

fn tick(app: &AppHandle, observer: &IpcObserver, marker_warned: &mut bool) {
    // 1. The frontend has never talked; there is no silence.
    let total_invokes = observer.total_invokes.load(Ordering::Relaxed);
    if total_invokes == 0 {
        return;
    }
    let now = now_ms();
    let last_invoke_at_ms = observer.last_invoke_at_ms.load(Ordering::Relaxed);
    // 2.
    let silent_for = now.saturating_sub(last_invoke_at_ms);
    let snapshot = observer.snapshot_for_tick(now);
    let silent = silent_for >= SILENCE_THRESHOLD;
    match (silent, snapshot.open_episode.clone()) {
        // 3. Open the episode.
        (true, None) => {
            let marker = FreezeMarker {
                backend_started_at_ms: observer.backend_started_at_ms,
                silence_started_at_ms: now,
                last_invoke_at_ms,
                last_invoke_cmd: snapshot.last_invoke_cmd.clone(),
                probes_emitted: 1,
                silence_ended_at_ms: None,
                ended_by: None,
            };
            *marker_warned = false;
            for line in silence_block(silent_for, last_invoke_at_ms, &snapshot, total_invokes) {
                log::warn!("{}", line);
            }
            write_marker(&marker, marker_warned);
            emit_probe(app, 1, now);
            observer.apply_tick(TickChange::Open {
                marker,
                next_probe_at_ms: now + PROBE_INTERVAL,
            });
        }
        // 4. Probe again. No further log lines: one block per episode.
        (true, Some(open)) => {
            if snapshot.probe_due && open.probes_emitted < MAX_PROBES_PER_EPISODE {
                let seq = open.probes_emitted + 1;
                let mut marker = open;
                marker.probes_emitted = seq;
                emit_probe(app, seq, now);
                write_marker(&marker, marker_warned);
                observer.apply_tick(TickChange::Probe {
                    marker,
                    next_probe_at_ms: now + PROBE_INTERVAL,
                });
            }
        }
        // 5. Close it by resumed traffic.
        (false, Some(open)) => {
            let mut marker = open;
            let started = marker.silence_started_at_ms;
            marker.silence_ended_at_ms = Some(now);
            marker.ended_by = Some("traffic".to_string());
            write_marker(&marker, marker_warned);
            log::warn!(
                "[ipc-observer] silence ended after {}ms; first command after: {}",
                now.saturating_sub(started),
                snapshot.last_invoke_cmd
            );
            observer.apply_tick(TickChange::Close);
        }
        (false, None) => {}
    }
}

/// Step 6 of the tick contract: the process was told to quit WHILE STILL
/// SILENT. `Some("shutdown")` is diagnostic, separating "the user gave up on a frozen window whose close path
/// still worked" from "force-killed from Task Manager". It must NOT neutralise
/// the episode, and it does not: only `Some("traffic")` suppresses the (c) and
/// (a) arms, and not even that suppresses the probe arm.
fn on_shutdown(observer: &IpcObserver, marker_warned: &mut bool) {
    let now = now_ms();
    let snapshot = observer.snapshot_for_tick(now);
    let Some(open) = snapshot.open_episode else {
        return;
    };
    let mut marker = open;
    marker.silence_ended_at_ms = Some(now);
    marker.ended_by = Some("shutdown".to_string());
    write_marker(&marker, marker_warned);
    observer.apply_tick(TickChange::Shutdown);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// An arbitrary but fixed wall-clock instant, standing in for the last
    /// invoke the backend saw.
    const L: u64 = 1_700_000_000_000;

    /// EVERY test in this file calls `ingest_with_marker`, never
    /// `ingest_records`: in a cargo test binary `config_dir()` resolves under
    /// `target/debug/deps`, so `ingest_records` would read, and could delete, a
    /// real marker file there.
    fn marker(silence_started_at_ms: u64, last_invoke_at_ms: u64) -> FreezeMarker {
        FreezeMarker {
            backend_started_at_ms: 1,
            silence_started_at_ms,
            last_invoke_at_ms,
            last_invoke_cmd: "switch_session".to_string(),
            probes_emitted: 3,
            silence_ended_at_ms: None,
            ended_by: None,
        }
    }

    fn record() -> BlackBoxRecord {
        BlackBoxRecord {
            v: 1,
            label: "main".to_string(),
            window_type: "main".to_string(),
            visible: true,
            ..Default::default()
        }
    }

    // 1
    #[test]
    fn classify_reports_healthy_for_a_live_record() {
        let mut r = record();
        r.started_at_ms = L - 600_000;
        r.written_at_ms = L;
        r.last_raf_at_ms = L - 1_000;
        assert_eq!(classify(&r, None), Verdict::Healthy);
    }

    // 2
    #[test]
    fn classify_reports_frame_stall_only_for_a_visible_window() {
        // `started_at_ms` must be at least 5 min before `written_at_ms`:
        // `no_episode` takes `raf_floor = last_raf_at_ms.max(started_at_ms)`, so
        // a fresh `started_at_ms` would swallow the stall and collapse both
        // assertions onto `Healthy`.
        let mut r = record();
        r.started_at_ms = L - 600_000;
        r.written_at_ms = L;
        r.last_raf_at_ms = L - 300_000;
        assert_eq!(classify(&r, None), Verdict::FrameStall);
        r.visible = false;
        assert_eq!(classify(&r, None), Verdict::Healthy);
    }

    // 3
    #[test]
    fn classify_reports_task_loop_stopped_when_the_record_died_with_the_channel() {
        let m = marker(L + 90_000, L);
        let mut r = record();
        r.started_at_ms = L - 600_000;
        r.written_at_ms = L;
        r.last_raf_at_ms = L;
        assert_eq!(classify(&r, Some(&m)), Verdict::TaskLoopStopped);
        r.written_at_ms = L + 4_000;
        assert_eq!(classify(&r, Some(&m)), Verdict::TaskLoopStopped);
        // The 20 s case moved with the constant (round 2: `Inconclusive`) and is
        // what phase 2's AC 6 actually produces.
        r.written_at_ms = L + 20_000;
        assert_eq!(classify(&r, Some(&m)), Verdict::TaskLoopStopped);
    }

    // 4
    #[test]
    fn classify_reports_send_path_broken_when_a_probe_arrived_during_the_episode() {
        let s = L + 90_000;
        let m = marker(s, L);
        let mut r = record();
        r.started_at_ms = L - 600_000;
        r.written_at_ms = L + 300_000;
        r.last_raf_at_ms = L + 300_000;
        r.probe_seq = Some(2);
        r.probe_at_ms = Some(s + 1_000);
        r.last_sent_at_ms = s + 2_000;
        assert_eq!(classify(&r, Some(&m)), Verdict::SendPathBroken);
    }

    // 5
    #[test]
    fn classify_reports_ipc_dead_both_ways_without_a_probe() {
        let s = L + 90_000;
        let m = marker(s, L);
        let mut r = record();
        r.started_at_ms = L - 600_000;
        r.written_at_ms = L + 300_000;
        r.last_raf_at_ms = L + 300_000;
        r.last_sent_at_ms = s + 2_000;
        r.probe_at_ms = None;
        assert_eq!(classify(&r, Some(&m)), Verdict::IpcDeadBothWays);
        // A stale probe from an earlier episode cannot forge a `SendPathBroken`.
        r.probe_at_ms = Some(s - 1_000);
        assert_eq!(classify(&r, Some(&m)), Verdict::IpcDeadBothWays);
    }

    // 6
    #[test]
    fn note_invoke_tracks_the_last_command_and_bounds_the_table() {
        let observer = IpcObserver::new();
        for i in 0..300 {
            observer.note_invoke(&format!("cmd_{i}"));
        }
        assert_eq!(observer.snapshot_for_tick(0).tracked, MAX_TRACKED_COMMANDS);
        assert_eq!(observer.last_invoke_snapshot().2, "cmd_299");
    }

    // 7
    #[test]
    fn ingest_skips_current_run_records_and_deletes_previous_run_records() {
        let observer = IpcObserver::new();
        let base = observer.backend_started_at_ms;
        let records = vec![
            StoredRecord {
                key: "ipcbb.old".to_string(),
                json: format!(
                    r#"{{"v":1,"label":"main","startedAtMs":{}}}"#,
                    base - 10_000
                ),
            },
            StoredRecord {
                key: "ipcbb.current".to_string(),
                json: format!(
                    r#"{{"v":1,"label":"main","startedAtMs":{}}}"#,
                    base + 10_000
                ),
            },
        ];
        let outcome = observer.ingest_with_marker(records, None);
        assert_eq!(outcome.delete_keys, vec!["ipcbb.old".to_string()]);
    }

    // 8
    #[test]
    fn ingest_deletes_unparseable_and_oversized_records() {
        let observer = IpcObserver::new();
        let records = vec![
            StoredRecord {
                key: "ipcbb.broken".to_string(),
                json: "{".to_string(),
            },
            StoredRecord {
                key: "ipcbb.huge".to_string(),
                json: "x".repeat(65 * 1024),
            },
        ];
        let outcome = observer.ingest_with_marker(records, None);
        assert!(outcome.delete_keys.contains(&"ipcbb.broken".to_string()));
        assert!(outcome.delete_keys.contains(&"ipcbb.huge".to_string()));
    }

    // 9
    #[test]
    fn black_box_record_parses_with_missing_and_unknown_fields() {
        let minimal: BlackBoxRecord = serde_json::from_str(r#"{"v":1,"label":"main"}"#).unwrap();
        assert_eq!(minimal.v, 1);
        assert_eq!(minimal.label, "main");
        // An older renderer sends no such key and must read as force-killed, not
        // as cleanly closed. Default and parse are separate assertions.
        assert!(!minimal.closed_cleanly);
        let future: BlackBoxRecord =
            serde_json::from_str(r#"{"v":1,"label":"main","futureField":42}"#).unwrap();
        assert_eq!(future.v, 1);
        let counted: BlackBoxRecord = serde_json::from_str(r#"{"overdueTotal":3}"#).unwrap();
        assert_eq!(counted.overdue_total, 3);
        let closed: BlackBoxRecord = serde_json::from_str(r#"{"closedCleanly":true}"#).unwrap();
        assert!(closed.closed_cleanly);
    }

    // 10
    #[test]
    fn freeze_marker_round_trips() {
        // A bare round-trip cannot see a renamed key; the key assertion can.
        let open = marker(L + 90_000, L);
        let text = serde_json::to_string(&open).unwrap();
        assert!(text.contains("silenceEndedAtMs"));
        assert_eq!(serde_json::from_str::<FreezeMarker>(&text).unwrap(), open);

        let mut closed = open.clone();
        closed.silence_ended_at_ms = Some(L + 100_000);
        closed.ended_by = Some("traffic".to_string());
        let text = serde_json::to_string(&closed).unwrap();
        assert!(text.contains("silenceEndedAtMs"));
        assert_eq!(serde_json::from_str::<FreezeMarker>(&text).unwrap(), closed);
    }

    // 11 (C2b)
    #[test]
    fn classify_prefers_the_probe_over_the_staleness_margin() {
        let s = L + 90_000;
        let m = marker(s, L);
        let mut r = record();
        r.started_at_ms = L - 600_000;
        // Inside round 1's 90-120 s band, where round 1 returned `TaskLoopStopped`.
        r.written_at_ms = L + 100_000;
        r.last_raf_at_ms = L + 100_000;
        r.probe_at_ms = Some(s + 1_000);
        r.last_sent_at_ms = s + 500;
        assert_eq!(classify(&r, Some(&m)), Verdict::SendPathBroken);
    }

    // 12 (C2a)
    #[test]
    fn classify_reports_ipc_dead_both_ways_inside_the_old_blind_band() {
        // The offset is now the (c) boundary, so it is pinned explicitly.
        let s = L + 90_000;
        let m = marker(s, L);
        let mut r = record();
        r.started_at_ms = L - 600_000;
        r.written_at_ms = L + 100_000;
        r.last_raf_at_ms = L + 100_000;
        r.probe_at_ms = None;
        r.last_sent_at_ms = s + 500;
        assert_eq!(classify(&r, Some(&m)), Verdict::IpcDeadBothWays);
    }

    // 13 (D3)
    #[test]
    fn classify_is_inconclusive_between_the_liveness_bound_and_the_silence_onset() {
        let s = L + 90_000;
        let m = marker(s, L);
        let mut r = record();
        r.started_at_ms = L - 600_000;
        r.written_at_ms = L + 50_000;
        r.last_raf_at_ms = L + 50_000;
        let verdict = classify(&r, Some(&m));
        assert_eq!(verdict, Verdict::Inconclusive);
        assert!(verdict.explanation().contains("30000"));
        // The runtime figure must still reach `app.log`.
        let block = record_block(&r, Some(&m), verdict).join("\n");
        assert!(block.contains("outlived_ms=50000"));

        let mut edge = r.clone();
        edge.written_at_ms = L + 29_000;
        assert_eq!(classify(&edge, Some(&m)), Verdict::TaskLoopStopped);
    }

    // 14 (C2c + D2 + E2 + F1 + H1)
    #[test]
    fn classify_keeps_a_recovered_episode_out_of_the_stale_arms_but_not_out_of_the_probe_arm() {
        let s = L + 90_000;
        // The end stamp must be pinned explicitly, not left unconstrained: the
        // corroborator reads it.
        let ended = s + 10_000;
        let mut m = marker(s, L);
        m.silence_ended_at_ms = Some(ended);
        m.ended_by = Some("traffic".to_string());

        // Every record below pins `written_at_ms` explicitly and carries a
        // `last_raf_at_ms` within `RAF_STALE_MS` of it, so no assertion can be
        // satisfied by a record that would have missed the arm anyway or that
        // lands on `FrameStall` instead of `Healthy`.
        let w = s + 60_000;
        let mut base = record();
        base.started_at_ms = L - 600_000;
        base.written_at_ms = w;
        base.last_raf_at_ms = w - 1_000;
        base.probe_seq = Some(2);
        base.probe_at_ms = Some(s + 1_000);
        base.last_sent_at_ms = s + 2_000;
        base.last_settled_at_ms = s - 1_000;
        base.overdue_total = 1;
        base.pending_total = 1;
        base.pending = vec![PendingEntry {
            id: 41_206,
            cmd: "get_active_session".to_string(),
            age_ms: 65_000,
            overdue: true,
        }];

        // Another window's traffic ended the process-wide silence; this window
        // received a probe inside the episode, and the call it still holds was
        // issued at `w - 65_000 = s - 5_000`, before the silence was even
        // declared, while nothing it sent had come back since the onset.
        assert_eq!(classify(&base, Some(&m)), Verdict::SendPathBroken);

        // With no outstanding call there is no per-window evidence left.
        let mut nothing_outstanding = base.clone();
        nothing_outstanding.overdue_total = 0;
        nothing_outstanding.pending_total = 0;
        nothing_outstanding.pending = Vec::new();
        assert_eq!(classify(&nothing_outstanding, Some(&m)), Verdict::Healthy);

        // (F1) The healthy window that merely held one slow, untimed call at
        // exit. The only assertion in the file that fails on the round-4
        // classifier and passes on the round-5 one.
        let mut kept_settling = base.clone();
        kept_settling.last_settled_at_ms = s + 1_000;
        assert_eq!(classify(&kept_settling, Some(&m)), Verdict::Healthy);

        // (H1) The only entry marked `overdue` was issued at
        // `w - 6_000 = s + 54_000`, 44 s AFTER traffic resumed. The leading
        // non-overdue dialog is the second guard: an implementation reading
        // `pending.first()` derives `s - 540_000`, clears the bound trivially
        // and returns `SendPathBroken`.
        let mut late_call = base.clone();
        late_call.pending_total = 2;
        late_call.pending = vec![
            PendingEntry {
                id: 5,
                cmd: "spec_board_pick_open".to_string(),
                age_ms: 600_000,
                overdue: false,
            },
            PendingEntry {
                id: 41_206,
                cmd: "get_active_session".to_string(),
                age_ms: 6_000,
                overdue: true,
            },
        ];
        assert_eq!(classify(&late_call, Some(&m)), Verdict::Healthy);

        // The window that froze SECOND: the derived instant `w - 55_000 =
        // s + 5_000` lands inside `(silence_started_at_ms, silence_ended_at_ms]`,
        // so this is what stops the bound from being tightened to the onset.
        let mut second_victim = base.clone();
        second_victim.pending[0].age_ms = 55_000;
        assert_eq!(classify(&second_victim, Some(&m)), Verdict::SendPathBroken);

        // Row (d) is still reachable while a closed marker sits on disk.
        let mut stalled = record();
        stalled.started_at_ms = w - 600_000;
        stalled.written_at_ms = w;
        stalled.last_raf_at_ms = w - 300_000;
        assert_eq!(classify(&stalled, Some(&m)), Verdict::FrameStall);

        // (D2) `ended_by: Some("shutdown")` is the opposite of `"traffic"`: the
        // silence never ended, so the episode is still an episode.
        let mut quit_while_silent = m.clone();
        quit_while_silent.ended_by = Some("shutdown".to_string());
        assert_eq!(
            classify(&base, Some(&quit_while_silent)),
            Verdict::SendPathBroken
        );
    }

    // 15 (C6b)
    #[test]
    fn ingest_reports_and_deletes_a_previous_run_marker_with_no_records() {
        let observer = IpcObserver::new();
        let mut previous = marker(L + 90_000, L);
        previous.backend_started_at_ms = observer.backend_started_at_ms - 1;
        let outcome = observer.ingest_with_marker(Vec::new(), Some(&previous));
        let text = outcome.lines.join("\n");
        assert!(text.contains("[ipc-blackbox] marker only:"));
        assert!(text.contains("NO VERDICT"));
        assert!(outcome.marker_reported);

        let mut current = previous.clone();
        current.backend_started_at_ms = observer.backend_started_at_ms;
        let outcome = observer.ingest_with_marker(Vec::new(), Some(&current));
        assert!(outcome.lines.is_empty());
        assert!(!outcome.marker_reported);
    }

    // 16
    #[test]
    fn summary_line_names_every_window_and_verdict() {
        let line = summary_line(&[
            ("main".to_string(), Verdict::SendPathBroken.tag()),
            ("spec-board".to_string(), Verdict::Healthy.tag()),
            ("watchers".to_string(), Verdict::FrameStall.tag()),
        ]);
        assert!(line.contains("SUMMARY 3 previous-run windows"));
        assert!(line.contains("main=b/send-path-broken"));
        assert!(line.contains("spec-board=healthy"));
        assert!(line.contains("watchers=d/frame-stall"));
    }

    // 17 (D1)
    #[test]
    fn classify_excludes_a_cleanly_closed_window_from_task_loop_stopped_and_inconclusive() {
        // The stale spec-board window: `last_invoke_at_ms` eight hours AFTER the
        // record's `written_at_ms`.
        let written = L;
        let last_invoke = written + 8 * 3_600_000;
        let s = last_invoke + 90_000;
        let m = marker(s, last_invoke);
        let mut r = record();
        r.started_at_ms = written - 600_000;
        r.written_at_ms = written;
        r.last_raf_at_ms = written;
        r.closed_cleanly = true;
        assert_eq!(classify(&r, Some(&m)), Verdict::Healthy);

        // The forged verdict reached by `saturating_sub` underflow.
        let mut killed = r.clone();
        killed.closed_cleanly = false;
        assert_eq!(classify(&killed, Some(&m)), Verdict::TaskLoopStopped);

        // The flag suppresses neither (b) nor (c). That third record's
        // `probe_at_ms` post-dates its own `written_at_ms` by about eight hours,
        // which is not a realizable state: it pins ARM ORDERING on a pure
        // function, it does not model a scenario.
        let mut probed = r.clone();
        probed.probe_at_ms = Some(s + 1_000);
        probed.last_sent_at_ms = s + 2_000;
        assert_eq!(classify(&probed, Some(&m)), Verdict::SendPathBroken);
    }

    // 18 (E1)
    #[test]
    fn classify_reports_bystander_when_a_probe_arrived_and_the_window_never_sent() {
        let s = L + 90_000;
        let m = marker(s, L);
        let mut r = record();
        r.started_at_ms = L - 600_000;
        r.written_at_ms = L + 50_000;
        r.last_raf_at_ms = L + 50_000;
        r.probe_at_ms = Some(s + 1_000);
        r.last_sent_at_ms = s - 60_000;
        assert_eq!(classify(&r, Some(&m)), Verdict::Bystander);

        // The send is the only thing separating the two.
        let mut sent = r.clone();
        sent.last_sent_at_ms = s + 1;
        assert_eq!(classify(&sent, Some(&m)), Verdict::SendPathBroken);

        // A recorded probe falsifies "recorded none" for that window, so it must
        // never fall into the (c) arm.
        let mut late = r.clone();
        late.written_at_ms = s + 60_000;
        late.last_raf_at_ms = s + 60_000;
        assert_eq!(classify(&late, Some(&m)), Verdict::Bystander);
    }
}
