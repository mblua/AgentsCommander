use futures::future::join_all;
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};

use crate::config::settings::SettingsState;
use crate::session::manager::SessionManager;
use crate::session::session::SessionRepo;

/// Resolve the preferred coding agent for a directory by matching the app
/// label from the agent's config.json against THIS instance's settings.
///
/// Flow: read config.json → get lastCodingAgent ID → get its `app` label
/// (e.g. "Claude Code") → find the agent in our settings with that label
/// → return OUR agent's ID. This decouples discovery from foreign agent IDs.
fn read_preferred_agent_id(dir: &Path, instance_agents: &[crate::config::settings::AgentConfig]) -> Option<String> {
    let config_path = dir.join("config.json");
    let content = std::fs::read_to_string(&config_path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&content).ok()?;
    let tooling = v.get("tooling")?;

    // Get the foreign agent ID and its app label
    let foreign_id = tooling.get("lastCodingAgent")?.as_str()?;
    let app_label = tooling.get("codingAgents")?
        .get(foreign_id)?
        .get("app")?
        .as_str()?;

    // Match by label against this instance's configured agents
    let matches: Vec<_> = instance_agents.iter().filter(|a| a.label == app_label).collect();
    if matches.len() > 1 {
        log::warn!(
            "[discovery] Multiple agents with label '{}' — using first match (id={})",
            app_label, matches[0].id
        );
    }
    let local_agent = matches.into_iter().next()?;
    Some(local_agent.id.clone())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcAgentMatrix {
    /// Display name: "{project_folder}/{agent_name}" with _agent_ prefix stripped
    pub name: String,
    /// Absolute path to the agent matrix directory
    pub path: String,
    /// Whether Role.md exists in the agent directory
    pub role_exists: bool,
    /// Preferred coding agent ID from config.json tooling.lastCodingAgent
    pub preferred_agent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcTeam {
    /// Team directory name with _team_ prefix stripped
    pub name: String,
    /// Agent display names belonging to this team
    pub agents: Vec<String>,
    /// Coordinator agent display name, if any
    pub coordinator: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcAgentReplica {
    /// Display name: agent dir name with __agent_ prefix stripped
    pub name: String,
    /// Absolute path to the replica agent directory
    pub path: String,
    /// Resolved identity path from config.json "identity" field
    pub identity_path: Option<String>,
    /// Project folder where the identity (matrix agent) lives
    pub origin_project: Option<String>,
    /// Preferred coding agent ID inherited from the identity matrix
    pub preferred_agent_id: Option<String>,
    /// Absolute paths to repos this replica works on (resolved from config.json "repos")
    pub repo_paths: Vec<String>,
    /// Git branch of the first repo (if exactly one repo), for sidebar display
    pub repo_branch: Option<String>,
    /// True if this replica is a coordinator of any discovered team.
    /// Computed at construction against a fresh `config::teams` snapshot;
    /// covers WG-aware suffix matching that simple `originProject/name`
    /// comparison on the frontend misses. See issue #69.
    pub is_coordinator: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcWorkgroup {
    /// Workgroup name (wg-* dir name)
    pub name: String,
    /// Absolute path to the workgroup directory
    pub path: String,
    /// First line of BRIEF.md (if exists)
    pub brief: Option<String>,
    /// Replica agents inside this workgroup
    pub agents: Vec<AcAgentReplica>,
    /// Absolute path to the first repo-* directory found (for CWD)
    pub repo_path: Option<String>,
    /// Team this workgroup belongs to (matched by replica membership)
    pub team_name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcDiscoveryResult {
    pub agents: Vec<AcAgentMatrix>,
    pub teams: Vec<AcTeam>,
    pub workgroups: Vec<AcWorkgroup>,
}

/// Extract the origin project name from a resolved identity path.
/// Looks for the folder immediately before ".ac-new" in the path.
fn extract_origin_project(identity_abs_path: &std::path::Path) -> Option<String> {
    let s = identity_abs_path.to_string_lossy().replace('\\', "/");
    let parts: Vec<&str> = s.split('/').collect();
    for (i, part) in parts.iter().enumerate() {
        if *part == ".ac-new" && i > 0 {
            return Some(parts[i - 1].to_string());
        }
    }
    None
}

/// Derive agent display name from its path.
/// Format: "{project_folder}/{agent_name}" where:
/// - project_folder = directory containing .ac-new/
/// - agent_name = folder name with "_agent_" prefix stripped
fn agent_display_name(project_folder: &str, dir_name: &str) -> String {
    let agent_name = dir_name
        .strip_prefix("_agent_")
        .unwrap_or(dir_name);
    format!("{}/{}", project_folder, agent_name)
}

/// Resolve an agent ref to a display name. Handles both relative refs
/// (e.g. "../_agent_tech-lead") and absolute paths.
/// For relative refs, uses project_folder as origin. For absolute paths,
/// extracts the origin project from the folder before ".ac-new".
fn resolve_agent_ref(project_folder: &str, agent_ref: &str) -> String {
    let normalized = agent_ref.replace('\\', "/");
    let trimmed = normalized
        .trim_start_matches("../")
        .trim_start_matches("./");
    if trimmed.contains(':') || trimmed.starts_with('/') {
        // Absolute path: extract origin project from folder before .ac-new
        let parts: Vec<&str> = trimmed.split('/').collect();
        let origin = parts.iter()
            .position(|p| *p == ".ac-new")
            .and_then(|i| if i > 0 { Some(parts[i - 1]) } else { None })
            .unwrap_or(project_folder);
        let dir_name = parts.last().unwrap_or(&trimmed);
        agent_display_name(origin, dir_name)
    } else {
        agent_display_name(project_folder, trimmed)
    }
}

/// Detect git branch synchronously for a given directory path.
fn detect_git_branch_sync(dir: &str) -> Option<String> {
    #[cfg(windows)]
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let mut cmd = std::process::Command::new("git");
    cmd.args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(dir);

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    match cmd.output() {
        Ok(out) if out.status.success() => {
            let branch = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if branch.is_empty() || branch == "HEAD" {
                None
            } else {
                Some(branch)
            }
        }
        _ => None,
    }
}

// --- Discovery Branch Watcher ---

const BRANCH_POLL_INTERVAL: Duration = Duration::from_secs(15);
const DETECT_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone)]
struct ReplicaBranchEntry {
    replica_path: String,
    /// (label, absolute repo path) pairs. Order = replica config.json `repos` array order.
    /// Never sort or dedupe — `Vec<SessionRepo>` equality in poll() depends on order.
    repos: Vec<(String, String)>,
    /// Session name format: "wg_name/replica_name"
    session_name: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiscoveryBranchPayload {
    replica_path: String,
    branch: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionGitReposPayload {
    session_id: String,
    repos: Vec<SessionRepo>,
}

pub struct DiscoveryBranchWatcher {
    app_handle: AppHandle,
    session_manager: Arc<tokio::sync::RwLock<SessionManager>>,
    /// Keyed by the project directory that DIRECTLY CONTAINS `.ac-new/` — NOT by
    /// `settings.project_paths` entries (which may be parent dirs holding many projects).
    /// Keying by the direct parent prevents both (a) the original overwrite-across-projects
    /// bug (Grinch #1) and (b) the double-registration that occurs when `project_paths`
    /// contains both a parent and a child (Grinch #12).
    replicas: Mutex<HashMap<String, Vec<ReplicaBranchEntry>>>,
    /// Single-repo branch cache — gates `ac_discovery_branch_updated` emission (panel UI).
    discovery_cache: Mutex<HashMap<String, Option<String>>>,
    /// Full per-repo state cache — gates `session_git_repos` emission. Independent from
    /// `discovery_cache` so multi-repo replicas re-emit on per-repo drift even when the
    /// single-branch view stays None.
    repos_cache: Mutex<HashMap<String, Vec<SessionRepo>>>,
}

impl DiscoveryBranchWatcher {
    pub fn new(
        app_handle: AppHandle,
        session_manager: Arc<tokio::sync::RwLock<SessionManager>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            app_handle,
            session_manager,
            replicas: Mutex::new(HashMap::new()),
            discovery_cache: Mutex::new(HashMap::new()),
            repos_cache: Mutex::new(HashMap::new()),
        })
    }

    /// Update this project's replicas in the watcher. `ac_new_parent_dir` is the directory
    /// that directly contains `.ac-new/` — NOT a grand-parent from `settings.project_paths`.
    /// See the invariant comment on the `replicas` field.
    pub fn update_replicas_for_project(
        &self,
        ac_new_parent_dir: &str,
        workgroups: &[AcWorkgroup],
    ) {
        // Invariant guard: catch mistaken call-site passes (e.g. a `base_path` parent)
        // in dev builds. Release builds log a warn and return to prevent silent corruption.
        let has_ac_new = Path::new(ac_new_parent_dir).join(".ac-new").is_dir();
        debug_assert!(
            has_ac_new,
            "update_replicas_for_project: {} does not contain .ac-new/",
            ac_new_parent_dir
        );
        if !has_ac_new {
            log::warn!(
                "[DiscoveryBranchWatcher] update_replicas_for_project called with {} which has no .ac-new/ — ignoring",
                ac_new_parent_dir
            );
            return;
        }

        // Canonicalize the key so callers that pass slightly-different string shapes
        // (backslash vs forward slash, trailing separator, unresolved "..") still
        // converge to one map slot per project. Without this, the same project can
        // end up with two entries (e.g. from `discover_ac_agents` reading `read_dir`
        // output vs `discover_project` receiving a user-typed path) and emit doubled.
        let canonical_key = std::fs::canonicalize(ac_new_parent_dir)
            .ok()
            .map(|p| {
                let s = p.to_string_lossy();
                s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
            })
            .unwrap_or_else(|| ac_new_parent_dir.to_string());

        // Invariant: git_repos order = replica.repo_paths order (which follows config.json `repos`).
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
                        let label = dir
                            .strip_prefix("repo-")
                            .map(str::to_string)
                            .unwrap_or(dir);
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
        let mut map = self.replicas.lock().unwrap();
        map.insert(canonical_key, entries);

        // Prune cache entries that no longer belong to ANY project.
        let valid: std::collections::HashSet<String> = map
            .values()
            .flatten()
            .map(|e| e.replica_path.clone())
            .collect();
        drop(map);
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
        {
            let mut map = self.replicas.lock().unwrap();
            for entries in map.values_mut() {
                entries.retain(|e| !replica_paths.iter().any(|p| p == &e.replica_path));
            }
        }
        {
            let mut dc = self.discovery_cache.lock().unwrap();
            let mut rc = self.repos_cache.lock().unwrap();
            for p in replica_paths {
                dc.remove(p);
                rc.remove(p);
            }
        }
        log::debug!(
            "[DiscoveryBranchWatcher] invalidated {} replica(s); awaiting next discover_project re-registration",
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
            let rt = tokio::runtime::Runtime::new()
                .expect("Failed to create tokio runtime for DiscoveryBranchWatcher");
            rt.block_on(async move {
                loop {
                    tokio::select! {
                        biased;
                        _ = shutdown.token().cancelled() => {
                            log::info!("[DiscoveryBranchWatcher] Shutdown signal received, stopping");
                            break;
                        }
                        _ = tokio::time::sleep(BRANCH_POLL_INTERVAL) => {
                            watcher.poll().await;
                        }
                    }
                }
            });
        });
    }

    async fn poll(&self) {
        // Flatten per-project entries.
        let entries: Vec<ReplicaBranchEntry> = {
            let map = self.replicas.lock().unwrap();
            map.values().flatten().cloned().collect()
        };
        if entries.is_empty() {
            return;
        }

        for entry in &entries {
            // Capture the session's git_repos_gen (if a session exists) BEFORE running detections.
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

            // Parallelize per-repo detection (Grinch #16). Each call individually bounded by
            // detect_branch_with_timeout (2s). Without join_all this was M*N*2s worst case.
            let branches: Vec<Option<String>> = join_all(
                entry
                    .repos
                    .iter()
                    .map(|(_, path)| Self::detect_branch_with_timeout(path)),
            )
            .await;

            let refreshed: Vec<SessionRepo> = entry
                .repos
                .iter()
                .zip(branches.into_iter())
                .map(|((label, path), branch)| SessionRepo {
                    label: label.clone(),
                    source_path: path.clone(),
                    branch,
                })
                .collect();

            // Gate A: emit ac_discovery_branch_updated (single-branch UI for AcDiscoveryPanel).
            // Only single-repo replicas surface a branch here; multi-repo = None so the panel hides it.
            let discovery_branch: Option<String> = if entry.repos.len() == 1 {
                refreshed[0].branch.clone()
            } else {
                None
            };
            let discovery_changed = {
                let mut cache = self.discovery_cache.lock().unwrap();
                let prev = cache.get(&entry.replica_path).cloned();
                if prev.as_ref() != Some(&discovery_branch) {
                    cache.insert(entry.replica_path.clone(), discovery_branch.clone());
                    true
                } else {
                    false
                }
            };
            if discovery_changed {
                let _ = self.app_handle.emit(
                    "ac_discovery_branch_updated",
                    DiscoveryBranchPayload {
                        replica_path: entry.replica_path.clone(),
                        branch: discovery_branch,
                    },
                );
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
                            "[DiscoveryBranchWatcher] gen mismatch for {} — refresh landed during poll; skipping stale emit",
                            entry.replica_path
                        );
                        // Clear our own cache entry so next tick re-evaluates against the fresh list.
                        self.repos_cache.lock().unwrap().remove(&entry.replica_path);
                    }
                }
                // If no session exists yet (un-instantiated replica), Gate A has already covered
                // the display surface — no session to push git_repos into.
            }
        }
    }

    async fn detect_branch_with_timeout(working_dir: &str) -> Option<String> {
        match tokio::time::timeout(DETECT_TIMEOUT, Self::detect_branch(working_dir)).await {
            Ok(result) => result,
            Err(_) => {
                log::warn!(
                    "[DiscoveryBranchWatcher] detect_branch timed out for {} (>{}s); treating as no-branch",
                    working_dir,
                    DETECT_TIMEOUT.as_secs()
                );
                None
            }
        }
    }

    async fn detect_branch(dir: &str) -> Option<String> {
        #[cfg(windows)]
        const CREATE_NO_WINDOW: u32 = 0x08000000;

        let mut cmd = tokio::process::Command::new("git");
        cmd.args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(dir)
            .kill_on_drop(true);

        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);

        match cmd.output().await {
            Ok(out) if out.status.success() => {
                let branch = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if branch.is_empty() || branch == "HEAD" {
                    None
                } else {
                    Some(branch)
                }
            }
            _ => None,
        }
    }
}

