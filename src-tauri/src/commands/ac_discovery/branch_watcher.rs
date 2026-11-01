use super::model::AcWorkgroup;
use super::replica_read::{normalize_projected_path, projected_path_string};
use super::task_snapshot::{
    deliver_poll_task, poll_task_snapshot, DiscoveryBranchPayload, ReplicaBranchEntry,
    SessionGitReposPayload, TaskCacheEntry, TaskUpdatedPayload, BRANCH_POLL_INTERVAL,
};
use crate::config::ac_root::has_ac_root;
use crate::config::settings::SettingsState;
use crate::session::manager::SessionManager;
use crate::session::session::SessionRepo;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::AppHandle;
use tauri::{Emitter, Manager};
use uuid::Uuid;

pub struct DiscoveryBranchWatcher {
    git_state: Arc<crate::pty::git_watcher::GitState>,
    pub(crate) app_handle: AppHandle,
    pub(crate) session_manager: Arc<tokio::sync::RwLock<SessionManager>>,
    /// Keyed by the project directory that directly contains the Project AC Root, not by
    /// `settings.project_paths` entries (which may be parent dirs holding many projects).
    /// Keying by the direct parent prevents both (a) the original overwrite-across-projects
    /// bug (Grinch #1) and (b) the double-registration that occurs when `project_paths`
    /// contains both a parent and a child (Grinch #12).
    pub(crate) replicas: Mutex<HashMap<String, Vec<ReplicaBranchEntry>>>,
    /// Last-emitted `ac_discovery_branch_updated` payload per replica, gating that
    /// emission (panel UI + the #943 B2 per-repo branches). Holds the whole payload,
    /// not just the single-repo branch it held before B2, so the gate fires on
    /// per-repo drift and on a repo-set change: the same branch text under a
    /// different repo list is not the same event.
    pub(crate) discovery_cache: Mutex<HashMap<String, DiscoveryBranchPayload>>,
    /// Full per-repo state cache — gates `session_git_repos` emission. Independent from
    /// `discovery_cache` so multi-repo replicas re-emit on per-repo drift even when the
    /// single-branch view stays None.
    pub(crate) repos_cache: Mutex<HashMap<String, Vec<SessionRepo>>>,
    /// Per-workgroup-root cache for Gate C (task detection). Keyed by the
    /// stripped (no `\\?\`) absolute path of the wg-* directory. Bounded
    /// implicitly by the union of (loaded-project replicas, active sessions);
    /// no explicit prune since entries are ~200B each and the upper bound is
    /// the user's project layout. A stale entry for a wg that no longer has
    /// sessions or replicas is harmless: next tick will simply not visit it.
    pub(crate) task_cache: Mutex<HashMap<PathBuf, TaskCacheEntry>>,
}

impl DiscoveryBranchWatcher {
    #[cfg(test)]
    pub fn new(
        app_handle: AppHandle,
        session_manager: Arc<tokio::sync::RwLock<SessionManager>>,
    ) -> Arc<Self> {
        Self::with_state(
            app_handle,
            session_manager,
            Arc::new(crate::pty::git_watcher::GitState::new()),
        )
    }

    pub fn with_state(
        app_handle: AppHandle,
        session_manager: Arc<tokio::sync::RwLock<SessionManager>>,
        git_state: Arc<crate::pty::git_watcher::GitState>,
    ) -> Arc<Self> {
        Arc::new(Self {
            git_state,
            app_handle,
            session_manager,
            replicas: Mutex::new(HashMap::new()),
            discovery_cache: Mutex::new(HashMap::new()),
            repos_cache: Mutex::new(HashMap::new()),
            task_cache: Mutex::new(HashMap::new()),
        })
    }

