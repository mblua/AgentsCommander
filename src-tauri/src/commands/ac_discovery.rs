use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};

use crate::config::settings::SettingsState;
use crate::session::manager::SessionManager;

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
    /// Preferred coding agent ID inherited from the identity matrix
    pub preferred_agent_id: Option<String>,
    /// Absolute paths to repos this replica works on (resolved from config.json "repos")
    pub repo_paths: Vec<String>,
    /// Git branch of the first repo (if exactly one repo), for sidebar display
    pub repo_branch: Option<String>,
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
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcDiscoveryResult {
    pub agents: Vec<AcAgentMatrix>,
    pub teams: Vec<AcTeam>,
    pub workgroups: Vec<AcWorkgroup>,
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

#[derive(Clone)]
struct ReplicaBranchEntry {
    replica_path: String,
    repo_path: String,
    /// Session name format: "wg_name/replica_name"
    session_name: String,
    /// Repo dir name with "repo-" prefix stripped, for formatting git branch display
    git_branch_prefix: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiscoveryBranchPayload {
    replica_path: String,
    branch: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionGitBranchPayload {
    session_id: String,
    branch: Option<String>,
}

pub struct DiscoveryBranchWatcher {
    app_handle: AppHandle,
    session_manager: Arc<tokio::sync::RwLock<SessionManager>>,
    replicas: Mutex<Vec<ReplicaBranchEntry>>,
    cache: Mutex<HashMap<String, Option<String>>>,
}

impl DiscoveryBranchWatcher {
    pub fn new(
        app_handle: AppHandle,
        session_manager: Arc<tokio::sync::RwLock<SessionManager>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            app_handle,
            session_manager,
            replicas: Mutex::new(Vec::new()),
            cache: Mutex::new(HashMap::new()),
        })
    }

    /// Update the list of replicas to watch from discovered workgroups.
    /// Does NOT pre-seed the cache — first poll() will push branches to matching sessions.
    pub fn update_replicas(&self, workgroups: &[AcWorkgroup]) {
        let mut entries = Vec::new();
        let mut known_branches: HashMap<String, Option<String>> = HashMap::new();
        for wg in workgroups {
            for agent in &wg.agents {
                if agent.repo_paths.len() == 1 {
                    // Derive git_branch_prefix from repo dir name (strip "repo-" prefix)
                    let repo_dir = agent.repo_paths[0]
                        .replace('\\', "/")
                        .split('/')
                        .last()
                        .unwrap_or("")
                        .to_string();
                    let prefix = if repo_dir.starts_with("repo-") {
                        repo_dir[5..].to_string()
                    } else {
                        repo_dir.clone()
                    };

                    entries.push(ReplicaBranchEntry {
                        replica_path: agent.path.clone(),
                        repo_path: agent.repo_paths[0].clone(),
                        session_name: format!("{}/{}", wg.name, agent.name),
                        git_branch_prefix: prefix,
                    });
                    known_branches.insert(agent.path.clone(), agent.repo_branch.clone());
                }
            }
        }

        log::info!(
            "[DiscoveryBranchWatcher] update_replicas: {} single-repo replicas registered",
            entries.len()
        );
        for e in &entries {
            log::info!(
                "[DiscoveryBranchWatcher]   replica={}, repo={}, discovery_branch={:?}",
                e.replica_path,
                e.repo_path,
                known_branches.get(&e.replica_path)
            );
        }

        // Prune stale cache entries — do NOT pre-seed new ones.
        // Leaving new entries absent from the cache forces the first poll()
        // to treat them as "changed" and push the branch to matching sessions.
        // Pre-seeding prevented this push, leaving restored sessions stale.
        let valid_paths: std::collections::HashSet<&str> =
            entries.iter().map(|e| e.replica_path.as_str()).collect();
        let mut cache = self.cache.lock().unwrap();
        cache.retain(|k, _| valid_paths.contains(k.as_str()));
        drop(cache);

        *self.replicas.lock().unwrap() = entries;
    }

    /// Start the polling loop on a dedicated thread.
    pub fn start(self: &Arc<Self>) {
        let watcher = Arc::clone(self);
        std::thread::spawn(move || {
            log::info!("[DiscoveryBranchWatcher] thread started, polling every {}s", BRANCH_POLL_INTERVAL.as_secs());
            let rt = tokio::runtime::Runtime::new()
                .expect("Failed to create tokio runtime for DiscoveryBranchWatcher");
            rt.block_on(async move {
                loop {
                    tokio::time::sleep(BRANCH_POLL_INTERVAL).await;
                    watcher.poll().await;
                }
            });
        });
    }

    async fn poll(&self) {
        let entries = self.replicas.lock().unwrap().clone();
        if entries.is_empty() {
            return;
        }

        log::info!("[DiscoveryBranchWatcher] poll: checking {} replicas", entries.len());

        for entry in &entries {
            let branch = Self::detect_branch(&entry.repo_path).await;

            // Single lock acquisition: check + update atomically
            let changed = {
                let mut cache = self.cache.lock().unwrap();
                let cached = cache.get(&entry.replica_path).cloned();
                log::info!(
                    "[DiscoveryBranchWatcher]   replica={}, repo={}, detected={:?}, cached={:?}",
                    entry.replica_path,
                    entry.repo_path,
                    branch,
                    cached
                );
                if cached.as_ref() != Some(&branch) {
                    cache.insert(entry.replica_path.clone(), branch.clone());
                    true
                } else {
                    false
                }
            };

            if changed {
                log::info!(
                    "[DiscoveryBranchWatcher] CHANGED -> emitting event for {}: {:?}",
                    entry.replica_path,
                    branch
                );

                // 1. Emit discovery branch event (for non-instanced replica display)
                let emit_result = self.app_handle.emit(
                    "ac_discovery_branch_updated",
                    DiscoveryBranchPayload {
                        replica_path: entry.replica_path.clone(),
                        branch: branch.clone(),
                    },
                );
                if let Err(e) = emit_result {
                    log::error!("[DiscoveryBranchWatcher] emit failed: {:?}", e);
                }

                // 2. Update active session's gitBranch if one exists for this replica
                let formatted_branch = branch.as_ref()
                    .map(|b| format!("{}/{}", entry.git_branch_prefix, b));

                let mgr = self.session_manager.read().await;
                if let Some(session_id) = mgr.find_by_name(&entry.session_name).await {
                    mgr.set_git_branch(session_id, formatted_branch.clone()).await;
                    let _ = self.app_handle.emit(
                        "session_git_branch",
                        SessionGitBranchPayload {
                            session_id: session_id.to_string(),
                            branch: formatted_branch,
                        },
                    );
                    log::info!(
                        "[DiscoveryBranchWatcher] session {} ({}) gitBranch updated",
                        entry.session_name,
                        session_id
                    );
                }
            }
        }
    }

    async fn detect_branch(dir: &str) -> Option<String> {
        #[cfg(windows)]
        const CREATE_NO_WINDOW: u32 = 0x08000000;

        let mut cmd = tokio::process::Command::new("git");
        cmd.args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(dir);

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
    settings: State<'_, SettingsState>,
    branch_watcher: State<'_, Arc<DiscoveryBranchWatcher>>,
) -> Result<AcDiscoveryResult, String> {
    let cfg = settings.read().await;
    let mut agents: Vec<AcAgentMatrix> = Vec::new();
    let mut teams: Vec<AcTeam> = Vec::new();
    let mut workgroups: Vec<AcWorkgroup> = Vec::new();

    for base_path in &cfg.repo_paths {
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

                    let preferred_agent_id = path.join("config.json")
                        .exists()
                        .then(|| std::fs::read_to_string(path.join("config.json")).ok())
                        .flatten()
                        .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
                        .and_then(|v| v.get("tooling")?.get("lastCodingAgent")?.as_str().map(String::from));

                    log::info!("[BUG#1] AC Discovery agent: dir={:?}, preferred_agent_id={:?}", dir_name, preferred_agent_id);

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

                                // Resolve identity to matrix dir and read its lastCodingAgent
                                let preferred_agent_id = identity_path.as_ref().and_then(|rel| {
                                    let matrix_dir = wg_path.join(rel);
                                    let matrix_config = matrix_dir.join("config.json");
                                    std::fs::read_to_string(&matrix_config).ok()
                                        .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
                                        .and_then(|v| v.get("tooling")?.get("lastCodingAgent")?.as_str().map(String::from))
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

                                wg_agents.push(AcAgentReplica {
                                    name: replica_name,
                                    path: wg_path.to_string_lossy().to_string(),
                                    identity_path,
                                    preferred_agent_id,
                                    repo_paths,
                                    repo_branch,
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
                    });
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

    // Update the branch watcher with discovered replicas
    branch_watcher.update_replicas(&workgroups);

    Ok(AcDiscoveryResult { agents, teams, workgroups })
}

/// Check if a folder has a .ac-new/ subdirectory.
#[tauri::command]
pub async fn check_project_path(path: String) -> Result<bool, String> {
    let ac_new = Path::new(&path).join(".ac-new");
    Ok(ac_new.is_dir())
}

/// Create a .ac-new/ directory inside the given path.
#[tauri::command]
pub async fn create_ac_project(path: String) -> Result<(), String> {
    let ac_new = Path::new(&path).join(".ac-new");
    std::fs::create_dir_all(&ac_new)
        .map_err(|e| format!("Failed to create .ac-new directory: {}", e))?;
    Ok(())
}

/// Discover AC agents/workgroups from a single project path.
/// Unlike discover_ac_agents which scans repo_paths from settings,
/// this targets a specific folder.
#[tauri::command]
pub async fn discover_project(
    path: String,
    branch_watcher: State<'_, Arc<DiscoveryBranchWatcher>>,
) -> Result<AcDiscoveryResult, String> {
    let base = Path::new(&path);
    if !base.is_dir() {
        return Err(format!("Path is not a directory: {}", path));
    }

    let ac_new_dir = base.join(".ac-new");
    if !ac_new_dir.is_dir() {
        return Ok(AcDiscoveryResult {
            agents: vec![],
            teams: vec![],
            workgroups: vec![],
        });
    }

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

            let preferred_agent_id = entry_path.join("config.json")
                .exists()
                .then(|| std::fs::read_to_string(entry_path.join("config.json")).ok())
                .flatten()
                .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
                .and_then(|v| v.get("tooling")?.get("lastCodingAgent")?.as_str().map(String::from));

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

                        let preferred_agent_id = identity_path.as_ref().and_then(|rel| {
                            let matrix_dir = wg_path.join(rel);
                            let matrix_config = matrix_dir.join("config.json");
                            std::fs::read_to_string(&matrix_config).ok()
                                .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
                                .and_then(|v| v.get("tooling")?.get("lastCodingAgent")?.as_str().map(String::from))
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

                        wg_agents.push(AcAgentReplica {
                            name: replica_name,
                            path: wg_path.to_string_lossy().to_string(),
                            identity_path,
                            preferred_agent_id,
                            repo_paths,
                            repo_branch,
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

    // Update the branch watcher with discovered replicas
    branch_watcher.update_replicas(&workgroups);

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