/// Discover AC-new agent matrices from .ac-new/ directories within configured repo paths.
#[tauri::command]
pub async fn discover_ac_agents(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    settings: State<'_, SettingsState>,
    branch_watcher: State<'_, Arc<DiscoveryBranchWatcher>>,
) -> Result<AcDiscoveryResult, String> {
    let cfg = settings.read().await;
    // Discovery-wide team snapshot — used per-replica for is_coordinator
    // and at the end for refresh_coordinator_flags. Computed once so a
    // single discovery pass presents a coherent coordinator view.
    // Lock-safe: discover_teams() reads settings from disk via load_settings()
    // and does NOT acquire SettingsState; the read guard above stays valid.
    let teams_snapshot = crate::config::teams::discover_teams();
    let mut agents: Vec<AcAgentMatrix> = Vec::new();
    let mut teams: Vec<AcTeam> = Vec::new();
    let mut workgroups: Vec<AcWorkgroup> = Vec::new();
    // Track the `.ac-new/`-containing dir each workgroup originated from. Keys are
    // `wg.name` values (unique within a discovery run; workgroup dir names include
    // the team name which collides only intentionally across projects). Populated as
    // we push to `workgroups` so we can later call `update_replicas_for_project` once
    // per project rather than once globally (Grinch #1 + #12).
    let mut wg_project_map: HashMap<String, String> = HashMap::new();

    for base_path in &cfg.project_paths {
        let base = Path::new(base_path);
        if !base.is_dir() {
            continue;
        }

        // Also check children of the base path (same pattern as search_repos)
        let dirs_to_check: Vec<std::path::PathBuf> = {
            let mut dirs = vec![base.to_path_buf()];
            if let Ok(entries) = std::fs::read_dir(base) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_dir() {
                        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                        if !name.starts_with('.') {
                            dirs.push(p);
                        }
                    }
                }
            }
            dirs
        };

        for repo_dir in dirs_to_check {
            let ac_new_dir = repo_dir.join(".ac-new");
            if !ac_new_dir.is_dir() {
                continue;
            }
            let repo_dir_str = repo_dir.to_string_lossy().to_string();

            // Opportunistic: ensure gitignore exists for existing projects
            let _ = ensure_ac_new_gitignore(&ac_new_dir);

            let project_folder = repo_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();

            let entries = match std::fs::read_dir(&ac_new_dir) {
                Ok(e) => e,
                Err(_) => continue,
            };

            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }

                let dir_name = match path.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n.to_string(),
                    None => continue,
                };

                // Agent matrices: _agent_* (single underscore prefix)
                if dir_name.starts_with("_agent_") {
                    let display_name = agent_display_name(&project_folder, &dir_name);
                    let role_exists = path.join("Role.md").exists();

                    let preferred_agent_id = read_preferred_agent_id(&path, &cfg.agents);

                    log::info!("[ac-discovery] agent: dir={:?}, preferred_agent_id={:?}", dir_name, preferred_agent_id);

                    agents.push(AcAgentMatrix {
                        name: display_name,
                        path: path.to_string_lossy().to_string(),
                        role_exists,
                        preferred_agent_id,
                    });
                }

                // Workgroups: wg-*
                if dir_name.starts_with("wg-") {
                    let brief = path.join("BRIEF.md")
                        .exists()
                        .then(|| std::fs::read_to_string(path.join("BRIEF.md")).ok())
                        .flatten()
                        .and_then(|content| content.lines().next().map(|l| l.trim_start_matches("# ").to_string()));

                    // Find first repo-* directory for CWD
                    let repo_path = std::fs::read_dir(&path)
                        .ok()
                        .and_then(|entries| {
                            entries.flatten().find(|e| {
                                let n = e.file_name();
                                let name = n.to_string_lossy();
                                name.starts_with("repo-") && e.path().is_dir()
                            })
                        })
                        .map(|e| e.path().to_string_lossy().to_string());

                    // Scan __agent_* replicas inside the WG
                    let mut wg_agents: Vec<AcAgentReplica> = Vec::new();
                    if let Ok(wg_entries) = std::fs::read_dir(&path) {
                        for wg_entry in wg_entries.flatten() {
                            let wg_path = wg_entry.path();
                            if !wg_path.is_dir() {
                                continue;
                            }
                            let wg_dir_name = match wg_path.file_name().and_then(|n| n.to_str()) {
                                Some(n) => n.to_string(),
                                None => continue,
                            };
                            if wg_dir_name.starts_with("__agent_") {
                                let replica_name = wg_dir_name
                                    .strip_prefix("__agent_")
                                    .unwrap_or(&wg_dir_name)
                                    .to_string();

                                let replica_config = wg_path.join("config.json")
                                    .exists()
                                    .then(|| std::fs::read_to_string(wg_path.join("config.json")).ok())
                                    .flatten()
                                    .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok());

                                let identity_path = replica_config.as_ref()
                                    .and_then(|v| v.get("identity")?.as_str().map(String::from));

                                // Resolve identity to determine origin project
                                let origin_project = identity_path.as_ref()
                                    .and_then(|rel| {
                                        let target = wg_path.join(rel);
                                        std::fs::canonicalize(&target)
                                            .inspect_err(|e| {
                                                log::warn!(
                                                    "[ac-discovery] identity canonicalize failed — replica='{}' target='{}' err={}",
                                                    wg_path.display(),
                                                    target.display(),
                                                    e
                                                );
                                            })
                                            .ok()
                                            .and_then(|abs| extract_origin_project(&abs))
                                    })
                                    .or_else(|| Some(project_folder.clone()));

                                // Resolve identity to matrix dir and read its lastCodingAgent
                                let preferred_agent_id = identity_path.as_ref().and_then(|rel| {
                                    read_preferred_agent_id(&wg_path.join(rel), &cfg.agents)
                                });

                                // Extract repos from config.json and resolve to absolute paths
                                let repo_paths: Vec<String> = replica_config.as_ref()
                                    .and_then(|v| v.get("repos")?.as_array().cloned())
                                    .unwrap_or_default()
                                    .iter()
                                    .filter_map(|r| r.as_str())
                                    .filter_map(|rel| {
                                        let resolved = wg_path.join(rel);
                                        std::fs::canonicalize(&resolved).ok()
                                            .map(|p| {
                                                let s = p.to_string_lossy();
                                                // Strip \\?\ UNC prefix that canonicalize adds on Windows
                                                s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
                                            })
                                    })
                                    .collect();

                                // Detect git branch for single-repo replicas
                                let repo_branch = if repo_paths.len() == 1 {
                                    detect_git_branch_sync(&repo_paths[0])
                                } else {
                                    None
                                };

                                // §AR2-strict: `is_coordinator` short-circuits on
                                // unqualified names, so build the project-qualified FQN
                                // (mirrors `agent_fqn_from_path`'s `<proj>:<wg>/<agent>`
                                // shape). Covered by
                                // teams::tests::is_any_coordinator_requires_qualified_fqn.
                                let is_coordinator = crate::config::teams::is_any_coordinator(
                                    &format!("{}:{}/{}", project_folder, dir_name, replica_name),
                                    &teams_snapshot,
                                );

                                wg_agents.push(AcAgentReplica {
                                    name: replica_name,
                                    path: wg_path.to_string_lossy().to_string(),
                                    identity_path,
                                    origin_project,
                                    preferred_agent_id,
                                    repo_paths,
                                    repo_branch,
                                    is_coordinator,
                                });
                            }
                        }
                    }
                    wg_agents.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

                    workgroups.push(AcWorkgroup {
                        name: dir_name.clone(),
                        path: path.to_string_lossy().to_string(),
                        brief,
                        agents: wg_agents,
                        repo_path,
                        team_name: None,
                    });
                    wg_project_map.insert(dir_name.clone(), repo_dir_str.clone());
                }

                // Teams: _team_*
                if dir_name.starts_with("_team_") {
                    let team_name = dir_name
                        .strip_prefix("_team_")
                        .unwrap_or(&dir_name)
                        .to_string();

                    let config_path = path.join("config.json");
                    if let Ok(content) = std::fs::read_to_string(&config_path) {
                        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&content) {
                            let team_agents = parsed
                                .get("agents")
                                .and_then(|a| a.as_array())
                                .map(|arr| {
                                    arr.iter()
                                        .filter_map(|v| v.as_str())
                                        .map(|r| resolve_agent_ref(&project_folder, r))
                                        .collect::<Vec<_>>()
                                })
                                .unwrap_or_default();

                            let coordinator = parsed
                                .get("coordinator")
                                .and_then(|c| c.as_str())
                                .map(|r| resolve_agent_ref(&project_folder, r));

                            teams.push(AcTeam {
                                name: team_name,
                                agents: team_agents,
                                coordinator,
                            });
                        }
                    }
                }
            }
        }
    }

    // Sort alphabetically
    agents.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    teams.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    workgroups.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    drop(cfg);

    // Associate each workgroup with its team by matching replica membership.
    // Two-pass approach: exact match across ALL teams first, then suffix fallback.
    // This prevents a suffix hit on team T1 from shadowing an exact hit on T2.
    for wg in &mut workgroups {
        // Pass 1: exact match (origin_project/name == team agent ref)
        let exact = teams.iter().find(|t| {
            wg.agents.iter().any(|agent| {
                let full_ref = format!(
                    "{}/{}",
                    agent.origin_project.as_deref().unwrap_or("unknown"),
                    agent.name
                );
                t.agents.contains(&full_ref)
            })
        });
        if let Some(t) = exact {
            wg.team_name = Some(t.name.clone());
            log::info!("[discovery] Workgroup '{}' → team '{}'", wg.name, t.name);
        } else {
            // Pass 2: suffix fallback — covers missing/stale identity, canonicalize
            // failure, or absolute-path team refs from different projects
            let suffix = teams.iter().find(|t| {
                wg.agents.iter().any(|agent| {
                    t.agents.iter().any(|team_ref| {
                        team_ref.rsplit('/').next()
                            .map_or(false, |s| s == agent.name)
                    })
                })
            });
            wg.team_name = suffix.map(|t| t.name.clone());
            if let Some(ref name) = wg.team_name {
                log::warn!(
                    "[discovery] Workgroup '{}' → team '{}' (matched via name suffix, identity may be missing)",
                    wg.name, name
                );
            } else {
                log::info!("[discovery] Workgroup '{}' → no team matched", wg.name);
            }
        }
    }

    // Update the branch watcher per-project. Each `.ac-new/`-containing dir gets its own
    // slot so multi-project setups don't overwrite each other (Grinch #1 + #12).
    let mut by_project: HashMap<String, Vec<AcWorkgroup>> = HashMap::new();
    for wg in &workgroups {
        if let Some(proj) = wg_project_map.get(&wg.name) {
            by_project.entry(proj.clone()).or_default().push(wg.clone());
        }
    }
    for (proj, wgs) in &by_project {
        branch_watcher.update_replicas_for_project(proj, wgs);
    }

    // Recompute coordinator flags on every live session against the hoisted team snapshot.
    let changes = {
        let mgr = session_mgr.read().await;
        mgr.refresh_coordinator_flags(&teams_snapshot).await
    };
    for (id, is_coord) in changes {
        let _ = app.emit(
            "session_coordinator_changed",
            crate::pty::git_watcher::CoordinatorChangedPayload {
                session_id: id.to_string(),
                is_coordinator: is_coord,
            },
        );
    }

    Ok(AcDiscoveryResult { agents, teams, workgroups })
}