    /// #1298 - the ONLY way to mutate `replicas`. Owns the lock scope and republishes
    /// the flattened watched repo-path list to the global git sweeper on the way out,
    /// so the map cannot be changed without the sweeper learning about it. The
    /// sweeper is push-fed rather than holding a handle to this watcher so that
    /// `pty::git_watcher` never imports a `commands::` type.
    ///
    /// Structural, not conventional, because the failure mode is silent: a future
    /// mutator that forgot to republish would leave the sweeper unaware of its paths,
    /// so `read_git_status` returns `None` for them forever and the surface is a
    /// permanently violet chip with no failing test and no log line. The guard is
    /// dropped before the push, inside this helper, where nobody has to remember it.
    pub(crate) fn with_replicas_mut<R>(
        &self,
        f: impl FnOnce(&mut HashMap<String, Vec<ReplicaBranchEntry>>) -> R,
    ) -> R {
        let result = {
            let mut map = self.replicas.lock().unwrap();
            f(&mut map)
        };
        let paths: Vec<String> = {
            let map = self.replicas.lock().unwrap();
            map.values()
                .flatten()
                .flat_map(|e| e.repos.iter().map(|(_, path)| path.clone()))
                .collect()
        };
        // #2064 - the room half of the same map, pushed in the same
        // guard-released window: `remote_watcher` needs (repo path, room dir) to
        // route a transition to the room that owns the repo. `room_dir` is the
        // parent of the replica path, so no new filesystem discovery happens.
        let rooms: Vec<(String, String)> = {
            let map = self.replicas.lock().unwrap();
            let mut rooms = Vec::new();
            for entry in map.values().flatten() {
                let Some(room_dir) = std::path::Path::new(&entry.replica_path)
                    .parent()
                    .map(|parent| parent.to_string_lossy().to_string())
                else {
                    continue;
                };
                for (_, path) in &entry.repos {
                    rooms.push((path.clone(), room_dir.clone()));
                }
            }
            rooms
        };
        crate::pty::git_watcher::set_discovery_repo_paths(paths, &self.git_state);
        crate::pty::git_watcher::set_discovery_repo_rooms(rooms, &self.git_state);
        result
    }

    /// Update this project's replicas in the watcher. `project_dir` is the directory
    /// that directly contains the Project AC Root, not a grand-parent from
    /// `settings.project_paths`.
    /// See the invariant comment on the `replicas` field.
    pub fn update_replicas_for_project(&self, project_dir: &str, workgroups: &[AcWorkgroup]) {
        // Invariant guard: catch mistaken call-site passes (e.g. a `base_path` parent)
        // in dev builds. Release builds log a warn and return to prevent silent corruption.
        let has_workspace = has_ac_root(Path::new(project_dir));
        debug_assert!(
            has_workspace,
            "update_replicas_for_project: {} does not contain a Project AC Root",
            project_dir
        );
        if !has_workspace {
            log::warn!(
                "[DiscoveryBranchWatcher] update_replicas_for_project called with {} which has no \
                    Project AC Root, ignoring",
                project_dir
            );
            return;
        }

        // Canonicalize the key so callers that pass slightly-different string shapes
        // (backslash vs forward slash, trailing separator, unresolved "..") still
        // converge to one map slot per project. Without this, the same project can
        // end up with two entries (e.g. from `discover_ac_agents` reading `read_dir`
        // output vs `discover_project` receiving a user-typed path) and emit doubled.
        let canonical_key =
            crate::config::projects::canonical_watch_project(Path::new(project_dir))
                .map(|p| projected_path_string(&p))
                .unwrap_or_else(|| projected_path_string(Path::new(project_dir)));

        // Invariant: git_repos order = replica.repo_paths order (which follows config.json
        // `repos`).
        // Never sort or dedupe here.
        let mut entries = Vec::new();
        for wg in workgroups {
            for agent in &wg.agents {
                if agent.repo_paths.is_empty() {
                    continue;
                }
                let repos: Vec<(String, String)> = agent
                    .repo_paths
                    .iter()
                    .map(|rp| {
                        let dir = rp
                            .replace('\\', "/")
                            .split('/')
                            .next_back()
                            .unwrap_or("")
                            .to_string();
                        let label = dir.strip_prefix("repo-").map(str::to_string).unwrap_or(dir);
                        (label, rp.clone())
                    })
                    .collect();
                entries.push(ReplicaBranchEntry {
                    replica_path: agent.path.clone(),
                    repos,
                    session_name: format!("{}/{}", wg.name, agent.name),
                });
            }
        }

        log::info!(
            "[DiscoveryBranchWatcher] update_replicas_for_project({}): {} replicas",
            canonical_key,
            entries.len()
        );

        // Swap in this project's entries; leave other projects alone.
        let valid: std::collections::HashSet<String> = self.with_replicas_mut(|map| {
            map.insert(canonical_key, entries);
            // Prune cache entries that no longer belong to ANY project.
            map.values()
                .flatten()
                .map(|e| e.replica_path.clone())
                .collect()
        });
        self.discovery_cache
            .lock()
            .unwrap()
            .retain(|k, _| valid.contains(k));
        self.repos_cache
            .lock()
            .unwrap()
            .retain(|k, _| valid.contains(k));
    }

