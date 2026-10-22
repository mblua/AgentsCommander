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

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Local, Utc};
use futures::stream::StreamExt;
use serde::Serialize;
use tokio::sync::mpsc;

use crate::config::remote_activity_cache::{
    PersistedCiState, PersistedRepoCi, REMOTE_ACTIVITY_SNAPSHOT_FILE_NAME,
    REMOTE_ACTIVITY_SNAPSHOT_MAX_AGE_SECS,
};
use crate::config::settings::SettingsState;
use crate::session::manager::SessionManager;
use crate::shutdown::ShutdownSignal;

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

/// A `gh` invocation, fully described before any process exists.
///
/// Fields carry NO visibility modifier, so they are private to this module; the
/// in-file `#[cfg(test)] mod tests` reads them as a child module. The builder
/// validates its inputs by construction, which is what makes an abbreviated SHA
/// impossible to send rather than detectable afterwards.
pub(super) struct GhCommandSpec {
    program: PathBuf,
    args: Vec<String>,
    envs: Vec<(String, String)>,
    creation_flags: u32,
}

pub(super) enum GhQuery<'a> {
    Ci {
        nwo: &'a str,
        sha40: &'a str,
    },
    CiRun {
        nwo: &'a str,
        run_id: u64,
    },
    CiAttemptJobs {
        nwo: &'a str,
        run_id: u64,
        attempt: u64,
    },
    Compare {
        nwo: &'a str,
        sha40: &'a str,
    },
    RepoInfo {
        nwo: &'a str,
    },
}

/// The only place a `gh` argument string is built. Every input is validated
/// here, so a malformed `nwo` or a short SHA yields `Err` and no spec at all.
fn build_gh_command_spec(gh: &Path, query: GhQuery<'_>) -> Result<GhCommandSpec, String> {
    let args = match query {
        GhQuery::Ci { nwo, sha40 } => {
            validate_nwo(nwo)?;
            validate_sha40(sha40)?;
            vec![
                "api".to_string(),
                format!("repos/{nwo}/actions/runs?head_sha={sha40}&per_page=100"),
            ]
        }
        GhQuery::CiRun { nwo, run_id } => {
            validate_nwo(nwo)?;
            if run_id == 0 {
                return Err("invalid run id".to_string());
            }
            vec![
                "api".to_string(),
                format!("repos/{nwo}/actions/runs/{run_id}"),
            ]
        }
        GhQuery::CiAttemptJobs {
            nwo,
            run_id,
            attempt,
        } => {
            validate_nwo(nwo)?;
            if run_id == 0 || attempt == 0 {
                return Err("invalid run identity".to_string());
            }
            vec![
                "api".to_string(),
                format!("repos/{nwo}/actions/runs/{run_id}/attempts/{attempt}/jobs?per_page=100"),
            ]
        }
        GhQuery::Compare { nwo, sha40 } => {
            validate_nwo(nwo)?;
            validate_sha40(sha40)?;
            vec![
                "api".to_string(),
                format!("repos/{nwo}/compare/HEAD...{sha40}"),
            ]
        }
        GhQuery::RepoInfo { nwo } => {
            validate_nwo(nwo)?;
            vec!["api".to_string(), format!("repos/{nwo}")]
        }
    };

    Ok(GhCommandSpec {
        program: gh.to_path_buf(),
        args,
        // `GH_PROMPT_DISABLED` is what makes an unauthenticated `gh` exit instead
        // of blocking a round on a prompt; `gh auth login` is never invoked.
        envs: vec![
            ("GH_PROMPT_DISABLED".to_string(), "1".to_string()),
            ("GH_NO_UPDATE_NOTIFIER".to_string(), "1".to_string()),
            ("GH_PAGER".to_string(), "cat".to_string()),
        ],
        creation_flags: {
            #[cfg(windows)]
            {
                CREATE_NO_WINDOW
            }
            #[cfg(not(windows))]
            {
                0
            }
        },
    })
}

fn validate_nwo(nwo: &str) -> Result<(), String> {
    let mut segments = nwo.split('/');
    let owner = segments.next().unwrap_or_default();
    let repo = segments.next().unwrap_or_default();
    if segments.next().is_some() || owner.is_empty() || repo.is_empty() {
        return Err(format!("invalid repository nwo: {nwo}"));
    }
    if !nwo
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/'))
    {
        return Err(format!("invalid repository nwo: {nwo}"));
    }
    Ok(())
}

fn validate_sha40(sha: &str) -> Result<(), String> {
    if is_sha40(sha) {
        Ok(())
    } else {
        Err(format!(
            "sha must be 40 lowercase hex characters, got: {sha}"
        ))
    }
}