/// Check if a folder has a .ac-new/ subdirectory.
#[tauri::command]
pub async fn check_project_path(path: String) -> Result<bool, String> {
    let ac_new = Path::new(&path).join(".ac-new");
    Ok(ac_new.is_dir())
}

/// Ensure .ac-new/.gitignore exists and contains all required exclusion patterns.
/// Called during project creation, workgroup creation, and opportunistically during discovery.
pub(crate) fn ensure_ac_new_gitignore(ac_new_dir: &Path) -> Result<(), String> {
    let gitignore_path = ac_new_dir.join(".gitignore");

    // Each entry: (pattern, comment explaining why)
    let required_entries: &[(&str, &str)] = &[
        (
            "wg-*/",
            "# AgentsCommander: exclude workgroup cloned repos from parent git tracking.\n# Without this, parent repo operations (checkout, reset) corrupt child clones.",
        ),
        (
            "**/__agent_*/last_ac_context.md",
            "# AgentsCommander: exclude managed session context files inside replica agent folders.",
        ),
        (
            "**/__agent_*/CLAUDE.md",
            "# AgentsCommander: exclude managed session context files inside replica agent folders.",
        ),
        (
            "**/__agent_*/GEMINI.md",
            "# AgentsCommander: exclude managed session context files inside replica agent folders.",
        ),
        (
            "**/__agent_*/AGENTS.md",
            "# AgentsCommander: exclude managed session context files inside replica agent folders.",
        ),
    ];

    if gitignore_path.exists() {
        let content = std::fs::read_to_string(&gitignore_path)
            .map_err(|e| format!("Failed to read .ac-new/.gitignore: {}", e))?;

        let mut additions = String::new();
        for (pattern, comment) in required_entries {
            if !content.lines().any(|line| line.trim() == *pattern) {
                additions.push_str(&format!("\n{}\n{}\n", comment, pattern));
            }
        }

        if !additions.is_empty() {
            let separator = if content.ends_with('\n') { "" } else { "\n" };
            std::fs::write(&gitignore_path, format!("{}{}{}", content, separator, additions))
                .map_err(|e| format!("Failed to update .ac-new/.gitignore: {}", e))?;
        }
    } else {
        let mut content = String::new();
        for (pattern, comment) in required_entries {
            content.push_str(&format!("{}\n{}\n\n", comment, pattern));
        }
        std::fs::write(&gitignore_path, content)
            .map_err(|e| format!("Failed to create .ac-new/.gitignore: {}", e))?;
    }

    Ok(())
}