    /// Remove the specified replicas from `replicas`, `discovery_cache`, and `repos_cache`.
    /// Called by `refresh_git_repos_for_sessions` callers (§2.1.e) so the next watcher
    /// tick does not iterate stale `source_path`s between a session-level refresh and the
    /// follow-up `discover_project` call that re-registers the replicas with NEW paths.
    pub fn invalidate_replicas(&self, replica_paths: &[String]) {
        self.with_replicas_mut(|map| {
            for entries in map.values_mut() {
                entries.retain(|e| !replica_paths.iter().any(|p| p == &e.replica_path));
            }
        });
        {
            let mut dc = self.discovery_cache.lock().unwrap();
            let mut rc = self.repos_cache.lock().unwrap();
            for p in replica_paths {
                dc.remove(p);
                rc.remove(p);
            }
        }
        log::debug!(
            "[DiscoveryBranchWatcher] invalidated {} replica(s); awaiting next discover_project \
                re-registration",
            replica_paths.len()
        );
    }

    /// Start the polling loop on a dedicated thread.
    pub fn start(self: &Arc<Self>, shutdown: crate::shutdown::ShutdownSignal) {
        let watcher = Arc::clone(self);
        std::thread::spawn(move || {
            log::info!(
                "[DiscoveryBranchWatcher] thread started, polling every {}s",
                BRANCH_POLL_INTERVAL.as_secs()
            );
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to create tokio runtime for DiscoveryBranchWatcher");
            rt.block_on(async move {
                loop {
                    tokio::select! {
                        biased;
                        _ = shutdown.token().cancelled() => {
                            log::info!("[DiscoveryBranchWatcher] Shutdown signal received, \
                                stopping");
                            break;
                        }
                        _ = tokio::time::sleep(BRANCH_POLL_INTERVAL) => {
                            if let Err(e) = watcher.poll().await {
                                log::warn!("[DiscoveryBranchWatcher] poll failed: {}", e);
                            }
                        }
                    }
                }
            });
        });
    }

    fn refreshed_repos(&self, entry: &ReplicaBranchEntry) -> Vec<SessionRepo> {
        // #1298 - pure map reads. This loop no longer spawns `git`: the global
        // `GitSweeper` is the only polling producer, and this poll consumes its
        // published snapshot. An unpublished path reads `None`, which is the
        // same "unknown" a failed detection produced before.
        let statuses: Vec<Option<crate::pty::git_watcher::GitStatus>> = entry
            .repos
            .iter()
            .map(|(_, path)| crate::pty::git_watcher::read_git_status(path, &self.git_state))
            .collect();

        entry
            .repos
            .iter()
            .zip(statuses)
            .map(|((label, path), status)| SessionRepo {
                label: label.clone(),
                source_path: path.clone(),
                // #1028 - `branch` keeps its old behaviour exactly: a failed
                // detection reports `None`, with no hold.
                branch: status.as_ref().and_then(|s| s.branch.clone()),
                // #1028 - identical shape to GitWatcher::poll, over the SAME
                // shared map. Both badge writers must compute `dirty` the same
                // way: they take turns writing this vec, and if only one of them
                // computed it the other would write `None` every 15s and flicker
                // the badge violet twice a cycle.
                dirty: crate::pty::git_watcher::remember_dirty(
                    path,
                    status.as_ref().map(|s| s.dirty),
                    &self.git_state,
                ),
            })
            .collect()
    }