fn is_sha40(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The only place a `gh` `Command` is constructed. Pure: it spawns nothing, so a
/// test can inspect the environment and flags it would apply.
fn command_from_spec(spec: GhCommandSpec) -> tokio::process::Command {
    let GhCommandSpec {
        program,
        args,
        envs,
        creation_flags,
    } = spec;
    let mut command = tokio::process::Command::new(program);
    command.args(&args);
    for (key, value) in &envs {
        command.env(key, value);
    }
    command.kill_on_drop(true);
    crate::pty::credentials::scrub_credentials_from_tokio_command(&mut command);
    crate::config::agent_path::apply_search_path_to_tokio_command(&mut command);
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(command.as_std_mut(), creation_flags);
    // On non-Windows there is no API to apply creation flags; the read keeps the
    // field part of the spec's contract rather than dead on those targets.
    #[cfg(not(windows))]
    let _ = creation_flags;
    command
}

/// The only place a `git` `Command` is constructed. Pure, like
/// `command_from_spec`. Gate 2 of `detect_git_status` (`GIT_CEILING_DIRECTORIES`)
/// lives here so a repo whose `.git` exists but is corrupt cannot walk up and
/// answer with an unrelated ancestor's data. Gate 1 (the async `.git` metadata
/// check) runs once per path in the caller, before any command is built.
fn git_command(path: &str, args: &[String]) -> tokio::process::Command {
    let mut command = tokio::process::Command::new("git");
    command
        .arg("-C")
        .arg(path)
        .arg("--no-optional-locks")
        .args(args)
        .kill_on_drop(true);
    crate::pty::credentials::scrub_credentials_from_tokio_command(&mut command);
    crate::config::agent_path::apply_search_path_to_tokio_command(&mut command);

    if let Some(parent) = Path::new(path).parent() {
        if let Ok(ceiling) = std::env::join_paths(std::iter::once(parent)) {
            command.env("GIT_CEILING_DIRECTORIES", ceiling);
        }
    }

    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(command.as_std_mut(), CREATE_NO_WINDOW);
    command
}

/// The emitter seam's boxed shape, named so the struct field is not a
/// `clippy::type_complexity` trigger.
type Emitter = Box<dyn FnMut(&RemoteActivityPayload) + Send>;

struct GhCallOutput {
    stdout: String,
    stderr: String,
    success: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum FailureKind {
    Timeout,
    RateLimited,
    SecondaryRateLimited,
    NotAuthenticated,
    Incomplete,
    Other,
}

type GhFuture =
    Pin<Box<dyn std::future::Future<Output = Result<GhCallOutput, FailureKind>> + Send>>;
type GhSpawner = Arc<dyn Fn(GhCommandSpec) -> GhFuture + Send + Sync>;

type LocalGitFuture = Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send>>;
type LocalGitRunner = Arc<dyn Fn(String, Vec<String>) -> LocalGitFuture + Send + Sync>;

type GhProbe = Arc<dyn Fn() -> Option<PathBuf> + Send + Sync>;

/// Returns a fraction in [0.0, 1.0). Production draws from `getrandom`; tests
/// pin it so every backoff assertion is exact.
type JitterSource = Arc<dyn Fn() -> f64 + Send + Sync>;

/// The process seams plus the jitter source. Production wires them in
/// `production_seams`; tests inject recorded results so no test ever spawns
/// `gh` or touches the network.
pub(crate) struct RemoteSweeperSeams {
    probe: GhProbe,
    spawner: GhSpawner,
    local_git: LocalGitRunner,
    jitter: JitterSource,
    /// Destination directory for the neutral snapshot, or `None` when the host
    /// wired no instance config directory. Captured once at construction; see
    /// [`SnapshotPersistence`].
    snapshot_dir: Option<PathBuf>,
    /// Reads the publication instant written as `generatedAt`, invoked once per
    /// round immediately before the blocking write.
    publication_clock: PublicationClock,
}

/// The publication clock seam. Production is `Utc::now`; tests inject a fixed
/// instant so freshness assertions are exact.
type PublicationClock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

/// Program-independent failure classification, from the text `gh` prints.
/// Deliberately textual: `gh api` reports HTTP outcomes as exit status 1 plus a
/// message, and the exact argument list is pinned by test 1, so no `--include`
/// is available to read headers.
fn failure_kind(output: &GhCallOutput) -> FailureKind {
    let text = output.stderr.to_ascii_lowercase();
    if text.contains("secondary rate limit") || text.contains("abuse detection") {
        FailureKind::SecondaryRateLimited
    } else if text.contains("rate limit") || text.contains("x-ratelimit-remaining: 0") {
        FailureKind::RateLimited
    } else if text.contains("not logged into any github hosts") || text.contains("http 401") {
        FailureKind::NotAuthenticated
    } else {
        FailureKind::Other
    }
}

/// The compare answer for one key: the staleness state, `behind_by`, `ahead_by`
/// and whether GitHub said `identical`. `ahead_by` is #2141's suppression input:
/// `Some(0)` means the branch carries no commit of its own, and `None` means the
/// field was absent or out of range, which fails open.
type CompareAnswer = Result<(StalenessState, Option<u32>, Option<u32>, bool), FailureKind>;

/// The CI answer for one branch: the filtered state plus whether that branch
/// has any run at all, which the identical-to-default rule reads. `run_ids`
/// and `pull_requests` (#2473) come from the kept rows that are not completed.
#[derive(Clone, Debug, PartialEq, Eq)]
struct CiAnswer {
    state: CiState,
    branch_has_runs: bool,
    observations: Vec<CiRunObservation>,
    run_ids: Vec<u64>,
    pull_requests: Vec<u64>,
}

const MAX_CI_KNOWN_RUNS: usize = 100;

#[derive(Clone, Debug, PartialEq, Eq)]
struct CiRunObservation {
    id: u64,
    workflow_id: u64,
    run_attempt: u64,
    head_sha: String,
    head_branch: String,
    status: Option<String>,
    conclusion: Option<String>,
    updated_at: String,
    pull_requests: Vec<u64>,
}

impl CiRunObservation {
    fn completed(&self) -> bool {
        self.status.as_deref() == Some("completed")
    }
    fn proof(&self) -> CiCompletionProof {
        CiCompletionProof {
            id: self.id,
            workflow_id: self.workflow_id,
            run_attempt: self.run_attempt,
            updated_at: self.updated_at.clone(),
            conclusion: self.conclusion.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CiCompletionProof {
    id: u64,
    workflow_id: u64,
    run_attempt: u64,
    updated_at: String,
    conclusion: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct CiCompletionState {
    known: BTreeMap<u64, CiRunObservation>,
    proofs: BTreeMap<u64, CiCompletionProof>,
    closing_ready: bool,
    overflow: bool,
    active_ids: Vec<u64>,
    active_prs: Vec<u64>,
}

impl CiCompletionState {
    fn cancel_candidate(&mut self) {
        self.proofs.clear();
        self.closing_ready = false;
    }
    fn all_proven(&self) -> bool {
        !self.known.is_empty()
            && self
                .known
                .values()
                .all(|run| self.proofs.get(&run.id) == Some(&run.proof()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CiCompletionDecision {
    Accepted,
    Pending,
}

fn parse_ci_run_response(body: &str, key: &QueryKey) -> Result<CiRunObservation, FailureKind> {
    let row: serde_json::Value = serde_json::from_str(body).map_err(|_| FailureKind::Other)?;
    let positive = |name| {
        row.get(name)
            .and_then(serde_json::Value::as_u64)
            .filter(|n| *n > 0)
            .ok_or(FailureKind::Other)
    };
    let head_sha = row
        .get("head_sha")
        .and_then(serde_json::Value::as_str)
        .ok_or(FailureKind::Other)?;
    let head_branch = row
        .get("head_branch")
        .and_then(serde_json::Value::as_str)
        .ok_or(FailureKind::Other)?;
    if head_sha != key.sha40 || head_branch != key.branch {
        return Err(FailureKind::Other);
    }
    let updated_at = row
        .get("updated_at")
        .and_then(serde_json::Value::as_str)
        .ok_or(FailureKind::Other)?;
    DateTime::parse_from_rfc3339(updated_at).map_err(|_| FailureKind::Other)?;
    let status = row
        .get("status")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let conclusion = row
        .get("conclusion")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let run = CiRunObservation {
        id: positive("id")?,
        workflow_id: positive("workflow_id")?,
        run_attempt: positive("run_attempt")?,
        head_sha: head_sha.to_string(),
        head_branch: head_branch.to_string(),
        status,
        conclusion,
        updated_at: updated_at.to_string(),
        pull_requests: row
            .get("pull_requests")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|pr| pr.get("number").and_then(serde_json::Value::as_u64))
            .collect(),
    };
    if run.completed()
        && run
            .conclusion
            .as_deref()
            .is_none_or(|c| c.trim().is_empty())
    {
        return Err(FailureKind::Incomplete);
    }
    Ok(run)
}

fn parse_ci_response(body: &str, branch: &str, sha40: &str) -> Result<CiAnswer, FailureKind> {
    let value: serde_json::Value = serde_json::from_str(body).map_err(|_| FailureKind::Other)?;
    let total = value
        .get("total_count")
        .and_then(serde_json::Value::as_u64)
        .ok_or(FailureKind::Other)?;
    let rows = value
        .get("workflow_runs")
        .and_then(serde_json::Value::as_array)
        .ok_or(FailureKind::Other)?;
    if rows.len() as u64 != total || rows.len() > MAX_CI_KNOWN_RUNS {
        return Err(FailureKind::Incomplete);
    }
    let key = QueryKey {
        nwo: String::new(),
        sha40: sha40.to_string(),
        branch: branch.to_string(),
    };
    let mut observations = Vec::new();
    let mut seen = HashSet::new();
    for row in rows
        .iter()
        .filter(|row| row.get("head_branch").and_then(serde_json::Value::as_str) == Some(branch))
    {
        let run = parse_ci_run_response(&row.to_string(), &key)?;
        if !seen.insert(run.id) {
            return Err(FailureKind::Other);
        }
        observations.push(run);
    }
    let active: Vec<_> = observations.iter().filter(|run| !run.completed()).collect();
    Ok(CiAnswer {
        state: if active.is_empty() {
            CiState::Idle
        } else {
            CiState::Running
        },
        branch_has_runs: !observations.is_empty(),
        run_ids: active.iter().map(|run| run.id).collect(),
        pull_requests: active
            .iter()
            .flat_map(|run| run.pull_requests.iter().copied())
            .collect(),
        observations,
    })
}

/// Latest attempt identities survive omissions and failures. Only positive evidence
/// replaces a high-water mark; proofs bind terminal metadata, not elapsed time.
fn merge_ci_observations(
    state: &mut CiCompletionState,
    observations: &[CiRunObservation],
    from_list: bool,
) -> Result<(), FailureKind> {
    let mut active_seen = false;
    let mut error = state.overflow.then_some(FailureKind::Incomplete);
    for run in observations {
        let mut observation = run.clone();
        if let Some(old) = state.known.get(&run.id) {
            let rejected = if old.workflow_id != run.workflow_id
                || old.head_sha != run.head_sha
                || old.head_branch != run.head_branch
            {
                Some(FailureKind::Other)
            } else if run.run_attempt < old.run_attempt {
                Some(FailureKind::Incomplete)
            } else {
                None
            };
            if let Some(kind) = rejected {
                state.proofs.remove(&run.id);
                state.closing_ready = false;
                // Identity contradictions dominate stale attempts regardless of
                // row order. Absorb compatible rows even when this round fails.
                error = Some(match (error, kind) {
                    (Some(FailureKind::Other), _) | (_, FailureKind::Other) => FailureKind::Other,
                    _ => FailureKind::Incomplete,
                });
                continue;
            }
            // Detail endpoints corroborate completion; the last list supplies
            // the PR projection for a remembered run omitted by a later list.
            if !from_list {
                observation.pull_requests = old.pull_requests.clone();
            }
            if run.run_attempt > old.run_attempt {
                state.cancel_candidate();
            } else if old.proof() != run.proof() || !run.completed() {
                state.proofs.remove(&run.id);
                state.closing_ready = false;
            }
        } else {
            state.cancel_candidate();
            if state.known.len() >= MAX_CI_KNOWN_RUNS {
                state.overflow = true;
                error.get_or_insert(FailureKind::Incomplete);
                continue;
            }
        }
        state.known.insert(run.id, observation.clone());
        if !run.completed() {
            if !active_seen {
                state.active_ids.clear();
                state.active_prs.clear();
                active_seen = true;
            }
            state.active_ids.push(run.id);
            state
                .active_prs
                .extend(observation.pull_requests.iter().copied());
            state.cancel_candidate();
        }
    }
    error.map_or(Ok(()), Err)
}

fn parse_ci_jobs_response(body: &str, run: &CiRunObservation) -> Result<bool, FailureKind> {
    let value: serde_json::Value = serde_json::from_str(body).map_err(|_| FailureKind::Other)?;
    let total = value
        .get("total_count")
        .and_then(serde_json::Value::as_u64)
        .ok_or(FailureKind::Other)?;
    let jobs = value
        .get("jobs")
        .and_then(serde_json::Value::as_array)
        .ok_or(FailureKind::Other)?;
    if jobs.len() as u64 != total || jobs.len() > 100 {
        return Err(FailureKind::Incomplete);
    }
    let mut seen = HashSet::new();
    let mut terminal = true;
    for job in jobs {
        let id = job
            .get("id")
            .and_then(serde_json::Value::as_u64)
            .filter(|n| *n > 0)
            .ok_or(FailureKind::Other)?;
        if !seen.insert(id)
            || job.get("run_id").and_then(serde_json::Value::as_u64) != Some(run.id)
            || job.get("head_sha").and_then(serde_json::Value::as_str)
                != Some(run.head_sha.as_str())
            || job
                .get("run_attempt")
                .is_some_and(|a| a.as_u64() != Some(run.run_attempt))
            || job
                .get("head_branch")
                .is_some_and(|b| b.as_str() != Some(run.head_branch.as_str()))
        {
            return Err(FailureKind::Other);
        }
        if job.get("status").and_then(serde_json::Value::as_str) != Some("completed") {
            terminal = false;
        } else if job
            .get("conclusion")
            .and_then(serde_json::Value::as_str)
            .is_none_or(|c| c.trim().is_empty())
        {
            return Err(FailureKind::Incomplete);
        }
    }
    Ok(terminal)
}

async fn corroborate_ci_run(
    spawner: &GhSpawner,
    gh: &Path,
    key: &QueryKey,
    id: u64,
    state: &mut CiCompletionState,
    calls: &mut u32,
) -> Result<CiState, FailureKind> {
    let expected = state.known[&id].clone();
    *calls += 1;
    let spec = build_gh_command_spec(
        gh,
        GhQuery::CiRun {
            nwo: &key.nwo,
            run_id: id,
        },
    )
    .map_err(|_| FailureKind::Other)?;
    let a = run_query(spawner, spec, |body| parse_ci_run_response(body, key)).await?;
    if a.id != id || a.workflow_id != expected.workflow_id {
        state.proofs.remove(&id);
        return Err(FailureKind::Other);
    }
    merge_ci_observations(state, std::slice::from_ref(&a), false)?;
    if !a.completed() {
        return Ok(CiState::Running);
    }
    if a.run_attempt > expected.run_attempt {
        return Ok(CiState::Idle);
    }
    *calls += 1;
    let spec = build_gh_command_spec(
        gh,
        GhQuery::CiAttemptJobs {
            nwo: &key.nwo,
            run_id: id,
            attempt: a.run_attempt,
        },
    )
    .map_err(|_| FailureKind::Other)?;
    let terminal = run_query(spawner, spec, |body| parse_ci_jobs_response(body, &a)).await?;
    if !terminal {
        state.active_ids = vec![id];
        state.active_prs = state.known[&id].pull_requests.clone();
        state.cancel_candidate();
        return Ok(CiState::Running);
    }
    *calls += 1;
    let spec = build_gh_command_spec(
        gh,
        GhQuery::CiRun {
            nwo: &key.nwo,
            run_id: id,
        },
    )
    .map_err(|_| FailureKind::Other)?;
    let c = run_query(spawner, spec, |body| parse_ci_run_response(body, key)).await?;
    if c.id != id || c.workflow_id != expected.workflow_id {
        state.proofs.remove(&id);
        return Err(FailureKind::Other);
    }
    merge_ci_observations(state, std::slice::from_ref(&c), false)?;
    if !c.completed() {
        return Ok(CiState::Running);
    }
    if a.proof() == c.proof() {
        state.proofs.insert(id, c.proof());
    }
    Ok(CiState::Idle)
}

/// The staleness mapping is unchanged; the extra flag reports whether GitHub
/// said `identical`, which is the same statement the CI suppression rule needs.
/// `ahead_by` is read with the same shape as `behind_by` and is NOT part of the
/// classification: an absent or out-of-range value is `None`, never an error,
/// because #2141 fails open on it.
fn parse_compare_response(body: &str) -> CompareAnswer {
    let value: serde_json::Value = serde_json::from_str(body).map_err(|_| FailureKind::Other)?;
    let status = value
        .get("status")
        .and_then(serde_json::Value::as_str)
        .ok_or(FailureKind::Other)?;
    let behind_by = value
        .get("behind_by")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok());
    let ahead_by = value
        .get("ahead_by")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok());
    let identical = status == "identical";
    match (status, behind_by) {
        ("identical" | "ahead", Some(0)) => {
            Ok((StalenessState::Current, None, ahead_by, identical))
        }
        ("behind" | "diverged", Some(behind)) if behind > 0 => {
            Ok((StalenessState::Stale, Some(behind), ahead_by, identical))
        }
        _ => Err(FailureKind::Other),
    }
}

/// `github.com` only, in both SSH and HTTPS spellings, with a trailing `.git`
/// stripped. Anything else yields no key and the path reads `Unknown`.
fn parse_github_nwo(origin: &str) -> Option<String> {
    const HOST: &str = "github.com";
    let origin = origin.trim();
    if origin.is_empty() {
        return None;
    }

    let (host, path) = if let Some((_scheme, rest)) = origin.split_once("://") {
        let rest = rest.split_once('@').map_or(rest, |(_, after)| after);
        rest.split_once('/')?
    } else if let Some((_user, rest)) = origin.split_once('@') {
        rest.split_once(':')?
    } else {
        return None;
    };

    if !host.eq_ignore_ascii_case(HOST) {
        return None;
    }

    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut segments = path.split('/');
    let owner = segments.next()?;
    let repo = segments.next()?;
    if segments.next().is_some() || owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}

fn clamp_interval_secs(name: &str, raw: u64) -> u64 {
    let clamped = raw.clamp(INTERVAL_FLOOR_SECS, INTERVAL_CEILING_SECS);
    if clamped != raw {
        log::debug!("[RemoteSweeper] {name} {raw} clamped to {clamped}");
    }
    clamped
}

fn due(now: Instant, next_due: Option<Instant>) -> bool {
    match next_due {
        Some(due_at) => now >= due_at,
        None => true,
    }
}

fn ci_base_interval(confirmed: Option<CiState>, dial: Duration) -> Duration {
    match confirmed {
        Some(CiState::Running) => Duration::from_secs(CI_RUNNING_INTERVAL_SECS),
        _ => dial,
    }
}

/// Backoff is per key and per axis. Doubling is the default; exactly two named
/// conditions jump straight to the cap (rate limiting and not authenticated),
/// because both are not transient and hammering after being told no is how an
/// annoyance becomes an account block. The `max(base, ..)` is load-bearing: a
/// dial clamped to 3600 s is above the 900 s cap, and without it a failing key
/// would re-query FASTER than a healthy one.
fn failure_interval(kind: FailureKind, base: Duration, current: Option<Duration>) -> Duration {
    match kind {
        FailureKind::RateLimited | FailureKind::NotAuthenticated => BACKOFF_CAP.max(base),
        FailureKind::SecondaryRateLimited => current
            .unwrap_or(SECONDARY_BACKOFF_BASE / 2)
            .saturating_mul(2)
            .min(BACKOFF_CAP)
            .max(base),
        FailureKind::Timeout | FailureKind::Incomplete | FailureKind::Other => current
            .unwrap_or(base)
            .saturating_mul(2)
            .min(BACKOFF_CAP)
            .max(base),
    }
}

/// The applied wait is `base + base * jitter()`, uniform over `[base, 2*base)`.
/// The offset is TRUNCATED to whole nanoseconds, not rounded: `Duration::mul_f64`
/// rounds, so a fraction just under 1.0 returns `base` itself and the range
/// would be closed at both ends. The clamp guards a misbehaving seam.
fn jitter_offset(base: Duration, fraction: f64) -> Duration {
    Duration::from_nanos((base.as_nanos() as f64 * fraction.clamp(0.0, 1.0)) as u64)
}

fn axis_label(axis: Axis) -> &'static str {
    match axis {
        Axis::Ci => "CI",
        Axis::Staleness => "staleness",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Axis {
    Ci,
    Staleness,
}

/// The query unit: one network call per distinct `(nwo, head_sha, branch)` per
/// axis. The branch belongs to the identity because one commit can carry runs
/// from several branches, and only the repo's current branch may answer.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct QueryKey {
    nwo: String,
    sha40: String,
    branch: String,
}

struct CiAxis {
    /// Last published answer. Set to `Unknown` on every failure; a round that is
    /// merely not due leaves it untouched.
    chip: CiState,
    /// Last CONFIRMED answer (`Idle`/`Running`). `Unknown` never overwrites it,
    /// which is what makes the transition stream ignore transient failures.
    confirmed: Option<CiState>,
    last_confirmed_at: Option<DateTime<Local>>,
    next_due: Option<Instant>,
    failure_interval: Option<Duration>,
    /// #2473: the published answer's in-progress run ids / PR numbers (kept
    /// only while `chip` is `Running`) and the failure behind an `Unknown`
    /// chip. A round that is not due leaves them untouched, like `chip`.
    run_ids: Vec<u64>,
    pull_requests: Vec<u64>,
    failure: Option<FailureKind>,
    completion: CiCompletionState,
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
struct StalenessCarry {
    confirmed: StalenessState,
    last_confirmed_at: Option<DateTime<Local>>,
}

struct StalenessAxis {
    chip: StalenessState,
    confirmed: Option<StalenessState>,
    behind_by: Option<u32>,
    last_confirmed_at: Option<DateTime<Local>>,
    next_due: Option<Instant>,
    failure_interval: Option<Duration>,
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
struct QueryState {
    ci: CiAxis,
    staleness: StalenessAxis,
}

/// The named, one-time construction state of the snapshot producer. A `None`
/// destination is a wiring fact captured by `with_capacity` together with the
/// single diagnostic logged there: rounds read this state, so a directory that
/// appears later cannot start writes and a missing one never warns again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SnapshotPersistence {
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
struct SnapshotWarningLimiter {
    last_admitted: Option<Instant>,
}

impl SnapshotWarningLimiter {
    fn admit(&mut self, now: Instant) -> bool {
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

    fn reset(&mut self) {
        self.last_admitted = None;
    }
}

struct SweeperState {
    gh_path: Option<PathBuf>,
    keys: HashMap<QueryKey, QueryState>,
    /// Base-branch labels resolved once per `nwo` for the life of the process.
    base_branches: HashMap<String, String>,
    warned_base_branch: HashSet<String>,
    warned_failures: HashSet<(QueryKey, Axis)>,
    warned_transition_drops: HashSet<String>,
    /// Once per PROCESS, not per key: a closed channel is a Phase B wiring fact.
    closed_warned: bool,
    last_payload: Option<RemoteActivityPayload>,
    /// Token bucket for sweeper-issued `gh` calls. Every `gh` call the round can
    /// issue is reserved from this bucket BEFORE it is issued, so the ceiling is
    /// a guarantee, not an estimate.
    budget_tokens: f64,
    budget_refilled_at: Option<Instant>,
    /// While set and not elapsed, the round issues NO `gh` call at all.
    throttled_until: Option<Instant>,
    /// One-warning-per-window limiter for snapshot write failures.
    snapshot_warnings: SnapshotWarningLimiter,
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

struct PathFacts {
    path: String,
    room_dirs: Vec<String>,
    branch: Option<String>,
    head_sha: Option<String>,
    nwo: Option<String>,
}

#[derive(Clone, Copy)]
struct KeyPlan {
    ci: bool,
    staleness: bool,
}

#[derive(Default)]
struct KeyOutcome {
    ci: Option<Result<CiState, FailureKind>>,
    /// Set when the CI answer was suppressed: the branch is the resolved default
    /// branch, or it is identical to the default branch (#2126). Carried beside
    /// `ci` because `CiState` has no `Suppressed` value.
    ci_suppressed: bool,
    /// Set only when the identity compare was issued as an EXTRA `gh` call, so
    /// the reservation made for it is refunded when it was never made.
    ci_identity_call: bool,
    staleness: Option<CompareAnswer>,
    /// #2473: `(run_ids, pull_requests)` of a non-suppressed `Ok` answer; empty
    /// otherwise.
    ci_runs: (Vec<u64>, Vec<u64>),
    ci_completion: Option<CiCompletionState>,
    ci_decision: Option<CiCompletionDecision>,
    ci_corroboration_calls: u32,
}

struct RoundCtx<'a> {
    facts: &'a [PathFacts],
    now: Instant,
    wall: DateTime<Local>,
    ci_dial: Duration,
    staleness_dial: Duration,
}

struct TransitionMeta {
    kind: TransitionKind,
    behind_by: Option<u32>,
    base_branch: String,
    last_confirmed_at: Option<DateTime<Local>>,
}

/// The single global producer of remote activity. Takes `SettingsState` and an
/// injected emitter rather than an `AppHandle`: it never emits anything itself,
/// so it must not acquire a Tauri transport dependency it does not need.
pub(crate) struct RemoteSweeper {
    session_manager: Arc<tokio::sync::RwLock<SessionManager>>,
    settings: SettingsState,
    emit: Mutex<Emitter>,
    probe: GhProbe,
    spawner: GhSpawner,
    local_git_runner: LocalGitRunner,
    jitter: JitterSource,
    transitions: mpsc::Sender<RemoteTransition>,
    /// Named one-time construction state; see [`SnapshotPersistence`].
    snapshot_persistence: SnapshotPersistence,
    /// `Some` iff `snapshot_persistence` is `Enabled`.
    snapshot_dir: Option<PathBuf>,
    publication_clock: PublicationClock,
    state: Mutex<SweeperState>,
}

impl RemoteSweeper {
    pub(crate) fn new(
        session_manager: Arc<tokio::sync::RwLock<SessionManager>>,
        settings: SettingsState,
        emit: Emitter,
        seams: RemoteSweeperSeams,
    ) -> (Arc<Self>, mpsc::Receiver<RemoteTransition>) {
        Self::with_capacity(
            session_manager,
            settings,
            emit,
            seams,
            TRANSITION_QUEUE_CAPACITY,
        )
    }

    fn with_capacity(
        session_manager: Arc<tokio::sync::RwLock<SessionManager>>,
        settings: SettingsState,
        emit: Emitter,
        seams: RemoteSweeperSeams,
        capacity: usize,
    ) -> (Arc<Self>, mpsc::Receiver<RemoteTransition>) {
        let RemoteSweeperSeams {
            probe,
            spawner,
            local_git,
            jitter,
            snapshot_dir,
            publication_clock,
        } = seams;
        // The one-time `None`-directory state: computed, named and diagnosed
        // exactly here. Rounds never re-check the directory, so this is the only
        // place the condition can be observed or logged.
        let snapshot_persistence = match &snapshot_dir {
            Some(_) => SnapshotPersistence::Enabled,
            None => {
                log::warn!(
                    "[RemoteSweeper] no snapshot directory was wired at construction; remote \
                        activity snapshots will not be published for this process"
                );
                SnapshotPersistence::DisabledMissingAtConstruction
            }
        };
        let (transitions, receiver) = mpsc::channel(capacity);
        let sweeper = Arc::new(Self {
            session_manager,
            settings,
            emit: Mutex::new(emit),
            probe,
            spawner,
            local_git_runner: local_git,
            jitter,
            transitions,
            snapshot_persistence,
            snapshot_dir,
            publication_clock,
            state: Mutex::new(SweeperState::default()),
        });
        (sweeper, receiver)
    }

    /// Production seams: the real `which`, the real `gh` spawner and the real
    /// local `git` runner. Kept in one factory so `lib.rs` names no seam type.
    /// `snapshot_dir` is the instance config directory (or `None` when the host
    /// has none); the publication clock is the real wall clock, read at
    /// publication time and never at round start.
    pub(crate) fn production_seams(snapshot_dir: Option<PathBuf>) -> RemoteSweeperSeams {
        RemoteSweeperSeams {
            probe: Arc::new(|| which::which("gh").ok()),
            spawner: Arc::new(|spec| Box::pin(spawn_gh(spec))),
            local_git: Arc::new(|path, args| Box::pin(run_local_git(path, args))),
            jitter: Arc::new(|| {
                let mut bytes = [0u8; 8];
                match getrandom::fill(&mut bytes) {
                    Ok(()) => (u64::from_le_bytes(bytes) >> 11) as f64 / (1u64 << 53) as f64,
                    // No entropy is not a reason to stop sweeping; mid-range is
                    // a safe, still-legal wait.
                    Err(_) => 0.5,
                }
            }),
            snapshot_dir,
            publication_clock: Arc::new(Utc::now),
        }
    }

    /// A dedicated thread owning its own runtime, exactly like `GitSweeper::start`.
    /// The startup path does no probe, no process, no I/O and no network on the
    /// caller's thread: the probe runs as the thread's first action.
    ///
    /// Returns `None` exactly when both axes are disabled, so a fully disabled
    /// feature costs no thread. The two enable dials are read once, synchronously:
    /// this runs on the setup thread before any runtime exists for this sweeper
    /// (`lib.rs` calls it beside `GitSweeper::start`), and the flags decide whether
    /// a thread exists at all.
    pub(crate) fn start(
        self: &Arc<Self>,
        shutdown: ShutdownSignal,
    ) -> Option<std::thread::JoinHandle<()>> {
        let enabled = {
            let settings = match self.settings.blocking_current_read() {
                Ok(settings) => settings,
                Err(_) => {
                    log::debug!("[RemoteSweeper] settings unavailable; not starting");
                    return None;
                }
            };
            settings.ci_activity_enabled || settings.branch_staleness_enabled
        };
        if !enabled {
            return None;
        }

        let sweeper = Arc::clone(self);
        Some(std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to create tokio runtime for RemoteSweeper");
            runtime.block_on(async move {
                // A `PATH` read, not a spawn. `None` means one log line and the
                // thread returns: it never loops and never probes again.
                let Some(gh) = (sweeper.probe)() else {
                    log::info!(
                        "[RemoteSweeper] gh not found on PATH; remote activity stays unknown"
                    );
                    return;
                };
                sweeper.lock_state().gh_path = Some(gh);

                loop {
                    tokio::select! {
                        biased;
                        _ = shutdown.token().cancelled() => break,
                        _ = tokio::time::sleep(ROUND_INTERVAL) => {}
                    }
                    sweeper
                        .run_round(std::time::Instant::now(), chrono::Local::now())
                        .await;
                }
                log::info!("[RemoteSweeper] Shutdown signal received, stopping");
            });
        }))
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, SweeperState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The work list, built exactly as `GitSweeper` builds it: union of discovery
    /// and live sessions, deduped by exact path string, archived projects dropped,
    /// sorted. `normalize_project_roots` and `build_work_list` canonicalize, so
    /// when `archived_roots` is non-empty both run together inside one blocking
    /// task (INV-1); the empty case short-circuits with zero syscalls.
    async fn work_list(&self, archived: Vec<String>) -> Vec<String> {
        let sessions: Vec<String> = {
            let manager = self.session_manager.read().await;
            manager.get_sessions_repos().await
        }
        .into_iter()
        .flat_map(|(_, repos, _)| repos.into_iter().map(|repo| repo.source_path))
        .collect();

        let discovery = crate::pty::git_watcher::discovery_repo_paths();

        if archived.is_empty() {
            crate::pty::git_watcher::build_work_list(discovery, sessions, &[])
        } else {
            let (discovery_fallback, sessions_fallback) = (discovery.clone(), sessions.clone());
            match tokio::task::spawn_blocking(move || {
                let roots = crate::config::sessions_persistence::normalize_project_roots(&archived);
                crate::pty::git_watcher::build_work_list(discovery, sessions, &roots)
            })
            .await
            {
                Ok(paths) => paths,
                Err(e) => {
                    log::warn!(
                        "[RemoteSweeper] work-list build failed ({e}); sweeping unfiltered this \
                            round"
                    );
                    crate::pty::git_watcher::build_work_list(
                        discovery_fallback,
                        sessions_fallback,
                        &[],
                    )
                }
            }
        }
    }

    /// One pass over the work list: local facts, remote questions, snapshot,
    /// payload, transitions. `now` drives every due time and `wall` every
    /// timestamp, so a test advances an hour by passing an advanced `now` and no
    /// test ever sleeps.
    async fn run_round(&self, now: Instant, wall: DateTime<Local>) {
        let (ci_enabled, staleness_enabled, ci_dial, staleness_dial, archived) = {
            let settings = match self.settings.current_read().await {
                Ok(settings) => settings,
                Err(_) => {
                    log::debug!("[RemoteSweeper] settings unavailable; skipping round");
                    return;
                }
            };
            (
                settings.ci_activity_enabled,
                settings.branch_staleness_enabled,
                Duration::from_secs(clamp_interval_secs(
                    "ciSweepMinIntervalSecs",
                    settings.ci_sweep_min_interval_secs,
                )),
                Duration::from_secs(clamp_interval_secs(
                    "branchStalenessIntervalSecs",
                    settings.branch_staleness_interval_secs,
                )),
                settings.archived_project_paths.clone(),
            )
        };

        let paths = self.work_list(archived).await;
        let rooms = room_map();

        let mut facts: Vec<PathFacts> = Vec::with_capacity(paths.len());
        for path in &paths {
            facts.push(self.read_local_facts(path, &rooms).await);
        }

        let mut groups: BTreeMap<QueryKey, Vec<usize>> = BTreeMap::new();
        for (index, fact) in facts.iter().enumerate() {
            let (Some(nwo), Some(head_sha), Some(branch)) =
                (&fact.nwo, &fact.head_sha, &fact.branch)
            else {
                continue;
            };
            groups
                .entry(QueryKey {
                    nwo: nwo.clone(),
                    sha40: head_sha.clone(),
                    branch: branch.clone(),
                })
                .or_default()
                .push(index);
        }

        // #2141: retire keys no path maps to any more, carrying the STALENESS
        // axis to a live key on the same `(nwo, branch)` first. A local commit
        // moves `head_sha`, so it retires the old key and creates a new one that
        // would otherwise start at `confirmed = None`; `None -> Stale` is not the
        // edge the notice fires on, so the branch would never be told.
        //
        // This runs BEFORE the apply loop, which is the hard requirement: the old
        // retirement site sat after it, so the commit's own round decided with
        // `None` and recorded `Stale`, and without a rebase no edge ever occurred
        // again. Running it before `plans` as well is safe, not required: an
        // absent key is already due, and a seeded one is due only because
        // `next_due` is NOT carried (see below).
        //
        // Only the staleness axis moves. A new commit has genuinely unknown CI,
        // and seeding it would assert a run state nobody observed.
        //
        // The carry lives inside ONE round and is not a guarantee: if the old key
        // is retired with no live key on its `(nwo, branch)` to receive it — the
        // path left the work list, the last room closed, `.git` went unreadable,
        // the repo was archived — the memory is gone, and the next observation of
        // that branch starts at `None`, which is not an edge. That case is silent
        // today too, so nothing regresses, but do not read this block as a
        // promise that survives a path dropping out of a round.
        {
            let mut state = self.lock_state();
            // Donors are computed BEFORE anything is deleted, and only from keys
            // this round retires. A live key is never a donor: two paths on one
            // branch at different commits (worktrees, or two clones on the same
            // branch) must not seed each other.
            let mut donors: HashMap<(String, String), (QueryKey, StalenessCarry)> = HashMap::new();
            for (key, entry) in state
                .keys
                .iter()
                .filter(|(key, _)| !groups.contains_key(*key))
            {
                let Some(confirmed) = entry.staleness.confirmed else {
                    continue;
                };
                let carry = StalenessCarry {
                    confirmed,
                    last_confirmed_at: entry.staleness.last_confirmed_at,
                };
                let id = (key.nwo.clone(), key.branch.clone());
                match donors.get(&id) {
                    // Most recently confirmed wins; `None` never beats `Some`; a
                    // remaining tie goes to the greater `sha40`, so the choice is
                    // deterministic rather than hash-order dependent.
                    Some((held_key, held))
                        if (held.last_confirmed_at, &held_key.sha40)
                            >= (carry.last_confirmed_at, &key.sha40) => {}
                    _ => {
                        donors.insert(id, (key.clone(), carry));
                    }
                }
            }

            state.keys.retain(|key, _| groups.contains_key(key));

            if !donors.is_empty() {
                for key in groups.keys() {
                    let Some((_, carry)) = donors.get(&(key.nwo.clone(), key.branch.clone()))
                    else {
                        continue;
                    };
                    let entry = state.keys.entry(key.clone()).or_default();
                    // A key that already has a confirmed answer of its own keeps
                    // it. `chip`, `behind_by`, `next_due` and `failure_interval`
                    // are never carried: the new commit is un-queried, and a
                    // carried `next_due` would leave it not due in the very round
                    // that has to decide.
                    if entry.staleness.confirmed.is_none() {
                        entry.staleness.confirmed = Some(carry.confirmed);
                        entry.staleness.last_confirmed_at = carry.last_confirmed_at;
                    }
                }
            }
        }

        let gh_path = self.lock_state().gh_path.clone();

        let mut plans: Vec<(QueryKey, KeyPlan)> = Vec::with_capacity(groups.len());
        {
            let state = self.lock_state();
            for key in groups.keys() {
                let ci_due = match state.keys.get(key) {
                    Some(entry) => due(now, entry.ci.next_due),
                    None => true,
                };
                let staleness_due = match state.keys.get(key) {
                    Some(entry) => due(now, entry.staleness.next_due),
                    None => true,
                };
                plans.push((
                    key.clone(),
                    KeyPlan {
                        ci: ci_enabled && gh_path.is_some() && ci_due,
                        staleness: staleness_enabled && gh_path.is_some() && staleness_due,
                    },
                ));
            }
        }

        // Refill the global `gh` budget before any call is considered, then
        // read the gate. A gated round issues NO `gh` call: no resolution, no
        // query. Nothing else about the round changes.
        let throttled = {
            let mut state = self.lock_state();
            let elapsed = state
                .budget_refilled_at
                .map(|last| now.saturating_duration_since(last))
                .unwrap_or_default();
            state.budget_tokens = (state.budget_tokens
                + elapsed.as_secs_f64() * MAX_GH_CALLS_PER_MINUTE / 60.0)
                .min(GH_BUDGET_BURST);
            state.budget_refilled_at = Some(now);
            match state.throttled_until {
                Some(until) if now < until => true,
                _ => {
                    state.throttled_until = None;
                    false
                }
            }
        };

        // Priority order, built once: a key the user is watching (`Running`)
        // first, then the least recently confirmed. A missing entry or a `None`
        // timestamp sorts first (never checked, so most owed) and the trailing
        // key makes the order total.
        let mut due_keys: Vec<(QueryKey, KeyPlan)> = plans
            .iter()
            .filter(|(_, plan)| plan.ci || plan.staleness)
            .cloned()
            .collect();
        {
            let state = self.lock_state();
            due_keys.sort_by_cached_key(|(key, _)| {
                let entry = state.keys.get(key);
                (
                    if entry.map(|e| e.ci.confirmed) == Some(Some(CiState::Running)) {
                        0u8
                    } else {
                        1u8
                    },
                    entry.and_then(|e| e.ci.last_confirmed_at),
                    entry.and_then(|e| e.staleness.last_confirmed_at),
                    key.clone(),
                )
            });
        }

        // Reserve-before-issue: every `gh` call this round can make - the
        // per-`nwo` base-branch resolution, the axis queries and the extra
        // identity compare - is subtracted from the bucket BEFORE it is issued.
        // A key that cannot pay is dropped from the round: no outcome, no
        // failure, chip and `next_due` untouched, first in line next round.
        //
        // The label chain runs on either axis, once per distinct nwo, and is
        // cached for the process lifetime: CI needs the name to decide the
        // identical-to-default suppression, staleness to render `%BASE%`.
        let mut pending_base: Vec<(String, String)> = Vec::new();
        let mut admitted: Vec<(QueryKey, KeyPlan)> = Vec::new();
        let mut completion_reserved: HashSet<QueryKey> = HashSet::new();
        let mut identity_reserved: HashSet<QueryKey> = HashSet::new();
        let mut unresolvable_nwos: HashSet<String> = HashSet::new();
        if gh_path.is_some() && !throttled {
            let mut state = self.lock_state();
            for (key, plan) in &due_keys {
                // A key is never queried with a base label a resolution would
                // have supplied, so suppression decisions are unchanged.
                if unresolvable_nwos.contains(&key.nwo) {
                    continue;
                }
                let resolved = state.base_branches.contains_key(&key.nwo)
                    || pending_base.iter().any(|(nwo, _)| nwo == &key.nwo);
                // An unresolved label must assume the worst case; step 7 refunds
                // the reservation when the compare is not issued.
                let identity = u32::from(
                    plan.ci
                        && !plan.staleness
                        && state.base_branches.get(&key.nwo).is_none_or(|label| {
                            label.as_str() != DEFAULT_BRANCH_LABEL && *label != key.branch
                        }),
                );
                let corroboration = plan.ci
                    && state.keys.get(key).is_some_and(|entry| {
                        entry.ci.confirmed == Some(CiState::Running)
                            && !entry.ci.completion.known.is_empty()
                    });
                let cost = u32::from(plan.staleness)
                    + u32::from(plan.ci)
                    + identity
                    + if corroboration { 3 } else { 0 };
                let needed = f64::from(cost) + if resolved { 0.0 } else { 1.0 };
                if state.budget_tokens < needed {
                    if !resolved {
                        unresolvable_nwos.insert(key.nwo.clone());
                    }
                    continue;
                }
                if !resolved {
                    let Some(index) = groups.get(key).and_then(|group| group.first()) else {
                        continue;
                    };
                    pending_base.push((key.nwo.clone(), facts[*index].path.clone()));
                }
                state.budget_tokens -= needed;
                if identity == 1 {
                    identity_reserved.insert(key.clone());
                }
                if corroboration {
                    completion_reserved.insert(key.clone());
                }
                admitted.push((key.clone(), *plan));
            }
        }
        if let Some(gh) = gh_path.as_deref() {
            for (nwo, path) in pending_base {
                let label = self.resolve_base_branch(gh, &nwo, &path).await;
                self.lock_state().base_branches.insert(nwo, label);
            }
        }

        let mut outcomes: Vec<(QueryKey, KeyOutcome)> = Vec::new();
        if let Some(gh) = gh_path.as_deref() {
            let query_keys: Vec<(QueryKey, KeyPlan)> = admitted;
            let defaults: HashMap<String, Option<String>> = {
                let state = self.lock_state();
                query_keys
                    .iter()
                    .map(|(key, _)| {
                        (
                            key.nwo.clone(),
                            state
                                .base_branches
                                .get(&key.nwo)
                                .filter(|label| label.as_str() != DEFAULT_BRANCH_LABEL)
                                .cloned(),
                        )
                    })
                    .collect()
            };
            let completion_inputs: HashMap<_, _> = {
                let state = self.lock_state();
                query_keys
                    .iter()
                    .map(|(key, _)| {
                        (
                            key.clone(),
                            state
                                .keys
                                .get(key)
                                .map(|entry| (entry.ci.completion.clone(), entry.ci.confirmed))
                                .unwrap_or_default(),
                        )
                    })
                    .collect()
            };
            let spawner = Arc::clone(&self.spawner);
            let gh = gh.to_path_buf();
            outcomes = futures::stream::iter(query_keys.into_iter().map(|(key, plan)| {
                let spawner = Arc::clone(&spawner);
                let gh = gh.clone();
                let default_branch = defaults.get(&key.nwo).cloned().flatten();
                let (completion, confirmed) =
                    completion_inputs.get(&key).cloned().unwrap_or_default();
                let reserved = completion_reserved.contains(&key);
                async move {
                    let outcome = query_key(
                        &spawner,
                        &gh,
                        &key,
                        plan,
                        default_branch.as_deref(),
                        (completion, confirmed),
                        reserved,
                    )
                    .await;
                    (key, outcome)
                }
            }))
            .buffer_unordered(QUERY_CONCURRENCY)
            .collect()
            .await;
        }

        // A reservation returns a call that was never made; it never forgives
        // one that was.
        if !identity_reserved.is_empty() {
            let refund = outcomes
                .iter()
                .filter(|(key, outcome)| {
                    identity_reserved.contains(key) && !outcome.ci_identity_call
                })
                .count() as f64;
            if refund > 0.0 {
                let mut state = self.lock_state();
                state.budget_tokens = (state.budget_tokens + refund).min(GH_BUDGET_BURST);
            }
        }

        let refund: f64 = outcomes
            .iter()
            .filter(|(key, _)| completion_reserved.contains(key))
            .map(|(_, outcome)| f64::from(3 - outcome.ci_corroboration_calls))
            .sum();
        if refund > 0.0 {
            let mut state = self.lock_state();
            state.budget_tokens = (state.budget_tokens + refund).min(GH_BUDGET_BURST);
        }

        let mut secondary_limited = false;
        let mut primary_limited = false;
        for (_, outcome) in &outcomes {
            let mut note = |kind: FailureKind| match kind {
                FailureKind::SecondaryRateLimited => secondary_limited = true,
                FailureKind::RateLimited => primary_limited = true,
                _ => {}
            };
            if let Some(Err(kind)) = &outcome.ci {
                note(*kind);
            }
            if let Some(Err(kind)) = &outcome.staleness {
                note(*kind);
            }
        }

        let ctx = RoundCtx {
            facts: &facts,
            now,
            wall,
            ci_dial,
            staleness_dial,
        };
        for (key, outcome) in outcomes {
            let indices = groups.get(&key).map(Vec::as_slice).unwrap_or(&[]);
            let staleness = outcome.staleness;
            self.apply_ci(&key, outcome, indices, &ctx);
            self.apply_staleness(&key, staleness, indices, &ctx);
        }

        // One gate per round, never shortened: a limit is an account-wide
        // signal, so every key waits, not just the ones that discovered it.
        if secondary_limited || primary_limited {
            let base = if primary_limited {
                BACKOFF_CAP
            } else {
                SECONDARY_BACKOFF_BASE
            };
            let candidate = now + base + jitter_offset(base, (self.jitter)());
            let mut state = self.lock_state();
            state.throttled_until = Some(match state.throttled_until {
                Some(until) if until > candidate => until,
                _ => candidate,
            });
        }

        let live: HashSet<String> = paths.iter().cloned().collect();
        let mut activities: Vec<RemoteActivity> = Vec::with_capacity(facts.len());
        {
            let state = self.lock_state();
            for fact in &facts {
                activities.push(activity_for(&state, fact));
            }
        }
        {
            let mut snapshot = remote_activity_snapshot()
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            for (fact, activity) in facts.iter().zip(&activities) {
                snapshot.insert(fact.path.clone(), activity.clone());
            }
            // GC runs UNCONDITIONALLY, including on an EMPTY work list: an empty
            // list means every repo went away, and the correct chip state for a
            // repo that went away is `Unknown`, which is what an emptied snapshot
            // produces. A `if !paths.is_empty()` guard would leave the last
            // colours on screen forever after the last room closes.
            snapshot.retain(|path, _| live.contains(path));
        }

        let mut payload = RemoteActivityPayload {
            repo_paths: Vec::with_capacity(facts.len()),
            ci_states: Vec::with_capacity(facts.len()),
            staleness_states: Vec::with_capacity(facts.len()),
            behind_by: Vec::with_capacity(facts.len()),
        };
        for (fact, activity) in facts.iter().zip(activities) {
            payload.repo_paths.push(fact.path.clone());
            payload.ci_states.push(activity.ci);
            payload.staleness_states.push(activity.staleness);
            payload.behind_by.push(activity.behind_by);
        }

        let changed = self.lock_state().last_payload.as_ref() != Some(&payload);
        if changed {
            {
                let mut emit = self.emit.lock().unwrap_or_else(|e| e.into_inner());
                (emit)(&payload);
            }
            self.lock_state().last_payload = Some(payload);
        }

        // #2374 - publish the neutral whole-file snapshot. The entries are
        // cloned from the same post-GC snapshot the payload was built from, and
        // this runs AFTER transitions were processed and the payload was emitted,
        // so persistence can never delay or suppress either. The publication
        // instant is read HERE, not taken from the round-start `wall`: a round
        // longer than the freshness window must not publish bytes that are
        // already stale. The blocking serialization/write is offloaded, so a
        // slow disk cannot stall the sweeper's async worker.
        if self.snapshot_persistence == SnapshotPersistence::Enabled {
            if let Some(snapshot_dir) = &self.snapshot_dir {
                let path = snapshot_dir.join(REMOTE_ACTIVITY_SNAPSHOT_FILE_NAME);
                let entries: Vec<(String, PersistedRepoCi)> = {
                    let snapshot = remote_activity_snapshot()
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    snapshot
                        .iter()
                        .map(|(path, activity)| (path.clone(), persisted_repo_ci(activity)))
                        .collect()
                };
                let generated_at = (self.publication_clock)();
                let write = tokio::task::spawn_blocking(move || {
                    crate::config::remote_activity_cache::write_snapshot(
                        &path,
                        generated_at,
                        &entries,
                    )
                })
                .await;
                match write {
                    Ok(Ok(())) => self.lock_state().snapshot_warnings.reset(),
                    Ok(Err(error)) => self.warn_snapshot_failure(now, error),
                    Err(error) => self
                        .warn_snapshot_failure(now, format!("snapshot write task failed: {error}")),
                }
            }
        }

        log::log!(
            ROUND_LOG_LEVEL,
            "[RemoteSweeper] round: {} path(s), {} key(s){}",
            paths.len(),
            groups.len(),
            if throttled { ", throttled" } else { "" }
        );
    }

    /// One warning per freshness window while the destination stays broken; a
    /// successful write resets the limiter, so a later failure warns at once.
    fn warn_snapshot_failure(&self, now: Instant, error: String) {
        if self.lock_state().snapshot_warnings.admit(now) {
            log::warn!("[RemoteSweeper] remote activity snapshot write failed: {error}");
        }
    }

    /// Gate 1 (`.git` metadata), then at most two local git reads for the SHA and
    /// the origin URL. The branch comes from the published git snapshot, so the
    /// branch costs zero new git.
    async fn read_local_facts(
        &self,
        path: &str,
        rooms: &HashMap<String, Vec<String>>,
    ) -> PathFacts {
        let room_dirs = rooms.get(path).cloned().unwrap_or_default();
        let branch =
            crate::pty::git_watcher::read_git_status(path).and_then(|status| status.branch);

        if tokio::fs::metadata(Path::new(path).join(".git"))
            .await
            .is_err()
        {
            return PathFacts {
                path: path.to_string(),
                room_dirs,
                branch,
                head_sha: None,
                nwo: None,
            };
        }

        let head_sha = match self.local_git(path, &["rev-parse", "HEAD"]).await {
            Ok(stdout) => {
                let trimmed = stdout.trim().to_string();
                if is_sha40(&trimmed) {
                    Some(trimmed)
                } else {
                    None
                }
            }
            Err(_) => None,
        };
        let nwo = match self
            .local_git(path, &["config", "--get", "remote.origin.url"])
            .await
        {
            Ok(stdout) => parse_github_nwo(stdout.trim()),
            Err(_) => None,
        };

        PathFacts {
            path: path.to_string(),
            room_dirs,
            branch,
            head_sha,
            nwo,
        }
    }

    async fn local_git(&self, path: &str, args: &[&str]) -> Result<String, String> {
        (self.local_git_runner)(
            path.to_string(),
            args.iter().map(|arg| arg.to_string()).collect(),
        )
        .await
    }

    /// Step 1 `gh api repos/<nwo>` (the same resolution the compare query's `HEAD`
    /// performs server-side, so label and answer cannot disagree), step 2 the
    /// local clone's `refs/remotes/origin/HEAD` (no network, no auth, but stale if
    /// the default branch was renamed), step 3 the literal label.
    async fn resolve_base_branch(&self, gh: &Path, nwo: &str, path: &str) -> String {
        if let Some(branch) = self.fetch_default_branch(gh, nwo).await {
            return branch;
        }
        let first = self.lock_state().warned_base_branch.insert(nwo.to_string());
        if first {
            log::warn!("[RemoteSweeper] base branch resolution failed for {nwo}; falling back");
        }
        if let Some(branch) = self.local_default_branch(path).await {
            return branch;
        }
        DEFAULT_BRANCH_LABEL.to_string()
    }

    async fn fetch_default_branch(&self, gh: &Path, nwo: &str) -> Option<String> {
        let spec = build_gh_command_spec(gh, GhQuery::RepoInfo { nwo }).ok()?;
        let output = (self.spawner)(spec).await.ok()?;
        if !output.success {
            return None;
        }
        // The WHOLE body is parsed with `serde_json`, so the body a test feeds is
        // byte-for-byte the shape production receives.
        let value: serde_json::Value = serde_json::from_str(&output.stdout).ok()?;
        let branch = value
            .get("default_branch")
            .and_then(serde_json::Value::as_str)?
            .to_string();
        if branch.is_empty() {
            None
        } else {
            Some(branch)
        }
    }

    async fn local_default_branch(&self, path: &str) -> Option<String> {
        let stdout = self
            .local_git(
                path,
                &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
            )
            .await
            .ok()?;
        let trimmed = stdout.trim();
        let branch = trimmed.strip_prefix("origin/")?;
        if branch.is_empty() {
            None
        } else {
            Some(branch.to_string())
        }
    }

    fn apply_ci(&self, key: &QueryKey, outcome: KeyOutcome, indices: &[usize], ctx: &RoundCtx<'_>) {
        let KeyOutcome {
            ci: result,
            ci_suppressed: suppressed,
            ci_runs: runs,
            ci_completion: completion,
            ci_decision: decision,
            ..
        } = outcome;
        let Some(result) = result else {
            return;
        };
        let mut state = self.lock_state();
        let entry = state.keys.entry(key.clone()).or_default();
        let base = ci_base_interval(entry.ci.confirmed, ctx.ci_dial);

        if let Some(completion) = completion {
            entry.ci.completion = completion;
        }
        if suppressed {
            entry.ci.completion = CiCompletionState::default();
        }
        match result {
            Ok(_) if !suppressed && decision == Some(CiCompletionDecision::Pending) => {
                entry.ci.chip = CiState::Running;
                entry.ci.run_ids = entry.ci.completion.active_ids.clone();
                entry.ci.pull_requests = entry.ci.completion.active_prs.clone();
                entry.ci.failure = None;
                entry.ci.failure_interval = None;
                entry.ci.next_due = Some(ctx.now + Duration::from_secs(CI_RUNNING_INTERVAL_SECS));
            }
            Ok(_) if suppressed => {
                // A suppressed answer has no edge into or out of it: `confirmed =
                // None` breaks the transition chain both ways, like a fresh key.
                // Both suppression rules (resolved default branch; identical to
                // the default branch) publish `Idle` here.
                entry.ci.chip = CiState::Idle;
                entry.ci.confirmed = None;
                entry.ci.last_confirmed_at = Some(ctx.wall);
                entry.ci.failure_interval = None;
                entry.ci.run_ids.clear();
                entry.ci.pull_requests.clear();
                entry.ci.failure = None;
                entry.ci.next_due = Some(ctx.now + ci_base_interval(None, ctx.ci_dial));
                drop(state);
            }
            Ok(answer) => {
                let prior_confirmed = entry.ci.confirmed;
                let prior_last = entry.ci.last_confirmed_at;
                entry.ci.chip = answer;
                entry.ci.confirmed = Some(answer);
                entry.ci.last_confirmed_at = Some(ctx.wall);
                entry.ci.failure_interval = None;
                if answer == CiState::Running {
                    (entry.ci.run_ids, entry.ci.pull_requests) = runs;
                } else {
                    entry.ci.run_ids.clear();
                    entry.ci.pull_requests.clear();
                }
                entry.ci.failure = None;
                entry.ci.next_due = Some(ctx.now + ci_base_interval(Some(answer), ctx.ci_dial));
                drop(state);

                let kind = match (prior_confirmed, answer) {
                    (Some(CiState::Idle), CiState::Running) => Some(TransitionKind::CiStarted),
                    (Some(CiState::Running), CiState::Idle) => Some(TransitionKind::CiFinished),
                    _ => None,
                };
                if let Some(kind) = kind {
                    self.fan_out(
                        key,
                        TransitionMeta {
                            kind,
                            behind_by: None,
                            // The CI template renders no base label, so the CI axis
                            // never waits on, and never spends a call for, one.
                            base_branch: String::new(),
                            last_confirmed_at: prior_last,
                        },
                        indices,
                        ctx,
                    );
                }
            }
            Err(kind) if suppressed => {
                // A failed query on the resolved default branch keeps the chip
                // Idle: no CI is published for that branch either way. The
                // failure is not hidden - it still backs off, warns and can arm
                // the round gate - and `last_confirmed_at` is left untouched.
                let next = failure_interval(kind, base, entry.ci.failure_interval);
                entry.ci.chip = CiState::Idle;
                entry.ci.confirmed = None;
                entry.ci.failure_interval = Some(next);
                entry.ci.run_ids.clear();
                entry.ci.pull_requests.clear();
                entry.ci.failure = None;
                entry.ci.next_due = Some(ctx.now + next);
                drop(state);
                self.warn_failure(key, Axis::Ci, kind);
            }
            Err(kind) => {
                let next = failure_interval(kind, base, entry.ci.failure_interval);
                entry.ci.chip = CiState::Unknown;
                entry.ci.failure_interval = Some(next);
                entry.ci.run_ids.clear();
                entry.ci.pull_requests.clear();
                entry.ci.failure = Some(kind);
                entry.ci.next_due = Some(ctx.now + next);
                drop(state);
                self.warn_failure(key, Axis::Ci, kind);
            }
        }
    }

    fn apply_staleness(
        &self,
        key: &QueryKey,
        result: Option<CompareAnswer>,
        indices: &[usize],
        ctx: &RoundCtx<'_>,
    ) {
        let Some(result) = result else {
            return;
        };
        let mut state = self.lock_state();
        let base_label = state
            .base_branches
            .get(&key.nwo)
            .cloned()
            .unwrap_or_else(|| DEFAULT_BRANCH_LABEL.to_string());
        // #2131: a repo sitting on its own default branch has no branch of its
        // own to rebase, so the notice is suppressed there while the chip keeps
        // its orange bar. Fail open: the sentinel label means the default branch
        // is unresolved, which is not an identity and never suppresses (the same
        // rule #2126 applies to CI).
        let on_default_branch = base_label != DEFAULT_BRANCH_LABEL && key.branch == base_label;
        let entry = state.keys.entry(key.clone()).or_default();
        let base = ctx.staleness_dial;

        match result {
            Ok((answer, behind_by, ahead_by, _identical)) => {
                let prior_confirmed = entry.staleness.confirmed;
                let prior_last = entry.staleness.last_confirmed_at;
                // #2141 precedence, in this order, one decision branch:
                //   1. `on_default_branch`  -> #2131, unchanged, records `answer`.
                //   2. `ahead_by == Some(0)` -> no own commits, records `Current`.
                //   3. otherwise             -> today's behaviour; `None` fails open.
                // A repo on its own default branch also reports `ahead_by == 0`,
                // so case 1 is tested FIRST or the two rules disagree about what
                // was written to `confirmed`.
                let no_own_commits = !on_default_branch && ahead_by == Some(0);
                // Unlike `apply_ci`, which writes `confirmed = None` for a
                // suppressed answer ("no edge into or out of it"), a suppressed
                // staleness answer records `Current`. The axes differ on purpose:
                // CI has no deferred notice to keep alive, staleness does. Writing
                // `None` here, or leaving the prior value, consumes or never opens
                // the `Current -> Stale` edge, and the branch is never told again.
                let recorded = if no_own_commits {
                    StalenessState::Current
                } else {
                    answer
                };
                entry.staleness.chip = answer;
                entry.staleness.confirmed = Some(recorded);
                entry.staleness.behind_by = behind_by;
                entry.staleness.last_confirmed_at = Some(ctx.wall);
                entry.staleness.failure_interval = None;
                entry.staleness.next_due = Some(ctx.now + ctx.staleness_dial);
                drop(state);

                let stale = matches!(
                    (prior_confirmed, answer),
                    (Some(StalenessState::Current), StalenessState::Stale)
                );
                if stale && !on_default_branch && !no_own_commits {
                    self.fan_out(
                        key,
                        TransitionMeta {
                            kind: TransitionKind::BranchStale,
                            behind_by,
                            base_branch: base_label,
                            last_confirmed_at: prior_last,
                        },
                        indices,
                        ctx,
                    );
                }
            }
            Err(kind) => {
                let next = failure_interval(kind, base, entry.staleness.failure_interval);
                entry.staleness.chip = StalenessState::Unknown;
                entry.staleness.failure_interval = Some(next);
                entry.staleness.next_due = Some(ctx.now + next);
                drop(state);
                self.warn_failure(key, Axis::Staleness, kind);
            }
        }
    }

    fn fan_out(&self, key: &QueryKey, meta: TransitionMeta, indices: &[usize], ctx: &RoundCtx<'_>) {
        for index in indices {
            let fact = &ctx.facts[*index];
            for room_dir in &fact.room_dirs {
                self.send_transition(RemoteTransition {
                    repo_path: fact.path.clone(),
                    room_dir: room_dir.clone(),
                    nwo: key.nwo.clone(),
                    branch: fact.branch.clone().unwrap_or_default(),
                    head_sha: key.sha40.clone(),
                    kind: meta.kind,
                    behind_by: meta.behind_by,
                    base_branch: meta.base_branch.clone(),
                    observed_at: ctx.wall,
                    last_confirmed_at: meta.last_confirmed_at,
                });
            }
        }
    }

    /// The transition channel is best-effort by design: a stuck consumer must
    /// never slow a round, because chip freshness outranks a notice.
    fn send_transition(&self, transition: RemoteTransition) {
        match self.transitions.try_send(transition) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(dropped)) => {
                let drop_key = format!(
                    "{}|{}|{}",
                    dropped.room_dir, dropped.repo_path, dropped.head_sha
                );
                if self.lock_state().warned_transition_drops.insert(drop_key) {
                    log::warn!(
                        "[RemoteSweeper] transition channel full; dropped transition for {}",
                        dropped.repo_path
                    );
                }
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                let mut state = self.lock_state();
                if !state.closed_warned {
                    state.closed_warned = true;
                    drop(state);
                    log::warn!(
                        "[RemoteSweeper] transition channel closed; transitions are dropped for \
                            the rest of this process"
                    );
                }
            }
        }
    }

    fn warn_failure(&self, key: &QueryKey, axis: Axis, kind: FailureKind) {
        let first = self
            .lock_state()
            .warned_failures
            .insert((key.clone(), axis));
        if first {
            log::warn!(
                "[RemoteSweeper] {} query failed for {}@{}: {:?}",
                axis_label(axis),
                key.nwo,
                key.sha40,
                kind
            );
        }
    }
}

fn persisted_ci_state(state: CiState) -> PersistedCiState {
    match state {
        CiState::Running => PersistedCiState::Running,
        CiState::Idle => PersistedCiState::Idle,
        CiState::Unknown => PersistedCiState::Unknown,
    }
}

/// #2473: the persisted reason code for a CI failure, published only while the
/// chip is `Unknown`.
fn persisted_failure_reason(kind: FailureKind) -> &'static str {
    match kind {
        FailureKind::Timeout => "ci-query-timeout",
        FailureKind::RateLimited => "ci-query-rate-limited",
        FailureKind::SecondaryRateLimited => "ci-query-secondary-rate-limited",
        FailureKind::NotAuthenticated => "ci-query-not-authenticated",
        FailureKind::Incomplete => "ci-query-incomplete",
        FailureKind::Other => "ci-query-failed",
    }
}

fn persisted_repo_ci(activity: &RemoteActivity) -> PersistedRepoCi {
    PersistedRepoCi {
        state: persisted_ci_state(activity.ci),
        run_ids: activity.run_ids.clone(),
        pull_requests: activity.pull_requests.clone(),
        unknown_reason: match activity.ci {
            CiState::Unknown => activity
                .failure
                .map(|kind| persisted_failure_reason(kind).to_string()),
            _ => None,
        },
    }
}

fn activity_for(state: &SweeperState, fact: &PathFacts) -> RemoteActivity {
    let (Some(nwo), Some(head_sha), Some(branch)) = (&fact.nwo, &fact.head_sha, &fact.branch)
    else {
        return RemoteActivity::unknown();
    };
    let key = QueryKey {
        nwo: nwo.clone(),
        sha40: head_sha.clone(),
        branch: branch.clone(),
    };
    match state.keys.get(&key) {
        Some(entry) => RemoteActivity {
            ci: entry.ci.chip,
            staleness: entry.staleness.chip,
            // `Some` only while Stale: the tooltip has nothing to say otherwise.
            behind_by: match entry.staleness.chip {
                StalenessState::Stale => entry.staleness.behind_by,
                _ => None,
            },
            run_ids: entry.ci.run_ids.clone(),
            pull_requests: entry.ci.pull_requests.clone(),
            failure: entry.ci.failure,
        },
        None => RemoteActivity::unknown(),
    }
}

fn room_map() -> HashMap<String, Vec<String>> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for (repo_path, room_dir) in crate::pty::git_watcher::discovery_repo_rooms() {
        let rooms = map.entry(repo_path).or_default();
        if !rooms.contains(&room_dir) {
            rooms.push(room_dir);
        }
    }
    map
}

async fn query_key(
    spawner: &GhSpawner,
    gh: &Path,
    key: &QueryKey,
    plan: KeyPlan,
    default_branch: Option<&str>,
    input: (CiCompletionState, Option<CiState>),
    reserved: bool,
) -> KeyOutcome {
    let (mut completion, confirmed) = input;
    let mut outcome = KeyOutcome::default();

    // The compare runs FIRST when the staleness axis also needs it, so the
    // identical-to-default decision below reuses this round's one call.
    if plan.staleness {
        outcome.staleness = Some(
            match build_gh_command_spec(
                gh,
                GhQuery::Compare {
                    nwo: &key.nwo,
                    sha40: &key.sha40,
                },
            ) {
                Ok(spec) => run_query(spawner, spec, parse_compare_response).await,
                Err(_) => Err(FailureKind::Other),
            },
        );
    }

    if !plan.ci {
        return outcome;
    }

    let ci = match build_gh_command_spec(
        gh,
        GhQuery::Ci {
            nwo: &key.nwo,
            sha40: &key.sha40,
        },
    ) {
        Ok(spec) => {
            let branch = key.branch.clone();
            run_query(spawner, spec, |body| {
                parse_ci_response(body, &branch, &key.sha40)
            })
            .await
        }
        Err(_) => Err(FailureKind::Other),
    };
    let closing_ready = completion.closing_ready;
    let merge_error = ci.as_ref().ok().and_then(|answer| {
        merge_ci_observations(&mut completion, &answer.observations, true).err()
    });
    let scripted_staleness = outcome.staleness;
    let on_default_branch = default_branch == Some(key.branch.as_str());
    outcome.ci_suppressed = on_default_branch;
    let mut ci_result = match ci {
        // A resolved default branch is suppressed after the branch-filtered
        // query and without any identity question: `Ok` publishes `Idle`,
        // while an `Err` falls through to the failure arm below so the round
        // still backs off, warns and can arm its gate.
        Ok(_) if on_default_branch => Ok(CiState::Idle),
        // Only a non-default branch that HAS runs needs the identity
        // question; a branch with no runs answers `Idle` without it.
        Ok(answer)
            if answer.branch_has_runs
                && default_branch.is_some_and(|default| key.branch != default) =>
        {
            let compare = match scripted_staleness {
                Some(result) => result,
                None => {
                    // The ONLY place an extra `gh` call is issued for the
                    // identity question, and so the only place the round's
                    // reservation for it is consumed.
                    outcome.ci_identity_call = true;
                    match build_gh_command_spec(
                        gh,
                        GhQuery::Compare {
                            nwo: &key.nwo,
                            sha40: &key.sha40,
                        },
                    ) {
                        Ok(spec) => run_query(spawner, spec, parse_compare_response).await,
                        Err(_) => Err(FailureKind::Other),
                    }
                }
            };
            match compare {
                Ok((_, _, _, true)) => {
                    outcome.ci_suppressed = true;
                    Ok(CiState::Idle)
                }
                Ok((_, _, _, false)) => {
                    outcome.ci_runs = (answer.run_ids, answer.pull_requests);
                    Ok(answer.state)
                }
                Err(kind) => Err(kind),
            }
        }
        Ok(answer) => {
            outcome.ci_runs = (answer.run_ids, answer.pull_requests);
            Ok(answer.state)
        }
        Err(kind) => Err(kind),
    };
    if outcome.ci_suppressed {
        outcome.ci = Some(ci_result);
        return outcome;
    }
    if let Some(kind) = merge_error.filter(|_| ci_result.is_ok()) {
        ci_result = Err(kind);
    }
    if let Some(Err(kind)) = outcome
        .staleness
        .filter(|_| confirmed == Some(CiState::Running) && ci_result == Ok(CiState::Idle))
    {
        ci_result = Err(kind);
    }
    if ci_result == Ok(CiState::Idle) && confirmed == Some(CiState::Running) {
        outcome.ci_decision = Some(CiCompletionDecision::Pending);
        if closing_ready && completion.closing_ready && completion.all_proven() {
            outcome.ci_decision = Some(CiCompletionDecision::Accepted);
        } else {
            let unproven = completion
                .known
                .values()
                .find(|run| completion.proofs.get(&run.id) != Some(&run.proof()))
                .map(|run| run.id);
            if let Some(id) = unproven.filter(|_| reserved) {
                ci_result = corroborate_ci_run(
                    spawner,
                    gh,
                    key,
                    id,
                    &mut completion,
                    &mut outcome.ci_corroboration_calls,
                )
                .await;
                if ci_result == Ok(CiState::Running) {
                    outcome.ci_decision = Some(CiCompletionDecision::Accepted);
                }
            }
            completion.closing_ready = completion.all_proven();
        }
    } else if ci_result.is_ok() {
        outcome.ci_decision = Some(CiCompletionDecision::Accepted);
    }
    if ci_result.is_err() || outcome.staleness.is_some_and(|result| result.is_err()) {
        completion.closing_ready = false;
    }
    if ci_result == Ok(CiState::Running) {
        outcome.ci_runs = (completion.active_ids.clone(), completion.active_prs.clone());
    }
    outcome.ci_completion = Some(completion);

    outcome.ci = Some(ci_result);

    outcome
}

async fn run_query<T, P>(
    spawner: &GhSpawner,
    spec: GhCommandSpec,
    parse: P,
) -> Result<T, FailureKind>
where
    P: FnOnce(&str) -> Result<T, FailureKind>,
{
    match (spawner)(spec).await {
        Ok(output) if output.success => parse(&output.stdout),
        Ok(output) => Err(failure_kind(&output)),
        Err(kind) => Err(kind),
    }
}

/// The production spawner. `CALL_TIMEOUT` plus `kill_on_drop(true)` is the same
/// mechanism as INV-1: the timeout drops `output()`, and the drop kills `gh.exe`.
async fn spawn_gh(spec: GhCommandSpec) -> Result<GhCallOutput, FailureKind> {
    let mut command = command_from_spec(spec);
    match tokio::time::timeout(CALL_TIMEOUT, command.output()).await {
        Ok(Ok(output)) => Ok(GhCallOutput {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            success: output.status.success(),
        }),
        Ok(Err(_)) => Err(FailureKind::Other),
        Err(_) => Err(FailureKind::Timeout),
    }
}

async fn run_local_git(path: String, args: Vec<String>) -> Result<String, String> {
    let mut command = git_command(&path, &args);
    match tokio::time::timeout(CALL_TIMEOUT, command.output()).await {
        Ok(Ok(output)) if output.status.success() => {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        }
        Ok(Ok(output)) => Err(String::from_utf8_lossy(&output.stderr).to_string()),
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err("timeout".to_string()),
    }
}

#[cfg(test)]
mod tests;
