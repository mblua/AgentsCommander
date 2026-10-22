use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use crate::config::ac_root::{
    ac_root_for_project, canonical_ac_root_label, existing_ac_root, find_ac_root_segment,
    has_ac_root,
};
use crate::config::coding_agent_profiles::{
    read_replica_current_coding_agent, read_replica_profile_result,
};
use crate::config::instance_artifacts::{
    CODING_AGENTS_BASE_TMP_ARTIFACT, CODING_AGENTS_LOCAL_ARTIFACT,
    CODING_AGENTS_LOCAL_TMP_ARTIFACT, CODING_AGENTS_LOCK_ARTIFACT,
    CODING_AGENTS_MIGRATION_BACKUP_ARTIFACT, CODING_AGENTS_MIGRATION_BACKUP_TMP_ARTIFACT,
    CODING_AGENTS_MIGRATION_JOURNAL_ARTIFACT, CODING_AGENTS_MIGRATION_JOURNAL_TMP_ARTIFACT,
    CODING_AGENTS_RETIRED_LOCK_ARTIFACT, CODING_AGENTS_SET_ASIDE_ARTIFACT, SETTINGS_FILE_NAME,
};
use crate::config::settings::{AppSettings, SettingsState};
use crate::pty::manager::{PendingSpawn, PtyManager};
use crate::session::manager::SessionManager;
use crate::session::session::{SessionInfo, SessionRepo, SessionStatus};

/// Per-call monotonic ID partitioning concurrent `discover_*` invocations in the log
/// stream. See plan §3 A0 (round-2 G5 + round-3 H1 placement-fix). Consumed only by
/// `format!`-into-log; `Relaxed` is canonical for an observed-but-not-synchronizing counter.
static DISCOVERY_CALL_ID: AtomicU64 = AtomicU64::new(0);

/// Resolve the preferred coding agent for a directory from its config.json
/// `tooling.lastCodingAgent` and that id's `codingAgents` descriptor.
///
/// #2434 - resolved through `resolve_portable_reference` (local id, cell digest,
/// case-insensitive label, canonical command), so a foreign id maps to OUR
/// agent. The sidebar lists rather than launches: no letter is requested and
/// only the resolved id is read. An unresolvable reference returns `None` with
/// a warning; another agent is never substituted.
fn read_preferred_agent_id(
    dir: &Path,
    settings: &crate::config::settings::AppSettings,
) -> Option<String> {
    let mut warnings = Vec::new();
    let preferred = read_preferred_agent_id_logged(dir, settings, &mut warnings);
    for warning in warnings {
        log::warn!("[discovery] {}", warning);
    }
    preferred
}

fn read_preferred_agent_id_logged(
    dir: &Path,
    settings: &crate::config::settings::AppSettings,
    warnings: &mut Vec<String>,
) -> Option<String> {
    let config_path = dir.join("config.json");
    // #2786 (C1) - through the shared loader: state file first, key-wise.
    let v = match crate::config::agent_config::read_agent_local_config_json(dir) {
        Ok(v) => v?,
        Err(e) => {
            warnings.push(format!(
                "malformed '{}', no coding-agent reference: {}",
                config_path.display(),
                e
            ));
            return None;
        }
    };
    let foreign_id = v.get("tooling")?.get("lastCodingAgent")?.as_str()?;
    let reference =
        crate::config::agent_command::StoredReference::from_config(Some(&v), foreign_id);
    match crate::config::agent_command::resolve_portable_reference(settings, &reference) {
        crate::config::agent_command::MatchOutcome::Matched(found) => Some(found.agent_id),
        crate::config::agent_command::MatchOutcome::NoMatch { reference } => {
            warnings.push(format!(
                "coding-agent reference '{}' in '{}' matches no configured agent",
                reference,
                config_path.display()
            ));
            None
        }
    }
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
    /// #1941 - the stored selection pair from the strict #1939 read, or `null`
    /// when no complete pair exists (including an invalid state).
    pub saved_pair: Option<AcReplicaSavedPair>,
    /// #1941 - `unlocked` | `locked` | `invalid`, from the strict state read.
    /// Never a synthesized default: an unreadable/malformed state is `invalid`.
    pub selection_state: Option<String>,
    /// #1941 - the strict-read diagnostic when `selection_state` is `invalid`.
    pub selection_error: Option<String>,
}

/// #1941 - the selection pair as discovery exposes it. A strict complete pair
/// fills both fields; the option type leaves room for legacy/partial data.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcReplicaSavedPair {
    pub coding_agent_id: Option<String>,
    pub requested_profile: Option<String>,
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
    find_ac_root_segment(&parts).and_then(|i| (i > 0).then(|| parts[i - 1].to_string()))
}

struct ReplicaIdentityRead {
    config: Option<serde_json::Value>,
    identity: Option<crate::config::replica_identity::WgReplicaIdentity>,
    invalid_identity: bool,
}

/// #1941 - strict protection state for discovery: `invalid` carries its
/// diagnostic and is never reported as an unlocked/false default.
fn replica_selection_wire_fields(
    replica_dir: &Path,
) -> (Option<AcReplicaSavedPair>, Option<String>, Option<String>) {
    use crate::config::coding_agent_profiles::ReplicaSelectionState;
    match crate::config::coding_agent_profiles::read_replica_selection_state(replica_dir) {
        ReplicaSelectionState::Unlocked { pair, .. } => (
            pair.map(|pair| AcReplicaSavedPair {
                coding_agent_id: Some(pair.coding_agent_id),
                requested_profile: Some(pair.requested_profile),
            }),
            Some("unlocked".to_string()),
            None,
        ),
        ReplicaSelectionState::Locked { pair, .. } => (
            Some(AcReplicaSavedPair {
                coding_agent_id: Some(pair.coding_agent_id),
                requested_profile: Some(pair.requested_profile),
            }),
            Some("locked".to_string()),
            None,
        ),
        ReplicaSelectionState::Invalid { diagnostic } => {
            (None, Some("invalid".to_string()), Some(diagnostic))
        }
    }
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
        let origin = find_ac_root_segment(&parts)
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

/// #1298 - bound for the one-shot branch detection below. Deliberately its own
/// constant and not the sweeper's `DETECT_TIMEOUT`: this is a discovery-time
/// one-shot on the async command body, not the poll loop's per-round bound, and
/// the two must be able to move independently.
const BRANCH_DETECT_TIMEOUT: Duration = Duration::from_secs(2);

/// Detect the git branch of a directory without blocking a runtime worker.
///
/// #1298 - the previous synchronous version spawned a `std::process::Command`
/// straight on the async body of `discover_ac_agents` / `discover_project` and
/// held its Tokio worker for as long as `git` took, with no bound. Now
/// `tokio::process` under `BRANCH_DETECT_TIMEOUT`, with `kill_on_drop(true)` so
/// the timeout's drop is what terminates `git.exe` (same mechanism as
/// `git_watcher.rs`'s detection).
///
/// Named `detect_repo_branch`, not `detect_git_branch`, because
/// `config/session_context.rs` already owns a private `detect_git_branch` that
/// runs a DIFFERENT command (`git -C <dir> branch --show-current`); two
/// same-named private detectors would break argv-based triage of `git.exe` peaks.
///
/// No `--no-optional-locks` here, unlike `detect_git_status` three functions
/// away where it IS load-bearing: `rev-parse` neither takes `.git/index.lock`
/// nor rewrites `.git/index`, so the flag would be cargo-culted noise.
async fn detect_repo_branch(dir: &str) -> Option<String> {
    #[cfg(windows)]
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let mut cmd = tokio::process::Command::new("git");
    crate::pty::credentials::scrub_credentials_from_tokio_command(&mut cmd);
    crate::config::agent_path::apply_search_path_to_tokio_command(&mut cmd);
    cmd.args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(dir)
        .kill_on_drop(true);

    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    match tokio::time::timeout(BRANCH_DETECT_TIMEOUT, cmd.output()).await {
        Ok(Ok(out)) if out.status.success() => {
            let branch = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if branch.is_empty() || branch == "HEAD" {
                None
            } else {
                Some(branch)
            }
        }
        Err(_) => {
            log::debug!(
                "[discovery] branch detection timed out after {BRANCH_DETECT_TIMEOUT:?}: {dir}"
            );
            None
        }
        _ => None,
    }
}

// --- Project file difference (#3060) ---

/// One total bound for every Git read of a single `get_project_file_difference` request.
const PROJECT_FILE_DIFFERENCE_TIMEOUT: Duration = Duration::from_secs(5);

/// Inherited Git controls that would point the child at another repository or index.
const PROJECT_GIT_REDIRECT_ENV: [&str; 6] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
];