/// Create a .ac-new/ directory inside the given path.
#[tauri::command]
pub async fn create_ac_project(path: String) -> Result<(), String> {
    let ac_new = Path::new(&path).join(".ac-new");
    std::fs::create_dir_all(&ac_new)
        .map_err(|e| format!("Failed to create .ac-new directory: {}", e))?;
    ensure_ac_new_gitignore(&ac_new)?;
    Ok(())
}

/// Discover AC agents/workgroups from a single project path.
/// Unlike discover_ac_agents which scans project_paths from settings,
/// this targets a specific folder.
#[tauri::command]
pub async fn discover_project(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    path: String,
    settings: State<'_, SettingsState>,
    branch_watcher: State<'_, Arc<DiscoveryBranchWatcher>>,
) -> Result<AcDiscoveryResult, String> {
    let base = Path::new(&path);
    if !base.is_dir() {
        return Err(format!("Path is not a directory: {}", path));
    }

    let cfg = settings.read().await;

    let ac_new_dir = base.join(".ac-new");
    if !ac_new_dir.is_dir() {
        return Ok(AcDiscoveryResult {
            agents: vec![],
            teams: vec![],
            workgroups: vec![],
        });
    }

    // Opportunistic: ensure gitignore protects workgroup clones
    let _ = ensure_ac_new_gitignore(&ac_new_dir);

    // Discovery-wide team snapshot — see discover_ac_agents for rationale.
    // Lock-safe: discover_teams() reads settings from disk via load_settings()
    // and does NOT acquire SettingsState; the read guard above stays valid.
    // Placed AFTER the .ac-new-missing early return so non-AC folders don't
    // pay a wasted filesystem scan (§15 Finding F1).
    let teams_snapshot = crate::config::teams::discover_teams();

    let project_folder = base
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    let mut agents: Vec<AcAgentMatrix> = Vec::new();
    let mut teams: Vec<AcTeam> = Vec::new();
    let mut workgroups: Vec<AcWorkgroup> = Vec::new();

    let entries = match std::fs::read_dir(&ac_new_dir) {
        Ok(e) => e,
        Err(e) => return Err(format!("Failed to read .ac-new directory: {}", e)),
    };

    for entry in entries.flatten() {
        let entry_path = entry.path();
        if !entry_path.is_dir() {
            continue;
        }

        let dir_name = match entry_path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        // Agent matrices: _agent_*
        if dir_name.starts_with("_agent_") {
            let display_name = agent_display_name(&project_folder, &dir_name);
            let role_exists = entry_path.join("Role.md").exists();

            let preferred_agent_id = read_preferred_agent_id(&entry_path, &cfg.agents);

            agents.push(AcAgentMatrix {
                name: display_name,
                path: entry_path.to_string_lossy().to_string(),
                role_exists,
                preferred_agent_id,
            });
        }

        // Workgroups: wg-*
        if dir_name.starts_with("wg-") {
            let brief = entry_path.join("BRIEF.md")
                .exists()
                .then(|| std::fs::read_to_string(entry_path.join("BRIEF.md")).ok())
                .flatten()
                .and_then(|content| content.lines().next().map(|l| l.trim_start_matches("# ").to_string()));

            let repo_path = std::fs::read_dir(&entry_path)
                .ok()
                .and_then(|entries| {
                    entries.flatten().find(|e| {
                        let n = e.file_name();
                        let name = n.to_string_lossy();
                        name.starts_with("repo-") && e.path().is_dir()
                    })
                })
                .map(|e| e.path().to_string_lossy().to_string());

            let mut wg_agents: Vec<AcAgentReplica> = Vec::new();
            if let Ok(wg_entries) = std::fs::read_dir(&entry_path) {
                for wg_entry in wg_entries.flatten() {
                    let wg_path = wg_entry.path();
                    if !wg_path.is_dir() {
                        continue;
                    }
                    let wg_dir_name = match wg_path.file_name().and_then(|n| n.to_str()) {
                        Some(n) => n.to_string(),
                        None => continue,
                    };
                    if wg_dir_name.starts_with("__agent_") {
                        let replica_name = wg_dir_name
                            .strip_prefix("__agent_")
                            .unwrap_or(&wg_dir_name)
                            .to_string();

                        let replica_config = wg_path.join("config.json")
                            .exists()
                            .then(|| std::fs::read_to_string(wg_path.join("config.json")).ok())
                            .flatten()
                            .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok());

                        let identity_path = replica_config.as_ref()
                            .and_then(|v| v.get("identity")?.as_str().map(String::from));

                        // Resolve identity to determine origin project
                        let origin_project = identity_path.as_ref()
                            .and_then(|rel| {
                                let target = wg_path.join(rel);
                                std::fs::canonicalize(&target)
                                    .inspect_err(|e| {
                                        log::warn!(
                                            "[ac-discovery] identity canonicalize failed — replica='{}' target='{}' err={}",
                                            wg_path.display(),
                                            target.display(),
                                            e
                                        );
                                    })
                                    .ok()
                                    .and_then(|abs| extract_origin_project(&abs))
                            })
                            .or_else(|| Some(project_folder.clone()));

                        let preferred_agent_id = identity_path.as_ref().and_then(|rel| {
                            read_preferred_agent_id(&wg_path.join(rel), &cfg.agents)
                        });

                        let repo_paths: Vec<String> = replica_config.as_ref()
                            .and_then(|v| v.get("repos")?.as_array().cloned())
                            .unwrap_or_default()
                            .iter()
                            .filter_map(|r| r.as_str())
                            .filter_map(|rel| {
                                let resolved = wg_path.join(rel);
                                std::fs::canonicalize(&resolved).ok()
                                    .map(|p| {
                                        let s = p.to_string_lossy();
                                        s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
                                    })
                            })
                            .collect();

                        let repo_branch = if repo_paths.len() == 1 {
                            detect_git_branch_sync(&repo_paths[0])
                        } else {
                            None
                        };

                        // §AR2-strict: `is_coordinator` short-circuits on
                        // unqualified names, so build the project-qualified FQN
                        // (mirrors `agent_fqn_from_path`'s `<proj>:<wg>/<agent>`
                        // shape). Covered by
                        // teams::tests::is_any_coordinator_requires_qualified_fqn.
                        let is_coordinator = crate::config::teams::is_any_coordinator(
                            &format!("{}:{}/{}", project_folder, dir_name, replica_name),
                            &teams_snapshot,
                        );

                        wg_agents.push(AcAgentReplica {
                            name: replica_name,
                            path: wg_path.to_string_lossy().to_string(),
                            identity_path,
                            origin_project,
                            preferred_agent_id,
                            repo_paths,
                            repo_branch,
                            is_coordinator,
                        });
                    }
                }
            }
            wg_agents.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

            workgroups.push(AcWorkgroup {
                name: dir_name.clone(),
                path: entry_path.to_string_lossy().to_string(),
                brief,
                agents: wg_agents,
                repo_path,
                team_name: None,
            });
        }

        // Teams: _team_*
        if dir_name.starts_with("_team_") {
            let team_name = dir_name
                .strip_prefix("_team_")
                .unwrap_or(&dir_name)
                .to_string();

            let config_path = entry_path.join("config.json");
            if let Ok(content) = std::fs::read_to_string(&config_path) {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&content) {
                    let team_agents = parsed
                        .get("agents")
                        .and_then(|a| a.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str())
                                .map(|r| resolve_agent_ref(&project_folder, r))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();

                    let coordinator = parsed
                        .get("coordinator")
                        .and_then(|c| c.as_str())
                        .map(|r| resolve_agent_ref(&project_folder, r));

                    teams.push(AcTeam {
                        name: team_name,
                        agents: team_agents,
                        coordinator,
                    });
                }
            }
        }
    }

    agents.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    teams.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    workgroups.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    // Associate each workgroup with its team by matching replica membership.
    // Two-pass approach: exact match across ALL teams first, then suffix fallback.
    // This prevents a suffix hit on team T1 from shadowing an exact hit on T2.
    for wg in &mut workgroups {
        // Pass 1: exact match (origin_project/name == team agent ref)
        let exact = teams.iter().find(|t| {
            wg.agents.iter().any(|agent| {
                let full_ref = format!(
                    "{}/{}",
                    agent.origin_project.as_deref().unwrap_or("unknown"),
                    agent.name
                );
                t.agents.contains(&full_ref)
            })
        });
        if let Some(t) = exact {
            wg.team_name = Some(t.name.clone());
            log::info!("[discovery] Workgroup '{}' → team '{}'", wg.name, t.name);
        } else {
            // Pass 2: suffix fallback — covers missing/stale identity, canonicalize
            // failure, or absolute-path team refs from different projects
            let suffix = teams.iter().find(|t| {
                wg.agents.iter().any(|agent| {
                    t.agents.iter().any(|team_ref| {
                        team_ref.rsplit('/').next()
                            .map_or(false, |s| s == agent.name)
                    })
                })
            });
            wg.team_name = suffix.map(|t| t.name.clone());
            if let Some(ref name) = wg.team_name {
                log::warn!(
                    "[discovery] Workgroup '{}' → team '{}' (matched via name suffix, identity may be missing)",
                    wg.name, name
                );
            } else {
                log::info!("[discovery] Workgroup '{}' → no team matched", wg.name);
            }
        }
    }

    drop(cfg);
    // Update the branch watcher for THIS project only.
    branch_watcher.update_replicas_for_project(&path, &workgroups);

    // Recompute coordinator flags on every live session against the hoisted team snapshot.
    let changes = {
        let mgr = session_mgr.read().await;
        mgr.refresh_coordinator_flags(&teams_snapshot).await
    };
    for (id, is_coord) in changes {
        let _ = app.emit(
            "session_coordinator_changed",
            crate::pty::git_watcher::CoordinatorChangedPayload {
                session_id: id.to_string(),
                is_coordinator: is_coord,
            },
        );
    }

    Ok(AcDiscoveryResult { agents, teams, workgroups })
}

