// Tests live in the child module so they can read private fields and pure
// helpers without widening the module's public surface.
use super::*;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::config::settings::AppSettings;

mod branch_filter_2126;
mod branch_filter_2126_rounds;
mod call_budget_2152;
mod call_budget_2152_rounds;
mod ci_jobs_2768;
mod ci_jobs_2768_rounds;
mod positive_control;
mod run_ids_2473;
mod snapshot_producer_2374;

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
                Box::pin(async move { gh.lock().unwrap_or_else(|e| e.into_inner()).next(&spec) })
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

        let settings: SettingsState = crate::config::settings::new_settings_state(settings);
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

// --- the #2126 incident: runs on a different branch must not count ---

fn drain(harness: &Harness) {
    let _ = harness.drain_transitions();
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

// --- #2152: the global call budget, the priority order and the gate ---

fn secondary_limit_output() -> GhCallOutput {
    GhCallOutput {
        stdout: String::new(),
        stderr: "You have exceeded a secondary rate limit. Please wait a few minutes before you \
            try again."
            .to_string(),
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

// --- #2374: the neutral snapshot producer. Tests are prefixed
// `remote_activity_cache_producer_` on purpose: the cache-unit guard runs
// `cargo test --lib config::remote_activity_cache::` and must be satisfied
// only by `config/remote_activity_cache.rs`'s own unit tests. ---

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

fn persisted_of(dir: &Path, repo: &str) -> crate::config::remote_activity_cache::PersistedRepoCi {
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

fn ci_jobs_body(run_id: u64, sha40: &str, branch: &str, attempt: u64, statuses: &[&str]) -> String {
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
        "edge_counts started={} finished={} expected_started={started} \
            expected_finished={finished}",
        edges
            .iter()
            .filter(|edge| edge.kind == TransitionKind::CiStarted)
            .count(),
        edges
            .iter()
            .filter(|edge| edge.kind == TransitionKind::CiFinished)
            .count(),
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
