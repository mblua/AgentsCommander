use super::delete_guard::metadata_is_link_or_reparse;
use super::matrix_layout::validate_existing_name;
use super::{RepoAssignment, TeamConfigResult};
use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub(super) const TEAM_CONFIG_MUTATION_LOCK_NAME: &str = ".team-config-write.lock";

pub(super) const TEAM_CONFIG_LOCK_POLL_INTERVAL: Duration = Duration::from_millis(50);

pub(super) const TEAM_CONFIG_LOCK_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy)]
pub(super) struct TeamConfigLockTiming {
    pub(super) poll_interval: Duration,
    pub(super) timeout: Duration,
}

impl TeamConfigLockTiming {
    const PRODUCTION: Self = Self {
        poll_interval: TEAM_CONFIG_LOCK_POLL_INTERVAL,
        timeout: TEAM_CONFIG_LOCK_TIMEOUT,
    };
}

#[derive(Debug)]
pub(crate) struct TeamConfigMutationGuard {
    pub(super) _file: File,
    pub(super) canonical_ac_root: PathBuf,
    pub(super) lock_path: PathBuf,
}

impl TeamConfigMutationGuard {
    pub(crate) fn acquire(ac_root: &Path) -> Result<Self, String> {
        Self::acquire_with_timing(ac_root, TeamConfigLockTiming::PRODUCTION)
    }

    pub(super) fn acquire_with_timing(
        ac_root: &Path,
        timing: TeamConfigLockTiming,
    ) -> Result<Self, String> {
        let requested_lock_path = ac_root.join(TEAM_CONFIG_MUTATION_LOCK_NAME);
        let canonical_ac_root = std::fs::canonicalize(ac_root)
            .map(|path| crate::path_utils::normalize_windows_verbatim_path_buf(&path))
            .map_err(|e| {
                format!(
                    "Failed to resolve workspace for team config lock '{}': {}",
                    requested_lock_path.display(),
                    e
                )
            })?;
        if !canonical_ac_root.is_dir() {
            return Err(format!(
                "Failed to open team config lock '{}': workspace is not a directory",
                requested_lock_path.display()
            ));
        }
        let lock_path = canonical_ac_root.join(TEAM_CONFIG_MUTATION_LOCK_NAME);

        let open_existing = || -> Result<File, String> {
            validate_team_config_lock_sentinel(&lock_path, &canonical_ac_root)?;
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(&lock_path)
                .map_err(|open_error| {
                    format!(
                        "Failed to open existing team config lock '{}': {}",
                        lock_path.display(),
                        open_error
                    )
                })
        };
        let file = match std::fs::symlink_metadata(&lock_path) {
            Ok(_) => open_existing()?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                match OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create_new(true)
                    .open(&lock_path)
                {
                    Ok(file) => file,
                    Err(create_error)
                        if create_error.kind() == std::io::ErrorKind::AlreadyExists =>
                    {
                        open_existing()?
                    }
                    Err(create_error) => {
                        return Err(format!(
                            "Failed to create team config lock '{}': {}",
                            lock_path.display(),
                            create_error
                        ));
                    }
                }
            }
            Err(e) => {
                return Err(format!(
                    "Failed to inspect team config lock '{}': {}",
                    lock_path.display(),
                    e
                ));
            }
        };
        validate_team_config_lock_sentinel(&lock_path, &canonical_ac_root)?;
        if !file
            .metadata()
            .map_err(|e| {
                format!(
                    "Failed to inspect opened team config lock '{}': {}",
                    lock_path.display(),
                    e
                )
            })?
            .is_file()
        {
            return Err(format!(
                "Team config lock '{}' must be a regular file",
                lock_path.display()
            ));
        }

        let started = Instant::now();
        loop {
            match file.try_lock() {
                Ok(()) => break,
                Err(TryLockError::WouldBlock) if started.elapsed() >= timing.timeout => {
                    return Err(format!(
                        "Timed out after {} ms waiting for team config lock '{}'",
                        timing.timeout.as_millis(),
                        lock_path.display()
                    ));
                }
                Err(TryLockError::WouldBlock) => {
                    let remaining = timing.timeout.saturating_sub(started.elapsed());
                    let delay = timing.poll_interval.min(remaining);
                    if delay.is_zero() {
                        std::thread::yield_now();
                    } else {
                        std::thread::sleep(delay);
                    }
                }
                Err(TryLockError::Error(e)) => {
                    return Err(format!(
                        "Failed to acquire team config lock '{}': {}",
                        lock_path.display(),
                        e
                    ));
                }
            }
        }