/// Read the `context` array from a replica's config.json.
/// Returns an empty vec if the field is absent or the file doesn't exist.
#[tauri::command]
pub async fn get_replica_context_files(path: String) -> Result<Vec<String>, String> {
    let config_path = Path::new(&path).join("config.json");
    if !config_path.exists() {
        return Ok(vec![]);
    }

    let content = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("Failed to read config.json: {}", e))?;
    let parsed: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse config.json: {}", e))?;

    let files = parsed
        .get("context")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    Ok(files)
}

/// Write the `context` array to a replica's config.json.
/// Preserves all other fields in the config.
#[tauri::command]
pub async fn set_replica_context_files(path: String, files: Vec<String>) -> Result<(), String> {
    let config_path = Path::new(&path).join("config.json");

    // Read existing config or start fresh
    let mut config: serde_json::Value = if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)
            .map_err(|e| format!("Failed to read config.json: {}", e))?;
        serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse config.json: {}", e))?
    } else {
        serde_json::json!({})
    };

    // Update context field
    if files.is_empty() {
        if let Some(obj) = config.as_object_mut() {
            obj.remove("context");
        }
    } else {
        config["context"] = serde_json::json!(files);
    }

    let serialized = serde_json::to_string_pretty(&config)
        .map_err(|e| format!("Failed to serialize config.json: {}", e))?;
    std::fs::write(&config_path, &serialized)
        .map_err(|e| format!("Failed to write config.json: {}", e))?;

    log::info!("Updated context files for replica at {}: {:?}", path, files);
    Ok(())
}
