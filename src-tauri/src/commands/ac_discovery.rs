use futures::future::join_all;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use crate::config::settings::SettingsState;
use crate::config::workspace::{
    canonical_workspace_dir_label, existing_workspace_dir, find_workspace_segment,
    has_workspace_dir, workspace_dir_for_project,
};
use crate::pty::manager::{PendingSpawn, PtyManager};
use crate::session::manager::SessionManager;
use crate::session::session::{SessionInfo, SessionRepo, SessionStatus};

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
    /// #552/#580 RFC3339 timestamp of the unified team-idle anchor
    /// `max(last_user_message_at, last_activity_at)` for this coordinator, or
    /// None. Only meaningful when `is_coordinator`. The badge displays and
    /// auto-close triggers on this value (closed time counts). The field keeps
    /// its #552 name (rename to `idleSinceAt` deferred, Decision C). Read from
    /// the persisted CoordinatorClocks store keyed by FQN.
    pub last_user_message_at: Option<String>,
    /// #552 RFC3339 timestamp this coordinator's team was auto-closed for
    /// inactivity, or None. Only meaningful when `is_coordinator`. Drives the
    /// "auto-closed" pill; cleared on reopen. Read from the same store entry.
    pub auto_closed_at: Option<String>,
    /// #588 RFC3339 time this coordinator was manually closed, or None. Only
    /// meaningful when `is_coordinator`. Drives the MANUALLY-CLOSED pill;
    /// cleared on reopen. Read from the same persisted store entry.
    pub manually_closed_at: Option<String>,
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
    pub context_template_updates:
        Vec<crate::config::seeded_context_templates::ContextTemplateUpdate>,
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

fn normalize_projected_path(p: &Path) -> PathBuf {
    crate::path_utils::normalize_windows_verbatim_path_buf(p)
}

