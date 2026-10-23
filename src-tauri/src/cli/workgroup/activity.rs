use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::commands::entity_creation::{list_workgroup_dirs, parse_task_title};
use crate::config::daemon_pid::{detect_daemon_state, DaemonState};
use crate::config::remote_activity_cache::{
    read_snapshot, snapshot_is_fresh, PersistedCiState, PersistedRepoCi, RemoteActivitySnapshot,
    REMOTE_ACTIVITY_SNAPSHOT_FILE_NAME, REMOTE_ACTIVITY_SNAPSHOT_MAX_AGE_SECS,
};
use crate::config::sessions_persistence::{load_sessions_raw, PersistedSession};
use crate::session::session::persisted_is_working;

use super::{print_json, resolve_cli_ac_root, resolve_cli_project, WorkgroupActivityArgs};

/// One room's aggregate activity. `working` is a persisted observation only,
/// never a proxy: an idle coordinator does not make its room work. `ci_state`
/// is exactly `running`, `idle` or `unknown`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WorkgroupActivityItem {
    pub(super) name: String,
    pub(super) team: String,
    pub(super) working: bool,
    pub(super) ci_state: &'static str,
    pub(super) task_title: Option<String>,
    /// #2473: sorted, deduped in-progress run ids; non-empty only when running.
    pub(super) ci_run_ids: Vec<u64>,
    /// #2473: sorted, deduped PR numbers of those runs; non-empty only when running.
    pub(super) ci_pull_requests: Vec<u64>,
    /// #2473: why `ci_state` is `unknown`; `None` otherwise.
    pub(super) ci_unknown_reason: Option<&'static str>,
}

/// The parsed `room activity` filters (#2473). Every present filter must match
/// (AND); an absent one matches everything.
#[derive(Debug, Default)]
pub(super) struct ActivityFilter {
    pub(super) rooms: Option<BTreeSet<u32>>,
    pub(super) team: Option<String>,
    pub(super) working: Option<bool>,
    pub(super) ci_state: Option<&'static str>,
    pub(super) title_regex: Option<regex::Regex>,
    pub(super) hide_clean: bool,
}

impl ActivityFilter {
    pub(super) fn matches(&self, item: &WorkgroupActivityItem) -> bool {
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
pub(super) fn title_is_clean(title: Option<&str>) -> bool {
    match title.map(str::trim) {
        None => true,
        Some(title) => title.is_empty() || title.eq_ignore_ascii_case("clean"),
    }
}

/// The N of `room-N-<team>` / `wg-N-<team>`; `None` for any other shape.
pub(super) fn room_number(name: &str) -> Option<u32> {
    let rest = crate::config::entity_prefix::strip_entity_prefix(name)?;
    let (number, _) = rest.split_once('-')?;
    number.parse::<u32>().ok()
}

/// Validate every filter value in a fixed order (rooms, team, working,
/// ci-state, title-regex); the first failure is the error.
pub(super) fn parse_activity_filter(
    args: &WorkgroupActivityArgs,
) -> Result<ActivityFilter, String> {
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
                            "Invalid --rooms '{raw}': expected \
                             comma-separated positive room numbers, e.g. 5,12,17"
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

/// `room activity` / `workgroup activity`: every room of the project, in the
/// same enumeration order `room list` uses, with aggregate working state, CI
/// state and TASK title. Read-only: no TASK, cache, session, daemon or project
/// write belongs to this verb.
pub(super) fn activity(args: WorkgroupActivityArgs) -> Result<(), String> {
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
                    "room activity: AgentsCommander daemon is not live ({daemon_state:?}); \
                     ciState is reported as \"unknown\""
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
pub(super) enum CiGate {
    /// The snapshot may be read (it still has to be current).
    Usable,
    /// `ciActivityEnabled` is false: no producer exists to trust.
    CiDisabled,
    /// The daemon is not live: the snapshot is not being refreshed.
    DaemonNotLive,
}

pub(super) fn ci_gate(ci_activity_enabled: bool, daemon_live: bool) -> CiGate {
    if !ci_activity_enabled {
        CiGate::CiDisabled
    } else if !daemon_live {
        CiGate::DaemonNotLive
    } else {
        CiGate::Usable
    }
}

pub(super) fn daemon_state_is_live(state: &DaemonState) -> bool {
    matches!(state, DaemonState::Running { .. })
}

/// One runtime warning. When `AC_MACHINE_OUTPUT` is set the caller asked for
/// log-only warnings; otherwise stderr is the channel. stdout is never used.
pub(super) fn warn_activity(message: &str) {
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
pub(super) fn read_fresh_ci_snapshot(
    dir: Option<&Path>,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<RemoteActivitySnapshot, String> {
    let dir = dir.ok_or_else(|| "the instance config directory is unavailable".to_string())?;
    let path = dir.join(REMOTE_ACTIVITY_SNAPSHOT_FILE_NAME);
    let snapshot = read_snapshot(&path)?;
    if !snapshot_is_fresh(&snapshot, now) {
        return Err(format!(
            "the remote activity snapshot is not current \
             (window {REMOTE_ACTIVITY_SNAPSHOT_MAX_AGE_SECS}s)"
        ));
    }
    Ok(snapshot)
}

/// One canonical key's CI view after duplicate folding (#2473).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct IndexedRepoCi {
    pub(super) ci: PersistedRepoCi,
    /// Two entries for this key disagreed on `state`.
    pub(super) conflicting: bool,
}

/// Canonical-key index over the snapshot's path/CI pairs. A duplicate key
/// with conflicting states collapses to `Unknown`: two observations that
/// disagree are not authority. Duplicates with the SAME state are not a
/// conflict: run ids / PRs are unioned and the first reason is kept, so
/// metadata can never change the state.
pub(super) fn index_ci_entries(
    snapshot: &RemoteActivitySnapshot,
) -> HashMap<String, IndexedRepoCi> {
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
pub(super) struct RoomCi {
    pub(super) state: &'static str,
    pub(super) run_ids: Vec<u64>,
    pub(super) pull_requests: Vec<u64>,
    pub(super) unknown_reason: Option<&'static str>,
}

/// The producer's persisted `ci-query-*` codes; anything else in the file is
/// reported as a plain explicit unknown.
pub(super) const PERSISTED_CI_REASONS: [&str; 6] = [
    "ci-query-timeout",
    "ci-query-rate-limited",
    "ci-query-secondary-rate-limited",
    "ci-query-not-authenticated",
    "ci-query-incomplete",
    "ci-query-failed",
];

/// Why one repo is neither idle nor running; `None` when it is.
pub(super) fn repo_unknown_reason(entry: Option<&IndexedRepoCi>) -> Option<&'static str> {
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
pub(super) fn aggregate_room_ci(
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
pub(super) fn strip_windows_verbatim_prefix(path: &str) -> String {
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
pub(super) fn room_is_working(
    room_dir: &Path,
    room_name: &str,
    sessions: &[PersistedSession],
) -> bool {
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
pub(super) fn list_replica_activity_dirs(wg_dir: &Path) -> Vec<(PathBuf, String)> {
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
pub(super) fn list_room_repo_dirs(wg_dir: &Path) -> Vec<PathBuf> {
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
pub(super) fn read_task_title(room_dir: &Path) -> Option<String> {
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
