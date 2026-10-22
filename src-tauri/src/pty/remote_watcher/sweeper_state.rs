use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};
use tokio::sync::mpsc;

use crate::config::remote_activity_cache::REMOTE_ACTIVITY_SNAPSHOT_MAX_AGE_SECS;
use crate::config::settings::SettingsState;
use crate::session::manager::SessionManager;

use super::{
    Axis, CiCompletionDecision, CiCompletionState, CiState, CompareAnswer, Emitter, FailureKind,
    GhProbe, GhSpawner, JitterSource, LocalGitRunner, PublicationClock, RemoteActivityPayload,
    RemoteTransition, StalenessState, TransitionKind, GH_BUDGET_BURST,
};

/// The query unit: one network call per distinct `(nwo, head_sha, branch)` per
/// axis. The branch belongs to the identity because one commit can carry runs
/// from several branches, and only the repo's current branch may answer.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct QueryKey {
    pub(crate) nwo: String,
    pub(crate) sha40: String,
    pub(crate) branch: String,
}

pub(crate) struct CiAxis {
    /// Last published answer. Set to `Unknown` on every failure; a round that is
    /// merely not due leaves it untouched.
    pub(crate) chip: CiState,
    /// Last CONFIRMED answer (`Idle`/`Running`). `Unknown` never overwrites it,
    /// which is what makes the transition stream ignore transient failures.
    pub(crate) confirmed: Option<CiState>,
    pub(crate) last_confirmed_at: Option<DateTime<Local>>,
    pub(crate) next_due: Option<Instant>,
    pub(crate) failure_interval: Option<Duration>,
    /// #2473: the published answer's in-progress run ids / PR numbers (kept
    /// only while `chip` is `Running`) and the failure behind an `Unknown`
    /// chip. A round that is not due leaves them untouched, like `chip`.
    pub(crate) run_ids: Vec<u64>,
    pub(crate) pull_requests: Vec<u64>,
    pub(crate) failure: Option<FailureKind>,
    pub(crate) completion: CiCompletionState,
}

impl Default for CiAxis {
    fn default() -> Self {
        Self {
            chip: CiState::Unknown,
            confirmed: None,
            last_confirmed_at: None,
            next_due: None,
            failure_interval: None,
            run_ids: Vec::new(),
            pull_requests: Vec::new(),
            failure: None,
            completion: CiCompletionState::default(),
        }
    }
}

/// What #2141 carries from a retired key to a live key on the same
/// `(nwo, branch)`. Deliberately just these two fields: see the seeding step.
#[derive(Clone, Copy)]
pub(crate) struct StalenessCarry {
    pub(crate) confirmed: StalenessState,
    pub(crate) last_confirmed_at: Option<DateTime<Local>>,
}

pub(crate) struct StalenessAxis {
    pub(crate) chip: StalenessState,
    pub(crate) confirmed: Option<StalenessState>,
    pub(crate) behind_by: Option<u32>,
    pub(crate) last_confirmed_at: Option<DateTime<Local>>,
    pub(crate) next_due: Option<Instant>,
    pub(crate) failure_interval: Option<Duration>,
}

impl Default for StalenessAxis {
    fn default() -> Self {
        Self {
            chip: StalenessState::Unknown,
            confirmed: None,
            behind_by: None,
            last_confirmed_at: None,
            next_due: None,
            failure_interval: None,
        }
    }
}

#[derive(Default)]
pub(crate) struct QueryState {
    pub(crate) ci: CiAxis,
    pub(crate) staleness: StalenessAxis,
}

/// The named, one-time construction state of the snapshot producer. A `None`
/// destination is a wiring fact captured by `with_capacity` together with the
/// single diagnostic logged there: rounds read this state, so a directory that
/// appears later cannot start writes and a missing one never warns again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SnapshotPersistence {
    /// A destination directory was wired; every round publishes into it.
    Enabled,
    /// Construction had no directory: the one-time diagnostic was logged and
    /// every round skips persistence without touching memory, emission,
    /// transitions or later rounds.
    DisabledMissingAtConstruction,
}

/// Rate limiter for snapshot write-failure warnings, following the observable
/// in-module `warned_failures` precedent. A broken destination is a per-round
/// condition at a 10 s cadence, so the log admits one warning per
/// `REMOTE_ACTIVITY_SNAPSHOT_MAX_AGE_SECS` window. Pure state: no logger and no
/// clock beyond the round's injected monotonic `Instant`.
#[derive(Default)]
pub(crate) struct SnapshotWarningLimiter {
    pub(crate) last_admitted: Option<Instant>,
}

impl SnapshotWarningLimiter {
    pub(crate) fn admit(&mut self, now: Instant) -> bool {
        match self.last_admitted {
            Some(last)
                if now.saturating_duration_since(last)
                    < Duration::from_secs(REMOTE_ACTIVITY_SNAPSHOT_MAX_AGE_SECS) =>
            {
                false
            }
            _ => {
                self.last_admitted = Some(now);
                true
            }
        }
    }