fn projected_path_string(p: &Path) -> String {
    crate::path_utils::path_to_string_without_windows_verbatim_prefix(p)
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

/// #280 §3.3 - per-path counter for `detect_git_status` timeouts (#1028 renamed
/// the detection; the counter and its policy are unchanged). Mirrors the
/// `git_watcher.rs` helper but lives in a separate module-local map so the
/// two watchers track their own paths independently (different scan sets).
///
/// #1028 note: this counter stays per-watcher on purpose, unlike
/// `git_watcher::remember_dirty`'s SHARED map. Sharing would make "1st + every 50th"
/// fire at the wrong times for both watchers; that reasoning is about a throttle
/// counter and does not transfer to a fact about a worktree.
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

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiscoveryBranchPayload {
    replica_path: String,
    /// Single-repo shorthand, semantics unchanged: `None` for a multi-repo replica,
    /// so the AcDiscoveryPanel chip keeps hiding the branch there.
    branch: Option<String>,
    /// #943 B2 - per-repo branches, positionally parallel to `repo_paths` below and
    /// therefore to `AcAgentReplica.repo_paths` (`update_replicas_for_project` maps
    /// 1:1 over that vec and never sorts or dedupes it). `None` = branch unknown
    /// (detached HEAD, not a work tree, git missing, detection timed out).
    ///
    /// This is data `poll()` already computed for every replica, multi-repo included,
    /// and then threw away: the single-repo `branch` above was all it kept. Module B2
    /// stops discarding it. Zero new git spawns.
    repo_branches: Vec<Option<String>>,
    /// #943 B2 - the repo paths those branches belong to, byte-identical to the
    /// `AcAgentReplica.repo_paths` strings discovery already sent the frontend.
    ///
    /// Emitted so the consumer can key the merge by path instead of by position. The
    /// positional guarantee holds at emit time, but the consumer stores this payload
    /// and re-reads it against a LATER discovery result: edit a replica's config.json
    /// `repos` (reorder, remove) and for up to one 15s tick the stored branches belong
    /// to the previous repo list. A positional merge would then paint repo A's branch
    /// onto repo B, silently. Matching on the path cannot.
    repo_paths: Vec<String>,
    /// #1028 - per-repo worktree-dirty, positionally parallel to `repo_paths` exactly as
    /// `repo_branches` is, and keyed by path by the consumer for the same reason.
    /// `Some(true)` paints the badge letters red; `None` = never successfully detected
    /// for that path (rendered violet, like clean).
    ///
    /// This rides the feed `repo_branches` already established, which is what covers
    /// DORMANT coordinator rows: Gate A runs for every replica whether or not a session
    /// exists, so a dormant row's dirtiness was already being computed and thrown away.
    /// Gate B below cannot cover them (it is inside `if let Some(session_id)`).
    ///
    /// This struct's `PartialEq` gates the emit, so adding the field is also what makes
    /// a dirty flip re-emit with no gate change.
    repo_dirty: Vec<Option<bool>>,
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
    /// Last-emitted `ac_discovery_branch_updated` payload per replica, gating that
    /// emission (panel UI + the #943 B2 per-repo branches). Holds the whole payload,
    /// not just the single-repo branch it held before B2, so the gate fires on
    /// per-repo drift and on a repo-set change: the same branch text under a
    /// different repo list is not the same event.
    discovery_cache: Mutex<HashMap<String, DiscoveryBranchPayload>>,
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
            .map(|p| projected_path_string(&p))
            .unwrap_or_else(|| projected_path_string(Path::new(project_dir)));

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
                            if let Err(e) = watcher.poll().await {
                                log::warn!("[DiscoveryBranchWatcher] poll failed: {}", e);
                            }
                        }
                    }
                }
            });
        });
    }

    async fn poll(&self) -> Result<(), String> {
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
                // detect_status_with_timeout (2s). Without join_all this was M*N*2s worst case.
                let statuses: Vec<Option<crate::pty::git_watcher::GitStatus>> = join_all(
                    entry
                        .repos
                        .iter()
                        .map(|(_, path)| Self::detect_status_with_timeout(path)),
                )
                .await;

                let refreshed: Vec<SessionRepo> = entry
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
                        ),
                    })
                    .collect();

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
        self.poll_tasks(&entries).await?;
        Ok(())
    }

    /// Gate C: detect TASK.md changes per unique workgroup root and emit
    /// `workgroup_task_updated` on change. Runs on every existing 15s tick
    /// of `poll()`; no new thread, no new cadence.
    async fn poll_tasks(&self, entries: &[ReplicaBranchEntry]) -> Result<(), String> {
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
            let mgr = self.session_manager.read().await;
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

    /// #1028 - detection is the shared `git_watcher::detect_git_status`, which replaced
    /// this watcher's own near-identical `detect_branch` copy. The two copies had already
    /// diverged (one set a ceiling env, this one did not), and both badge writers must
    /// compute `dirty` identically.
    ///
    /// The timeout wrapper stays here rather than merging with GitWatcher's: this one
    /// owns `note_discovery_timeout` and the `[DiscoveryBranchWatcher]` tag, which is
    /// what keeps the two watchers distinguishable in app.log.
    ///
    /// Because `timeout` DROPS the future on expiry, `detect_git_status` never returns
    /// here and cannot remember anything itself: `poll` applies the dirty hold.
    async fn detect_status_with_timeout(
        working_dir: &str,
    ) -> Option<crate::pty::git_watcher::GitStatus> {
        match tokio::time::timeout(
            DETECT_TIMEOUT,
            crate::pty::git_watcher::detect_git_status(working_dir),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => {
                let n = note_discovery_timeout(working_dir);
                // #280 §3.3 — same dampening policy as git_watcher.rs: log
                // first sighting + every 50th. Module tag remains
                // `[DiscoveryBranchWatcher]` so the two watchers stay
                // distinguishable in app.log.
                if n == 1 || n.is_multiple_of(50) {
                    log::warn!(
                        "[DiscoveryBranchWatcher] detect_git_status timed out for {} (>{}s); occurrence={} (logging 1st + every 50th)",
                        working_dir,
                        DETECT_TIMEOUT.as_secs(),
                        n
                    );
                }
                None
            }
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
    coordinator_clocks: State<'_, crate::config::coordinator_clocks::CoordinatorClocksState>,
) -> Result<AcDiscoveryResult, String> {
    let cfg = settings.read().await.clone();
    // #552 snapshot the persisted badge/auto-closed store once so the per-replica
    // scan below never locks inside the loop.
    let clocks_snapshot: std::collections::HashMap<
        String,
        crate::config::coordinator_clocks::ClockEntry,
    > = coordinator_clocks
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .snapshot();
    // Discovery-wide team snapshot — used per-replica for is_coordinator
    // and at the end for refresh_coordinator_flags. Computed once so a
    // single discovery pass presents a coherent coordinator view.
    // Lock-safe: the settings snapshot is owned and the read guard was dropped
    // before any filesystem scan.
    let teams_snapshot = crate::config::teams::discover_teams();
    let call_id = DISCOVERY_CALL_ID.fetch_add(1, Ordering::Relaxed);
    let mut agents: Vec<AcAgentMatrix> = Vec::new();
    let mut teams: Vec<AcTeam> = Vec::new();
    let mut workgroups: Vec<AcWorkgroup> = Vec::new();
    let mut loops: Vec<crate::config::loops::AcLoopSummary> = Vec::new();
    let mut context_template_updates = Vec::new();
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
            let repo_dir_str = projected_path_string(&repo_dir);

            // Opportunistic: ensure gitignore exists for existing projects
            let _ = ensure_workspace_gitignore(&workspace_dir);
            let context_scan = crate::config::seed_manifest::run_blocking_owned({
                let repo_dir = repo_dir.clone();
                let workspace_dir = workspace_dir.clone();
                move || {
                    crate::config::seeded_context_templates::scan_project_context_template_updates(
                        &repo_dir,
                        &workspace_dir,
                    )
                }
            })
            .await;
            match context_scan {
                Ok(Ok(mut updates)) => context_template_updates.append(&mut updates),
                Ok(Err(e)) => log::warn!(
                    "[context-templates] scan failed for {}: {}",
                    workspace_dir.display(),
                    e
                ),
                Err(e) => log::warn!(
                    "[context-templates] blocking scan failed for {}: {}",
                    workspace_dir.display(),
                    e
                ),
            }
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
                        path: projected_path_string(&path),
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
                        .map(|e| projected_path_string(&e.path()));

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
                                        std::fs::canonicalize(&resolved)
                                            .ok()
                                            .map(|p| projected_path_string(&p))
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

                                // #552 look up this coordinator's persisted clocks by FQN
                                // (same shape as agent_fqn_from_path: <project>:<wg>/<agent>).
                                let fqn =
                                    format!("{}:{}/{}", project_folder, dir_name, replica_name);
                                let clock_entry = if is_coordinator {
                                    clocks_snapshot.get(&fqn)
                                } else {
                                    None
                                };
                                // (#580) the badge field carries the unified idle
                                // anchor max(user, activity) (rename deferred,
                                // Decision C) so a dormant/just-loaded row shows the
                                // real accumulated idle (closed time included), not
                                // just the user-message age.
                                let last_user_message_at = clock_entry
                                    .and_then(|e| e.idle_anchor())
                                    .map(|dt| dt.to_rfc3339());
                                let auto_closed_at = clock_entry
                                    .and_then(|e| e.auto_closed_at)
                                    .map(|dt| dt.to_rfc3339());
                                let manually_closed_at = clock_entry
                                    .and_then(|e| e.manually_closed_at)
                                    .map(|dt| dt.to_rfc3339());

                                wg_agents.push(AcAgentReplica {
                                    name: replica_name,
                                    path: projected_path_string(&wg_path),
                                    identity_path,
                                    origin_project,
                                    preferred_agent_id,
                                    current_coding_agent_id,
                                    current_profile,
                                    repo_paths,
                                    repo_branch,
                                    is_coordinator,
                                    last_user_message_at,
                                    auto_closed_at,
                                    manually_closed_at,
                                });
                            }
                        }
                    }
                    wg_agents.sort_by_key(|a| a.name.to_lowercase());

                    workgroups.push(AcWorkgroup {
                        name: dir_name.clone(),
                        path: projected_path_string(&path),
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
    crate::config::seeded_context_templates::dedupe_context_template_updates(
        &mut context_template_updates,
    );

    Ok(AcDiscoveryResult {
        agents,
        teams,
        workgroups,
        loops,
        context_template_updates,
    })
}

/// Check if a folder has a Project AC Root subdirectory.
pub(crate) fn check_project_path_inner(path: &str) -> bool {
    existing_workspace_dir(Path::new(path)).is_some()
}

/// Check if a folder has a Project AC Root subdirectory.
#[tauri::command]
pub async fn check_project_path(path: String) -> Result<bool, String> {
    Ok(check_project_path_inner(&path))
}

/// Ensure the Project AC Root .gitignore exists and contains all required exclusion patterns.
/// Called during project creation, workgroup creation, and opportunistically during discovery.
pub(crate) fn ensure_workspace_gitignore(workspace_dir: &Path) -> Result<(), String> {
    let gitignore_path = workspace_dir.join(".gitignore");
    const SEED_MANIFEST_GITIGNORE_BLOCK: &str = "# AgentsCommander: exclude seed-manifest coordination files.\n/.seed-manifest.lock\n/.seed-manifest.*.tmp\n\n# AgentsCommander: keep the seed publication manifest reviewable.\n!/seed-manifest.toml\n";
    const SEED_MANIFEST_PATTERNS: [&str; 3] = [
        "/.seed-manifest.lock",
        "/.seed-manifest.*.tmp",
        "!/seed-manifest.toml",
    ];

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
        (
            "/.team-config-write.lock",
            "# AgentsCommander: exclude team-config coordination files.",
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
        if !SEED_MANIFEST_PATTERNS
            .iter()
            .all(|pattern| content.lines().any(|line| line.trim() == *pattern))
        {
            additions.push('\n');
            additions.push_str(SEED_MANIFEST_GITIGNORE_BLOCK);
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
        content.push_str(SEED_MANIFEST_GITIGNORE_BLOCK);
        std::fs::write(&gitignore_path, content)
            .map_err(|e| format!("Failed to create Project AC Root .gitignore: {}", e))?;
    }

    Ok(())
}

/// Create a canonical .ac/ directory inside the given path.
#[tauri::command]
pub async fn create_ac_project(path: String) -> Result<(), String> {
    crate::config::seed_manifest::run_blocking_owned(move || {
        create_ac_project_impl(&path, |workspace_dir| {
            crate::config::session_context::create_default_context_templates(workspace_dir)
        })
    })
    .await
    .map_err(|error| format!("AC project blocking preparation failed: {error}"))?
}

fn create_ac_project_impl<F>(path: &str, create_context_templates: F) -> Result<(), String>
where
    F: FnOnce(&Path) -> Result<(), String>,
{
    create_ac_project_impl_with_hook(path, create_context_templates, |_, _| {})
}

fn create_ac_project_impl_with_hook<F, H>(
    path: &str,
    create_context_templates: F,
    after_create_before_gate: H,
) -> Result<(), String>
where
    F: FnOnce(&Path) -> Result<(), String>,
    H: FnOnce(&Path, bool),
{
    let project_dir =
        crate::config::projects::absolutise(path).map_err(|error| error.to_string())?;
    match std::fs::symlink_metadata(&project_dir) {
        Ok(metadata) if !metadata.is_dir() => {
            return Err(format!(
                "Project path is not a directory: {}",
                project_dir.display()
            ))
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(&project_dir).map_err(|error| {
                format!(
                    "Failed to create project directory {}: {}",
                    project_dir.display(),
                    error
                )
            })?;
        }
        Err(error) => {
            return Err(format!(
                "Failed to inspect project directory {}: {}",
                project_dir.display(),
                error
            ))
        }
    }

    let workspace_dir = workspace_dir_for_project(&project_dir);
    let created = match std::fs::create_dir(&workspace_dir) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(error) => {
            return Err(format!(
                "Failed to create {} directory: {}",
                canonical_workspace_dir_label(),
                error
            ))
        }
    };
    let pinned_project = crate::config::seed_manifest::PinnedDirectory::open(&project_dir)
        .map_err(|error| {
            format!(
                "Failed to pin project directory {}: {}",
                project_dir.display(),
                error
            )
        })?;
    let pinned_workspace = crate::config::seed_manifest::PinnedDirectory::open(&workspace_dir)
        .map_err(|error| {
            format!(
                "Failed to pin {} directory {}: {}",
                canonical_workspace_dir_label(),
                workspace_dir.display(),
                error
            )
        })?;
    after_create_before_gate(&workspace_dir, created);
    pinned_project.revalidate().map_err(|error| {
        format!(
            "AC project setup at {} changed before gate acquisition: {}; retry the operation",
            project_dir.display(),
            error
        )
    })?;
    pinned_workspace.revalidate().map_err(|error| {
        format!(
            "AC project setup at {} changed before gate acquisition: {}; retry the operation",
            project_dir.display(),
            error
        )
    })?;

    let guard = crate::config::seed_manifest::ProjectSeedManifestGuard::acquire(&project_dir)
        .map_err(|error| {
            format!(
                "AC project setup at {} changed or is unavailable: {}; retry the operation",
                project_dir.display(),
                error
            )
        })?;
    pinned_project.revalidate().map_err(|error| {
        format!(
            "AC project setup at {} changed before gate acquisition: {}; retry the operation",
            project_dir.display(),
            error
        )
    })?;
    pinned_workspace
        .revalidate_at(guard.ac_root())
        .map_err(|error| {
            format!(
                "AC project setup at {} changed before gate acquisition: {}; retry the operation",
                project_dir.display(),
                error
            )
        })?;
    guard.revalidate_owner().map_err(|error| {
        format!(
            "AC project setup at {} changed while preparing: {}; retry the operation",
            project_dir.display(),
            error
        )
    })?;
    if created {
        ensure_workspace_gitignore(&workspace_dir)
            .and_then(|_| create_context_templates(&workspace_dir))?;
    } else {
        ensure_workspace_gitignore(&workspace_dir)?;
    }
    guard.revalidate_owner().map_err(|error| {
        format!(
            "AC project setup at {} changed while preparing: {}; retry the operation",
            project_dir.display(),
            error
        )
    })?;
    guard.release();
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
    coordinator_clocks: State<'_, crate::config::coordinator_clocks::CoordinatorClocksState>,
) -> Result<AcDiscoveryResult, String> {
    discover_project_inner(
        &app,
        session_mgr.inner(),
        &path,
        settings.inner(),
        branch_watcher.inner(),
        coordinator_clocks.inner(),
    )
    .await
}

pub(crate) async fn discover_project_inner(
    app: &AppHandle,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    path: &str,
    settings: &SettingsState,
    branch_watcher: &Arc<DiscoveryBranchWatcher>,
    coordinator_clocks: &crate::config::coordinator_clocks::CoordinatorClocksState,
) -> Result<AcDiscoveryResult, String> {
    let base = Path::new(path);
    if !base.is_dir() {
        return Err(format!("Path is not a directory: {}", path));
    }

    let cfg = settings.read().await.clone();
    // #552 snapshot the persisted badge/auto-closed store once (see discover_ac_agents).
    let clocks_snapshot: std::collections::HashMap<
        String,
        crate::config::coordinator_clocks::ClockEntry,
    > = coordinator_clocks
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .snapshot();

    let Some(workspace_dir) = existing_workspace_dir(base) else {
        return Ok(AcDiscoveryResult {
            agents: vec![],
            teams: vec![],
            workgroups: vec![],
            loops: vec![],
            context_template_updates: vec![],
        });
    };

    // Opportunistic: ensure gitignore protects workgroup clones
    let _ = ensure_workspace_gitignore(&workspace_dir);
    let context_scan = crate::config::seed_manifest::run_blocking_owned({
        let base = base.to_path_buf();
        let workspace_dir = workspace_dir.clone();
        move || {
            crate::config::seeded_context_templates::scan_project_context_template_updates(
                &base,
                &workspace_dir,
            )
        }
    })
    .await;
    let mut context_template_updates = match context_scan {
        Ok(Ok(updates)) => updates,
        Ok(Err(e)) => {
            log::warn!(
                "[context-templates] scan failed for {}: {}",
                workspace_dir.display(),
                e
            );
            Vec::new()
        }
        Err(e) => {
            log::warn!(
                "[context-templates] blocking scan failed for {}: {}",
                workspace_dir.display(),
                e
            );
            Vec::new()
        }
    };

    // Discovery-wide team snapshot — see discover_ac_agents for rationale.
    // Lock-safe: the settings snapshot is owned and the read guard was dropped
    // before any filesystem scan.
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
                path: projected_path_string(&entry_path),
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
                .map(|e| projected_path_string(&e.path()));

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
                                std::fs::canonicalize(&resolved)
                                    .ok()
                                    .map(|p| projected_path_string(&p))
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

                        // #552 look up this coordinator's persisted clocks by FQN
                        // (same shape as agent_fqn_from_path: <project>:<wg>/<agent>).
                        let fqn = format!("{}:{}/{}", project_folder, dir_name, replica_name);
                        let clock_entry = if is_coordinator {
                            clocks_snapshot.get(&fqn)
                        } else {
                            None
                        };
                        // (#580) the badge field carries the unified idle anchor
                        // max(user, activity) (rename deferred, Decision C) so a
                        // dormant/just-loaded row shows the real accumulated idle
                        // (closed time included), not just the user-message age.
                        let last_user_message_at = clock_entry
                            .and_then(|e| e.idle_anchor())
                            .map(|dt| dt.to_rfc3339());
                        let auto_closed_at = clock_entry
                            .and_then(|e| e.auto_closed_at)
                            .map(|dt| dt.to_rfc3339());
                        let manually_closed_at = clock_entry
                            .and_then(|e| e.manually_closed_at)
                            .map(|dt| dt.to_rfc3339());

                        wg_agents.push(AcAgentReplica {
                            name: replica_name,
                            path: projected_path_string(&wg_path),
                            identity_path,
                            origin_project,
                            preferred_agent_id,
                            current_coding_agent_id,
                            current_profile,
                            repo_paths,
                            repo_branch,
                            is_coordinator,
                            last_user_message_at,
                            auto_closed_at,
                            manually_closed_at,
                        });
                    }
                }
            }
            wg_agents.sort_by_key(|a| a.name.to_lowercase());

            workgroups.push(AcWorkgroup {
                name: dir_name.clone(),
                path: projected_path_string(&entry_path),
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
    branch_watcher.update_replicas_for_project(path, &workgroups);

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
        let coordinator_session_mgr = Arc::clone(session_mgr);
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
    crate::config::seeded_context_templates::dedupe_context_template_updates(
        &mut context_template_updates,
    );

    Ok(AcDiscoveryResult {
        agents,
        teams,
        workgroups,
        loops,
        context_template_updates,
    })
}

#[tauri::command]
pub async fn keep_custom_context_template(
    path: String,
    filename: String,
    current_file_sha256: String,
    current_default_sha256: String,
) -> Result<(), String> {
    let workspace_dir = existing_workspace_dir(Path::new(&path))
        .ok_or_else(|| format!("Project AC Root not found for {}", path))?;
    crate::config::seeded_context_templates::dismiss_context_template_update(
        &workspace_dir,
        &filename,
        &current_file_sha256,
        &current_default_sha256,
    )
}

#[tauri::command]
pub async fn overwrite_context_template_with_default(
    path: String,
    filename: String,
    current_file_sha256: String,
    current_default_sha256: String,
) -> Result<crate::config::seeded_context_templates::ContextTemplateOverwriteResult, String> {
    let workspace_dir = existing_workspace_dir(Path::new(&path))
        .ok_or_else(|| format!("Project AC Root not found for {}", path))?;
    crate::config::seed_manifest::run_blocking_owned(move || {
        crate::config::seeded_context_templates::overwrite_context_template_with_default(
            &workspace_dir,
            &filename,
            &current_file_sha256,
            &current_default_sha256,
        )
    })
    .await
    .map_err(|error| format!("context template overwrite blocking task failed: {error}"))?
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

async fn mutate_project_paths_with_settings_path<T>(
    settings: &SettingsState,
    settings_path: Option<&Path>,
    mutate: impl FnOnce(&mut crate::config::settings::AppSettings) -> Result<T, String>,
) -> Result<T, String> {
    let mut s = settings.write().await;
    // #778: reconcile project_paths from disk BEFORE the upsert so a concurrent
    // CLI append is folded in, not clobbered, then write the deliberate list
    // verbatim (project_paths is disk-authoritative under Design S). Aborts on a
    // non-NotFound read error (G2) rather than registering against a stale list.
    if let Some(path) = settings_path {
        crate::config::settings::refresh_project_paths_from_path(&mut s, path)?;
    } else {
        crate::config::settings::refresh_project_paths_from_disk(&mut s)?;
    }
    let result = mutate(&mut s)?;
    let snapshot = s.clone();
    if let Some(path) = settings_path {
        crate::config::settings::save_settings_with_project_paths_to_path(&snapshot, path)?;
    } else {
        crate::config::settings::save_settings_with_project_paths(&snapshot)?;
    }
    drop(s); // explicit; lock released AFTER the disk write completes
    Ok(result)
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ArchiveChangeReason {
    Archive,
    Unarchive,
    AutoUnarchive,
    Open,
    Remove,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectArchiveChanged {
    pub path: String,
    pub folder_name: String,
    pub archived: bool,
    pub reason: ArchiveChangeReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
}

fn project_folder_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path)
        .to_string()
}

pub(crate) fn broadcast_project_archive_changed<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    path: &str,
    archived: bool,
    reason: ArchiveChangeReason,
    session_name: Option<&str>,
) {
    let payload = serde_json::json!(ProjectArchiveChanged {
        path: path.to_string(),
        folder_name: project_folder_name(path),
        archived,
        reason,
        session_name: session_name.map(|name| name.to_string()),
    });
    crate::web::commands::broadcast_all_r(app, "project_archive_changed", &payload);
}

/// Validate an existing AC project at `path` and register it in
/// `settings.project_paths`.
pub(crate) async fn open_project_inner<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    settings: &SettingsState,
    path: &str,
) -> Result<crate::config::projects::ProjectRegistration, String> {
    open_project_inner_with_event_and_settings_path(app, settings, path, None).await
}

