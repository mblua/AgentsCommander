use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use serde::Serialize;

use crate::cli::create_agent_matrix::{write_project_refresh_request, ProjectRefreshRequest};
use crate::commands::entity_creation::{
    acquire_lifecycle_project_gate, check_workgroup_repos_dirty, clone_missing_repos_for_workgroup,
    create_workgroup_on_disk, list_workgroup_dirs, parse_task_title, prune_workgroup_config_scope,
    read_team_config, resolve_agent_ref, sanitize_name, validate_delete_root_not_link_or_reparse,
    validate_existing_name, RepoAssignment, TeamConfigResult, WgDeleteOutcome,
    WorkgroupDiskCreateArgs,
};
use crate::config::ac_root::existing_ac_root;
use crate::config::daemon_pid::{detect_daemon_state, DaemonState};
use crate::config::projects::resolve_project_reference;
use crate::config::remote_activity_cache::{
    read_snapshot, snapshot_is_fresh, PersistedCiState, PersistedRepoCi, RemoteActivitySnapshot,
    REMOTE_ACTIVITY_SNAPSHOT_FILE_NAME, REMOTE_ACTIVITY_SNAPSHOT_MAX_AGE_SECS,
};
use crate::config::sessions_persistence::{load_sessions_raw, PersistedSession};
use crate::session::session::persisted_is_working;

#[derive(Args)]
pub struct WorkgroupArgs {
    #[command(subcommand)]
    command: WorkgroupCommand,
}

#[derive(Subcommand)]
enum WorkgroupCommand {
    /// List rooms in a project
    List(WorkgroupListArgs),
    /// Show each room's working state, CI state and task title
    Activity(WorkgroupActivityArgs),
    /// Create an auto-numbered room
    Add(WorkgroupAddArgs),
    /// Remove a room
    Remove(WorkgroupRemoveArgs),
}

#[derive(Args)]
struct WorkgroupActivityArgs {
    #[arg(long)]
    project: String,
    /// Keep only these room numbers (comma-separated, e.g. 5,12,17)
    #[arg(long, allow_hyphen_values = true)]
    rooms: Option<String>,
    /// Keep only rooms of this team (exact name)
    #[arg(long, allow_hyphen_values = true)]
    team: Option<String>,
    /// Keep only rooms whose working state is true or false
    #[arg(long, allow_hyphen_values = true)]
    working: Option<String>,
    /// Keep only rooms whose ciState is running, idle or unknown
    #[arg(long = "ci-state", allow_hyphen_values = true)]
    ci_state: Option<String>,
    /// Keep only rooms whose task title matches this regex (case-sensitive; (?i) for insensitive)
    #[arg(long = "title-regex", allow_hyphen_values = true)]
    title_regex: Option<String>,
    /// Drop rooms whose task title is missing, blank or "clean" (any case)
    #[arg(long = "hide-clean")]
    hide_clean: bool,
}

#[derive(Args)]
struct WorkgroupListArgs {
    #[arg(long)]
    project: String,
}

#[derive(Args)]
struct WorkgroupAddArgs {
    #[arg(long)]
    project: String,
    #[arg(long)]
    team: String,
    #[arg(long)]
    title: String,
    #[arg(long, hide = true)]
    coordinator: Option<String>,
    #[arg(long = "agent", hide = true)]
    agents: Vec<String>,
    #[arg(long = "repo", hide = true)]
    repos: Vec<String>,
    #[arg(long = "repo-agents", hide = true)]
    repo_agents: Vec<String>,
    #[arg(long = "repo-exclude-agents", hide = true)]
    repo_exclude_agents: Vec<String>,
}

#[derive(Args)]
struct WorkgroupRemoveArgs {
    #[arg(long)]
    project: String,
    #[arg(long = "room", alias = "workgroup", value_name = "ROOM")]
    workgroup: String,
    #[arg(long = "force-dirty")]
    force_dirty: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkgroupListItem {
    name: String,
    team: String,
    path: String,
    has_messaging: bool,
    has_task: bool,
    replicas: Vec<String>,
}

/// One room's aggregate activity. `working` is a persisted observation only,
/// never a proxy: an idle coordinator does not make its room work. `ci_state`
/// is exactly `running`, `idle` or `unknown`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkgroupActivityItem {
    name: String,
    team: String,
    working: bool,
    ci_state: &'static str,
    task_title: Option<String>,
    /// #2473: sorted, deduped in-progress run ids; non-empty only when running.
    ci_run_ids: Vec<u64>,
    /// #2473: sorted, deduped PR numbers of those runs; non-empty only when running.
    ci_pull_requests: Vec<u64>,
    /// #2473: why `ci_state` is `unknown`; `None` otherwise.
    ci_unknown_reason: Option<&'static str>,
}

/// The parsed `room activity` filters (#2473). Every present filter must match
/// (AND); an absent one matches everything.
#[derive(Debug, Default)]
struct ActivityFilter {
    rooms: Option<BTreeSet<u32>>,
    team: Option<String>,
    working: Option<bool>,
    ci_state: Option<&'static str>,
    title_regex: Option<regex::Regex>,
    hide_clean: bool,
}

impl ActivityFilter {
    fn matches(&self, item: &WorkgroupActivityItem) -> bool {
        if let Some(rooms) = &self.rooms {
            if !room_number(&item.name).is_some_and(|number| rooms.contains(&number)) {
                return false;
            }
        }
        if let Some(team) = &self.team {
            if item.team != *team {
                return false;
            }
        }
        if let Some(working) = self.working {
            if item.working != working {
                return false;
            }
        }
        if let Some(ci_state) = self.ci_state {
            if item.ci_state != ci_state {
                return false;
            }
        }
        if let Some(regex) = &self.title_regex {
            if !item
                .task_title
                .as_deref()
                .is_some_and(|title| regex.is_match(title))
            {
                return false;
            }
        }
        if self.hide_clean && title_is_clean(item.task_title.as_deref()) {
            return false;
        }
        true
    }
}

/// `--hide-clean`: a missing title, a blank one, or `clean` in any ASCII case
/// after trimming.
fn title_is_clean(title: Option<&str>) -> bool {
    match title.map(str::trim) {
        None => true,
        Some(title) => title.is_empty() || title.eq_ignore_ascii_case("clean"),
    }
}

/// The N of `room-N-<team>` / `wg-N-<team>`; `None` for any other shape.
fn room_number(name: &str) -> Option<u32> {
    let rest = crate::config::entity_prefix::strip_entity_prefix(name)?;
    let (number, _) = rest.split_once('-')?;
    number.parse::<u32>().ok()
}

/// Validate every filter value in a fixed order (rooms, team, working,
/// ci-state, title-regex); the first failure is the error.
fn parse_activity_filter(args: &WorkgroupActivityArgs) -> Result<ActivityFilter, String> {
    let rooms = match &args.rooms {
        None => None,
        Some(raw) => {
            let mut set = BTreeSet::new();
            for item in raw.split(',') {
                match item.trim().parse::<u32>() {
                    Ok(number) if number > 0 => {
                        set.insert(number);
                    }
                    _ => {
                        return Err(format!(
                            "Invalid --rooms '{raw}': expected comma-separated positive room numbers, e.g. 5,12,17"
                        ))
                    }
                }
            }
            Some(set)
        }
    };
    let team = match &args.team {
        None => None,
        Some(raw) if raw.trim().is_empty() => {
            return Err(format!("Invalid --team '{raw}': expected a team name"))
        }
        Some(raw) => Some(raw.clone()),
    };
    let working = match args.working.as_deref() {
        None => None,
        Some("true") => Some(true),
        Some("false") => Some(false),
        Some(raw) => return Err(format!("Invalid --working '{raw}': expected true or false")),
    };
    let ci_state = match args.ci_state.as_deref() {
        None => None,
        Some("running") => Some("running"),
        Some("idle") => Some("idle"),
        Some("unknown") => Some("unknown"),
        Some(raw) => {
            return Err(format!(
                "Invalid --ci-state '{raw}': expected running, idle or unknown"
            ))
        }
    };
    let title_regex = match &args.title_regex {
        None => None,
        Some(raw) => Some(
            regex::Regex::new(raw).map_err(|e| format!("Invalid --title-regex '{raw}': {e}"))?,
        ),
    };
    Ok(ActivityFilter {
        rooms,
        team,
        working,
        ci_state,
        title_regex,
        hide_clean: args.hide_clean,
    })
}

pub fn execute(args: WorkgroupArgs) -> i32 {
    let result = match args.command {
        WorkgroupCommand::List(args) => list(args),
        WorkgroupCommand::Activity(args) => activity(args),
        WorkgroupCommand::Add(args) => add(args),
        WorkgroupCommand::Remove(args) => remove(args),
    };
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    }
}

