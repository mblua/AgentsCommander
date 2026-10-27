use super::super::names::{
    normalize_context_alert_percentages, normalize_team_agent_refs, resolve_agent_ref,
    validate_existing_name, RepoAssignment, TeamConfigResult,
};
use super::conventions;
use crate::config::entity_creation::copies::{verify_owned_entry, verify_team_copies};
use crate::filesystem::projects::entities::copies::{
    write_owned_team_file, MatrixCopyHooks, MatrixCopyKind, MatrixCopyStage, OwnedCopyArtifacts,
    OwnedCopyEntry,
};
use crate::filesystem::projects::entities::publication;
use crate::filesystem::projects::entities::team_lock::{self, LockCause, LockFailure, LockStep};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub(crate) const TEAM_CONFIG_MUTATION_LOCK_NAME: &str = ".team-config-write.lock";

pub(crate) const TEAM_CONFIG_LOCK_POLL_INTERVAL: Duration = Duration::from_millis(50);

pub(crate) const TEAM_CONFIG_LOCK_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy)]
pub(crate) struct TeamConfigLockTiming {
    pub(crate) poll_interval: Duration,
    pub(crate) timeout: Duration,
}

impl TeamConfigLockTiming {
    const PRODUCTION: Self = Self {
        poll_interval: TEAM_CONFIG_LOCK_POLL_INTERVAL,
        timeout: TEAM_CONFIG_LOCK_TIMEOUT,
    };
}

#[derive(Debug)]
pub(crate) struct TeamConfigMutationGuard {
    _lock: team_lock::LockedTeam,
}

impl TeamConfigMutationGuard {
    pub(crate) fn acquire(ac_root: &Path) -> Result<Self, String> {
        Self::acquire_with_timing(ac_root, TeamConfigLockTiming::PRODUCTION)
    }

    pub(crate) fn acquire_with_timing(
        ac_root: &Path,
        timing: TeamConfigLockTiming,
    ) -> Result<Self, String> {
        let lock = team_lock::acquire(
            ac_root,
            TEAM_CONFIG_MUTATION_LOCK_NAME,
            timing.poll_interval,
            timing.timeout,
        )
        .map_err(map_lock_failure)?;
        Ok(Self { _lock: lock })
    }
    pub(crate) fn require_ac_root(&self, ac_root: &Path) -> Result<(), String> {
        self._lock
            .require_workspace(ac_root)
            .map_err(map_lock_failure)
    }
}

fn map_lock_failure(failure: LockFailure) -> String {
    let LockFailure { step, path, cause } = failure;
    match (step, cause) {
        (LockStep::ResolveWorkspace, LockCause::Io(e)) => format!(
            "Failed to resolve workspace for team config lock '{}': {}",
            path.display(),
            e
        ),
        (LockStep::ResolveWorkspace, LockCause::NotDirectory) => format!(
            "Failed to open team config lock '{}': workspace is not a directory",
            path.display()
        ),
        (LockStep::OpenExisting, LockCause::Io(open_error)) => format!(
            "Failed to open existing team config lock '{}': {}",
            path.display(),
            open_error
        ),
        (LockStep::CreateSentinel, LockCause::Io(create_error)) => format!(
            "Failed to create team config lock '{}': {}",
            path.display(),
            create_error
        ),
        (LockStep::InspectSentinel, LockCause::Io(e)) => format!(
            "Failed to inspect team config lock '{}': {}",
            path.display(),
            e
        ),
        (LockStep::InspectOpened, LockCause::Io(e)) => format!(
            "Failed to inspect opened team config lock '{}': {}",
            path.display(),
            e
        ),
        (LockStep::InspectOpened, LockCause::NonRegular) => format!(
            "Team config lock '{}' must be a regular file",
            path.display()
        ),
        (LockStep::Acquire, LockCause::Timeout { milliseconds }) => format!(
            "Timed out after {} ms waiting for team config lock '{}'",
            milliseconds,
            path.display()
        ),
        (LockStep::Acquire, LockCause::Io(e)) => format!(
            "Failed to acquire team config lock '{}': {}",
            path.display(),
            e
        ),
        (LockStep::VerifyWorkspace, LockCause::Io(e)) => format!(
            "Failed to verify workspace for team config lock '{}': {}",
            path.display(),
            e
        ),
        (LockStep::VerifyWorkspace, LockCause::WrongWorkspace { expected, actual }) => format!(
            "Team config lock '{}' belongs to workspace '{}', not '{}'",
            path.display(),
            expected.display(),
            actual.display()
        ),

        (LockStep::InspectSentinel, LockCause::NonRegularOrLink) => format!(
            "Team config lock '{}' must be a regular non-link file",
            path.display()
        ),
        (LockStep::ResolveSentinel, LockCause::Io(e)) => format!(
            "Failed to resolve team config lock '{}': {}",
            path.display(),
            e
        ),
        (LockStep::ResolveSentinel, LockCause::MissingParent) => format!(
            "Team config lock '{}' has no canonical parent",
            path.display()
        ),
        (LockStep::ResolveSentinel, LockCause::EscapedParent { canonical_root }) => format!(
            "Team config lock '{}' escapes canonical workspace '{}'",
            path.display(),
            canonical_root.display()
        ),
        _ => unreachable!("lock failure step/cause contract"),
    }
}