async fn open_project_inner_with_event_and_settings_path<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    settings: &SettingsState,
    path: &str,
    settings_path: Option<&Path>,
) -> Result<crate::config::projects::ProjectRegistration, String> {
    let (result, archived_changed) =
        open_project_inner_with_settings_path(settings, path, settings_path).await?;
    if archived_changed {
        broadcast_project_archive_changed(
            app,
            &result.path,
            false,
            ArchiveChangeReason::Open,
            None,
        );
    }
    Ok(result)
}

async fn open_project_inner_with_settings_path(
    settings: &SettingsState,
    path: &str,
    settings_path: Option<&Path>,
) -> Result<(crate::config::projects::ProjectRegistration, bool), String> {
    mutate_project_paths_with_settings_path(settings, settings_path, |s| {
        let before = s.archived_project_paths.clone();
        let result = crate::config::projects::register_existing_project(s, path)
            .map_err(|e| e.to_string())?;
        Ok((result, before != s.archived_project_paths))
    })
    .await
}

#[tauri::command]
pub async fn open_project(
    app: tauri::AppHandle,
    settings: State<'_, SettingsState>,
    path: String,
) -> Result<crate::config::projects::ProjectRegistration, String> {
    open_project_inner(&app, settings.inner(), &path).await
}

/// Ensure an AC project at `path` (creating `.ac/` if missing) and
/// register it in `settings.project_paths`. Mirrors the ActionBar "New
/// Project" flow at `src/sidebar/components/ActionBar.tsx:58-71`.
#[tauri::command]
pub async fn new_project(
    app: tauri::AppHandle,
    settings: State<'_, SettingsState>,
    path: String,
) -> Result<crate::config::projects::ProjectRegistration, String> {
    new_project_inner(&app, settings.inner(), &path).await
}

pub(crate) async fn new_project_inner<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    settings: &SettingsState,
    path: &str,
) -> Result<crate::config::projects::ProjectRegistration, String> {
    new_project_inner_with_event_and_settings_path(app, settings, path, None).await
}

async fn new_project_inner_with_event_and_settings_path<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    settings: &SettingsState,
    path: &str,
    settings_path: Option<&Path>,
) -> Result<crate::config::projects::ProjectRegistration, String> {
    let (result, archived_changed) =
        new_project_inner_with_settings_path(settings, path, settings_path).await?;
    if archived_changed {
        broadcast_project_archive_changed(
            app,
            &result.path,
            false,
            ArchiveChangeReason::Open,
            None,
        );
    }
    Ok(result)
}

async fn new_project_inner_with_settings_path(
    settings: &SettingsState,
    path: &str,
    settings_path: Option<&Path>,
) -> Result<(crate::config::projects::ProjectRegistration, bool), String> {
    new_project_inner_with_settings_path_and_hooks(
        settings,
        path,
        settings_path,
        NewProjectTransactionHooks::default(),
    )
    .await
}

#[cfg(test)]
type NewProjectFinalRevalidationHook = std::sync::Arc<dyn Fn(&Path) + Send + Sync>;

#[derive(Clone, Default)]
struct NewProjectTransactionHooks {
    #[cfg(test)]
    fail_settings_save: bool,
    #[cfg(test)]
    before_final_revalidation: Option<NewProjectFinalRevalidationHook>,
}

impl NewProjectTransactionHooks {
    fn save_settings(
        &self,
        settings: &crate::config::settings::AppSettings,
        settings_path: Option<&Path>,
    ) -> Result<(), String> {
        #[cfg(test)]
        if self.fail_settings_save {
            return Err("injected new-project settings save failure".to_string());
        }
        if let Some(path) = settings_path {
            crate::config::settings::save_settings_with_project_paths_to_path(settings, path)
        } else {
            crate::config::settings::save_settings_with_project_paths(settings)
        }
    }

    fn before_final_revalidation(&self, workspace_dir: &Path) {
        #[cfg(test)]
        if let Some(hook) = &self.before_final_revalidation {
            hook(workspace_dir);
        }
        #[cfg(not(test))]
        let _ = workspace_dir;
    }
}

async fn new_project_inner_with_settings_path_and_hooks(
    settings: &SettingsState,
    path: &str,
    settings_path: Option<&Path>,
    hooks: NewProjectTransactionHooks,
) -> Result<(crate::config::projects::ProjectRegistration, bool), String> {
    let path = path.to_string();
    let settings = std::sync::Arc::clone(settings);
    let settings_path = settings_path.map(Path::to_path_buf);
    crate::config::seed_manifest::run_blocking_owned(move || {
        let prepared = crate::config::projects::prepare_new_project(&path)
            .map_err(|error| error.to_string())?;
        let mut current = settings.blocking_write();
        if let Some(path) = settings_path.as_deref() {
            crate::config::settings::refresh_project_paths_from_path(&mut current, path)?;
        } else {
            crate::config::settings::refresh_project_paths_from_disk(&mut current)?;
        }

        let before_settings = current.clone();
        let before_archived = current.archived_project_paths.clone();
        let result = crate::config::projects::commit_prepared_new_project(&mut current, &prepared);
        if let Err(error) = prepared.revalidate() {
            *current = before_settings;
            return Err(error.to_string());
        }

        let snapshot = current.clone();
        // Preserve the established save-failure contract: once the deliberate
        // upsert is in live memory, a disk-save error is returned without
        // reverting that in-memory list.
        hooks.save_settings(&snapshot, settings_path.as_deref())?;
        hooks.before_final_revalidation(prepared.workspace_dir());
        if let Err(error) = prepared.revalidate() {
            *current = before_settings.clone();
            let rollback = hooks.save_settings(&before_settings, settings_path.as_deref());
            return Err(match rollback {
                Ok(()) => error.to_string(),
                Err(rollback_error) => format!(
                    "{}; failed to roll back project registration settings: {}",
                    error, rollback_error
                ),
            });
        }
        let archived_changed = before_archived != current.archived_project_paths;
        drop(current);
        prepared.release();
        Ok((result, archived_changed))
    })
    .await
    .map_err(|error| format!("new project blocking transaction failed: {error}"))?
}

/// #778 Part 3: targeted project removal. `project_paths` is disk-authoritative
/// under Design S, so a removal cannot ride a whole-object settings save (the
/// default `save_settings` preserves the on-disk list and would ignore the
/// removal, and without a baseline the backend cannot tell a removal from a stale
/// passthrough). So removal is its own command: reconcile the list from disk
/// (G2: abort on a non-NotFound read error), drop the one entry whose
/// `normalize_for_compare` key matches `path`, re-derive the head, and write the
/// deliberate list verbatim. Removing against the FRESH disk list means a
/// CLI-appended entry the sidebar never showed is preserved, not dropped.
pub(crate) async fn remove_project_inner<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    settings: &SettingsState,
    path: &str,
) -> Result<(), String> {
    let archived_changed = remove_project_inner_with_settings_path(settings, path, None).await?;
    if archived_changed {
        broadcast_project_archive_changed(app, path, false, ArchiveChangeReason::Remove, None);
    }
    Ok(())
}