pub(crate) fn resolve_cli_project(project: &str) -> Result<PathBuf, String> {
    let settings = crate::config::settings::load_settings_for_cli();
    let resolved =
        resolve_project_reference(&settings.project_paths, project).map_err(|e| e.to_string())?;
    Ok(resolved.path)
}

pub(crate) fn resolve_cli_ac_root(project_path: &Path) -> Result<PathBuf, String> {
    existing_ac_root(project_path).ok_or_else(|| {
        format!(
            "Project AC Root not found in {} (.ac)",
            project_path.display()
        )
    })
}

pub(crate) fn write_refresh(project_path: &Path, changed_path: &Path, name: &str, reason: &str) {
    let canonical_project_path =
        std::fs::canonicalize(project_path).unwrap_or_else(|_| project_path.to_path_buf());
    let canonical_changed_path =
        std::fs::canonicalize(changed_path).unwrap_or_else(|_| changed_path.to_path_buf());
    let request = ProjectRefreshRequest {
        id: uuid::Uuid::new_v4().to_string(),
        project_path: canonical_project_path.to_string_lossy().to_string(),
        changed_path: Some(canonical_changed_path.to_string_lossy().to_string()),
        changed_name: Some(name.to_string()),
        reason: reason.to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    };
    if let Err(e) = write_project_refresh_request(&request) {
        eprintln!("Warning: failed to request project refresh: {}", e);
    }
}

pub(crate) fn write_project_registration_refresh(project_path: &Path, reason: &str) {
    let canonical_project_path =
        std::fs::canonicalize(project_path).unwrap_or_else(|_| project_path.to_path_buf());
    let request = ProjectRefreshRequest {
        id: uuid::Uuid::new_v4().to_string(),
        project_path: canonical_project_path.to_string_lossy().to_string(),
        changed_path: None,
        changed_name: None,
        reason: reason.to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    };
    if let Err(e) = write_project_refresh_request(&request) {
        eprintln!("Warning: failed to request project refresh: {}", e);
    }
}

fn list(args: WorkgroupListArgs) -> Result<(), String> {
    let project_path = resolve_cli_project(&args.project)?;
    let ac_root = resolve_cli_ac_root(&project_path)?;
    let items: Vec<WorkgroupListItem> = list_workgroup_dirs(&ac_root)
        .into_iter()
        .filter_map(|path| {
            let name = path.file_name()?.to_str()?.to_string();
            let team =
                crate::commands::entity_creation::parse_team_from_workgroup_name(&name).ok()?;
            let replicas = list_replicas(&path);
            Some(WorkgroupListItem {
                name,
                team,
                path: path.to_string_lossy().to_string(),
                has_messaging: path
                    .join(crate::phone::messaging::MESSAGING_DIR_NAME)
                    .is_dir(),
                has_task: path.join("TASK.md").is_file(),
                replicas,
            })
        })
        .collect();
    print_json(&items)
}

/// `room activity` / `workgroup activity`: every room of the project, in the
/// same enumeration order `room list` uses, with aggregate working state, CI
/// state and TASK title. Read-only: no TASK, cache, session, daemon or project
/// write belongs to this verb.
fn activity(args: WorkgroupActivityArgs) -> Result<(), String> {
    let filter = parse_activity_filter(&args)?;
    let project_path = resolve_cli_project(&args.project)?;
    let ac_root = resolve_cli_ac_root(&project_path)?;
    // One instant for the whole run, captured before the cache read, so every
    // room is judged against the same freshness boundary.
    let now = chrono::Utc::now();
    let settings = crate::config::settings::load_settings_for_cli();
    let daemon_state = detect_daemon_state();

    let (ci_entries, ci_gate_reason): (HashMap<String, IndexedRepoCi>, Option<&'static str>) =
        match ci_gate(
            settings.ci_activity_enabled,
            daemon_state_is_live(&daemon_state),
        ) {
            CiGate::CiDisabled => {
                warn_activity(
                    "room activity: CI activity is disabled; ciState is reported as \"unknown\"",
                );
                (HashMap::new(), Some("ci-disabled"))
            }
            CiGate::DaemonNotLive => {
                warn_activity(&format!(
                "room activity: AgentsCommander daemon is not live ({daemon_state:?}); ciState is reported as \"unknown\""
            ));
                (HashMap::new(), Some("daemon-not-live"))
            }
            CiGate::Usable => {
                let cache_dir = crate::config::config_dir();
                match read_fresh_ci_snapshot(cache_dir.as_deref(), now) {
                    Ok(snapshot) => (index_ci_entries(&snapshot), None),
                    Err(reason) => {
                        warn_activity(&format!(
                            "room activity: {reason}; ciState is reported as \"unknown\""
                        ));
                        (HashMap::new(), Some("snapshot-unavailable"))
                    }
                }
            }
        };

    let sessions = load_sessions_raw();
    let mut items: Vec<WorkgroupActivityItem> = Vec::new();
    for path in list_workgroup_dirs(&ac_root) {
        let Some(name) = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_string)
        else {
            continue;
        };
        let Ok(team) = crate::commands::entity_creation::parse_team_from_workgroup_name(&name)
        else {
            continue;
        };
        let working = room_is_working(&path, &name, &sessions);
        let repo_keys: Vec<String> = list_room_repo_dirs(&path)
            .iter()
            .map(|repo| canonical_activity_path_key(repo))
            .collect();
        let ci = aggregate_room_ci(&repo_keys, &ci_entries, ci_gate_reason);
        let task_title = read_task_title(&path);
        items.push(WorkgroupActivityItem {
            name,
            team,
            working,
            ci_state: ci.state,
            task_title,
            ci_run_ids: ci.run_ids,
            ci_pull_requests: ci.pull_requests,
            ci_unknown_reason: ci.unknown_reason,
        });
    }
    items.retain(|item| filter.matches(item));
    print_json(&items)
}

/// The three states of the CI gate, evaluated once per invocation before any
/// cache file is read. Named so the warning and the tests can tell the reasons
/// apart instead of collapsing them into one silent boolean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CiGate {
    /// The snapshot may be read (it still has to be current).
    Usable,
    /// `ciActivityEnabled` is false: no producer exists to trust.
    CiDisabled,
    /// The daemon is not live: the snapshot is not being refreshed.
    DaemonNotLive,
}

fn ci_gate(ci_activity_enabled: bool, daemon_live: bool) -> CiGate {
    if !ci_activity_enabled {
        CiGate::CiDisabled
    } else if !daemon_live {
        CiGate::DaemonNotLive
    } else {
        CiGate::Usable
    }
}

fn daemon_state_is_live(state: &DaemonState) -> bool {
    matches!(state, DaemonState::Running { .. })
}

/// One runtime warning. When `AC_MACHINE_OUTPUT` is set the caller asked for
/// log-only warnings; otherwise stderr is the channel. stdout is never used.
fn warn_activity(message: &str) {
    if std::env::var_os("AC_MACHINE_OUTPUT").is_some() {
        log::warn!("{message}");
    } else {
        eprintln!("{message}");
    }
}

/// Read and freshness-check the snapshot at `<config_dir>/remote-activity.json`.
/// A missing directory, an unreadable or malformed file, an unsupported schema,
/// an invalid timestamp and a stale (or far-future) one are all `Err` with the
/// reason the caller warns about; none is a panic.
fn read_fresh_ci_snapshot(
    dir: Option<&Path>,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<RemoteActivitySnapshot, String> {
    let dir = dir.ok_or_else(|| "the instance config directory is unavailable".to_string())?;
    let path = dir.join(REMOTE_ACTIVITY_SNAPSHOT_FILE_NAME);
    let snapshot = read_snapshot(&path)?;
    if !snapshot_is_fresh(&snapshot, now) {
        return Err(format!(
            "the remote activity snapshot is not current (window {REMOTE_ACTIVITY_SNAPSHOT_MAX_AGE_SECS}s)"
        ));
    }
    Ok(snapshot)
}

/// One canonical key's CI view after duplicate folding (#2473).
#[derive(Debug, Clone, PartialEq)]
struct IndexedRepoCi {
    ci: PersistedRepoCi,
    /// Two entries for this key disagreed on `state`.
    conflicting: bool,
}

/// Canonical-key index over the snapshot's path/CI pairs. A duplicate key
/// with conflicting states collapses to `Unknown`: two observations that
/// disagree are not authority. Duplicates with the SAME state are not a
/// conflict: run ids / PRs are unioned and the first reason is kept, so
/// metadata can never change the state.
fn index_ci_entries(snapshot: &RemoteActivitySnapshot) -> HashMap<String, IndexedRepoCi> {
    let mut entries: HashMap<String, IndexedRepoCi> = HashMap::new();
    for (path, ci) in &snapshot.repos {
        let key = canonical_activity_path_key(Path::new(path));
        match entries.entry(key) {
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(IndexedRepoCi {
                    ci: ci.clone(),
                    conflicting: false,
                });
            }
            std::collections::hash_map::Entry::Occupied(mut slot) => {
                let existing = slot.get_mut();
                if existing.conflicting {
                    continue;
                }
                if existing.ci.state != ci.state {
                    *existing = IndexedRepoCi {
                        ci: PersistedRepoCi {
                            state: PersistedCiState::Unknown,
                            run_ids: Vec::new(),
                            pull_requests: Vec::new(),
                            unknown_reason: None,
                        },
                        conflicting: true,
                    };
                } else {
                    existing.ci.run_ids.extend_from_slice(&ci.run_ids);
                    existing
                        .ci
                        .pull_requests
                        .extend_from_slice(&ci.pull_requests);
                    if existing.ci.unknown_reason.is_none() {
                        existing.ci.unknown_reason = ci.unknown_reason.clone();
                    }
                }
            }
        }
    }
    entries
}

