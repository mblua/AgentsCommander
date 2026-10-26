//! #2064 Phase A - the remote activity producer.
//!
//! One polling sweeper answers two questions per repo: is CI running on the
//! repo's exact `HEAD`, and is the repository's default branch ahead of it. Both
//! answers come from the external `gh` binary, because the local shortcut is
//! invalid: room repos are shallow clones, so a missing object does not mean "not
//! an ancestor".
//!
//! The sweeper publishes a path-keyed snapshot (`REMOTE_ACTIVITY_SNAPSHOT`) and
//! emits one `ac_remote_activity_updated` payload per changed round. The
//! transition stream (`RemoteTransition`) is produced here but consumed only by
//! Phase B, so `lib.rs` drops its receiver for the life of this phase.
//!
//! `gh` is not pinned by the repository and is absent on many machines. Every
//! failure - absent, unauthenticated, timed out, rate limited, unparseable -
//! maps to `Unknown`, never to a confident `Idle`/`Running`/`Current`. No test in
//! this module invokes the real `gh` or the network: queries go through the
//! injected `GhSpawner`, local git reads through the injected `LocalGitRunner`.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

#[cfg(test)]
use crate::config::remote_activity_cache::{PersistedCiState, REMOTE_ACTIVITY_SNAPSHOT_FILE_NAME};
#[cfg(test)]
use crate::config::settings::SettingsState;
#[cfg(test)]
use crate::session::manager::SessionManager;
#[cfg(test)]
use crate::shutdown::ShutdownSignal;
#[cfg(test)]
use chrono::Utc;
use chrono::{DateTime, Local};
use serde::Serialize;
#[cfg(test)]
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::Arc;
#[cfg(test)]
use std::time::Instant;
#[cfg(test)]
use tokio::sync::mpsc;

mod ci_parse;
mod gh_command;
mod outcome;
mod query;
mod schedule;
mod seams;
mod sweeper;
mod sweeper_state;

use ci_parse::{
    corroborate_ci_run, merge_ci_observations, parse_ci_response, parse_compare_response,
    parse_github_nwo, CiCompletionDecision, CiCompletionState, CompareAnswer,
};
#[cfg(test)]
use ci_parse::{parse_ci_jobs_response, parse_ci_run_response, CiAnswer, CiRunObservation};
use gh_command::{build_gh_command_spec, is_sha40, GhCommandSpec, GhQuery};
#[cfg(test)]
use outcome::persisted_failure_reason;
use outcome::{activity_for, persisted_repo_ci};
use query::{query_key, room_map, run_local_git, run_query, spawn_gh};
use schedule::{
    axis_label, ci_base_interval, clamp_interval_secs, due, failure_interval, jitter_offset, Axis,
};
pub(crate) use seams::RemoteSweeperSeams;
use seams::{
    failure_kind, Emitter, FailureKind, GhCallOutput, GhProbe, GhSpawner, JitterSource,
    LocalGitRunner, PublicationClock,
};
pub(crate) use sweeper_state::RemoteSweeper;
#[cfg(test)]
use sweeper_state::SnapshotWarningLimiter;
use sweeper_state::{
    KeyOutcome, KeyPlan, PathFacts, QueryKey, RoundCtx, SnapshotPersistence, StalenessCarry,
    SweeperState, TransitionMeta,
};

/// The CI check cadence for a key whose last confirmed state is `Running`, and
/// the clamp floor for both interval dials.
const CI_RUNNING_INTERVAL_SECS: u64 = 10;
const INTERVAL_FLOOR_SECS: u64 = 10;
const INTERVAL_CEILING_SECS: u64 = 3600;
/// Bound on one `gh` call and one local git call. Hanging is not an answer.
const CALL_TIMEOUT: Duration = Duration::from_secs(10);
/// Ceiling on a failing key's interval. 15 minutes.
const BACKOFF_CAP: Duration = Duration::from_secs(900);
/// These are calls to one host; more parallelism buys 429s, not freshness.
const QUERY_CONCURRENCY: usize = 2;
/// Matches `CONTEXT_SAMPLE_QUEUE_CAPACITY`: a stuck consumer must never slow a round.
const TRANSITION_QUEUE_CAPACITY: usize = 1024;
/// The loop tick. Per-key due times decide the work, this only decides how often
/// due times are checked.
const ROUND_INTERVAL: Duration = Duration::from_secs(10);

/// Global ceiling on sweeper-issued `gh` calls, per minute, per process.
/// GitHub's secondary budget is per account and per minute, and the same
/// account may run a second instance on another machine, so the ceiling is
/// set well under the budget rather than at it.
const MAX_GH_CALLS_PER_MINUTE: f64 = 30.0;
/// Burst ceiling for the bucket. Equal to the per-minute rate: one minute of
/// idleness buys at most one minute of work.
const GH_BUDGET_BURST: f64 = MAX_GH_CALLS_PER_MINUTE;
/// First backoff for a SECONDARY limit. A secondary limit is a burst signal,
/// not an exhausted quota, so it does not jump to `BACKOFF_CAP`.
const SECONDARY_BACKOFF_BASE: Duration = Duration::from_secs(60);