async fn remove_project_inner_with_settings_path(
    settings: &SettingsState,
    path: &str,
    settings_path: Option<&Path>,
) -> Result<bool, String> {
    mutate_project_paths_with_settings_path(settings, settings_path, |s| {
        let before = s.archived_project_paths.clone();
        crate::config::projects::remove_project_path(s, path);
        Ok(before != s.archived_project_paths)
    })
    .await
}

#[tauri::command]
pub async fn remove_project(
    app: tauri::AppHandle,
    settings: State<'_, SettingsState>,
    path: String,
) -> Result<(), String> {
    remove_project_inner(&app, settings.inner(), &path).await
}

pub(crate) struct ArchiveSnapshot {
    sessions: Vec<(SessionInfo, bool)>,
    pending: Vec<PendingSpawn>,
}

async fn snapshot_archive_state(
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
) -> ArchiveSnapshot {
    let sessions = {
        let mgr = session_mgr.read().await;
        mgr.list_sessions().await
    };
    let session_ids = sessions
        .iter()
        .map(|s| Uuid::parse_str(&s.id).ok())
        .collect::<Vec<_>>();
    let ids = session_ids.iter().flatten().copied().collect::<Vec<_>>();
    let (pending, live) = {
        let pty = pty_mgr.lock().unwrap_or_else(|e| e.into_inner());
        pty.archive_liveness(&ids)
    };
    let live_by_id = ids.into_iter().zip(live).collect::<HashMap<_, _>>();
    let sessions = sessions
        .into_iter()
        .zip(session_ids)
        .map(|(s, id)| {
            let has_pty = id
                .and_then(|id| live_by_id.get(&id).copied())
                .unwrap_or(false);
            (s, has_pty)
        })
        .collect();
    ArchiveSnapshot { sessions, pending }
}

fn session_is_live(status: &SessionStatus, has_pty: bool) -> bool {
    match status {
        SessionStatus::Exited(_) => false,
        SessionStatus::Active | SessionStatus::Running | SessionStatus::Idle => has_pty,
    }
}

fn archive_scope_contains(cwd: &str, project_path: &str) -> bool {
    let scope = [project_path.to_string()];
    crate::config::sessions_persistence::working_directory_under_any_project_path(cwd, &scope)
}

fn is_root_agent_cwd(cwd: &str) -> bool {
    crate::config::root_agent::is_root_agent_dir_name(cwd)
        || crate::config::root_agent::is_root_agent_path(cwd)
}

pub(crate) fn archive_blockers(snapshot: &ArchiveSnapshot, project_path: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut blockers = Vec::new();

    for pending in &snapshot.pending {
        if is_root_agent_cwd(&pending.cwd) {
            continue;
        }
        if !archive_scope_contains(&pending.cwd, project_path) {
            continue;
        }
        if seen.insert(pending.label.clone()) {
            blockers.push(pending.label.clone());
        }
    }

    for (session, _) in snapshot
        .sessions
        .iter()
        .filter(|(s, _)| !(s.is_root_agent || is_root_agent_cwd(&s.working_directory)))
        .filter(|(s, has_pty)| session_is_live(&s.status, *has_pty))
        .filter(|(s, _)| archive_scope_contains(&s.working_directory, project_path))
    {
        if seen.insert(session.name.clone()) {
            blockers.push(session.name.clone());
        }
    }

    blockers
}

fn archive_blocked_message(blockers: &[String]) -> String {
    const MAX_NAMED: usize = 3;
    let named = blockers
        .iter()
        .take(MAX_NAMED)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    let overflow = blockers.len().saturating_sub(MAX_NAMED);
    if overflow > 0 {
        format!(
            "Cannot archive: {} open session(s) in this project ({}, and {} more). Close them first.",
            blockers.len(),
            named,
            overflow
        )
    } else {
        format!(
            "Cannot archive: {} open session(s) in this project ({}). Close them first.",
            blockers.len(),
            named
        )
    }
}

pub(crate) async fn archive_project_inner<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    settings: &SettingsState,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    path: &str,
) -> Result<(), String> {
    archive_project_inner_with_settings_path(app, settings, session_mgr, pty_mgr, path, None).await
}

async fn archive_project_inner_with_settings_path<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    settings: &SettingsState,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    path: &str,
    settings_path: Option<&Path>,
) -> Result<(), String> {
    let path = crate::config::projects::absolutise(path)
        .map_err(|e| e.to_string())?
        .to_string_lossy()
        .into_owned();
    let before = snapshot_archive_state(session_mgr, pty_mgr).await;
    let blockers = archive_blockers(&before, &path);
    if !blockers.is_empty() {
        return Err(archive_blocked_message(&blockers));
    }

    mutate_project_paths_with_settings_path(settings, settings_path, |s| {
        crate::config::projects::archive_project_path(s, &path);
        Ok(())
    })
    .await?;

    let after = snapshot_archive_state(session_mgr, pty_mgr).await;
    let late = archive_blockers(&after, &path);
    if late.is_empty() {
        broadcast_project_archive_changed(app, &path, true, ArchiveChangeReason::Archive, None);
        return Ok(());
    }

    let restored: Result<bool, String> =
        mutate_project_paths_with_settings_path(settings, settings_path, |s| {
            let key = crate::config::projects::normalize_for_compare(&path);
            let still_archived = s
                .archived_project_paths
                .iter()
                .any(|p| crate::config::projects::normalize_for_compare(p) == key);
            if !still_archived {
                return Ok(false);
            }
            Ok(crate::config::projects::unarchive_project_path(s, &path))
        })
        .await;

    match restored {
        Ok(false) => Err(archive_blocked_message(&late)),
        Ok(true) => {
            broadcast_project_archive_changed(
                app,
                &path,
                false,
                ArchiveChangeReason::Unarchive,
                None,
            );
            Err(archive_blocked_message(&late))
        }
        Err(e) => {
            log::error!(
                "[archive] '{}' has {} live session(s) but could not be restored: {}",
                path,
                late.len(),
                e
            );
            Err(format!(
                "{} The project could not be restored automatically ({}). Open Archived Projects to restore it.",
                archive_blocked_message(&late),
                e
            ))
        }
    }
}

#[tauri::command]
pub async fn archive_project(
    app: tauri::AppHandle,
    settings: State<'_, SettingsState>,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    path: String,
) -> Result<(), String> {
    archive_project_inner(
        &app,
        settings.inner(),
        session_mgr.inner(),
        pty_mgr.inner(),
        &path,
    )
    .await
}

pub(crate) async fn unarchive_project_inner<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    settings: &SettingsState,
    path: &str,
) -> Result<crate::config::projects::ProjectRegistration, String> {
    let result = mutate_project_paths_with_settings_path(settings, None, |s| {
        crate::config::projects::register_existing_project(s, path).map_err(|e| e.to_string())
    })
    .await?;
    broadcast_project_archive_changed(
        app,
        &result.path,
        false,
        ArchiveChangeReason::Unarchive,
        None,
    );
    Ok(result)
}

#[tauri::command]
pub async fn unarchive_project(
    app: tauri::AppHandle,
    settings: State<'_, SettingsState>,
    path: String,
) -> Result<crate::config::projects::ProjectRegistration, String> {
    unarchive_project_inner(&app, settings.inner(), &path).await
}

pub(crate) async fn list_archived_projects_inner(
    settings: &SettingsState,
) -> Result<Vec<crate::config::projects::ArchivedProject>, String> {
    let paths = {
        let s = settings.read().await;
        s.archived_project_paths.clone()
    };
    tokio::task::spawn_blocking(move || {
        crate::config::projects::archived_projects_from_paths(&paths)
    })
    .await
    .map_err(|e| format!("archived project stat task failed: {}", e))
}

