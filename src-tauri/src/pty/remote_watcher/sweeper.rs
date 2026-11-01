mod round;

use round::RoundReservations;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Local, Utc};
use tokio::sync::mpsc;

use crate::config::settings::SettingsState;
use crate::session::manager::SessionManager;
use crate::shutdown::ShutdownSignal;

use super::{
    build_gh_command_spec, clamp_interval_secs, is_sha40, jitter_offset, parse_github_nwo,
    room_map, run_local_git, spawn_gh, Emitter, FailureKind, GhQuery, PathFacts, QueryKey,
    RemoteSweeper, RemoteSweeperSeams, RemoteTransition, RoundCtx, SnapshotPersistence,
    SweeperState, BACKOFF_CAP, DEFAULT_BRANCH_LABEL, ROUND_INTERVAL, ROUND_LOG_LEVEL,
    SECONDARY_BACKOFF_BASE, TRANSITION_QUEUE_CAPACITY,
};

impl RemoteSweeper {
    #[cfg(test)]
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

    pub(crate) fn with_state(
        session_manager: Arc<tokio::sync::RwLock<SessionManager>>,
        settings: SettingsState,
        emit: Emitter,
        seams: RemoteSweeperSeams,
        git_state: Arc<crate::pty::git_watcher::GitState>,
        remote_state: Arc<super::RemoteActivityState>,
    ) -> (Arc<Self>, mpsc::Receiver<RemoteTransition>) {
        Self::with_capacity_and_state(
            session_manager,
            settings,
            emit,
            seams,
            TRANSITION_QUEUE_CAPACITY,
            git_state,
            remote_state,
        )
    }

    #[cfg(test)]
    pub(crate) fn with_capacity(
        session_manager: Arc<tokio::sync::RwLock<SessionManager>>,
        settings: SettingsState,
        emit: Emitter,
        seams: RemoteSweeperSeams,
        capacity: usize,
    ) -> (Arc<Self>, mpsc::Receiver<RemoteTransition>) {
        Self::with_capacity_and_state(
            session_manager,
            settings,
            emit,
            seams,
            capacity,
            Arc::new(crate::pty::git_watcher::GitState::new()),
            Arc::new(super::RemoteActivityState::new()),
        )
    }

    pub(crate) fn with_capacity_and_state(
        session_manager: Arc<tokio::sync::RwLock<SessionManager>>,
        settings: SettingsState,
        emit: Emitter,
        seams: RemoteSweeperSeams,
        capacity: usize,
        git_state: Arc<crate::pty::git_watcher::GitState>,
        remote_state: Arc<super::RemoteActivityState>,
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
            remote_state,
            git_state,
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

    pub(crate) fn lock_state(&self) -> std::sync::MutexGuard<'_, SweeperState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The work list, built exactly as `GitSweeper` builds it: union of discovery
    /// and live sessions, deduped by exact path string, archived projects dropped,
    /// sorted. `normalize_project_roots` and `build_work_list` canonicalize, so
    /// when `archived_roots` is non-empty both run together inside one blocking
    /// task (INV-1); the empty case short-circuits with zero syscalls.
    pub(crate) async fn work_list(&self, archived: Vec<String>) -> Vec<String> {
        let sessions: Vec<String> = {
            let manager = self.session_manager.read().await;
            manager.get_sessions_repos().await
        }
        .into_iter()
        .flat_map(|(_, repos, _)| repos.into_iter().map(|repo| repo.source_path))
        .collect();

        let discovery = crate::pty::git_watcher::discovery_repo_paths(&self.git_state);

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
    pub(crate) async fn run_round(&self, now: Instant, wall: DateTime<Local>) {
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
        let rooms = room_map(&self.git_state);

        let facts = self.read_round_facts(&paths, &rooms).await;

        let groups = query_groups(&facts);

        self.carry_staleness(&groups);

        let gh_path = self.lock_state().gh_path.clone();

        let (due_keys, throttled) = self.plan_round_queries(
            &groups,
            now,
            ci_enabled,
            staleness_enabled,
            gh_path.as_deref(),
        );

        let RoundReservations {
            pending_base,
            admitted,
            completion_reserved,
            identity_reserved,
        } = self.reserve_queries(&due_keys, &groups, &facts, gh_path.as_deref(), throttled);

        if let Some(gh) = gh_path.as_deref() {
            for (nwo, path) in pending_base {
                let label = self.resolve_base_branch(gh, &nwo, &path).await;
                self.lock_state().base_branches.insert(nwo, label);
            }
        }

        let outcomes = self
            .query_round(gh_path.as_deref(), admitted, &completion_reserved)
            .await;

        self.refund_round_reservations(&outcomes, &identity_reserved, &completion_reserved);

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

        self.publish_round(&paths, &facts, now).await;

        log::log!(
            ROUND_LOG_LEVEL,
            "[RemoteSweeper] round: {} path(s), {} key(s){}",
            paths.len(),
            groups.len(),
            if throttled { ", throttled" } else { "" }
        );
    }

    async fn read_round_facts(
        &self,
        paths: &[String],
        rooms: &HashMap<String, Vec<String>>,
    ) -> Vec<PathFacts> {
        let mut facts: Vec<PathFacts> = Vec::with_capacity(paths.len());
        for path in paths {
            facts.push(self.read_local_facts(path, rooms).await);
        }
        facts
    }

    /// One warning per freshness window while the destination stays broken; a
    /// successful write resets the limiter, so a later failure warns at once.
    pub(crate) fn warn_snapshot_failure(&self, now: Instant, error: String) {
        if self.lock_state().snapshot_warnings.admit(now) {
            log::warn!("[RemoteSweeper] remote activity snapshot write failed: {error}");
        }
    }

    /// Gate 1 (`.git` metadata), then at most two local git reads for the SHA and
    /// the origin URL. The branch comes from the published git snapshot, so the
    /// branch costs zero new git.
    pub(crate) async fn read_local_facts(
        &self,
        path: &str,
        rooms: &HashMap<String, Vec<String>>,
    ) -> PathFacts {
        let room_dirs = rooms.get(path).cloned().unwrap_or_default();
        let branch = crate::pty::git_watcher::read_git_status(path, &self.git_state)
            .and_then(|status| status.branch);

        if crate::filesystem::repositories::git_root_metadata(&Path::new(path).join(".git"))
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

    pub(crate) async fn local_git(&self, path: &str, args: &[&str]) -> Result<String, String> {
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
    pub(crate) async fn resolve_base_branch(&self, gh: &Path, nwo: &str, path: &str) -> String {
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

    pub(crate) async fn fetch_default_branch(&self, gh: &Path, nwo: &str) -> Option<String> {
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

    pub(crate) async fn local_default_branch(&self, path: &str) -> Option<String> {
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
}

fn query_groups(facts: &[PathFacts]) -> BTreeMap<QueryKey, Vec<usize>> {
    let mut groups: BTreeMap<QueryKey, Vec<usize>> = BTreeMap::new();
    for (index, fact) in facts.iter().enumerate() {
        let (Some(nwo), Some(head_sha), Some(branch)) = (&fact.nwo, &fact.head_sha, &fact.branch)
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
    groups
}