    pub(crate) fn reset(&mut self) {
        self.last_admitted = None;
    }
}

pub(crate) struct SweeperState {
    pub(crate) gh_path: Option<PathBuf>,
    pub(crate) keys: HashMap<QueryKey, QueryState>,
    /// Base-branch labels resolved once per `nwo` for the life of the process.
    pub(crate) base_branches: HashMap<String, String>,
    pub(crate) warned_base_branch: HashSet<String>,
    pub(crate) warned_failures: HashSet<(QueryKey, Axis)>,
    pub(crate) warned_transition_drops: HashSet<String>,
    /// Once per PROCESS, not per key: a closed channel is a Phase B wiring fact.
    pub(crate) closed_warned: bool,
    pub(crate) last_payload: Option<RemoteActivityPayload>,
    /// Token bucket for sweeper-issued `gh` calls. Every `gh` call the round can
    /// issue is reserved from this bucket BEFORE it is issued, so the ceiling is
    /// a guarantee, not an estimate.
    pub(crate) budget_tokens: f64,
    pub(crate) budget_refilled_at: Option<Instant>,
    /// While set and not elapsed, the round issues NO `gh` call at all.
    pub(crate) throttled_until: Option<Instant>,
    /// One-warning-per-window limiter for snapshot write failures.
    pub(crate) snapshot_warnings: SnapshotWarningLimiter,
}

impl Default for SweeperState {
    fn default() -> Self {
        Self {
            gh_path: None,
            keys: HashMap::new(),
            base_branches: HashMap::new(),
            warned_base_branch: HashSet::new(),
            warned_failures: HashSet::new(),
            warned_transition_drops: HashSet::new(),
            closed_warned: false,
            last_payload: None,
            budget_tokens: GH_BUDGET_BURST,
            budget_refilled_at: None,
            throttled_until: None,
            snapshot_warnings: SnapshotWarningLimiter::default(),
        }
    }
}

pub(crate) struct PathFacts {
    pub(crate) path: String,
    pub(crate) room_dirs: Vec<String>,
    pub(crate) branch: Option<String>,
    pub(crate) head_sha: Option<String>,
    pub(crate) nwo: Option<String>,
}

#[derive(Clone, Copy)]
pub(crate) struct KeyPlan {
    pub(crate) ci: bool,
    pub(crate) staleness: bool,
}

#[derive(Default)]
pub(crate) struct KeyOutcome {
    pub(crate) ci: Option<Result<CiState, FailureKind>>,
    /// Set when the CI answer was suppressed: the branch is the resolved default
    /// branch, or it is identical to the default branch (#2126). Carried beside
    /// `ci` because `CiState` has no `Suppressed` value.
    pub(crate) ci_suppressed: bool,
    /// Set only when the identity compare was issued as an EXTRA `gh` call, so
    /// the reservation made for it is refunded when it was never made.
    pub(crate) ci_identity_call: bool,
    pub(crate) staleness: Option<CompareAnswer>,
    /// #2473: `(run_ids, pull_requests)` of a non-suppressed `Ok` answer; empty
    /// otherwise.
    pub(crate) ci_runs: (Vec<u64>, Vec<u64>),
    pub(crate) ci_completion: Option<CiCompletionState>,
    pub(crate) ci_decision: Option<CiCompletionDecision>,
    pub(crate) ci_corroboration_calls: u32,
}

pub(crate) struct RoundCtx<'a> {
    pub(crate) facts: &'a [PathFacts],
    pub(crate) now: Instant,
    pub(crate) wall: DateTime<Local>,
    pub(crate) ci_dial: Duration,
    pub(crate) staleness_dial: Duration,
}

pub(crate) struct TransitionMeta {
    pub(crate) kind: TransitionKind,
    pub(crate) behind_by: Option<u32>,
    pub(crate) base_branch: String,
    pub(crate) last_confirmed_at: Option<DateTime<Local>>,
}

/// The single global producer of remote activity. Takes `SettingsState` and an
/// injected emitter rather than an `AppHandle`: it never emits anything itself,
/// so it must not acquire a Tauri transport dependency it does not need.
pub(crate) struct RemoteSweeper {
    pub(crate) session_manager: Arc<tokio::sync::RwLock<SessionManager>>,
    pub(crate) settings: SettingsState,
    pub(crate) emit: Mutex<Emitter>,
    pub(crate) probe: GhProbe,
    pub(crate) spawner: GhSpawner,
    pub(crate) local_git_runner: LocalGitRunner,
    pub(crate) jitter: JitterSource,
    pub(crate) transitions: mpsc::Sender<RemoteTransition>,
    /// Named one-time construction state; see [`SnapshotPersistence`].
    pub(crate) snapshot_persistence: SnapshotPersistence,
    /// `Some` iff `snapshot_persistence` is `Enabled`.
    pub(crate) snapshot_dir: Option<PathBuf>,
    pub(crate) publication_clock: PublicationClock,
    pub(crate) state: Mutex<SweeperState>,
}