/// A room's aggregate CI view (#2473): the legacy tri-state plus its detail.
#[derive(Debug, Clone, PartialEq)]
struct RoomCi {
    state: &'static str,
    run_ids: Vec<u64>,
    pull_requests: Vec<u64>,
    unknown_reason: Option<&'static str>,
}

/// The producer's persisted `ci-query-*` codes; anything else in the file is
/// reported as a plain explicit unknown.
const PERSISTED_CI_REASONS: [&str; 6] = [
    "ci-query-timeout",
    "ci-query-rate-limited",
    "ci-query-secondary-rate-limited",
    "ci-query-not-authenticated",
    "ci-query-incomplete",
    "ci-query-failed",
];

/// Why one repo is neither idle nor running; `None` when it is.
fn repo_unknown_reason(entry: Option<&IndexedRepoCi>) -> Option<&'static str> {
    let Some(entry) = entry else {
        return Some("repo-not-in-snapshot");
    };
    if entry.conflicting {
        return Some("conflicting-snapshot-entries");
    }
    match entry.ci.state {
        PersistedCiState::Running | PersistedCiState::Idle => None,
        PersistedCiState::Unknown => Some(
            entry
                .ci
                .unknown_reason
                .as_deref()
                .and_then(|reason| {
                    PERSISTED_CI_REASONS
                        .iter()
                        .copied()
                        .find(|code| *code == reason)
                })
                .unwrap_or("repo-ci-unknown"),
        ),
    }
}

/// The room's CI state: `running` if any room repo is running; `idle` only if
/// the room has at least one immediate `repo-*` and EVERY repo has a matched
/// idle entry; `unknown` for no repositories, any missing match, or any
/// explicit unknown unless another repository is running. Pure, so the whole
/// tri-state is unit-tested without files.
///
/// #2473: `gate_reason` is the gate/snapshot reason (`ci-disabled`,
/// `daemon-not-live`, `snapshot-unavailable`) when no entry could be read. Run
/// ids / PRs are reported only when running; the reason only when unknown.
fn aggregate_room_ci(
    repo_keys: &[String],
    entries: &HashMap<String, IndexedRepoCi>,
    gate_reason: Option<&'static str>,
) -> RoomCi {
    let mut any_running = false;
    let mut all_idle = !repo_keys.is_empty();
    let mut run_ids = BTreeSet::new();
    let mut pull_requests = BTreeSet::new();
    for key in repo_keys {
        match entries.get(key).map(|entry| entry.ci.state) {
            Some(PersistedCiState::Running) => {
                any_running = true;
                let ci = &entries[key].ci;
                run_ids.extend(ci.run_ids.iter().copied());
                pull_requests.extend(ci.pull_requests.iter().copied());
            }
            Some(PersistedCiState::Idle) => {}
            Some(PersistedCiState::Unknown) | None => all_idle = false,
        }
    }
    if any_running {
        return RoomCi {
            state: "running",
            run_ids: run_ids.into_iter().collect(),
            pull_requests: pull_requests.into_iter().collect(),
            unknown_reason: None,
        };
    }
    if all_idle {
        return RoomCi {
            state: "idle",
            run_ids: Vec::new(),
            pull_requests: Vec::new(),
            unknown_reason: None,
        };
    }
    let unknown_reason = gate_reason.or_else(|| {
        if repo_keys.is_empty() {
            return Some("no-repos");
        }
        let mut sorted: Vec<&String> = repo_keys.iter().collect();
        sorted.sort();
        sorted
            .into_iter()
            .find_map(|key| repo_unknown_reason(entries.get(key)))
    });
    RoomCi {
        state: "unknown",
        run_ids: Vec::new(),
        pull_requests: Vec::new(),
        unknown_reason,
    }
}

/// The path identity applied to EVERY operand of a room/session/cache match:
/// `canonicalize` first (resolves `.`/`..` and symlinks when the path exists),
/// the original bytes on failure, then the four Windows verbatim/UNC prefixes
/// removed unconditionally, and only then separators, ASCII case and a trailing
/// separator normalized. Applying one function to both sides makes one-sided
/// normalization impossible.
pub(crate) fn canonical_activity_path_key(path: &Path) -> String {
    let raw = match std::fs::canonicalize(path) {
        Ok(canonical) => canonical.to_string_lossy().to_string(),
        Err(_) => path.to_string_lossy().to_string(),
    };
    strip_windows_verbatim_prefix(&raw)
        .replace('\\', "/")
        .to_lowercase()
        .trim_end_matches('/')
        .to_string()
}

/// The four Windows verbatim/UNC prefix forms, rewritten on EVERY host so the
/// same bytes normalize identically on Linux CI and Windows. The UNC arms
/// produce `\\` (the ordinary UNC lead-in), mirroring the `path_utils`
/// precedent; this module deliberately does not call that helper, which is
/// private and cfg-gated.
fn strip_windows_verbatim_prefix(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = path.strip_prefix(r"\\?\") {
        rest.to_string()
    } else if let Some(rest) = path.strip_prefix(r"\??\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = path.strip_prefix(r"\??\") {
        rest.to_string()
    } else {
        path.to_string()
    }
}

/// A room is working only when one of its own immediate `__agent_*` replicas
/// has a persisted row that names exactly `<room>/<agent>`, resolves to that
/// replica's path, and passes `persisted_is_working`. A coordinator counts only
/// through its own matched row; it is never substituted for the other agents.
fn room_is_working(room_dir: &Path, room_name: &str, sessions: &[PersistedSession]) -> bool {
    list_replica_activity_dirs(room_dir)
        .iter()
        .any(|(replica_path, agent)| {
            let replica_key = canonical_activity_path_key(replica_path);
            let expected_name = format!("{room_name}/{agent}");
            sessions.iter().any(|row| {
                row.id.is_some()
                    && row.name == expected_name
                    && canonical_activity_path_key(Path::new(&row.working_directory)) == replica_key
                    && persisted_is_working(row.status.as_ref(), row.waiting_for_input)
            })
        })
}

/// Immediate `__agent_*` directories of a room as (path, agent-name) pairs,
/// sorted by agent name. The agent name is the directory name with the existing
/// `__agent_` prefix removed, so directory `__agent_x` matches only the
/// persisted name `<room>/x`.
fn list_replica_activity_dirs(wg_dir: &Path) -> Vec<(PathBuf, String)> {
    let mut replicas = Vec::new();
    if let Ok(entries) = std::fs::read_dir(wg_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(agent) = name.strip_prefix("__agent_") {
                replicas.push((path, agent.to_string()));
            }
        }
    }
    replicas.sort_by(|left, right| left.1.cmp(&right.1));
    replicas
}

/// Immediate `repo-*` directories of a room, sorted by file name. The CI axis
/// is aggregated over exactly these, never over arbitrary descendants.
fn list_room_repo_dirs(wg_dir: &Path) -> Vec<PathBuf> {
    let mut repos = Vec::new();
    if let Ok(entries) = std::fs::read_dir(wg_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("repo-") {
                repos.push(path);
            }
        }
    }
    repos.sort_by(|left, right| left.file_name().cmp(&right.file_name()));
    repos
}

/// Read `<room>/TASK.md` once. A missing file and a present-but-titleless file
/// are both `None` WITHOUT a warning; every other read failure and invalid
/// UTF-8 warn once for that room and still report `None`. Never locks, writes,
/// backs up or repairs the file.
fn read_task_title(room_dir: &Path) -> Option<String> {
    let path = room_dir.join("TASK.md");
    match std::fs::read(&path) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => parse_task_title(&text),
            Err(_) => {
                warn_activity(&format!(
                    "room activity: {} is not valid UTF-8; taskTitle is null",
                    path.display()
                ));
                None
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            warn_activity(&format!(
                "room activity: cannot read {}: {error}; taskTitle is null",
                path.display()
            ));
            None
        }
    }
}