pub(crate) async fn acquire_team_config_mutation_guard_async(
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
    let config_path = config_path(ac_root, team_name);
    validate_existing_name(team_name, "Team").map_err(|cause| TeamConfigReadError::Invalid {
        path: config_path.clone(),
        cause,
    })?;
    let content = publication::read_team_config_bytes(&config_path).map_err(|source| {
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

pub(crate) fn normalized_team_config_bytes(
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

pub(crate) fn write_existing_team_config_guarded(
    ac_root: &Path,
    team_name: &str,
    config: &TeamConfigResult,
    guard: &TeamConfigMutationGuard,
) -> Result<PathBuf, String> {
    guard.require_ac_root(ac_root)?;
    validate_existing_name(team_name, "Team")?;
    let team_dir = ac_root.join(format!("_team_{}", team_name));
    if !publication::team_directory_present(&team_dir) {
        return Err(format!("Team '{}' not found", team_name));
    }
    write_team_config_guarded(ac_root, team_name, config, guard)
}

pub(crate) fn write_team_config_guarded_with_publisher<P>(
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
    publication::ensure_team_directory(&team_dir)
        .map_err(|e| format!("Failed to create team directory: {}", e))?;
    publication::ensure_team_memory_directory(&team_dir)
        .map_err(|e| format!("Failed to create memory directory: {}", e))?;
    conventions::ensure_empty_if_absent(&team_dir)?;
    publication::publish_team_config(&config_path(ac_root, team_name), &config_bytes, publish)
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
    publication::create_team_directory(&team_dir).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => format!("Team '{}' already exists", team_name),
        _ => format!("Failed to create team directory: {}", e),
    })?;
    publication::create_team_memory_directory(&team_dir)
        .map_err(|e| format!("Failed to create memory directory: {}", e))?;
    conventions::write_empty_for_new_team(&team_dir)?;
    publication::publish_team_config(
        &config_path(ac_root, team_name),
        &config_bytes,
        crate::config::local_config_io::write_file_atomic,
    )
    .map_err(|e| format!("Failed to publish config.json: {}", e))?;
    Ok(team_dir)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextAlertWriteMode {
    Create,
    Update,
}

pub(crate) fn resolve_context_alert_argument(
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

pub(crate) fn config_path(ac_root: &Path, team_name: &str) -> PathBuf {
    ac_root
        .join(format!("_team_{}", team_name))
        .join("config.json")
}

pub(crate) fn publish_owned_team(
    owned: &mut OwnedCopyArtifacts,
    bytes: &[u8],
    copies: &[OwnedCopyArtifacts],
    hooks: &mut MatrixCopyHooks<'_>,
) -> Result<(), String> {
    owned.create_directory(Path::new(""))?;
    owned.create_directory(Path::new("memory"))?;
    hooks.run(MatrixCopyStage::TeamConventions, &owned.root)?;
    conventions::write_owned_empty(owned)?;
    let temporary = PathBuf::from(format!(".config-{}.tmp", uuid::Uuid::new_v4()));
    write_owned_team_file(owned, &temporary, bytes)?;
    hooks.run(MatrixCopyStage::TeamPublication, &owned.root)?;
    verify_team_copies(copies)?;
    for entry in &owned.entries {
        verify_owned_entry(owned, entry)?;
    }
    let temporary_entry = owned.entries.last().expect("temporary entry");
    let identity = temporary_entry.identity;
    let digest = temporary_entry.digest;
    let config = owned.root.join("config.json");
    publication::link_owned_team_config(&owned.root.join(&temporary), &config)
        .map_err(|e| format!("Failed to publish config.json: {e}"))?;
    owned.entries.push(OwnedCopyEntry {
        relative: PathBuf::from("config.json"),
        identity,
        kind: MatrixCopyKind::File,
        digest,
    });
    hooks.run(MatrixCopyStage::TeamUnlink, &owned.root)?;
    // Verify both names and their directory chain before removing our temporary.
    for entry in owned.entries.iter().rev().take(2) {
        verify_owned_entry(owned, entry)?;
    }
    publication::unlink_owned_team_temporary(&owned.root.join(&temporary))
        .map_err(|e| e.to_string())?;
    owned.entries.retain(|entry| entry.relative != temporary);
    Ok(())
}

#[cfg(test)]
mod tests;