        Ok(Self {
            _file: file,
            canonical_ac_root,
            lock_path,
        })
    }

    pub(super) fn require_ac_root(&self, ac_root: &Path) -> Result<(), String> {
        let canonical = std::fs::canonicalize(ac_root)
            .map(|path| crate::path_utils::normalize_windows_verbatim_path_buf(&path))
            .map_err(|e| {
                format!(
                    "Failed to verify workspace for team config lock '{}': {}",
                    self.lock_path.display(),
                    e
                )
            })?;
        if canonical != self.canonical_ac_root {
            return Err(format!(
                "Team config lock '{}' belongs to workspace '{}', not '{}'",
                self.lock_path.display(),
                self.canonical_ac_root.display(),
                canonical.display()
            ));
        }
        Ok(())
    }
}

pub(super) fn validate_team_config_lock_sentinel(
    lock_path: &Path,
    canonical_ac_root: &Path,
) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(lock_path).map_err(|e| {
        format!(
            "Failed to inspect team config lock '{}': {}",
            lock_path.display(),
            e
        )
    })?;
    if metadata_is_link_or_reparse(&metadata) || !metadata.is_file() {
        return Err(format!(
            "Team config lock '{}' must be a regular non-link file",
            lock_path.display()
        ));
    }
    let canonical_lock = std::fs::canonicalize(lock_path)
        .map(|path| crate::path_utils::normalize_windows_verbatim_path_buf(&path))
        .map_err(|e| {
            format!(
                "Failed to resolve team config lock '{}': {}",
                lock_path.display(),
                e
            )
        })?;
    let canonical_parent = canonical_lock.parent().ok_or_else(|| {
        format!(
            "Team config lock '{}' has no canonical parent",
            lock_path.display()
        )
    })?;
    if canonical_parent != canonical_ac_root {
        return Err(format!(
            "Team config lock '{}' escapes canonical workspace '{}'",
            lock_path.display(),
            canonical_ac_root.display()
        ));
    }
    Ok(())
}

pub(super) async fn acquire_team_config_mutation_guard_async(
    ac_root: &Path,
) -> Result<TeamConfigMutationGuard, String> {
    let ac_root = ac_root.to_path_buf();
    let ac_root_label = ac_root.display().to_string();
    tokio::task::spawn_blocking(move || TeamConfigMutationGuard::acquire(&ac_root))
        .await
        .map_err(|e| {
            format!(
                "Failed to acquire team config lock for workspace '{}': blocking task failed: {}",
                ac_root_label, e
            )
        })?
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum TeamConfigReadError {
    #[error("Team config not found at '{path}': {source}")]
    NotFound {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Team config at '{path}' is unreadable: {source}")]
    Unreadable {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Team config at '{path}' is malformed: {source}")]
    Malformed {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("Team config at '{path}' is invalid: {cause}")]
    Invalid { path: PathBuf, cause: String },
}

impl TeamConfigReadError {
    // Consumed by the Phase 2 runtime; Phase 1 pins the stable classification contract.
    pub(crate) fn class(&self) -> &'static str {
        match self {
            Self::NotFound { .. } => "not_found",
            Self::Unreadable { .. } => "unreadable",
            Self::Malformed { .. } => "malformed",
            Self::Invalid { .. } => "invalid",
        }
    }
}

pub(crate) fn read_team_config_classified(
    ac_root: &Path,
    team_name: &str,
) -> Result<TeamConfigResult, TeamConfigReadError> {
    let config_path = ac_root
        .join(format!("_team_{}", team_name))
        .join("config.json");
    validate_existing_name(team_name, "Team").map_err(|cause| TeamConfigReadError::Invalid {
        path: config_path.clone(),
        cause,
    })?;
    let content = std::fs::read(&config_path).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            TeamConfigReadError::NotFound {
                path: config_path.clone(),
                source,
            }
        } else {
            TeamConfigReadError::Unreadable {
                path: config_path.clone(),
                source,
            }
        }
    })?;
    let config: TeamConfigResult =
        serde_json::from_slice(&content).map_err(|source| TeamConfigReadError::Malformed {
            path: config_path.clone(),
            source,
        })?;
    normalize_team_config_for_project(ac_root, &config).map_err(|cause| {
        TeamConfigReadError::Invalid {
            path: config_path,
            cause,
        }
    })
}