#[tauri::command]
pub async fn list_archived_projects(
    settings: State<'_, SettingsState>,
) -> Result<Vec<crate::config::projects::ArchivedProject>, String> {
    list_archived_projects_inner(settings.inner()).await
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
    use crate::config::settings::AppSettings;
    use crate::pty::git_watcher::GitWatcher;
    use crate::pty::idle_detector::IdleDetector;
    use crate::web::broadcast::{WsBroadcaster, WsOutMsg};
    use serde_json::{json, Value};

    fn archive_test_session(
        name: &str,
        working_directory: &str,
        status: SessionStatus,
        is_root_agent: bool,
    ) -> SessionInfo {
        SessionInfo {
            id: Uuid::new_v4().to_string(),
            name: name.to_string(),
            shell: "shell".to_string(),
            shell_args: Vec::new(),
            backend_kind: crate::pty::backend::SessionBackendKind::LocalProcess,
            effective_shell_args: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            working_directory: working_directory.to_string(),
            status,
            waiting_for_input: false,
            communication: None,
            pending_review: false,
            last_prompt: None,
            agent_id: None,
            agent_label: None,
            git_repos: Vec::new(),
            workgroup_task: None,
            is_coordinator: false,
            is_root_agent,
            token: "token".to_string(),
            agent_kind: None,
            requested_profile: None,
            effective_profile: None,
            profile_fallback_chain: Vec::new(),
            profile_fallback_applied: false,
            effective_codex_home: None,
            profile_content_hash: None,
            profile_outdated: false,
            telegram_bot_id: None,
            was_detached: false,
            detached_geometry: None,
            start_fresh_on_restore: false,
        }
    }

    fn archive_scope() -> (tempfile::TempDir, String, String, String) {
        let temp = tempfile::tempdir().expect("tempdir");
        let project = temp.path().join("project");
        let agent = project.join(".ac").join("wg-1").join("__agent_dev");
        let other = temp
            .path()
            .join("other")
            .join(".ac")
            .join("wg-1")
            .join("__agent_other");
        std::fs::create_dir_all(&agent).expect("agent");
        std::fs::create_dir_all(&other).expect("other");
        (
            temp,
            project.to_string_lossy().to_string(),
            agent.to_string_lossy().to_string(),
            other.to_string_lossy().to_string(),
        )
    }

    type ArchiveCommandApp = (
        tauri::App,
        SettingsState,
        tokio::sync::mpsc::Receiver<WsOutMsg>,
        Arc<tokio::sync::RwLock<SessionManager>>,
        Arc<Mutex<PtyManager>>,
    );

    fn archive_command_app(settings: AppSettings) -> ArchiveCommandApp {
        let settings_state: SettingsState = Arc::new(tokio::sync::RwLock::new(settings));
        let broadcaster = WsBroadcaster::new();
        let receiver = broadcaster.subscribe();
        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let app = tauri::Builder::default()
            .any_thread()
            .manage(settings_state.clone())
            .manage(broadcaster.clone())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build archive command test app");
        let app_handle = app.handle().clone();
        let idle_detector = IdleDetector::new(|_| {}, |_| {});
        let git_watcher = GitWatcher::new(Arc::clone(&session_mgr), app_handle);
        let pty_mgr = Arc::new(Mutex::new(PtyManager::new(
            Arc::new(Mutex::new(HashMap::new())),
            idle_detector,
            git_watcher,
            Some(broadcaster),
            None,
        )));
        (app, settings_state, receiver, session_mgr, pty_mgr)
    }

    #[derive(Default)]
    struct FlippingLiveBackend {
        calls: std::sync::atomic::AtomicUsize,
    }

    impl crate::pty::backend::PtyBackend for FlippingLiveBackend {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn spawn(
            &self,
            _spec: crate::pty::backend::BackendSpawnSpec,
        ) -> futures::future::BoxFuture<'_, Result<(), crate::errors::AppError>> {
            Box::pin(async { Ok(()) })
        }

        fn write(&self, _id: Uuid, _data: &[u8]) -> Result<(), crate::errors::AppError> {
            Ok(())
        }

        fn resize(&self, _id: Uuid, _cols: u16, _rows: u16) -> Result<(), crate::errors::AppError> {
            Ok(())
        }

        fn kill(&self, _id: Uuid) -> Result<(), crate::errors::AppError> {
            Ok(())
        }

        fn has_session(&self, _id: Uuid) -> bool {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0
        }

        fn get_screen_snapshot(&self, _id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
            None
        }

        fn get_pty_size(&self, _id: Uuid) -> Option<(u16, u16)> {
            None
        }

        fn get_screen_rows(&self, _id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
            crate::pty::context_scrape::ScreenRowsRead::SessionOver
        }

        fn register_response_watcher(
            &self,
            _session_id: Uuid,
            _request_id: String,
            _response_dir: PathBuf,
        ) {
        }

        fn terminate_job_for_session(&self, _id: Uuid) -> bool {
            false
        }

        fn kill_all_jobs(&self) -> (usize, usize) {
            (0, 0)
        }
    }

    fn recv_ws_event(rx: &mut tokio::sync::mpsc::Receiver<WsOutMsg>) -> Value {
        match rx.try_recv().expect("broadcast event") {
            WsOutMsg::Text(text) => serde_json::from_str::<Value>(&text).expect("parse event"),
            other => panic!("expected text event, got {other:?}"),
        }
    }

    /// CWD is process-wide; serialize tests that intentionally use relative paths.
    struct CwdGuard(PathBuf);
    impl Drop for CwdGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.0);
        }
    }

    static CWD_MUTEX: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();

    async fn cwd_lock() -> tokio::sync::MutexGuard<'static, ()> {
        CWD_MUTEX
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await
    }

    fn archive_snapshot(
        sessions: Vec<(SessionInfo, bool)>,
        pending: Vec<PendingSpawn>,
    ) -> ArchiveSnapshot {
        ArchiveSnapshot { sessions, pending }
    }

    #[test]
    fn archive_blockers_names_live_pty_sessions_under_project() {
        let (_temp, project, agent, _) = archive_scope();
        let snapshot = archive_snapshot(
            vec![(
                archive_test_session("live", &agent, SessionStatus::Running, false),
                true,
            )],
            Vec::new(),
        );

        assert_eq!(archive_blockers(&snapshot, &project), vec!["live"]);
    }

    #[test]
    fn archive_blockers_ignores_exited_session_under_project() {
        let (_temp, project, agent, _) = archive_scope();
        let snapshot = archive_snapshot(
            vec![(
                archive_test_session("done", &agent, SessionStatus::Exited(0), false),
                true,
            )],
            Vec::new(),
        );

        assert!(archive_blockers(&snapshot, &project).is_empty());
    }

    #[test]
    fn archive_blockers_ignores_running_session_with_no_pty() {
        let (_temp, project, agent, _) = archive_scope();
        let snapshot = archive_snapshot(
            vec![(
                archive_test_session("phantom", &agent, SessionStatus::Running, false),
                false,
            )],
            Vec::new(),
        );

        assert!(archive_blockers(&snapshot, &project).is_empty());
    }

    #[test]
    fn archive_blockers_ignores_root_agent_session() {
        let (_temp, project, agent, _) = archive_scope();
        let snapshot = archive_snapshot(
            vec![(
                archive_test_session("root", &agent, SessionStatus::Running, true),
                true,
            )],
            Vec::new(),
        );

        assert!(archive_blockers(&snapshot, &project).is_empty());
    }

    #[test]
    fn archive_blockers_ignores_root_agent_by_dir_name_when_flag_is_false() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project = temp.path().join("project");
        let root_dir = project
            .join(".ac")
            .join(crate::config::root_agent::ROOT_AGENT_DIR_NAME);
        std::fs::create_dir_all(&root_dir).expect("root dir");
        let snapshot = archive_snapshot(
            vec![(
                archive_test_session(
                    "root",
                    &root_dir.to_string_lossy(),
                    SessionStatus::Running,
                    false,
                ),
                true,
            )],
            Vec::new(),
        );

        assert!(archive_blockers(&snapshot, &project.to_string_lossy()).is_empty());
    }

    #[test]
    fn archive_blockers_ignores_sessions_in_other_projects() {
        let (_temp, project, _, other) = archive_scope();
        let snapshot = archive_snapshot(
            vec![(
                archive_test_session("other", &other, SessionStatus::Running, false),
                true,
            )],
            Vec::new(),
        );

        assert!(archive_blockers(&snapshot, &project).is_empty());
    }

    #[test]
    fn archive_blocked_message_caps_named_sessions_at_three() {
        let names = vec![
            "one".to_string(),
            "two".to_string(),
            "three".to_string(),
            "four".to_string(),
        ];

        let message = archive_blocked_message(&names);

        assert!(message.contains("one, two, three, and 1 more"), "{message}");
        assert!(!message.contains("four"), "{message}");
    }

    #[test]
    fn archive_blockers_names_pending_spawn_under_project() {
        let (_temp, project, agent, _) = archive_scope();
        let snapshot = archive_snapshot(
            Vec::new(),
            vec![PendingSpawn {
                cwd: agent,
                label: "spawning".to_string(),
            }],
        );

        assert_eq!(archive_blockers(&snapshot, &project), vec!["spawning"]);
    }

    #[test]
    fn archive_blockers_ignores_pending_root_agent_by_dir_name() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project = temp.path().join("project");
        let root_dir = project
            .join(".ac")
            .join(crate::config::root_agent::ROOT_AGENT_DIR_NAME);
        std::fs::create_dir_all(&root_dir).expect("root dir");
        let snapshot = archive_snapshot(
            Vec::new(),
            vec![PendingSpawn {
                cwd: root_dir.to_string_lossy().to_string(),
                label: "root".to_string(),
            }],
        );

        assert!(archive_blockers(&snapshot, &project.to_string_lossy()).is_empty());
    }

    #[test]
    fn archive_blockers_dedupes_pending_mark_and_live_record_by_name() {
        let (_temp, project, agent, _) = archive_scope();
        let snapshot = archive_snapshot(
            vec![(
                archive_test_session("same", &agent, SessionStatus::Running, false),
                true,
            )],
            vec![PendingSpawn {
                cwd: agent,
                label: "same".to_string(),
            }],
        );

        assert_eq!(archive_blockers(&snapshot, &project), vec!["same"]);
    }

    #[test]
    fn project_archive_changed_serializes_frontend_wire_shape() {
        let payload = serde_json::to_value(ProjectArchiveChanged {
            path: "C:/Projects/App".to_string(),
            folder_name: "App".to_string(),
            archived: false,
            reason: ArchiveChangeReason::AutoUnarchive,
            session_name: Some("Dev Session".to_string()),
        })
        .expect("serialize payload");

        assert_eq!(payload["folderName"], json!("App"));
        assert_eq!(payload["sessionName"], json!("Dev Session"));
        assert_eq!(payload["reason"], json!("autoUnarchive"));
        assert!(payload.get("folder_name").is_none());
        assert!(payload.get("session_name").is_none());
    }

    #[tokio::test]
    async fn open_project_inner_broadcasts_only_when_archived_list_changes() {
        let temp = tempfile::tempdir().expect("tempdir");
        let settings_path = temp.path().join("settings.json");
        let project = temp.path().join("proj-open");
        std::fs::create_dir_all(project.join(".ac")).expect("create .ac");
        let path = project.to_string_lossy().to_string();
        let settings = AppSettings {
            archived_project_paths: vec![path.clone()],
            ..AppSettings::default()
        };
        let (app, state, mut rx, _, _) = archive_command_app(settings);
        let app_handle = app.handle().clone();

        open_project_inner_with_event_and_settings_path(
            &app_handle,
            &state,
            &path,
            Some(&settings_path),
        )
        .await
        .expect("open archived project");

        let event = recv_ws_event(&mut rx);
        assert_eq!(event["event"], json!("project_archive_changed"));
        assert_eq!(event["payload"]["path"], json!(path));
        assert_eq!(event["payload"]["archived"], json!(false));
        assert_eq!(event["payload"]["reason"], json!("open"));

        open_project_inner_with_event_and_settings_path(
            &app_handle,
            &state,
            &path,
            Some(&settings_path),
        )
        .await
        .expect("open already active project");

        assert!(
            rx.try_recv().is_err(),
            "opening an already-active project must not broadcast"
        );
    }

    #[tokio::test]
    async fn new_project_inner_broadcasts_only_when_archived_list_changes() {
        let temp = tempfile::tempdir().expect("tempdir");
        let settings_path = temp.path().join("settings.json");
        let project = temp.path().join("proj-new");
        let path = project.to_string_lossy().to_string();
        let settings = AppSettings {
            archived_project_paths: vec![path.clone()],
            ..AppSettings::default()
        };
        let (app, state, mut rx, _, _) = archive_command_app(settings);
        let app_handle = app.handle().clone();

        new_project_inner_with_event_and_settings_path(
            &app_handle,
            &state,
            &path,
            Some(&settings_path),
        )
        .await
        .expect("new archived project");

        let event = recv_ws_event(&mut rx);
        assert_eq!(event["event"], json!("project_archive_changed"));
        assert_eq!(event["payload"]["path"], json!(path));
        assert_eq!(event["payload"]["archived"], json!(false));
        assert_eq!(event["payload"]["reason"], json!("open"));

        new_project_inner_with_event_and_settings_path(
            &app_handle,
            &state,
            &path,
            Some(&settings_path),
        )
        .await
        .expect("new already active project");

        assert!(
            rx.try_recv().is_err(),
            "creating an already-active project must not broadcast"
        );
    }

    #[tokio::test]
    async fn archive_project_inner_absolutizes_relative_path_and_emits_archive_event() {
        let _cwd_lock = cwd_lock().await;
        let temp = tempfile::tempdir().expect("tempdir");
        let settings_path = temp.path().join("settings.json");
        let project = temp.path().join("proj-archive-relative");
        std::fs::create_dir_all(project.join(".ac")).expect("create .ac");
        let path = project.to_string_lossy().to_string();
        let settings = AppSettings {
            project_paths: vec![path.clone()],
            project_path: Some(path.clone()),
            ..AppSettings::default()
        };
        let (app, state, mut rx, session_mgr, pty_mgr) = archive_command_app(settings);
        let app_handle = app.handle().clone();
        let prev = std::env::current_dir().expect("current dir");
        let _guard = CwdGuard(prev);
        std::env::set_current_dir(&project).expect("set cwd");

        archive_project_inner_with_settings_path(
            &app_handle,
            &state,
            &session_mgr,
            &pty_mgr,
            ".",
            Some(&settings_path),
        )
        .await
        .expect("archive relative project");

        let stored = state.read().await.archived_project_paths.clone();
        assert_eq!(stored, vec![path.clone()]);
        let event = recv_ws_event(&mut rx);
        assert_eq!(event["event"], json!("project_archive_changed"));
        assert_eq!(event["payload"]["path"], json!(path));
        assert_eq!(event["payload"]["archived"], json!(true));
        assert_eq!(event["payload"]["reason"], json!("archive"));
    }

    #[tokio::test]
    async fn archive_project_inner_archives_vanished_registered_project() {
        let temp = tempfile::tempdir().expect("tempdir");
        let settings_path = temp.path().join("settings.json");
        let project = temp.path().join("proj-vanished");
        let path = project.to_string_lossy().to_string();
        let settings = AppSettings {
            project_paths: vec![path.clone()],
            project_path: Some(path.clone()),
            ..AppSettings::default()
        };
        let (app, state, mut rx, session_mgr, pty_mgr) = archive_command_app(settings);
        let app_handle = app.handle().clone();

        archive_project_inner_with_settings_path(
            &app_handle,
            &state,
            &session_mgr,
            &pty_mgr,
            &path,
            Some(&settings_path),
        )
        .await
        .expect("archive vanished project");

        let settings = state.read().await;
        assert!(settings.project_paths.is_empty());
        assert_eq!(settings.archived_project_paths, vec![path.clone()]);
        drop(settings);
        let event = recv_ws_event(&mut rx);
        assert_eq!(event["payload"]["path"], json!(path));
        assert_eq!(event["payload"]["archived"], json!(true));
        assert_eq!(event["payload"]["reason"], json!("archive"));
    }

    #[tokio::test]
    async fn archive_project_inner_allows_active_record_without_pty() {
        let temp = tempfile::tempdir().expect("tempdir");
        let settings_path = temp.path().join("settings.json");
        let project = temp.path().join("proj-f1");
        let agent = project.join(".ac").join("wg-1").join("__agent_dev");
        std::fs::create_dir_all(&agent).expect("create agent dir");
        let path = project.to_string_lossy().to_string();
        let settings = AppSettings {
            project_paths: vec![path.clone()],
            project_path: Some(path.clone()),
            ..AppSettings::default()
        };
        let (app, state, mut rx, session_mgr, pty_mgr) = archive_command_app(settings);
        let app_handle = app.handle().clone();
        {
            let mgr = session_mgr.read().await;
            mgr.create_session(
                "shell".to_string(),
                Vec::new(),
                agent.to_string_lossy().to_string(),
                None,
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create active record without PTY");
        }

        archive_project_inner_with_settings_path(
            &app_handle,
            &state,
            &session_mgr,
            &pty_mgr,
            &path,
            Some(&settings_path),
        )
        .await
        .expect("active record without PTY must not block archive");

        assert_eq!(
            state.read().await.archived_project_paths,
            vec![path.clone()]
        );
        let event = recv_ws_event(&mut rx);
        assert_eq!(event["payload"]["path"], json!(path));
        assert_eq!(event["payload"]["archived"], json!(true));
        assert_eq!(event["payload"]["reason"], json!("archive"));
    }

    #[tokio::test]
    async fn archive_project_inner_pending_spawn_mark_blocks_precheck() {
        let temp = tempfile::tempdir().expect("tempdir");
        let settings_path = temp.path().join("settings.json");
        let project = temp.path().join("proj-pending");
        let agent = project.join(".ac").join("wg-1").join("__agent_dev");
        std::fs::create_dir_all(&agent).expect("create agent dir");
        let path = project.to_string_lossy().to_string();
        let settings = AppSettings {
            project_paths: vec![path.clone()],
            project_path: Some(path.clone()),
            ..AppSettings::default()
        };
        let (app, state, mut rx, session_mgr, pty_mgr) = archive_command_app(settings);
        let app_handle = app.handle().clone();
        let _mark = {
            let pty = pty_mgr.lock().unwrap_or_else(|e| e.into_inner());
            pty.mark_spawning(&agent.to_string_lossy(), "spawning")
        };

        let err = archive_project_inner_with_settings_path(
            &app_handle,
            &state,
            &session_mgr,
            &pty_mgr,
            &path,
            Some(&settings_path),
        )
        .await
        .expect_err("pending spawn should block archive before settings write");

        assert!(err.contains("spawning"), "{err}");
        let settings = state.read().await;
        assert_eq!(settings.project_paths, vec![path]);
        assert!(settings.archived_project_paths.is_empty());
        drop(settings);
        assert!(rx.try_recv().is_err(), "precheck failure must not emit");
    }

    #[tokio::test]
    async fn archive_project_inner_rolls_back_with_unarchive_event_when_liveness_appears_after_write(
    ) {
        let temp = tempfile::tempdir().expect("tempdir");
        let settings_path = temp.path().join("settings.json");
        let project = temp.path().join("proj-rollback");
        let agent = project.join(".ac").join("wg-1").join("__agent_dev");
        std::fs::create_dir_all(&agent).expect("create agent dir");
        let path = project.to_string_lossy().to_string();
        let settings = AppSettings {
            project_paths: vec![path.clone()],
            project_path: Some(path.clone()),
            ..AppSettings::default()
        };
        let (app, state, mut rx, session_mgr, _) = archive_command_app(settings);
        let pty_mgr = Arc::new(Mutex::new(PtyManager::new_for_test(Arc::new(
            FlippingLiveBackend::default(),
        ))));
        let app_handle = app.handle().clone();
        let session_id = {
            let mgr = session_mgr.read().await;
            mgr.create_session(
                "shell".to_string(),
                Vec::new(),
                agent.to_string_lossy().to_string(),
                None,
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create session")
            .id
        };
        pty_mgr
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .record_route(
                session_id,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            );

        let err = archive_project_inner_with_settings_path(
            &app_handle,
            &state,
            &session_mgr,
            &pty_mgr,
            &path,
            Some(&settings_path),
        )
        .await
        .expect_err("late spawn should roll back archive");

        assert!(err.contains("open session"), "{err}");
        let settings = state.read().await;
        assert_eq!(settings.project_paths, vec![path.clone()]);
        assert!(settings.archived_project_paths.is_empty());
        drop(settings);
        let event = recv_ws_event(&mut rx);
        assert_eq!(event["event"], json!("project_archive_changed"));
        assert_eq!(event["payload"]["path"], json!(path));
        assert_eq!(event["payload"]["archived"], json!(false));
        assert_eq!(event["payload"]["reason"], json!("unarchive"));
    }

    #[tokio::test]
    async fn archive_project_inner_rolls_back_vanished_project_without_validation() {
        let temp = tempfile::tempdir().expect("tempdir");
        let settings_path = temp.path().join("settings.json");
        let project = temp.path().join("proj-vanished-rollback");
        let agent = project.join(".ac").join("wg-1").join("__agent_dev");
        let path = project.to_string_lossy().to_string();
        let settings = AppSettings {
            project_paths: vec![path.clone()],
            project_path: Some(path.clone()),
            ..AppSettings::default()
        };
        let (app, state, mut rx, session_mgr, _) = archive_command_app(settings);
        let pty_mgr = Arc::new(Mutex::new(PtyManager::new_for_test(Arc::new(
            FlippingLiveBackend::default(),
        ))));
        let app_handle = app.handle().clone();
        let session_id = {
            let mgr = session_mgr.read().await;
            mgr.create_session(
                "shell".to_string(),
                Vec::new(),
                agent.to_string_lossy().to_string(),
                None,
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create session")
            .id
        };
        pty_mgr
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .record_route(
                session_id,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            );

        let err = archive_project_inner_with_settings_path(
            &app_handle,
            &state,
            &session_mgr,
            &pty_mgr,
            &path,
            Some(&settings_path),
        )
        .await
        .expect_err("late liveness should roll back vanished project");

        assert!(err.contains("open session"), "{err}");
        let settings = state.read().await;
        assert_eq!(settings.project_paths, vec![path.clone()]);
        assert!(settings.archived_project_paths.is_empty());
        drop(settings);
        let event = recv_ws_event(&mut rx);
        assert_eq!(event["payload"]["path"], json!(path));
        assert_eq!(event["payload"]["archived"], json!(false));
        assert_eq!(event["payload"]["reason"], json!("unarchive"));
    }

    #[tokio::test]
    async fn poll_tasks_filters_archived_session_workgroups() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project = temp.path().join("proj-task-filter");
        let wg_root = project.join(".ac").join("wg-1");
        let agent = wg_root.join("__agent_dev");
        std::fs::create_dir_all(&agent).expect("create agent dir");
        std::fs::write(wg_root.join("TASK.md"), "Task: should stay hidden").expect("write task");
        let path = project.to_string_lossy().to_string();
        let settings = AppSettings {
            archived_project_paths: vec![path],
            ..AppSettings::default()
        };
        let (app, _, _, session_mgr, _) = archive_command_app(settings);
        let app_handle = app.handle().clone();
        {
            let mgr = session_mgr.read().await;
            mgr.create_session(
                "shell".to_string(),
                Vec::new(),
                agent.to_string_lossy().to_string(),
                None,
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create archived session");
        }
        let watcher = DiscoveryBranchWatcher::new(app_handle, Arc::clone(&session_mgr));

        watcher.poll_tasks(&[]).await.expect("poll tasks");

        assert!(
            watcher.task_cache.lock().unwrap().is_empty(),
            "archived session workgroups must not be checked for TASK.md"
        );
    }

    #[cfg(windows)]
    #[test]
    fn projected_path_string_converts_verbatim_unc() {
        assert_eq!(
            projected_path_string(Path::new(r"\\?\UNC\server\share\repo")),
            r"\\server\share\repo"
        );
    }

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
    async fn create_ac_project_preserves_partial_root_for_registration_repair() {
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
            tmp.path().join(".ac").exists(),
            "fresh partial .ac directory must remain after seed failure"
        );

        create_ac_project(path.clone())
            .await
            .expect("bare retry keeps the existing project");

        assert!(tmp
            .path()
            .join(".ac")
            .join(crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME)
            .is_file());
        assert!(!tmp
            .path()
            .join(".ac")
            .join(crate::config::session_context::COORDINATOR_CONTEXT_TEMPLATE_FILENAME)
            .exists());

        let mut settings = crate::config::settings::AppSettings::default();
        crate::config::projects::register_new_project(&mut settings, &path)
            .expect("registration repairs the partial project");
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
    fn create_ac_project_creation_intent_comes_only_from_the_create_dir_winner() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().to_string_lossy().to_string();
        let creator_path = path.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let creator = std::thread::spawn(move || {
            create_ac_project_impl_with_hook(
                &creator_path,
                |workspace_dir| {
                    crate::config::session_context::create_default_context_templates(workspace_dir)
                },
                move |_, created| {
                    assert!(created, "creator must own the create_dir win");
                    ready_tx.send(()).expect("signal visible root");
                    release_rx
                        .recv_timeout(std::time::Duration::from_secs(5))
                        .expect("release creator");
                },
            )
        });

        ready_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("creator published .ac");
        create_ac_project_impl(&path, |_| {
            panic!("create_dir loser must not backfill context templates")
        })
        .expect("contending bare create");
        assert!(!tmp
            .path()
            .join(".ac")
            .join(crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME)
            .exists());

        release_tx.send(()).expect("release creator");
        creator
            .join()
            .expect("join creator")
            .expect("creator completes");
        assert!(tmp
            .path()
            .join(".ac")
            .join(crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME)
            .is_file());
    }

    #[tokio::test]
    async fn missing_settings_file_project_mutators_preserve_live_list() {
        let temp = tempfile::tempdir().expect("tempdir");
        let settings_path = temp.path().join("settings.json");
        assert!(!settings_path.exists(), "precondition: no settings.json");

        fn base(paths: &[String]) -> Vec<&str> {
            paths
                .iter()
                .map(|p| p.rsplit(['/', '\\']).next().unwrap())
                .collect()
        }

        let mk = |name: &str| {
            let path = temp.path().join(name);
            std::fs::create_dir_all(path.join(".ac")).expect("create project .ac");
            path.to_string_lossy().to_string()
        };
        let (a, b, c) = (mk("proj-a"), mk("proj-b"), mk("proj-c"));

        let settings = crate::config::settings::AppSettings {
            project_paths: vec![a.clone(), b.clone()],
            project_path: Some(a.clone()),
            ..Default::default()
        };
        let state: SettingsState = std::sync::Arc::new(tokio::sync::RwLock::new(settings));

        let disk = || -> Vec<String> {
            let value: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
            value["projectPaths"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_str().unwrap().to_string())
                .collect()
        };

        remove_project_inner_with_settings_path(&state, &a, Some(&settings_path))
            .await
            .expect("remove");
        let memory = state.read().await.project_paths.clone();
        assert_eq!(
            base(&memory),
            vec!["proj-b"],
            "A removed, B preserved in memory"
        );
        assert_eq!(
            base(&disk()),
            vec!["proj-b"],
            "A removed, B preserved on disk"
        );

        open_project_inner_with_settings_path(&state, &c, Some(&settings_path))
            .await
            .expect("open");
        let memory = state.read().await.project_paths.clone();
        assert_eq!(base(&memory), vec!["proj-b", "proj-c"]);
        assert_eq!(base(&disk()), vec!["proj-b", "proj-c"]);

        std::fs::remove_file(&settings_path).expect("remove settings.json");
        let d_path = temp.path().join("proj-d");
        let d = d_path.to_string_lossy().to_string();
        new_project_inner_with_settings_path(&state, &d, Some(&settings_path))
            .await
            .expect("new project");
        assert!(d_path.join(".ac").is_dir());
        let memory = state.read().await.project_paths.clone();
        assert_eq!(base(&memory), vec!["proj-b", "proj-c", "proj-d"]);
        assert_eq!(
            base(&disk()),
            vec!["proj-b", "proj-c", "proj-d"],
            "B and C must not be lost"
        );
    }

    #[tokio::test]
    async fn new_project_settings_save_failure_keeps_live_registration_and_valid_root() {
        let temp = tempfile::tempdir().expect("tempdir");
        let settings_path = temp.path().join("settings.json");
        let project = temp.path().join("project");
        let project_text = project.to_string_lossy().to_string();
        let state: SettingsState =
            std::sync::Arc::new(tokio::sync::RwLock::new(AppSettings::default()));
        let hooks = NewProjectTransactionHooks {
            fail_settings_save: true,
            ..NewProjectTransactionHooks::default()
        };

        let error = new_project_inner_with_settings_path_and_hooks(
            &state,
            &project_text,
            Some(&settings_path),
            hooks,
        )
        .await
        .expect_err("injected save failure must surface");

        assert!(error.contains("injected new-project settings save failure"));
        assert_eq!(state.read().await.project_paths, vec![project_text]);
        assert!(!settings_path.exists());
        assert!(project
            .join(".ac")
            .join(crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME)
            .is_file());
        assert!(project
            .join(".ac")
            .join(crate::config::session_context::COORDINATOR_CONTEXT_TEMPLATE_FILENAME)
            .is_file());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn new_project_replacement_after_save_rolls_back_registration_and_is_retryable() {
        let temp = tempfile::tempdir().expect("tempdir");
        let settings_path = temp.path().join("settings.json");
        let project = temp.path().join("project");
        let project_text = project.to_string_lossy().to_string();
        let state: SettingsState =
            std::sync::Arc::new(tokio::sync::RwLock::new(AppSettings::default()));
        let hooks = NewProjectTransactionHooks {
            before_final_revalidation: Some(std::sync::Arc::new(|workspace_dir| {
                let detached = workspace_dir.with_extension("ac-detached-after-save");
                std::fs::rename(workspace_dir, &detached).expect("detach prepared root");
                std::fs::create_dir(workspace_dir).expect("install replacement root");
                std::fs::write(workspace_dir.join("foreign.txt"), b"FOREIGN")
                    .expect("write replacement marker");
            })),
            ..NewProjectTransactionHooks::default()
        };

        let error = new_project_inner_with_settings_path_and_hooks(
            &state,
            &project_text,
            Some(&settings_path),
            hooks,
        )
        .await
        .expect_err("replacement must reject registration");

        assert!(error.contains("retry the operation"));
        assert!(state.read().await.project_paths.is_empty());
        let disk: AppSettings =
            serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
        assert!(disk.project_paths.is_empty());
        assert_eq!(
            std::fs::read(project.join(".ac").join("foreign.txt")).unwrap(),
            b"FOREIGN"
        );
        assert!(!project
            .join(".ac")
            .join(crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME)
            .exists());
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn new_project_lock_replacement_after_save_rolls_back_registration_and_is_retryable() {
        let temp = tempfile::tempdir().expect("tempdir");
        let settings_path = temp.path().join("settings.json");
        let project = temp.path().join("project");
        let project_text = project.to_string_lossy().to_string();
        let state: SettingsState =
            std::sync::Arc::new(tokio::sync::RwLock::new(AppSettings::default()));
        let hooks = NewProjectTransactionHooks {
            before_final_revalidation: Some(std::sync::Arc::new(|workspace_dir| {
                let lock =
                    workspace_dir.join(crate::config::seed_manifest::SEED_MANIFEST_LOCK_FILENAME);
                let detached = workspace_dir.join(".seed-manifest.lock-detached-after-save");
                std::fs::rename(&lock, &detached).expect("detach held lock identity");
                std::fs::write(&lock, b"").expect("install replacement lock identity");
            })),
            ..NewProjectTransactionHooks::default()
        };

        let error = new_project_inner_with_settings_path_and_hooks(
            &state,
            &project_text,
            Some(&settings_path),
            hooks,
        )
        .await
        .expect_err("lock replacement must reject registration");

        assert!(error.contains("retry the operation"));
        assert!(state.read().await.project_paths.is_empty());
        let disk: AppSettings =
            serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
        assert!(disk.project_paths.is_empty());
        assert!(project
            .join(".ac")
            .join(".seed-manifest.lock-detached-after-save")
            .is_file());
    }

    #[tokio::test]
    async fn keep_custom_context_template_rejects_workspace_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let workspace = tmp.path().join(".ac");
        std::fs::create_dir(&workspace).expect("create .ac");

        let err = keep_custom_context_template(
            workspace.to_string_lossy().to_string(),
            crate::config::session_context::COORDINATOR_CONTEXT_TEMPLATE_FILENAME.to_string(),
            "file".to_string(),
            "default".to_string(),
        )
        .await
        .expect_err("workspace path must not be accepted as project path");

        assert!(err.contains("Project AC Root not found"));
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
        assert!(content.contains(concat!(
            "# AgentsCommander: exclude seed-manifest coordination files.\n",
            "/.seed-manifest.lock\n",
            "/.seed-manifest.*.tmp\n",
            "\n",
            "# AgentsCommander: keep the seed publication manifest reviewable.\n",
            "!/seed-manifest.toml\n"
        )));
    }

    #[test]
    fn ensure_workspace_gitignore_writes_team_config_lock_block_on_create() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let workspace = tmp.path().join(".ac");
        std::fs::create_dir(&workspace).expect("create .ac");

        ensure_workspace_gitignore(&workspace).expect("ensure workspace .gitignore");

        let content =
            std::fs::read_to_string(workspace.join(".gitignore")).expect("read .gitignore");
        let block =
            "# AgentsCommander: exclude team-config coordination files.\n/.team-config-write.lock";
        assert_eq!(
            content.matches(block).count(),
            1,
            "workspace .gitignore must contain the exact team-config lock block once"
        );
        assert_eq!(
            content
                .lines()
                .filter(|line| *line == "/.team-config-write.lock")
                .count(),
            1,
            "workspace .gitignore must contain the anchored team-config lock pattern once"
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

    #[test]
    fn seed_manifest_gitignore_rules_are_anchored_to_ac_root() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project = tmp.path().join("project");
        let workspace = project.join(".ac");
        std::fs::create_dir_all(workspace.join("nested")).expect("create .ac tree");
        ensure_workspace_gitignore(&workspace).expect("ensure workspace .gitignore");

        for relative in [
            ".ac/.seed-manifest.lock",
            ".ac/.seed-manifest.00000000-0000-0000-0000-000000000000.tmp",
            ".ac/seed-manifest.toml",
            ".ac/nested/.seed-manifest.lock",
            ".ac/nested/.seed-manifest.00000000-0000-0000-0000-000000000000.tmp",
            ".ac/nested/seed-manifest.toml",
        ] {
            std::fs::write(project.join(relative), b"fixture").expect("write Git fixture");
        }

        let init = std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&project)
            .output()
            .expect("git init");
        assert!(
            init.status.success(),
            "git init failed: {}",
            String::from_utf8_lossy(&init.stderr)
        );

        let ignored = |relative: &str| {
            std::process::Command::new("git")
                .args(["check-ignore", "--quiet", "--no-index", "--", relative])
                .current_dir(&project)
                .status()
                .expect("git check-ignore")
                .success()
        };
        assert!(ignored(".ac/.seed-manifest.lock"));
        assert!(ignored(
            ".ac/.seed-manifest.00000000-0000-0000-0000-000000000000.tmp"
        ));
        assert!(!ignored(".ac/seed-manifest.toml"));
        assert!(!ignored(".ac/nested/.seed-manifest.lock"));
        assert!(!ignored(
            ".ac/nested/.seed-manifest.00000000-0000-0000-0000-000000000000.tmp"
        ));
        assert!(!ignored(".ac/nested/seed-manifest.toml"));
    }

    #[test]
    fn ensure_workspace_gitignore_appends_team_config_lock_block_preserving_bytes_and_is_idempotent(
    ) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let workspace = tmp.path().join(".ac");
        std::fs::create_dir(&workspace).expect("create .ac");
        let gitignore_path = workspace.join(".gitignore");
        let original = b"# User-authored rules\r\n!important.txt\r\n\r\n# AgentsCommander: exclude workgroup cloned repos from parent git tracking.\r\n# Without this, parent repo operations (checkout, reset) corrupt child clones.\r\nwg-*/\r\n"
            .to_vec();
        std::fs::write(&gitignore_path, &original).expect("write .gitignore");

        ensure_workspace_gitignore(&workspace).expect("ensure workspace .gitignore");

        let updated = std::fs::read(&gitignore_path).expect("read updated .gitignore");
        assert!(
            updated.starts_with(&original),
            "workspace .gitignore must preserve the original bytes as an exact prefix"
        );
        let updated_text = std::str::from_utf8(&updated).expect("updated .gitignore is UTF-8");
        let block =
            "# AgentsCommander: exclude team-config coordination files.\n/.team-config-write.lock";
        assert_eq!(
            updated_text.matches(block).count(),
            1,
            "workspace .gitignore must append the exact team-config lock block once"
        );
        assert_eq!(
            updated_text
                .lines()
                .filter(|line| *line == "/.team-config-write.lock")
                .count(),
            1,
            "workspace .gitignore must contain the anchored team-config lock pattern once"
        );

        let once_updated = updated;
        ensure_workspace_gitignore(&workspace).expect("ensure workspace .gitignore again");
        let twice_updated = std::fs::read(&gitignore_path).expect("read .gitignore again");
        assert_eq!(
            twice_updated, once_updated,
            "a repeated ensure must leave .gitignore byte-identical"
        );
    }

    #[test]
    fn ensure_workspace_gitignore_team_config_lock_rule_is_root_anchored() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project = tmp.path().join("project");
        let workspace = project.join(".ac");
        std::fs::create_dir_all(&workspace).expect("create .ac");
        ensure_workspace_gitignore(&workspace).expect("ensure workspace .gitignore");

        let init_status = std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&project)
            .status()
            .expect("git init must execute");
        assert!(init_status.success(), "git init must succeed");

        let empty_excludes = project.join("empty-global-excludes");
        std::fs::write(&empty_excludes, []).expect("create empty global excludes file");
        std::fs::write(workspace.join(".team-config-write.lock"), [])
            .expect("create root team-config lock");
        std::fs::create_dir_all(workspace.join("nested")).expect("create nested directory");
        std::fs::write(workspace.join("nested").join(".team-config-write.lock"), [])
            .expect("create nested team-config lock");

        let excludes_override = format!(
            "core.excludesFile={}",
            empty_excludes.to_string_lossy().replace('\\', "/")
        );
        let root_output = std::process::Command::new("git")
            .arg("-c")
            .arg(&excludes_override)
            .args([
                "check-ignore",
                "-v",
                "--no-index",
                "--",
                ".ac/.team-config-write.lock",
            ])
            .current_dir(&project)
            .output()
            .expect("root git check-ignore must execute");
        assert!(
            root_output.status.success(),
            "root git check-ignore must match: {}",
            String::from_utf8_lossy(&root_output.stderr)
        );

        let root_stdout = String::from_utf8(root_output.stdout).expect("root output is UTF-8");
        let root_line = root_stdout.trim_end_matches(&['\r', '\n'][..]);
        let (source_and_pattern, target) = root_line
            .split_once('\t')
            .expect("verbose git check-ignore output contains a tab");
        let mut source_fields = source_and_pattern.rsplitn(3, ':');
        let pattern = source_fields.next().expect("matched pattern");
        let line_number = source_fields.next().expect("matched line number");
        let source = source_fields.next().expect("matched source");
        assert_eq!(
            source, ".ac/.gitignore",
            "match source must be .ac/.gitignore"
        );
        assert!(
            line_number.parse::<usize>().is_ok(),
            "match line number must be numeric"
        );
        assert_eq!(
            pattern, "/.team-config-write.lock",
            "match pattern must be the anchored team-config lock rule"
        );
        assert_eq!(
            target, ".ac/.team-config-write.lock",
            "match target must be the root team-config lock"
        );

        let nested_output = std::process::Command::new("git")
            .arg("-c")
            .arg(&excludes_override)
            .args([
                "check-ignore",
                "-v",
                "--no-index",
                "--",
                ".ac/nested/.team-config-write.lock",
            ])
            .current_dir(&project)
            .output()
            .expect("nested git check-ignore must execute");
        assert_eq!(
            nested_output.status.code(),
            Some(1),
            "nested git check-ignore must report no match: {}",
            String::from_utf8_lossy(&nested_output.stderr)
        );
        assert!(
            nested_output.stdout.is_empty(),
            "nested git check-ignore must emit no stdout"
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

    // --- #943 Module B2: the widened ac_discovery_branch_updated payload ---

    /// Repo tuple: (path, branch, dirty). #1028 added the third column.
    fn b2_payload(
        branch: Option<&str>,
        repos: &[(&str, Option<&str>, Option<bool>)],
    ) -> DiscoveryBranchPayload {
        DiscoveryBranchPayload {
            replica_path: "C:/proj/.ac/wg-1-team/__agent_coord".to_string(),
            branch: branch.map(str::to_string),
            repo_branches: repos
                .iter()
                .map(|(_, b, _)| b.map(str::to_string))
                .collect(),
            repo_paths: repos.iter().map(|(p, _, _)| p.to_string()).collect(),
            repo_dirty: repos.iter().map(|(_, _, d)| *d).collect(),
        }
    }

    /// The wire contract the frontend codes against. `#[serde(rename_all =
    /// "camelCase")]` must produce exactly these five keys, `None` must serialize
    /// as `null` (not omitted: the consumer distinguishes "unknown branch" from
    /// "missing entry"), and `repoBranches[i]` / `repoDirty[i]` must line up with
    /// `repoPaths[i]`.
    ///
    /// #1028: this is the one test that can catch a `repo_dirty` vs `repoDirty` wire
    /// name mismatch. A frontend test cannot: its payload is written to match the TS
    /// interface, so if Rust emitted `repo_dirty` the frontend test would still pass
    /// and every badge in production would go permanently violet.
    #[test]
    fn discovery_branch_payload_wire_shape_is_camel_case_with_explicit_nulls() {
        let payload = b2_payload(
            None,
            &[
                (
                    "C:/wg/repo-AgentsCommander",
                    Some("feature/943"),
                    Some(true),
                ),
                ("C:/wg/repo-webpage", None, None),
            ],
        );

        let value = serde_json::to_value(&payload).expect("serialize");

        assert_eq!(
            value,
            serde_json::json!({
                "replicaPath": "C:/proj/.ac/wg-1-team/__agent_coord",
                "branch": null,
                "repoBranches": ["feature/943", null],
                "repoPaths": ["C:/wg/repo-AgentsCommander", "C:/wg/repo-webpage"],
                "repoDirty": [true, null],
            })
        );
    }

    /// The single-repo shorthand is untouched by B2: one repo still fills `branch`,
    /// and `repo_branches` carries the same value so a consumer can read either.
    #[test]
    fn discovery_branch_payload_keeps_the_single_repo_shorthand() {
        let payload = b2_payload(Some("main"), &[("C:/wg/repo-solo", Some("main"), None)]);
        let value = serde_json::to_value(&payload).expect("serialize");

        assert_eq!(value["branch"], serde_json::json!("main"));
        assert_eq!(value["repoBranches"], serde_json::json!(["main"]));
    }

    /// The emission gate is `cache.get(path) != Some(&payload)`, so payload equality
    /// IS the gate. This is the case B2 exists for: a multi-repo replica has
    /// `branch: None` forever, so the pre-B2 gate (which compared only `branch`)
    /// saw None == None and never re-emitted. A repo changing branch must now fire.
    #[test]
    fn discovery_branch_payload_gate_fires_on_per_repo_drift() {
        let before = b2_payload(
            None,
            &[
                ("C:/wg/repo-a", Some("main"), None),
                ("C:/wg/repo-b", Some("main"), None),
            ],
        );
        let after = b2_payload(
            None,
            &[
                ("C:/wg/repo-a", Some("feature/943"), None),
                ("C:/wg/repo-b", Some("main"), None),
            ],
        );

        assert_eq!(before.branch, after.branch, "multi-repo: both stay None");
        assert_ne!(before, after, "per-repo drift must re-emit");
    }

    /// #1028 - a dirty flip with every branch identical must re-emit, or the badge
    /// never turns red on a cold row. `PartialEq` over the whole payload is what
    /// delivers this, which is why `repo_dirty` had to join the payload rather than
    /// ride a separate event: no gate change was needed, but the field must be IN the
    /// compared value.
    #[test]
    fn discovery_branch_payload_gate_fires_on_dirty_flip() {
        let clean = b2_payload(
            None,
            &[
                ("C:/wg/repo-a", Some("main"), Some(false)),
                ("C:/wg/repo-b", Some("main"), Some(false)),
            ],
        );
        let dirtied = b2_payload(
            None,
            &[
                ("C:/wg/repo-a", Some("main"), Some(true)),
                ("C:/wg/repo-b", Some("main"), Some(false)),
            ],
        );
        let unknown = b2_payload(
            None,
            &[
                ("C:/wg/repo-a", Some("main"), None),
                ("C:/wg/repo-b", Some("main"), Some(false)),
            ],
        );

        assert_eq!(
            clean.repo_branches, dirtied.repo_branches,
            "only dirty moved"
        );
        assert_ne!(clean, dirtied, "clean -> dirty must re-emit");
        assert_ne!(dirtied, unknown, "dirty -> unknown must re-emit");
        assert_ne!(clean, unknown, "false and null are distinct on the wire");
    }

    /// A repo-set change with identical branch text must also fire: swapping a repo
    /// for another one that happens to sit on `main` leaves the branch vec equal, and
    /// gating on branches alone would leave the consumer holding the previous repo
    /// list. Pinning this is why `repo_paths` is part of the payload and the gate.
    #[test]
    fn discovery_branch_payload_gate_fires_on_repo_set_change() {
        let before = b2_payload(
            None,
            &[
                ("C:/wg/repo-a", Some("main"), None),
                ("C:/wg/repo-b", Some("main"), None),
            ],
        );
        let swapped = b2_payload(
            None,
            &[
                ("C:/wg/repo-a", Some("main"), None),
                ("C:/wg/repo-c", Some("main"), None),
            ],
        );
        let reordered = b2_payload(
            None,
            &[
                ("C:/wg/repo-b", Some("main"), None),
                ("C:/wg/repo-a", Some("main"), None),
            ],
        );

        assert_eq!(before.repo_branches, swapped.repo_branches);
        assert_ne!(before, swapped, "a swapped repo must re-emit");
        assert_ne!(before, reordered, "a reordered repo list must re-emit");
    }
}
