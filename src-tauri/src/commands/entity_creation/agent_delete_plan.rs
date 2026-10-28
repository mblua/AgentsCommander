use super::delete_guard::{
    path_key_for_delete, target_keys_for_delete, validate_delete_root_not_link_or_reparse,
};
use super::matrix_layout::validate_existing_name;
use super::team_config::TeamConfigMutationGuard;
use super::workgroup::list_workgroup_dirs;
use crate::config::entity_creation::teams::collect_agent_team_mutations_raw;
pub(super) use crate::config::entity_creation::teams::AgentTeamMutation;
use crate::config::entity_creation::{directories, teams};
use crate::config::settings::{AppSettings, SettingsState};
use crate::session::manager::SessionManager;
use crate::session::session::SessionStatus;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
#[cfg(test)]
use std::sync::atomic::Ordering;
use std::sync::Arc;

pub(super) fn resolve_agent_delete_identity(
    base: &Path,
    agent_path: &Path,
) -> Result<(String, String, PathBuf), String> {
    if !agent_path.is_absolute() {
        return Err("Agent path must be absolute".to_string());
    }
    let final_segment = agent_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "Agent path must end with an agent matrix directory".to_string())?;
    let agent_name = final_segment
        .strip_prefix("_agent_")
        .ok_or_else(|| "Agent path must end with _agent_<name>".to_string())?;
    validate_existing_name(agent_name, "Agent")?;
    validate_delete_root_not_link_or_reparse(agent_path)
        .map_err(|e| format!("Selected agent path is not deletable: {}", e))?;

    let agent_ref = format!("_agent_{}", agent_name);
    let origin_dir = base.join(&agent_ref);
    if path_key_for_delete(agent_path) != path_key_for_delete(&origin_dir) {
        return Err("Agent path does not match the selected project".to_string());
    }
    validate_delete_root_not_link_or_reparse(&origin_dir)
        .map_err(|e| format!("Agent '{}' not found or not deletable: {}", agent_name, e))?;

    Ok((agent_name.to_string(), agent_ref, origin_dir))
}

#[cfg(test)]
pub(super) fn collect_agent_delete_plan(
    base: &Path,
    agent_path: &Path,
) -> Result<AgentDeletePlan, String> {
    let guard = TeamConfigMutationGuard::acquire(base)?;
    collect_agent_delete_plan_guarded(base, agent_path, &guard)
}