/// Distinct paths of the Project's own repository that differ from the cached
/// `origin/main`, untracked non-ignored files included. `None` = unknown; never
/// a zero substituted for a failed read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectFileDifference {
    pub count: Option<u32>,
}

/// Shared by the Tauri command and the WebSocket `BrowserProject` route.
pub async fn get_project_file_difference_inner(path: &str) -> ProjectFileDifference {
    read_project_file_difference(
        Path::new(path),
        PROJECT_FILE_DIFFERENCE_TIMEOUT,
        run_project_git,
    )
    .await
}

#[tauri::command]
pub async fn get_project_file_difference(path: String) -> Result<ProjectFileDifference, String> {
    Ok(get_project_file_difference_inner(&path).await)
}

/// Runner and bound are parameters so tests can inject a stalled or malformed Git;
/// production always passes `run_project_git` and `PROJECT_FILE_DIFFERENCE_TIMEOUT`.
async fn read_project_file_difference<R, Fut>(
    project: &Path,
    limit: Duration,
    run: R,
) -> ProjectFileDifference
where
    R: Fn(PathBuf, Vec<String>) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<u8>, String>>,
{
    let count =
        match tokio::time::timeout(limit, count_project_file_difference(project, &run)).await {
            Ok(Ok(count)) => Some(count),
            Ok(Err(reason)) => {
                log::debug!(
                    "[discovery] project file difference unknown for {}: {reason}",
                    project.display()
                );
                None
            }
            // Dropping the future drops the in-flight child, which `kill_on_drop` terminates.
            Err(_) => {
                log::debug!(
                    "[discovery] project file difference timed out after {limit:?}: {}",
                    project.display()
                );
                None
            }
        };
    ProjectFileDifference { count }
}

async fn count_project_file_difference<R, Fut>(project: &Path, run: &R) -> Result<u32, String>
where
    R: Fn(PathBuf, Vec<String>) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<u8>, String>>,
{
    if existing_ac_root(project).is_none() {
        return Err("not an AC project root".to_string());
    }
    let project_canonical =
        std::fs::canonicalize(project).map_err(|e| format!("canonicalize project: {e}"))?;
    let git = |args: &[&str]| run(project.to_path_buf(), project_git_args(args));

    // A Project nested inside another repository must not report the parent's count.
    let toplevel = git(&["rev-parse", "--show-toplevel"]).await?;
    let toplevel = std::str::from_utf8(strip_line_terminator(&toplevel))
        .map_err(|_| "toplevel is not UTF-8".to_string())?;
    let toplevel_canonical =
        std::fs::canonicalize(toplevel).map_err(|e| format!("canonicalize toplevel: {e}"))?;
    if toplevel_canonical != project_canonical {
        return Err("project is not its repository toplevel".to_string());
    }

    // Pin the OID so both reads compare against the same commit.
    let oid = git(&["rev-parse", "--verify", "refs/remotes/origin/main^{commit}"]).await?;
    let oid = std::str::from_utf8(strip_line_terminator(&oid))
        .ok()
        .filter(|oid| !oid.is_empty() && oid.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| "origin/main did not resolve to an OID".to_string())?
        .to_string();

    let diff = git(&[
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-renames",
        "--name-only",
        "-z",
        &oid,
        "--",
    ])
    .await?;
    let untracked = git(&["ls-files", "--others", "--exclude-standard", "-z", "--"]).await?;

    let mut paths = std::collections::HashSet::new();
    paths.extend(parse_nul_records(&diff)?);
    paths.extend(parse_nul_records(&untracked)?);
    u32::try_from(paths.len()).map_err(|_| "path count exceeds u32".to_string())
}

fn project_git_args(args: &[&str]) -> Vec<String> {
    ["--no-optional-locks", "-c", "core.fsmonitor=false"]
        .iter()
        .chain(args)
        .map(|arg| arg.to_string())
        .collect()
}

/// Strips one trailing LF or CRLF only; spaces in a path are preserved.
fn strip_line_terminator(bytes: &[u8]) -> &[u8] {
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    bytes.strip_suffix(b"\r").unwrap_or(bytes)
}

/// NUL-terminated `-z` records as raw bytes. Non-empty output that is not
/// NUL-terminated, or an empty record, is malformed.
fn parse_nul_records(bytes: &[u8]) -> Result<Vec<&[u8]>, String> {
    let Some(body) = bytes.strip_suffix(b"\0") else {
        return if bytes.is_empty() {
            Ok(Vec::new())
        } else {
            Err("git output is not NUL-terminated".to_string())
        };
    };
    let records: Vec<&[u8]> = body.split(|b| *b == 0).collect();
    if records.iter().any(|record| record.is_empty()) {
        return Err("git output has an empty record".to_string());
    }
    Ok(records)
}

async fn run_project_git(cwd: PathBuf, args: Vec<String>) -> Result<Vec<u8>, String> {
    #[cfg(windows)]
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let mut cmd = tokio::process::Command::new("git");
    crate::pty::credentials::scrub_credentials_from_tokio_command(&mut cmd);
    crate::config::agent_path::apply_search_path_to_tokio_command(&mut cmd);
    for var in PROJECT_GIT_REDIRECT_ENV {
        cmd.env_remove(var);
    }
    cmd.args(&args)
        .current_dir(&cwd)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);

    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    // args[3] is the subcommand after the shared prefix; stderr stays out of the result.
    let subcommand = args.get(3).map(String::as_str).unwrap_or("?");
    let out = cmd
        .output()
        .await
        .map_err(|e| format!("spawn git {subcommand}: {e}"))?;
    if !out.status.success() {
        return Err(format!("git {subcommand} exited with {}", out.status));
    }
    Ok(out.stdout)
}

// --- Discovery Branch Watcher ---

const BRANCH_POLL_INTERVAL: Duration = Duration::from_secs(15);

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
    /// #1028/#1078 - per-repo local-work state, positionally parallel to `repo_paths`
    /// exactly as `repo_branches` is, and keyed by path by the consumer for the same
    /// reason. `Some(true)` means Git emitted a non-ignored worktree/index entry or the
    /// checked-out `HEAD` was not confirmed reachable from the configured cached
    /// `origin/*` upstream. `None` means never successfully detected for that path
    /// (rendered violet, like clean).
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

#[derive(Clone, PartialEq, Eq)]
struct TaskSentinels {
    task: Option<StatSentinel>,
    status: Option<StatSentinel>,
    journal: bool,
}

#[derive(Clone)]
struct TaskCacheEntry {
    sentinel: TaskSentinels,
    task: Option<String>,
    task_title: Option<String>,
    status: Option<String>,
    revision: String,
    tail_incomplete: bool,
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
    status: Option<String>,
    revision: String,
    tail_incomplete: bool,
}