fn add(args: WorkgroupAddArgs) -> Result<(), String> {
    let project_path = resolve_cli_project(&args.project)?;
    let ac_root = resolve_cli_ac_root(&project_path)?;
    let safe_team = sanitize_name(&args.team)?;
    let has_legacy_team_flags = args.coordinator.is_some()
        || !args.agents.is_empty()
        || !args.repos.is_empty()
        || !args.repo_agents.is_empty()
        || !args.repo_exclude_agents.is_empty();
    let team_dir = ac_root.join(format!("_team_{}", safe_team));
    let config_path = team_dir.join("config.json");
    let team_config_exists = team_dir.exists() || config_path.exists();
    let provisioning_config = if team_config_exists {
        read_team_config(&ac_root, &safe_team)?;
        if has_legacy_team_flags {
            return Err(format!(
                "Team '{}' already exists. `room add` no longer updates team configuration. Use `team create` before `room add`, or `team add-member` for membership changes.",
                safe_team
            ));
        }
        None
    } else if has_legacy_team_flags {
        let coordinator = args.coordinator.as_deref().ok_or_else(|| {
            "--coordinator is required when supplying team details on room add".to_string()
        })?;
        eprintln!(
            "Warning: created missing team configuration from supplied room details. Prefer creating the team before activating a room."
        );
        Some(build_new_team_config(
            &ac_root,
            coordinator,
            &args.agents,
            &args.repos,
            &args.repo_agents,
            &args.repo_exclude_agents,
        )?)
    } else {
        return Err(format!(
            "Team '{}' config not found. Create it first with `team create`.",
            safe_team
        ));
    };
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| format!("Failed to create async runtime: {}", e))?;
    let (coordinator, agents, repos) = if let Some(config) = provisioning_config {
        (
            Some(config.coordinator.clone()),
            config.agents.clone(),
            config.repos.clone(),
        )
    } else {
        (None, Vec::new(), Vec::new())
    };
    let result = runtime.block_on(create_workgroup_on_disk(WorkgroupDiskCreateArgs {
        project_path: project_path.clone(),
        team_name: safe_team,
        task_title: args.title,
        coordinator,
        agents,
        repos,
    }))?;
    let changed_path = PathBuf::from(&result.path);
    write_refresh(
        &project_path,
        &changed_path,
        changed_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("workgroup"),
        "workgroupCreated",
    );
    print_json(&result)
}

fn remove(args: WorkgroupRemoveArgs) -> Result<(), String> {
    // #1065 Stage F: activated with the sole production token; no lock-order test barrier.
    let activation = crate::config::seed_manifest::ManifestActivationToken::production();
    remove_hooked(args, Some(&activation), |_| {})
}