    pub(crate) async fn poll(&self) -> Result<(), String> {
        // Flatten per-project entries.
        let entries: Vec<ReplicaBranchEntry> = {
            let map = self.replicas.lock().unwrap();
            map.values().flatten().cloned().collect()
        };

        // Gate A + Gate B (existing replica-driven git detection). Wrapped so
        // an empty `entries` (no project loaded) still falls through to Gate C,
        // which is also driven by active sessions — sessions can exist for
        // workgroups whose project is not in `settings.projectPaths`.
        if !entries.is_empty() {
            for entry in &entries {
                // Capture the session's git_repos_gen (if a session exists) BEFORE running
                // detections.
                // Used for CAS on set_git_repos_if_gen (Grinch #14).
                let (session_id_opt, gen_snapshot) = {
                    let mgr = self.session_manager.read().await;
                    match mgr.find_by_name(&entry.session_name).await {
                        Some(id) => {
                            let gen = mgr.get_git_repos_gen(id).await.unwrap_or(0);
                            (Some(id), gen)
                        }
                        None => (None, 0),
                    }
                };

                let refreshed = self.refreshed_repos(entry);

                // Gate A: emit ac_discovery_branch_updated (AcDiscoveryPanel's branch
                // chip, plus #943 B2's per-repo branches for cold replicas).
                //
                // `branch` keeps its exact old semantics (single-repo only, None for
                // multi-repo, so the panel chip still hides there). `repo_branches` is
                // the vec `refreshed` already holds and Gate A used to discard, which
                // is the whole of Module B2: a cold multi-repo coordinator learns each
                // repo's branch without a single new git spawn.
                let payload = DiscoveryBranchPayload {
                    replica_path: entry.replica_path.clone(),
                    branch: if entry.repos.len() == 1 {
                        refreshed[0].branch.clone()
                    } else {
                        None
                    },
                    repo_branches: refreshed.iter().map(|r| r.branch.clone()).collect(),
                    repo_paths: refreshed.iter().map(|r| r.source_path.clone()).collect(),
                    repo_dirty: refreshed.iter().map(|r| r.dirty).collect(),
                };
                // Gate on the whole payload. Comparing only the single-repo `branch`
                // (what this did before B2) means a multi-repo replica whose repo just
                // changed branch compares None against None and never re-emits.
                let discovery_changed = {
                    let mut cache = self.discovery_cache.lock().unwrap();
                    if cache.get(&entry.replica_path) != Some(&payload) {
                        cache.insert(entry.replica_path.clone(), payload.clone());
                        true
                    } else {
                        false
                    }
                };
                if discovery_changed {
                    let _ = self.app_handle.emit("ac_discovery_branch_updated", payload);
                }

                // Gate B: emit session_git_repos (full per-repo state for SessionItem).
                // Independent cache so multi-repo replicas re-emit on per-repo drift even when
                // Gate A stays None.
                let repos_changed = {
                    let mut cache = self.repos_cache.lock().unwrap();
                    let prev = cache.get(&entry.replica_path);
                    if prev != Some(&refreshed) {
                        cache.insert(entry.replica_path.clone(), refreshed.clone());
                        true
                    } else {
                        false
                    }
                };
                if repos_changed {
                    if let Some(session_id) = session_id_opt {
                        // CAS write: skip if a refresh bumped gen during our detection window.
                        let wrote = {
                            let mgr = self.session_manager.read().await;
                            mgr.set_git_repos_if_gen(session_id, refreshed.clone(), gen_snapshot)
                                .await
                        };
                        if wrote {
                            let _ = self.app_handle.emit(
                                "session_git_repos",
                                SessionGitReposPayload {
                                    session_id: session_id.to_string(),
                                    repos: refreshed.clone(),
                                },
                            );
                        } else {
                            log::debug!(
                            "[DiscoveryBranchWatcher] gen mismatch for {} — refresh landed during \
                                poll; skipping stale emit",
                            entry.replica_path
                        );
                            // Clear our own cache entry so next tick re-evaluates against the fresh
                            // list.
                            self.repos_cache.lock().unwrap().remove(&entry.replica_path);
                        }
                    }
                    // If no session exists yet (un-instantiated replica), Gate A has already
                    // covered
                    // the display surface — no session to push git_repos into.
                }
            }
        }

        // Gate C: TASK.md detection. Runs every tick whether or not Gate A/B
        // had work to do — sessions in workgroups whose project is not loaded
        // still need brief updates.
        self.poll_tasks(&entries).await?;
        Ok(())
    }