fn validate_snapshot_room(raw: &str, projects: &[String]) -> Result<PathBuf, String> {
    let canonical =
        std::fs::canonicalize(raw).map_err(|error| format!("invalid room root: {error}"))?;
    let room_name = canonical.file_name().and_then(|name| name.to_str());
    let workspace_name = canonical
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str());
    if !canonical.is_dir()
        || !room_name.is_some_and(crate::config::entity_prefix::has_entity_prefix)
        || !workspace_name.is_some_and(crate::config::ac_root::is_ac_root_name)
    {
        return Err(
            "room root must be a room-* or wg-* directory directly under an AC workspace".into(),
        );
    }
    let configured = projects
        .iter()
        .filter_map(|project| std::fs::canonicalize(project).ok())
        .any(|project| canonical.starts_with(project));
    if !configured {
        return Err("room root is outside configured project paths".into());
    }
    Ok(PathBuf::from(raw))
}

/// Shared Tauri/WS admission and bounded snapshot reader. All guards are dropped
/// before path IO, storage locking or recovery on the blocking worker.
pub(crate) async fn task_snapshot_inner(
    manager: &Arc<tokio::sync::RwLock<SessionManager>>,
    settings: &SettingsState,
    session_id: Option<&str>,
    workgroup_root: Option<&str>,
) -> Result<serde_json::Value, String> {
    let (cwd, raw, projects) = match (session_id, workgroup_root) {
        (Some(id), None) => {
            let uuid =
                Uuid::parse_str(id).map_err(|error| format!("invalid session id: {error}"))?;
            let manager = manager.read().await.clone();
            let session = manager
                .get_session(uuid)
                .await
                .ok_or_else(|| "session not found".to_string())?;
            (Some(session.working_directory), None, Vec::new())
        }
        (None, Some(raw)) => {
            let projects = settings.read().await.project_paths.clone();
            (None, Some(raw.to_owned()), projects)
        }
        _ => return Err("exactly one sessionId or workgroupRoot is required".into()),
    };
    tokio::task::spawn_blocking(move || {
        let root = match cwd {
            Some(cwd) => crate::session::session::find_workgroup_task_path_for_cwd(&cwd)
                .and_then(|path| path.parent().map(Path::to_path_buf))
                .ok_or_else(|| "session is outside a room".to_string())?,
            None => validate_snapshot_room(raw.as_deref().ok_or("missing room root")?, &projects)?,
        };
        crate::commands::entity_creation::read_room_task_snapshot_json(&root)
    })
    .await
    .map_err(|error| format!("snapshot worker failed: {error}"))?
}

#[tauri::command]
pub async fn task_get_snapshot(
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    settings: State<'_, SettingsState>,
    session_id: String,
) -> Result<serde_json::Value, String> {
    task_snapshot_inner(&session_mgr, &settings, Some(&session_id), None).await
}

#[tauri::command]
pub async fn task_get_snapshot_at(
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    settings: State<'_, SettingsState>,
    workgroup_root: String,
) -> Result<serde_json::Value, String> {
    task_snapshot_inner(&session_mgr, &settings, None, Some(&workgroup_root)).await
}

fn task_sentinels(root: &Path) -> Result<TaskSentinels, String> {
    let stat = |name| match std::fs::metadata(root.join(name)) {
        Ok(metadata) => Ok(Some(StatSentinel {
            len: metadata.len(),
            mtime: metadata.modified().ok(),
        })),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("snapshot sentinel read failed: {error}")),
    };
    Ok(TaskSentinels {
        task: stat("TASK.md")?,
        status: stat("TASK-status.jsonl")?,
        journal: stat("TASK-clean.pending.json")?.is_some(),
    })
}

fn poll_task_snapshot(
    root: &Path,
    previous: Option<TaskSentinels>,
) -> Result<Option<TaskCacheEntry>, String> {
    let before = task_sentinels(root)?;
    if !before.journal && previous.as_ref() == Some(&before) {
        return Ok(None);
    }
    let value = crate::commands::entity_creation::read_room_task_snapshot_json(root)?;
    let after = task_sentinels(root)?;
    if before != after {
        return Ok(None);
    }
    let task: Option<String> =
        serde_json::from_value(value["task"].clone()).map_err(|error| error.to_string())?;
    Ok(Some(TaskCacheEntry {
        sentinel: after,
        task: task.as_deref().and_then(extract_task_first_line),
        task_title: serde_json::from_value(value["taskTitle"].clone())
            .map_err(|error| error.to_string())?,
        status: serde_json::from_value(value["status"].clone())
            .map_err(|error| error.to_string())?,
        revision: value["revision"]
            .as_str()
            .ok_or("snapshot revision missing")?
            .to_owned(),
        tail_incomplete: value["tailIncomplete"]
            .as_bool()
            .ok_or("snapshot tail flag missing")?,
    }))
}