pub(super) fn collect_agent_delete_plan_guarded(
    base: &Path,
    agent_path: &Path,
    guard: &TeamConfigMutationGuard,
) -> Result<AgentDeletePlan, String> {
    guard.require_ac_root(base)?;
    let (agent_name, agent_ref, origin_dir) = resolve_agent_delete_identity(base, agent_path)?;
    let team_mutations = collect_agent_team_mutations_raw(base, &agent_name, &agent_ref)?;

    let mut targets = vec![AgentDeleteTarget {
        original_key: path_key_for_delete(&origin_dir),
        original_path: origin_dir.clone(),
        label: agent_ref.clone(),
    }];

    for wg_dir in list_workgroup_dirs(base) {
        let replica = wg_dir.join(format!("__agent_{}", agent_name));
        // #1063: exact `symlink_metadata` handling instead of `Path::exists`, which
        // collapses access and other inspection errors to false. Exact `NotFound`
        // means absent and adds no target (committed-intent pruning by agent
        // component still covers any stale rows at commit); every other inspection
        // error aborts preflight; an existing entry must pass the non-reparse
        // directory validation before its prefix is retained as a target.
        match directories::inspect_agent_replica(&replica) {
            Ok(_) => {
                validate_delete_root_not_link_or_reparse(&replica).map_err(|e| {
                    format!(
                        "Agent replica '{}' is not deletable: {}",
                        replica.display(),
                        e
                    )
                })?;
                let wg_name = wg_dir
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("workgroup");
                targets.push(AgentDeleteTarget {
                    original_key: path_key_for_delete(&replica),
                    original_path: replica,
                    label: format!("{}/__agent_{}", wg_name, agent_name),
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(format!(
                    "Failed to inspect agent replica '{}': {}",
                    replica.display(),
                    e
                ));
            }
        }
    }

    targets.sort_by(|a, b| a.original_key.cmp(&b.original_key));
    let mut target_keys = Vec::new();
    for target in &targets {
        for key in target_keys_for_delete(&target.original_path) {
            if !target_keys.contains(&key) {
                target_keys.push(key);
            }
        }
    }

    Ok(AgentDeletePlan {
        ac_root: base.to_path_buf(),
        agent_name,
        agent_ref,
        origin_dir,
        target_keys,
        targets,
        team_mutations,
    })
}

pub(super) async fn preflight_agent_delete(
    plan: &AgentDeletePlan,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
) -> Result<(), String> {
    for target in &plan.targets {
        validate_delete_root_not_link_or_reparse(&target.original_path).map_err(|e| {
            format!(
                "Cannot delete agent '{}': target '{}' is not deletable: {}",
                plan.agent_name, target.label, e
            )
        })?;
    }
    ensure_no_live_sessions_under_target_keys(&plan.target_keys, &plan.agent_name, session_mgr)
        .await
}

pub(super) async fn ensure_no_live_sessions_under_target_keys(
    target_keys: &[String],
    agent_name: &str,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
) -> Result<(), String> {
    let manager = { session_mgr.read().await.clone() };
    let sessions = manager.list_sessions().await;
    let blockers: Vec<_> = sessions
        .into_iter()
        .filter(|session| !matches!(session.status, SessionStatus::Exited(_)))
        .filter(|session| {
            let working_dir = Path::new(&session.working_directory);
            let working_key = path_key_for_delete(working_dir);
            target_keys.iter().any(|root_key| {
                working_key == *root_key || working_key.starts_with(&(root_key.clone() + "/"))
            })
        })
        .collect();

    if blockers.is_empty() {
        return Ok(());
    }

    let details = blockers
        .iter()
        .map(|session| {
            format!(
                "  - {} at {} ({:?})",
                session.name, session.working_directory, session.status
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Err(format!(
        "Cannot delete agent '{}' while live sessions exist:\n{}",
        agent_name, details
    ))
}

pub(super) async fn prepare_agent_delete_metadata(
    plan: &AgentDeletePlan,
    settings: &SettingsState,
) -> Result<PreparedAgentDeleteMetadata, String> {
    let settings_snapshot = settings.read().await;
    let settings_changed = settings_snapshot
        .auto_self_clear_by_agent
        .contains_key(&plan.agent_name)
        || settings_snapshot
            .coding_agent_profiles
            .default_profile_by_agent
            .contains_key(&plan.agent_name);

    Ok(PreparedAgentDeleteMetadata {
        ac_root: plan.ac_root.clone(),
        team_mutations: plan.team_mutations.clone(),
        agent_name: plan.agent_name.clone(),
        settings_changed: Arc::new(AtomicBool::new(settings_changed)),
    })
}

// #1063: the async agent-delete metadata helpers below are superseded in
// production by `persist_agent_delete_metadata_blocking` (run under the blocking
// project→team→settings transaction). They are retained only as `#[cfg(test)]`
// scaffolding for the pre-existing focused rollback/restore/format tests.
#[cfg(test)]
pub(super) async fn persist_agent_delete_metadata_with_saver<S>(
    metadata: &PreparedAgentDeleteMetadata,
    settings: &SettingsState,
    team_guard: &TeamConfigMutationGuard,
    save_settings_fn: S,
) -> Result<(), String>
where
    S: Fn(&AppSettings) -> Result<AppSettings, String>,
{
    persist_agent_delete_metadata_with_writers(
        metadata,
        settings,
        team_guard,
        |config_path, json| {
            write_team_config_json_atomic(&metadata.ac_root, team_guard, config_path, json)
        },
        save_settings_fn,
    )
    .await
}

#[cfg(test)]
pub(super) async fn persist_agent_delete_metadata_with_writers<T, S>(
    metadata: &PreparedAgentDeleteMetadata,
    settings: &SettingsState,
    team_guard: &TeamConfigMutationGuard,
    mut write_team_config_fn: T,
    save_settings_fn: S,
) -> Result<(), String>
where
    T: FnMut(&Path, &[u8]) -> Result<(), String>,
    S: Fn(&AppSettings) -> Result<AppSettings, String>,
{
    team_guard.require_ac_root(&metadata.ac_root)?;
    metadata.settings_changed.store(false, Ordering::SeqCst);

    for mutation in &metadata.team_mutations {
        if let Err(e) = write_team_config_fn(&mutation.config_path, &mutation.after_json) {
            let restore = restore_team_config_snapshots(metadata, team_guard);
            return Err(format!(
                "Failed to write team '{}' config during agent delete: {}{}",
                mutation.team_name,
                e,
                format_restore_suffix(restore)
            ));
        }
    }

    let settings_result = {
        let mut guard = settings.write().await;
        let removed_auto = guard.auto_self_clear_by_agent.remove(&metadata.agent_name);
        let removed_default = guard
            .coding_agent_profiles
            .default_profile_by_agent
            .remove(&metadata.agent_name);
        let changed = removed_auto.is_some() || removed_default.is_some();
        metadata.settings_changed.store(changed, Ordering::SeqCst);

        if changed {
            let mut candidate = guard.clone();
            if let Err(e) = crate::config::settings::validate_and_repair_settings(&mut candidate) {
                restore_removed_settings(
                    &mut guard,
                    &metadata.agent_name,
                    removed_auto,
                    removed_default,
                );
                Err(format!(
                    "Failed to validate settings after agent delete: {}",
                    e
                ))
            } else {
                match save_settings_fn(&candidate) {
                    Ok(written) => {
                        *guard = written;
                        Ok(())
                    }
                    Err(e) => {
                        restore_removed_settings(
                            &mut guard,
                            &metadata.agent_name,
                            removed_auto,
                            removed_default,
                        );
                        Err(format!("Failed to save settings after agent delete: {}", e))
                    }
                }
            }
        } else {
            Ok(())
        }
    };

    if let Err(e) = settings_result {
        let restore = restore_team_config_snapshots(metadata, team_guard);
        return Err(format!("{}{}", e, format_restore_suffix(restore)));
    }

    Ok(())
}

pub(super) fn restore_removed_settings(
    settings: &mut AppSettings,
    agent_name: &str,
    removed_auto: Option<bool>,
    removed_default: Option<String>,
) {
    if let Some(value) = removed_auto {
        settings
            .auto_self_clear_by_agent
            .insert(agent_name.to_string(), value);
    }
    if let Some(value) = removed_default {
        settings
            .coding_agent_profiles
            .default_profile_by_agent
            .insert(agent_name.to_string(), value);
    }
}

#[cfg(test)]
pub(super) async fn restore_agent_delete_metadata_snapshots(
    metadata: &PreparedAgentDeleteMetadata,
    _settings: &SettingsState,
    team_guard: &TeamConfigMutationGuard,
) -> Result<(), String> {
    restore_team_config_snapshots(metadata, team_guard)
}

#[cfg(test)]
pub(super) fn restore_team_config_snapshots(
    metadata: &PreparedAgentDeleteMetadata,
    team_guard: &TeamConfigMutationGuard,
) -> Result<(), String> {
    team_guard.require_ac_root(&metadata.ac_root)?;
    let mut errors = Vec::new();
    for mutation in &metadata.team_mutations {
        if let Err(e) = write_team_config_json_atomic(
            &metadata.ac_root,
            team_guard,
            &mutation.config_path,
            &mutation.before_json,
        ) {
            errors.push(format!("{}: {}", mutation.config_path.display(), e));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Failed to restore team config snapshot(s): {}",
            errors.join("; ")
        ))
    }
}

pub(super) fn format_restore_suffix(restore: Result<(), String>) -> String {
    match restore {
        Ok(()) => String::new(),
        Err(e) => format!("; snapshot restore failed: {}", e),
    }
}

pub(super) fn write_team_config_json_atomic(
    ac_root: &Path,
    team_guard: &TeamConfigMutationGuard,
    config_path: &Path,
    json: &[u8],
) -> Result<(), String> {
    teams::write_team_config_json_atomic(ac_root, team_guard, config_path, json)
}

#[cfg(test)]
pub(super) fn format_agent_delete_metadata_failure(
    persist_error: String,
    restore_result: Result<(), String>,
    dir_rollback_result: Result<(), String>,
) -> String {
    match (restore_result, dir_rollback_result) {
        (Ok(()), Ok(())) => persist_error,
        (Err(restore), Ok(())) => format!(
            "{}. Metadata snapshot restore also failed: {}",
            persist_error, restore
        ),
        (Ok(()), Err(rollback)) => format!(
            "{}. Directory rollback also failed: {}",
            persist_error, rollback
        ),
        (Err(restore), Err(rollback)) => format!(
            "{}. Metadata snapshot restore also failed: {}. Directory rollback also failed: {}",
            persist_error, restore, rollback
        ),
    }
}

#[cfg(test)]
pub(super) fn format_agent_delete_post_stage_live_failure(
    live_error: String,
    dir_rollback_result: Result<(), String>,
) -> String {
    match dir_rollback_result {
        Ok(()) => live_error,
        Err(rollback) => format!(
            "Agent delete post-stage live-session check failed and rollback also failed. \
             Live-session error: {}. Rollback error: {}",
            live_error, rollback
        ),
    }
}

#[derive(Debug, Clone)]
pub(super) struct AgentDeleteTarget {
    pub(super) original_path: PathBuf,
    pub(super) original_key: String,
    pub(super) label: String,
}

#[derive(Debug, Clone)]
pub(super) struct PreparedAgentDeleteMetadata {
    pub(super) ac_root: PathBuf,
    pub(super) team_mutations: Vec<AgentTeamMutation>,
    pub(super) agent_name: String,
    pub(super) settings_changed: Arc<AtomicBool>,
}

#[derive(Debug, Clone)]
pub(super) struct AgentDeletePlan {
    pub(super) ac_root: PathBuf,
    pub(super) agent_name: String,
    pub(super) agent_ref: String,
    pub(super) origin_dir: PathBuf,
    pub(super) target_keys: Vec<String>,
    pub(super) targets: Vec<AgentDeleteTarget>,
    pub(super) team_mutations: Vec<AgentTeamMutation>,
}