/// The label rendered for `%BASE%` when neither GitHub nor the local clone can
/// name the default branch. It is a label in a sentence, never a query argument,
/// so an unresolved label degrades the text and nothing else. A `BranchStale`
/// notice is NEVER suppressed for want of it.
const DEFAULT_BRANCH_LABEL: &str = "the default branch";

/// #2129 - the rendered `%BASE%` for a branch-stale notice. The comparison is
/// `repos/<nwo>/compare/HEAD...<sha>`, answered by GitHub, so the text says
/// GitHub and never `origin/<x>`: a local remote may be named otherwise, be
/// stale, or not exist. The branch name is never printed twice: an unresolved
/// base keeps its sentinel prose, and a base equal to the branch is named by
/// relation. The raw label is left untouched everywhere else, because the
/// #2131 suppression compares it for identity.
pub(crate) fn base_branch_display(branch: &str, base_label: &str) -> String {
    if base_label == DEFAULT_BRANCH_LABEL {
        format!("{DEFAULT_BRANCH_LABEL} on GitHub")
    } else if base_label == branch {
        "its counterpart on GitHub".to_string()
    } else {
        format!("{base_label} on GitHub")
    }
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// The per-round line is a `debug!`, deliberately diverging from `GitSweeper`'s
/// `info!` round line: at this cadence an `info!` is about 2880 lines a day for
/// every user. Exposed as a value so a test can read the level instead of
/// pattern-matching a literal buried in a macro call.
pub(super) const ROUND_LOG_LEVEL: log::Level = log::Level::Debug;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum CiState {
    Unknown,
    Idle,
    Running,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum StalenessState {
    Unknown,
    Current,
    Stale,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemoteActivityPayload {
    repo_paths: Vec<String>,
    ci_states: Vec<CiState>,
    staleness_states: Vec<StalenessState>,
    behind_by: Vec<Option<u32>>,
}

/// One path's published answer. `behind_by` is `Some` only while `Stale`, which
/// is the only state whose template renders it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RemoteActivity {
    ci: CiState,
    staleness: StalenessState,
    behind_by: Option<u32>,
    /// #2473 snapshot-only detail: in-progress run ids and their PR numbers
    /// (non-empty only while `ci` is `Running`) and the failure behind an
    /// `Unknown` chip. Never part of `RemoteActivityPayload`.
    run_ids: Vec<u64>,
    pull_requests: Vec<u64>,
    failure: Option<FailureKind>,
}

impl RemoteActivity {
    fn unknown() -> Self {
        Self {
            ci: CiState::Unknown,
            staleness: StalenessState::Unknown,
            behind_by: None,
            run_ids: Vec::new(),
            pull_requests: Vec::new(),
            failure: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TransitionKind {
    CiStarted,
    CiFinished,
    BranchStale,
}

/// One notice candidate. The transition key is `(room_dir, repo_path, head_sha)`:
/// the query unit is per commit, the notice unit is per room, because two rooms
/// are two working contexts with two orchestrators.
///
/// `dead_code` is allowed on the fields past the drop log: they are the Phase B
/// contract, and nothing in Phase A reads them (the in-file tests do).
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct RemoteTransition {
    pub repo_path: String,
    pub room_dir: String,
    pub nwo: String,
    pub branch: String,
    /// Full 40 characters, never abbreviated: an abbreviated SHA makes the CI
    /// query answer `total_count: 0`, a confident, silent `Idle`.
    pub head_sha: String,
    pub kind: TransitionKind,
    /// `Some` only for `BranchStale`.
    pub behind_by: Option<u32>,
    /// Non-empty iff `BranchStale`; `""` for `CiStarted`/`CiFinished`.
    pub base_branch: String,
    pub observed_at: DateTime<Local>,
    /// The `wall` of the previous confirmed query for this key, never the previous
    /// state CHANGE, so `observed_at - last_confirmed_at` is time since a
    /// successful query and not time since the state moved.
    pub last_confirmed_at: Option<DateTime<Local>>,
}

/// The path-keyed snapshot the UI consumer (Phase C) reads. Keyed by the EXACT
/// path string the work list handed the sweeper (INV-3): any normalization here
/// silently turns a consumer read into a miss, which renders as a permanent
/// unknown chip with no error anywhere.
///
/// `.unwrap_or_else(|e| e.into_inner())` is required, not stylistic: one
/// poisoning with `.unwrap()` takes the sweeper down forever.
static REMOTE_ACTIVITY_SNAPSHOT: OnceLock<Mutex<HashMap<String, RemoteActivity>>> = OnceLock::new();

fn remote_activity_snapshot() -> &'static Mutex<HashMap<String, RemoteActivity>> {
    REMOTE_ACTIVITY_SNAPSHOT.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(test)]
mod tests;