pub(crate) fn read_team_config(
    ac_root: &Path,
    team_name: &str,
) -> Result<TeamConfigResult, String> {
    read_team_config_classified(ac_root, team_name).map_err(|e| e.to_string())
}

pub(super) fn normalized_team_config_bytes(
    ac_root: &Path,
    config: &TeamConfigResult,
) -> Result<Vec<u8>, String> {
    let normalized = normalize_team_config_for_project(ac_root, config)?;
    serde_json::to_vec_pretty(&normalized)
        .map_err(|e| format!("Failed to serialize config.json: {}", e))
}

pub(crate) fn write_team_config_guarded(
    ac_root: &Path,
    team_name: &str,
    config: &TeamConfigResult,
    guard: &TeamConfigMutationGuard,
) -> Result<PathBuf, String> {
    write_team_config_guarded_with_publisher(
        ac_root,
        team_name,
        config,
        guard,
        crate::config::local_config_io::write_file_atomic,
    )
}

pub(super) fn write_existing_team_config_guarded(
    ac_root: &Path,
    team_name: &str,
    config: &TeamConfigResult,
    guard: &TeamConfigMutationGuard,
) -> Result<PathBuf, String> {
    guard.require_ac_root(ac_root)?;
    validate_existing_name(team_name, "Team")?;
    let team_dir = ac_root.join(format!("_team_{}", team_name));
    if !team_dir.exists() {
        return Err(format!("Team '{}' not found", team_name));
    }
    write_team_config_guarded(ac_root, team_name, config, guard)
}

pub(super) fn write_team_config_guarded_with_publisher<P>(
    ac_root: &Path,
    team_name: &str,
    config: &TeamConfigResult,
    guard: &TeamConfigMutationGuard,
    publish: P,
) -> Result<PathBuf, String>
where
    P: FnOnce(&Path, &[u8]) -> Result<(), String>,
{
    guard.require_ac_root(ac_root)?;
    validate_existing_name(team_name, "Team")?;
    let config_bytes = normalized_team_config_bytes(ac_root, config)?;
    let team_dir = ac_root.join(format!("_team_{}", team_name));
    std::fs::create_dir_all(&team_dir)
        .map_err(|e| format!("Failed to create team directory: {}", e))?;
    std::fs::create_dir_all(team_dir.join("memory"))
        .map_err(|e| format!("Failed to create memory directory: {}", e))?;
    let conventions = team_dir.join("conventions.md");
    if !conventions.exists() {
        std::fs::write(&conventions, "")
            .map_err(|e| format!("Failed to write conventions.md: {}", e))?;
    }
    publish(&team_dir.join("config.json"), &config_bytes)
        .map_err(|e| format!("Failed to publish config.json: {}", e))?;
    Ok(team_dir)
}

pub(crate) fn create_new_team_config_on_disk(
    ac_root: &Path,
    team_name: &str,
    config: &TeamConfigResult,
) -> Result<PathBuf, String> {
    let guard = TeamConfigMutationGuard::acquire(ac_root)?;
    create_new_team_config_on_disk_guarded(ac_root, team_name, config, &guard)
}

pub(crate) fn create_new_team_config_on_disk_guarded(
    ac_root: &Path,
    team_name: &str,
    config: &TeamConfigResult,
    guard: &TeamConfigMutationGuard,
) -> Result<PathBuf, String> {
    guard.require_ac_root(ac_root)?;
    validate_existing_name(team_name, "Team")?;
    let config_bytes = normalized_team_config_bytes(ac_root, config)?;
    let team_dir = ac_root.join(format!("_team_{}", team_name));
    std::fs::create_dir(&team_dir).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => format!("Team '{}' already exists", team_name),
        _ => format!("Failed to create team directory: {}", e),
    })?;
    std::fs::create_dir(team_dir.join("memory"))
        .map_err(|e| format!("Failed to create memory directory: {}", e))?;
    std::fs::write(team_dir.join("conventions.md"), "")
        .map_err(|e| format!("Failed to write conventions.md: {}", e))?;
    crate::config::local_config_io::write_file_atomic(&team_dir.join("config.json"), &config_bytes)
        .map_err(|e| format!("Failed to publish config.json: {}", e))?;
    Ok(team_dir)
}