/// The caller commits the returned cache only on success. Failed emission leaves
/// the old sentinels intact, so an unchanged-stat next pass retries delivery.
fn deliver_poll_task(
    previous: Option<&TaskCacheEntry>,
    next: TaskCacheEntry,
    emit: impl FnOnce(&TaskCacheEntry) -> Result<(), String>,
) -> Result<TaskCacheEntry, String> {
    let changed = previous.is_none_or(|old| {
        old.task != next.task
            || old.task_title != next.task_title
            || old.status != next.status
            || old.revision != next.revision
            || old.tail_incomplete != next.tail_incomplete
    });
    if changed {
        emit(&next)?;
    }
    Ok(next)
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
    fn with_replicas_mut<R>(
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
        crate::pty::git_watcher::set_discovery_repo_paths(paths);
        crate::pty::git_watcher::set_discovery_repo_rooms(rooms);
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
        let canonical_key = std::fs::canonicalize(project_dir)
            .ok()
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

                // #1298 - pure map reads. This loop no longer spawns `git`: the global
                // `GitSweeper` is the only polling producer, and this poll consumes its
                // published snapshot. An unpublished path reads `None`, which is the
                // same "unknown" a failed detection produced before.
                let statuses: Vec<Option<crate::pty::git_watcher::GitStatus>> = entry
                    .repos
                    .iter()
                    .map(|(_, path)| crate::pty::git_watcher::read_git_status(path))
                    .collect();

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
    async fn check_workgroup_task(&self, wg_root: PathBuf, session_ids: Vec<Uuid>) {
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

/// #1065 Stage F: run the project context-template scan under the seed-manifest
/// gate, recording each recognized create/update at its commit-point time.
///
/// A pre-contention gate degradation runs the scan untracked (writes proceed,
/// unrecorded); a contention/unsafe failure skips the scan so no write races a
/// cooperating writer (plan sections 5.2/6.1). `activation` is `None` for a
/// `#[cfg(test)]` lib build or an unactivated caller, which runs the plain scan
/// unchanged. Runs inside the discovery `run_blocking_owned` closure so the
/// 10-second gate poll never blocks a Tokio worker.
fn scan_project_context_templates_recorded(
    project_root: &Path,
    ac_root: &Path,
    activation: Option<&crate::config::seed_manifest::ManifestActivationToken>,
) -> Result<Vec<crate::config::seeded_context_templates::ContextTemplateUpdate>, String> {
    use crate::config::seed_manifest::SoftProjectGate;
    use crate::config::seeded_context_templates::{
        scan_project_context_template_updates,
        scan_project_context_template_updates_with_publications, ContextPublication,
    };
    let Some(token) = activation else {
        return scan_project_context_template_updates(project_root, ac_root);
    };
    match crate::config::seed_manifest::acquire_project_gate_soft(project_root) {
        SoftProjectGate::Held(mut guard) => {
            let result = {
                let mut on_publication =
                    |filename: &'static str, publication: ContextPublication| {
                        crate::config::session_context::record_project_context_publication(
                            &mut guard,
                            token,
                            filename,
                            publication.published_at,
                        );
                    };
                scan_project_context_template_updates_with_publications(
                    project_root,
                    ac_root,
                    &mut on_publication,
                )
            };
            guard.release();
            result
        }
        SoftProjectGate::DegradedUntracked => {
            scan_project_context_template_updates(project_root, ac_root)
        }
        SoftProjectGate::Unavailable(error) => {
            log::warn!(
                "[context-templates] skipping gated context scan for {}: {}",
                ac_root.display(),
                error
            );
            Ok(Vec::new())
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
            let Some(ac_root) = existing_ac_root(&repo_dir) else {
                continue;
            };
            let repo_dir_str = projected_path_string(&repo_dir);

            // Opportunistic: ensure gitignore exists for existing projects
            let _ = ensure_ac_root_gitignore_with_names(
                &ac_root,
                &crate::config::agent_command::managed_instructions_filenames(&cfg),
            );
            let context_scan = crate::config::seed_manifest::run_blocking_owned({
                let repo_dir = repo_dir.clone();
                let ac_root = ac_root.clone();
                move || {
                    // #1065 Stage F: production records recognized context updates
                    // under the gate; a `#[cfg(test)]` lib build stays non-emitting.
                    #[cfg(not(test))]
                    let activation =
                        Some(crate::config::seed_manifest::ManifestActivationToken::production());
                    #[cfg(test)]
                    let activation: Option<
                        crate::config::seed_manifest::ManifestActivationToken,
                    > = None;
                    scan_project_context_templates_recorded(
                        &repo_dir,
                        &ac_root,
                        activation.as_ref(),
                    )
                }
            })
            .await;
            match context_scan {
                Ok(Ok(mut updates)) => context_template_updates.append(&mut updates),
                Ok(Err(e)) => log::warn!(
                    "[context-templates] scan failed for {}: {}",
                    ac_root.display(),
                    e
                ),
                Err(e) => log::warn!(
                    "[context-templates] blocking scan failed for {}: {}",
                    ac_root.display(),
                    e
                ),
            }
            loops.extend(crate::config::loops::discover_loops_in_project(&repo_dir));

            let project_folder = repo_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();

            let entries = match std::fs::read_dir(&ac_root) {
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

                    let preferred_agent_id = read_preferred_agent_id(&path, &cfg);

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
                if crate::config::entity_prefix::has_entity_prefix(&dir_name) {
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
                                        read_preferred_agent_id(&identity.matrix_dir, &cfg)
                                    });
                                let current_coding_agent_id =
                                    read_replica_current_coding_agent(&wg_path).filter(|id| {
                                        cfg.agents.iter().any(|agent| agent.id == *id)
                                    });
                                let profile_read = read_replica_profile_result(&wg_path);
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
                                    detect_repo_branch(&repo_paths[0]).await
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
                                        &ac_root, &path,
                                    )
                                    .map(|resolved| resolved.agent_name == replica_name)
                                    .unwrap_or(false);

                                log::debug!(
                                    "[ac-discovery] call={} replica — project='{}' wg='{}' \
                                        replica='{}' fqn='{}:{}/{}' is_coordinator={}",
                                    call_id,
                                    project_folder,
                                    dir_name,
                                    replica_name,
                                    project_folder,
                                    dir_name,
                                    replica_name,
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

                                // #1941 - strict protection state from the same
                                // validated replica directory; an invalid state is
                                // surfaced as `invalid`, never as default false.
                                let (saved_pair, selection_state, selection_error) =
                                    replica_selection_wire_fields(&wg_path);

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
                                    saved_pair,
                                    selection_state,
                                    selection_error,
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
                    "[discovery] Workgroup '{}' → team '{}' (matched via name suffix, identity may \
                        be missing)",
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
        "[ac-discovery] call={} discover_ac_agents: summary — workgroups={} teams={} replicas={} \
            coordinator={}",
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
    existing_ac_root(Path::new(path)).is_some()
}

/// Check if a folder has a Project AC Root subdirectory.
#[tauri::command]
pub async fn check_project_path(path: String) -> Result<bool, String> {
    Ok(check_project_path_inner(&path))
}

/// Ensure the Project AC Root .gitignore exists and contains all required exclusion patterns.
/// Called during project creation, workgroup creation, and opportunistically during discovery.
///
/// #2434 - also ignores every configured agent's instructions filename, read
/// from the saved settings. Callers that already hold an `AppSettings` call
/// [`ensure_ac_root_gitignore_with_names`] directly.
pub(crate) fn ensure_ac_root_gitignore(ac_root: &Path) -> Result<(), String> {
    // Test builds never read the developer's real `settings.json` through this
    // writer: they get the legacy floor rows only.
    #[cfg(not(test))]
    let configured = crate::config::agent_command::managed_instructions_filenames(
        &crate::config::settings::load_settings_for_cli_strict()?,
    );
    #[cfg(test)]
    let configured: Vec<String> = Vec::new();
    ensure_ac_root_gitignore_with_names(ac_root, &configured)
}

/// #2434 - escape a bare instructions filename for a gitignore pattern: `\`,
/// `*`, `?`, `[`, `]` anywhere, and a leading `#` or `!`, each get a `\`
/// prefix. Separators and trailing spaces are already rejected upstream.
fn escape_gitignore_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 2);
    for (index, ch) in name.chars().enumerate() {
        if matches!(ch, '\\' | '*' | '?' | '[' | ']') || (index == 0 && matches!(ch, '#' | '!')) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// [`ensure_ac_root_gitignore`] with the already-resolved instructions
/// filenames. The `CLAUDE.md` / `AGENTS.md` floor rows are always written; each
/// other distinct name adds an escaped `**/__agent_*/<name>` row. A name failing
/// `is_safe_instructions_filename` is dropped with a warning.
pub(crate) fn ensure_ac_root_gitignore_with_names(
    ac_root: &Path,
    configured: &[String],
) -> Result<(), String> {
    let gitignore_path = ac_root.join(".gitignore");
    // #2717 - the project-settings rows have one home, shared with the
    // migration's sweep.
    let project_settings_rows = crate::config::naming_migration::project_settings_ignore_rows();
    let [project_settings_row, project_settings_lock_row, set_aside_row] = project_settings_rows
        .each_ref()
        .map(|(pattern, comment)| (pattern.as_str(), *comment));
    let project_settings_gitignore_pattern = project_settings_row.0;
    // #2718 - the Loop state rows, shared with the Loop scope's sweep.
    let loop_state_rows = crate::config::naming_migration::loop_state_ignore_rows();
    let [loop_state_row, loop_state_tmp_row, loop_set_aside_row] = loop_state_rows
        .each_ref()
        .map(|(pattern, comment)| (pattern.as_str(), *comment));
    // #2807 - the agent state rows, shared with the config pair's sweep.
    let config_state_rows = crate::config::naming_migration::config_state_ignore_rows();
    let [config_state_row, config_state_lock_row, config_state_tmp_row] = config_state_rows
        .each_ref()
        .map(|(pattern, comment)| (pattern.as_str(), *comment));
    const SEED_MANIFEST_COORDINATION_BLOCK: &str = "# AgentsCommander: exclude seed-manifest \
        coordination files.\n/.seed-manifest.lock\n/.seed-manifest.*.tmp\n";
    const SEED_MANIFEST_MANIFEST_BLOCK: &str = "# AgentsCommander: exclude the seed publication \
        manifest from Git tracking.\n/seed-manifest.toml\n";
    const SEED_MANIFEST_COORDINATION_PATTERNS: [&str; 2] =
        ["/.seed-manifest.lock", "/.seed-manifest.*.tmp"];
    const SEED_MANIFEST_MANIFEST_PATTERN: &str = "/seed-manifest.toml";

    // #1968 - the machine-local coding-agent catalog children, rooted at `.ac/`.
    let catalog = [
        CODING_AGENTS_LOCAL_ARTIFACT,
        CODING_AGENTS_MIGRATION_BACKUP_ARTIFACT,
        CODING_AGENTS_MIGRATION_JOURNAL_ARTIFACT,
        CODING_AGENTS_LOCK_ARTIFACT,
        CODING_AGENTS_BASE_TMP_ARTIFACT,
        CODING_AGENTS_LOCAL_TMP_ARTIFACT,
        CODING_AGENTS_MIGRATION_BACKUP_TMP_ARTIFACT,
        CODING_AGENTS_MIGRATION_JOURNAL_TMP_ARTIFACT,
        CODING_AGENTS_RETIRED_LOCK_ARTIFACT,
        CODING_AGENTS_SET_ASIDE_ARTIFACT,
    ]
    .map(|artifact| format!("/{artifact}"));
    let [catalog_local, catalog_migration_backup, catalog_migration_journal, ..] = &catalog;
    let [_, _, _, catalog_lock, catalog_base_tmp, catalog_local_tmp, ..] = &catalog;
    let [.., catalog_migration_backup_tmp, catalog_migration_journal_tmp, _, _] = &catalog;
    let [.., catalog_retired_lock, catalog_set_aside] = &catalog;

    // #2434 - configured instructions filenames beyond the floor, escaped.
    let mut custom_patterns: Vec<String> = Vec::new();
    for name in configured {
        if !crate::config::agent_command::is_safe_instructions_filename(name) {
            log::warn!(
                "[ac-gitignore] dropping unsafe instructions filename {:?}",
                name
            );
            continue;
        }
        if name == "CLAUDE.md" || name == "AGENTS.md" {
            continue;
        }
        let pattern = format!("**/__agent_*/{}", escape_gitignore_name(name));
        if !custom_patterns.contains(&pattern) {
            custom_patterns.push(pattern);
        }
    }

    // Each entry: (pattern, comment explaining why)
    let mut required_entries: Vec<(&str, &str)> = vec![
        (
            "room-*/",
            "# AgentsCommander: exclude room cloned repos from parent git tracking.\n# Without \
                this, parent repo operations (checkout, reset) corrupt child clones.",
        ),
        (
            "wg-*/",
            "# AgentsCommander: exclude legacy room cloned repos from parent git tracking.\n# \
                Without this, parent repo operations (checkout, reset) corrupt child clones.",
        ),
        (
            ".deleting-*/",
            "# AgentsCommander: exclude temporary room delete sentinels/orphans.",
        ),
        loop_state_row,
        loop_state_tmp_row,
        loop_set_aside_row,
        (
            "_loop_*/audit.jsonl",
            "# AgentsCommander: exclude Loop runtime audit logs with prompt snapshots.",
        ),
        (
            "**/__agent_*/last_ac_context.md",
            "# AgentsCommander: exclude managed session context files inside replica agent \
                folders.",
        ),
        (
            "**/__agent_*/CLAUDE.md",
            "# AgentsCommander: exclude managed session context files inside replica agent \
                folders.",
        ),
        (
            "**/__agent_*/AGENTS.md",
            "# AgentsCommander: exclude managed session context files inside replica agent \
                folders.",
        ),
        project_settings_row,
        (
            "/.team-config-write.lock",
            "# AgentsCommander: exclude team-config coordination files.",
        ),
        (
            "**/.config.json.lock",
            "# AgentsCommander: exclude local config write-lock sidecars.",
        ),
        (
            "/.project-settings.json.lock",
            "# AgentsCommander: exclude project-settings write-lock sidecars.",
        ),
        // #2717 - the migrated project settings take their own sidecar, and the
        // naming migration sets a displaced project file aside in `.ac/`.
        project_settings_lock_row,
        set_aside_row,
        (
            "_agent_*/rtk-matrix-history*.db",
            "# AgentsCommander: exclude RTK matrix-history databases.",
        ),
        (
            "_agent_*/rtk-matrix-history*.db-shm",
            "# AgentsCommander: exclude RTK matrix-history database sidecars.",
        ),
        (
            "_agent_*/rtk-matrix-history*.db-wal",
            "# AgentsCommander: exclude RTK matrix-history database sidecars.",
        ),
        (
            "_agent_*/rtk_ignored_tools*.md",
            "# AgentsCommander: exclude RTK ignored-tools runtime records.",
        ),
        (
            "_agent_*/rtk-ignored-tools-*.md",
            "# AgentsCommander: exclude RTK ignored-tools runtime records.",
        ),
        (
            "_agent_*/**/__pycache__/",
            "# AgentsCommander: exclude Python bytecode caches inside agent folders.",
        ),
        (
            "_agent_*/**/*.pyc",
            "# AgentsCommander: exclude Python bytecode inside agent folders.",
        ),
        // #1968 - the machine-local coding-agent catalog children. The tracked
        // `agents.json` and `_seed/` masters stay tracked; these are the
        // sidecars, lock and publication temporaries that must never be
        // committed. Composed from the registry: `config::instance_artifacts`
        // is a leaf with zero outgoing arcs, so an arc into it carries no path
        // back into the cyclic SCC, and modules across the tree, among them
        // `commands::config`, already hold that arc.
        (
            catalog_local.as_str(),
            "# AgentsCommander: exclude machine-local coding-agent overrides.",
        ),
        (
            catalog_migration_backup.as_str(),
            "# AgentsCommander: exclude the coding-agent migration backup (byte-exact \
                pre-migration source copy).",
        ),
        (
            catalog_migration_journal.as_str(),
            "# AgentsCommander: exclude the coding-agent migration journal.",
        ),
        (
            catalog_lock.as_str(),
            "# AgentsCommander: exclude the coding-agent catalog write-lock sidecar.",
        ),
        (
            catalog_base_tmp.as_str(),
            "# AgentsCommander: exclude coding-agent base publication temporaries.",
        ),
        (
            catalog_local_tmp.as_str(),
            "# AgentsCommander: exclude coding-agent local override publication temporaries.",
        ),
        (
            catalog_migration_backup_tmp.as_str(),
            "# AgentsCommander: exclude coding-agent migration backup publication temporaries.",
        ),
        (
            catalog_migration_journal_tmp.as_str(),
            "# AgentsCommander: exclude coding-agent migration journal publication temporaries \
                (the journal name starts with a dot).",
        ),
        // #2715 - the pre-migration lock sidecar the naming migration leaves on
        // disk for good, and every catalog file it sets aside.
        (
            catalog_retired_lock.as_str(),
            "# AgentsCommander: exclude the retired coding-agent catalog write-lock sidecar.",
        ),
        (
            catalog_set_aside.as_str(),
            "# AgentsCommander: exclude coding-agent catalog files the naming migration set aside.",
        ),
        // #2807 - the agent state file beside each tracked `config.json`, its
        // lock sidecar and its publish temporary.
        config_state_row,
        config_state_lock_row,
        config_state_tmp_row,
    ];
    for pattern in &custom_patterns {
        required_entries.push((
            pattern.as_str(),
            "# AgentsCommander: exclude managed session context files inside replica agent \
                folders.",
        ));
    }

    if gitignore_path.exists() {
        let content = std::fs::read_to_string(&gitignore_path)
            .map_err(|e| format!("Failed to read Project AC Root .gitignore: {}", e))?;
        let (content, migrated) = migrate_legacy_seed_manifest_gitignore(content);
        // #2715 - the append-only writer never removes a row, so the catalog
        // rows the naming migration renamed are retired here.
        let (content, retired) = crate::config::naming_migration::retire_ignore_pairs(
            &content,
            &[
                RETIRED_CATALOG_IGNORE_PAIRS.as_slice(),
                &RETIRED_PROJECT_SETTINGS_IGNORE_PAIRS,
                &RETIRED_LOOP_STATE_IGNORE_PAIRS,
            ]
            .concat(),
        );
        let migrated = migrated || retired;

        let mut additions = String::new();
        for (pattern, comment) in &required_entries {
            let is_present = content.lines().any(|line| {
                if *pattern == project_settings_gitignore_pattern {
                    line == *pattern
                } else {
                    line.trim() == *pattern
                }
            });
            if !is_present {
                additions.push_str(&format!("\n{}\n{}\n", comment, pattern));
            }
        }
        if !SEED_MANIFEST_COORDINATION_PATTERNS
            .iter()
            .all(|pattern| content.lines().any(|line| line.trim() == *pattern))
        {
            additions.push('\n');
            additions.push_str(SEED_MANIFEST_COORDINATION_BLOCK);
        }
        if !content
            .lines()
            .any(|line| line.trim() == SEED_MANIFEST_MANIFEST_PATTERN)
        {
            additions.push('\n');
            additions.push_str(SEED_MANIFEST_MANIFEST_BLOCK);
        }

        if migrated || !additions.is_empty() {
            let separator = if content.ends_with('\n') { "" } else { "\n" };
            std::fs::write(
                &gitignore_path,
                format!("{}{}{}", content, separator, additions),
            )
            .map_err(|e| format!("Failed to update Project AC Root .gitignore: {}", e))?;
        }
    } else {
        let mut content = String::new();
        for (pattern, comment) in &required_entries {
            content.push_str(&format!("{}\n{}\n\n", comment, pattern));
        }
        content.push_str(SEED_MANIFEST_COORDINATION_BLOCK);
        content.push('\n');
        content.push_str(SEED_MANIFEST_MANIFEST_BLOCK);
        std::fs::write(&gitignore_path, content)
            .map_err(|e| format!("Failed to create Project AC Root .gitignore: {}", e))?;
    }

    Ok(())
}

/// #2715 - the three `.ac/.gitignore` pairs whose value the `agents.*` rename
/// changed. The lock row is not here: its leftover stays on disk and ignored.
const RETIRED_CATALOG_IGNORE_PAIRS: [(&str, &str); 3] = [
    (
        "# AgentsCommander: exclude machine-local coding-agent overrides.",
        "/coding-agents/agents.local.json",
    ),
    (
        "# AgentsCommander: exclude coding-agent base publication temporaries.",
        "/coding-agents/.agents.json.*.tmp",
    ),
    (
        "# AgentsCommander: exclude coding-agent local override publication temporaries.",
        "/coding-agents/.agents.local.json.*.tmp",
    ),
];

/// #2717 - the `.ac/.gitignore` pair whose value the project-settings rename
/// changed. The lock row is not here: its leftover stays on disk and ignored.
const RETIRED_PROJECT_SETTINGS_IGNORE_PAIRS: [(&str, &str); 1] = [(
    "# AgentsCommander: exclude generated project-local settings.",
    "/project-settings.json",
)];

/// #2718 - the `.ac/.gitignore` pair whose value the Loop state rename changed.
const RETIRED_LOOP_STATE_IGNORE_PAIRS: [(&str, &str); 1] = [(
    "# AgentsCommander: exclude Loop scheduler runtime state.",
    "_loop_*/state.json",
)];

/// #2090 - migrate the retired `!/seed-manifest.toml` un-ignore pair written by
/// older builds. Only the two AC-managed lines (the retired comment directly
/// above the negation and, when the managed block wrote one, the blank line
/// before that comment) are removed; a bare `!/seed-manifest.toml` without the
/// retired comment is user intent and is preserved. Every other byte, including
/// CRLF and a missing final newline, is preserved; the flag is false when
/// nothing changed.
fn migrate_legacy_seed_manifest_gitignore(content: String) -> (String, bool) {
    const LEGACY_COMMENT: &str =
        "# AgentsCommander: keep the seed publication manifest reviewable.";
    const LEGACY_NEGATION: &str = "!/seed-manifest.toml";

    let lines: Vec<&str> = content.split_inclusive('\n').collect();
    let mut removed = vec![false; lines.len()];
    let mut changed = false;
    for (index, line) in lines.iter().enumerate() {
        if line.trim() != LEGACY_NEGATION {
            continue;
        }
        let Some(previous) = index.checked_sub(1) else {
            continue;
        };
        if lines[previous].trim() != LEGACY_COMMENT {
            continue;
        }
        removed[previous] = true;
        removed[index] = true;
        if let Some(blank) = previous.checked_sub(1) {
            if lines[blank].trim().is_empty() {
                removed[blank] = true;
            }
        }
        changed = true;
    }
    if !changed {
        return (content, false);
    }
    let migrated: String = lines
        .iter()
        .zip(&removed)
        .filter(|(_, drop)| !**drop)
        .map(|(line, _)| *line)
        .collect();
    (migrated, true)
}

/// Create a canonical .ac/ directory inside the given path.
#[tauri::command]
pub async fn create_ac_project(path: String) -> Result<(), String> {
    crate::config::seed_manifest::run_blocking_owned(move || {
        // #1065 Stage F: the bare fresh-root create publishes its project context
        // templates under the sole production token; a `#[cfg(test)]` lib build
        // stays non-emitting so unit fixtures keep manifest-free `.ac` roots.
        #[cfg(not(test))]
        let activation = Some(crate::config::seed_manifest::ManifestActivationToken::production());
        #[cfg(test)]
        let activation: Option<crate::config::seed_manifest::ManifestActivationToken> = None;
        create_ac_project_impl(&path, activation.as_ref(), |ac_root, on_publication| {
            crate::config::session_context::create_default_context_templates_with_publications(
                ac_root,
                on_publication,
            )
        })?;
        // #1318 - a fresh `.ac` root seeds the catalog + masters immediately
        // (no restart needed). INSIDE this blocking closure, so the gate acquire
        // runs on the blocking thread. Fail-soft: any error is logged and the
        // boot loop covers the project at the next boot.
        crate::config::coding_agents_catalog::ensure_seeded_for_project(Path::new(&path));
        Ok(())
    })
    .await
    .map_err(|error| format!("AC project blocking preparation failed: {error}"))?
}

fn create_ac_project_impl<F>(
    path: &str,
    activation: Option<&crate::config::seed_manifest::ManifestActivationToken>,
    create_context_templates: F,
) -> Result<(), String>
where
    F: FnOnce(
        &Path,
        &mut dyn FnMut(&'static str, crate::config::seeded_context_templates::ContextPublication),
    ) -> Result<(), String>,
{
    create_ac_project_impl_with_hook(path, activation, create_context_templates, |_, _| {})
}

fn create_ac_project_impl_with_hook<F, H>(
    path: &str,
    activation: Option<&crate::config::seed_manifest::ManifestActivationToken>,
    create_context_templates: F,
    after_create_before_gate: H,
) -> Result<(), String>
where
    F: FnOnce(
        &Path,
        &mut dyn FnMut(&'static str, crate::config::seeded_context_templates::ContextPublication),
    ) -> Result<(), String>,
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

    let ac_root = ac_root_for_project(&project_dir);
    let created = match std::fs::create_dir(&ac_root) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(error) => {
            return Err(format!(
                "Failed to create {} directory: {}",
                canonical_ac_root_label(),
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
    let pinned_ac_root =
        crate::config::seed_manifest::PinnedDirectory::open(&ac_root).map_err(|error| {
            format!(
                "Failed to pin {} directory {}: {}",
                canonical_ac_root_label(),
                ac_root.display(),
                error
            )
        })?;
    after_create_before_gate(&ac_root, created);
    pinned_project.revalidate().map_err(|error| {
        format!(
            "AC project setup at {} changed before gate acquisition: {}; retry the operation",
            project_dir.display(),
            error
        )
    })?;
    pinned_ac_root.revalidate().map_err(|error| {
        format!(
            "AC project setup at {} changed before gate acquisition: {}; retry the operation",
            project_dir.display(),
            error
        )
    })?;

    let mut guard = crate::config::seed_manifest::ProjectSeedManifestGuard::acquire(&project_dir)
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
    pinned_ac_root
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
        ensure_ac_root_gitignore(&ac_root)?;
        // #1065 Stage F: record each freshly created project context template under
        // the held gate at its commit-point time. A pre-existing `.ac` is not
        // backfilled (the `else` branch creates no templates), preserving the bare
        // create's no-backfill contract (acceptance item 38).
        let mut on_publication =
            |filename: &'static str,
             publication: crate::config::seeded_context_templates::ContextPublication| {
                if let Some(token) = activation {
                    crate::config::session_context::record_project_context_publication(
                        &mut guard,
                        token,
                        filename,
                        publication.published_at,
                    );
                }
            };
        create_context_templates(&ac_root, &mut on_publication)?;
    } else {
        ensure_ac_root_gitignore(&ac_root)?;
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

    let Some(ac_root) = existing_ac_root(base) else {
        return Ok(AcDiscoveryResult {
            agents: vec![],
            teams: vec![],
            workgroups: vec![],
            loops: vec![],
            context_template_updates: vec![],
        });
    };

    // Opportunistic: ensure gitignore protects workgroup clones
    let _ = ensure_ac_root_gitignore_with_names(
        &ac_root,
        &crate::config::agent_command::managed_instructions_filenames(&cfg),
    );
    let context_scan = crate::config::seed_manifest::run_blocking_owned({
        let base = base.to_path_buf();
        let ac_root = ac_root.clone();
        move || {
            // #1065 Stage F: production records recognized context updates under the
            // gate; a `#[cfg(test)]` lib build stays non-emitting.
            #[cfg(not(test))]
            let activation =
                Some(crate::config::seed_manifest::ManifestActivationToken::production());
            #[cfg(test)]
            let activation: Option<
                crate::config::seed_manifest::ManifestActivationToken,
            > = None;
            scan_project_context_templates_recorded(&base, &ac_root, activation.as_ref())
        }
    })
    .await;
    let mut context_template_updates = match context_scan {
        Ok(Ok(updates)) => updates,
        Ok(Err(e)) => {
            log::warn!(
                "[context-templates] scan failed for {}: {}",
                ac_root.display(),
                e
            );
            Vec::new()
        }
        Err(e) => {
            log::warn!(
                "[context-templates] blocking scan failed for {}: {}",
                ac_root.display(),
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

    let entries = match std::fs::read_dir(&ac_root) {
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

            let preferred_agent_id = read_preferred_agent_id(&entry_path, &cfg);

            agents.push(AcAgentMatrix {
                name: display_name,
                path: projected_path_string(&entry_path),
                role_exists,
                preferred_agent_id,
            });
        }

        // Workgroups: wg-*
        if crate::config::entity_prefix::has_entity_prefix(&dir_name) {
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
                                read_preferred_agent_id(&identity.matrix_dir, &cfg)
                            });
                        let current_coding_agent_id = read_replica_current_coding_agent(&wg_path)
                            .filter(|id| cfg.agents.iter().any(|agent| agent.id == *id));
                        let profile_read = read_replica_profile_result(&wg_path);
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
                            detect_repo_branch(&repo_paths[0]).await
                        } else {
                            None
                        };

                        // §AR2-strict: `is_coordinator` short-circuits on
                        // unqualified names, so build the project-qualified FQN
                        // (mirrors `agent_fqn_from_path`'s `<proj>:<wg>/<agent>`
                        // shape). Covered by
                        // teams::tests::is_any_coordinator_requires_qualified_fqn.
                        let is_coordinator = crate::config::teams::resolve_wg_coordinator_replica(
                            &ac_root,
                            &entry_path,
                        )
                        .map(|resolved| resolved.agent_name == replica_name)
                        .unwrap_or(false);

                        log::debug!(
                            "[ac-discovery] call={} replica — project='{}' wg='{}' replica='{}' \
                                fqn='{}:{}/{}' is_coordinator={}",
                            call_id,
                            project_folder,
                            dir_name,
                            replica_name,
                            project_folder,
                            dir_name,
                            replica_name,
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

                        // #1941 - strict protection state from the same validated
                        // replica directory; an invalid state is surfaced as
                        // `invalid`, never as default false.
                        let (saved_pair, selection_state, selection_error) =
                            replica_selection_wire_fields(&wg_path);

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
                            saved_pair,
                            selection_state,
                            selection_error,
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
                    "[discovery] Workgroup '{}' → team '{}' (matched via name suffix, identity may \
                        be missing)",
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
        "[ac-discovery] call={} discover_project: summary — path='{}' workgroups={} teams={} \
            replicas={} coordinator={}",
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
    let ac_root = existing_ac_root(Path::new(&path))
        .ok_or_else(|| format!("Project AC Root not found for {}", path))?;
    crate::config::seeded_context_templates::dismiss_context_template_update(
        &ac_root,
        &filename,
        &current_file_sha256,
        &current_default_sha256,
    )
}

/// #1065 Stage F: explicit context-template overwrite recorded under the gate.
///
/// Overwrite is a user-initiated action, so a contention/unsafe gate failure
/// returns a retryable busy error (never a silent skip); a pre-contention gate
/// degradation overwrites untracked. The publication is recorded at the commit
/// point via the callback, before `persist_state_strict`, so a strict state-save
/// error still returns an IPC error without losing the manifest attempt (plan
/// section 5.2). `activation` is `None` for a `#[cfg(test)]` build or an
/// unactivated caller, which runs the plain ungated overwrite unchanged.
fn overwrite_context_template_recorded(
    project_root: &Path,
    ac_root: &Path,
    filename: &str,
    expected_file_sha256: &str,
    expected_default_sha256: &str,
    activation: Option<&crate::config::seed_manifest::ManifestActivationToken>,
) -> Result<crate::config::seeded_context_templates::ContextTemplateOverwriteResult, String> {
    use crate::config::seed_manifest::SoftProjectGate;
    use crate::config::seeded_context_templates::{
        overwrite_context_template_with_default,
        overwrite_context_template_with_default_with_publications, ContextPublication,
    };
    let Some(token) = activation else {
        return overwrite_context_template_with_default(
            ac_root,
            filename,
            expected_file_sha256,
            expected_default_sha256,
        );
    };
    match crate::config::seed_manifest::acquire_project_gate_soft(project_root) {
        SoftProjectGate::Held(mut guard) => {
            let execution = {
                let mut on_publication = |fname: &'static str, publication: ContextPublication| {
                    crate::config::session_context::record_project_context_publication(
                        &mut guard,
                        token,
                        fname,
                        publication.published_at,
                    );
                };
                overwrite_context_template_with_default_with_publications(
                    ac_root,
                    filename,
                    expected_file_sha256,
                    expected_default_sha256,
                    &mut on_publication,
                )
            };
            guard.release();
            execution.completion
        }
        SoftProjectGate::DegradedUntracked => overwrite_context_template_with_default(
            ac_root,
            filename,
            expected_file_sha256,
            expected_default_sha256,
        ),
        SoftProjectGate::Unavailable(error) => Err(format!(
            "Project is busy; retry the context template overwrite ({error})"
        )),
    }
}

#[tauri::command]
pub async fn overwrite_context_template_with_default(
    path: String,
    filename: String,
    current_file_sha256: String,
    current_default_sha256: String,
) -> Result<crate::config::seeded_context_templates::ContextTemplateOverwriteResult, String> {
    let ac_root = existing_ac_root(Path::new(&path))
        .ok_or_else(|| format!("Project AC Root not found for {}", path))?;
    crate::config::seed_manifest::run_blocking_owned(move || {
        // #1065 Stage F: production records the overwrite under the gate; a
        // `#[cfg(test)]` lib build stays non-emitting.
        #[cfg(not(test))]
        let activation = Some(crate::config::seed_manifest::ManifestActivationToken::production());
        #[cfg(test)]
        let activation: Option<crate::config::seed_manifest::ManifestActivationToken> = None;
        overwrite_context_template_recorded(
            Path::new(&path),
            &ac_root,
            &filename,
            &current_file_sha256,
            &current_default_sha256,
            activation.as_ref(),
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

    crate::config::agent_config::update_agent_config(&config_path, |obj, _state| {
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

async fn mutate_project_paths_with_settings_path<T: Send + 'static>(
    settings: &SettingsState,
    settings_path: Option<&Path>,
    mutate: impl FnOnce(&mut AppSettings) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let path = match settings_path {
        Some(path) => path.to_path_buf(),
        None => crate::config::config_dir()
            .ok_or("Could not determine settings directory")?
            .join(SETTINGS_FILE_NAME),
    };
    let settings = std::sync::Arc::clone(settings);
    crate::config::seed_manifest::run_blocking_owned(move || {
        let mut current = settings.blocking_write();
        let mut transaction =
            crate::config::settings::begin_project_paths_transaction_at_path(&current, &path)?;
        let mut candidate = transaction.fresh_settings().clone();
        transaction.refresh_raw_projects(&mut candidate)?;
        if crate::config::settings::project_state_has_structural(&candidate) {
            return Err("project-paths.json has malformed project metadata".to_string());
        }
        let result = mutate(&mut candidate)?;
        crate::config::settings::resync_project_state_from_runtime(&mut candidate);
        match transaction.commit_project_groups(&candidate, true, true) {
            Ok(written) => {
                *current = written;
                Ok(result)
            }
            Err(error) => {
                *current = transaction.fresh_settings().clone();
                Err(error)
            }
        }
    })
    .await
    .map_err(|error| error.to_string())?
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
    // #1318 - a first open of an unseeded project seeds the catalog + masters
    // immediately (no restart needed). The gate acquire (kernel lock, up to
    // DEFAULT_LOCK_TIMEOUT 10s polling) must run on a blocking thread, never on
    // the async executor, hence run_blocking_owned. Fail-soft: any error is
    // logged and the boot loop covers the project at the next boot.
    if result.registered {
        let path = result.path.clone();
        if let Err(error) = crate::config::seed_manifest::run_blocking_owned(move || {
            crate::config::coding_agents_catalog::ensure_seeded_for_project(Path::new(&path));
        })
        .await
        {
            log::warn!(
                "[ac-discovery] catalog seed after open-project failed: {}",
                error
            );
        }
    }
    Ok(result)
}

async fn open_project_inner_with_settings_path(
    settings: &SettingsState,
    path: &str,
    settings_path: Option<&Path>,
) -> Result<(crate::config::projects::ProjectRegistration, bool), String> {
    let mutation_path = path.to_string();
    mutate_project_paths_with_settings_path(settings, settings_path, move |s| {
        let path = mutation_path.as_str();
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
        transaction: &mut crate::config::settings::ProjectPathsWriteTransaction,
    ) -> Result<crate::config::settings::AppSettings, String> {
        #[cfg(test)]
        if self.fail_settings_save {
            return Err("injected new-project settings save failure".to_string());
        }
        transaction.commit_project_groups(settings, true, true)
    }

    fn before_final_revalidation(&self, ac_root: &Path) {
        #[cfg(test)]
        if let Some(hook) = &self.before_final_revalidation {
            hook(ac_root);
        }
        #[cfg(not(test))]
        let _ = ac_root;
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
        let settings_path = match settings_path {
            Some(path) => path,
            None => crate::config::config_dir()
                .ok_or(
                    "Could not determine settings \
                directory",
                )?
                .join(SETTINGS_FILE_NAME),
        };
        let mut transaction = crate::config::settings::begin_project_paths_transaction_at_path(
            &current,
            &settings_path,
        )?;
        let before_raw = transaction.raw_project_projection()?;
        let mut candidate = transaction.fresh_settings().clone();
        transaction.refresh_raw_projects(&mut candidate)?;
        if crate::config::settings::project_state_has_structural(&candidate) {
            return Err("project-paths.json has malformed project metadata".to_string());
        }
        let before_archived = candidate.archived_project_paths.clone();
        let result =
            crate::config::projects::commit_prepared_new_project(&mut candidate, &prepared);
        crate::config::settings::resync_project_state_from_runtime(&mut candidate);
        prepared.revalidate().map_err(|error| error.to_string())?;
        let written = match hooks.save_settings(&candidate, &mut transaction) {
            Ok(written) => written,
            Err(error) => {
                *current = transaction.fresh_settings().clone();
                return Err(error);
            }
        };
        hooks.before_final_revalidation(prepared.ac_root());
        if let Err(error) = prepared.revalidate() {
            return Err(
                match transaction.restore_raw_project_projection(&before_raw) {
                    Ok(written) => {
                        *current = written;
                        error.to_string()
                    }
                    Err(rollback_error) => {
                        *current = transaction.fresh_settings().clone();
                        current.project_paths_persistence_error = Some(rollback_error.clone());
                        format!(
                            "{error}; failed to roll back project registration settings: \
                        {rollback_error}"
                        )
                    }
                },
            );
        }
        *current = written;
        drop(transaction);
        let archived_changed = before_archived != current.archived_project_paths;
        drop(current);
        // #1318 - capture the project root BEFORE release (`abs` is private;
        // `ac_root()` returns the `.ac` dir).
        let project_root = prepared.ac_root().parent().map(Path::to_path_buf);
        prepared.release();
        // #1318 - a fresh registration seeds the catalog + masters immediately
        // (no restart needed). INSIDE this blocking closure, so the gate acquire
        // runs on the blocking thread. Fail-soft: any error is logged and the
        // boot loop covers the project at the next boot.
        if let Some(root) = project_root {
            crate::config::coding_agents_catalog::ensure_seeded_for_project(&root);
        }
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
    let mutation_path = path.to_string();
    mutate_project_paths_with_settings_path(settings, settings_path, move |s| {
        let path = mutation_path.as_str();
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
            "Cannot archive: {} open session(s) in this project ({}, and {} more). Close them \
                first.",
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

    let mutation_path = path.to_string();
    mutate_project_paths_with_settings_path(settings, settings_path, move |s| {
        let path = mutation_path.as_str();
        crate::config::projects::archive_project_path(s, path);
        Ok(())
    })
    .await?;

    let after = snapshot_archive_state(session_mgr, pty_mgr).await;
    let late = archive_blockers(&after, &path);
    if late.is_empty() {
        broadcast_project_archive_changed(app, &path, true, ArchiveChangeReason::Archive, None);
        return Ok(());
    }

    let mutation_path = path.clone();
    let restored: Result<bool, String> =
        mutate_project_paths_with_settings_path(settings, settings_path, move |s| {
            let path = mutation_path.as_str();
            let key = crate::config::projects::normalize_for_compare(path);
            let still_archived = s
                .archived_project_paths
                .iter()
                .any(|p| crate::config::projects::normalize_for_compare(p) == key);
            if !still_archived {
                return Ok(false);
            }
            Ok(crate::config::projects::unarchive_project_path(s, path))
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
            // #1318 - the rollback re-registered the project; seed its catalog
            // now so reads target the project `.ac` without a restart.
            let seed_path = path.clone();
            if let Err(error) = crate::config::seed_manifest::run_blocking_owned(move || {
                crate::config::coding_agents_catalog::ensure_seeded_for_project(Path::new(
                    &seed_path,
                ));
            })
            .await
            {
                log::warn!(
                    "[ac-discovery] catalog seed after archive rollback failed: {}",
                    error
                );
            }
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
                "{} The project could not be restored automatically ({}). Open Archived Projects \
                    to restore it.",
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
    let mutation_path = path.to_string();
    let result = mutate_project_paths_with_settings_path(settings, None, move |s| {
        crate::config::projects::register_existing_project(s, &mutation_path)
            .map_err(|e| e.to_string())
    })
    .await?;
    broadcast_project_archive_changed(
        app,
        &result.path,
        false,
        ArchiveChangeReason::Unarchive,
        None,
    );
    // #1318 - unarchive is a re-registration; seed the catalog + masters now so
    // reads target the project `.ac` without a restart (the boot loop covers it
    // at the next boot either way). Fail-soft; gate acquire on a blocking thread.
    if result.registered {
        let path = result.path.clone();
        if let Err(error) = crate::config::seed_manifest::run_blocking_owned(move || {
            crate::config::coding_agents_catalog::ensure_seeded_for_project(Path::new(&path));
        })
        .await
        {
            log::warn!(
                "[ac-discovery] catalog seed after unarchive failed: {}",
                error
            );
        }
    }
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
mod tests;
