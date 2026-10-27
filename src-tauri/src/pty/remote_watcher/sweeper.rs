use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Local, Utc};
use futures::stream::StreamExt;
use tokio::sync::mpsc;

use crate::config::remote_activity_cache::{PersistedRepoCi, REMOTE_ACTIVITY_SNAPSHOT_FILE_NAME};
use crate::config::settings::SettingsState;
use crate::session::manager::SessionManager;
use crate::shutdown::ShutdownSignal;

use super::{
    activity_for, build_gh_command_spec, clamp_interval_secs, due, is_sha40, jitter_offset,
    parse_github_nwo, persisted_repo_ci, query_key, remote_activity_snapshot, room_map,
    run_local_git, spawn_gh, CiState, Emitter, FailureKind, GhQuery, KeyOutcome, KeyPlan,
    PathFacts, QueryKey, RemoteActivity, RemoteActivityPayload, RemoteSweeper, RemoteSweeperSeams,
    RemoteTransition, RoundCtx, SnapshotPersistence, StalenessCarry, SweeperState, BACKOFF_CAP,
    DEFAULT_BRANCH_LABEL, GH_BUDGET_BURST, MAX_GH_CALLS_PER_MINUTE, QUERY_CONCURRENCY,
    ROUND_INTERVAL, ROUND_LOG_LEVEL, SECONDARY_BACKOFF_BASE, TRANSITION_QUEUE_CAPACITY,
};

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

    pub(crate) fn with_capacity(
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
        let branch =
            crate::pty::git_watcher::read_git_status(path).and_then(|status| status.branch);

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