    /// Gate C: detect TASK.md changes per unique workgroup root and emit
    /// `workgroup_task_updated` on change. Runs on every existing 15s tick
    /// of `poll()`; no new thread, no new cadence.
    pub(crate) async fn poll_tasks(&self, entries: &[ReplicaBranchEntry]) -> Result<(), String> {
        // Build the union of workgroup roots to watch:
        //   1. Replicas in loaded projects (from `entries`) — covers the
        //      sidebar `ProjectPanel` surface.
        //   2. Active sessions (walked up from cwd via the existing helper)
        //      — covers the terminal `WorkgroupTask` surface for sessions
        //      whose project is NOT loaded.
        // The map's value is the list of session UUIDs that resolve to this
        // wg-root (used for the event payload's `sessionIds`).
        let mut wg_roots: HashMap<PathBuf, Vec<Uuid>> = HashMap::new();

        for entry in entries {
            // replica.path is `<wg-root>/__agent_<name>` — its parent IS the
            // wg-root. We do NOT call `find_workgroup_task_path_for_cwd`
            // here because the parent is already the answer; calling it
            // would re-walk and add no information.
            if let Some(parent) = Path::new(&entry.replica_path).parent() {
                wg_roots
                    .entry(normalize_projected_path(parent))
                    .or_default();
            }
        }

        let archived = {
            let settings = self.app_handle.state::<SettingsState>();
            let cfg = settings.read().await;
            cfg.archived_project_paths.clone()
        };
        let sessions: Vec<(Uuid, String)> = {
            let mgr = self.session_manager.read().await.clone();
            mgr.get_sessions_working_dirs().await
        };
        let sessions = if archived.is_empty() {
            sessions
        } else {
            tokio::task::spawn_blocking(move || {
                let roots = crate::config::sessions_persistence::normalize_project_roots(&archived);
                crate::phone::mailbox::retain_unarchived_session_dirs(sessions, &roots)
            })
            .await
            .map_err(|e| format!("archived task-session filter task failed: {}", e))?
        };
        for (id, cwd) in sessions {
            if let Some(task_path) = crate::session::session::find_workgroup_task_path_for_cwd(&cwd)
            {
                if let Some(parent) = task_path.parent() {
                    wg_roots
                        .entry(normalize_projected_path(parent))
                        .or_default()
                        .push(id);
                }
            }
        }

        if wg_roots.is_empty() {
            return Ok(());
        }

        for (wg_root, session_ids) in wg_roots {
            self.check_workgroup_task(wg_root, session_ids).await;
        }
        Ok(())
    }

    /// Per-workgroup brief check. Stat short-circuits unchanged files; on
    /// stat-change, reads (with size cap), re-stats (defends against torn
    /// in-place editor saves), and emits if content actually changed.
    pub(crate) async fn check_workgroup_task(&self, wg_root: PathBuf, session_ids: Vec<Uuid>) {
        let previous = self.task_cache.lock().unwrap().get(&wg_root).cloned();
        let sentinel = previous.as_ref().map(|entry| entry.sentinel.clone());
        let root = wg_root.clone();
        let next =
            match tokio::task::spawn_blocking(move || poll_task_snapshot(&root, sentinel)).await {
                Ok(Ok(Some(next))) => next,
                Ok(Ok(None)) => return,
                Ok(Err(error)) => {
                    log::warn!(
                    "[DiscoveryBranchWatcher] task snapshot failed for {}: {}; retrying next pass",
                    wg_root.display(),
                    error
                );
                    return;
                }
                Err(error) => {
                    log::warn!(
                        "[DiscoveryBranchWatcher] task snapshot worker failed for {}: {}; \
                    retrying next pass",
                        wg_root.display(),
                        error
                    );
                    return;
                }
            };
        let shipped = deliver_poll_task(previous.as_ref(), next, |next| {
            self.app_handle
                .emit(
                    "workgroup_task_updated",
                    TaskUpdatedPayload {
                        workgroup_root: projected_path_string(&wg_root),
                        source: "poll".into(),
                        task: next.task.clone(),
                        task_title: next.task_title.clone(),
                        session_ids: session_ids.iter().map(Uuid::to_string).collect(),
                        status: next.status.clone(),
                        revision: next.revision.clone(),
                        tail_incomplete: next.tail_incomplete,
                    },
                )
                .map_err(|error| error.to_string())
        });
        match shipped {
            Ok(next) => {
                self.task_cache.lock().unwrap().insert(wg_root, next);
            }
            Err(error) => log::warn!(
                "[DiscoveryBranchWatcher] task emit failed for {}: {}; retrying next pass",
                wg_root.display(),
                error
            ),
        }
    }
}
