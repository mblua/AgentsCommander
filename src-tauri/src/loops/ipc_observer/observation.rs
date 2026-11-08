use std::collections::HashMap;

use serde::{Deserialize, Serialize};

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
                "the window was visible and painted no animation frame for at least RAF_STALE_MS \
                    (120000), so the renderer's paint loop stalled while the IPC channel kept \
                    working"
            }
            Verdict::TaskLoopStopped => {
                "the renderer stopped writing its black box within LOOP_OUTLIVED_MS (30000) of the \
                    last invoke the backend saw, so its JS task loop was already dead before the \
                    silence onset and it never had a probe to miss"
            }
            Verdict::SendPathBroken => {
                "this window received a silence probe and went on handing calls to the transport \
                    during the episode, yet the backend saw none of them arrive and nothing this \
                    window sent came back, so the frontend -> backend send path is broken"
            }
            Verdict::IpcDeadBothWays => {
                "the renderer went on writing its black box at or after the silence onset and \
                    recorded no probe, so neither direction of the IPC channel was working"
            }
            Verdict::Bystander => {
                "this window received a silence probe, so its JS task loop and the backend -> \
                    renderer direction were both alive during the episode, and it issued no invoke \
                    at or after the silence onset, so its own send path was never exercised: the \
                    episode is not this window's to explain"
            }
            Verdict::Inconclusive => {
                "the renderer's last record predates the silence onset, so it may or may not have \
                    been running when the first probe went out; it outlived the last invoke by at \
                    least LOOP_OUTLIVED_MS (30000), so it cannot be called a/task-loop-stopped \
                    either"
            }
        }
    }
}

/// Everything the tick's DECISION half needs, all owned, taken in one
/// acquisition so no formatting or I/O happens under the lock.
pub(super) struct TickSnapshot {
    pub(super) open_episode: Option<FreezeMarker>,
    pub(super) last_invoke_cmd: String,
    /// `(name, count, last_at_ms)`, most recent first, `COMMANDS_LOGGED` long.
    pub(super) recent: Vec<(String, u64, u64)>,
    pub(super) tracked: usize,
    pub(super) probe_due: bool,
}

/// The ceiling BELOW which the renderer's task loop is proved to have stopped
/// before the silence onset, and it is used only in that direction:
/// `outlived_ms < LOOP_OUTLIVED_MS` proves the loop was already dead before the
/// first probe was emitted. The forward reading ("`>=` proves liveness") is its
/// converse and is NOT what this bound licenses.
pub(super) const LOOP_OUTLIVED_MS: u64 = 30_000;
