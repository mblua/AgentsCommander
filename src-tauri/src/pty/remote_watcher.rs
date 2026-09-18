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

use chrono::{DateTime, Local};
use futures::stream::StreamExt;
use serde::Serialize;
use tokio::sync::mpsc;

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

/// The label rendered for `%BASE%` when neither GitHub nor the local clone can
/// name the default branch. It is a label in a sentence, never a query argument,
/// so an unresolved label degrades the text and nothing else. A `BranchStale`
/// notice is NEVER suppressed for want of it.
const DEFAULT_BRANCH_LABEL: &str = "the default branch";

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
}

impl RemoteActivity {
    fn unknown() -> Self {
        Self {
            ci: CiState::Unknown,
            staleness: StalenessState::Unknown,
            behind_by: None,
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
    Ci { nwo: &'a str, sha40: &'a str },
    Compare { nwo: &'a str, sha40: &'a str },
    RepoInfo { nwo: &'a str },
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

/// The three process seams. Production wires them in `production_seams`; tests
/// inject recorded results so no test ever spawns `gh` or touches the network.
pub(crate) struct RemoteSweeperSeams {
    probe: GhProbe,
    spawner: GhSpawner,
    local_git: LocalGitRunner,
}

/// Program-independent failure classification, from the text `gh` prints.
/// Deliberately textual: `gh api` reports HTTP outcomes as exit status 1 plus a
/// message, and the exact argument list is pinned by test 1, so no `--include`
/// is available to read headers.
fn failure_kind(output: &GhCallOutput) -> FailureKind {
    let text = output.stderr.to_ascii_lowercase();
    if text.contains("rate limit") || text.contains("x-ratelimit-remaining: 0") {
        FailureKind::RateLimited
    } else if text.contains("not logged into any github hosts") || text.contains("http 401") {
        FailureKind::NotAuthenticated
    } else {
        FailureKind::Other
    }
}

type CompareAnswer = Result<(StalenessState, Option<u32>, bool), FailureKind>;

/// The CI answer for one branch: the filtered state plus whether that branch
/// has any run at all, which the identical-to-default rule reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CiAnswer {
    state: CiState,
    branch_has_runs: bool,
}

/// `rows < total_count` means the page cannot answer the question. `total_count`
/// is a completeness proof, not a heuristic, and it is checked against EVERY row
/// before the branch filter: a short page must read `Incomplete`, never a
/// confident answer built from the rows that happened to fit.
///
/// Only rows whose `head_branch` equals `branch` exactly (byte equality: `main`
/// matches neither `main-2` nor `origin/main`) count. A row with a missing or
/// null `head_branch` is dropped.
fn parse_ci_response(body: &str, branch: &str) -> Result<CiAnswer, FailureKind> {
    let value: serde_json::Value = serde_json::from_str(body).map_err(|_| FailureKind::Other)?;
    let total_count = value
        .get("total_count")
        .and_then(serde_json::Value::as_u64)
        .ok_or(FailureKind::Other)?;
    let rows = value
        .get("workflow_runs")
        .and_then(serde_json::Value::as_array)
        .ok_or(FailureKind::Other)?;
    if (rows.len() as u64) < total_count {
        return Err(FailureKind::Incomplete);
    }
    let kept: Vec<&serde_json::Value> = rows
        .iter()
        .filter(|row| row.get("head_branch").and_then(serde_json::Value::as_str) == Some(branch))
        .collect();
    // `status != "completed"` is a one-value blacklist on purpose: a future GitHub
    // status counts as running, which is the correct bias, because a whitelist
    // would go silently dark.
    let running = kept
        .iter()
        .any(|row| row.get("status").and_then(serde_json::Value::as_str) != Some("completed"));
    Ok(CiAnswer {
        state: if running {
            CiState::Running
        } else {
            CiState::Idle
        },
        branch_has_runs: !kept.is_empty(),
    })
}

/// The staleness mapping is unchanged; the extra flag reports whether GitHub
/// said `identical`, which is the same statement the CI suppression rule needs.
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
    let identical = status == "identical";
    match (status, behind_by) {
        ("identical" | "ahead", Some(0)) => Ok((StalenessState::Current, None, identical)),
        ("behind" | "diverged", Some(behind)) if behind > 0 => {
            Ok((StalenessState::Stale, Some(behind), identical))
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
        FailureKind::Timeout | FailureKind::Incomplete | FailureKind::Other => current
            .unwrap_or(base)
            .saturating_mul(2)
            .min(BACKOFF_CAP)
            .max(base),
    }
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
}

impl Default for CiAxis {
    fn default() -> Self {
        Self {
            chip: CiState::Unknown,
            confirmed: None,
            last_confirmed_at: None,
            next_due: None,
            failure_interval: None,
        }
    }
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

#[derive(Default)]
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
    /// Set when the CI answer was suppressed by the identical-to-default rule.
    /// Carried beside `ci` because `CiState` has no `Suppressed` value.
    ci_suppressed: bool,
    staleness: Option<CompareAnswer>,
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
    transitions: mpsc::Sender<RemoteTransition>,
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
        let (transitions, receiver) = mpsc::channel(capacity);
        let sweeper = Arc::new(Self {
            session_manager,
            settings,
            emit: Mutex::new(emit),
            probe: seams.probe,
            spawner: seams.spawner,
            local_git_runner: seams.local_git,
            transitions,
            state: Mutex::new(SweeperState::default()),
        });
        (sweeper, receiver)
    }

    /// Production seams: the real `which`, the real `gh` spawner and the real
    /// local `git` runner. Kept in one factory so `lib.rs` names no seam type.
    pub(crate) fn production_seams() -> RemoteSweeperSeams {
        RemoteSweeperSeams {
            probe: Arc::new(|| which::which("gh").ok()),
            spawner: Arc::new(|spec| Box::pin(spawn_gh(spec))),
            local_git: Arc::new(|path, args| Box::pin(run_local_git(path, args))),
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

        // The label chain runs on either axis, once per distinct nwo, and is
        // cached for the process lifetime: CI needs the name to decide the
        // identical-to-default suppression, staleness to render `%BASE%`.
        let mut pending_base: Vec<(String, String)> = Vec::new();
        if gh_path.is_some() {
            let state = self.lock_state();
            for (key, plan) in &plans {
                if !(plan.ci || plan.staleness)
                    || state.base_branches.contains_key(&key.nwo)
                    || pending_base.iter().any(|(nwo, _)| nwo == &key.nwo)
                {
                    continue;
                }
                if let Some(index) = groups.get(key).and_then(|group| group.first()) {
                    pending_base.push((key.nwo.clone(), facts[*index].path.clone()));
                }
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
            let query_keys: Vec<(QueryKey, KeyPlan)> = plans
                .iter()
                .filter(|(_, plan)| plan.ci || plan.staleness)
                .cloned()
                .collect();
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
            let spawner = Arc::clone(&self.spawner);
            let gh = gh.to_path_buf();
            outcomes = futures::stream::iter(query_keys.into_iter().map(|(key, plan)| {
                let spawner = Arc::clone(&spawner);
                let gh = gh.clone();
                let default_branch = defaults.get(&key.nwo).cloned().flatten();
                async move {
                    let outcome =
                        query_key(&spawner, &gh, &key, plan, default_branch.as_deref()).await;
                    (key, outcome)
                }
            }))
            .buffer_unordered(QUERY_CONCURRENCY)
            .collect()
            .await;
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
            self.apply_ci(&key, outcome.ci, outcome.ci_suppressed, indices, &ctx);
            self.apply_staleness(&key, outcome.staleness, indices, &ctx);
        }

        // Retire keys no path maps to any more: a local commit moves `head_sha`
        // (or the branch changes on the same commit), the new key starts at
        // `Unknown`, and the old key must not linger.
        self.lock_state()
            .keys
            .retain(|key, _| groups.contains_key(key));

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

        log::log!(
            ROUND_LOG_LEVEL,
            "[RemoteSweeper] round: {} path(s), {} key(s)",
            paths.len(),
            groups.len()
        );
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

    fn apply_ci(
        &self,
        key: &QueryKey,
        result: Option<Result<CiState, FailureKind>>,
        suppressed: bool,
        indices: &[usize],
        ctx: &RoundCtx<'_>,
    ) {
        let Some(result) = result else {
            return;
        };
        let mut state = self.lock_state();
        let entry = state.keys.entry(key.clone()).or_default();
        let base = ci_base_interval(entry.ci.confirmed, ctx.ci_dial);

        match result {
            Ok(_) if suppressed => {
                // A suppressed answer has no edge into or out of it: `confirmed =
                // None` breaks the transition chain both ways, like a fresh key.
                entry.ci.chip = CiState::Idle;
                entry.ci.confirmed = None;
                entry.ci.last_confirmed_at = Some(ctx.wall);
                entry.ci.failure_interval = None;
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
            Err(kind) => {
                let next = failure_interval(kind, base, entry.ci.failure_interval);
                entry.ci.chip = CiState::Unknown;
                entry.ci.failure_interval = Some(next);
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
            Ok((answer, behind_by, _identical)) => {
                let prior_confirmed = entry.staleness.confirmed;
                let prior_last = entry.staleness.last_confirmed_at;
                entry.staleness.chip = answer;
                entry.staleness.confirmed = Some(answer);
                entry.staleness.behind_by = behind_by;
                entry.staleness.last_confirmed_at = Some(ctx.wall);
                entry.staleness.failure_interval = None;
                entry.staleness.next_due = Some(ctx.now + ctx.staleness_dial);
                drop(state);

                let stale = matches!(
                    (prior_confirmed, answer),
                    (Some(StalenessState::Current), StalenessState::Stale)
                );
                if stale && !on_default_branch {
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
) -> KeyOutcome {
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
                run_query(spawner, spec, |body| parse_ci_response(body, &branch)).await
            }
            Err(_) => Err(FailureKind::Other),
        };
        outcome.ci = Some(match ci {
            // Only a non-default branch that HAS runs needs the identity
            // question; a branch with no runs answers `Idle` without it, and the
            // default branch is never suppressed.
            Ok(answer)
                if answer.branch_has_runs
                    && default_branch.is_some_and(|default| key.branch != default) =>
            {
                let compare = match &outcome.staleness {
                    Some(result) => *result,
                    None => match build_gh_command_spec(
                        gh,
                        GhQuery::Compare {
                            nwo: &key.nwo,
                            sha40: &key.sha40,
                        },
                    ) {
                        Ok(spec) => run_query(spawner, spec, parse_compare_response).await,
                        Err(_) => Err(FailureKind::Other),
                    },
                };
                match compare {
                    Ok((_, _, true)) => {
                        outcome.ci_suppressed = true;
                        Ok(CiState::Idle)
                    }
                    Ok((_, _, false)) => Ok(answer.state),
                    Err(kind) => Err(kind),
                }
            }
            Ok(answer) => Ok(answer.state),
            Err(kind) => Err(kind),
        });
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

    fn ci_body(statuses: &[&str]) -> String {
        ci_body_with_total(statuses, statuses.len() as u64)
    }

    fn ci_body_with_total(statuses: &[&str], total: u64) -> String {
        let rows: Vec<serde_json::Value> = statuses
            .iter()
            .map(|status| serde_json::json!({ "head_branch": "main", "status": status }))
            .collect();
        serde_json::json!({ "total_count": total, "workflow_runs": rows }).to_string()
    }

    /// Mixed-branch rows, so a test can replay one commit carrying runs for two
    /// branches. `total_count` matches the row count, as GitHub reports it.
    fn ci_rows(rows: &[(&str, &str)]) -> String {
        let values: Vec<serde_json::Value> = rows
            .iter()
            .map(|(branch, status)| serde_json::json!({ "head_branch": branch, "status": status }))
            .collect();
        serde_json::json!({ "total_count": values.len(), "workflow_runs": values }).to_string()
    }

    fn compare_body(status: &str, behind_by: u64) -> String {
        serde_json::json!({
            "status": status,
            "behind_by": behind_by,
            "ahead_by": 0,
        })
        .to_string()
    }

    #[derive(Default)]
    struct GhScripts {
        ci: VecDeque<Result<GhCallOutput, FailureKind>>,
        compare: VecDeque<Result<GhCallOutput, FailureKind>>,
        repo_info: VecDeque<Result<GhCallOutput, FailureKind>>,
        /// Per-endpoint overrides, keyed by a substring of the endpoint. Checked
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
            let routed = self
                .routes
                .iter_mut()
                .find(|(marker, queue)| target.contains(marker.as_str()) && !queue.is_empty())
                .and_then(|(_, queue)| queue.pop_front());
            if let Some(result) = routed {
                return result;
            }
            if target.contains("/actions/runs?") {
                self.ci
                    .pop_front()
                    .unwrap_or_else(|| Ok(ok_output(&ci_body(&[]))))
            } else if target.contains("/compare/") {
                self.compare
                    .pop_front()
                    .unwrap_or_else(|| Ok(ok_output(&compare_body("identical", 0))))
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
                        !target.contains("/actions/runs?")
                            && !target.contains("/compare/")
                            && !target.contains("symbolic-ref")
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
    }

    impl Harness {
        fn new(settings: AppSettings) -> Self {
            Self::with_capacity(settings, TRANSITION_QUEUE_CAPACITY)
        }

        fn with_capacity(settings: AppSettings, capacity: usize) -> Self {
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
            parse_ci_response(&ci_body(&["completed", "in_progress"]), "main"),
            Ok(CiAnswer {
                state: CiState::Running,
                branch_has_runs: true,
            })
        );
        assert_eq!(
            parse_ci_response(&ci_body(&["queued"]), "main"),
            Ok(CiAnswer {
                state: CiState::Running,
                branch_has_runs: true,
            }),
            "an unknown future status counts as running, never as idle"
        );
    }

    #[test]
    fn ci_idle_when_every_row_is_completed() {
        assert_eq!(
            parse_ci_response(&ci_body(&[]), "main"),
            Ok(CiAnswer {
                state: CiState::Idle,
                branch_has_runs: false,
            })
        );
        assert_eq!(
            parse_ci_response(&ci_body(&["completed", "completed"]), "main"),
            Ok(CiAnswer {
                state: CiState::Idle,
                branch_has_runs: true,
            })
        );
    }

    #[test]
    fn ci_incomplete_page_is_unknown_not_idle() {
        let body = ci_body_with_total(&["completed"; 99], 240);
        assert_eq!(
            parse_ci_response(&body, "main"),
            Err(FailureKind::Incomplete)
        );
    }

    #[test]
    fn rows_without_a_branch_are_dropped_and_the_page_stays_complete() {
        let body = serde_json::json!({
            "total_count": 2,
            "workflow_runs": [
                { "status": "in_progress" },
                { "head_branch": "main", "status": "completed" },
            ],
        })
        .to_string();
        assert_eq!(
            parse_ci_response(&body, "main"),
            Ok(CiAnswer {
                state: CiState::Idle,
                branch_has_runs: true,
            }),
            "a row with no head_branch cannot answer for any branch"
        );

        let short_page = serde_json::json!({
            "total_count": 5,
            "workflow_runs": [{ "head_branch": "main", "status": "completed" }],
        })
        .to_string();
        assert_eq!(
            parse_ci_response(&short_page, "main"),
            Err(FailureKind::Incomplete),
            "the missing rows might be other branches, so the page cannot answer"
        );
    }

    #[test]
    fn branch_filter_is_exact_match() {
        let body = ci_rows(&[
            ("main-2", "in_progress"),
            ("origin/main", "in_progress"),
            ("main", "completed"),
        ]);
        assert_eq!(
            parse_ci_response(&body, "main"),
            Ok(CiAnswer {
                state: CiState::Idle,
                branch_has_runs: true,
            })
        );
        assert_eq!(
            parse_ci_response(&body, "main-2"),
            Ok(CiAnswer {
                state: CiState::Running,
                branch_has_runs: true,
            })
        );
    }

    #[test]
    fn compare_status_table_maps_to_staleness() {
        assert_eq!(
            parse_compare_response(&compare_body("identical", 0)),
            Ok((StalenessState::Current, None, true))
        );
        assert_eq!(
            parse_compare_response(&compare_body("ahead", 0)),
            Ok((StalenessState::Current, None, false)),
            "ahead with behind_by 0 is not stale"
        );
        assert_eq!(
            parse_compare_response(&compare_body("behind", 3)),
            Ok((StalenessState::Stale, Some(3), false))
        );
        assert_eq!(
            parse_compare_response(&compare_body("diverged", 7)),
            Ok((StalenessState::Stale, Some(7), false))
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
            gh.ci.push_back(Ok(ok_output(&ci_body(&[]))));
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
            gh.ci.push_back(Ok(ok_output(&ci_body(&["in_progress"]))));
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
        for body in [ci_body(&["in_progress"]), ci_body(&[])] {
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
            gh.ci.push_back(Ok(ok_output(&ci_rows(&[
                ("fix/2124-ignore-short-idle-bursts", "in_progress"),
                ("fix/2124-ignore-short-idle-bursts", "in_progress"),
                ("main", "completed"),
            ]))));
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
            let round_one = ci_rows(&[
                ("fix/2124-ignore-short-idle-bursts", "in_progress"),
                ("fix/2124-ignore-short-idle-bursts", "in_progress"),
                ("main", "completed"),
            ]);
            let round_two = ci_rows(&[
                ("fix/2124-ignore-short-idle-bursts", "completed"),
                ("fix/2124-ignore-short-idle-bursts", "completed"),
                ("main", "completed"),
            ]);
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
            for _ in 0..3 {
                gh.compare
                    .push_back(Ok(ok_output(&compare_body("ahead", 0))));
            }
            let bodies = [
                ci_rows(&[("fix/2124-ignore-short-idle-bursts", "completed")]),
                ci_rows(&[("fix/2124-ignore-short-idle-bursts", "in_progress")]),
                ci_rows(&[("fix/2124-ignore-short-idle-bursts", "completed")]),
            ];
            for body in bodies {
                gh.ci.push_back(Ok(ok_output(&body)));
                gh.ci.push_back(Ok(ok_output(&body)));
            }
        }

        let mut now = Instant::now();
        let mut wall = Local::now();
        for _ in 0..3 {
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
            3,
            "one identity compare per round for the non-default branch, none for the default one"
        );
    }

    #[tokio::test]
    async fn default_branch_is_never_suppressed() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        let repo = harness.repo("repo-a");
        harness.set_work(std::slice::from_ref(&repo));
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.repo_info
                .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
            gh.ci.push_back(Ok(ok_output(&ci_body(&[]))));
            gh.ci.push_back(Ok(ok_output(&ci_body(&["in_progress"]))));
            gh.ci.push_back(Ok(ok_output(&ci_body(&[]))));
        }

        let mut now = Instant::now();
        let mut wall = Local::now();
        for _ in 0..3 {
            harness.round(now, wall).await;
            tick(&mut now, &mut wall, 60);
        }

        let transitions = harness.drain_transitions();
        assert_eq!(transitions.len(), 2);
        assert_eq!(transitions[0].kind, TransitionKind::CiStarted);
        assert_eq!(transitions[1].kind, TransitionKind::CiFinished);
        assert_eq!(
            harness
                .gh
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .count("/compare/"),
            1,
            "one staleness compare in round 1; the default branch adds none"
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
            gh.ci
                .push_back(Ok(ok_output(&ci_rows(&[("fix/x", "completed")]))));
            gh.ci
                .push_back(Ok(ok_output(&ci_rows(&[("fix/x", "in_progress")]))));
            gh.ci
                .push_back(Ok(ok_output(&ci_rows(&[("fix/x", "completed")]))));
        }

        let mut now = Instant::now();
        let mut wall = Local::now();
        for _ in 0..3 {
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
            gh.ci
                .push_back(Ok(ok_output(&ci_rows(&[("fix/x", "in_progress")]))));
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
            gh.ci
                .push_back(Ok(ok_output(&ci_rows(&[("main", "in_progress")]))));
            gh.ci
                .push_back(Ok(ok_output(&ci_rows(&[("fix/x", "completed")]))));
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
                gh.ci.push_back(Ok(ok_output(&ci_body(&["in_progress"]))));
                gh.ci.push_back(Err(FailureKind::Timeout));
                gh.ci.push_back(Err(FailureKind::Timeout));
                gh.ci.push_back(Err(FailureKind::Timeout));
                gh.ci.push_back(Ok(ok_output(&ci_body(&[]))));
            }

            let mut now = Instant::now();
            let mut wall = Local::now();
            let running_wall = wall;
            harness.round(now, wall).await;
            for _ in 0..3 {
                tick(&mut now, &mut wall, advance);
                harness.round(now, wall).await;
            }
            tick(&mut now, &mut wall, advance);
            let idle_wall = wall;
            harness.round(now, wall).await;

            let transitions = harness.drain_transitions();
            assert_eq!(transitions.len(), 1, "advance={advance}");
            let transition = &transitions[0];
            assert_eq!(transition.kind, TransitionKind::CiFinished);
            assert_eq!(transition.last_confirmed_at, Some(running_wall));
            assert_eq!(transition.observed_at, idle_wall);
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
                gh.ci.push_back(Ok(ok_output(&ci_body(&["in_progress"]))));
            }
            gh.ci.push_back(Ok(ok_output(&ci_body(&[]))));
        }

        let mut now = Instant::now();
        let mut wall = Local::now();
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

        let transitions = harness.drain_transitions();
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].last_confirmed_at, Some(twentieth_wall));
        assert_ne!(
            transitions[0].last_confirmed_at,
            Some(wall - chrono::Duration::seconds(20 * 60)),
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
            gh.ci.push_back(Ok(ok_output(&ci_body(&["in_progress"]))));
            gh.ci.push_back(Ok(ok_output(&ci_body(&[]))));
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
                .push_back(Ok(ok_output(&compare_body("behind", 2))));
            gh.compare
                .push_back(Ok(ok_output(&compare_body("identical", 0))));
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
                .push_back(Ok(ok_output(&compare_body("identical", 0))));
            for _ in 0..6 {
                gh.compare
                    .push_back(Ok(ok_output(&compare_body("behind", 4))));
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
                .push_back(Ok(ok_output(&compare_body("identical", 0))));
            gh.compare
                .push_back(Ok(ok_output(&compare_body("behind", 4))));
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
                .push_back(Ok(ok_output(&compare_body("identical", 0))));
            gh.compare
                .push_back(Ok(ok_output(&compare_body("behind", 4))));
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
                .push_back(Ok(ok_output(&compare_body("identical", 0))));
            gh.compare
                .push_back(Ok(ok_output(&compare_body("behind", 4))));
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

    #[tokio::test]
    async fn two_rooms_on_one_commit_are_one_call_and_two_transitions() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        let first = harness.repo("repo-a");
        let second = harness.repo("repo-b");
        harness.set_work(&[first.clone(), second.clone()]);
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.ci.push_back(Ok(ok_output(&ci_body(&["in_progress"]))));
            gh.ci.push_back(Ok(ok_output(&ci_body(&[]))));
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

        let transitions = harness.drain_transitions();
        assert_eq!(transitions.len(), 2, "one notice per room");
        assert_eq!(
            harness
                .gh
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .count("/actions/runs?"),
            2,
            "one call per round for the shared key"
        );
        let mut paths: Vec<String> = transitions
            .iter()
            .map(|transition| transition.repo_path.clone())
            .collect();
        paths.sort();
        assert_eq!(paths, vec![first, second]);
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
                gh.ci.push_back(Ok(ok_output(&ci_body(&[]))));
            }
            // The failing repository is scripted by endpoint, so the assertion
            // cannot depend on which of two concurrent queries is polled first.
            // Only the compare calls for the failing nwo fail, so a routed marker
            // cannot be consumed by the base-label or CI calls.
            gh.route("failing/compare", Err(FailureKind::Timeout));
            gh.route("failing/compare", Err(FailureKind::Timeout));
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
            gh.ci.push_back(Ok(ok_output(&ci_body(&[]))));
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
            gh.ci.push_back(Ok(ok_output(&ci_body(&[]))));
            gh.ci.push_back(Ok(ok_output(&ci_body(&["in_progress"]))));
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
            gh.ci.push_back(Ok(ok_output(&ci_body(&["in_progress"]))));
            gh.ci.push_back(Ok(ok_output(&ci_body(&[]))));
        }

        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;
        tick(&mut now, &mut wall, 60);
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
            gh.ci.push_back(Ok(ok_output(&ci_body(&[]))));
            gh.ci.push_back(Ok(ok_output(&ci_body(&["in_progress"]))));
            gh.ci.push_back(Ok(ok_output(&ci_body(&["in_progress"]))));
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
            gh.ci.push_back(Ok(ok_output(&ci_body(&[]))));
            gh.ci.push_back(Ok(ok_output(&ci_body(&["in_progress"]))));
            gh.ci.push_back(Ok(ok_output(&ci_body(&[]))));
        }

        let mut now = Instant::now();
        let mut wall = Local::now();
        for _ in 0..3 {
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
                    .push_back(Ok(ok_output(&compare_body("identical", 0))));
                gh.compare
                    .push_back(Ok(ok_output(&compare_body("behind", 2))));
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
                    .push_back(Ok(ok_output(&compare_body("identical", 0))));
                gh.compare
                    .push_back(Ok(ok_output(&compare_body("behind", 2))));
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
                    .push_back(Ok(ok_output(&compare_body("identical", 0))));
                gh.compare
                    .push_back(Ok(ok_output(&compare_body("behind", 2))));
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
                    .push_back(Ok(ok_output(&compare_body("behind", 2))));
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
            gh.ci.push_back(Ok(ok_output(&ci_body(&[]))));
            gh.ci.push_back(Ok(ok_output(&ci_body(&[]))));
            gh.compare
                .push_back(Ok(ok_output(&compare_body("identical", 0))));
            gh.compare
                .push_back(Ok(ok_output(&compare_body("identical", 0))));
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
}
