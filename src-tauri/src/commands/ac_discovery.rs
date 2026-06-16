use futures::future::join_all;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use crate::config::settings::SettingsState;
use crate::config::workspace::{
    canonical_workspace_dir_label, existing_workspace_dir, find_workspace_segment,
    has_workspace_dir, workspace_dir_for_project,
};
use crate::session::manager::SessionManager;
use crate::session::session::SessionRepo;

/// Per-call monotonic ID partitioning concurrent `discover_*` invocations in the log
/// stream. See plan §3 A0 (round-2 G5 + round-3 H1 placement-fix). Consumed only by
/// `format!`-into-log; `Relaxed` is canonical for an observed-but-not-synchronizing counter.
static DISCOVERY_CALL_ID: AtomicU64 = AtomicU64::new(0);

/// Resolve the preferred coding agent for a directory by matching the app
/// label from the agent's config.json against THIS instance's settings.
///
/// Flow: read config.json → get lastCodingAgent ID → get its `app` label
/// (e.g. "Claude Code") → find the agent in our settings with that label
/// → return OUR agent's ID. This decouples discovery from foreign agent IDs.
fn read_preferred_agent_id(
    dir: &Path,
    instance_agents: &[crate::config::settings::AgentConfig],
) -> Option<String> {
    let config_path = dir.join("config.json");
    let content = std::fs::read_to_string(&config_path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&content).ok()?;
    let tooling = v.get("tooling")?;

    // Get the foreign agent ID and its app label
    let foreign_id = tooling.get("lastCodingAgent")?.as_str()?;
    let app_label = tooling
        .get("codingAgents")?
        .get(foreign_id)?
        .get("app")?
        .as_str()?;

    // Match by label against this instance's configured agents
    let matches: Vec<_> = instance_agents
        .iter()
        .filter(|a| a.label == app_label)
        .collect();
    if matches.len() > 1 {
        log::warn!(
            "[discovery] Multiple agents with label '{}' — using first match (id={})",
            app_label,
            matches[0].id
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
    /// Persisted selection UI coding agent ID for this replica
    pub current_coding_agent_id: Option<String>,
    /// Persisted selection UI profile for this replica
    pub current_profile: Option<String>,
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
    /// First line of TASK.md (if exists)
    pub task: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_title: Option<String>,
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
    pub loops: Vec<crate::config::loops::AcLoopSummary>,
}

/// Extract the origin project name from a resolved identity path.
/// Looks for the folder immediately before the rightmost Project AC Root marker.
fn extract_origin_project(identity_abs_path: &std::path::Path) -> Option<String> {
    let s = identity_abs_path.to_string_lossy().replace('\\', "/");
    let parts: Vec<&str> = s.split('/').collect();
    find_workspace_segment(&parts).and_then(|i| (i > 0).then(|| parts[i - 1].to_string()))
}

struct ReplicaIdentityRead {
    config: Option<serde_json::Value>,
    identity: Option<crate::config::replica_identity::WgReplicaIdentity>,
    invalid_identity: bool,
}

fn read_replica_config_with_valid_identity(replica_dir: &Path) -> ReplicaIdentityRead {
    if !replica_dir.join("config.json").exists() {
        return ReplicaIdentityRead {
            config: None,
            identity: None,
            invalid_identity: false,
        };
    }

    match crate::config::replica_identity::read_and_repair_wg_replica_config(
        replica_dir,
        crate::config::replica_identity::WG_REPLICA_REQUIRED_CONTEXT,
    ) {
        Ok((config, identity)) => ReplicaIdentityRead {
            config: Some(config),
            identity: Some(identity),
            invalid_identity: false,
        },
        Err(e) => {
            log::warn!(
                "[ac-discovery] Rejected invalid WG replica identity for '{}': {}",
                replica_dir.display(),
                e
            );
            let config = std::fs::read_to_string(replica_dir.join("config.json"))
                .ok()
                .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok());
            ReplicaIdentityRead {
                config,
                identity: None,
                invalid_identity: true,
            }
        }
    }
}

fn origin_project_for_replica_identity(
    project_folder: &str,
    identity_read: &ReplicaIdentityRead,
) -> Option<String> {
    if identity_read.invalid_identity {
        return None;
    }
    identity_read
        .identity
        .as_ref()
        .and_then(|identity| extract_origin_project(&identity.matrix_dir))
        .or_else(|| Some(project_folder.to_string()))
}

/// Derive agent display name from its path.
/// Format: "{project_folder}/{agent_name}" where:
/// - project_folder = directory containing the Project AC Root
/// - agent_name = folder name with "_agent_" prefix stripped
fn agent_display_name(project_folder: &str, dir_name: &str) -> String {
    let agent_name = dir_name.strip_prefix("_agent_").unwrap_or(dir_name);
    format!("{}/{}", project_folder, agent_name)
}

/// Resolve an agent ref to a display name. Handles both relative refs
/// (e.g. "../_agent_tech-lead") and absolute paths.
/// For relative refs, uses project_folder as origin. For absolute paths,
/// extracts the origin project from the folder before the Project AC Root marker.
fn resolve_agent_ref(project_folder: &str, agent_ref: &str) -> String {
    let normalized = agent_ref.replace('\\', "/");
    let trimmed = normalized
        .trim_start_matches("../")
        .trim_start_matches("./");
    if trimmed.contains(':') || trimmed.starts_with('/') {
        // Absolute path: extract origin project from folder before the Project AC Root marker
        let parts: Vec<&str> = trimmed.split('/').collect();
        let origin = find_workspace_segment(&parts)
            .and_then(|i| (i > 0).then_some(parts[i - 1]))
            .unwrap_or(project_folder);
        let dir_name = parts.last().unwrap_or(&trimmed);
        agent_display_name(origin, dir_name)
    } else {
        agent_display_name(project_folder, trimmed)
    }
}

/// Extract the first content line from a TASK.md-style markdown file,
/// skipping any YAML frontmatter block at the top (delimited by `---` on
/// its own line). Leading `# ` heading markers are stripped from the result.
///
/// Without this, a TASK.md that opens with `---` (frontmatter) sends the
/// literal `---` to the sidebar — the `wg.task` field then defeats the
/// frontend's frontmatter-stripping renderer (issue #161).
fn extract_task_first_line(content: &str) -> Option<String> {
    // Strip a UTF-8 BOM if present — `read_to_string` does not, and `str::trim`
    // does not treat U+FEFF as whitespace, so without this the frontmatter opener
    // check below sees `\u{FEFF}---` instead of `---` and the heading-strip below
    // leaves the BOM on the result.
    let content = content.strip_prefix('\u{FEFF}').unwrap_or(content);

    let mut lines = content.lines().peekable();

    // Drain leading blank lines first so a stray "\n\n" before the frontmatter
    // opener doesn't bypass detection and leak the literal `---` into the sidebar.
    let mut first_non_empty = lines.peek().map(|l| l.trim());
    while first_non_empty == Some("") {
        lines.next();
        first_non_empty = lines.peek().map(|l| l.trim());
    }

    // YAML frontmatter: opener `---` on the first non-empty line, drain through closer.
    // If the closer is missing, the for-loop drains the rest and `find` returns None.
    if first_non_empty == Some("---") {
        lines.next();
        for line in lines.by_ref() {
            if line.trim() == "---" {
                break;
            }
        }
    }

    lines
        .find(|l| !l.trim().is_empty())
        .map(|l| l.trim_start_matches("# ").to_string())
}

/// Strip the Windows verbatim/UNC `\\?\` prefix if present so the emitted
/// path matches the form `discover_project` produces (which never has the
/// prefix because it comes from a `read_dir` walk). The codebase already
/// applies the same strip downstream when embedding paths into the agent
/// init prompt — see the `find_workgroup_task_path_handles_unc_prefix_input`
/// test note in `session.rs`.
fn strip_verbatim_prefix(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    s.strip_prefix(r"\\?\")
        .map(PathBuf::from)
        .unwrap_or_else(|| p.to_path_buf())
}

/// Detect git branch synchronously for a given directory path.
fn detect_git_branch_sync(dir: &str) -> Option<String> {
    #[cfg(windows)]
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let mut cmd = std::process::Command::new("git");
    crate::pty::credentials::scrub_credentials_from_std_command(&mut cmd);
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

/// #280 §3.3 — per-path counter for `detect_branch` timeouts. Mirrors the
/// `git_watcher.rs` helper but lives in a separate module-local map so the
/// two watchers track their own paths independently (different scan sets).
fn note_discovery_timeout(path: &str) -> u64 {
    use std::sync::OnceLock;
    static TIMEOUT_COUNTS: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
    let map = TIMEOUT_COUNTS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut g = map.lock().unwrap_or_else(|e| e.into_inner());
    let count = g.entry(path.to_string()).or_insert(0);
    *count += 1;
    *count
}

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

#[derive(Clone, PartialEq, Eq)]
struct StatSentinel {
    len: u64,
    mtime: Option<SystemTime>,
}

#[derive(Clone)]
struct TaskCacheEntry {
    sentinel: Option<StatSentinel>,
    task: Option<String>,
    task_title: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskUpdatedPayload {
    workgroup_root: String,
    source: String,
    task: Option<String>,
    // Always serialize taskTitle, even when None, so the consumer receives an
    // explicit signal (null) rather than `undefined`. Matches the manual-emit
    // payload built by `commands::task::emit_task_updated`. Architect sign-off
    // on PR #304: normalize both emitters to explicit-null.
    task_title: Option<String>,
    session_ids: Vec<String>,
}

pub struct DiscoveryBranchWatcher {
    app_handle: AppHandle,
    session_manager: Arc<tokio::sync::RwLock<SessionManager>>,
    /// Keyed by the project directory that directly contains the Project AC Root, not by
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
    /// Per-workgroup-root cache for Gate C (task detection). Keyed by the
    /// stripped (no `\\?\`) absolute path of the wg-* directory. Bounded
    /// implicitly by the union of (loaded-project replicas, active sessions);
    /// no explicit prune since entries are ~200B each and the upper bound is
    /// the user's project layout. A stale entry for a wg that no longer has
    /// sessions or replicas is harmless: next tick will simply not visit it.
    task_cache: Mutex<HashMap<PathBuf, TaskCacheEntry>>,
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
            task_cache: Mutex::new(HashMap::new()),
        })
    }

    /// Update this project's replicas in the watcher. `project_dir` is the directory
    /// that directly contains the Project AC Root, not a grand-parent from `settings.project_paths`.
    /// See the invariant comment on the `replicas` field.
    pub fn update_replicas_for_project(&self, project_dir: &str, workgroups: &[AcWorkgroup]) {
        // Invariant guard: catch mistaken call-site passes (e.g. a `base_path` parent)
        // in dev builds. Release builds log a warn and return to prevent silent corruption.
        let has_workspace = has_workspace_dir(Path::new(project_dir));
        debug_assert!(
            has_workspace,
            "update_replicas_for_project: {} does not contain a Project AC Root",
            project_dir
        );
        if !has_workspace {
            log::warn!(
                "[DiscoveryBranchWatcher] update_replicas_for_project called with {} which has no Project AC Root, ignoring",
                project_dir
            );
            return;
        }

        // Canonicalize the key so callers that pass slightly-different string shapes
        // (backslash vs forward slash, trailing separator, unresolved "..") still
        // converge to one map slot per project. Without this, the same project can
        // end up with two entries (e.g. from `discover_ac_agents` reading `read_dir`
        // output vs `discover_project` receiving a user-typed path) and emit doubled.
        let canonical_key = std::fs::canonicalize(project_dir)
            .ok()
            .map(|p| {
                let s = p.to_string_lossy();
                s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
            })
            .unwrap_or_else(|| project_dir.to_string());

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

        // Gate A + Gate B (existing replica-driven git detection). Wrapped so
        // an empty `entries` (no project loaded) still falls through to Gate C,
        // which is also driven by active sessions — sessions can exist for
        // workgroups whose project is not in `settings.projectPaths`.
        if !entries.is_empty() {
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
                    .zip(branches)
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

        // Gate C: TASK.md detection. Runs every tick whether or not Gate A/B
        // had work to do — sessions in workgroups whose project is not loaded
        // still need brief updates.
        self.poll_tasks(&entries).await;
    }

    /// Gate C: detect TASK.md changes per unique workgroup root and emit
    /// `workgroup_task_updated` on change. Runs on every existing 15s tick
    /// of `poll()`; no new thread, no new cadence.
    async fn poll_tasks(&self, entries: &[ReplicaBranchEntry]) {
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
                wg_roots.entry(strip_verbatim_prefix(parent)).or_default();
            }
        }

        let sessions: Vec<(Uuid, String)> = {
            let mgr = self.session_manager.read().await;
            mgr.get_sessions_working_dirs().await
        };
        for (id, cwd) in sessions {
            if let Some(task_path) = crate::session::session::find_workgroup_task_path_for_cwd(&cwd)
            {
                if let Some(parent) = task_path.parent() {
                    wg_roots
                        .entry(strip_verbatim_prefix(parent))
                        .or_default()
                        .push(id);
                }
            }
        }

        if wg_roots.is_empty() {
            return;
        }

        for (wg_root, session_ids) in wg_roots {
            self.check_workgroup_task(wg_root, session_ids).await;
        }
    }

    /// Per-workgroup brief check. Stat short-circuits unchanged files; on
    /// stat-change, reads (with size cap), re-stats (defends against torn
    /// in-place editor saves), and emits if content actually changed.
    async fn check_workgroup_task(&self, wg_root: PathBuf, session_ids: Vec<Uuid>) {
        let task_path = wg_root.join("TASK.md");

        let now_sentinel = std::fs::metadata(&task_path).ok().map(|m| StatSentinel {
            len: m.len(),
            mtime: m.modified().ok(),
        });

        // Mutex held only for the duration of the get; released before any I/O.
        // CRITICAL: never hold std::sync::Mutex across an .await — it's not
        // tokio-aware and would deadlock under load.
        let prev = self.task_cache.lock().unwrap().get(&wg_root).cloned();

        // Stat-equality short-circuit — the steady-state path. Cost: one
        // metadata() call per wg per tick when nothing has changed.
        if let Some(ref prev_entry) = prev {
            if prev_entry.sentinel == now_sentinel {
                return;
            }
        }

        // Read with a 256 KiB cap. A bigger TASK.md is either accidental
        // (someone catted /dev/urandom into it) or adversarial; either way,
        // we don't want it streamed through Tauri IPC every 15s. On overflow
        // we treat the file as effectively missing (None); the frontend
        // already handles `brief: null` (panel falls back to "...").
        let (new_task, new_title) = read_task_fields(wg_root.as_path());

        // Re-stat. If the file changed during our read window (external
        // editor mid-save — Notepad does CreateFile(OPEN_EXISTING) +
        // SetEndOfFile + write, NOT atomic rename), the read may be torn.
        // Defer to the next tick when the stat has settled.
        let post_sentinel = std::fs::metadata(&task_path).ok().map(|m| StatSentinel {
            len: m.len(),
            mtime: m.modified().ok(),
        });
        if post_sentinel != now_sentinel {
            log::debug!(
                "[DiscoveryBranchWatcher] stat changed during read of {} (likely torn — external editor mid-save); deferring to next tick",
                task_path.display()
            );
            return;
        }

        let content_changed = match prev.as_ref() {
            Some(p) => p.task != new_task || p.task_title != new_title,
            None => true,
        };

        // ALWAYS refresh the sentinel (next-tick short-circuit depends on
        // it). Insert a placeholder `task = prev.task` so a failed emit
        // below leaves the cache holding the previously-shipped content,
        // not the new content — that way the next stat-change retries
        // emission instead of silently accepting the failed state.
        {
            let mut cache = self.task_cache.lock().unwrap();
            cache
                .entry(wg_root.clone())
                .and_modify(|e| e.sentinel = now_sentinel.clone())
                .or_insert(TaskCacheEntry {
                    sentinel: now_sentinel.clone(),
                    task: prev.as_ref().and_then(|p| p.task.clone()),
                    task_title: prev.as_ref().and_then(|p| p.task_title.clone()),
                });
        }

        if !content_changed {
            return;
        }

        let payload = TaskUpdatedPayload {
            workgroup_root: wg_root.to_string_lossy().into_owned(),
            source: "poll".to_string(),
            task: new_task.clone(),
            task_title: new_title.clone(),
            session_ids: session_ids.iter().map(|u| u.to_string()).collect(),
        };
        match self.app_handle.emit("workgroup_task_updated", payload) {
            Ok(()) => {
                // Commit shipped content. Mirrors GitWatcher's emit-then-cache
                // ordering — invariant: the cache's `brief` field is the last
                // value the FRONTEND has, not the last value we read.
                self.task_cache
                    .lock()
                    .unwrap()
                    .entry(wg_root.clone())
                    .and_modify(|e| {
                        e.task = new_task;
                        e.task_title = new_title;
                    });
            }
            Err(e) => {
                log::warn!(
                    "[DiscoveryBranchWatcher] task emit failed for {} ({}); leaving cached task stale so next stat-change retries",
                    wg_root.display(),
                    e
                );
            }
        }
    }

    async fn detect_branch_with_timeout(working_dir: &str) -> Option<String> {
        match tokio::time::timeout(DETECT_TIMEOUT, Self::detect_branch(working_dir)).await {
            Ok(result) => result,
            Err(_) => {
                let n = note_discovery_timeout(working_dir);
                // #280 §3.3 — same dampening policy as git_watcher.rs: log
                // first sighting + every 50th. Module tag remains
                // `[DiscoveryBranchWatcher]` so the two watchers stay
                // distinguishable in app.log.
                if n == 1 || n.is_multiple_of(50) {
                    log::warn!(
                        "[DiscoveryBranchWatcher] detect_branch timed out for {} (>{}s); occurrence={} (logging 1st + every 50th)",
                        working_dir,
                        DETECT_TIMEOUT.as_secs(),
                        n
                    );
                }
                None
            }
        }
    }

    async fn detect_branch(dir: &str) -> Option<String> {
        #[cfg(windows)]
        const CREATE_NO_WINDOW: u32 = 0x08000000;

        let mut cmd = tokio::process::Command::new("git");
        crate::pty::credentials::scrub_credentials_from_tokio_command(&mut cmd);
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

/// Discover AC agent matrices from Project AC Root directories within configured repo paths.
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
    let call_id = DISCOVERY_CALL_ID.fetch_add(1, Ordering::Relaxed);
    let mut agents: Vec<AcAgentMatrix> = Vec::new();
    let mut teams: Vec<AcTeam> = Vec::new();
    let mut workgroups: Vec<AcWorkgroup> = Vec::new();
    let mut loops: Vec<crate::config::loops::AcLoopSummary> = Vec::new();
    // Track the Project AC Root-containing dir each workgroup originated from. Keys are
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
            let Some(workspace_dir) = existing_workspace_dir(&repo_dir) else {
                continue;
            };
            let repo_dir_str = repo_dir.to_string_lossy().to_string();

            // Opportunistic: ensure gitignore exists for existing projects
            let _ = ensure_workspace_gitignore(&workspace_dir);
            loops.extend(crate::config::loops::discover_loops_in_project(&repo_dir));

            let project_folder = repo_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();

            let entries = match std::fs::read_dir(&workspace_dir) {
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

                    log::info!(
                        "[ac-discovery] agent: dir={:?}, preferred_agent_id={:?}",
                        dir_name,
                        preferred_agent_id
                    );

                    agents.push(AcAgentMatrix {
                        name: display_name,
                        path: path.to_string_lossy().to_string(),
                        role_exists,
                        preferred_agent_id,
                    });
                }

                // Workgroups: wg-*
                if dir_name.starts_with("wg-") {
                    let (task, task_title) = read_task_fields(&path);

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

                                let identity_read =
                                    read_replica_config_with_valid_identity(&wg_path);

                                let identity_path = identity_read
                                    .identity
                                    .as_ref()
                                    .map(|identity| identity.identity.clone());

                                let origin_project = origin_project_for_replica_identity(
                                    &project_folder,
                                    &identity_read,
                                );

                                // Resolve identity to matrix dir and read its lastCodingAgent
                                let preferred_agent_id =
                                    identity_read.identity.as_ref().and_then(|identity| {
                                        read_preferred_agent_id(&identity.matrix_dir, &cfg.agents)
                                    });
                                let current_coding_agent_id = crate::config::coding_agent_profiles::read_replica_current_coding_agent(&wg_path)
                                    .filter(|id| cfg.agents.iter().any(|agent| agent.id == *id));
                                let profile_read =
                                    crate::config::coding_agent_profiles::read_replica_profile_result(&wg_path);
                                if let Some(warning) = &profile_read.warning {
                                    log::warn!("[ac-discovery] {}", warning);
                                }
                                let current_profile = profile_read.profile;

                                // Extract repos from config.json and resolve to absolute paths
                                let repo_paths: Vec<String> = identity_read
                                    .config
                                    .as_ref()
                                    .and_then(|v| v.get("repos")?.as_array().cloned())
                                    .unwrap_or_default()
                                    .iter()
                                    .filter_map(|r| r.as_str())
                                    .filter_map(|rel| {
                                        let resolved = wg_path.join(rel);
                                        std::fs::canonicalize(&resolved).ok().map(|p| {
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
                                let is_coordinator =
                                    crate::config::teams::resolve_wg_coordinator_replica(
                                        &workspace_dir,
                                        &path,
                                    )
                                    .map(|resolved| resolved.agent_name == replica_name)
                                    .unwrap_or(false);

                                log::debug!(
                                    "[ac-discovery] call={} replica — project='{}' wg='{}' replica='{}' fqn='{}:{}/{}' is_coordinator={}",
                                    call_id,
                                    project_folder,
                                    dir_name,
                                    replica_name,
                                    project_folder, dir_name, replica_name,
                                    is_coordinator
                                );

                                wg_agents.push(AcAgentReplica {
                                    name: replica_name,
                                    path: wg_path.to_string_lossy().to_string(),
                                    identity_path,
                                    origin_project,
                                    preferred_agent_id,
                                    current_coding_agent_id,
                                    current_profile,
                                    repo_paths,
                                    repo_branch,
                                    is_coordinator,
                                });
                            }
                        }
                    }
                    wg_agents.sort_by_key(|a| a.name.to_lowercase());

                    workgroups.push(AcWorkgroup {
                        name: dir_name.clone(),
                        path: path.to_string_lossy().to_string(),
                        task,
                        task_title,
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
    agents.sort_by_key(|a| a.name.to_lowercase());
    teams.sort_by_key(|a| a.name.to_lowercase());
    workgroups.sort_by_key(|a| a.name.to_lowercase());
    loops.sort_by_key(|a| a.name.to_lowercase());
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
                        team_ref.rsplit('/').next().is_some_and(|s| s == agent.name)
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

    // Update the branch watcher per-project. Each Project AC Root-containing dir gets its own
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

    let total_replicas: usize = workgroups.iter().map(|wg| wg.agents.len()).sum();
    let total_coordinator: usize = workgroups
        .iter()
        .flat_map(|wg| wg.agents.iter())
        .filter(|a| a.is_coordinator)
        .count();
    log::debug!(
        "[ac-discovery] call={} discover_ac_agents: summary — workgroups={} teams={} replicas={} coordinator={}",
        call_id,
        workgroups.len(),
        teams.len(),
        total_replicas,
        total_coordinator
    );

    Ok(AcDiscoveryResult {
        agents,
        teams,
        workgroups,
        loops,
    })
}

/// Check if a folder has a Project AC Root subdirectory.
#[tauri::command]
pub async fn check_project_path(path: String) -> Result<bool, String> {
    Ok(existing_workspace_dir(Path::new(&path)).is_some())
}

/// Ensure the Project AC Root .gitignore exists and contains all required exclusion patterns.
/// Called during project creation, workgroup creation, and opportunistically during discovery.
pub(crate) fn ensure_workspace_gitignore(workspace_dir: &Path) -> Result<(), String> {
    let gitignore_path = workspace_dir.join(".gitignore");

    // Each entry: (pattern, comment explaining why)
    let required_entries: &[(&str, &str)] = &[
        (
            "wg-*/",
            "# AgentsCommander: exclude workgroup cloned repos from parent git tracking.\n# Without this, parent repo operations (checkout, reset) corrupt child clones.",
        ),
        (
            ".deleting-*/",
            "# AgentsCommander: exclude temporary workgroup delete sentinels/orphans.",
        ),
        (
            "_loop_*/state.json",
            "# AgentsCommander: exclude Loop scheduler runtime state.",
        ),
        (
            "_loop_*/audit.jsonl",
            "# AgentsCommander: exclude Loop runtime audit logs with prompt snapshots.",
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
            .map_err(|e| format!("Failed to read Project AC Root .gitignore: {}", e))?;

        let mut additions = String::new();
        for (pattern, comment) in required_entries {
            if !content.lines().any(|line| line.trim() == *pattern) {
                additions.push_str(&format!("\n{}\n{}\n", comment, pattern));
            }
        }

        if !additions.is_empty() {
            let separator = if content.ends_with('\n') { "" } else { "\n" };
            std::fs::write(
                &gitignore_path,
                format!("{}{}{}", content, separator, additions),
            )
            .map_err(|e| format!("Failed to update Project AC Root .gitignore: {}", e))?;
        }
    } else {
        let mut content = String::new();
        for (pattern, comment) in required_entries {
            content.push_str(&format!("{}\n{}\n\n", comment, pattern));
        }
        std::fs::write(&gitignore_path, content)
            .map_err(|e| format!("Failed to create Project AC Root .gitignore: {}", e))?;
    }

    Ok(())
}

/// Create a canonical .ac/ directory inside the given path.
#[tauri::command]
pub async fn create_ac_project(path: String) -> Result<(), String> {
    create_ac_project_impl(&path, |workspace_dir| {
        crate::config::session_context::create_default_context_templates(workspace_dir)
    })
}

fn create_ac_project_impl<F>(path: &str, create_context_templates: F) -> Result<(), String>
where
    F: FnOnce(&Path) -> Result<(), String>,
{
    let workspace_dir = workspace_dir_for_project(Path::new(&path));
    let created = !workspace_dir.exists();
    std::fs::create_dir_all(&workspace_dir).map_err(|e| {
        format!(
            "Failed to create {} directory: {}",
            canonical_workspace_dir_label(),
            e
        )
    })?;
    if created {
        if let Err(err) = ensure_workspace_gitignore(&workspace_dir)
            .and_then(|_| create_context_templates(&workspace_dir))
        {
            if let Err(cleanup_err) = std::fs::remove_dir_all(&workspace_dir) {
                log::warn!(
                    "Failed to clean up newly created {} directory after setup failure: {}",
                    workspace_dir.display(),
                    cleanup_err
                );
            }
            return Err(err);
        }
    } else {
        ensure_workspace_gitignore(&workspace_dir)?;
    }
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

    let Some(workspace_dir) = existing_workspace_dir(base) else {
        return Ok(AcDiscoveryResult {
            agents: vec![],
            teams: vec![],
            workgroups: vec![],
            loops: vec![],
        });
    };

    // Opportunistic: ensure gitignore protects workgroup clones
    let _ = ensure_workspace_gitignore(&workspace_dir);

    // Discovery-wide team snapshot — see discover_ac_agents for rationale.
    // Lock-safe: discover_teams() reads settings from disk via load_settings()
    // and does NOT acquire SettingsState; the read guard above stays valid.
    // Placed AFTER the workspace-missing early return so non-AC folders don't
    // pay a wasted filesystem scan (§15 Finding F1).
    let teams_snapshot = crate::config::teams::discover_teams();
    let call_id = DISCOVERY_CALL_ID.fetch_add(1, Ordering::Relaxed);

    let project_folder = base
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    let mut agents: Vec<AcAgentMatrix> = Vec::new();
    let mut teams: Vec<AcTeam> = Vec::new();
    let mut workgroups: Vec<AcWorkgroup> = Vec::new();
    let mut loops = crate::config::loops::discover_loops_in_project(base);

    let entries = match std::fs::read_dir(&workspace_dir) {
        Ok(e) => e,
        Err(e) => return Err(format!("Failed to read Project AC Root directory: {}", e)),
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
            let (task, task_title) = read_task_fields(&entry_path);

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

                        let identity_read = read_replica_config_with_valid_identity(&wg_path);

                        let identity_path = identity_read
                            .identity
                            .as_ref()
                            .map(|identity| identity.identity.clone());

                        let origin_project =
                            origin_project_for_replica_identity(&project_folder, &identity_read);

                        let preferred_agent_id =
                            identity_read.identity.as_ref().and_then(|identity| {
                                read_preferred_agent_id(&identity.matrix_dir, &cfg.agents)
                            });
                        let current_coding_agent_id = crate::config::coding_agent_profiles::read_replica_current_coding_agent(&wg_path)
                            .filter(|id| cfg.agents.iter().any(|agent| agent.id == *id));
                        let profile_read =
                            crate::config::coding_agent_profiles::read_replica_profile_result(
                                &wg_path,
                            );
                        if let Some(warning) = &profile_read.warning {
                            log::warn!("[ac-discovery] {}", warning);
                        }
                        let current_profile = profile_read.profile;

                        let repo_paths: Vec<String> = identity_read
                            .config
                            .as_ref()
                            .and_then(|v| v.get("repos")?.as_array().cloned())
                            .unwrap_or_default()
                            .iter()
                            .filter_map(|r| r.as_str())
                            .filter_map(|rel| {
                                let resolved = wg_path.join(rel);
                                std::fs::canonicalize(&resolved).ok().map(|p| {
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
                        let is_coordinator = crate::config::teams::resolve_wg_coordinator_replica(
                            &workspace_dir,
                            &entry_path,
                        )
                        .map(|resolved| resolved.agent_name == replica_name)
                        .unwrap_or(false);

                        log::debug!(
                            "[ac-discovery] call={} replica — project='{}' wg='{}' replica='{}' fqn='{}:{}/{}' is_coordinator={}",
                            call_id,
                            project_folder,
                            dir_name,
                            replica_name,
                            project_folder, dir_name, replica_name,
                            is_coordinator
                        );

                        wg_agents.push(AcAgentReplica {
                            name: replica_name,
                            path: wg_path.to_string_lossy().to_string(),
                            identity_path,
                            origin_project,
                            preferred_agent_id,
                            current_coding_agent_id,
                            current_profile,
                            repo_paths,
                            repo_branch,
                            is_coordinator,
                        });
                    }
                }
            }
            wg_agents.sort_by_key(|a| a.name.to_lowercase());

            workgroups.push(AcWorkgroup {
                name: dir_name.clone(),
                path: entry_path.to_string_lossy().to_string(),
                task,
                task_title,
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

    agents.sort_by_key(|a| a.name.to_lowercase());
    teams.sort_by_key(|a| a.name.to_lowercase());
    workgroups.sort_by_key(|a| a.name.to_lowercase());
    loops.sort_by_key(|a| a.name.to_lowercase());

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
                        team_ref.rsplit('/').next().is_some_and(|s| s == agent.name)
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

    // Recompute coordinator flags on every live session against the hoisted team
    // snapshot. Spawned (not awaited) so populating the project tree NEVER blocks on
    // `session_mgr` / inner `sessions` lock contention during the boot-concurrent
    // session restore (#384 empty-tree hang fix). This refresh only emits
    // `session_coordinator_changed` for live sessions; it is not needed to build the
    // returned tree (each replica's `is_coordinator` is computed inline above), so
    // moving it off the return path is behavior-preserving for the tree while keeping
    // the coordinator-flag events flowing.
    {
        let coordinator_app = app.clone();
        let coordinator_session_mgr = Arc::clone(session_mgr.inner());
        tokio::spawn(async move {
            let changes = {
                let mgr = coordinator_session_mgr.read().await;
                mgr.refresh_coordinator_flags(&teams_snapshot).await
            };
            for (id, is_coord) in changes {
                let _ = coordinator_app.emit(
                    "session_coordinator_changed",
                    crate::pty::git_watcher::CoordinatorChangedPayload {
                        session_id: id.to_string(),
                        is_coordinator: is_coord,
                    },
                );
            }
        });
    }

    let total_replicas: usize = workgroups.iter().map(|wg| wg.agents.len()).sum();
    let total_coordinator: usize = workgroups
        .iter()
        .flat_map(|wg| wg.agents.iter())
        .filter(|a| a.is_coordinator)
        .count();
    log::debug!(
        "[ac-discovery] call={} discover_project: summary — path='{}' workgroups={} teams={} replicas={} coordinator={}",
        call_id,
        path,
        workgroups.len(),
        teams.len(),
        total_replicas,
        total_coordinator
    );

    Ok(AcDiscoveryResult {
        agents,
        teams,
        workgroups,
        loops,
    })
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

    crate::config::local_config_io::update_config_json_object(&config_path, true, |obj| {
        if files.is_empty() {
            obj.remove("context");
        } else {
            obj.insert("context".to_string(), serde_json::json!(files));
        }
        Ok(())
    })?;

    log::info!("Updated context files for replica at {}: {:?}", path, files);
    Ok(())
}

// ── #191 — shared open/new project commands ──────────────────────────────

/// Validate an existing AC project at `path` and register it in
/// `settings.project_paths`. Mirrors the ActionBar "Open Project" flow at
/// `src/sidebar/components/ActionBar.tsx:78-94` but performs the dedup +
/// persist atomically against `SettingsState`.
///
/// Holds the SettingsState write lock through `save_settings` — same pattern
/// as `set_inject_rtk_hook` (`src-tauri/src/commands/config.rs:184-194`) — so
/// concurrent `update_settings` calls cannot race.
#[tauri::command]
pub async fn open_project(
    settings: State<'_, SettingsState>,
    path: String,
) -> Result<crate::config::projects::ProjectRegistration, String> {
    let mut s = settings.write().await;
    let result = crate::config::projects::register_existing_project(&mut s, &path)
        .map_err(|e| e.to_string())?;
    let snapshot = s.clone();
    crate::config::settings::save_settings(&snapshot)?;
    drop(s); // explicit; lock released AFTER the disk write completes
    Ok(result)
}

/// Ensure an AC project at `path` (creating `.ac/` if missing) and
/// register it in `settings.project_paths`. Mirrors the ActionBar "New
/// Project" flow at `src/sidebar/components/ActionBar.tsx:58-71`.
#[tauri::command]
pub async fn new_project(
    settings: State<'_, SettingsState>,
    path: String,
) -> Result<crate::config::projects::ProjectRegistration, String> {
    let mut s = settings.write().await;
    let result =
        crate::config::projects::register_new_project(&mut s, &path).map_err(|e| e.to_string())?;
    let snapshot = s.clone();
    crate::config::settings::save_settings(&snapshot)?;
    drop(s); // explicit; lock released AFTER the disk write completes
    Ok(result)
}

type TaskFields = (Option<String>, Option<String>);

fn read_task_fields(wg_path: &Path) -> TaskFields {
    let task_path = wg_path.join("TASK.md");
    let Ok(content) = std::fs::read_to_string(&task_path) else {
        return (None, None);
    };
    let task = extract_task_first_line(&content);
    let task_title = crate::commands::entity_creation::parse_task_title(&content);
    (task, task_title)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn check_project_path_accepts_ac() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(tmp.path().join(".ac")).expect("create .ac");

        assert!(check_project_path(tmp.path().to_string_lossy().to_string())
            .await
            .expect("check path"));
    }

    #[tokio::test]
    async fn check_project_path_rejects_non_ac_workspace() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(tmp.path().join(".workspace")).expect("create non-AC workspace");

        assert!(
            !check_project_path(tmp.path().to_string_lossy().to_string())
                .await
                .expect("check path")
        );
    }

    #[tokio::test]
    async fn create_ac_project_creates_ac() {
        let tmp = tempfile::tempdir().expect("tempdir");

        create_ac_project(tmp.path().to_string_lossy().to_string())
            .await
            .expect("create project");

        assert!(tmp.path().join(".ac").is_dir());
        assert!(tmp.path().join(".ac").join(".gitignore").is_file());
        assert!(tmp
            .path()
            .join(".ac")
            .join(crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME)
            .is_file());
        assert!(tmp
            .path()
            .join(".ac")
            .join(crate::config::session_context::COORDINATOR_CONTEXT_TEMPLATE_FILENAME)
            .is_file());
        assert!(!tmp.path().join(".ac").join("templates").exists());
    }

    #[tokio::test]
    async fn create_ac_project_retries_after_failed_fresh_seed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().to_string_lossy().to_string();

        let first_result = create_ac_project_impl(&path, |workspace_dir| {
            std::fs::write(
                workspace_dir
                    .join(crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME),
                crate::config::session_context::get_default_agent_template(),
            )
            .expect("write partial agent context");
            Err("injected seed failure".to_string())
        });

        assert!(first_result.is_err());
        assert!(
            !tmp.path().join(".ac").exists(),
            "fresh .ac directory should be cleaned up after seed failure"
        );

        create_ac_project(path).await.expect("retry create project");

        assert!(tmp
            .path()
            .join(".ac")
            .join(crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME)
            .is_file());
        assert!(tmp
            .path()
            .join(".ac")
            .join(crate::config::session_context::COORDINATOR_CONTEXT_TEMPLATE_FILENAME)
            .is_file());
    }

    #[tokio::test]
    async fn create_ac_project_does_not_backfill_existing_ac_contexts() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let workspace = tmp.path().join(".ac");
        std::fs::create_dir(&workspace).expect("create .ac");

        create_ac_project(tmp.path().to_string_lossy().to_string())
            .await
            .expect("create project");

        assert!(!workspace
            .join(crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME)
            .exists());
        assert!(!workspace
            .join(crate::config::session_context::COORDINATOR_CONTEXT_TEMPLATE_FILENAME)
            .exists());
    }

    #[test]
    fn ensure_workspace_gitignore_includes_delete_sentinels_on_create() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let workspace = tmp.path().join(".ac");
        std::fs::create_dir(&workspace).expect("create .ac");

        ensure_workspace_gitignore(&workspace).expect("ensure workspace .gitignore");

        let content =
            std::fs::read_to_string(workspace.join(".gitignore")).expect("read .gitignore");
        assert!(
            content.lines().any(|line| line.trim() == ".deleting-*/"),
            "workspace .gitignore must ignore workgroup delete sentinel directories"
        );
        assert!(
            content
                .lines()
                .any(|line| line.trim() == "_loop_*/state.json"),
            "workspace .gitignore must ignore Loop state files"
        );
        assert!(
            content
                .lines()
                .any(|line| line.trim() == "_loop_*/audit.jsonl"),
            "workspace .gitignore must ignore Loop audit files"
        );
        assert!(
            !content
                .lines()
                .any(|line| line.trim() == "_loop_*/config.toml"),
            "workspace .gitignore must not ignore Loop config files"
        );
    }

    #[test]
    fn ensure_workspace_gitignore_appends_delete_sentinels_to_existing_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let workspace = tmp.path().join(".ac");
        std::fs::create_dir(&workspace).expect("create .ac");
        std::fs::write(workspace.join(".gitignore"), "wg-*/\n").expect("write .gitignore");

        ensure_workspace_gitignore(&workspace).expect("ensure workspace .gitignore");

        let content =
            std::fs::read_to_string(workspace.join(".gitignore")).expect("read .gitignore");
        let count = content
            .lines()
            .filter(|line| line.trim() == ".deleting-*/")
            .count();
        assert_eq!(
            count, 1,
            "workspace .gitignore should append the delete sentinel pattern exactly once"
        );
    }

    // ── extract_task_first_line — issue #161 ──

    #[test]
    fn task_no_frontmatter_with_heading() {
        assert_eq!(
            extract_task_first_line("# My Brief\n\nbody"),
            Some("My Brief".to_string())
        );
    }

    #[test]
    fn task_no_frontmatter_plain() {
        assert_eq!(
            extract_task_first_line("My Brief\nbody"),
            Some("My Brief".to_string())
        );
    }

    #[test]
    fn task_with_yaml_frontmatter() {
        let content = "---\ntitle: x\nauthor: y\n---\n# Real Title\nbody";
        assert_eq!(
            extract_task_first_line(content),
            Some("Real Title".to_string())
        );
    }

    #[test]
    fn task_with_frontmatter_then_blank_lines() {
        let content = "---\ntitle: x\n---\n\n\n# Real Title";
        assert_eq!(
            extract_task_first_line(content),
            Some("Real Title".to_string())
        );
    }

    #[test]
    fn task_empty_file() {
        assert_eq!(extract_task_first_line(""), None);
    }

    #[test]
    fn task_only_frontmatter_no_body() {
        assert_eq!(extract_task_first_line("---\nfoo: bar\n---\n"), None);
    }

    #[test]
    fn task_unclosed_frontmatter_returns_none() {
        // Pathological: opener with no closer drains the iterator.
        assert_eq!(extract_task_first_line("---\nfoo: bar\nno closer"), None);
    }

    #[test]
    fn task_leading_blank_no_frontmatter() {
        assert_eq!(
            extract_task_first_line("\n\n# Title"),
            Some("Title".to_string())
        );
    }

    #[test]
    fn task_frontmatter_delimiter_tolerates_whitespace() {
        // `---` lines may carry trailing/leading whitespace from editors.
        let content = "--- \ntitle: x\n  ---  \n# Body";
        assert_eq!(extract_task_first_line(content), Some("Body".to_string()));
    }

    #[test]
    fn task_with_bom_no_frontmatter() {
        // Editors (notably Notepad) save UTF-8 with a BOM. Without the strip,
        // `trim_start_matches("# ")` leaves the BOM on the heading text.
        let content = "\u{FEFF}# My Brief\n\nbody";
        assert_eq!(
            extract_task_first_line(content),
            Some("My Brief".to_string())
        );
    }

    #[test]
    fn task_with_bom_and_frontmatter() {
        // BOM in front of the `---` opener used to make the frontmatter check
        // fail (because `\u{FEFF}---` != `---`), exposing the literal frontmatter.
        let content = "\u{FEFF}---\ntitle: x\n---\n# Real Title\nbody";
        assert_eq!(
            extract_task_first_line(content),
            Some("Real Title".to_string())
        );
    }

    #[test]
    fn task_leading_blanks_before_frontmatter() {
        // Regression: leading blank lines before `---` used to bypass the
        // frontmatter check and leak the literal `---` into the sidebar.
        let content = "\n\n---\ntitle: x\n---\n# Body";
        assert_eq!(extract_task_first_line(content), Some("Body".to_string()));
    }

    #[test]
    fn task_leading_blanks_no_frontmatter() {
        let content = "\n\n# Body line";
        assert_eq!(
            extract_task_first_line(content),
            Some("Body line".to_string())
        );
    }

    /// #280 §3.3 — `note_discovery_timeout` is a monotonic per-path
    /// counter, mirroring `git_watcher::note_timeout` but isolated to the
    /// discovery watcher's path set. Two paths get independent counters.
    #[test]
    fn note_discovery_timeout_counts_per_path() {
        let p1 = "/test-280-3-3/ac-discovery/path-a";
        let p2 = "/test-280-3-3/ac-discovery/path-b";
        assert_eq!(note_discovery_timeout(p1), 1);
        assert_eq!(note_discovery_timeout(p1), 2);
        assert_eq!(note_discovery_timeout(p2), 1);
        assert_eq!(note_discovery_timeout(p1), 3);
    }

    #[test]
    fn invalid_replica_identity_is_not_reported_as_repaired_discovery_identity() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project = temp.path().join("AgentsCommander_ac");
        let workspace = project.join(".ac");
        let team_dir = workspace.join("_team_dev-team");
        let matrix_dir = workspace.join("_agent_tech-lead");
        let wg_dir = workspace.join("wg-2-dev-team");
        let replica_dir = wg_dir.join("__agent_tech-lead");

        for dir in [&team_dir, &matrix_dir, &replica_dir] {
            std::fs::create_dir_all(dir).expect("create fixture dir");
        }
        std::fs::write(
            team_dir.join("config.json"),
            r#"{"agents":["../_agent_tech-lead"],"coordinator":"../_agent_tech-lead"}"#,
        )
        .expect("write team config");
        std::fs::write(
            replica_dir.join("config.json"),
            r#"{"identity":"tech-lead"}"#,
        )
        .expect("write invalid replica config");

        let identity_read = read_replica_config_with_valid_identity(&replica_dir);

        assert!(identity_read.invalid_identity);
        assert!(identity_read.config.is_some());
        assert!(
            identity_read.identity.is_none(),
            "invalid persisted identity must not become a repaired identity path"
        );
        assert_eq!(
            origin_project_for_replica_identity("AgentsCommander_ac", &identity_read),
            None,
            "invalid identity must not be masked as the local origin project"
        );
        assert!(
            crate::config::teams::resolve_wg_coordinator_replica(&workspace, &wg_dir).is_none(),
            "invalid identity must not grant coordinator/root authority"
        );

        let preferred_agent_id = identity_read
            .identity
            .as_ref()
            .and_then(|identity| read_preferred_agent_id(&identity.matrix_dir, &[]));
        assert!(
            preferred_agent_id.is_none(),
            "invalid identity must not be used to read preferred coding agent"
        );
    }
}