/// CLI workgroup removal with the #1063 project-only lock order.
///
/// Workgroup deletion is a project-only mutation: it acquires the project
/// seed-manifest gate before the atomic rename and NEVER acquires the
/// `TeamConfigMutationGuard` (plan sections 5.4/6.3). It prunes `Deleted`/`Partial`
/// logical paths while holding the gate, then releases the gate before converting
/// the structured outcome into the CLI success/error contract or formatting an
/// error. `remove` is synchronous, so no guard ever crosses `.await`.
///
/// `activation` is `Some(ManifestActivationToken::production())` in production
/// (#1065 Stage F); tests may pass `Some(for_test())` or `None`.
/// `after_project_acquired` is a `#[cfg(test)]` inversion barrier that fires after
/// the project gate is held; production passes a no-op.
fn remove_hooked(
    args: WorkgroupRemoveArgs,
    activation: Option<&crate::config::seed_manifest::ManifestActivationToken>,
    after_project_acquired: impl FnOnce(&Path),
) -> Result<(), String> {
    validate_existing_name(&args.workgroup, "Room")?;
    let project_path = resolve_cli_project(&args.project)?;
    let ac_root = resolve_cli_ac_root(&project_path)?;
    let wg_dir = ac_root.join(&args.workgroup);
    validate_delete_root_not_link_or_reparse(&wg_dir)?;
    crate::cli::session_safety::ensure_no_live_sessions_under(&wg_dir)?;
    if !args.force_dirty {
        let dirty = check_workgroup_repos_dirty(std::slice::from_ref(&wg_dir));
        if !dirty.is_empty() {
            let list = dirty
                .iter()
                .map(|(repo, reason)| format!("  - {} ({})", repo, reason))
                .collect::<Vec<_>>()
                .join("\n");
            return Err(format!(
                "Cannot delete room: the following repos have pending work:\n{}\n\nCommit or push changes before deleting, or pass --force-dirty.",
                list
            ));
        }
    }

    // Project-only gate: acquire before the atomic rename, prune Deleted/Partial
    // while held, then release before formatting the outcome. Never take the team guard.
    let mut project_gate = acquire_lifecycle_project_gate(&project_path)?;
    after_project_acquired(&ac_root);
    let outcome = crate::commands::entity_creation::try_atomic_delete_wg(&wg_dir);
    if outcome.logical_path_removed() {
        prune_workgroup_config_scope(project_gate.as_mut(), activation, &args.workgroup);
    }
    drop(project_gate);

    match cli_remove_refresh_decision(outcome)? {
        RemoveRefreshDecision::EmitWorkgroupRemoved => {}
    }
    // (#621) Drop the workgroup's coordinator_clocks keys. CLI is its own process,
    // so load+save the on-disk map directly (the startup prune backstops the
    // GUI-running-concurrently race).
    if let Some(project_name) = project_path.file_name().and_then(|n| n.to_str()) {
        match crate::config::coordinator_clocks::remove_workgroup_on_disk(
            project_name,
            &args.workgroup,
        ) {
            Ok(n) if n > 0 => {
                log::info!(
                    "[workgroup-remove] dropped {} clock key(s) for {}",
                    n,
                    args.workgroup
                )
            }
            Ok(_) => {}
            Err(e) => log::warn!("[workgroup-remove] clock cleanup failed: {}", e),
        }
    }
    write_refresh(&project_path, &wg_dir, &args.workgroup, "workgroupRemoved");
    if std::env::var_os("AC_MACHINE_OUTPUT").is_some() {
        print_json(&serde_json::json!({
            "workgroup": args.workgroup,
            "path": wg_dir.to_string_lossy(),
            "removed": true
        }))
    } else {
        crate::cli_println!("Removed room {}", args.workgroup);
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemoveRefreshDecision {
    EmitWorkgroupRemoved,
}

pub(crate) fn cli_remove_refresh_decision(
    outcome: WgDeleteOutcome,
) -> Result<RemoveRefreshDecision, String> {
    match outcome {
        WgDeleteOutcome::Deleted => {}
        WgDeleteOutcome::Blocked(e) => {
            return Err(format!("Failed to delete room, file in use: {}", e));
        }
        WgDeleteOutcome::Partial { orphan_path, error } => {
            return Err(format!(
                "Failed to fully delete room directory; renamed room to orphan '{}', but failed to remove orphan: {}",
                orphan_path.display(),
                error
            ));
        }
        WgDeleteOutcome::Other(e) => {
            return Err(format!("Failed to delete room directory: {}", e));
        }
    }
    Ok(RemoveRefreshDecision::EmitWorkgroupRemoved)
}

pub(crate) fn build_new_team_config(
    ac_root: &Path,
    coordinator: &str,
    agents: &[String],
    repos: &[String],
    repo_agents: &[String],
    repo_exclude_agents: &[String],
) -> Result<TeamConfigResult, String> {
    let coordinator = resolve_agent_ref(ac_root, coordinator)?;
    let mut roster = vec![coordinator.clone()];
    for agent in agents {
        push_unique(&mut roster, resolve_agent_ref(ac_root, agent)?);
    }
    let repo_config =
        build_repo_assignments(ac_root, &roster, repos, repo_agents, repo_exclude_agents)?;
    Ok(TeamConfigResult {
        agents: roster,
        coordinator,
        repos: repo_config,
        context_alert_percentages: Vec::new(),
    })
}

fn build_repo_assignments(
    ac_root: &Path,
    roster: &[String],
    repos: &[String],
    repo_agents: &[String],
    repo_exclude_agents: &[String],
) -> Result<Vec<RepoAssignment>, String> {
    let mut order = Vec::new();
    let mut default_urls = BTreeSet::new();
    for repo in repos {
        let url = repo.trim().to_string();
        if url.is_empty() {
            return Err("--repo cannot be empty".to_string());
        }
        if default_urls.insert(url.clone()) {
            order.push(url);
        }
    }

    let include = parse_assignment_specs(repo_agents, "--repo-agents")?;
    let exclude = parse_assignment_specs(repo_exclude_agents, "--repo-exclude-agents")?;
    for url in include.keys() {
        if exclude.contains_key(url) {
            return Err(format!(
                "Repo '{}' cannot use both --repo-agents and --repo-exclude-agents",
                url
            ));
        }
        if !order.contains(url) {
            order.push(url.clone());
        }
    }
    for url in exclude.keys() {
        if !order.contains(url) {
            order.push(url.clone());
        }
    }

    let mut out = Vec::new();
    for url in order {
        let agents = if let Some(list) = include.get(&url) {
            resolve_assignment_agents(ac_root, roster, list)?
        } else if let Some(list) = exclude.get(&url) {
            let excluded = resolve_assignment_agents(ac_root, roster, list)?;
            roster
                .iter()
                .filter(|agent| !excluded.contains(agent))
                .cloned()
                .collect()
        } else {
            roster.to_vec()
        };
        out.push(RepoAssignment { url, agents });
    }
    Ok(out)
}

fn parse_assignment_specs(
    specs: &[String],
    flag: &str,
) -> Result<HashMap<String, Vec<String>>, String> {
    let mut out = HashMap::new();
    for spec in specs {
        let Some((url, agents)) = spec.split_once('=') else {
            return Err(format!("{} expects URL=agent-a,agent-b", flag));
        };
        let url = url.trim();
        if url.is_empty() {
            return Err(format!("{} URL cannot be empty", flag));
        }
        if out.contains_key(url) {
            return Err(format!("{} repeated for repo '{}'", flag, url));
        }
        let agents = agents
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        if agents.is_empty() {
            return Err(format!("{} must list at least one agent", flag));
        }
        out.insert(url.to_string(), agents);
    }
    Ok(out)
}

fn resolve_assignment_agents(
    ac_root: &Path,
    roster: &[String],
    agents: &[String],
) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for agent in agents {
        let resolved = resolve_agent_ref(ac_root, agent)?;
        if !roster.contains(&resolved) {
            return Err(format!(
                "Repo assignment references agent '{}' which is not in the final team roster",
                agent
            ));
        }
        push_unique(&mut out, resolved);
    }
    Ok(out)
}

pub(crate) fn push_unique(items: &mut Vec<String>, value: String) {
    if !items.contains(&value) {
        items.push(value);
    }
}

fn list_replicas(wg_dir: &Path) -> Vec<String> {
    list_replica_activity_dirs(wg_dir)
        .into_iter()
        .map(|(_, agent)| agent)
        .collect()
}

pub(crate) async fn clone_missing_for_config(
    wg_dir: &Path,
    repos: &[RepoAssignment],
) -> Vec<crate::commands::entity_creation::CloneError> {
    clone_missing_repos_for_workgroup(wg_dir, repos).await
}

fn print_json<T: Serialize>(value: &T) -> Result<(), String> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|e| format!("Failed to serialize JSON output: {}", e))?;
    crate::cli_println!("{}", json);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::session::SessionStatus;

    fn activity_row(
        name: &str,
        working_directory: &Path,
        status: SessionStatus,
        waiting_for_input: bool,
    ) -> PersistedSession {
        PersistedSession {
            name: name.to_string(),
            shell: "powershell.exe".to_string(),
            shell_args: Vec::new(),
            working_directory: working_directory.to_string_lossy().to_string(),
            id: Some(uuid::Uuid::new_v4().to_string()),
            status: Some(status),
            waiting_for_input: Some(waiting_for_input),
            ..PersistedSession::default()
        }
    }

    fn activity_instant(text: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(text)
            .expect("instant")
            .with_timezone(&chrono::Utc)
    }

    #[test]
    fn activity_path_key_normalizes_all_four_verbatim_forms_on_every_host() {
        // Host-independent by construction: these paths do not exist, so
        // `canonicalize` fails and the raw bytes go through the four-arm
        // normalizer on Linux and Windows alike.
        let plain_drive = canonical_activity_path_key(Path::new(r"C:\Repo\Rooms\A"));
        assert_eq!(
            canonical_activity_path_key(Path::new(r"\\?\C:\Repo\Rooms\A")),
            plain_drive
        );
        assert_eq!(
            canonical_activity_path_key(Path::new(r"\??\C:\Repo\Rooms\A")),
            plain_drive
        );
        assert_eq!(
            canonical_activity_path_key(Path::new("c:/repo/rooms/a/")),
            plain_drive
        );

        let plain_unc = canonical_activity_path_key(Path::new(r"\\server\share\repo"));
        assert_eq!(
            canonical_activity_path_key(Path::new(r"\\?\UNC\server\share\repo")),
            plain_unc
        );
        assert_eq!(
            canonical_activity_path_key(Path::new(r"\??\UNC\server\share\repo")),
            plain_unc
        );
        assert_eq!(
            canonical_activity_path_key(Path::new("//SERVER/SHARE/REPO/")),
            plain_unc
        );
        assert_ne!(plain_drive, plain_unc);
    }

    #[test]
    fn activity_path_key_resolves_dot_segments_for_existing_paths() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("create repo");
        let detoured = tmp.path().join("repo").join(".").join("..").join("repo");
        assert_eq!(
            canonical_activity_path_key(&repo),
            canonical_activity_path_key(&detoured)
        );
    }

    fn indexed(state: PersistedCiState) -> IndexedRepoCi {
        indexed_with(state, &[], &[], None)
    }

    fn indexed_with(
        state: PersistedCiState,
        run_ids: &[u64],
        pull_requests: &[u64],
        unknown_reason: Option<&str>,
    ) -> IndexedRepoCi {
        IndexedRepoCi {
            ci: repo_ci(state, run_ids, pull_requests, unknown_reason),
            conflicting: false,
        }
    }

    fn repo_ci(
        state: PersistedCiState,
        run_ids: &[u64],
        pull_requests: &[u64],
        unknown_reason: Option<&str>,
    ) -> PersistedRepoCi {
        PersistedRepoCi {
            state,
            run_ids: run_ids.to_vec(),
            pull_requests: pull_requests.to_vec(),
            unknown_reason: unknown_reason.map(str::to_string),
        }
    }

    fn state_of(repo_keys: &[String], entries: &HashMap<String, IndexedRepoCi>) -> &'static str {
        aggregate_room_ci(repo_keys, entries, None).state
    }

    #[test]
    fn activity_ci_aggregation_is_the_documented_tri_state() {
        let entries = HashMap::from([
            ("a".to_string(), indexed(PersistedCiState::Running)),
            ("b".to_string(), indexed(PersistedCiState::Idle)),
        ]);
        assert_eq!(state_of(&[], &entries), "unknown");
        assert_eq!(state_of(&["a".to_string()], &entries), "running");
        assert_eq!(state_of(&["b".to_string()], &entries), "idle");
        assert_eq!(
            state_of(&["b".to_string(), "missing".to_string()], &entries),
            "unknown"
        );
        assert_eq!(
            state_of(
                &["b".to_string(), "c".to_string()],
                &HashMap::from([
                    ("b".to_string(), indexed(PersistedCiState::Idle)),
                    ("c".to_string(), indexed(PersistedCiState::Unknown)),
                ])
            ),
            "unknown"
        );
        assert_eq!(
            state_of(&["a".to_string(), "c".to_string()], &entries),
            "running",
            "a running repo outranks an unmatched one"
        );
    }

    #[test]
    fn activity_ci_index_matches_verbatim_case_separator_and_trailing_variants() {
        let snapshot = RemoteActivitySnapshot {
            generated_at: chrono::Utc::now(),
            repos: vec![(
                r"\\?\C:\Repo\Rooms\A\".to_string(),
                repo_ci(PersistedCiState::Idle, &[], &[], None),
            )],
        };
        let entries = index_ci_entries(&snapshot);
        let room_repo_keys = vec![canonical_activity_path_key(Path::new("c:/repo/rooms/a"))];
        assert_eq!(state_of(&room_repo_keys, &entries), "idle");
    }

    #[test]
    fn activity_ci_index_collapses_conflicting_duplicates_to_unknown() {
        let snapshot = RemoteActivitySnapshot {
            generated_at: chrono::Utc::now(),
            repos: vec![
                (
                    "A:/repo".to_string(),
                    repo_ci(PersistedCiState::Running, &[], &[], None),
                ),
                (
                    "a:/repo/".to_string(),
                    repo_ci(PersistedCiState::Idle, &[], &[], None),
                ),
            ],
        };
        let entries = index_ci_entries(&snapshot);
        assert_eq!(state_of(&["a:/repo".to_string()], &entries), "unknown");
    }

    // --- #2473 filters and CI detail ---

    fn filter_args() -> WorkgroupActivityArgs {
        WorkgroupActivityArgs {
            project: "p".to_string(),
            rooms: None,
            team: None,
            working: None,
            ci_state: None,
            title_regex: None,
            hide_clean: false,
        }
    }

    #[test]
    fn u1_parse_activity_filter_rejects_each_bad_value_with_the_exact_message() {
        let rooms = |v: &str| {
            format!(
                "Invalid --rooms '{v}': expected comma-separated positive room numbers, e.g. 5,12,17"
            )
        };
        let regex_error = regex::Regex::new(&String::from("("))
            .expect_err("bad regex")
            .to_string();
        let cases: Vec<(WorkgroupActivityArgs, String)> = vec![
            (
                WorkgroupActivityArgs {
                    rooms: Some("5,,7".into()),
                    ..filter_args()
                },
                rooms("5,,7"),
            ),
            (
                WorkgroupActivityArgs {
                    rooms: Some("0".into()),
                    ..filter_args()
                },
                rooms("0"),
            ),
            (
                WorkgroupActivityArgs {
                    rooms: Some("-1".into()),
                    ..filter_args()
                },
                rooms("-1"),
            ),
            (
                WorkgroupActivityArgs {
                    rooms: Some("abc".into()),
                    ..filter_args()
                },
                rooms("abc"),
            ),
            (
                WorkgroupActivityArgs {
                    rooms: Some("".into()),
                    ..filter_args()
                },
                rooms(""),
            ),
            (
                WorkgroupActivityArgs {
                    rooms: Some("5,".into()),
                    ..filter_args()
                },
                rooms("5,"),
            ),
            (
                WorkgroupActivityArgs {
                    rooms: Some("4294967296".into()),
                    ..filter_args()
                },
                rooms("4294967296"),
            ),
            (
                WorkgroupActivityArgs {
                    team: Some(" ".into()),
                    ..filter_args()
                },
                "Invalid --team ' ': expected a team name".to_string(),
            ),
            (
                WorkgroupActivityArgs {
                    team: Some("".into()),
                    ..filter_args()
                },
                "Invalid --team '': expected a team name".to_string(),
            ),
            (
                WorkgroupActivityArgs {
                    working: Some("TRUE".into()),
                    ..filter_args()
                },
                "Invalid --working 'TRUE': expected true or false".to_string(),
            ),
            (
                WorkgroupActivityArgs {
                    working: Some("1".into()),
                    ..filter_args()
                },
                "Invalid --working '1': expected true or false".to_string(),
            ),
            (
                WorkgroupActivityArgs {
                    ci_state: Some("Running".into()),
                    ..filter_args()
                },
                "Invalid --ci-state 'Running': expected running, idle or unknown".to_string(),
            ),
            (
                WorkgroupActivityArgs {
                    ci_state: Some("done".into()),
                    ..filter_args()
                },
                "Invalid --ci-state 'done': expected running, idle or unknown".to_string(),
            ),
            (
                WorkgroupActivityArgs {
                    title_regex: Some("(".into()),
                    ..filter_args()
                },
                format!("Invalid --title-regex '(': {regex_error}"),
            ),
            (
                // First failing flag in the documented order wins.
                WorkgroupActivityArgs {
                    rooms: Some("x".into()),
                    team: Some(" ".into()),
                    working: Some("no".into()),
                    ..filter_args()
                },
                rooms("x"),
            ),
            (
                WorkgroupActivityArgs {
                    working: Some("no".into()),
                    title_regex: Some("(".into()),
                    ..filter_args()
                },
                "Invalid --working 'no': expected true or false".to_string(),
            ),
        ];
        assert!(cases.len() >= 12);
        for (args, expected) in cases {
            assert_eq!(parse_activity_filter(&args).err(), Some(expected));
        }
        let ok = parse_activity_filter(&WorkgroupActivityArgs {
            rooms: Some(" 5, 12,5 ".into()),
            team: Some("-t".into()),
            working: Some("false".into()),
            ci_state: Some("idle".into()),
            title_regex: Some("-x".into()),
            hide_clean: true,
            ..filter_args()
        })
        .expect("valid filter");
        assert_eq!(ok.rooms, Some(BTreeSet::from([5, 12])));
        assert_eq!(ok.team.as_deref(), Some("-t"));
        assert_eq!(ok.working, Some(false));
        assert_eq!(ok.ci_state, Some("idle"));
        assert!(ok.hide_clean);
    }

    fn item(
        name: &str,
        team: &str,
        working: bool,
        ci: &'static str,
        title: Option<&str>,
    ) -> WorkgroupActivityItem {
        WorkgroupActivityItem {
            name: name.to_string(),
            team: team.to_string(),
            working,
            ci_state: ci,
            task_title: title.map(str::to_string),
            ci_run_ids: Vec::new(),
            ci_pull_requests: Vec::new(),
            ci_unknown_reason: None,
        }
    }

    #[test]
    fn u2_each_predicate_passes_one_item_and_rejects_another() {
        let base = item("room-5-dev", "dev", true, "running", Some("Fix #12 parser"));
        let cases: Vec<(ActivityFilter, WorkgroupActivityItem)> = vec![
            (
                ActivityFilter {
                    rooms: Some(BTreeSet::from([5, 17])),
                    ..ActivityFilter::default()
                },
                item("room-6-dev", "dev", true, "running", Some("Fix #12 parser")),
            ),
            (
                ActivityFilter {
                    team: Some("dev".into()),
                    ..ActivityFilter::default()
                },
                item("room-5-ops", "ops", true, "running", Some("Fix #12 parser")),
            ),
            (
                ActivityFilter {
                    working: Some(true),
                    ..ActivityFilter::default()
                },
                item(
                    "room-5-dev",
                    "dev",
                    false,
                    "running",
                    Some("Fix #12 parser"),
                ),
            ),
            (
                ActivityFilter {
                    ci_state: Some("running"),
                    ..ActivityFilter::default()
                },
                item("room-5-dev", "dev", true, "idle", Some("Fix #12 parser")),
            ),
            (
                ActivityFilter {
                    title_regex: Some(regex::Regex::new("#1\\d").expect("regex")),
                    ..ActivityFilter::default()
                },
                item("room-5-dev", "dev", true, "running", None),
            ),
            (
                ActivityFilter {
                    hide_clean: true,
                    ..ActivityFilter::default()
                },
                item("room-5-dev", "dev", true, "running", Some("  CLEAN ")),
            ),
        ];
        for (filter, failing) in &cases {
            assert!(filter.matches(&base), "{filter:?} passes the base item");
            assert!(!filter.matches(failing), "{filter:?} rejects {failing:?}");
        }
        let regex = ActivityFilter {
            title_regex: Some(regex::Regex::new("fix").expect("regex")),
            ..ActivityFilter::default()
        };
        assert!(!regex.matches(&base), "the regex is case-sensitive");
        let hide = ActivityFilter {
            hide_clean: true,
            ..ActivityFilter::default()
        };
        assert!(!hide.matches(&item("room-5-dev", "dev", true, "running", None)));
        assert!(!hide.matches(&item("room-5-dev", "dev", true, "running", Some("  "))));
        assert!(hide.matches(&item(
            "room-5-dev",
            "dev",
            true,
            "running",
            Some("Clean up")
        )));
        let rooms = ActivityFilter {
            rooms: Some(BTreeSet::from([5])),
            ..ActivityFilter::default()
        };
        assert!(!rooms.matches(&item("room-x", "dev", true, "running", None)));
        assert!(ActivityFilter::default().matches(&base));
    }

    #[test]
    fn u3_room_number_reads_both_prefixes() {
        assert_eq!(room_number("room-15-x"), Some(15));
        assert_eq!(room_number("wg-3-x"), Some(3));
        assert_eq!(room_number("room-x"), None);
        assert_eq!(room_number("other-3-x"), None);
    }

    #[test]
    fn u4_aggregation_reports_every_reason_and_unions_runs() {
        let key = |k: &str| k.to_string();
        let entries = HashMap::from([
            (
                key("run1"),
                indexed_with(PersistedCiState::Running, &[30, 10], &[7], None),
            ),
            (
                key("run2"),
                indexed_with(PersistedCiState::Running, &[10, 20], &[7, 2], None),
            ),
            (key("idle"), indexed(PersistedCiState::Idle)),
            (key("unk"), indexed(PersistedCiState::Unknown)),
            (
                key("timeout"),
                indexed_with(
                    PersistedCiState::Unknown,
                    &[],
                    &[],
                    Some("ci-query-timeout"),
                ),
            ),
            (
                key("weird"),
                indexed_with(PersistedCiState::Unknown, &[], &[], Some("made-up")),
            ),
            (
                key("conflict"),
                IndexedRepoCi {
                    ci: repo_ci(PersistedCiState::Unknown, &[], &[], None),
                    conflicting: true,
                },
            ),
        ]);
        let reason = |keys: &[&str], gate: Option<&'static str>| {
            let keys: Vec<String> = keys.iter().map(|k| k.to_string()).collect();
            aggregate_room_ci(&keys, &entries, gate)
        };
        let running = reason(&["run1", "run2", "unk", "idle"], None);
        assert_eq!(
            running,
            RoomCi {
                state: "running",
                run_ids: vec![10, 20, 30],
                pull_requests: vec![2, 7],
                unknown_reason: None,
            }
        );
        let idle = reason(&["idle"], None);
        assert_eq!(
            (
                idle.state,
                idle.run_ids,
                idle.pull_requests,
                idle.unknown_reason
            ),
            ("idle", vec![], vec![], None)
        );
        assert_eq!(reason(&[], None).unknown_reason, Some("no-repos"));
        assert_eq!(
            reason(&["idle", "missing"], None).unknown_reason,
            Some("repo-not-in-snapshot")
        );
        assert_eq!(
            reason(&["idle", "conflict"], None).unknown_reason,
            Some("conflicting-snapshot-entries")
        );
        assert_eq!(
            reason(&["timeout", "idle"], None).unknown_reason,
            Some("ci-query-timeout")
        );
        assert_eq!(
            reason(&["unk"], None).unknown_reason,
            Some("repo-ci-unknown")
        );
        assert_eq!(
            reason(&["weird"], None).unknown_reason,
            Some("repo-ci-unknown")
        );
        // First repo in sorted order decides: "timeout" < "unk".
        assert_eq!(
            reason(&["unk", "timeout"], None).unknown_reason,
            Some("ci-query-timeout")
        );
        for gate in ["ci-disabled", "daemon-not-live", "snapshot-unavailable"] {
            let empty = HashMap::new();
            let gated = aggregate_room_ci(&[], &empty, Some(gate));
            assert_eq!((gated.state, gated.unknown_reason), ("unknown", Some(gate)));
            let gated = aggregate_room_ci(&[key("a")], &empty, Some(gate));
            assert_eq!(
                gated.unknown_reason,
                Some(gate),
                "the gate precedes repo reasons"
            );
            assert!(gated.run_ids.is_empty() && gated.pull_requests.is_empty());
        }
        let unknown = reason(&["unk"], None);
        assert!(unknown.run_ids.is_empty() && unknown.pull_requests.is_empty());
    }

    #[test]
    fn p4_duplicate_entries_union_metadata_but_conflicting_states_are_unknown() {
        let snapshot = RemoteActivitySnapshot {
            generated_at: chrono::Utc::now(),
            repos: vec![
                (
                    "A:/repo".to_string(),
                    repo_ci(PersistedCiState::Running, &[1, 3], &[9], None),
                ),
                (
                    "a:/repo/".to_string(),
                    repo_ci(PersistedCiState::Running, &[2, 3], &[8], None),
                ),
                (
                    "b:/repo".to_string(),
                    repo_ci(PersistedCiState::Running, &[5], &[], None),
                ),
                (
                    "B:/repo/".to_string(),
                    repo_ci(PersistedCiState::Idle, &[], &[], None),
                ),
            ],
        };
        let entries = index_ci_entries(&snapshot);
        let same = aggregate_room_ci(&["a:/repo".to_string()], &entries, None);
        assert_eq!(same.state, "running");
        assert_eq!(same.run_ids, vec![1, 2, 3]);
        assert_eq!(same.pull_requests, vec![8, 9]);
        let conflict = aggregate_room_ci(&["b:/repo".to_string()], &entries, None);
        assert_eq!(conflict.state, "unknown");
        assert_eq!(
            conflict.unknown_reason,
            Some("conflicting-snapshot-entries")
        );
        assert!(conflict.run_ids.is_empty());
    }

    #[test]
    fn activity_ci_gate_prefers_disabled_then_daemon() {
        assert_eq!(ci_gate(true, true), CiGate::Usable);
        assert_eq!(ci_gate(false, true), CiGate::CiDisabled);
        assert_eq!(ci_gate(false, false), CiGate::CiDisabled);
        assert_eq!(ci_gate(true, false), CiGate::DaemonNotLive);
    }

    #[test]
    fn activity_cache_read_rejects_missing_stale_future_and_malformed_and_accepts_fresh() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join(REMOTE_ACTIVITY_SNAPSHOT_FILE_NAME);
        let now = activity_instant("2026-09-22T11:41:41Z");
        let entry = vec![(
            "a:/repo".to_string(),
            repo_ci(PersistedCiState::Running, &[], &[], None),
        )];

        assert!(
            read_fresh_ci_snapshot(Some(tmp.path()), now).is_err(),
            "a missing snapshot is an error"
        );
        assert!(
            read_fresh_ci_snapshot(None, now).is_err(),
            "an unavailable config directory is a missing snapshot"
        );

        crate::config::remote_activity_cache::write_snapshot(
            &path,
            now - chrono::Duration::seconds(30),
            &entry,
        )
        .expect("write fresh snapshot");
        let fresh = read_fresh_ci_snapshot(Some(tmp.path()), now).expect("fresh snapshot");
        assert_eq!(fresh.repos, entry);

        crate::config::remote_activity_cache::write_snapshot(
            &path,
            now - chrono::Duration::seconds(31),
            &entry,
        )
        .expect("write stale snapshot");
        assert!(read_fresh_ci_snapshot(Some(tmp.path()), now).is_err());

        crate::config::remote_activity_cache::write_snapshot(
            &path,
            now + chrono::Duration::seconds(31),
            &entry,
        )
        .expect("write future snapshot");
        assert!(read_fresh_ci_snapshot(Some(tmp.path()), now).is_err());

        std::fs::write(&path, b"{not json").expect("write malformed");
        assert!(read_fresh_ci_snapshot(Some(tmp.path()), now).is_err());
    }

    #[test]
    fn activity_working_requires_exact_name_path_and_working_status() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let room = tmp.path().join("room-1-dev-team");
        let coord = room.join("__agent_coord");
        let dev = room.join("__agent_dev");
        std::fs::create_dir_all(&coord).expect("coord replica");
        std::fs::create_dir_all(&dev).expect("dev replica");
        let foreign = tmp.path().join("ForeignProject").join("__agent_dev");
        std::fs::create_dir_all(&foreign).expect("foreign replica");

        let idle_coordinator =
            activity_row("room-1-dev-team/coord", &coord, SessionStatus::Idle, true);
        assert!(
            !room_is_working(
                &room,
                "room-1-dev-team",
                std::slice::from_ref(&idle_coordinator)
            ),
            "an idle coordinator must not make its room work"
        );

        let mut no_id = activity_row("room-1-dev-team/dev", &dev, SessionStatus::Running, false);
        no_id.id = None;
        let rejected = vec![
            // The raw directory name is not the persisted session name.
            activity_row(
                "room-1-dev-team/__agent_dev",
                &dev,
                SessionStatus::Running,
                false,
            ),
            // Similarly prefixed room and agent names are not this room's.
            activity_row("room-1-dev-teams/dev", &dev, SessionStatus::Running, false),
            activity_row("room-2-dev-team/dev", &dev, SessionStatus::Running, false),
            activity_row("room-1-dev-team/dev-2", &dev, SessionStatus::Running, false),
            // Same name, different normalized working directory.
            activity_row(
                "room-1-dev-team/dev",
                &foreign,
                SessionStatus::Running,
                false,
            ),
            // Waiting/exited rows are not working.
            activity_row("room-1-dev-team/dev", &dev, SessionStatus::Running, true),
            activity_row("room-1-dev-team/dev", &dev, SessionStatus::Idle, false),
            no_id,
        ];
        for row in rejected {
            assert!(
                !room_is_working(&room, "room-1-dev-team", &[row]),
                "the row must not count as working"
            );
        }

        let running_dev = activity_row("room-1-dev-team/dev", &dev, SessionStatus::Running, false);
        assert!(
            room_is_working(&room, "room-1-dev-team", &[idle_coordinator, running_dev]),
            "a local non-coordinator working row flips the room true"
        );
    }

    #[test]
    fn activity_repo_dirs_are_immediate_prefix_matches_only() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let room = tmp.path().join("room-1-dev-team");
        std::fs::create_dir_all(room.join("repo-b")).expect("repo-b");
        std::fs::create_dir_all(room.join("repo-a")).expect("repo-a");
        std::fs::create_dir_all(room.join("repo-a").join("nested")).expect("nested");
        std::fs::create_dir_all(room.join("not-a-repo")).expect("not-a-repo");
        std::fs::write(room.join("repo-file"), b"file").expect("repo-file");
        let names: Vec<String> = list_room_repo_dirs(&room)
            .iter()
            .map(|path| {
                path.file_name()
                    .expect("file name")
                    .to_string_lossy()
                    .to_string()
            })
            .collect();
        assert_eq!(names, vec!["repo-a", "repo-b"]);
    }

    #[test]
    fn activity_task_title_reads_quoted_and_bare_titles_and_ignores_missing_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert_eq!(read_task_title(tmp.path()), None, "missing TASK.md is null");

        std::fs::write(tmp.path().join("TASK.md"), "---\ntitle: 'Quoted'\n---\n")
            .expect("write quoted");
        assert_eq!(read_task_title(tmp.path()).as_deref(), Some("Quoted"));

        std::fs::write(tmp.path().join("TASK.md"), "---\ntitle: Bare\n---\n").expect("write bare");
        assert_eq!(read_task_title(tmp.path()).as_deref(), Some("Bare"));

        std::fs::write(tmp.path().join("TASK.md"), "---\ntitle: ''\n---\n").expect("write empty");
        assert_eq!(read_task_title(tmp.path()), None, "an empty title is null");

        std::fs::write(tmp.path().join("TASK.md"), [0xFF_u8, 0xFE]).expect("write invalid");
        assert_eq!(read_task_title(tmp.path()), None, "invalid UTF-8 is null");
    }

    #[test]
    fn partial_delete_outcome_does_not_authorize_removed_refresh() {
        let outcome = WgDeleteOutcome::Partial {
            orphan_path: PathBuf::from(".deleting-wg-1-test-orphan"),
            error: std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "forced remove failure",
            ),
        };

        let err = cli_remove_refresh_decision(outcome)
            .expect_err("partial delete must not authorize workgroupRemoved refresh");
        assert!(err.contains("Failed to fully delete room directory"));
        assert!(err.contains(".deleting-wg-1-test-orphan"));
    }

    #[test]
    fn clean_delete_outcome_authorizes_removed_refresh() {
        assert_eq!(
            cli_remove_refresh_decision(WgDeleteOutcome::Deleted)
                .expect("deleted outcome should refresh"),
            RemoveRefreshDecision::EmitWorkgroupRemoved
        );
    }

    #[test]
    fn cli_team_builder_defaults_context_alerts_to_disabled() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let ac_root = tmp.path().join(".ac");
        std::fs::create_dir_all(ac_root.join("_agent_coordinator")).expect("coordinator matrix");
        std::fs::create_dir_all(ac_root.join("_agent_member")).expect("member matrix");

        let config = build_new_team_config(
            &ac_root,
            "coordinator",
            &["member".to_string()],
            &[],
            &[],
            &[],
        )
        .expect("build config");
        assert!(config.context_alert_percentages.is_empty());
    }

    #[tokio::test]
    async fn legacy_workgroup_provisioning_never_upserts_concurrent_team_winner() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project = tmp.path().join("Project");
        let ac_root = project.join(".ac");
        std::fs::create_dir_all(ac_root.join("_agent_coordinator")).expect("coordinator matrix");
        let winner = TeamConfigResult {
            agents: vec!["_agent_coordinator".to_string()],
            coordinator: "_agent_coordinator".to_string(),
            repos: Vec::new(),
            context_alert_percentages: vec![75],
        };
        crate::commands::entity_creation::create_new_team_config_on_disk(
            &ac_root, "dev-team", &winner,
        )
        .expect("concurrent winner");

        let err = create_workgroup_on_disk(WorkgroupDiskCreateArgs {
            project_path: project,
            team_name: "dev-team".to_string(),
            task_title: "Race test".to_string(),
            coordinator: Some("_agent_coordinator".to_string()),
            agents: vec!["_agent_coordinator".to_string()],
            repos: Vec::new(),
        })
        .await
        .expect_err("legacy loser must not upsert");

        assert!(err.contains("Team 'dev-team' already exists"), "{err}");
        assert_eq!(
            read_team_config(&ac_root, "dev-team")
                .expect("winner config")
                .context_alert_percentages,
            vec![75]
        );
        assert!(list_workgroup_dirs(&ac_root).is_empty());
    }

    // #1063 Stage D-owned exact ignored CHILD HELPER for the CLI workgroup
    // cross-process lock-order inversion. Frozen fully-qualified name (a Stage E
    // parent spawns this verbatim via `current_exe --exact <name> --ignored`):
    //   crate::cli::workgroup::tests::cli_workgroup_lock_order_inversion_child
    // No-ops (no guard, no mutation) unless the child-mode action + per-spawn nonce
    // + control dir are all supplied; when driven, it calls the real private
    // `remove_hooked` (project-only) and drives the `after_project_acquired` barrier.
    #[test]
    #[ignore]
    fn cli_workgroup_lock_order_inversion_child() {
        use crate::cli::team::stage_d_lock_order_child as child;
        let Some(ctx) = child::child_context(child::WORKGROUP_ACTION) else {
            return;
        };
        let project = ctx.build_workgroup_fixture();
        let args = WorkgroupRemoveArgs {
            project,
            workgroup: "wg-1-dev-team".to_string(),
            force_dirty: false,
        };
        let result = remove_hooked(args, None, |_ac_root: &Path| ctx.report_and_wait());
        if let Err(error) = &result {
            println!("STAGE_D_LOCK_ORDER_ERROR {} workgroup {}", ctx.nonce, error);
        }
        println!(
            "STAGE_D_LOCK_ORDER_DONE {} workgroup ok={}",
            ctx.nonce,
            result.is_ok()
        );
    }

    // #1063: prove the driven project-only lock-order path in-process (no
    // `current_exe` parent - that machinery is Stage E). Env-guarded and `#[ignore]`
    // so the parallel `--lib` regression is untouched. Like the member driver it
    // no-ops unless `DRIVE_VAR` is set, so a bare `cargo test --lib -- --ignored` run
    // never drives it. Enable and isolate it deliberately (it pins the once-cached
    // config dir, so it must own the process):
    // `AC_STAGE_D_LOCK_ORDER_DRIVE=1 cargo test --lib -- --ignored --test-threads=1
    // --exact cli::workgroup::tests::cli_workgroup_lock_order_inversion_driver`.
    #[test]
    #[ignore]
    fn cli_workgroup_lock_order_inversion_driver() {
        use crate::cli::team::stage_d_lock_order_child::{
            driver_enabled, EnvGuard, ACTION_VAR, CONTROL_DIR_VAR, NONCE_VAR, WORKGROUP_ACTION,
        };
        if !driver_enabled() {
            return;
        }
        let control = tempfile::tempdir().expect("tempdir");
        let nonce = "driver-workgroup";
        let _guard = EnvGuard::capture(&[
            ACTION_VAR,
            NONCE_VAR,
            CONTROL_DIR_VAR,
            "AGENTSCOMMANDER_TEST_CONFIG_DIR",
        ]);
        std::env::set_var(ACTION_VAR, WORKGROUP_ACTION);
        std::env::set_var(NONCE_VAR, nonce);
        std::env::set_var(CONTROL_DIR_VAR, control.path());

        let reached = control.path().join(format!("reached-{nonce}"));
        let release = control.path().join(format!("release-{nonce}"));
        let releaser = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
            while std::time::Instant::now() < deadline && !reached.exists() {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert!(
                reached.exists(),
                "child never reported reaching the barrier"
            );
            std::fs::write(&release, b"go").expect("write release");
        });

        cli_workgroup_lock_order_inversion_child();
        releaser.join().expect("join releaser");

        assert!(
            control.path().join(format!("reached-{nonce}")).exists(),
            "the barrier must have fired after the project gate was acquired"
        );
        let wg_dir = control
            .path()
            .join(format!("fixture-{nonce}"))
            .join("Project")
            .join(".ac")
            .join("wg-1-dev-team");
        assert!(
            !wg_dir.exists(),
            "the workgroup must be removed after the barrier releases"
        );
    }

    /// #1795 `T12`. Drives the real production call site, `create_workgroup_on_disk`,
    /// end to end. A test that called `create_room_shared_dir` directly would stay
    /// green with the production call deleted, which is exactly the hole control `C6`
    /// probes.
    #[tokio::test]
    async fn create_workgroup_on_disk_creates_the_room_shared_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project = tmp.path().join("Project");
        let ac_root = project.join(".ac");
        std::fs::create_dir_all(ac_root.join("_agent_coordinator")).expect("coordinator matrix");

        create_workgroup_on_disk(WorkgroupDiskCreateArgs {
            project_path: project,
            team_name: "dev-team".to_string(),
            task_title: "Room shared directory".to_string(),
            coordinator: Some("_agent_coordinator".to_string()),
            agents: vec!["_agent_coordinator".to_string()],
            repos: Vec::new(),
        })
        .await
        .expect("create the room");

        let rooms = list_workgroup_dirs(&ac_root);
        assert_eq!(rooms.len(), 1, "exactly one room must have been created");
        let wg_dir = &rooms[0];

        assert!(
            wg_dir.join("room-shared").is_dir(),
            "`room-shared` must exist under {}",
            wg_dir.display()
        );
        assert!(
            wg_dir.join("messaging").is_dir(),
            "`messaging` must exist under {}",
            wg_dir.display()
        );
        assert!(
            wg_dir.join("TASK.md").is_file(),
            "`TASK.md` must exist under {}",
            wg_dir.display()
        );
    }
}
