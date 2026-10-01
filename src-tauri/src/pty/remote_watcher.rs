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
    for run in observations {
        let mut observation = run.clone();
        if let Some(old) = state.known.get(&run.id) {
            if old.workflow_id != run.workflow_id
                || old.head_sha != run.head_sha
                || old.head_branch != run.head_branch
            {
                state.proofs.remove(&run.id);
                state.closing_ready = false;
                return Err(FailureKind::Other);
            }
            if run.run_attempt < old.run_attempt {
                state.proofs.remove(&run.id);
                state.closing_ready = false;
                return Err(FailureKind::Incomplete);
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
                return Err(FailureKind::Incomplete);
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
    if state.overflow {
        return Err(FailureKind::Incomplete);
    }
    Ok(())
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
                    "[RemoteSweeper] no snapshot directory was wired at construction; remote activity snapshots will not be published for this process"
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
            let settings = self.settings.blocking_read();
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
                        "[RemoteSweeper] work-list build failed ({e}); sweeping unfiltered this round"
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
            let settings = self.settings.read().await;
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
                        "[RemoteSweeper] transition channel closed; transitions are dropped for the rest of this process"
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

    if plan.ci {
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
        if on_default_branch {
            outcome.ci_suppressed = true;
        }
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
        if !outcome.ci_suppressed {
            if ci_result.is_ok() {
                if let Some(kind) = merge_error {
                    ci_result = Err(kind);
                }
            }
            if confirmed == Some(CiState::Running) && ci_result == Ok(CiState::Idle) {
                if let Some(Err(kind)) = outcome.staleness {
                    ci_result = Err(kind);
                }
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
                    if reserved {
                        if let Some(id) = unproven {
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
        }
        outcome.ci = Some(ci_result);
    }

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
mod tests {
    // Tests live in the child module so they can read private fields and pure
    // helpers without widening the module's public surface.
    use super::*;

    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::config::settings::AppSettings;

    /// Round-driving tests share the process-global snapshot and the discovery
    /// maps, so they MUST NOT run concurrently with each other or with the
    /// ac_discovery test that pushes the room map: a peer test's GC or discovery
    /// write would clear another test's entries. The lock lives beside the maps.
    ///
    /// Async-aware on purpose: a std guard held across the round's awaits is the
    /// exact shape `clippy::await_holding_lock` exists to reject.
    async fn round_test_lock() -> tokio::sync::MutexGuard<'static, ()> {
        crate::pty::git_watcher::DISCOVERY_TEST_LOCK.lock().await
    }

    fn sha_of(c: char) -> String {
        std::iter::repeat_n(c, 40).collect()
    }

    fn ok_output(body: &str) -> GhCallOutput {
        GhCallOutput {
            stdout: body.to_string(),
            stderr: String::new(),
            success: true,
        }
    }

    /// Republishes a repo's branch: `Harness::repo` publishes `main`, and every
    /// #2141 staleness test needs a branch that is NOT the default one.
    fn publish_branch(path: &str, branch: &str) {
        crate::pty::git_watcher::publish_git_status(
            path,
            Some(crate::pty::git_watcher::GitStatus {
                branch: Some(branch.to_string()),
                dirty: false,
            }),
        );
    }

    fn ci_body(sha40: &str, statuses: &[&str]) -> String {
        ci_body_with_total(sha40, statuses, statuses.len() as u64)
    }

    fn ci_body_with_total(sha40: &str, statuses: &[&str], total: u64) -> String {
        let rows: Vec<serde_json::Value> = statuses
            .iter()
            .enumerate()
            .map(|(i, status)| run_row(i as u64 + 1, sha40, "main", status, &[]))
            .collect();
        serde_json::json!({ "total_count": total, "workflow_runs": rows }).to_string()
    }

    /// Mixed-branch rows, so a test can replay one commit carrying runs for two
    /// branches. `total_count` matches the row count, as GitHub reports it.
    fn ci_rows(sha40: &str, rows: &[(&str, &str)]) -> String {
        let values: Vec<serde_json::Value> = rows
            .iter()
            .enumerate()
            .map(|(i, (branch, status))| run_row(i as u64 + 1, sha40, branch, status, &[]))
            .collect();
        serde_json::json!({ "total_count": values.len(), "workflow_runs": values }).to_string()
    }

    /// `ahead_by` is explicit at every call site, with no defaulting wrapper:
    /// since #2141 it decides whether the notice is suppressed, so a staleness
    /// fixture that does not state whether the branch has commits of its own is
    /// not stating its own premise.
    fn compare_body(status: &str, behind_by: u64, ahead_by: u64) -> String {
        serde_json::json!({
            "status": status,
            "behind_by": behind_by,
            "ahead_by": ahead_by,
        })
        .to_string()
    }

    #[derive(Default)]
    struct GhScripts {
        ci: VecDeque<Result<GhCallOutput, FailureKind>>,
        compare: VecDeque<Result<GhCallOutput, FailureKind>>,
        repo_info: VecDeque<Result<GhCallOutput, FailureKind>>,
        /// Per-endpoint overrides, keyed by the exact endpoint. Checked
        /// first so a test can script one key deterministically without depending
        /// on the order two concurrent queries are polled in.
        routes: HashMap<String, VecDeque<Result<GhCallOutput, FailureKind>>>,
        calls: Vec<Vec<String>>,
    }

    impl GhScripts {
        fn route(&mut self, marker: &str, result: Result<GhCallOutput, FailureKind>) {
            self.routes
                .entry(marker.to_string())
                .or_default()
                .push_back(result);
        }

        fn next(&mut self, spec: &GhCommandSpec) -> Result<GhCallOutput, FailureKind> {
            let target = spec.args.get(1).cloned().unwrap_or_default();
            self.calls.push(spec.args.clone());
            if let Some(queue) = self.routes.get_mut(&target) {
                return queue
                    .pop_front()
                    .unwrap_or_else(|| panic!("exhausted gh route: {target}"));
            }
            if target.contains("/actions/runs/") {
                panic!("missing gh route: {target}");
            }
            if target.contains("/actions/runs?") {
                self.ci
                    .pop_front()
                    .unwrap_or_else(|| Ok(ok_output(&ci_body(&sha_of('a'), &[]))))
            } else if target.contains("/compare/") {
                self.compare
                    .pop_front()
                    .unwrap_or_else(|| Ok(ok_output(&compare_body("identical", 0, 0))))
            } else {
                self.repo_info
                    .pop_front()
                    .unwrap_or(Err(FailureKind::Other))
            }
        }

        fn count(&self, needle: &str) -> usize {
            self.calls
                .iter()
                .filter(|args| args.get(1).is_some_and(|target| target.contains(needle)))
                .count()
        }

        fn count_repo_info(&self) -> usize {
            self.calls
                .iter()
                .filter(|args| {
                    args.get(1).is_some_and(|target| {
                        target.starts_with("repos/") && target.split('/').count() == 3
                    })
                })
                .count()
        }
    }

    const DEFAULT_ORIGIN: &str = "git@github.com:mblua/AgentsCommander.git";

    /// Per-PATH scripts, not queues: an origin URL is a property of the clone and
    /// must stay stable across rounds, while a queue that runs dry silently
    /// switches the repo to the default and creates a new key on every round.
    #[derive(Default)]
    struct LocalGitScripts {
        head_sha: HashMap<String, Result<String, String>>,
        origin_url: HashMap<String, Result<String, String>>,
        symbolic_ref: VecDeque<Result<String, String>>,
        calls: Vec<(String, Vec<String>)>,
    }

    impl LocalGitScripts {
        fn set_head_sha(&mut self, path: &str, sha: char) {
            self.head_sha.insert(path.to_string(), Ok(sha_of(sha)));
        }

        fn set_origin(&mut self, path: &str, origin: &str) {
            self.origin_url
                .insert(path.to_string(), Ok(origin.to_string()));
        }

        fn set_origin_failure(&mut self, path: &str, reason: &str) {
            self.origin_url
                .insert(path.to_string(), Err(reason.to_string()));
        }

        fn next(&mut self, path: &str, args: Vec<String>) -> Result<String, String> {
            self.calls.push((path.to_string(), args.clone()));
            match args.first().map(String::as_str) {
                Some("rev-parse") => self
                    .head_sha
                    .get(path)
                    .cloned()
                    .unwrap_or_else(|| Ok(sha_of('a'))),
                Some("config") => self
                    .origin_url
                    .get(path)
                    .cloned()
                    .unwrap_or_else(|| Ok(DEFAULT_ORIGIN.to_string())),
                Some("symbolic-ref") => self
                    .symbolic_ref
                    .pop_front()
                    .unwrap_or_else(|| Err("no symbolic ref".to_string())),
                other => Err(format!("unexpected local git call: {other:?}")),
            }
        }
    }

    struct Harness {
        temp: tempfile::TempDir,
        sweeper: Arc<RemoteSweeper>,
        /// `Receiver::try_recv` needs `&mut`, and most round tests hold the
        /// harness immutably, so the receiver gets interior mutability instead of
        /// forcing every test to declare `let mut harness`.
        transitions: Mutex<mpsc::Receiver<RemoteTransition>>,
        emitted: Arc<Mutex<Vec<RemoteActivityPayload>>>,
        gh: Arc<Mutex<GhScripts>>,
        git: Arc<Mutex<LocalGitScripts>>,
        jitter: Arc<Mutex<f64>>,
        /// The publication-clock seam's current value. Producer tests pin it;
        /// everything else reads the construction instant.
        publication_clock: Arc<Mutex<DateTime<Utc>>>,
    }

    impl Harness {
        fn new(settings: AppSettings) -> Self {
            Self::with_capacity(settings, TRANSITION_QUEUE_CAPACITY)
        }

        /// Round tests that do not assert persistence get no snapshot
        /// directory: the sweeper's one-time `DisabledMissingAtConstruction`
        /// state is then exercised by the whole existing suite.
        fn with_capacity(settings: AppSettings, capacity: usize) -> Self {
            Self::with_snapshot_dir_and_capacity(settings, capacity, None)
        }

        /// A harness with explicit snapshot wiring: `Some(dir)` publishes
        /// `remote-activity.json` into `dir`, `None` models a host that wired
        /// no instance config directory.
        fn with_snapshot_dir(settings: AppSettings, snapshot_dir: Option<PathBuf>) -> Self {
            Self::with_snapshot_dir_and_capacity(settings, TRANSITION_QUEUE_CAPACITY, snapshot_dir)
        }

        fn with_snapshot_dir_and_capacity(
            settings: AppSettings,
            capacity: usize,
            snapshot_dir: Option<PathBuf>,
        ) -> Self {
            let temp = tempfile::tempdir().expect("tempdir");
            let emitted = Arc::new(Mutex::new(Vec::new()));
            let emit: Emitter = {
                let emitted = Arc::clone(&emitted);
                Box::new(move |payload| {
                    emitted
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(payload.clone());
                })
            };

            let gh = Arc::new(Mutex::new(GhScripts::default()));
            let spawner: GhSpawner = {
                let gh = Arc::clone(&gh);
                Arc::new(move |spec| {
                    let gh = Arc::clone(&gh);
                    Box::pin(
                        async move { gh.lock().unwrap_or_else(|e| e.into_inner()).next(&spec) },
                    )
                })
            };
            let git = Arc::new(Mutex::new(LocalGitScripts::default()));
            let local_git: LocalGitRunner = {
                let git = Arc::clone(&git);
                Arc::new(move |path, args| {
                    let git = Arc::clone(&git);
                    Box::pin(async move {
                        git.lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .next(&path, args)
                    })
                })
            };
            let probe: GhProbe = Arc::new(|| Some(PathBuf::from("gh")));
            // Pinned, not random: every backoff assertion in this module is
            // exact, so the default draw is the low end of the range.
            let jitter_value = Arc::new(Mutex::new(0.0f64));
            let jitter: JitterSource = {
                let jitter_value = Arc::clone(&jitter_value);
                Arc::new(move || *jitter_value.lock().unwrap_or_else(|e| e.into_inner()))
            };
            let publication_value = Arc::new(Mutex::new(Utc::now()));
            let publication_clock: PublicationClock = {
                let publication_value = Arc::clone(&publication_value);
                Arc::new(move || *publication_value.lock().unwrap_or_else(|e| e.into_inner()))
            };

            let settings: SettingsState = Arc::new(tokio::sync::RwLock::new(settings));
            let sessions = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
            let (sweeper, transitions) = RemoteSweeper::with_capacity(
                sessions,
                settings,
                emit,
                RemoteSweeperSeams {
                    probe,
                    spawner,
                    local_git,
                    jitter,
                    snapshot_dir,
                    publication_clock,
                },
                capacity,
            );
            sweeper.lock_state().gh_path = Some(PathBuf::from("gh"));

            Self {
                temp,
                sweeper,
                transitions: Mutex::new(transitions),
                emitted,
                gh,
                git,
                jitter: jitter_value,
                publication_clock: publication_value,
            }
        }

        /// A real directory with a `.git` entry (Gate 1 passes) and a published
        /// branch, so the sweeper treats it as a live repo with zero real git.
        fn repo(&self, name: &str) -> String {
            let path = self.temp.path().join(name);
            std::fs::create_dir_all(path.join(".git")).expect("create fake repo");
            let path = path.to_string_lossy().replace('\\', "/");
            crate::pty::git_watcher::publish_git_status(
                &path,
                Some(crate::pty::git_watcher::GitStatus {
                    branch: Some("main".to_string()),
                    dirty: false,
                }),
            );
            path
        }

        fn set_jitter(&self, value: f64) {
            *self.jitter.lock().unwrap_or_else(|e| e.into_inner()) = value;
        }

        fn set_publication_clock(&self, value: DateTime<Utc>) {
            *self
                .publication_clock
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = value;
        }

        /// Pins the bucket AND clears the refill mark, so the next round refills
        /// by zero and the pinned value is exactly what that round may spend.
        fn set_budget(&self, tokens: f64) {
            let mut state = self.sweeper.lock_state();
            state.budget_tokens = tokens;
            state.budget_refilled_at = None;
        }

        fn gh_call_count(&self) -> usize {
            self.gh
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .calls
                .len()
        }

        fn set_work(&self, paths: &[String]) {
            crate::pty::git_watcher::set_discovery_repo_paths(paths.to_vec());
            crate::pty::git_watcher::set_discovery_repo_rooms(
                paths
                    .iter()
                    .map(|path| (path.clone(), format!("{path}/room")))
                    .collect(),
            );
        }

        async fn round(&self, now: Instant, wall: DateTime<Local>) {
            self.sweeper.run_round(now, wall).await;
        }

        fn emitted_count(&self) -> usize {
            self.emitted.lock().unwrap_or_else(|e| e.into_inner()).len()
        }

        fn snapshot(&self) -> HashMap<String, RemoteActivity> {
            remote_activity_snapshot()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }

        fn drain_transitions(&self) -> Vec<RemoteTransition> {
            let mut transitions = Vec::new();
            while let Ok(transition) = self
                .transitions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .try_recv()
            {
                transitions.push(transition);
            }
            transitions
        }

        fn try_recv_transition(&self) -> Option<RemoteTransition> {
            self.transitions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .try_recv()
                .ok()
        }

        fn close_transitions(&self) {
            self.transitions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .close();
        }
    }

    fn tick(now: &mut Instant, wall: &mut DateTime<Local>, secs: u64) {
        *now += Duration::from_secs(secs);
        *wall += chrono::Duration::seconds(secs as i64);
    }

    // --- 1: the positive control the verification-difficulty veto rests on ---

    #[test]
    fn ci_query_endpoint_is_exactly_the_expected_string() {
        let gh = Path::new("gh");
        let nwo = "mblua/AgentsCommander";
        let sha = sha_of('a');

        let ci = build_gh_command_spec(gh, GhQuery::Ci { nwo, sha40: &sha }).expect("ci spec");
        assert_eq!(
            ci.args,
            vec![
                "api".to_string(),
                format!("repos/{nwo}/actions/runs?head_sha={sha}&per_page=100"),
            ]
        );

        let compare =
            build_gh_command_spec(gh, GhQuery::Compare { nwo, sha40: &sha }).expect("compare spec");
        assert_eq!(
            compare.args,
            vec![
                "api".to_string(),
                format!("repos/{nwo}/compare/HEAD...{sha}")
            ]
        );

        let info = build_gh_command_spec(gh, GhQuery::RepoInfo { nwo }).expect("info spec");
        assert_eq!(info.args, vec!["api".to_string(), format!("repos/{nwo}")]);

        // Rejection by construction: an abbreviated SHA is impossible to send.
        assert!(build_gh_command_spec(
            gh,
            GhQuery::Ci {
                nwo,
                sha40: "abc1234"
            }
        )
        .is_err());
        assert!(build_gh_command_spec(
            gh,
            GhQuery::Compare {
                nwo,
                sha40: "abc1234"
            }
        )
        .is_err());
        assert!(
            build_gh_command_spec(
                gh,
                GhQuery::Ci {
                    nwo,
                    sha40: &format!("{sha}a")
                }
            )
            .is_err(),
            "41 characters must be rejected"
        );
        assert!(
            build_gh_command_spec(
                gh,
                GhQuery::Ci {
                    nwo,
                    sha40: &sha.to_uppercase()
                }
            )
            .is_err(),
            "uppercase hex must be rejected"
        );
        assert!(build_gh_command_spec(gh, GhQuery::RepoInfo { nwo: "no-slash" }).is_err());
    }

    #[test]
    fn ci_running_when_any_row_is_not_completed() {
        assert_eq!(
            parse_ci_response(
                &ci_body(&sha_of('a'), &["completed", "in_progress"]),
                "main",
                &sha_of('a')
            ),
            Ok(CiAnswer {
                state: CiState::Running,
                branch_has_runs: true,
                observations: vec![
                    expected_run(1, "main", "completed", &[]),
                    expected_run(2, "main", "in_progress", &[])
                ],
                run_ids: vec![2],
                pull_requests: Vec::new(),
            })
        );
        assert_eq!(
            parse_ci_response(&ci_body(&sha_of('a'), &["queued"]), "main", &sha_of('a')),
            Ok(CiAnswer {
                state: CiState::Running,
                branch_has_runs: true,
                observations: vec![expected_run(1, "main", "queued", &[])],
                run_ids: vec![1],
                pull_requests: Vec::new(),
            }),
            "an unknown future status counts as running, never as idle"
        );
    }

    #[test]
    fn ci_idle_when_every_row_is_completed() {
        assert_eq!(
            parse_ci_response(&ci_body(&sha_of('a'), &[]), "main", &sha_of('a')),
            Ok(CiAnswer {
                state: CiState::Idle,
                branch_has_runs: false,
                observations: vec![],
                run_ids: Vec::new(),
                pull_requests: Vec::new(),
            })
        );
        assert_eq!(
            parse_ci_response(
                &ci_body(&sha_of('a'), &["completed", "completed"]),
                "main",
                &sha_of('a')
            ),
            Ok(CiAnswer {
                state: CiState::Idle,
                branch_has_runs: true,
                observations: vec![
                    expected_run(1, "main", "completed", &[]),
                    expected_run(2, "main", "completed", &[])
                ],
                run_ids: Vec::new(),
                pull_requests: Vec::new(),
            })
        );
    }

    #[test]
    fn ci_incomplete_page_is_unknown_not_idle() {
        let body = ci_body_with_total(&sha_of('a'), &["completed"; 99], 240);
        assert_eq!(
            parse_ci_response(&body, "main", &sha_of('a')),
            Err(FailureKind::Incomplete)
        );
    }

    #[test]
    fn rows_without_a_branch_are_dropped_and_the_page_stays_complete() {
        let body = serde_json::json!({
            "total_count": 2,
            "workflow_runs": [
                { "status": "in_progress" },
                run_row(1, &sha_of('a'), "main", "completed", &[]),
            ],
        })
        .to_string();
        assert_eq!(
            parse_ci_response(&body, "main", &sha_of('a')),
            Ok(CiAnswer {
                state: CiState::Idle,
                branch_has_runs: true,
                observations: vec![expected_run(1, "main", "completed", &[])],
                run_ids: Vec::new(),
                pull_requests: Vec::new(),
            }),
            "a row with no head_branch cannot answer for any branch"
        );

        let short_page = serde_json::json!({
            "total_count": 5,
            "workflow_runs": [{ "head_branch": "main", "status": "completed" }],
        })
        .to_string();
        assert_eq!(
            parse_ci_response(&short_page, "main", &sha_of('a')),
            Err(FailureKind::Incomplete),
            "the missing rows might be other branches, so the page cannot answer"
        );
    }

    #[test]
    fn branch_filter_is_exact_match() {
        let body = ci_rows(
            &sha_of('a'),
            &[
                ("main-2", "in_progress"),
                ("origin/main", "in_progress"),
                ("main", "completed"),
            ],
        );
        assert_eq!(
            parse_ci_response(&body, "main", &sha_of('a')),
            Ok(CiAnswer {
                state: CiState::Idle,
                branch_has_runs: true,
                observations: vec![expected_run(3, "main", "completed", &[])],
                run_ids: Vec::new(),
                pull_requests: Vec::new(),
            })
        );
        assert_eq!(
            parse_ci_response(&body, "main-2", &sha_of('a')),
            Ok(CiAnswer {
                state: CiState::Running,
                branch_has_runs: true,
                observations: vec![expected_run(1, "main-2", "in_progress", &[])],
                run_ids: vec![1],
                pull_requests: Vec::new(),
            })
        );
    }

    #[test]
    fn compare_status_table_maps_to_staleness() {
        assert_eq!(
            parse_compare_response(&compare_body("identical", 0, 0)),
            Ok((StalenessState::Current, None, Some(0), true))
        );
        assert_eq!(
            parse_compare_response(&compare_body("ahead", 0, 1)),
            Ok((StalenessState::Current, None, Some(1), false)),
            "ahead with behind_by 0 is not stale"
        );
        assert_eq!(
            parse_compare_response(&compare_body("behind", 3, 0)),
            Ok((StalenessState::Stale, Some(3), Some(0), false)),
            "behind carries ahead_by 0: nothing of its own to validate"
        );
        assert_eq!(
            parse_compare_response(&compare_body("diverged", 7, 2)),
            Ok((StalenessState::Stale, Some(7), Some(2), false))
        );
    }

    /// #2141 fail-open: `ahead_by` is a suppression input, not part of the
    /// classification, so a response without the field still maps to the same
    /// staleness state and reads `None`, which never suppresses.
    #[test]
    fn compare_without_ahead_by_reads_none_and_keeps_its_state() {
        let body = serde_json::json!({ "status": "behind", "behind_by": 4 }).to_string();
        assert_eq!(
            parse_compare_response(&body),
            Ok((StalenessState::Stale, Some(4), None, false))
        );
        let out_of_range = serde_json::json!({
            "status": "behind",
            "behind_by": 4,
            "ahead_by": u64::from(u32::MAX) + 1,
        })
        .to_string();
        assert_eq!(
            parse_compare_response(&out_of_range),
            Ok((StalenessState::Stale, Some(4), None, false))
        );
    }

    #[tokio::test]
    async fn unpushed_head_404_maps_to_unknown() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        let repo = harness.repo("repo-a");
        harness.set_work(std::slice::from_ref(&repo));
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            // CI: no runs exist for an unpushed SHA, which is honestly idle.
            gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
            // Staleness: compare returns 404, which is unknown, not current.
            gh.compare.push_back(Ok(GhCallOutput {
                stdout: String::new(),
                stderr: "gh: Not Found (HTTP 404)".to_string(),
                success: false,
            }));
        }

        harness.round(Instant::now(), Local::now()).await;

        let snapshot = harness.snapshot();
        let activity = snapshot.get(&repo).expect("snapshot entry");
        assert_eq!(activity.ci, CiState::Idle);
        assert_eq!(activity.staleness, StalenessState::Unknown);
    }

    #[tokio::test]
    async fn every_failure_maps_to_unknown() {
        let _guard = round_test_lock().await;
        type ScriptedFailure = Box<dyn Fn(&mut GhScripts)>;
        let cases: Vec<ScriptedFailure> = vec![
            Box::new(|gh: &mut GhScripts| gh.ci.push_back(Err(FailureKind::Timeout))),
            Box::new(|gh: &mut GhScripts| {
                gh.ci.push_back(Ok(GhCallOutput {
                    stdout: String::new(),
                    stderr: "gh: Forbidden (HTTP 403)".to_string(),
                    success: false,
                }));
            }),
            Box::new(|gh: &mut GhScripts| {
                gh.ci.push_back(Ok(GhCallOutput {
                    stdout: String::new(),
                    stderr: "gh: Not Found (HTTP 404)".to_string(),
                    success: false,
                }));
            }),
            Box::new(|gh: &mut GhScripts| {
                gh.ci.push_back(Ok(GhCallOutput {
                    stdout: String::new(),
                    stderr: "fatal: something".to_string(),
                    success: false,
                }));
            }),
            Box::new(|gh: &mut GhScripts| gh.ci.push_back(Ok(ok_output("{not json")))),
            // No origin and a non-GitHub remote: no key at all.
            Box::new(|_gh: &mut GhScripts| {}),
            Box::new(|_gh: &mut GhScripts| {}),
        ];

        for (index, case) in cases.iter().enumerate() {
            let harness = Harness::new(AppSettings::default());
            let repo = harness.repo(&format!("repo-{index}"));
            harness.set_work(std::slice::from_ref(&repo));
            if index == 5 {
                harness
                    .git
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .set_origin_failure(&repo, "no origin");
            }
            if index == 6 {
                harness
                    .git
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .set_origin(&repo, "https://gitlab.com/a/b.git");
            }
            case(&mut harness.gh.lock().unwrap_or_else(|e| e.into_inner()));

            harness.round(Instant::now(), Local::now()).await;

            let snapshot = harness.snapshot();
            let activity = snapshot.get(&repo).expect("snapshot entry");
            assert_eq!(
                activity.ci,
                CiState::Unknown,
                "case {index} must read Unknown, never Idle/Running"
            );
        }
    }

    #[tokio::test]
    async fn failure_does_not_retain_the_previous_state() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        let repo = harness.repo("repo-a");
        harness.set_work(std::slice::from_ref(&repo));
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.ci
                .push_back(Ok(ok_output(&ci_body(&sha_of('a'), &["in_progress"]))));
            gh.ci.push_back(Err(FailureKind::Timeout));
        }

        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;
        assert_eq!(
            harness.snapshot().get(&repo).expect("entry").ci,
            CiState::Running
        );

        tick(&mut now, &mut wall, 60);
        harness.round(now, wall).await;
        assert_eq!(
            harness.snapshot().get(&repo).expect("entry").ci,
            CiState::Unknown,
            "a retained Running would be a permanent yellow chip over a finished CI"
        );
    }

    #[tokio::test]
    async fn cold_start_emits_no_transition() {
        let _guard = round_test_lock().await;
        for body in [
            ci_body(&sha_of('a'), &["in_progress"]),
            ci_body(&sha_of('a'), &[]),
        ] {
            let harness = Harness::new(AppSettings::default());
            let repo = harness.repo("repo-a");
            harness.set_work(&[repo]);
            harness
                .gh
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .ci
                .push_back(Ok(ok_output(&body)));

            harness.round(Instant::now(), Local::now()).await;

            assert!(
                harness.drain_transitions().is_empty(),
                "no edge out of Unknown exists"
            );
        }
    }

    // --- the #2126 incident: runs on a different branch must not count ---

    #[tokio::test]
    async fn ci_counts_only_runs_on_the_current_branch() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        let repo = harness.repo("repo-a");
        harness.set_work(std::slice::from_ref(&repo));
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.ci.push_back(Ok(ok_output(&ci_rows(
                &sha_of('a'),
                &[
                    ("fix/2124-ignore-short-idle-bursts", "in_progress"),
                    ("fix/2124-ignore-short-idle-bursts", "in_progress"),
                    ("main", "completed"),
                ],
            ))));
        }

        harness.round(Instant::now(), Local::now()).await;

        assert_eq!(
            harness.snapshot().get(&repo).expect("entry").ci,
            CiState::Idle,
            "only the run on the repo's current branch counts"
        );
    }

    #[tokio::test]
    async fn incident_replay_identical_branch_and_default_get_no_notice() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        let default_repo = harness.repo("repo-a");
        let feature_repo = harness.repo("repo-b");
        crate::pty::git_watcher::publish_git_status(
            &feature_repo,
            Some(crate::pty::git_watcher::GitStatus {
                branch: Some("fix/2124-ignore-short-idle-bursts".to_string()),
                dirty: false,
            }),
        );
        harness.set_work(&[default_repo.clone(), feature_repo.clone()]);
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.repo_info
                .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
            let round_one = ci_rows(
                &sha_of('a'),
                &[
                    ("fix/2124-ignore-short-idle-bursts", "in_progress"),
                    ("fix/2124-ignore-short-idle-bursts", "in_progress"),
                    ("main", "completed"),
                ],
            );
            let round_two = ci_rows(
                &sha_of('a'),
                &[
                    ("fix/2124-ignore-short-idle-bursts", "completed"),
                    ("fix/2124-ignore-short-idle-bursts", "completed"),
                    ("main", "completed"),
                ],
            );
            // Both keys poll every round and the queue is drained in call order,
            // so both orders must see that round's body.
            for body in [&round_one, &round_two, &round_one, &round_two] {
                gh.ci.push_back(Ok(ok_output(body)));
            }
        }

        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;
        let round_one_feature = harness
            .snapshot()
            .get(&feature_repo)
            .expect("feature repo")
            .ci;
        let round_one_default = harness
            .snapshot()
            .get(&default_repo)
            .expect("default repo")
            .ci;

        tick(&mut now, &mut wall, 60);
        harness.round(now, wall).await;

        let transitions = harness.drain_transitions();
        assert_eq!(
            transitions.len(),
            0,
            "the incident emitted CiFinished to both rooms"
        );
        let snapshot = harness.snapshot();
        assert_eq!(
            round_one_feature,
            CiState::Idle,
            "round 1: the feature branch is identical to the default branch"
        );
        assert_eq!(round_one_default, CiState::Idle);
        assert_eq!(
            snapshot.get(&feature_repo).expect("feature repo").ci,
            CiState::Idle
        );
        assert_eq!(
            snapshot.get(&default_repo).expect("default repo").ci,
            CiState::Idle
        );
    }

    #[tokio::test]
    async fn non_identical_branch_notifies_only_its_own_rooms() {
        let _guard = round_test_lock().await;
        let settings = AppSettings {
            branch_staleness_enabled: false,
            ..AppSettings::default()
        };
        let harness = Harness::new(settings);
        let default_repo = harness.repo("repo-a");
        let feature_repo = harness.repo("repo-b");
        crate::pty::git_watcher::publish_git_status(
            &feature_repo,
            Some(crate::pty::git_watcher::GitStatus {
                branch: Some("fix/2124-ignore-short-idle-bursts".to_string()),
                dirty: false,
            }),
        );
        harness.set_work(&[default_repo.clone(), feature_repo.clone()]);
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.repo_info
                .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
            for _ in 0..4 {
                gh.compare
                    .push_back(Ok(ok_output(&compare_body("ahead", 0, 1))));
            }
            let bodies = [
                ci_rows(
                    &sha_of('a'),
                    &[("fix/2124-ignore-short-idle-bursts", "completed")],
                ),
                ci_rows(
                    &sha_of('a'),
                    &[("fix/2124-ignore-short-idle-bursts", "in_progress")],
                ),
                ci_rows(
                    &sha_of('a'),
                    &[("fix/2124-ignore-short-idle-bursts", "completed")],
                ),
                ci_rows(
                    &sha_of('a'),
                    &[("fix/2124-ignore-short-idle-bursts", "completed")],
                ),
            ];
            for body in bodies {
                gh.ci.push_back(Ok(ok_output(&body)));
                gh.ci.push_back(Ok(ok_output(&body)));
            }
        }

        queue_completion(
            &harness,
            "mblua/AgentsCommander",
            run_row(
                1,
                &sha_of('a'),
                "fix/2124-ignore-short-idle-bursts",
                "completed",
                &[],
            ),
        );
        let mut now = Instant::now();
        let mut wall = Local::now();
        for _ in 0..4 {
            harness.round(now, wall).await;
            tick(&mut now, &mut wall, 60);
        }

        let transitions = harness.drain_transitions();
        assert_eq!(transitions.len(), 2);
        assert_eq!(transitions[0].kind, TransitionKind::CiStarted);
        assert_eq!(transitions[0].repo_path, feature_repo);
        assert_eq!(transitions[1].kind, TransitionKind::CiFinished);
        assert_eq!(transitions[1].repo_path, feature_repo);
        assert_eq!(
            harness
                .gh
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .count("/compare/"),
            4,
            "one identity compare per round for the non-default branch, none for the default one"
        );
    }

    /// #2329: a repo on its RESOLVED default branch publishes no CI at all,
    /// even while GitHub has a run in progress on that exact branch and SHA.
    /// The CI query is still issued (the reserved token is spent); only its
    /// answer is suppressed.
    #[tokio::test]
    async fn resolved_default_branch_ci_is_suppressed() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        let repo = harness.repo("repo-a");
        harness.set_work(std::slice::from_ref(&repo));
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.repo_info
                .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
            gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
            gh.ci
                .push_back(Ok(ok_output(&ci_body(&sha_of('a'), &["in_progress"]))));
            gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
        }

        let mut now = Instant::now();
        let mut wall = Local::now();
        for round in 0..3 {
            harness.round(now, wall).await;
            assert_eq!(
                harness.snapshot().get(&repo).expect("entry").ci,
                CiState::Idle,
                "round {round}: a resolved default branch publishes Idle, never Running"
            );
            tick(&mut now, &mut wall, 60);
        }

        assert!(
            harness.drain_transitions().is_empty(),
            "a suppressed CI answer has no edge into or out of it"
        );
        {
            let state = harness.sweeper.lock_state();
            let (_, entry) = state.keys.iter().next().expect("one key");
            assert_eq!(
                entry.ci.confirmed, None,
                "the transition chain stays broken across suppressed rounds"
            );
        }
        let gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(
            gh.count("/actions/runs?"),
            3,
            "the CI call is still issued every round; only its answer is suppressed"
        );
        assert_eq!(
            gh.count("/compare/"),
            1,
            "one staleness compare in round 1; the default branch adds no identity compare"
        );
    }

    #[tokio::test]
    async fn unresolved_default_branch_does_not_suppress() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        let repo = harness.repo("repo-a");
        crate::pty::git_watcher::publish_git_status(
            &repo,
            Some(crate::pty::git_watcher::GitStatus {
                branch: Some("fix/x".to_string()),
                dirty: false,
            }),
        );
        harness.set_work(std::slice::from_ref(&repo));
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            // No repo_info answer is queued and no symbolic-ref answer is queued,
            // so the label stays unresolved and rule 2 cannot be decided.
            gh.ci.push_back(Ok(ok_output(&ci_rows(
                &sha_of('a'),
                &[("fix/x", "completed")],
            ))));
            gh.ci.push_back(Ok(ok_output(&ci_rows(
                &sha_of('a'),
                &[("fix/x", "in_progress")],
            ))));
            gh.ci.push_back(Ok(ok_output(&ci_rows(
                &sha_of('a'),
                &[("fix/x", "completed")],
            ))));
            gh.ci.push_back(Ok(ok_output(&ci_rows(
                &sha_of('a'),
                &[("fix/x", "completed")],
            ))));
        }

        queue_completion(
            &harness,
            "mblua/AgentsCommander",
            run_row(1, &sha_of('a'), "fix/x", "completed", &[]),
        );
        let mut now = Instant::now();
        let mut wall = Local::now();
        for _ in 0..4 {
            harness.round(now, wall).await;
            tick(&mut now, &mut wall, 60);
        }

        let transitions = harness.drain_transitions();
        assert_eq!(
            transitions.len(),
            2,
            "an unresolved default branch cannot suppress"
        );
        assert_eq!(transitions[0].kind, TransitionKind::CiStarted);
        assert_eq!(transitions[1].kind, TransitionKind::CiFinished);
    }

    #[tokio::test]
    async fn identity_compare_failure_is_unknown() {
        let _guard = round_test_lock().await;
        let settings = AppSettings {
            branch_staleness_enabled: false,
            ..AppSettings::default()
        };
        let harness = Harness::new(settings);
        let repo = harness.repo("repo-a");
        crate::pty::git_watcher::publish_git_status(
            &repo,
            Some(crate::pty::git_watcher::GitStatus {
                branch: Some("fix/x".to_string()),
                dirty: false,
            }),
        );
        harness.set_work(std::slice::from_ref(&repo));
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.repo_info
                .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
            gh.ci.push_back(Ok(ok_output(&ci_rows(
                &sha_of('a'),
                &[("fix/x", "in_progress")],
            ))));
            gh.compare.push_back(Err(FailureKind::Other));
        }

        harness.round(Instant::now(), Local::now()).await;

        assert_eq!(
            harness.snapshot().get(&repo).expect("entry").ci,
            CiState::Unknown,
            "a failed identity compare is Unknown, never a silent suppression"
        );
    }

    #[tokio::test]
    async fn branch_change_on_same_commit_retires_the_key_without_a_transition() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        let repo = harness.repo("repo-a");
        harness.set_work(std::slice::from_ref(&repo));
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.ci.push_back(Ok(ok_output(&ci_rows(
                &sha_of('a'),
                &[("main", "in_progress")],
            ))));
            gh.ci.push_back(Ok(ok_output(&ci_rows(
                &sha_of('a'),
                &[("fix/x", "completed")],
            ))));
        }
        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;
        crate::pty::git_watcher::publish_git_status(
            &repo,
            Some(crate::pty::git_watcher::GitStatus {
                branch: Some("fix/x".to_string()),
                dirty: false,
            }),
        );
        tick(&mut now, &mut wall, 60);
        harness.round(now, wall).await;

        assert!(
            harness.drain_transitions().is_empty(),
            "a branch change on the same commit is a new key, not an edge"
        );
        assert_eq!(
            harness.snapshot().get(&repo).expect("entry").ci,
            CiState::Idle
        );
    }

    #[tokio::test]
    async fn unknown_is_transparent_and_never_expires() {
        let _guard = round_test_lock().await;
        // 1000s clears any backoff interval the failing rounds could have set,
        // and 3600s is the phase's long-advance variant; the point is that the
        // `Unknown` rounds never expire the previous confirmed state.
        for advance in [1000_u64, 3600] {
            let harness = Harness::new(AppSettings::default());
            let repo = harness.repo("repo-a");
            harness.set_work(&[repo]);
            {
                let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
                gh.ci
                    .push_back(Ok(ok_output(&ci_body(&sha_of('a'), &["in_progress"]))));
                gh.ci.push_back(Err(FailureKind::Timeout));
                gh.ci.push_back(Err(FailureKind::Timeout));
                gh.ci.push_back(Err(FailureKind::Timeout));
                gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
                gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
            }

            queue_completion(
                &harness,
                "mblua/AgentsCommander",
                run_row(1, &sha_of('a'), "main", "completed", &[]),
            );
            let mut now = Instant::now();
            let mut wall = Local::now();
            let running_wall = wall;
            harness.round(now, wall).await;
            for _ in 0..3 {
                tick(&mut now, &mut wall, advance);
                harness.round(now, wall).await;
            }
            tick(&mut now, &mut wall, advance);
            harness.round(now, wall).await;
            assert_eq!(
                harness.snapshot().values().next().unwrap().ci,
                CiState::Running
            );
            assert!(harness.drain_transitions().is_empty());
            tick(&mut now, &mut wall, 10);
            let closing_wall = wall;
            harness.round(now, wall).await;

            let transitions = harness.drain_transitions();
            assert_eq!(transitions.len(), 1, "advance={advance}");
            let transition = &transitions[0];
            assert_eq!(transition.kind, TransitionKind::CiFinished);
            assert_eq!(transition.last_confirmed_at, Some(running_wall));
            assert_eq!(transition.observed_at, closing_wall);
        }
    }

    #[tokio::test]
    async fn last_confirmed_at_advances_on_every_confirmed_round() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        let repo = harness.repo("repo-a");
        harness.set_work(&[repo]);
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            for _ in 0..20 {
                gh.ci
                    .push_back(Ok(ok_output(&ci_body(&sha_of('a'), &["in_progress"]))));
            }
            gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
            gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
        }

        queue_completion(
            &harness,
            "mblua/AgentsCommander",
            run_row(1, &sha_of('a'), "main", "completed", &[]),
        );
        let mut now = Instant::now();
        let mut wall = Local::now();
        let first_wall = wall;
        let mut twentieth_wall = wall;
        for index in 0..20 {
            if index > 0 {
                tick(&mut now, &mut wall, 60);
            }
            if index == 19 {
                twentieth_wall = wall;
            }
            harness.round(now, wall).await;
        }
        tick(&mut now, &mut wall, 60);
        harness.round(now, wall).await;

        assert!(harness.drain_transitions().is_empty());
        tick(&mut now, &mut wall, 10);
        harness.round(now, wall).await;
        let transitions = harness.drain_transitions();
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].last_confirmed_at, Some(twentieth_wall));
        assert_ne!(
            transitions[0].last_confirmed_at,
            Some(first_wall),
            "must be the 20th round, not the first"
        );
    }

    #[tokio::test]
    async fn head_sha_change_retires_the_key_without_a_transition() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        let repo = harness.repo("repo-a");
        harness.set_work(std::slice::from_ref(&repo));
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.ci
                .push_back(Ok(ok_output(&ci_body(&sha_of('a'), &["in_progress"]))));
            gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
        }
        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;
        harness
            .git
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_head_sha(&repo, 'b');
        tick(&mut now, &mut wall, 60);
        harness.round(now, wall).await;

        assert!(
            harness.drain_transitions().is_empty(),
            "a new SHA starts at Unknown and must not emit the old SHA's finish"
        );
        assert_eq!(
            harness.snapshot().get(&repo).expect("entry").ci,
            CiState::Idle
        );
    }

    #[tokio::test]
    async fn stale_to_current_emits_nothing() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        let repo = harness.repo("repo-a");
        harness.set_work(&[repo]);
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.compare
                .push_back(Ok(ok_output(&compare_body("behind", 2, 1))));
            gh.compare
                .push_back(Ok(ok_output(&compare_body("identical", 0, 0))));
        }

        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;
        drain(&harness);
        tick(&mut now, &mut wall, 300);
        harness.round(now, wall).await;

        assert!(harness.drain_transitions().is_empty());
    }

    #[tokio::test]
    async fn current_to_stale_emits_once_only() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        let repo = harness.repo("repo-a");
        crate::pty::git_watcher::publish_git_status(
            &repo,
            Some(crate::pty::git_watcher::GitStatus {
                branch: Some("fix/2131".to_string()),
                dirty: false,
            }),
        );
        harness.set_work(&[repo]);
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.compare
                .push_back(Ok(ok_output(&compare_body("identical", 0, 0))));
            for _ in 0..6 {
                gh.compare
                    .push_back(Ok(ok_output(&compare_body("behind", 4, 1))));
            }
            gh.repo_info
                .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
        }

        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;
        drain(&harness);
        for _ in 0..6 {
            tick(&mut now, &mut wall, 300);
            harness.round(now, wall).await;
        }

        let transitions = harness.drain_transitions();
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].kind, TransitionKind::BranchStale);
        assert_eq!(transitions[0].behind_by, Some(4));
    }

    #[tokio::test]
    async fn stale_on_default_branch_sends_no_notice_but_keeps_the_chip() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        let repo = harness.repo("repo-a");
        harness.set_work(std::slice::from_ref(&repo));
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.repo_info
                .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
            gh.compare
                .push_back(Ok(ok_output(&compare_body("identical", 0, 0))));
            gh.compare
                .push_back(Ok(ok_output(&compare_body("behind", 4, 0))));
        }

        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;
        drain(&harness);
        tick(&mut now, &mut wall, 300);
        harness.round(now, wall).await;

        assert!(
            harness.drain_transitions().is_empty(),
            "the default branch has no branch of its own to rebase: no notice"
        );
        let snapshot = harness.snapshot();
        let activity = snapshot.get(&repo).expect("entry");
        assert_eq!(
            activity.staleness,
            StalenessState::Stale,
            "the chip keeps its orange bar"
        );
        assert_eq!(activity.behind_by, Some(4));
    }

    /// #2141: a repo on its default branch reports `ahead_by == 0` too, so the
    /// #2131 gate is evaluated FIRST and the `ahead_by` rule never runs there.
    /// Own commits on the default branch change nothing.
    #[tokio::test]
    async fn stale_on_default_branch_with_own_commits_still_sends_no_notice() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        let repo = harness.repo("repo-a");
        harness.set_work(std::slice::from_ref(&repo));
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.repo_info
                .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
            gh.compare
                .push_back(Ok(ok_output(&compare_body("identical", 0, 0))));
            gh.compare
                .push_back(Ok(ok_output(&compare_body("diverged", 4, 3))));
        }

        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;
        drain(&harness);
        tick(&mut now, &mut wall, 300);
        harness.round(now, wall).await;

        assert!(
            harness.drain_transitions().is_empty(),
            "#2131 wins over the #2141 rule on the default branch"
        );
        assert_eq!(
            harness.snapshot().get(&repo).expect("entry").staleness,
            StalenessState::Stale
        );
    }

    /// #2141: a branch with no commits of its own has nothing that a stale base
    /// could invalidate, so the notice is suppressed while the chip still turns.
    #[tokio::test]
    async fn stale_without_own_commits_sends_no_notice_but_keeps_the_chip() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        let repo = harness.repo("repo-a");
        publish_branch(&repo, "fix/2141");
        harness.set_work(std::slice::from_ref(&repo));
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.repo_info
                .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
            gh.compare
                .push_back(Ok(ok_output(&compare_body("identical", 0, 0))));
            gh.compare
                .push_back(Ok(ok_output(&compare_body("behind", 2, 0))));
        }

        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;
        drain(&harness);
        tick(&mut now, &mut wall, 300);
        harness.round(now, wall).await;

        assert!(
            harness.drain_transitions().is_empty(),
            "no commits of its own: nothing to validate against a stale base"
        );
        let snapshot = harness.snapshot();
        let activity = snapshot.get(&repo).expect("entry");
        assert_eq!(
            activity.staleness,
            StalenessState::Stale,
            "the chip keeps its orange bar"
        );
        assert_eq!(activity.behind_by, Some(2));
    }

    /// #2141, the whole point: the suppressed round records `Current`, the commit
    /// moves `head_sha` and the carry-over hands that `Current` to the new key,
    /// so the notice arrives in the round OF the commit and exactly once.
    ///
    /// The `head_sha` MUST change here. A same-sha version of this test exercises
    /// one key and proves nothing about the carry-over.
    #[tokio::test]
    async fn stale_without_own_commits_notifies_once_after_the_first_own_commit() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        let repo = harness.repo("repo-a");
        publish_branch(&repo, "fix/2141");
        harness.set_work(std::slice::from_ref(&repo));
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.repo_info
                .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
            gh.compare
                .push_back(Ok(ok_output(&compare_body("behind", 2, 0))));
            gh.compare
                .push_back(Ok(ok_output(&compare_body("behind", 2, 1))));
            gh.compare
                .push_back(Ok(ok_output(&compare_body("behind", 5, 1))));
        }

        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;
        assert!(
            harness.drain_transitions().is_empty(),
            "round 1: empty branch, no notice"
        );
        drain(&harness);

        // The first commit of its own: a new `head_sha`, so a new QueryKey.
        harness
            .git
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_head_sha(&repo, 'b');
        tick(&mut now, &mut wall, 300);
        harness.round(now, wall).await;

        let transitions = harness.drain_transitions();
        assert_eq!(
            transitions.len(),
            1,
            "round 2: the branch now has work of its own, and it is behind"
        );
        assert_eq!(transitions[0].kind, TransitionKind::BranchStale);
        assert_eq!(transitions[0].behind_by, Some(2));

        tick(&mut now, &mut wall, 300);
        harness.round(now, wall).await;
        assert!(
            harness.drain_transitions().is_empty(),
            "round 3: still stale on the same key, no second notice"
        );
    }

    /// #2141 closes a defect that predates it: a branch that is current, commits,
    /// and only then falls behind used to observe `None -> Stale` on its new key,
    /// which is not the edge the notice fires on, so it was never told at all.
    #[tokio::test]
    async fn branch_that_commits_while_behind_notifies_after_the_carry_over() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        let repo = harness.repo("repo-a");
        publish_branch(&repo, "fix/2141");
        harness.set_work(std::slice::from_ref(&repo));
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.repo_info
                .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
            gh.compare
                .push_back(Ok(ok_output(&compare_body("identical", 0, 0))));
            gh.compare
                .push_back(Ok(ok_output(&compare_body("behind", 2, 1))));
        }

        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;
        drain(&harness);
        harness
            .git
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_head_sha(&repo, 'b');
        tick(&mut now, &mut wall, 300);
        harness.round(now, wall).await;

        let transitions = harness.drain_transitions();
        assert_eq!(
            transitions.len(),
            1,
            "the carry-over preserves the `Current` the commit used to destroy"
        );
        assert_eq!(transitions[0].behind_by, Some(2));
    }

    /// The other direction of the same carry: a `Stale` that already notified is
    /// carried too, so a commit does not re-open the edge and notify twice. The
    /// #2131 dedup now survives a commit, which it did not before.
    #[tokio::test]
    async fn carry_over_does_not_duplicate_a_notice_across_a_commit() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        let repo = harness.repo("repo-a");
        publish_branch(&repo, "fix/2141");
        harness.set_work(std::slice::from_ref(&repo));
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.repo_info
                .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
            // `ahead 0/1`, not `identical 0/1`: GitHub answers `ahead` for an
            // up-to-date branch that carries commits of its own.
            gh.compare
                .push_back(Ok(ok_output(&compare_body("ahead", 0, 1))));
            gh.compare
                .push_back(Ok(ok_output(&compare_body("behind", 2, 1))));
            gh.compare
                .push_back(Ok(ok_output(&compare_body("behind", 2, 2))));
        }

        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;
        drain(&harness);
        tick(&mut now, &mut wall, 300);
        harness.round(now, wall).await;
        assert_eq!(
            harness.drain_transitions().len(),
            1,
            "the branch is behind with work of its own: one notice"
        );

        harness
            .git
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_head_sha(&repo, 'b');
        tick(&mut now, &mut wall, 300);
        harness.round(now, wall).await;
        assert!(
            harness.drain_transitions().is_empty(),
            "the new key inherits `Stale`, so there is no second edge"
        );
    }

    /// The carry moves the staleness axis ONLY. A new commit has genuinely
    /// unknown CI, and seeding it would assert a run state nobody observed.
    #[tokio::test]
    async fn carry_over_leaves_the_ci_axis_unknown_on_a_new_sha() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings {
            ci_activity_enabled: true,
            ..AppSettings::default()
        });
        let repo = harness.repo("repo-a");
        publish_branch(&repo, "fix/2141");
        harness.set_work(std::slice::from_ref(&repo));
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.repo_info
                .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
            gh.compare
                .push_back(Ok(ok_output(&compare_body("behind", 2, 0))));
            gh.ci.push_back(Ok(ok_output(&ci_rows(
                &sha_of('a'),
                &[("fix/2141", "completed")],
            ))));
        }

        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;
        drain(&harness);
        {
            let state = harness.sweeper.lock_state();
            let (_, entry) = state.keys.iter().next().expect("one key");
            assert_eq!(entry.ci.confirmed, Some(CiState::Idle), "CI answered once");
        }

        harness
            .git
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_head_sha(&repo, 'b');
        tick(&mut now, &mut wall, 300);
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.compare
                .push_back(Ok(ok_output(&compare_body("behind", 2, 0))));
            gh.ci.push_back(Err(FailureKind::Other));
        }
        harness.round(now, wall).await;

        let state = harness.sweeper.lock_state();
        let (_, entry) = state.keys.iter().next().expect("one key");
        assert_eq!(
            entry.staleness.confirmed,
            Some(StalenessState::Current),
            "the staleness axis is carried across the new sha"
        );
        assert_eq!(
            entry.ci.confirmed, None,
            "the CI axis is NOT carried: a new commit has unknown CI"
        );
    }

    /// Two live paths on one `(nwo, branch)` at different commits — worktrees, or
    /// two clones on the same branch. Neither is retired, so neither may donate:
    /// a live key answering for itself must not be overwritten by the other.
    ///
    /// The shape is load-bearing, and the obvious shapes do not test the rule.
    /// One round proves nothing: on the first round `state.keys` is empty, so
    /// there are no donors of any kind and the donor filter is never reached. Two
    /// clean rounds prove nothing either: both keys would hold
    /// `confirmed = Some(..)` and the `confirmed.is_none()` guard would protect
    /// them whatever the filter said. One live key has to sit at
    /// `confirmed = None` while the other holds a `Current`, and a failed compare
    /// is the only way to get there.
    ///
    /// Drop the `!groups.contains_key` filter and B is seeded from A's `Current`,
    /// reaches `Current -> Stale` in round 2 and notifies, so this test fails.
    /// Each key is scripted by its own sha through `route`, because two due keys
    /// are polled concurrently and a shared queue would not be deterministic.
    #[tokio::test]
    async fn two_live_keys_on_one_branch_never_seed_each_other() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        let first = harness.repo("repo-a");
        let second = harness.repo("repo-b");
        publish_branch(&first, "fix/2141");
        publish_branch(&second, "fix/2141");
        harness.set_work(&[first.clone(), second.clone()]);
        {
            let mut git = harness.git.lock().unwrap_or_else(|e| e.into_inner());
            git.set_head_sha(&first, 'a');
            git.set_head_sha(&second, 'b');
        }
        let sha_a = sha_of('a');
        let sha_b = sha_of('b');
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.repo_info
                .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
            // Round 1: A answers and records `Current`. B's compare fails, so B
            // keeps `confirmed = None` while both keys stay live.
            gh.route(
                &format!("repos/mblua/AgentsCommander/compare/HEAD...{sha_a}"),
                Ok(ok_output(&compare_body("ahead", 0, 1))),
            );
            gh.route(
                &format!("repos/mblua/AgentsCommander/compare/HEAD...{sha_b}"),
                Err(FailureKind::Other),
            );
        }

        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;
        drain(&harness);

        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            // Round 2: A unchanged, B behind with work of its own. B's own history
            // is `None`, so `None -> Stale` is not an edge and B stays quiet —
            // unless it was wrongly seeded with A's `Current`.
            gh.route(
                &format!("repos/mblua/AgentsCommander/compare/HEAD...{sha_a}"),
                Ok(ok_output(&compare_body("ahead", 0, 1))),
            );
            gh.route(
                &format!("repos/mblua/AgentsCommander/compare/HEAD...{sha_b}"),
                Ok(ok_output(&compare_body("behind", 3, 1))),
            );
        }
        // 900s, not 300s: B's failed round backs off to 600s, and the rule is
        // only exercised when BOTH keys are due and live in the same round.
        tick(&mut now, &mut wall, 900);
        harness.round(now, wall).await;

        assert!(
            harness.drain_transitions().is_empty(),
            "a live key must not donate to another live key on the same branch"
        );
    }

    /// #2141 fails open on a missing `ahead_by`: the field is a suppression
    /// input, and an absent one is not evidence that the branch is empty.
    #[tokio::test]
    async fn stale_with_a_missing_ahead_by_still_sends_the_notice() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        let repo = harness.repo("repo-a");
        publish_branch(&repo, "fix/2141");
        harness.set_work(std::slice::from_ref(&repo));
        let no_ahead_by = serde_json::json!({ "status": "behind", "behind_by": 4 }).to_string();
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.repo_info
                .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
            gh.compare
                .push_back(Ok(ok_output(&compare_body("identical", 0, 0))));
            gh.compare.push_back(Ok(ok_output(&no_ahead_by)));
        }

        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;
        drain(&harness);
        tick(&mut now, &mut wall, 300);
        harness.round(now, wall).await;

        let transitions = harness.drain_transitions();
        assert_eq!(transitions.len(), 1, "an unknown ahead_by never suppresses");
        assert_eq!(transitions[0].behind_by, Some(4));
    }

    #[tokio::test]
    async fn stale_on_a_feature_branch_still_sends_the_notice() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        let repo = harness.repo("repo-a");
        crate::pty::git_watcher::publish_git_status(
            &repo,
            Some(crate::pty::git_watcher::GitStatus {
                branch: Some("fix/2131".to_string()),
                dirty: false,
            }),
        );
        harness.set_work(std::slice::from_ref(&repo));
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.repo_info
                .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
            gh.compare
                .push_back(Ok(ok_output(&compare_body("identical", 0, 0))));
            gh.compare
                .push_back(Ok(ok_output(&compare_body("behind", 4, 1))));
        }

        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;
        drain(&harness);
        tick(&mut now, &mut wall, 300);
        harness.round(now, wall).await;

        let transitions = harness.drain_transitions();
        assert_eq!(transitions.len(), 1, "a feature branch still notifies");
        assert_eq!(transitions[0].kind, TransitionKind::BranchStale);
        assert_eq!(transitions[0].base_branch, "main");
        assert_eq!(transitions[0].behind_by, Some(4));
        assert_eq!(
            harness.snapshot().get(&repo).expect("entry").staleness,
            StalenessState::Stale
        );
    }

    #[tokio::test]
    async fn stale_with_an_unresolved_default_branch_still_sends_the_notice() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        let repo = harness.repo("repo-a");
        // Overwrite `Harness::repo`'s `main` with the sentinel literal itself: an
        // unresolved base and a branch equal to that base are the one pair a bare
        // `key.branch == base_label` comparison would suppress, so this test fails
        // if `base_label != DEFAULT_BRANCH_LABEL &&` is removed. Production cannot
        // reach this pair (git refnames forbid spaces), which is why publishing it
        // is safe here and why the sentinel guard is the only thing under test.
        crate::pty::git_watcher::publish_git_status(
            &repo,
            Some(crate::pty::git_watcher::GitStatus {
                branch: Some(DEFAULT_BRANCH_LABEL.to_string()),
                dirty: false,
            }),
        );
        harness.set_work(std::slice::from_ref(&repo));
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            // No repo_info and no symbolic-ref answer is queued, so the default
            // branch stays unresolved and the sentinel is the base label; the
            // published branch equals that sentinel, so the sentinel guard is the
            // only thing keeping the notice alive.
            gh.compare
                .push_back(Ok(ok_output(&compare_body("identical", 0, 0))));
            gh.compare
                .push_back(Ok(ok_output(&compare_body("behind", 4, 1))));
        }

        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;
        drain(&harness);
        tick(&mut now, &mut wall, 300);
        harness.round(now, wall).await;

        let transitions = harness.drain_transitions();
        assert_eq!(
            transitions.len(),
            1,
            "an unresolved default branch cannot suppress"
        );
        assert_eq!(transitions[0].kind, TransitionKind::BranchStale);
        assert_eq!(transitions[0].base_branch, DEFAULT_BRANCH_LABEL);
        // Companion premise assertion: the branch this notice was produced for is
        // the sentinel itself, so the test cannot pass on a live `main` by accident.
        assert_eq!(transitions[0].branch, DEFAULT_BRANCH_LABEL);
        assert_eq!(transitions[0].behind_by, Some(4));
    }

    fn drain(harness: &Harness) {
        let _ = harness.drain_transitions();
    }

    /// #2329: two rooms sharing one repo and one commit on the resolved default
    /// branch behave identically - both stay idle, neither gets a CI notice -
    /// while the shared key still costs exactly one CI query per round.
    #[tokio::test]
    async fn two_rooms_on_one_default_commit_are_one_call_and_no_ci_transition() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        let first = harness.repo("repo-a");
        let second = harness.repo("repo-b");
        harness.set_work(&[first.clone(), second.clone()]);
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.repo_info
                .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
            gh.ci
                .push_back(Ok(ok_output(&ci_body(&sha_of('a'), &["in_progress"]))));
            gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
        }

        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;
        let first_round_calls = harness
            .gh
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .count("/actions/runs?");
        assert_eq!(first_round_calls, 1, "one call for one commit");

        tick(&mut now, &mut wall, 60);
        harness.round(now, wall).await;

        assert!(
            harness.drain_transitions().is_empty(),
            "the resolved default branch emits no CI notice for either room"
        );
        assert_eq!(
            harness
                .gh
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .count("/actions/runs?"),
            2,
            "one call per round for the shared key"
        );
        let snapshot = harness.snapshot();
        for path in [&first, &second] {
            assert_eq!(
                snapshot.get(path).expect("entry").ci,
                CiState::Idle,
                "both rooms publish the same idle chip"
            );
        }
    }

    #[tokio::test]
    async fn backoff_is_per_key_and_per_axis() {
        let _guard = round_test_lock().await;
        let settings = AppSettings {
            ci_sweep_min_interval_secs: 30,
            branch_staleness_interval_secs: 300,
            ..AppSettings::default()
        };
        let harness = Harness::new(settings);
        let first = harness.repo("repo-a");
        let second = harness.repo("repo-b");
        harness.set_work(&[first.clone(), second.clone()]);
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            for _ in 0..4 {
                gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
            }
            // The failing repository is scripted by endpoint, so the assertion
            // cannot depend on which of two concurrent queries is polled first.
            // Only the compare calls for the failing nwo fail, so a routed marker
            // cannot be consumed by the base-label or CI calls.
            gh.route(
                &format!("repos/mblua/failing/compare/HEAD...{}", sha_of('a')),
                Err(FailureKind::Timeout),
            );
            gh.route(
                &format!("repos/mblua/failing/compare/HEAD...{}", sha_of('a')),
                Err(FailureKind::Timeout),
            );
        }
        {
            let mut git = harness.git.lock().unwrap_or_else(|e| e.into_inner());
            git.set_origin(&first, "git@github.com:mblua/failing.git");
            git.set_origin(&second, "git@github.com:mblua/healthy.git");
        }

        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;
        assert_eq!(compare_count(&harness, "failing"), 1);
        assert_eq!(compare_count(&harness, "healthy"), 1);

        tick(&mut now, &mut wall, 29);
        harness.round(now, wall).await;
        eprintln!(
            "DEBUG calls: {:?}",
            harness.gh.lock().unwrap_or_else(|e| e.into_inner()).calls
        );
        assert_eq!(
            ci_count(&harness),
            2,
            "two keys, no CI query before their due time"
        );

        tick(&mut now, &mut wall, 1);
        harness.round(now, wall).await;
        assert_eq!(
            ci_count(&harness),
            4,
            "the failing staleness axis must not disturb the CI cadence"
        );
        assert_eq!(
            compare_count(&harness, "failing"),
            1,
            "the failing key doubles: 300 -> 600 and is not due yet"
        );

        tick(&mut now, &mut wall, 569);
        harness.round(now, wall).await;
        assert_eq!(
            compare_count(&harness, "failing"),
            1,
            "not queried at due - 1s"
        );

        tick(&mut now, &mut wall, 1);
        harness.round(now, wall).await;
        assert_eq!(
            compare_count(&harness, "failing"),
            2,
            "queried at the doubled interval"
        );
        assert_eq!(
            compare_count(&harness, "healthy"),
            2,
            "the healthy key follows its own cadence"
        );
    }

    fn ci_count(harness: &Harness) -> usize {
        harness
            .gh
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .count("/actions/runs?")
    }

    fn compare_count(harness: &Harness, nwo: &str) -> usize {
        harness
            .gh
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .calls
            .iter()
            .filter(|args| {
                args.get(1)
                    .is_some_and(|target| target.contains("/compare/") && target.contains(nwo))
            })
            .count()
    }

    #[tokio::test]
    async fn rate_limit_jumps_straight_to_the_cap() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        let repo = harness.repo("repo-a");
        harness.set_work(&[repo]);
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
            gh.ci.push_back(Ok(GhCallOutput {
                stdout: String::new(),
                stderr: "gh: API rate limit exceeded for user ID 1 (HTTP 403)".to_string(),
                success: false,
            }));
        }

        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;
        tick(&mut now, &mut wall, 30);
        harness.round(now, wall).await;

        let key = QueryKey {
            nwo: "mblua/AgentsCommander".to_string(),
            sha40: sha_of('a'),
            branch: "main".to_string(),
        };
        let due_at = harness
            .sweeper
            .lock_state()
            .keys
            .get(&key)
            .expect("key state")
            .ci
            .next_due
            .expect("due time");
        assert_eq!(due_at, now + Duration::from_secs(900));
    }

    // --- #2152: the global call budget, the priority order and the gate ---

    fn secondary_limit_output() -> GhCallOutput {
        GhCallOutput {
            stdout: String::new(),
            stderr: "You have exceeded a secondary rate limit. Please wait a few minutes before you try again.".to_string(),
            success: false,
        }
    }

    fn primary_limit_output() -> GhCallOutput {
        GhCallOutput {
            stdout: String::new(),
            stderr: "gh: API rate limit exceeded for user ID 1 (HTTP 403)".to_string(),
            success: false,
        }
    }

    fn ci_count_for(harness: &Harness, nwo: &str) -> usize {
        harness
            .gh
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .count(&format!("{nwo}/actions/runs"))
    }

    fn throttled_until(harness: &Harness) -> Option<Instant> {
        harness.sweeper.lock_state().throttled_until
    }

    fn ci_next_due(harness: &Harness, nwo: &str) -> Option<Instant> {
        let key = QueryKey {
            nwo: format!("mblua/{nwo}"),
            sha40: sha_of('a'),
            branch: "main".to_string(),
        };
        harness
            .sweeper
            .lock_state()
            .keys
            .get(&key)
            .and_then(|entry| entry.ci.next_due)
    }

    /// Two repos, two distinct `nwo`, so `compare_count` can separate them.
    fn two_repos(harness: &Harness) -> (String, String) {
        let first = harness.repo("a");
        let second = harness.repo("b");
        harness.set_work(&[first.clone(), second.clone()]);
        {
            let mut git = harness.git.lock().unwrap_or_else(|e| e.into_inner());
            git.set_origin(&first, "git@github.com:mblua/repo-a.git");
            git.set_origin(&second, "git@github.com:mblua/repo-b.git");
        }
        (first, second)
    }

    #[tokio::test]
    async fn secondary_rate_limit_backs_off_one_minute() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        let repo = harness.repo("repo-a");
        harness.set_work(&[repo]);
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
            gh.ci.push_back(Ok(secondary_limit_output()));
        }

        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;
        tick(&mut now, &mut wall, 30);
        harness.round(now, wall).await;

        let key = QueryKey {
            nwo: "mblua/AgentsCommander".to_string(),
            sha40: sha_of('a'),
            branch: "main".to_string(),
        };
        let due_at = harness
            .sweeper
            .lock_state()
            .keys
            .get(&key)
            .expect("key state")
            .ci
            .next_due
            .expect("due time");
        assert_eq!(
            due_at,
            now + Duration::from_secs(60),
            "a secondary limit is a burst signal, not an exhausted quota"
        );
    }

    /// #2329: a failing CI query on the resolved default branch keeps the chip
    /// idle instead of falling back to Unknown, but the failure itself is not
    /// swallowed: the per-key backoff and the round gate match the kind, and the
    /// early next round issues no call at all. `branchStalenessEnabled` is off
    /// so the ONLY failure kind in play is the CI one.
    #[tokio::test]
    async fn default_branch_ci_failure_keeps_idle_chip_and_arms_the_backoff() {
        let _guard = round_test_lock().await;
        for (failure, expected) in [
            (primary_limit_output(), BACKOFF_CAP),
            (secondary_limit_output(), SECONDARY_BACKOFF_BASE),
        ] {
            let harness = Harness::new(AppSettings {
                branch_staleness_enabled: false,
                ..AppSettings::default()
            });
            harness.set_jitter(0.0);
            let repo = harness.repo("repo-a");
            harness.set_work(std::slice::from_ref(&repo));
            {
                let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
                gh.repo_info
                    .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
                gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
            }

            let mut now = Instant::now();
            let mut wall = Local::now();
            harness.round(now, wall).await;
            assert_eq!(
                harness.snapshot().get(&repo).expect("entry").ci,
                CiState::Idle,
                "round 1: the resolved default branch is suppressed"
            );
            assert!(harness.drain_transitions().is_empty());

            harness
                .gh
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .ci
                .push_back(Ok(failure));
            tick(&mut now, &mut wall, 60);
            harness.round(now, wall).await;

            assert_eq!(
                harness.snapshot().get(&repo).expect("entry").ci,
                CiState::Idle,
                "a failed default-branch CI query keeps the idle chip"
            );
            assert!(
                harness.drain_transitions().is_empty(),
                "no CI edge is ever published for the default branch"
            );
            let key = QueryKey {
                nwo: "mblua/AgentsCommander".to_string(),
                sha40: sha_of('a'),
                branch: "main".to_string(),
            };
            {
                let state = harness.sweeper.lock_state();
                let entry = state.keys.get(&key).expect("key state");
                assert_eq!(
                    entry.ci.failure_interval,
                    Some(expected),
                    "the failure kind schedules its own retry"
                );
                assert_eq!(
                    entry.ci.next_due,
                    Some(now + expected),
                    "the per-key retry is due on the failure interval"
                );
                assert_eq!(entry.ci.confirmed, None, "confirmed stays cleared");
                assert!(
                    state.warned_failures.contains(&(key.clone(), Axis::Ci)),
                    "warn_failure ran for the CI axis"
                );
                assert_eq!(
                    state.throttled_until,
                    Some(now + expected),
                    "the round gate is armed for the kind: 900s primary, 60s secondary"
                );
            }

            let calls_before = harness.gh_call_count();
            tick(&mut now, &mut wall, 30);
            harness.round(now, wall).await;
            assert_eq!(
                harness.gh_call_count(),
                calls_before,
                "the early next round is gated and issues no call"
            );
            assert!(
                harness.drain_transitions().is_empty(),
                "a gated round changes no chip"
            );
        }
    }

    #[test]
    fn failure_kind_classifies_the_three_403_texts() {
        assert_eq!(
            failure_kind(&primary_limit_output()),
            FailureKind::RateLimited
        );
        assert_eq!(
            failure_kind(&secondary_limit_output()),
            FailureKind::SecondaryRateLimited
        );
        assert_eq!(
            failure_kind(&GhCallOutput {
                stdout: String::new(),
                stderr: "You have triggered an abuse detection mechanism".to_string(),
                success: false,
            }),
            FailureKind::SecondaryRateLimited
        );
    }

    #[tokio::test]
    async fn budget_starved_key_is_not_queried_and_keeps_its_chip() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        let _paths = two_repos(&harness);
        // One resolution plus two axis queries: exactly one key fits.
        harness.set_budget(3.0);

        let now = Instant::now();
        let wall = Local::now();
        harness.round(now, wall).await;

        assert_eq!(
            compare_count(&harness, "repo-b"),
            0,
            "the starved key must issue no gh call at all"
        );
        assert_eq!(ci_count_for(&harness, "repo-b"), 0);
        assert_eq!(
            ci_count(&harness),
            1,
            "exactly the admitted key's CI query was issued"
        );
        assert!(
            ci_next_due(&harness, "repo-b").is_none(),
            "a key without budget never fails and never gets a due time"
        );
        assert_eq!(
            harness
                .snapshot()
                .values()
                .filter(|activity| activity.ci != CiState::Unknown)
                .count(),
            1,
            "the starved path keeps its transparent chip"
        );

        // Positive control: the identical fixture with a full bucket queries both.
        let full = Harness::new(AppSettings::default());
        let _paths = two_repos(&full);
        full.set_budget(GH_BUDGET_BURST);
        full.round(now, wall).await;
        assert_eq!(ci_count_for(&full, "repo-a"), 1);
        assert_eq!(ci_count_for(&full, "repo-b"), 1);
    }

    #[tokio::test]
    async fn running_ci_wins_the_last_token() {
        let _guard = round_test_lock().await;
        for budget in [4.0, 1.0] {
            let harness = Harness::new(AppSettings::default());
            let first = harness.repo("a");
            let second = harness.repo("z");
            harness.set_work(&[first.clone(), second.clone()]);
            {
                let mut git = harness.git.lock().unwrap_or_else(|e| e.into_inner());
                git.set_origin(&first, "git@github.com:mblua/repo-a.git");
                git.set_origin(&second, "git@github.com:mblua/repo-z.git");
            }
            {
                let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
                gh.route(
                    &format!(
                        "repos/mblua/repo-z/actions/runs?head_sha={}&per_page=100",
                        sha_of('a')
                    ),
                    Ok(ok_output(&ci_body(&sha_of('a'), &["in_progress"]))),
                );
                if budget == 4.0 {
                    gh.route(
                        &format!(
                            "repos/mblua/repo-z/actions/runs?head_sha={}&per_page=100",
                            sha_of('a')
                        ),
                        Ok(ok_output(&ci_body(&sha_of('a'), &["in_progress"]))),
                    );
                }
            }

            let mut now = Instant::now();
            let mut wall = Local::now();
            harness.round(now, wall).await;
            assert_eq!(ci_count_for(&harness, "repo-a"), 1);
            assert_eq!(ci_count_for(&harness, "repo-z"), 1);

            // The `Running` key is made to lose every fallback: it is last
            // alphabetically and it was confirmed most recently.
            {
                let mut state = harness.sweeper.lock_state();
                let key = QueryKey {
                    nwo: "mblua/repo-z".to_string(),
                    sha40: sha_of('a'),
                    branch: "main".to_string(),
                };
                let entry = state.keys.get_mut(&key).expect("running key state");
                assert_eq!(entry.ci.confirmed, Some(CiState::Running));
                entry.ci.last_confirmed_at = Some(wall + chrono::Duration::seconds(1));
            }

            tick(&mut now, &mut wall, 30);
            // Both nwo are resolved, staleness is not due: one token, one CI query.
            harness.set_budget(budget);
            harness.round(now, wall).await;

            assert_eq!(
                ci_count_for(&harness, "repo-z"),
                if budget == 4.0 { 2 } else { 1 },
                "a Running key requires the full reservation"
            );
            assert_eq!(
                ci_count_for(&harness, "repo-a"),
                if budget == 4.0 { 1 } else { 2 },
                "an affordable Idle key can proceed when Running cannot pay"
            );
            assert_eq!(
                harness
                    .gh
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .count("/actions/runs/"),
                0
            );
            if budget == 4.0 {
                assert_eq!(harness.sweeper.lock_state().budget_tokens, 3.0);
            }
        }
    }

    #[tokio::test]
    async fn a_secondary_limit_silences_the_next_round() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        harness.set_jitter(0.0);
        let _paths = two_repos(&harness);
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.route(
                &format!(
                    "repos/mblua/repo-a/actions/runs?head_sha={}&per_page=100",
                    sha_of('a')
                ),
                Ok(secondary_limit_output()),
            );
            gh.route(
                &format!(
                    "repos/mblua/repo-a/actions/runs?head_sha={}&per_page=100",
                    sha_of('a')
                ),
                Ok(ok_output(&ci_body(&sha_of('a'), &[]))),
            );
        }

        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;
        let after_first = harness.gh_call_count();
        let emitted = harness.emitted_count();
        assert_eq!(
            throttled_until(&harness),
            Some(now + Duration::from_secs(60)),
            "the gate is armed for the whole process, not just the failing key"
        );
        assert!(ci_count_for(&harness, "repo-b") > 0);
        let healthy_before = ci_count_for(&harness, "repo-b");

        tick(&mut now, &mut wall, 30);
        harness.round(now, wall).await;
        assert_eq!(
            harness.gh_call_count(),
            after_first,
            "the healthy key was due, and the gate still issued nothing"
        );
        assert_eq!(
            harness.emitted_count(),
            emitted,
            "a gated round changes no chip"
        );

        tick(&mut now, &mut wall, 31);
        harness.round(now, wall).await;
        assert!(
            ci_count_for(&harness, "repo-b") > healthy_before,
            "the gate opens after the armed interval"
        );
    }

    #[tokio::test]
    async fn jitter_spans_the_half_open_range() {
        let _guard = round_test_lock().await;
        let now = Instant::now();
        let wall = Local::now();

        let low = Harness::new(AppSettings::default());
        low.set_jitter(0.0);
        let repo = low.repo("repo-a");
        low.set_work(&[repo]);
        low.gh
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .ci
            .push_back(Ok(secondary_limit_output()));
        low.round(now, wall).await;
        assert_eq!(
            throttled_until(&low),
            Some(now + SECONDARY_BACKOFF_BASE),
            "the low end of the range is closed"
        );

        let high = Harness::new(AppSettings::default());
        high.set_jitter(1.0 - f64::EPSILON);
        let repo = high.repo("repo-a");
        high.set_work(&[repo]);
        high.gh
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .ci
            .push_back(Ok(secondary_limit_output()));
        high.round(now, wall).await;
        let gate = throttled_until(&high).expect("armed gate");
        assert!(gate > now + SECONDARY_BACKOFF_BASE);
        assert!(
            gate < now + 2 * SECONDARY_BACKOFF_BASE,
            "the high end of the range is open"
        );
    }

    #[tokio::test]
    async fn a_primary_limit_closes_the_gate_for_fifteen_minutes() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        harness.set_jitter(0.0);
        let _paths = two_repos(&harness);
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.route(
                &format!(
                    "repos/mblua/repo-a/actions/runs?head_sha={}&per_page=100",
                    sha_of('a')
                ),
                Ok(primary_limit_output()),
            );
            gh.route(
                &format!(
                    "repos/mblua/repo-a/actions/runs?head_sha={}&per_page=100",
                    sha_of('a')
                ),
                Ok(ok_output(&ci_body(&sha_of('a'), &[]))),
            );
        }

        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;
        assert_eq!(
            throttled_until(&harness),
            Some(now + BACKOFF_CAP),
            "an exhausted quota closes the gate for the cap"
        );
        let after_first = harness.gh_call_count();
        let healthy_before = ci_count_for(&harness, "repo-b");

        tick(&mut now, &mut wall, 899);
        harness.round(now, wall).await;
        assert_eq!(harness.gh_call_count(), after_first, "still gated at +899s");

        tick(&mut now, &mut wall, 2);
        harness.round(now, wall).await;
        assert!(
            ci_count_for(&harness, "repo-b") > healthy_before,
            "the healthy key resumes once the gate opens"
        );
    }

    const BUDGET_FIXTURE_REPOS: usize = 40;

    /// The worst case: 40 distinct `nwo`, CI only, every key on a non-default
    /// branch that has runs, so each key costs a resolution, a query and an
    /// identity compare on its first round.
    fn budget_fixture() -> (Harness, Vec<String>) {
        let settings = AppSettings {
            branch_staleness_enabled: false,
            ci_sweep_min_interval_secs: INTERVAL_FLOOR_SECS,
            ..AppSettings::default()
        };
        let harness = Harness::new(settings);
        let mut paths = Vec::with_capacity(BUDGET_FIXTURE_REPOS);
        for index in 0..BUDGET_FIXTURE_REPOS {
            let name = format!("repo-{index:02}");
            let path = harness.repo(&name);
            crate::pty::git_watcher::publish_git_status(
                &path,
                Some(crate::pty::git_watcher::GitStatus {
                    branch: Some("feature".to_string()),
                    dirty: false,
                }),
            );
            harness
                .git
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .set_origin(&path, &format!("git@github.com:mblua/{name}.git"));
            paths.push(path);
        }
        harness.set_work(&paths);
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            for _ in 0..(BUDGET_FIXTURE_REPOS * 40) {
                gh.repo_info.push_back(Ok(ok_output(
                    &serde_json::json!({ "default_branch": "main" }).to_string(),
                )));
                gh.ci.push_back(Ok(ok_output(&ci_rows(
                    &sha_of('a'),
                    &[("feature", "completed")],
                ))));
            }
        }
        (harness, paths)
    }

    fn fixture_nwo(index: usize) -> String {
        format!("repo-{index:02}")
    }

    #[tokio::test]
    async fn sustained_spend_never_exceeds_the_ceiling() {
        let _guard = round_test_lock().await;
        let (harness, _paths) = budget_fixture();

        let mut now = Instant::now();
        let mut wall = Local::now();
        let mut totals: Vec<usize> = Vec::new();
        for step in 0..12 {
            if step > 0 {
                tick(&mut now, &mut wall, INTERVAL_FLOOR_SECS);
            }
            harness.round(now, wall).await;
            totals.push(harness.gh_call_count());
        }

        // Every call is reserved before it is issued and the bucket is clamped,
        // so a window of T seconds can spend at most one burst plus T's refill.
        let bound_60 = GH_BUDGET_BURST + 60.0 * MAX_GH_CALLS_PER_MINUTE / 60.0;
        let bound_120 = GH_BUDGET_BURST + 120.0 * MAX_GH_CALLS_PER_MINUTE / 60.0;
        let first_minute = totals[5];
        let second_minute = totals[11] - totals[5];
        assert!(
            first_minute as f64 <= bound_60,
            "{first_minute} calls in [0, 60) exceeds {bound_60}"
        );
        assert!(
            second_minute as f64 <= bound_60,
            "{second_minute} calls in [60, 120) exceeds {bound_60}"
        );
        assert!(
            totals[11] as f64 <= bound_120,
            "{} calls in [0, 120) exceeds {bound_120}",
            totals[11]
        );
        // Positive control: a sweeper that issues nothing must not pass.
        assert!(totals[11] >= 60, "only {} calls issued", totals[11]);
    }

    #[tokio::test]
    async fn starved_keys_are_served_in_a_later_round() {
        let _guard = round_test_lock().await;
        let (harness, _paths) = budget_fixture();

        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;

        let first_unqueried = (0..BUDGET_FIXTURE_REPOS)
            .map(fixture_nwo)
            .find(|nwo| ci_count_for(&harness, nwo) == 0)
            .expect("the cold-start bucket cannot pay for every key");

        for _ in 0..6 {
            tick(&mut now, &mut wall, INTERVAL_FLOOR_SECS);
            harness.round(now, wall).await;
        }
        assert!(
            ci_count_for(&harness, &first_unqueried) > 0,
            "{first_unqueried} was first in line and still unserved after a minute"
        );

        for _ in 0..30 {
            tick(&mut now, &mut wall, INTERVAL_FLOOR_SECS);
            harness.round(now, wall).await;
        }
        for index in 0..BUDGET_FIXTURE_REPOS {
            let nwo = fixture_nwo(index);
            assert!(
                ci_count_for(&harness, &nwo) > 0,
                "{nwo} was never queried in 360s"
            );
        }
    }

    #[test]
    fn missing_gh_spawns_no_process() {
        let probes = Arc::new(AtomicUsize::new(0));
        let spawns = Arc::new(AtomicUsize::new(0));
        let probe: GhProbe = {
            let probes = Arc::clone(&probes);
            Arc::new(move || {
                probes.fetch_add(1, Ordering::SeqCst);
                None
            })
        };
        let spawner: GhSpawner = {
            let spawns = Arc::clone(&spawns);
            Arc::new(move |_spec| {
                spawns.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Err(FailureKind::Other) })
            })
        };
        let local_git: LocalGitRunner =
            Arc::new(|_path, _args| Box::pin(async { Err("unused".to_string()) }));
        let settings: SettingsState = Arc::new(tokio::sync::RwLock::new(AppSettings::default()));
        let sessions = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let (sweeper, _receiver) = RemoteSweeper::new(
            sessions,
            settings,
            Box::new(|_payload| {}),
            RemoteSweeperSeams {
                probe,
                spawner,
                local_git,
                jitter: Arc::new(|| 0.0),
                snapshot_dir: None,
                publication_clock: Arc::new(Utc::now),
            },
        );
        let shutdown = ShutdownSignal::new();

        let handle = sweeper.start(shutdown).expect("thread handle");
        handle.join().expect("thread returns after one probe");
        assert_eq!(probes.load(Ordering::SeqCst), 1, "never probes again");
        assert_eq!(spawns.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn start_does_no_io_on_the_callers_thread() {
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let spawns = Arc::new(AtomicUsize::new(0));
        let probe: GhProbe = {
            let barrier = Arc::clone(&barrier);
            Arc::new(move || {
                let _ = entered_tx.send(());
                barrier.wait();
                Some(PathBuf::from("gh"))
            })
        };
        let spawner: GhSpawner = {
            let spawns = Arc::clone(&spawns);
            Arc::new(move |_spec| {
                spawns.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Err(FailureKind::Other) })
            })
        };
        let local_git: LocalGitRunner =
            Arc::new(|_path, _args| Box::pin(async { Err("unused".to_string()) }));
        let settings: SettingsState = Arc::new(tokio::sync::RwLock::new(AppSettings::default()));
        let sessions = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let (sweeper, _receiver) = RemoteSweeper::new(
            sessions,
            settings,
            Box::new(|_payload| {}),
            RemoteSweeperSeams {
                probe,
                spawner,
                local_git,
                jitter: Arc::new(|| 0.0),
                snapshot_dir: None,
                publication_clock: Arc::new(Utc::now),
            },
        );
        let shutdown = ShutdownSignal::new();

        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let started_sweeper = Arc::clone(&sweeper);
        let started_shutdown = shutdown.clone();
        let starter = std::thread::spawn(move || {
            let handle = started_sweeper.start(started_shutdown);
            started_tx.send(handle.is_some()).expect("send");
            handle
        });

        started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("start must return while the probe is still blocked");
        entered_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the probe runs on the sweeper thread");
        assert_eq!(
            spawns.load(Ordering::SeqCst),
            0,
            "start must not spawn anything on the caller's thread"
        );

        barrier.wait();
        shutdown.trigger();
        let handle = starter.join().expect("starter thread").expect("handle");
        handle.join().expect("sweeper thread exits on shutdown");
    }

    #[test]
    fn both_features_disabled_does_not_spawn_the_thread() {
        let probe_calls = Arc::new(AtomicUsize::new(0));
        let probe: GhProbe = {
            let probe_calls = Arc::clone(&probe_calls);
            Arc::new(move || {
                probe_calls.fetch_add(1, Ordering::SeqCst);
                Some(PathBuf::from("gh"))
            })
        };
        let spawner: GhSpawner = Arc::new(|_spec| Box::pin(async { Err(FailureKind::Other) }));
        let local_git: LocalGitRunner =
            Arc::new(|_path, _args| Box::pin(async { Err("unused".to_string()) }));
        let settings = AppSettings {
            ci_activity_enabled: false,
            branch_staleness_enabled: false,
            ..AppSettings::default()
        };
        let settings: SettingsState = Arc::new(tokio::sync::RwLock::new(settings));
        let sessions = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let (sweeper, _receiver) = RemoteSweeper::new(
            sessions,
            settings,
            Box::new(|_payload| {}),
            RemoteSweeperSeams {
                probe,
                spawner,
                local_git,
                jitter: Arc::new(|| 0.0),
                snapshot_dir: None,
                publication_clock: Arc::new(Utc::now),
            },
        );

        assert!(sweeper.start(ShutdownSignal::new()).is_none());
        assert_eq!(probe_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn disabled_axis_is_skipped_in_the_round() {
        let _guard = round_test_lock().await;
        let settings = AppSettings {
            ci_activity_enabled: false,
            ..AppSettings::default()
        };
        let harness = Harness::new(settings);
        let repo = harness.repo("repo-a");
        harness.set_work(&[repo]);

        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;
        tick(&mut now, &mut wall, 300);
        harness.round(now, wall).await;

        let gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(gh.count("/actions/runs?"), 0);
        assert!(gh.count("/compare/") > 0, "staleness still runs");
    }

    #[test]
    fn clamps_hold_at_the_floor_and_ceiling() {
        assert_eq!(clamp_interval_secs("ci", 0), 10);
        assert_eq!(clamp_interval_secs("ci", 99999), 3600);
        assert_eq!(clamp_interval_secs("staleness", 0), 10);
        assert_eq!(clamp_interval_secs("staleness", 99999), 3600);
        assert_eq!(clamp_interval_secs("ci", 30), 30);
    }

    #[tokio::test]
    async fn snapshot_is_keyed_by_the_exact_unnormalized_path() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        harness.repo("repo-b");
        let unnormalized = format!(
            "{}/./repo-b",
            harness.temp.path().to_string_lossy().replace('\\', "/")
        );
        crate::pty::git_watcher::publish_git_status(
            &unnormalized,
            Some(crate::pty::git_watcher::GitStatus {
                branch: Some("main".to_string()),
                dirty: false,
            }),
        );
        harness.set_work(std::slice::from_ref(&unnormalized));

        harness.round(Instant::now(), Local::now()).await;

        let snapshot = harness.snapshot();
        assert!(
            snapshot.contains_key(&unnormalized),
            "the exact path string must be the key, never a normalized form"
        );
    }

    #[tokio::test]
    async fn gc_removes_a_path_that_left_the_work_list() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        let first = harness.repo("repo-a");
        let second = harness.repo("repo-b");
        harness.set_work(&[first.clone(), second.clone()]);
        harness.round(Instant::now(), Local::now()).await;
        assert_eq!(harness.snapshot().len(), 2);

        harness.set_work(std::slice::from_ref(&first));
        harness.round(Instant::now(), Local::now()).await;

        let snapshot = harness.snapshot();
        assert!(snapshot.contains_key(&first));
        assert!(!snapshot.contains_key(&second));
    }

    #[tokio::test]
    async fn gc_runs_on_an_empty_work_list() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        let first = harness.repo("repo-a");
        let second = harness.repo("repo-b");
        harness.set_work(&[first.clone(), second.clone()]);
        harness.round(Instant::now(), Local::now()).await;
        assert_eq!(harness.snapshot().len(), 2);
        let emitted_before = harness.emitted_count();

        harness.set_work(&[]);
        harness.round(Instant::now(), Local::now()).await;

        assert!(harness.snapshot().is_empty());
        assert_eq!(
            harness.emitted_count(),
            emitted_before + 1,
            "the emptied payload is emitted exactly once"
        );
    }

    #[test]
    fn payload_wire_shape_is_camel_case() {
        let payload = RemoteActivityPayload {
            repo_paths: vec!["C:/wg/repo-a".to_string()],
            ci_states: vec![CiState::Running],
            staleness_states: vec![StalenessState::Stale],
            behind_by: vec![Some(3)],
        };

        assert_eq!(
            serde_json::to_value(&payload).expect("serialize"),
            serde_json::json!({
                "repoPaths": ["C:/wg/repo-a"],
                "ciStates": ["running"],
                "stalenessStates": ["stale"],
                "behindBy": [3],
            })
        );
    }

    #[tokio::test]
    async fn payload_vectors_are_the_same_length_and_sorted_by_path() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        let first = harness.repo("repo-a");
        let second = harness.repo("repo-b");
        // Deliberately reversed so the sort has something to do.
        harness.set_work(&[second.clone(), first.clone()]);

        harness.round(Instant::now(), Local::now()).await;

        let emitted = harness.emitted.lock().unwrap_or_else(|e| e.into_inner());
        let payload = emitted.last().expect("payload");
        assert_eq!(payload.repo_paths, vec![first, second]);
        assert_eq!(payload.repo_paths.len(), payload.ci_states.len());
        assert_eq!(payload.repo_paths.len(), payload.staleness_states.len());
        assert_eq!(payload.repo_paths.len(), payload.behind_by.len());
    }

    #[tokio::test]
    async fn event_is_gated_on_whole_payload_equality() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        let repo = harness.repo("repo-a");
        harness.set_work(&[repo]);
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
            gh.ci
                .push_back(Ok(ok_output(&ci_body(&sha_of('a'), &["in_progress"]))));
        }

        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;
        assert_eq!(harness.emitted_count(), 1);

        // Nothing due: identical payload, no second emission.
        harness.round(now, wall).await;
        assert_eq!(harness.emitted_count(), 1);

        tick(&mut now, &mut wall, 60);
        harness.round(now, wall).await;
        assert_eq!(
            harness.emitted_count(),
            2,
            "a single state flip emits again"
        );
    }

    #[tokio::test]
    async fn transition_channel_full_drops_and_keeps_publishing() {
        let _guard = round_test_lock().await;
        let harness = Harness::with_capacity(AppSettings::default(), 1);
        let first = harness.repo("repo-a");
        let second = harness.repo("repo-b");
        harness.set_work(&[first.clone(), second.clone()]);
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.ci
                .push_back(Ok(ok_output(&ci_body(&sha_of('a'), &["in_progress"]))));
            gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
            gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
        }

        queue_completion(
            &harness,
            "mblua/AgentsCommander",
            run_row(1, &sha_of('a'), "main", "completed", &[]),
        );
        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;
        tick(&mut now, &mut wall, 60);
        harness.round(now, wall).await;

        assert!(harness.try_recv_transition().is_none());
        tick(&mut now, &mut wall, 10);
        harness.round(now, wall).await;
        assert!(harness.try_recv_transition().is_some());
        assert!(harness.try_recv_transition().is_none(), "one was dropped");
        let snapshot = harness.snapshot();
        assert_eq!(snapshot.get(&first).expect("first").ci, CiState::Idle);
        assert_eq!(snapshot.get(&second).expect("second").ci, CiState::Idle);
        assert_eq!(harness.emitted_count(), 2, "publishing continued");
    }

    #[tokio::test]
    async fn transition_channel_closed_drops_and_keeps_publishing() {
        let _guard = round_test_lock().await;
        let harness = Harness::with_capacity(AppSettings::default(), 4);
        let repo = harness.repo("repo-a");
        harness.set_work(std::slice::from_ref(&repo));
        harness.close_transitions();
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
            gh.ci
                .push_back(Ok(ok_output(&ci_body(&sha_of('a'), &["in_progress"]))));
            gh.ci
                .push_back(Ok(ok_output(&ci_body(&sha_of('a'), &["in_progress"]))));
        }

        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;
        tick(&mut now, &mut wall, 60);
        harness.round(now, wall).await;

        assert!(harness.sweeper.lock_state().closed_warned);
        assert_eq!(
            harness.snapshot().get(&repo).expect("entry").ci,
            CiState::Running
        );

        tick(&mut now, &mut wall, 60);
        harness.round(now, wall).await;
        assert!(
            harness.sweeper.lock_state().closed_warned,
            "the latch stays set after a second round"
        );
        assert_eq!(harness.emitted_count(), 2, "publishing continued");
    }

    #[tokio::test]
    async fn ci_transitions_carry_an_empty_base_branch() {
        let _guard = round_test_lock().await;
        let settings = AppSettings {
            branch_staleness_enabled: false,
            ..AppSettings::default()
        };
        let harness = Harness::new(settings);
        let repo = harness.repo("repo-a");
        harness.set_work(&[repo]);
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
            gh.ci
                .push_back(Ok(ok_output(&ci_body(&sha_of('a'), &["in_progress"]))));
            gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
            gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
        }

        queue_completion(
            &harness,
            "mblua/AgentsCommander",
            run_row(1, &sha_of('a'), "main", "completed", &[]),
        );
        let mut now = Instant::now();
        let mut wall = Local::now();
        for _ in 0..4 {
            harness.round(now, wall).await;
            tick(&mut now, &mut wall, 60);
        }

        let transitions = harness.drain_transitions();
        assert_eq!(transitions.len(), 2);
        assert_eq!(transitions[0].kind, TransitionKind::CiStarted);
        assert_eq!(transitions[1].kind, TransitionKind::CiFinished);
        for transition in &transitions {
            assert_eq!(transition.base_branch, "");
            assert_eq!(transition.behind_by, None);
        }
        assert_eq!(
            harness
                .gh
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .count_repo_info(),
            1,
            "the label is resolved once per nwo: CI needs it to decide suppression"
        );
    }

    #[tokio::test]
    async fn base_branch_resolution_walks_the_three_step_chain() {
        let _guard = round_test_lock().await;

        // Step 1: the GitHub default branch.
        {
            let harness = Harness::new(AppSettings::default());
            let repo = harness.repo("repo-a");
            crate::pty::git_watcher::publish_git_status(
                &repo,
                Some(crate::pty::git_watcher::GitStatus {
                    branch: Some("fix/2131".to_string()),
                    dirty: false,
                }),
            );
            harness.set_work(&[repo]);
            {
                let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
                gh.repo_info
                    .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
                gh.compare
                    .push_back(Ok(ok_output(&compare_body("identical", 0, 0))));
                gh.compare
                    .push_back(Ok(ok_output(&compare_body("behind", 2, 1))));
            }
            let mut now = Instant::now();
            let mut wall = Local::now();
            harness.round(now, wall).await;
            drain(&harness);
            tick(&mut now, &mut wall, 300);
            harness.round(now, wall).await;
            let transitions = harness.drain_transitions();
            assert_eq!(transitions.len(), 1);
            assert_eq!(transitions[0].base_branch, "main");
        }

        // Step 2: the local clone's cached origin HEAD, with `origin/` stripped.
        {
            let harness = Harness::new(AppSettings::default());
            let repo = harness.repo("repo-a");
            harness.set_work(&[repo]);
            {
                let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
                gh.compare
                    .push_back(Ok(ok_output(&compare_body("identical", 0, 0))));
                gh.compare
                    .push_back(Ok(ok_output(&compare_body("behind", 2, 1))));
            }
            harness
                .git
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .symbolic_ref
                .push_back(Ok("origin/develop".to_string()));
            let mut now = Instant::now();
            let mut wall = Local::now();
            harness.round(now, wall).await;
            drain(&harness);
            tick(&mut now, &mut wall, 300);
            harness.round(now, wall).await;
            let transitions = harness.drain_transitions();
            assert_eq!(transitions.len(), 1);
            assert_eq!(transitions[0].base_branch, "develop");
        }

        // Step 3: the literal label.
        {
            let harness = Harness::new(AppSettings::default());
            let repo = harness.repo("repo-a");
            harness.set_work(&[repo]);
            {
                let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
                gh.compare
                    .push_back(Ok(ok_output(&compare_body("identical", 0, 0))));
                gh.compare
                    .push_back(Ok(ok_output(&compare_body("behind", 2, 1))));
            }
            let mut now = Instant::now();
            let mut wall = Local::now();
            harness.round(now, wall).await;
            drain(&harness);
            tick(&mut now, &mut wall, 300);
            harness.round(now, wall).await;
            let transitions = harness.drain_transitions();
            assert_eq!(transitions.len(), 1);
            assert_eq!(transitions[0].base_branch, DEFAULT_BRANCH_LABEL);
        }
    }

    #[tokio::test]
    async fn base_branch_is_resolved_once_per_nwo_and_cached() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        let first = harness.repo("repo-a");
        let second = harness.repo("repo-b");
        harness.set_work(&[first.clone(), second.clone()]);
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            for _ in 0..10 {
                gh.compare
                    .push_back(Ok(ok_output(&compare_body("behind", 2, 1))));
            }
        }
        {
            let mut git = harness.git.lock().unwrap_or_else(|e| e.into_inner());
            git.set_origin(&first, "git@github.com:mblua/AgentsCommander.git");
            git.set_origin(&second, "https://github.com/mblua/other.git");
        }

        let mut now = Instant::now();
        let mut wall = Local::now();
        for _ in 0..5 {
            harness.round(now, wall).await;
            tick(&mut now, &mut wall, 300);
        }

        assert_eq!(
            harness
                .gh
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .count_repo_info(),
            2,
            "exactly one repos/<nwo> call per distinct nwo, for the process lifetime"
        );
        let snapshot = harness.snapshot();
        assert_eq!(
            snapshot.get(&first).expect("first").staleness,
            StalenessState::Stale,
            "a failed label resolution never changes the answer"
        );
        assert_eq!(
            snapshot.get(&second).expect("second").staleness,
            StalenessState::Stale
        );
    }

    #[test]
    fn gh_command_spec_pins_the_child_environment_and_creation_flags() {
        let spec = build_gh_command_spec(
            Path::new("gh"),
            GhQuery::RepoInfo {
                nwo: "mblua/AgentsCommander",
            },
        )
        .expect("spec");

        let mut envs = spec.envs.clone();
        envs.sort();
        assert_eq!(
            envs,
            vec![
                ("GH_NO_UPDATE_NOTIFIER".to_string(), "1".to_string()),
                ("GH_PAGER".to_string(), "cat".to_string()),
                ("GH_PROMPT_DISABLED".to_string(), "1".to_string()),
            ],
            "whole-vector equality: a silently dropped variable must fail"
        );

        #[cfg(windows)]
        assert_eq!(spec.creation_flags, 0x08000000);
        #[cfg(not(windows))]
        assert_eq!(spec.creation_flags, 0);
    }

    #[tokio::test]
    async fn every_round_process_goes_through_the_spawner_seam() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        let first = harness.repo("repo-a");
        let second = harness.repo("repo-b");
        harness.set_work(&[first.clone(), second.clone()]);
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
            gh.ci.push_back(Ok(ok_output(&ci_body(&sha_of('a'), &[]))));
            gh.compare
                .push_back(Ok(ok_output(&compare_body("identical", 0, 0))));
            gh.compare
                .push_back(Ok(ok_output(&compare_body("identical", 0, 0))));
            gh.repo_info
                .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
            gh.repo_info
                .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
        }
        {
            let mut git = harness.git.lock().unwrap_or_else(|e| e.into_inner());
            git.set_head_sha(&first, 'a');
            git.set_head_sha(&second, 'b');
            git.set_origin(&first, "git@github.com:mblua/AgentsCommander.git");
            git.set_origin(&second, "https://github.com/mblua/other.git");
        }

        let now = Instant::now();
        let wall = Local::now();
        harness.round(now, wall).await;

        let gh_calls = harness
            .gh
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .calls
            .len();
        let git_calls = harness
            .git
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .calls
            .len();
        assert_eq!(gh_calls, 6, "2 CI + 2 compare + 2 base labels");
        assert_eq!(git_calls, 4, "2 paths, 2 local reads each");
    }

    #[test]
    fn no_code_path_can_build_a_gh_auth_invocation() {
        let gh = Path::new("gh");
        let sha = sha_of('a');
        let specs = [
            build_gh_command_spec(
                gh,
                GhQuery::Ci {
                    nwo: "a/b",
                    sha40: &sha,
                },
            )
            .expect("ci"),
            build_gh_command_spec(
                gh,
                GhQuery::Compare {
                    nwo: "a/b",
                    sha40: &sha,
                },
            )
            .expect("compare"),
            build_gh_command_spec(gh, GhQuery::RepoInfo { nwo: "a/b" }).expect("info"),
        ];
        for spec in &specs {
            assert_eq!(spec.args.first().map(String::as_str), Some("api"));
            assert!(
                !spec.args.iter().any(|arg| arg == "auth"),
                "gh auth login is never invoked on any path"
            );
        }
    }

    #[test]
    fn round_log_level_is_debug() {
        assert_eq!(ROUND_LOG_LEVEL, log::Level::Debug);
    }

    #[tokio::test]
    async fn archived_roots_are_normalized_before_filtering() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        let archived_root = harness.temp.path().join("archived");
        let repo = archived_root.join(".ac").join("wg-1").join("repo-x");
        std::fs::create_dir_all(&repo).expect("create archived repo");
        let repo = repo.to_string_lossy().replace('\\', "/");
        let raw_root = format!("{}/.", archived_root.to_string_lossy().replace('\\', "/"));
        harness.set_work(std::slice::from_ref(&repo));

        let filtered = harness.sweeper.work_list(vec![raw_root.clone()]).await;
        assert!(
            !filtered.contains(&repo),
            "the normalized archived root must exclude the repo"
        );

        let unfiltered = crate::pty::git_watcher::build_work_list(
            vec![repo.clone()],
            vec![],
            std::slice::from_ref(&raw_root),
        );
        assert!(
            unfiltered.contains(&repo),
            "the raw spelling is the round-2 bug: it defeats the filter"
        );
    }

    #[test]
    fn every_child_command_is_credential_scrubbed() {
        let spec = build_gh_command_spec(
            Path::new("gh"),
            GhQuery::RepoInfo {
                nwo: "mblua/AgentsCommander",
            },
        )
        .expect("spec");
        let gh_command = command_from_spec(spec);
        let git_command = git_command("C:/repo-a", &["rev-parse".to_string(), "HEAD".to_string()]);

        let mut expected: Vec<&str> = crate::pty::credentials::CREDENTIAL_ENV_KEYS.to_vec();
        expected.sort_unstable();

        for command in [&gh_command, &git_command] {
            let mut removed: Vec<&str> = command
                .as_std()
                .get_envs()
                .filter(|(_key, value)| value.is_none())
                .map(|(key, _value)| key.to_str().expect("utf-8 env key"))
                .collect();
            removed.sort_unstable();
            assert_eq!(removed, expected, "whole-vector equality");
        }
    }

    #[test]
    fn nwo_is_parsed_from_github_urls_only() {
        for origin in [
            "git@github.com:mblua/AgentsCommander.git",
            "https://github.com/mblua/AgentsCommander.git",
            "https://github.com/mblua/AgentsCommander",
            "ssh://git@github.com/mblua/AgentsCommander.git",
        ] {
            assert_eq!(
                parse_github_nwo(origin).as_deref(),
                Some("mblua/AgentsCommander"),
                "{origin}"
            );
        }

        for origin in [
            "https://gitlab.com/a/b.git",
            "https://github.com.evil.example/a/b",
            "https://github.com/a",
            "",
        ] {
            assert_eq!(parse_github_nwo(origin), None, "{origin}");
        }
    }

    #[test]
    fn failure_interval_never_drops_below_base() {
        let one_hour = Duration::from_secs(3600);
        assert_eq!(
            failure_interval(
                FailureKind::Timeout,
                one_hour,
                Some(Duration::from_secs(900))
            ),
            one_hour
        );
        assert_eq!(
            failure_interval(FailureKind::RateLimited, one_hour, None),
            one_hour,
            "a rate-limited key must not re-query faster than a healthy one"
        );

        let thirty = Duration::from_secs(30);
        assert_eq!(
            failure_interval(FailureKind::RateLimited, thirty, None),
            Duration::from_secs(900)
        );
        assert_eq!(
            failure_interval(FailureKind::Timeout, thirty, None),
            Duration::from_secs(60)
        );
        assert_eq!(
            failure_interval(FailureKind::Timeout, thirty, Some(Duration::from_secs(900))),
            Duration::from_secs(900),
            "the cap holds"
        );
    }
    /// #2129 - T6: the three cases of the rendering rule, plus an empty label,
    /// which takes case 3 and is unreachable for stale notices because
    /// `for_remote_activity` rejects an empty stale base.
    #[test]
    fn issue_2129_base_branch_display_covers_every_case() {
        assert_eq!(
            base_branch_display("main", DEFAULT_BRANCH_LABEL),
            "the default branch on GitHub"
        );
        assert_eq!(
            base_branch_display("main", "main"),
            "its counterpart on GitHub"
        );
        assert_eq!(
            base_branch_display("feature/2083-2064-remote-alerts", "main"),
            "main on GitHub"
        );
        assert_eq!(base_branch_display("main", ""), " on GitHub");
    }

    // --- #2374: the neutral snapshot producer. Tests are prefixed
    // `remote_activity_cache_producer_` on purpose: the cache-unit guard runs
    // `cargo test --lib config::remote_activity_cache::` and must be satisfied
    // only by `config/remote_activity_cache.rs`'s own unit tests. ---

    /// Positive control for the factory: `production_seams` must FORWARD its
    /// argument. `None` could be a dropped default, so only the `Some` case can
    /// fail when the parameter is discarded.
    #[test]
    fn remote_activity_cache_producer_factory_keeps_snapshot_dir() {
        let dir = PathBuf::from("snapshot-dir");
        assert_eq!(
            RemoteSweeper::production_seams(Some(dir.clone())).snapshot_dir,
            Some(dir)
        );
        assert_eq!(RemoteSweeper::production_seams(None).snapshot_dir, None);
    }

    /// Inspection guard for the real construction site: the factory test above
    /// is the positive control, this one refuses a `lib.rs` that stopped
    /// passing the instance config directory. Whitespace is flattened so a
    /// `rustfmt` re-wrap cannot hide the call.
    #[test]
    fn remote_activity_cache_producer_lib_call_site_passes_config_dir() {
        let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs"))
            .expect("read lib.rs");
        let flattened: String = source.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(
            flattened.contains("RemoteSweeper::production_seams(crate::config::config_dir()"),
            "lib.rs must construct the sweeper with the instance config directory"
        );
    }

    /// Pure state, no logger capture and no sleeping: first failure, suppressed
    /// inside the window, admitted at the boundary and later, and immediately
    /// re-admitted after a success reset.
    #[test]
    fn remote_activity_cache_producer_warning_limiter_admits_once_per_window() {
        let mut limiter = SnapshotWarningLimiter::default();
        let start = Instant::now();
        assert!(limiter.admit(start), "the first failure warns");
        assert!(
            !limiter.admit(start + Duration::from_secs(29)),
            "just inside the window is suppressed"
        );
        assert!(
            limiter.admit(start + Duration::from_secs(30)),
            "the exact boundary admits"
        );
        assert!(
            !limiter.admit(start + Duration::from_secs(59)),
            "the new window suppresses again"
        );
        limiter.reset();
        assert!(
            limiter.admit(start + Duration::from_secs(59)),
            "a successful write resets the limiter"
        );
    }

    #[tokio::test]
    async fn remote_activity_cache_producer_positive_wiring() {
        let _guard = round_test_lock().await;
        let snapshot_dir = tempfile::tempdir().expect("snapshot dir");
        let settings = AppSettings {
            branch_staleness_enabled: false,
            ..AppSettings::default()
        };
        let harness = Harness::with_snapshot_dir(settings, Some(snapshot_dir.path().to_path_buf()));
        let repo_running = harness.repo("repo-a");
        let repo_idle = harness.repo("repo-b");
        publish_branch(&repo_running, "feature-a");
        publish_branch(&repo_idle, "feature-b");
        {
            let mut git = harness.git.lock().unwrap_or_else(|e| e.into_inner());
            git.set_head_sha(&repo_running, 'a');
            git.set_head_sha(&repo_idle, 'b');
        }
        harness.set_work(&[repo_running.clone(), repo_idle.clone()]);

        let publication = Utc::now();
        harness.set_publication_clock(publication);
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.repo_info
                .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
            // A run on the branch, then an identity compare that says the branch
            // is ahead, so `Running` is published rather than suppressed.
            gh.route(
                &format!(
                    "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
                    sha_of('a')
                ),
                Ok(ok_output(&ci_rows(
                    &sha_of('a'),
                    &[("feature-a", "in_progress")],
                ))),
            );
            gh.compare
                .push_back(Ok(ok_output(&compare_body("ahead", 0, 1))));
            // No run on this branch: `Idle` needs no identity call.
            gh.route(
                &format!(
                    "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
                    sha_of('b')
                ),
                Ok(ok_output(&ci_rows(&sha_of('b'), &[]))),
            );
        }

        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;

        let path = snapshot_dir.path().join(REMOTE_ACTIVITY_SNAPSHOT_FILE_NAME);
        let bytes = std::fs::read(&path).expect("the exact destination exists");
        let raw: serde_json::Value = serde_json::from_slice(&bytes).expect("snapshot json");
        assert_eq!(raw["schemaVersion"], 1);
        assert_eq!(
            raw["generatedAt"],
            publication.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        );
        let repos = raw["repos"].as_array().expect("repos array");
        assert_eq!(repos.len(), 2);
        let raw_paths: Vec<&str> = repos
            .iter()
            .map(|repo| repo["path"].as_str().expect("path"))
            .collect();
        let mut sorted = raw_paths.clone();
        sorted.sort();
        assert_eq!(raw_paths, sorted, "entries are sorted by raw path string");
        assert_eq!(repos[0]["path"], repo_running);
        assert_eq!(repos[0]["ciState"], "running");
        assert_eq!(repos[1]["path"], repo_idle);
        assert_eq!(repos[1]["ciState"], "idle");

        // Producer-to-reader round trip through the validating reader the CLI
        // uses: the produced bytes carry identical path/state data.
        let snapshot =
            crate::config::remote_activity_cache::read_snapshot(&path).expect("read produced");
        assert_eq!(
            snapshot.repos,
            vec![
                (
                    repo_running.clone(),
                    crate::config::remote_activity_cache::PersistedRepoCi {
                        state: crate::config::remote_activity_cache::PersistedCiState::Running,
                        run_ids: vec![1],
                        pull_requests: Vec::new(),
                        unknown_reason: None,
                    },
                ),
                (
                    repo_idle.clone(),
                    crate::config::remote_activity_cache::PersistedRepoCi {
                        state: crate::config::remote_activity_cache::PersistedCiState::Idle,
                        run_ids: Vec::new(),
                        pull_requests: Vec::new(),
                        unknown_reason: None,
                    },
                ),
            ]
        );

        // Empty-round GC: the next round owns an empty work list, so the file is
        // atomically replaced with `repos: []` instead of keeping survivors.
        tick(&mut now, &mut wall, 60);
        harness.set_work(&[]);
        harness.round(now, wall).await;
        let raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).expect("read empty")).expect("empty json");
        assert_eq!(raw["repos"].as_array().expect("repos").len(), 0);
    }

    #[tokio::test]
    async fn remote_activity_cache_producer_forced_write_failure_keeps_round_effects() {
        let _guard = round_test_lock().await;
        let blocker = tempfile::tempdir().expect("blocker dir");
        let blocked_destination = blocker.path().join("not-a-directory");
        std::fs::write(&blocked_destination, b"file").expect("blocker file");
        let settings = AppSettings {
            branch_staleness_enabled: false,
            ..AppSettings::default()
        };
        let harness = Harness::with_snapshot_dir(settings, Some(blocked_destination.clone()));
        let repo = harness.repo("repo-a");
        publish_branch(&repo, "feature-a");
        harness
            .git
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_head_sha(&repo, 'a');
        harness.set_work(std::slice::from_ref(&repo));
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.repo_info
                .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
            gh.route(
                &format!(
                    "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
                    sha_of('a')
                ),
                Ok(ok_output(&ci_rows(
                    &sha_of('a'),
                    &[("feature-a", "completed")],
                ))),
            );
            gh.route(
                &format!(
                    "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
                    sha_of('a')
                ),
                Ok(ok_output(&ci_rows(
                    &sha_of('a'),
                    &[("feature-a", "in_progress")],
                ))),
            );
            for _ in 0..2 {
                gh.compare
                    .push_back(Ok(ok_output(&compare_body("ahead", 0, 1))));
            }
        }

        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;
        tick(&mut now, &mut wall, 60);
        harness.round(now, wall).await;

        let transitions = harness.drain_transitions();
        assert_eq!(transitions.len(), 1, "the transition still fires");
        assert_eq!(transitions[0].kind, TransitionKind::CiStarted);
        assert_eq!(harness.emitted_count(), 2, "both payloads were emitted");
        assert_eq!(
            harness.snapshot().get(&repo).expect("entry").ci,
            CiState::Running,
            "the in-memory snapshot still updated"
        );
        assert!(
            harness
                .sweeper
                .lock_state()
                .snapshot_warnings
                .last_admitted
                .is_some(),
            "the forced failure was admitted once into the warning limiter"
        );
        assert!(
            !blocked_destination
                .join(REMOTE_ACTIVITY_SNAPSHOT_FILE_NAME)
                .exists(),
            "the destination stays unwritten"
        );
    }

    #[tokio::test]
    async fn remote_activity_cache_producer_publication_time_is_not_the_round_start() {
        let _guard = round_test_lock().await;
        let snapshot_dir = tempfile::tempdir().expect("snapshot dir");
        let settings = AppSettings {
            branch_staleness_enabled: false,
            ..AppSettings::default()
        };
        let harness = Harness::with_snapshot_dir(settings, Some(snapshot_dir.path().to_path_buf()));
        let repo = harness.repo("repo-a");
        publish_branch(&repo, "feature-a");
        harness.set_work(std::slice::from_ref(&repo));

        // Whole-second instant: the codec writes seconds precision.
        let publication =
            chrono::DateTime::from_timestamp(Utc::now().timestamp(), 0).expect("whole second");
        harness.set_publication_clock(publication);
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.repo_info
                .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
            gh.route(
                &format!(
                    "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
                    sha_of('a')
                ),
                Ok(ok_output(&ci_rows(&sha_of('a'), &[]))),
            );
        }

        // Round-start wall deliberately more than the freshness window older
        // than the publication instant: reusing it would publish stale bytes.
        let round_start_wall = (publication - chrono::Duration::seconds(45)).with_timezone(&Local);
        harness.round(Instant::now(), round_start_wall).await;

        let path = snapshot_dir.path().join(REMOTE_ACTIVITY_SNAPSHOT_FILE_NAME);
        let snapshot =
            crate::config::remote_activity_cache::read_snapshot(&path).expect("read produced");
        assert_eq!(snapshot.generated_at, publication);
        assert!(crate::config::remote_activity_cache::snapshot_is_fresh(
            &snapshot,
            publication
        ));
        assert!(
            !crate::config::remote_activity_cache::snapshot_is_fresh(
                &snapshot,
                round_start_wall.with_timezone(&Utc)
            ),
            "the round-start clock would have published already-stale bytes"
        );
    }

    #[tokio::test]
    async fn remote_activity_cache_producer_missing_directory_skips_persistence_only() {
        let _guard = round_test_lock().await;
        let settings = AppSettings {
            branch_staleness_enabled: false,
            ..AppSettings::default()
        };
        let harness = Harness::with_snapshot_dir(settings, None);
        assert_eq!(
            harness.sweeper.snapshot_persistence,
            SnapshotPersistence::DisabledMissingAtConstruction,
            "construction records the one-time diagnostic state"
        );
        let repo = harness.repo("repo-a");
        publish_branch(&repo, "feature-a");
        harness
            .git
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_head_sha(&repo, 'a');
        harness.set_work(std::slice::from_ref(&repo));
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.repo_info
                .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
            gh.route(
                &format!(
                    "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
                    sha_of('a')
                ),
                Ok(ok_output(&ci_rows(
                    &sha_of('a'),
                    &[("feature-a", "completed")],
                ))),
            );
            gh.route(
                &format!(
                    "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
                    sha_of('a')
                ),
                Ok(ok_output(&ci_rows(
                    &sha_of('a'),
                    &[("feature-a", "in_progress")],
                ))),
            );
            for _ in 0..2 {
                gh.compare
                    .push_back(Ok(ok_output(&compare_body("ahead", 0, 1))));
            }
        }

        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;
        tick(&mut now, &mut wall, 60);
        harness.round(now, wall).await;

        assert_eq!(harness.emitted_count(), 2, "payload emission is untouched");
        let transitions = harness.drain_transitions();
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].kind, TransitionKind::CiStarted);
        assert_eq!(
            harness.snapshot().get(&repo).expect("entry").ci,
            CiState::Running,
            "post-GC memory is untouched"
        );
        assert!(
            harness
                .sweeper
                .lock_state()
                .snapshot_warnings
                .last_admitted
                .is_none(),
            "a missing directory is not a per-round persistence failure"
        );
    }

    // --- #2473: run ids, PR numbers and the failure reason ---

    fn run_row(id: u64, sha40: &str, branch: &str, status: &str, prs: &[u64]) -> serde_json::Value {
        let prs: Vec<serde_json::Value> = prs
            .iter()
            .map(|number| serde_json::json!({ "number": number }))
            .collect();
        serde_json::json!({
            "id": id,
            "workflow_id": 700,
            "run_attempt": 1,
            "head_sha": sha40,
            "updated_at": "2026-10-01T00:00:00Z",
            "conclusion": if status == "completed" { Some("success") } else { None },
            "head_branch": branch,
            "status": status,
            "pull_requests": prs,
        })
    }

    fn expected_run(id: u64, branch: &str, status: &str, prs: &[u64]) -> CiRunObservation {
        CiRunObservation {
            id,
            workflow_id: 700,
            run_attempt: 1,
            head_sha: sha_of('a'),
            head_branch: branch.to_string(),
            status: Some(status.to_string()),
            conclusion: (status == "completed").then(|| "success".to_string()),
            updated_at: "2026-10-01T00:00:00Z".to_string(),
            pull_requests: prs.to_vec(),
        }
    }

    fn runs_body(rows: Vec<serde_json::Value>) -> String {
        serde_json::json!({ "total_count": rows.len(), "workflow_runs": rows }).to_string()
    }

    #[test]
    fn p1_parse_ci_response_collects_ids_and_prs_only_from_running_kept_rows() {
        assert_eq!(
            parse_ci_response(
                &runs_body(vec![
                    serde_json::json!({"head_branch":"feature","status":"in_progress"})
                ]),
                "feature",
                &sha_of('a')
            ),
            Err(FailureKind::Other)
        );
        let body = runs_body(vec![
            run_row(11, &sha_of('a'), "feature", "in_progress", &[5, 6]),
            run_row(12, &sha_of('a'), "feature", "queued", &[5]),
            run_row(13, &sha_of('a'), "feature", "completed", &[99]),
            run_row(14, &sha_of('a'), "other", "in_progress", &[98]),
        ]);
        assert_eq!(
            parse_ci_response(&body, "feature", &sha_of('a')),
            Ok(CiAnswer {
                state: CiState::Running,
                branch_has_runs: true,
                observations: vec![
                    expected_run(11, "feature", "in_progress", &[5, 6]),
                    expected_run(12, "feature", "queued", &[5]),
                    expected_run(13, "feature", "completed", &[99])
                ],
                run_ids: vec![11, 12],
                pull_requests: vec![5, 6, 5],
            })
        );
        let idle = runs_body(vec![
            run_row(13, &sha_of('a'), "feature", "completed", &[99]),
            run_row(14, &sha_of('a'), "other", "in_progress", &[98]),
        ]);
        assert_eq!(
            parse_ci_response(&idle, "feature", &sha_of('a')),
            Ok(CiAnswer {
                state: CiState::Idle,
                branch_has_runs: true,
                observations: vec![expected_run(13, "feature", "completed", &[99])],
                run_ids: Vec::new(),
                pull_requests: Vec::new(),
            })
        );
    }

    #[tokio::test]
    async fn p2_apply_ci_clears_runs_on_suppressed_and_err_and_sets_failure_only_on_plain_err() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        let key = QueryKey {
            nwo: "mblua/AgentsCommander".to_string(),
            sha40: sha_of('a'),
            branch: "feature".to_string(),
        };
        let now = Instant::now();
        let ctx = RoundCtx {
            facts: &[],
            now,
            wall: Local::now(),
            ci_dial: Duration::from_secs(60),
            staleness_dial: Duration::from_secs(60),
        };
        let runs = || (vec![1, 2], vec![7]);
        let direct_completion = || {
            let rows = vec![
                run_row(1, &key.sha40, &key.branch, "in_progress", &[7]),
                run_row(2, &key.sha40, &key.branch, "queued", &[]),
            ];
            let answer = parse_ci_response(&runs_body(rows), &key.branch, &key.sha40).unwrap();
            let mut ledger = CiCompletionState::default();
            merge_ci_observations(&mut ledger, &answer.observations, true).unwrap();
            ledger
        };
        let read = |harness: &Harness| {
            let state = harness.sweeper.lock_state();
            let ci = &state.keys[&key].ci;
            (
                ci.chip,
                ci.run_ids.clone(),
                ci.pull_requests.clone(),
                ci.failure,
            )
        };
        let s = &harness.sweeper;

        s.apply_ci(
            &key,
            KeyOutcome {
                ci: Some(Ok(CiState::Running)),
                ci_suppressed: false,
                ci_runs: runs(),
                ci_completion: Some(direct_completion()),
                ci_decision: Some(CiCompletionDecision::Accepted),
                ..KeyOutcome::default()
            },
            &[],
            &ctx,
        );
        assert_eq!(
            read(&harness),
            (CiState::Running, vec![1, 2], vec![7], None)
        );

        s.apply_ci(
            &key,
            KeyOutcome {
                ci: Some(Ok(CiState::Idle)),
                ci_suppressed: true,
                ci_runs: runs(),
                ci_completion: Some(direct_completion()),
                ci_decision: Some(CiCompletionDecision::Accepted),
                ..KeyOutcome::default()
            },
            &[],
            &ctx,
        );
        assert_eq!(read(&harness), (CiState::Idle, vec![], vec![], None));
        assert!(s.lock_state().keys[&key].ci.completion.known.is_empty());

        s.apply_ci(
            &key,
            KeyOutcome {
                ci: Some(Ok(CiState::Running)),
                ci_suppressed: false,
                ci_runs: runs(),
                ci_completion: Some(direct_completion()),
                ci_decision: Some(CiCompletionDecision::Accepted),
                ..KeyOutcome::default()
            },
            &[],
            &ctx,
        );
        s.apply_ci(
            &key,
            KeyOutcome {
                ci: Some(Err(FailureKind::Timeout)),
                ci_suppressed: true,
                ci_runs: runs(),
                ci_completion: Some(direct_completion()),
                ci_decision: Some(CiCompletionDecision::Accepted),
                ..KeyOutcome::default()
            },
            &[],
            &ctx,
        );
        assert_eq!(read(&harness), (CiState::Idle, vec![], vec![], None));
        assert!(s.lock_state().keys[&key].ci.completion.known.is_empty());

        s.apply_ci(
            &key,
            KeyOutcome {
                ci: Some(Ok(CiState::Running)),
                ci_suppressed: false,
                ci_runs: runs(),
                ci_completion: Some(direct_completion()),
                ci_decision: Some(CiCompletionDecision::Accepted),
                ..KeyOutcome::default()
            },
            &[],
            &ctx,
        );
        s.apply_ci(
            &key,
            KeyOutcome {
                ci: Some(Err(FailureKind::Incomplete)),
                ci_suppressed: false,
                ci_runs: runs(),
                ci_completion: Some(direct_completion()),
                ci_decision: Some(CiCompletionDecision::Accepted),
                ..KeyOutcome::default()
            },
            &[],
            &ctx,
        );
        assert_eq!(
            read(&harness),
            (
                CiState::Unknown,
                vec![],
                vec![],
                Some(FailureKind::Incomplete)
            )
        );

        assert_eq!(s.lock_state().keys[&key].ci.completion.known.len(), 2);
        s.apply_ci(
            &key,
            KeyOutcome {
                ci: Some(Ok(CiState::Idle)),
                ci_suppressed: false,
                ci_runs: runs(),
                ci_completion: Some(direct_completion()),
                ci_decision: Some(CiCompletionDecision::Accepted),
                ..KeyOutcome::default()
            },
            &[],
            &ctx,
        );
        assert_eq!(read(&harness), (CiState::Idle, vec![], vec![], None));
        assert_eq!(s.lock_state().keys[&key].ci.completion.known.len(), 2);

        s.apply_ci(
            &key,
            KeyOutcome {
                ci: None,
                ci_suppressed: false,
                ci_runs: runs(),
                ci_completion: Some(direct_completion()),
                ci_decision: Some(CiCompletionDecision::Accepted),
                ..KeyOutcome::default()
            },
            &[],
            &ctx,
        );
        assert_eq!(
            read(&harness),
            (CiState::Idle, vec![], vec![], None),
            "a key that is not due keeps its published detail"
        );
    }

    #[test]
    fn persisted_failure_reason_maps_every_kind() {
        assert_eq!(
            persisted_failure_reason(FailureKind::Timeout),
            "ci-query-timeout"
        );
        assert_eq!(
            persisted_failure_reason(FailureKind::RateLimited),
            "ci-query-rate-limited"
        );
        assert_eq!(
            persisted_failure_reason(FailureKind::SecondaryRateLimited),
            "ci-query-secondary-rate-limited"
        );
        assert_eq!(
            persisted_failure_reason(FailureKind::NotAuthenticated),
            "ci-query-not-authenticated"
        );
        assert_eq!(
            persisted_failure_reason(FailureKind::Incomplete),
            "ci-query-incomplete"
        );
        assert_eq!(
            persisted_failure_reason(FailureKind::Other),
            "ci-query-failed"
        );
    }

    fn persisted_of(
        dir: &Path,
        repo: &str,
    ) -> crate::config::remote_activity_cache::PersistedRepoCi {
        let snapshot = crate::config::remote_activity_cache::read_snapshot(
            &dir.join(REMOTE_ACTIVITY_SNAPSHOT_FILE_NAME),
        )
        .expect("read produced snapshot");
        snapshot
            .repos
            .into_iter()
            .find(|(path, _)| path == repo)
            .map(|(_, ci)| ci)
            .expect("repo is in the snapshot")
    }

    fn expected_ci(
        state: PersistedCiState,
        run_ids: &[u64],
        pull_requests: &[u64],
        unknown_reason: Option<&str>,
    ) -> crate::config::remote_activity_cache::PersistedRepoCi {
        crate::config::remote_activity_cache::PersistedRepoCi {
            state,
            run_ids: run_ids.to_vec(),
            pull_requests: pull_requests.to_vec(),
            unknown_reason: unknown_reason.map(str::to_string),
        }
    }

    /// P3 a, c, d, f: no resolved default branch, so arm `Ok(answer)` answers
    /// without any identity compare.
    #[tokio::test]
    async fn p3_producer_publishes_run_ids_prs_and_reason_end_to_end() {
        let _guard = round_test_lock().await;
        let dir = tempfile::tempdir().expect("snapshot dir");
        let settings = AppSettings {
            branch_staleness_enabled: false,
            ..AppSettings::default()
        };
        let harness = Harness::with_snapshot_dir(settings, Some(dir.path().to_path_buf()));
        let repo = harness.repo("repo-a");
        publish_branch(&repo, "feature-a");
        harness
            .git
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_head_sha(&repo, 'a');
        harness.set_work(std::slice::from_ref(&repo));
        let marker = format!(
            "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
            sha_of('a')
        );
        let script = |result: Result<GhCallOutput, FailureKind>| {
            harness
                .gh
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .route(&marker, result);
        };

        // a. running: exact ids / PRs from real `gh` JSON, no compare issued.
        script(Ok(ok_output(&runs_body(vec![
            run_row(502, &sha_of('a'), "feature-a", "in_progress", &[40, 41]),
            run_row(501, &sha_of('a'), "feature-a", "queued", &[40]),
            run_row(400, &sha_of('a'), "feature-a", "completed", &[39]),
            run_row(600, &sha_of('a'), "main", "in_progress", &[1]),
        ]))));
        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;
        assert_eq!(
            persisted_of(dir.path(), &repo),
            expected_ci(PersistedCiState::Running, &[501, 502], &[40, 41], None)
        );
        assert_eq!(
            harness
                .gh
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .count("/compare/"),
            0,
            "no identity compare without a resolved default branch"
        );

        // f. the CI axis is not due one second later: the same detail again.
        tick(&mut now, &mut wall, 1);
        let calls = harness.gh_call_count();
        harness.round(now, wall).await;
        assert_eq!(
            harness
                .gh
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .count("/actions/runs?"),
            1,
            "the CI axis was not due (calls before: {calls})"
        );
        assert_eq!(
            persisted_of(dir.path(), &repo),
            expected_ci(PersistedCiState::Running, &[501, 502], &[40, 41], None)
        );

        // All remembered runs need proof, followed by a distinct closing list.
        for id in [400, 501, 502] {
            script(Ok(ok_output(&runs_body(vec![run_row(
                502,
                &sha_of('a'),
                "feature-a",
                "completed",
                &[40],
            )]))));
            queue_completion(
                &harness,
                "mblua/AgentsCommander",
                run_row(id, &sha_of('a'), "feature-a", "completed", &[]),
            );
            tick(&mut now, &mut wall, if id == 400 { 3600 } else { 10 });
            harness.round(now, wall).await;
            assert_eq!(
                persisted_of(dir.path(), &repo),
                expected_ci(PersistedCiState::Running, &[501, 502], &[40, 41], None)
            );
            assert!(harness.drain_transitions().is_empty());
        }
        script(Ok(ok_output(&runs_body(vec![run_row(
            502,
            &sha_of('a'),
            "feature-a",
            "completed",
            &[40],
        )]))));
        tick(&mut now, &mut wall, 10);
        harness.round(now, wall).await;
        assert_eq!(
            persisted_of(dir.path(), &repo),
            expected_ci(PersistedCiState::Idle, &[], &[], None)
        );
        let edges = harness.drain_transitions();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].observed_at, wall);
        // d. Running, then a timeout -> Unknown with the reason, then recovery.
        script(Ok(ok_output(&runs_body(vec![run_row(
            700,
            &sha_of('a'),
            "feature-a",
            "in_progress",
            &[50],
        )]))));
        tick(&mut now, &mut wall, 3600);
        harness.round(now, wall).await;
        assert_eq!(
            persisted_of(dir.path(), &repo),
            expected_ci(PersistedCiState::Running, &[700], &[50], None)
        );
        script(Err(FailureKind::Timeout));
        tick(&mut now, &mut wall, 3600);
        harness.round(now, wall).await;
        assert_eq!(
            persisted_of(dir.path(), &repo),
            expected_ci(
                PersistedCiState::Unknown,
                &[],
                &[],
                Some("ci-query-timeout")
            )
        );
        script(Ok(ok_output(&runs_body(vec![run_row(
            800,
            &sha_of('a'),
            "feature-a",
            "in_progress",
            &[51],
        )]))));
        tick(&mut now, &mut wall, 3600);
        harness.round(now, wall).await;
        assert_eq!(
            persisted_of(dir.path(), &repo),
            expected_ci(PersistedCiState::Running, &[800], &[51], None)
        );
    }

    /// P3 b and e: a resolved default branch. A non-default branch whose
    /// compare is not identical publishes its runs; the default branch and an
    /// identical-to-default branch are suppressed to Idle with no detail.
    #[tokio::test]
    async fn p3_producer_publishes_runs_after_identity_compare_and_clears_them_when_suppressed() {
        let _guard = round_test_lock().await;
        let dir = tempfile::tempdir().expect("snapshot dir");
        let settings = AppSettings {
            branch_staleness_enabled: false,
            ..AppSettings::default()
        };
        let harness = Harness::with_snapshot_dir(settings, Some(dir.path().to_path_buf()));
        let feature = harness.repo("repo-a");
        let default = harness.repo("repo-b");
        publish_branch(&feature, "feature-a");
        {
            let mut git = harness.git.lock().unwrap_or_else(|e| e.into_inner());
            git.set_head_sha(&feature, 'a');
            git.set_head_sha(&default, 'b');
        }
        harness.set_work(&[feature.clone(), default.clone()]);
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.repo_info
                .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
            gh.route(
                &format!(
                    "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
                    sha_of('a')
                ),
                Ok(ok_output(&runs_body(vec![
                    run_row(902, &sha_of('a'), "feature-a", "in_progress", &[77]),
                    run_row(901, &sha_of('a'), "feature-a", "in_progress", &[76, 77]),
                ]))),
            );
            gh.route(
                &format!("repos/mblua/AgentsCommander/compare/HEAD...{}", sha_of('a')),
                Ok(ok_output(&compare_body("ahead", 0, 1))),
            );
            gh.route(
                &format!(
                    "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
                    sha_of('b')
                ),
                Ok(ok_output(&runs_body(vec![run_row(
                    990,
                    &sha_of('b'),
                    "main",
                    "in_progress",
                    &[3],
                )]))),
            );
        }
        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;
        assert_eq!(
            persisted_of(dir.path(), &feature),
            expected_ci(PersistedCiState::Running, &[901, 902], &[76, 77], None)
        );
        assert_eq!(
            persisted_of(dir.path(), &default),
            expected_ci(PersistedCiState::Idle, &[], &[], None),
            "the default branch is suppressed"
        );

        // e. identical to the default branch: suppressed, detail cleared.
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.route(
                &format!(
                    "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
                    sha_of('a')
                ),
                Ok(ok_output(&runs_body(vec![run_row(
                    903,
                    &sha_of('a'),
                    "feature-a",
                    "in_progress",
                    &[78],
                )]))),
            );
            gh.route(
                &format!(
                    "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
                    sha_of('b')
                ),
                Ok(ok_output(&ci_body(&sha_of('b'), &[]))),
            );
            gh.route(
                &format!("repos/mblua/AgentsCommander/compare/HEAD...{}", sha_of('a')),
                Ok(ok_output(&compare_body("identical", 0, 0))),
            );
        }
        tick(&mut now, &mut wall, 3600);
        harness.round(now, wall).await;
        assert_eq!(
            persisted_of(dir.path(), &feature),
            expected_ci(PersistedCiState::Idle, &[], &[], None)
        );
    }
    fn ci_jobs_body(
        run_id: u64,
        sha40: &str,
        branch: &str,
        attempt: u64,
        statuses: &[&str],
    ) -> String {
        let jobs: Vec<_> = statuses
            .iter()
            .enumerate()
            .map(|(i, status)| {
                serde_json::json!({
                    "id": i + 1, "run_id": run_id, "head_sha": sha40, "head_branch": branch,
                    "run_attempt": attempt, "status": status,
                    "conclusion": if *status == "completed" { Some("success") } else { None },
                })
            })
            .collect();
        serde_json::json!({"total_count": jobs.len(), "jobs": jobs}).to_string()
    }

    fn queue_endpoint(h: &Harness, endpoint: &str, result: Result<GhCallOutput, FailureKind>) {
        h.gh.lock()
            .unwrap_or_else(|e| e.into_inner())
            .route(endpoint, result);
    }

    fn queue_completion(h: &Harness, nwo: &str, row: serde_json::Value) {
        let id = row["id"].as_u64().unwrap();
        let attempt = row["run_attempt"].as_u64().unwrap();
        let detail = format!("repos/{nwo}/actions/runs/{id}");
        queue_endpoint(h, &detail, Ok(ok_output(&row.to_string())));
        queue_endpoint(
            h,
            &format!("{detail}/attempts/{attempt}/jobs?per_page=100"),
            Ok(ok_output(&ci_jobs_body(
                id,
                row["head_sha"].as_str().unwrap(),
                row["head_branch"].as_str().unwrap(),
                attempt,
                &["completed"],
            ))),
        );
        queue_endpoint(h, &detail, Ok(ok_output(&row.to_string())));
    }

    fn i2768_key() -> QueryKey {
        QueryKey {
            nwo: "mblua/AgentsCommander".to_string(),
            sha40: sha_of('a'),
            branch: "main".to_string(),
        }
    }

    fn i2768_harness() -> (Harness, String) {
        let h = Harness::new(AppSettings {
            branch_staleness_enabled: false,
            ..AppSettings::default()
        });
        let repo = h.repo("i2768");
        h.set_work(std::slice::from_ref(&repo));
        queue_endpoint(&h, "repos/mblua/AgentsCommander", Err(FailureKind::Other));
        (h, repo)
    }

    fn i2768_list(h: &Harness, rows: Vec<serde_json::Value>) {
        queue_endpoint(
            h,
            &format!(
                "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
                sha_of('a')
            ),
            Ok(ok_output(&runs_body(rows))),
        );
    }

    fn i2768_row(id: u64, status: &str) -> serde_json::Value {
        run_row(id, &sha_of('a'), "main", status, &[id + 100])
    }

    fn i2768_assert_edges(h: &Harness, started: usize, finished: usize) {
        let edges = h.drain_transitions();
        println!(
            "edge_counts started={} finished={} expected_started={started} expected_finished={finished}",
            edges.iter().filter(|edge| edge.kind == TransitionKind::CiStarted).count(),
            edges.iter().filter(|edge| edge.kind == TransitionKind::CiFinished).count(),
        );
        assert_eq!(
            edges
                .iter()
                .filter(|edge| edge.kind == TransitionKind::CiFinished)
                .count(),
            finished,
            "finished edge count"
        );
        assert_eq!(
            edges
                .iter()
                .filter(|edge| edge.kind == TransitionKind::CiStarted)
                .count(),
            started,
            "started edge count"
        );
    }

    fn i2768_assert_running(h: &Harness, repo: &str) {
        assert_eq!(h.snapshot()[repo].ci, CiState::Running);
        assert_eq!(h.sweeper.lock_state().keys[&i2768_key()].ci.failure, None);
    }

    fn i2768_assert_consumed(h: &Harness) {
        let gh = h.gh.lock().unwrap_or_else(|e| e.into_inner());
        for (endpoint, queue) in &gh.routes {
            assert!(
                queue.is_empty(),
                "unconsumed endpoint {endpoint}: {}",
                queue.len()
            );
        }
        assert!(gh.ci.is_empty() && gh.compare.is_empty() && gh.repo_info.is_empty());
    }

    fn i2768_calls(h: &Harness) -> Vec<String> {
        h.gh.lock()
            .unwrap_or_else(|e| e.into_inner())
            .calls
            .iter()
            .map(|args| args[1].clone())
            .collect()
    }

    fn i2768_details(h: &Harness) -> Vec<String> {
        i2768_calls(h)
            .into_iter()
            .filter(|endpoint| endpoint.contains("/actions/runs/"))
            .collect()
    }

    #[tokio::test]
    async fn i2768_missing_active_run_does_not_finish() {
        let _guard = round_test_lock().await;
        for empty in [false, true] {
            let (h, repo) = i2768_harness();
            h.sweeper.settings.write().await.ci_sweep_min_interval_secs = 10;
            let mut now = Instant::now();
            let mut wall = Local::now();
            i2768_list(&h, vec![]);
            h.round(now, wall).await;
            i2768_list(
                &h,
                vec![i2768_row(5, "completed"), i2768_row(50, "in_progress")],
            );
            tick(&mut now, &mut wall, 60);
            h.round(now, wall).await;
            let mut expected = Vec::new();
            for _ in 0..2 {
                queue_completion(&h, &i2768_key().nwo, i2768_row(5, "completed"));
                queue_endpoint(
                    &h,
                    "repos/mblua/AgentsCommander/actions/runs/50",
                    Ok(ok_output(&i2768_row(50, "in_progress").to_string())),
                );
                for _ in 0..2 {
                    i2768_list(
                        &h,
                        if empty {
                            vec![]
                        } else {
                            vec![i2768_row(5, "completed")]
                        },
                    );
                    tick(&mut now, &mut wall, 10);
                    h.round(now, wall).await;
                }
                expected.extend(
                    [
                        "repos/mblua/AgentsCommander/actions/runs/5",
                        "repos/mblua/AgentsCommander/actions/runs/5/attempts/1/jobs?per_page=100",
                        "repos/mblua/AgentsCommander/actions/runs/5",
                        "repos/mblua/AgentsCommander/actions/runs/50",
                    ]
                    .map(str::to_string),
                );
            }
            i2768_list(&h, vec![i2768_row(50, "in_progress")]);
            tick(&mut now, &mut wall, 10);
            h.round(now, wall).await;
            // Edge assertion comes first so the M1/M2 probes fail on behavior.
            i2768_assert_edges(&h, 1, 0);
            i2768_assert_running(&h, &repo);
            assert!(h.sweeper.lock_state().keys[&i2768_key()]
                .ci
                .completion
                .known
                .contains_key(&50));
            assert_eq!(i2768_details(&h), expected);
            for id in [5, 50] {
                i2768_list(
                    &h,
                    vec![i2768_row(5, "completed"), i2768_row(50, "completed")],
                );
                queue_completion(&h, &i2768_key().nwo, i2768_row(id, "completed"));
                tick(&mut now, &mut wall, 10);
                h.round(now, wall).await;
                i2768_assert_running(&h, &repo);
            }
            i2768_list(&h, vec![]);
            tick(&mut now, &mut wall, 10);
            h.round(now, wall).await;
            i2768_assert_edges(&h, 0, 1);
            assert_eq!(h.snapshot()[&repo].ci, CiState::Idle);
            i2768_assert_consumed(&h);
        }
    }

    #[tokio::test]
    async fn i2768_jobs_contradict_terminal_run() {
        let _guard = round_test_lock().await;
        let (h, repo) = i2768_harness();
        let mut now = Instant::now();
        let mut wall = Local::now();
        i2768_list(&h, vec![]);
        h.round(now, wall).await;
        i2768_list(&h, vec![i2768_row(50, "in_progress")]);
        tick(&mut now, &mut wall, 60);
        h.round(now, wall).await;
        let endpoint = "repos/mblua/AgentsCommander/actions/runs/50";
        let jobs = format!("{endpoint}/attempts/1/jobs?per_page=100");
        // Three terminal detail responses include the C responses needed by M3.
        for _ in 0..4 {
            queue_endpoint(
                &h,
                endpoint,
                Ok(ok_output(&i2768_row(50, "completed").to_string())),
            );
        }
        for _ in 0..2 {
            queue_endpoint(
                &h,
                &jobs,
                Ok(ok_output(&ci_jobs_body(
                    50,
                    &sha_of('a'),
                    "main",
                    1,
                    &["in_progress"],
                ))),
            );
            i2768_list(&h, vec![i2768_row(50, "completed")]);
            tick(&mut now, &mut wall, 10);
            h.round(now, wall).await;
        }
        // M3 can accept on its second false list; baseline has no proof.
        i2768_assert_edges(&h, 1, 0);
        i2768_assert_running(&h, &repo);
        assert!(h.sweeper.lock_state().keys[&i2768_key()]
            .ci
            .completion
            .proofs
            .is_empty());
        assert_eq!(
            i2768_details(&h),
            vec![
                endpoint.to_string(),
                jobs.clone(),
                endpoint.to_string(),
                jobs.clone()
            ]
        );
        queue_endpoint(
            &h,
            &jobs,
            Ok(ok_output(&ci_jobs_body(
                50,
                &sha_of('a'),
                "main",
                1,
                &["completed"],
            ))),
        );
        i2768_list(&h, vec![i2768_row(50, "completed")]);
        tick(&mut now, &mut wall, 10);
        h.round(now, wall).await;
        i2768_assert_edges(&h, 0, 0);
        i2768_list(&h, vec![]);
        tick(&mut now, &mut wall, 10);
        h.round(now, wall).await;
        i2768_assert_edges(&h, 0, 1);
        i2768_assert_consumed(&h);
    }

    #[tokio::test]
    async fn i2768_closing_round_is_required() {
        let _guard = round_test_lock().await;
        let (h, repo) = i2768_harness();
        let mut now = Instant::now();
        let mut wall = Local::now();
        i2768_list(&h, vec![i2768_row(1, "in_progress")]);
        h.round(now, wall).await;
        let confirmed_wall = wall;
        i2768_list(&h, vec![i2768_row(1, "completed")]);
        queue_completion(&h, &i2768_key().nwo, i2768_row(1, "completed"));
        tick(&mut now, &mut wall, 10);
        h.round(now, wall).await;
        i2768_assert_edges(&h, 0, 0);
        i2768_assert_running(&h, &repo);
        {
            let state = h.sweeper.lock_state();
            let ci = &state.keys[&i2768_key()].ci;
            assert_eq!(ci.last_confirmed_at, Some(confirmed_wall));
            assert_eq!(ci.run_ids, vec![1]);
            assert_eq!(ci.completion.proofs.len(), 1);
        }
        i2768_list(&h, vec![]);
        tick(&mut now, &mut wall, 10);
        h.round(now, wall).await;
        let edges = h.drain_transitions();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].kind, TransitionKind::CiFinished);
        assert_eq!(edges[0].observed_at, wall);
        assert_eq!(edges[0].last_confirmed_at, Some(confirmed_wall));
        assert_eq!(i2768_details(&h).len(), 3);
        i2768_assert_consumed(&h);
    }
    #[tokio::test]
    async fn i2768_stale_completed_list_does_not_finish() {
        let _guard = round_test_lock().await;
        let (h, repo) = i2768_harness();
        let mut now = Instant::now();
        let mut wall = Local::now();
        i2768_list(&h, vec![i2768_row(50, "in_progress")]);
        h.round(now, wall).await;
        let list = "repos/mblua/AgentsCommander/actions/runs/50";
        for _ in 0..2 {
            i2768_list(&h, vec![i2768_row(50, "completed")]);
            queue_endpoint(
                &h,
                list,
                Ok(ok_output(&i2768_row(50, "in_progress").to_string())),
            );
            tick(&mut now, &mut wall, 10);
            h.round(now, wall).await;
            i2768_assert_edges(&h, 0, 0);
            i2768_assert_running(&h, &repo);
        }
        i2768_list(&h, vec![i2768_row(50, "in_progress")]);
        tick(&mut now, &mut wall, 10);
        h.round(now, wall).await;
        i2768_assert_edges(&h, 0, 0);
        i2768_assert_running(&h, &repo);
        assert_eq!(i2768_details(&h), vec![list.to_string(); 2]);
        i2768_assert_consumed(&h);
    }

    #[tokio::test]
    async fn i2768_all_runs_finish_once() {
        let _guard = round_test_lock().await;
        let (h, repo) = i2768_harness();
        let mut now = Instant::now();
        let mut wall = Local::now();
        let rows = |status| {
            (1..=12)
                .rev()
                .map(|id| {
                    let mut row = i2768_row(id, status);
                    row["workflow_id"] = serde_json::json!(700 + id % 2);
                    row
                })
                .collect::<Vec<_>>()
        };
        i2768_list(&h, rows("in_progress"));
        h.round(now, wall).await;
        let confirmed_wall = wall;
        let proof_start = now + Duration::from_secs(10);
        let mut expected = vec![
            "repos/mblua/AgentsCommander".to_string(),
            format!(
                "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
                sha_of('a')
            ),
        ];
        for id in 1..=12 {
            let mut row = i2768_row(id, "completed");
            row["workflow_id"] = serde_json::json!(700 + id % 2);
            if id == 7 {
                let proofs = h.sweeper.lock_state().keys[&i2768_key()]
                    .ci
                    .completion
                    .proofs
                    .clone();
                assert_eq!(proofs.len(), 6);
                i2768_list(&h, rows("completed"));
                queue_endpoint(
                    &h,
                    "repos/mblua/AgentsCommander/actions/runs/7",
                    Ok(ok_output(&row.to_string())),
                );
                queue_endpoint(
                    &h,
                    "repos/mblua/AgentsCommander/actions/runs/7/attempts/1/jobs?per_page=100",
                    Err(FailureKind::Timeout),
                );
                tick(&mut now, &mut wall, 10);
                h.round(now, wall).await;
                i2768_assert_edges(&h, 0, 0);
                assert_eq!(h.snapshot()[&repo].ci, CiState::Unknown);
                {
                    let state = h.sweeper.lock_state();
                    let ci = &state.keys[&i2768_key()].ci;
                    assert_eq!(ci.failure, Some(FailureKind::Timeout));
                    assert_eq!(ci.completion.proofs, proofs);
                    assert!(!ci.completion.closing_ready);
                }
                expected.extend([
                    format!(
                        "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
                        sha_of('a')
                    ),
                    "repos/mblua/AgentsCommander/actions/runs/7".to_string(),
                    "repos/mblua/AgentsCommander/actions/runs/7/attempts/1/jobs?per_page=100"
                        .to_string(),
                ]);
            }
            i2768_list(&h, rows("completed"));
            queue_completion(&h, &i2768_key().nwo, row);
            tick(&mut now, &mut wall, if id == 7 { 20 } else { 10 });
            h.round(now, wall).await;
            i2768_assert_edges(&h, 0, 0);
            i2768_assert_running(&h, &repo);
            {
                let state = h.sweeper.lock_state();
                let ci = &state.keys[&i2768_key()].ci;
                assert_eq!(ci.completion.proofs.len(), id as usize);
                assert_eq!(ci.last_confirmed_at, Some(confirmed_wall));
                assert_eq!(ci.run_ids, (1..=12).rev().collect::<Vec<_>>());
                assert!(state.budget_tokens >= 0.0);
            }
            expected.extend([
                format!(
                    "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
                    sha_of('a')
                ),
                format!("repos/mblua/AgentsCommander/actions/runs/{id}"),
                format!(
                    "repos/mblua/AgentsCommander/actions/runs/{id}/attempts/1/jobs?per_page=100"
                ),
                format!("repos/mblua/AgentsCommander/actions/runs/{id}"),
            ]);
        }
        i2768_list(&h, rows("completed"));
        tick(&mut now, &mut wall, 10);
        h.round(now, wall).await;
        i2768_assert_edges(&h, 0, 1);
        assert_eq!(h.snapshot()[&repo].ci, CiState::Idle);
        assert_eq!(now.duration_since(proof_start), Duration::from_secs(140));
        expected.push(format!(
            "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
            sha_of('a')
        ));
        assert_eq!(i2768_calls(&h), expected);
        {
            let state = h.sweeper.lock_state();
            let ci = &state.keys[&i2768_key()].ci;
            assert!(ci.run_ids.is_empty() && ci.pull_requests.is_empty());
        }
        println!(
            "N=12 proofs=12 timeout=B7 elapsed_first_proof_to_accept=140s calls={} budget={}",
            h.gh_call_count(),
            h.sweeper.lock_state().budget_tokens
        );
        i2768_list(&h, rows("completed"));
        tick(&mut now, &mut wall, 60);
        h.round(now, wall).await;
        i2768_assert_edges(&h, 0, 0);
        i2768_assert_consumed(&h);
    }

    #[test]
    fn i2768_parser_filter_and_count_contract() {
        let key = i2768_key();
        for total in [0, 2] {
            let body =
                serde_json::json!({"total_count":total,"workflow_runs":[i2768_row(1,"completed")]})
                    .to_string();
            assert_eq!(
                parse_ci_response(&body, &key.branch, &key.sha40),
                Err(FailureKind::Incomplete)
            );
        }
        let mut bad = i2768_row(2, "completed");
        bad["id"] = serde_json::Value::Null;
        for branch in [
            serde_json::Value::Null,
            serde_json::json!(42),
            serde_json::json!("other"),
        ] {
            bad["head_branch"] = branch;
            let body = runs_body(vec![bad.clone(), i2768_row(1, "completed")]);
            let answer = parse_ci_response(&body, &key.branch, &key.sha40).unwrap();
            assert_eq!(answer.observations.len(), 1);
        }
        bad.as_object_mut().unwrap().remove("head_branch");
        assert!(
            parse_ci_response(&runs_body(vec![bad.clone()]), &key.branch, &key.sha40)
                .unwrap()
                .observations
                .is_empty()
        );
        bad["head_branch"] = serde_json::json!("main");
        assert_eq!(
            parse_ci_response(&runs_body(vec![bad]), &key.branch, &key.sha40),
            Err(FailureKind::Other)
        );
        for conclusion in [
            serde_json::Value::Null,
            serde_json::json!(""),
            serde_json::json!("   "),
            serde_json::json!(9),
        ] {
            let mut row = i2768_row(1, "completed");
            row["conclusion"] = conclusion;
            assert_eq!(
                parse_ci_response(&runs_body(vec![row]), &key.branch, &key.sha40),
                Err(FailureKind::Incomplete)
            );
        }
        let mut no_conclusion = i2768_row(1, "completed");
        no_conclusion.as_object_mut().unwrap().remove("conclusion");
        assert_eq!(
            parse_ci_response(&runs_body(vec![no_conclusion]), &key.branch, &key.sha40),
            Err(FailureKind::Incomplete)
        );
        for status in [None, Some("future")] {
            let mut row = i2768_row(1, "in_progress");
            if let Some(status) = status {
                row["status"] = serde_json::json!(status);
            } else {
                row.as_object_mut().unwrap().remove("status");
            }
            assert_eq!(
                parse_ci_response(&runs_body(vec![row]), &key.branch, &key.sha40)
                    .unwrap()
                    .state,
                CiState::Running
            );
        }
        for conclusion in ["failure", "cancelled", "skipped", "timed_out"] {
            let mut row = i2768_row(1, "completed");
            row["conclusion"] = serde_json::json!(conclusion);
            assert_eq!(
                parse_ci_response(&runs_body(vec![row]), &key.branch, &key.sha40)
                    .unwrap()
                    .state,
                CiState::Idle
            );
        }
        let row = i2768_row(1, "completed");
        assert_eq!(
            parse_ci_response(&runs_body(vec![row.clone(), row]), &key.branch, &key.sha40),
            Err(FailureKind::Other)
        );
        for branch in [serde_json::Value::Null, serde_json::json!("other")] {
            let mut row = i2768_row(1, "completed");
            row["head_branch"] = branch;
            assert_eq!(
                parse_ci_run_response(&row.to_string(), &key),
                Err(FailureKind::Other)
            );
        }
    }
    #[tokio::test]
    async fn i2768_identity_and_attempt_fail_closed() {
        let _guard = round_test_lock().await;
        for (field, value) in [
            ("head_sha", serde_json::json!(sha_of('b'))),
            ("head_branch", serde_json::json!("other")),
            ("workflow_id", serde_json::json!(701)),
            ("id", serde_json::json!(50)),
            ("id", serde_json::Value::Null),
            ("run_attempt", serde_json::json!(0)),
            ("updated_at", serde_json::json!("not a date")),
        ] {
            let (h, repo) = i2768_harness();
            let mut now = Instant::now();
            let mut wall = Local::now();
            i2768_list(&h, vec![i2768_row(1, "in_progress")]);
            h.round(now, wall).await;
            let old = h.sweeper.lock_state().keys[&i2768_key()]
                .ci
                .completion
                .known
                .clone();
            let mut bad = i2768_row(1, "completed");
            bad[field] = value;
            queue_endpoint(
                &h,
                "repos/mblua/AgentsCommander/actions/runs/1",
                Ok(ok_output(&bad.to_string())),
            );
            i2768_list(&h, vec![]);
            tick(&mut now, &mut wall, 10);
            h.round(now, wall).await;
            i2768_assert_edges(&h, 0, 0);
            assert_eq!(h.snapshot()[&repo].ci, CiState::Unknown);
            let state = h.sweeper.lock_state();
            let ci = &state.keys[&i2768_key()].ci;
            assert_eq!(ci.failure, Some(FailureKind::Other));
            assert_eq!(ci.completion.known, old);
            drop(state);
            i2768_assert_consumed(&h);
        }
        let mut ledger = CiCompletionState::default();
        let mut row = i2768_row(1, "in_progress");
        row["run_attempt"] = serde_json::json!(2);
        let high = parse_ci_run_response(&row.to_string(), &i2768_key()).unwrap();
        merge_ci_observations(&mut ledger, std::slice::from_ref(&high), true).unwrap();
        let low =
            parse_ci_run_response(&i2768_row(1, "completed").to_string(), &i2768_key()).unwrap();
        assert_eq!(
            merge_ci_observations(&mut ledger, &[low], true),
            Err(FailureKind::Incomplete)
        );
        assert_eq!(ledger.known[&1], high);
        assert_eq!(
            parse_ci_run_response("{", &i2768_key()),
            Err(FailureKind::Other)
        );
        let mut row = i2768_row(1, "completed");
        row.as_object_mut().unwrap().remove("conclusion");
        assert_eq!(
            parse_ci_run_response(&row.to_string(), &i2768_key()),
            Err(FailureKind::Incomplete)
        );
        let rows = vec![i2768_row(1, "completed"), i2768_row(2, "completed")];
        let answer = parse_ci_response(&runs_body(rows), "main", &sha_of('a')).unwrap();
        assert_eq!(answer.observations.len(), 2);
    }

    #[tokio::test]
    async fn i2768_incomplete_evidence_never_finishes() {
        let _guard = round_test_lock().await;
        let run =
            parse_ci_run_response(&i2768_row(1, "completed").to_string(), &i2768_key()).unwrap();
        for count in [0, 2, 101] {
            let mut jobs: serde_json::Value =
                serde_json::from_str(&ci_jobs_body(1, &sha_of('a'), "main", 1, &["completed"]))
                    .unwrap();
            jobs["total_count"] = serde_json::json!(count);
            assert_eq!(
                parse_ci_jobs_response(&jobs.to_string(), &run),
                Err(FailureKind::Incomplete)
            );
        }
        let jobs = ci_jobs_body(1, &sha_of('a'), "main", 1, &["completed"; 101]);
        assert_eq!(
            parse_ci_jobs_response(&jobs, &run),
            Err(FailureKind::Incomplete)
        );
        for (field, value, kind) in [
            ("run_id", serde_json::json!(50), FailureKind::Other),
            (
                "head_sha",
                serde_json::json!(sha_of('b')),
                FailureKind::Other,
            ),
            ("run_attempt", serde_json::Value::Null, FailureKind::Other),
            ("head_branch", serde_json::Value::Null, FailureKind::Other),
            ("id", serde_json::json!(0), FailureKind::Other),
            (
                "conclusion",
                serde_json::Value::Null,
                FailureKind::Incomplete,
            ),
        ] {
            let (h, repo) = i2768_harness();
            let mut now = Instant::now();
            let mut wall = Local::now();
            i2768_list(&h, vec![i2768_row(1, "in_progress")]);
            h.round(now, wall).await;
            i2768_list(&h, vec![i2768_row(1, "completed")]);
            queue_endpoint(
                &h,
                "repos/mblua/AgentsCommander/actions/runs/1",
                Ok(ok_output(&i2768_row(1, "completed").to_string())),
            );
            let mut jobs: serde_json::Value =
                serde_json::from_str(&ci_jobs_body(1, &sha_of('a'), "main", 1, &["completed"]))
                    .unwrap();
            jobs["jobs"][0][field] = value;
            queue_endpoint(
                &h,
                "repos/mblua/AgentsCommander/actions/runs/1/attempts/1/jobs?per_page=100",
                Ok(ok_output(&jobs.to_string())),
            );
            tick(&mut now, &mut wall, 10);
            h.round(now, wall).await;
            i2768_assert_edges(&h, 0, 0);
            assert_eq!(h.snapshot()[&repo].ci, CiState::Unknown);
            assert_eq!(
                h.sweeper.lock_state().keys[&i2768_key()].ci.failure,
                Some(kind)
            );
            i2768_assert_consumed(&h);
        }
        let mut jobs: serde_json::Value =
            serde_json::from_str(&ci_jobs_body(1, &sha_of('a'), "main", 1, &["completed"]))
                .unwrap();
        jobs["jobs"][0].as_object_mut().unwrap().remove("status");
        assert_eq!(parse_ci_jobs_response(&jobs.to_string(), &run), Ok(false));
        let mut jobs: serde_json::Value =
            serde_json::from_str(&ci_jobs_body(1, &sha_of('a'), "main", 1, &["completed"]))
                .unwrap();
        jobs["jobs"][0]
            .as_object_mut()
            .unwrap()
            .remove("run_attempt");
        jobs["jobs"][0]
            .as_object_mut()
            .unwrap()
            .remove("head_branch");
        assert_eq!(parse_ci_jobs_response(&jobs.to_string(), &run), Ok(true));
        let duplicate = jobs["jobs"][0].clone();
        jobs["jobs"].as_array_mut().unwrap().push(duplicate);
        jobs["total_count"] = serde_json::json!(2);
        assert_eq!(
            parse_ci_jobs_response(&jobs.to_string(), &run),
            Err(FailureKind::Other)
        );
        let mut ledger = CiCompletionState::default();
        let runs = (1..=100)
            .map(|id| {
                parse_ci_run_response(&i2768_row(id, "completed").to_string(), &i2768_key())
                    .unwrap()
            })
            .collect::<Vec<_>>();
        merge_ci_observations(&mut ledger, &runs, true).unwrap();
        let overflow =
            parse_ci_run_response(&i2768_row(101, "completed").to_string(), &i2768_key()).unwrap();
        assert_eq!(
            merge_ci_observations(&mut ledger, &[overflow], true),
            Err(FailureKind::Incomplete)
        );
        assert!(ledger.overflow);
        assert_eq!(
            merge_ci_observations(&mut ledger, &[], true),
            Err(FailureKind::Incomplete)
        );
        assert_eq!(ledger.known.len(), 100);
        // A cancelled run with no jobs still needs matching latest details.
        let (h, repo) = i2768_harness();
        let mut now = Instant::now();
        let mut wall = Local::now();
        i2768_list(&h, vec![i2768_row(1, "in_progress")]);
        h.round(now, wall).await;
        for changed in [true, false] {
            i2768_list(&h, vec![i2768_row(1, "completed")]);
            let a = i2768_row(1, "completed");
            let mut c = a.clone();
            if changed {
                c["updated_at"] = serde_json::json!("2026-10-01T00:00:01Z");
            }
            queue_endpoint(
                &h,
                "repos/mblua/AgentsCommander/actions/runs/1",
                Ok(ok_output(&a.to_string())),
            );
            queue_endpoint(
                &h,
                "repos/mblua/AgentsCommander/actions/runs/1/attempts/1/jobs?per_page=100",
                Ok(ok_output(&ci_jobs_body(1, &sha_of('a'), "main", 1, &[]))),
            );
            queue_endpoint(
                &h,
                "repos/mblua/AgentsCommander/actions/runs/1",
                Ok(ok_output(&c.to_string())),
            );
            tick(&mut now, &mut wall, 10);
            h.round(now, wall).await;
            i2768_assert_edges(&h, 0, 0);
            i2768_assert_running(&h, &repo);
            assert_eq!(
                h.sweeper.lock_state().keys[&i2768_key()]
                    .ci
                    .completion
                    .proofs
                    .len(),
                usize::from(!changed)
            );
        }
        i2768_list(&h, vec![]);
        tick(&mut now, &mut wall, 10);
        h.round(now, wall).await;
        i2768_assert_edges(&h, 0, 1);
        i2768_assert_consumed(&h);
    }

    #[tokio::test]
    async fn i2768_rerun_invalidates_completion() {
        let _guard = round_test_lock().await;
        for point in ["A", "C", "closing"] {
            let (h, repo) = i2768_harness();
            let mut now = Instant::now();
            let mut wall = Local::now();
            i2768_list(&h, vec![]);
            h.round(now, wall).await;
            i2768_list(&h, vec![i2768_row(1, "in_progress")]);
            tick(&mut now, &mut wall, 60);
            h.round(now, wall).await;
            i2768_assert_edges(&h, 1, 0);
            let terminal = i2768_row(1, "completed");
            let mut active = i2768_row(1, "in_progress");
            active["run_attempt"] = serde_json::json!(2);
            i2768_list(&h, vec![terminal.clone()]);
            match point {
                "A" => queue_endpoint(
                    &h,
                    "repos/mblua/AgentsCommander/actions/runs/1",
                    Ok(ok_output(&active.to_string())),
                ),
                "C" => {
                    queue_endpoint(
                        &h,
                        "repos/mblua/AgentsCommander/actions/runs/1",
                        Ok(ok_output(&terminal.to_string())),
                    );
                    queue_endpoint(
                        &h,
                        "repos/mblua/AgentsCommander/actions/runs/1/attempts/1/jobs?per_page=100",
                        Ok(ok_output(&ci_jobs_body(
                            1,
                            &sha_of('a'),
                            "main",
                            1,
                            &["completed"],
                        ))),
                    );
                    queue_endpoint(
                        &h,
                        "repos/mblua/AgentsCommander/actions/runs/1",
                        Ok(ok_output(&active.to_string())),
                    );
                }
                _ => queue_completion(&h, &i2768_key().nwo, terminal),
            }
            tick(&mut now, &mut wall, 10);
            h.round(now, wall).await;
            if point == "closing" {
                i2768_list(&h, vec![active.clone()]);
                tick(&mut now, &mut wall, 10);
                h.round(now, wall).await;
            }
            i2768_assert_edges(&h, 0, 0);
            i2768_assert_running(&h, &repo);
            {
                let state = h.sweeper.lock_state();
                let ledger = &state.keys[&i2768_key()].ci.completion;
                assert_eq!(ledger.known[&1].run_attempt, 2);
                assert!(ledger.proofs.is_empty());
            }
            let mut terminal = i2768_row(1, "completed");
            terminal["run_attempt"] = serde_json::json!(2);
            i2768_list(&h, vec![terminal.clone()]);
            queue_completion(&h, &i2768_key().nwo, terminal);
            tick(&mut now, &mut wall, 10);
            h.round(now, wall).await;
            i2768_list(&h, vec![]);
            tick(&mut now, &mut wall, 10);
            h.round(now, wall).await;
            i2768_assert_edges(&h, 0, 1);
            // A real rerun after accepted completion is a new legitimate pair.
            active["run_attempt"] = serde_json::json!(3);
            i2768_list(&h, vec![active]);
            tick(&mut now, &mut wall, 60);
            h.round(now, wall).await;
            i2768_assert_edges(&h, 1, 0);
            let mut terminal = i2768_row(1, "completed");
            terminal["run_attempt"] = serde_json::json!(3);
            i2768_list(&h, vec![terminal.clone()]);
            queue_completion(&h, &i2768_key().nwo, terminal);
            tick(&mut now, &mut wall, 10);
            h.round(now, wall).await;
            i2768_list(&h, vec![]);
            tick(&mut now, &mut wall, 10);
            h.round(now, wall).await;
            i2768_assert_edges(&h, 0, 1);
            i2768_assert_consumed(&h);
        }
    }
    #[tokio::test]
    async fn i2768_failures_preserve_known_runs() {
        let _guard = round_test_lock().await;
        let failures = [
            Err(FailureKind::Timeout),
            Ok(ok_output("{")),
            Ok(GhCallOutput {
                success: false,
                stdout: String::new(),
                stderr: "HTTP 401".to_string(),
            }),
            Ok(GhCallOutput {
                success: false,
                stdout: String::new(),
                stderr: "HTTP 404 deleted".to_string(),
            }),
            Ok(primary_limit_output()),
            Ok(secondary_limit_output()),
            Err(FailureKind::Other),
        ];
        let kinds = [
            FailureKind::Timeout,
            FailureKind::Other,
            FailureKind::NotAuthenticated,
            FailureKind::Other,
            FailureKind::RateLimited,
            FailureKind::SecondaryRateLimited,
            FailureKind::Other,
        ];
        for point in ["A", "B", "C", "list", "compare"] {
            for (failure, kind) in failures.iter().zip(kinds) {
                let failure = || match failure {
                    Ok(output) => Ok(GhCallOutput {
                        success: output.success,
                        stdout: output.stdout.clone(),
                        stderr: output.stderr.clone(),
                    }),
                    Err(kind) => Err(*kind),
                };
                let (h, repo) = i2768_harness();
                let mut now = Instant::now();
                let mut wall = Local::now();
                let terminal = || vec![i2768_row(1, "completed"), i2768_row(2, "completed")];
                i2768_list(
                    &h,
                    vec![i2768_row(1, "in_progress"), i2768_row(2, "in_progress")],
                );
                h.round(now, wall).await;
                i2768_list(&h, terminal());
                queue_completion(&h, &i2768_key().nwo, i2768_row(1, "completed"));
                tick(&mut now, &mut wall, 10);
                h.round(now, wall).await;
                let proofs = h.sweeper.lock_state().keys[&i2768_key()]
                    .ci
                    .completion
                    .proofs
                    .clone();
                assert_eq!(proofs.len(), 1);
                if point == "list" {
                    queue_endpoint(
                        &h,
                        &format!(
                            "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
                            sha_of('a')
                        ),
                        failure(),
                    );
                } else {
                    i2768_list(&h, terminal());
                }
                if point == "compare" {
                    h.sweeper
                        .lock_state()
                        .base_branches
                        .insert(i2768_key().nwo, "base".to_string());
                    queue_endpoint(
                        &h,
                        &format!("repos/mblua/AgentsCommander/compare/HEAD...{}", sha_of('a')),
                        failure(),
                    );
                }
                if ["A", "B", "C"].contains(&point) {
                    let endpoint = "repos/mblua/AgentsCommander/actions/runs/2";
                    queue_endpoint(
                        &h,
                        endpoint,
                        if point == "A" {
                            failure()
                        } else {
                            Ok(ok_output(&i2768_row(2, "completed").to_string()))
                        },
                    );
                    if point != "A" {
                        queue_endpoint(
                            &h,
                            &format!("{endpoint}/attempts/1/jobs?per_page=100"),
                            if point == "B" {
                                failure()
                            } else {
                                Ok(ok_output(&ci_jobs_body(
                                    2,
                                    &sha_of('a'),
                                    "main",
                                    1,
                                    &["completed"],
                                )))
                            },
                        );
                    }
                    if point == "C" {
                        queue_endpoint(&h, endpoint, failure());
                    }
                }
                tick(&mut now, &mut wall, 10);
                h.round(now, wall).await;
                i2768_assert_edges(&h, 0, 0);
                assert_eq!(h.snapshot()[&repo].ci, CiState::Unknown);
                {
                    let state = h.sweeper.lock_state();
                    let ci = &state.keys[&i2768_key()].ci;
                    assert_eq!(ci.failure, Some(kind), "point={point}");
                    assert_eq!(ci.completion.known.len(), 2);
                    assert_eq!(ci.completion.active_ids, vec![1, 2]);
                    assert_eq!(ci.completion.proofs, proofs);
                    assert!(!ci.completion.closing_ready);
                }
                // Three permanent failures remain unknown; no age-based success.
                if point == "A" && kind == FailureKind::Other {
                    for _ in 0..3 {
                        i2768_list(&h, terminal());
                        queue_endpoint(
                            &h,
                            "repos/mblua/AgentsCommander/actions/runs/2",
                            Ok(GhCallOutput {
                                success: false,
                                stdout: String::new(),
                                stderr: "HTTP 404".to_string(),
                            }),
                        );
                        tick(&mut now, &mut wall, 900);
                        h.round(now, wall).await;
                        i2768_assert_edges(&h, 0, 0);
                        assert_eq!(h.snapshot()[&repo].ci, CiState::Unknown);
                    }
                }
                if point == "compare" {
                    h.sweeper
                        .lock_state()
                        .base_branches
                        .insert(i2768_key().nwo, DEFAULT_BRANCH_LABEL.to_string());
                }
                i2768_list(&h, terminal());
                queue_completion(&h, &i2768_key().nwo, i2768_row(2, "completed"));
                tick(&mut now, &mut wall, 900);
                h.round(now, wall).await;
                i2768_assert_running(&h, &repo);
                i2768_assert_edges(&h, 0, 0);
                i2768_list(&h, vec![]);
                tick(&mut now, &mut wall, 10);
                h.round(now, wall).await;
                i2768_assert_edges(&h, 0, 1);
                let details = i2768_details(&h);
                assert_eq!(
                    details
                        .iter()
                        .filter(|endpoint| endpoint.as_str()
                            == "repos/mblua/AgentsCommander/actions/runs/1")
                        .count(),
                    2
                );
                i2768_assert_consumed(&h);
            }
        }
        // Even all old proofs cannot finish through a failed closing list.
        let (h, repo) = i2768_harness();
        let mut now = Instant::now();
        let mut wall = Local::now();
        i2768_list(&h, vec![i2768_row(1, "in_progress")]);
        h.round(now, wall).await;
        i2768_list(&h, vec![]);
        queue_completion(&h, &i2768_key().nwo, i2768_row(1, "completed"));
        tick(&mut now, &mut wall, 10);
        h.round(now, wall).await;
        queue_endpoint(
            &h,
            &format!(
                "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
                sha_of('a')
            ),
            Err(FailureKind::Timeout),
        );
        tick(&mut now, &mut wall, 10);
        h.round(now, wall).await;
        i2768_assert_edges(&h, 0, 0);
        assert_eq!(h.snapshot()[&repo].ci, CiState::Unknown);
        assert!(
            !h.sweeper.lock_state().keys[&i2768_key()]
                .ci
                .completion
                .closing_ready
        );
        i2768_list(&h, vec![]);
        tick(&mut now, &mut wall, 20);
        h.round(now, wall).await;
        i2768_assert_edges(&h, 0, 0);
        i2768_assert_running(&h, &repo);
        i2768_list(&h, vec![]);
        tick(&mut now, &mut wall, 10);
        h.round(now, wall).await;
        i2768_assert_edges(&h, 0, 1);
        assert_eq!(i2768_details(&h).len(), 3);
        i2768_assert_consumed(&h);
    }

    #[tokio::test]
    async fn i2768_closing_list_changes_restart_candidate() {
        let _guard = round_test_lock().await;
        for change in ["new-completed", "new-active", "metadata", "attempt"] {
            let (h, repo) = i2768_harness();
            let mut now = Instant::now();
            let mut wall = Local::now();
            i2768_list(&h, vec![i2768_row(1, "in_progress")]);
            h.round(now, wall).await;
            i2768_list(&h, vec![]);
            queue_completion(&h, &i2768_key().nwo, i2768_row(1, "completed"));
            tick(&mut now, &mut wall, 10);
            h.round(now, wall).await;
            let mut first = i2768_row(1, "completed");
            let mut closing = vec![first.clone()];
            match change {
                "metadata" => {
                    first["updated_at"] = serde_json::json!("2026-10-01T00:00:01Z");
                    closing = vec![first.clone()];
                }
                "attempt" => {
                    first["run_attempt"] = serde_json::json!(2);
                    closing = vec![first.clone()];
                }
                "new-active" => closing.push(i2768_row(2, "in_progress")),
                _ => closing.push(i2768_row(2, "completed")),
            }
            i2768_list(&h, closing);
            if change != "new-active" {
                queue_completion(&h, &i2768_key().nwo, first.clone());
            }
            tick(&mut now, &mut wall, 10);
            h.round(now, wall).await;
            i2768_assert_edges(&h, 0, 0);
            i2768_assert_running(&h, &repo);
            if change == "new-active" {
                assert!(h.sweeper.lock_state().keys[&i2768_key()]
                    .ci
                    .completion
                    .proofs
                    .is_empty());
                i2768_list(&h, vec![first.clone(), i2768_row(2, "completed")]);
                queue_completion(&h, &i2768_key().nwo, first.clone());
                tick(&mut now, &mut wall, 10);
                h.round(now, wall).await;
            }
            if change.starts_with("new") {
                assert_eq!(
                    h.sweeper.lock_state().keys[&i2768_key()]
                        .ci
                        .completion
                        .proofs
                        .len(),
                    1
                );
                i2768_list(&h, vec![first.clone(), i2768_row(2, "completed")]);
                queue_completion(&h, &i2768_key().nwo, i2768_row(2, "completed"));
                tick(&mut now, &mut wall, 10);
                h.round(now, wall).await;
            }
            i2768_list(&h, vec![]);
            tick(&mut now, &mut wall, 10);
            h.round(now, wall).await;
            i2768_assert_edges(&h, 0, 1);
            i2768_assert_consumed(&h);
        }
    }
    #[tokio::test]
    async fn i2768_lifecycle_isolation() {
        let _guard = round_test_lock().await;
        let (h, repo) = i2768_harness();
        let alias = h.repo("alias");
        h.set_work(&[repo.clone(), alias.clone()]);
        let mut now = Instant::now();
        let mut wall = Local::now();
        i2768_list(&h, vec![]);
        h.round(now, wall).await;
        i2768_list(&h, vec![i2768_row(1, "in_progress")]);
        tick(&mut now, &mut wall, 60);
        h.round(now, wall).await;
        i2768_assert_edges(&h, 2, 0);
        let before = {
            let state = h.sweeper.lock_state();
            let ci = &state.keys[&i2768_key()].ci;
            (
                ci.chip,
                ci.confirmed,
                ci.last_confirmed_at,
                ci.next_due,
                ci.failure_interval,
                ci.failure,
                ci.run_ids.clone(),
                ci.pull_requests.clone(),
                ci.completion.clone(),
            )
        };
        h.sweeper.settings.write().await.ci_activity_enabled = false;
        let calls = h.gh_call_count();
        tick(&mut now, &mut wall, 100);
        h.round(now, wall).await;
        {
            let state = h.sweeper.lock_state();
            let ci = &state.keys[&i2768_key()].ci;
            assert_eq!(
                (
                    ci.chip,
                    ci.confirmed,
                    ci.last_confirmed_at,
                    ci.next_due,
                    ci.failure_interval,
                    ci.failure,
                    ci.run_ids.clone(),
                    ci.pull_requests.clone(),
                    ci.completion.clone()
                ),
                before
            );
        }
        assert_eq!(h.gh_call_count(), calls);
        i2768_assert_edges(&h, 0, 0);
        h.sweeper.settings.write().await.ci_activity_enabled = true;
        i2768_list(&h, vec![]);
        queue_completion(&h, &i2768_key().nwo, i2768_row(1, "completed"));
        h.round(now, wall).await;
        i2768_assert_running(&h, &repo);
        i2768_list(&h, vec![]);
        tick(&mut now, &mut wall, 10);
        h.round(now, wall).await;
        i2768_assert_edges(&h, 0, 2);
        i2768_assert_consumed(&h);
        // Distinct heads and branches coexist without carrying CI evidence.
        h.git
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_head_sha(&alias, 'b');
        publish_branch(&alias, "feature");
        queue_endpoint(
            &h,
            &format!(
                "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
                sha_of('b')
            ),
            Ok(ok_output(&runs_body(vec![run_row(
                2,
                &sha_of('b'),
                "feature",
                "in_progress",
                &[],
            )]))),
        );
        tick(&mut now, &mut wall, 1);
        h.round(now, wall).await;
        i2768_assert_edges(&h, 0, 0);
        let second = QueryKey {
            sha40: sha_of('b'),
            branch: "feature".to_string(),
            ..i2768_key()
        };
        {
            let state = h.sweeper.lock_state();
            assert_eq!(state.keys.len(), 2);
            assert_eq!(
                state.keys[&second]
                    .ci
                    .completion
                    .known
                    .keys()
                    .copied()
                    .collect::<Vec<_>>(),
                vec![2]
            );
            assert_eq!(
                state.keys[&i2768_key()]
                    .ci
                    .completion
                    .known
                    .keys()
                    .copied()
                    .collect::<Vec<_>>(),
                vec![1]
            );
        }
        // Suppression clears the ledger and both directions of the chain.
        h.sweeper
            .lock_state()
            .base_branches
            .insert(i2768_key().nwo, "feature".to_string());
        queue_endpoint(
            &h,
            &format!(
                "repos/mblua/AgentsCommander/actions/runs?head_sha={}&per_page=100",
                sha_of('b')
            ),
            Ok(ok_output(&runs_body(vec![run_row(
                2,
                &sha_of('b'),
                "feature",
                "in_progress",
                &[],
            )]))),
        );
        tick(&mut now, &mut wall, 10);
        h.round(now, wall).await;
        {
            let state = h.sweeper.lock_state();
            let ci = &state.keys[&second].ci;
            assert!(ci.completion.known.is_empty());
            assert_eq!(ci.confirmed, None);
        }
        h.sweeper.settings.write().await.ci_activity_enabled = false;
        h.set_work(&[]);
        h.round(now, wall).await;
        assert!(h.sweeper.lock_state().keys.is_empty());
        i2768_assert_edges(&h, 0, 0);
        i2768_assert_consumed(&h);
        let restarted = Harness::new(AppSettings::default());
        assert!(restarted.sweeper.lock_state().keys.is_empty());
    }

    #[tokio::test]
    async fn i2768_budget_accounts_for_corroboration() {
        let _guard = round_test_lock().await;
        for point in ["active", "A", "B", "suppressed", "closing"] {
            let (h, _) = i2768_harness();
            let mut now = Instant::now();
            let mut wall = Local::now();
            i2768_list(&h, vec![i2768_row(1, "in_progress")]);
            h.round(now, wall).await;
            tick(&mut now, &mut wall, 10);
            h.set_budget(3.0);
            let calls = h.gh_call_count();
            let due = h.sweeper.lock_state().keys[&i2768_key()].ci.next_due;
            h.round(now, wall).await;
            assert_eq!(h.gh_call_count(), calls);
            assert_eq!(h.sweeper.lock_state().keys[&i2768_key()].ci.next_due, due);
            h.set_budget(4.0);
            if point == "active" || point == "suppressed" {
                if point == "suppressed" {
                    h.sweeper
                        .lock_state()
                        .base_branches
                        .insert(i2768_key().nwo, "main".to_string());
                }
                i2768_list(&h, vec![i2768_row(1, "in_progress")]);
            } else {
                i2768_list(&h, vec![]);
                if point == "closing" {
                    queue_completion(&h, &i2768_key().nwo, i2768_row(1, "completed"));
                } else {
                    queue_endpoint(
                        &h,
                        "repos/mblua/AgentsCommander/actions/runs/1",
                        if point == "A" {
                            Err(FailureKind::Timeout)
                        } else {
                            Ok(ok_output(&i2768_row(1, "completed").to_string()))
                        },
                    );
                    if point == "B" {
                        queue_endpoint(&h,"repos/mblua/AgentsCommander/actions/runs/1/attempts/1/jobs?per_page=100",Err(FailureKind::Timeout));
                    }
                }
            }
            h.round(now, wall).await;
            let expected = match point {
                "A" => 2.0,
                "B" => 1.0,
                "closing" => 0.0,
                _ => 3.0,
            };
            assert_eq!(h.sweeper.lock_state().budget_tokens, expected);
            if point == "closing" {
                h.set_budget(4.0);
                i2768_list(&h, vec![]);
                tick(&mut now, &mut wall, 10);
                h.set_budget(4.0);
                h.round(now, wall).await;
                assert_eq!(h.sweeper.lock_state().budget_tokens, 3.0);
                i2768_assert_edges(&h, 0, 1);
            } else {
                i2768_assert_edges(&h, 0, 0);
            }
            i2768_assert_consumed(&h);
        }
        // Three Running keys contend with an Idle key in a finite fixture.
        let mut h = Harness::new(AppSettings {
            branch_staleness_enabled: false,
            ..AppSettings::default()
        });
        let inflight = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let peak = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        {
            let s = Arc::get_mut(&mut h.sweeper).unwrap();
            let original = Arc::clone(&s.spawner);
            let live = Arc::clone(&inflight);
            let max = Arc::clone(&peak);
            s.spawner = Arc::new(move |spec| {
                let original = Arc::clone(&original);
                let live = Arc::clone(&live);
                let max = Arc::clone(&max);
                Box::pin(async move {
                    let count = live.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                    max.fetch_max(count, std::sync::atomic::Ordering::SeqCst);
                    tokio::task::yield_now().await;
                    let result = original(spec).await;
                    live.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                    result
                })
            });
        }
        let paths = ["a", "b", "c", "z"].map(|name| {
            let path = h.repo(name);
            h.git
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .set_origin(&path, &format!("git@github.com:mblua/{name}.git"));
            path
        });
        h.set_work(&paths);
        let list = |name: &str| {
            format!(
                "repos/mblua/{name}/actions/runs?head_sha={}&per_page=100",
                sha_of('a')
            )
        };
        for name in ["a", "b", "c", "z"] {
            queue_endpoint(&h, &format!("repos/mblua/{name}"), Err(FailureKind::Other));
            queue_endpoint(
                &h,
                &list(name),
                Ok(ok_output(&runs_body(if name == "z" {
                    vec![]
                } else {
                    vec![i2768_row(1, "in_progress")]
                }))),
            );
        }
        let mut now = Instant::now();
        let mut wall = Local::now();
        h.round(now, wall).await;
        tick(&mut now, &mut wall, 60);
        h.set_budget(3.0);
        queue_endpoint(&h, &list("z"), Ok(ok_output(&runs_body(vec![]))));
        let calls = h.gh_call_count();
        h.round(now, wall).await;
        assert_eq!(h.gh_call_count(), calls + 1);
        for (closing, proof) in [
            (None, Some("a")),
            (Some("a"), Some("b")),
            (Some("b"), Some("c")),
            (Some("c"), None),
        ] {
            if closing == Some("c") {
                queue_endpoint(&h, &list("z"), Ok(ok_output(&runs_body(vec![]))));
            }
            if let Some(name) = closing {
                queue_endpoint(&h, &list(name), Ok(ok_output(&runs_body(vec![]))));
            }
            if let Some(name) = proof {
                queue_endpoint(
                    &h,
                    &list(name),
                    Ok(ok_output(&runs_body(vec![i2768_row(1, "completed")]))),
                );
                queue_completion(&h, &format!("mblua/{name}"), i2768_row(1, "completed"));
            }
            tick(&mut now, &mut wall, 10);
            h.round(now, wall).await;
            assert!(h.sweeper.lock_state().budget_tokens >= 0.0);
            let state = h.sweeper.lock_state();
            println!(
                "contention fixture calls={} tokens={} deferred={:?}",
                h.gh_call_count(),
                state.budget_tokens,
                state
                    .keys
                    .iter()
                    .filter(|(_, v)| v.ci.chip == CiState::Running)
                    .map(|(k, _)| k.nwo.clone())
                    .collect::<Vec<_>>()
            );
        }
        i2768_assert_edges(&h, 0, 3);
        for path in &paths {
            assert_eq!(h.snapshot()[path].ci, CiState::Idle);
        }
        assert_eq!(peak.load(std::sync::atomic::Ordering::SeqCst), 2);
        i2768_assert_consumed(&h);
    }
    #[test]
    fn i2768_scripts_route_exactly_and_panic() {
        let mut scripts = GhScripts::default();
        for (endpoint, text) in [
            ("repos/mblua/AgentsCommander/actions/runs/5", "five"),
            ("repos/mblua/AgentsCommander/actions/runs/50", "fifty"),
            (
                "repos/mblua/AgentsCommander/actions/runs/5/attempts/1/jobs?per_page=100",
                "jobs",
            ),
            ("repos/mblua/AgentsCommander", "repo"),
        ] {
            scripts.route(endpoint, Ok(ok_output(text)));
        }
        let gh = Path::new("gh");
        let nwo = "mblua/AgentsCommander";
        for (query, text) in [
            (GhQuery::CiRun { nwo, run_id: 50 }, "fifty"),
            (
                GhQuery::CiAttemptJobs {
                    nwo,
                    run_id: 5,
                    attempt: 1,
                },
                "jobs",
            ),
            (GhQuery::CiRun { nwo, run_id: 5 }, "five"),
            (GhQuery::RepoInfo { nwo }, "repo"),
        ] {
            assert_eq!(
                scripts
                    .next(&build_gh_command_spec(gh, query).unwrap())
                    .unwrap()
                    .stdout,
                text
            );
        }
        assert_eq!(scripts.count_repo_info(), 1);
        assert!(scripts.routes.values().all(VecDeque::is_empty));
        for query in [
            GhQuery::CiRun { nwo, run_id: 0 },
            GhQuery::CiAttemptJobs {
                nwo,
                run_id: 0,
                attempt: 1,
            },
            GhQuery::CiAttemptJobs {
                nwo,
                run_id: 5,
                attempt: 0,
            },
            GhQuery::CiRun {
                nwo: "bad",
                run_id: 5,
            },
        ] {
            assert!(build_gh_command_spec(gh, query).is_err());
        }
        let detail = build_gh_command_spec(gh, GhQuery::CiRun { nwo, run_id: 5 }).unwrap();
        assert_eq!(
            detail.envs,
            vec![
                ("GH_PROMPT_DISABLED".to_string(), "1".to_string()),
                ("GH_NO_UPDATE_NOTIFIER".to_string(), "1".to_string()),
                ("GH_PAGER".to_string(), "cat".to_string())
            ]
        );
    }

    #[test]
    #[should_panic(expected = "missing gh route")]
    fn i2768_scripts_missing_detail_panics() {
        let mut scripts = GhScripts::default();
        scripts
            .next(
                &build_gh_command_spec(
                    Path::new("gh"),
                    GhQuery::CiRun {
                        nwo: "mblua/AgentsCommander",
                        run_id: 5,
                    },
                )
                .unwrap(),
            )
            .unwrap();
    }

    #[test]
    #[should_panic(expected = "missing gh route")]
    fn i2768_scripts_missing_jobs_panics() {
        let mut scripts = GhScripts::default();
        scripts
            .next(
                &build_gh_command_spec(
                    Path::new("gh"),
                    GhQuery::CiAttemptJobs {
                        nwo: "mblua/AgentsCommander",
                        run_id: 5,
                        attempt: 1,
                    },
                )
                .unwrap(),
            )
            .unwrap();
    }

    #[test]
    #[should_panic(expected = "exhausted gh route")]
    fn i2768_scripts_exhausted_detail_panics() {
        let mut scripts = GhScripts::default();
        let spec = build_gh_command_spec(
            Path::new("gh"),
            GhQuery::CiRun {
                nwo: "mblua/AgentsCommander",
                run_id: 5,
            },
        )
        .unwrap();
        scripts.route(&spec.args[1], Ok(ok_output("{}")));
        scripts.next(&spec).unwrap();
        scripts.next(&spec).unwrap();
    }

    #[test]
    #[should_panic(expected = "exhausted gh route")]
    fn i2768_scripts_exhausted_jobs_panics() {
        let mut scripts = GhScripts::default();
        let spec = build_gh_command_spec(
            Path::new("gh"),
            GhQuery::CiAttemptJobs {
                nwo: "mblua/AgentsCommander",
                run_id: 5,
                attempt: 1,
            },
        )
        .unwrap();
        scripts.route(&spec.args[1], Ok(ok_output("{}")));
        scripts.next(&spec).unwrap();
        scripts.next(&spec).unwrap();
    }
}