pub(crate) fn normalize_context_alert_percentages(values: &[u8]) -> Result<Vec<u8>, String> {
    if values.len() > 3 {
        return Err("contextAlertPercentages must contain at most 3 values".to_string());
    }
    if values.iter().any(|value| !(1..=100).contains(value)) {
        return Err(
            "contextAlertPercentages values must be integer percentages from 1 through 100"
                .to_string(),
        );
    }
    let mut normalized = values.to_vec();
    normalized.sort_unstable();
    if normalized.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err("contextAlertPercentages values must be distinct".to_string());
    }
    Ok(normalized)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ContextAlertWriteMode {
    Create,
    Update,
}

pub(super) fn resolve_context_alert_argument(
    mode: ContextAlertWriteMode,
    supplied: Option<Vec<u8>>,
    current: Option<&[u8]>,
) -> Result<Vec<u8>, String> {
    match supplied {
        Some(values) => normalize_context_alert_percentages(&values),
        None if mode == ContextAlertWriteMode::Create => Ok(Vec::new()),
        None => current.map(|values| values.to_vec()).ok_or_else(|| {
            "Current team config is required when contextAlertPercentages is omitted on update"
                .to_string()
        }),
    }
}

pub(crate) fn normalize_team_config_for_project(
    ac_root: &Path,
    config: &TeamConfigResult,
) -> Result<TeamConfigResult, String> {
    let agents = normalize_team_agent_refs(ac_root, &config.agents)?;
    let coordinator = if config.coordinator.trim().is_empty() {
        String::new()
    } else {
        resolve_agent_ref(ac_root, &config.coordinator)?
    };
    if !coordinator.is_empty() && !agents.contains(&coordinator) {
        return Err("Orchestrator must be one of the selected agents".to_string());
    }
    let mut repos = Vec::with_capacity(config.repos.len());
    for repo in &config.repos {
        repos.push(RepoAssignment {
            url: repo.url.clone(),
            agents: normalize_team_agent_refs(ac_root, &repo.agents)?,
        });
    }
    let context_alert_percentages =
        normalize_context_alert_percentages(&config.context_alert_percentages)?;
    Ok(TeamConfigResult {
        agents,
        coordinator,
        repos,
        context_alert_percentages,
    })
}

pub(super) fn normalize_team_agent_refs(
    ac_root: &Path,
    refs: &[String],
) -> Result<Vec<String>, String> {
    let mut normalized = Vec::new();
    for agent in refs {
        let resolved = resolve_agent_ref(ac_root, agent)?;
        if !normalized.contains(&resolved) {
            normalized.push(resolved);
        }
    }
    Ok(normalized)
}

pub(crate) fn resolve_agent_ref(ac_root: &Path, raw_agent: &str) -> Result<String, String> {
    let trimmed = raw_agent.trim();
    if trimmed.is_empty() {
        return Err("Agent reference cannot be empty".to_string());
    }
    if trimmed.contains('\0') {
        return Err("Agent reference must not contain NUL".to_string());
    }
    if trimmed.contains('/') || trimmed.contains('\\') || trimmed.contains(':') {
        return Err(format!(
            "Invalid agent reference '{}': team configs must use portable refs like '_agent_name' \
             or 'name', not filesystem paths",
            raw_agent
        ));
    }
    if Path::new(trimmed).is_absolute() {
        return Err(format!(
            "Invalid agent reference '{}': absolute paths are not allowed in team configs",
            raw_agent
        ));
    }
    let bare = trimmed.strip_prefix("_agent_").unwrap_or(trimmed);
    validate_existing_name(bare, "Agent")?;
    let canonical_ref = format!("_agent_{}", bare);
    let matrix_dir = ac_root.join(&canonical_ref);
    if !matrix_dir.is_dir() {
        return Err(format!("Agent '{}' not found", bare));
    }
    Ok(canonical_ref)
}
