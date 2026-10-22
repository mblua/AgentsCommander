use super::agent_delete_plan::{
    collect_agent_delete_plan_guarded, format_restore_suffix, preflight_agent_delete,
    prepare_agent_delete_metadata, restore_removed_settings, write_team_config_json_atomic,
    AgentDeletePlan, AgentDeleteTarget, PreparedAgentDeleteMetadata,
};
use super::delete_guard::{is_rename_blocked_by_handle, path_key_for_delete};
use super::matrix_layout::{
    create_agent_matrix_on_disk, selected_ac_root, CreateAgentMatrixDiskArgs,
};
use super::team_config::{acquire_team_config_mutation_guard_async, TeamConfigMutationGuard};
use super::team_update::emit_coordinator_refresh;
use super::workgroup::{
    acquire_lifecycle_project_gate, prune_agent_component_scope, prune_replica_config_scope,
};
use super::CreatedEntityResult;
use crate::config::seed_manifest::{ManifestActivationToken, ProjectSeedManifestGuard};
use crate::config::settings::{AppSettings, SettingsState};
use crate::session::manager::SessionManager;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

/// Create an agent matrix directory inside {project_path}/.ac/_agent_{name}/
///
/// `role_template_id` is the picker selection from `list_role_templates`; when
/// `Some`, the resolved template's body is inserted as a `## Role Profile`
/// section and (if the template ships one) its `skills/` are copied into the
/// new matrix. `None` (or a missing arg from older callers) preserves the
/// pre-#271 behavior exactly.
#[tauri::command]
pub async fn create_agent_matrix(
    settings: State<'_, SettingsState>,
    project_path: String,
    name: String,
    description: String,
    role_template_id: Option<String>,
) -> Result<CreatedEntityResult, String> {
    let settings_snapshot = settings.read().await.clone();
    let config_dir = crate::config::config_dir();

    let created = create_agent_matrix_on_disk(CreateAgentMatrixDiskArgs {
        project_path: &project_path,
        name: &name,
        description: &description,
        role_template_id: role_template_id.as_deref(),
        settings: &settings_snapshot,
        config_dir: config_dir.as_deref(),
    })?;

    let result_path =
        crate::path_utils::path_to_string_without_windows_verbatim_prefix(&created.agent_dir);
    log::debug!(
        "[entity_creation] Created agent matrix safe name: {}",
        created.safe_name
    );
    log::info!("[entity_creation] Created agent matrix: {}", result_path);
    Ok(CreatedEntityResult { path: result_path })
}

/// Delete an agent matrix identity from a project.
#[tauri::command]
pub async fn delete_agent_matrix(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    settings: State<'_, SettingsState>,
    project_path: String,
    agent_path: String,
) -> Result<(), String> {
    // #1065 Stage F: activated with the sole production token; no lock-order test barrier.
    delete_agent_matrix_inner(
        &app,
        session_mgr.inner(),
        settings.inner(),
        &project_path,
        &agent_path,
        Some(ManifestActivationToken::production()),
        |_| {},
    )
    .await
}

/// GUI Agent Matrix deletion with the #1063 global lock order (plan sections
/// 5.4/6.3/11.32/11.43).
///
/// Async preflight collects the delete plan under a short preliminary team guard
/// (copying exact raw config bytes and intent), then drops that guard; it runs the
/// public live-session preflight and prepares owned settings/team metadata. The
/// blocking transaction then acquires project → (deletion-only pending-inclusive
/// manager snapshot, both reads dropped) → team → settings, in that order, with no
/// synchronous guard across `.await`. It stages targets, re-reads each planned team
/// config under the reacquired guard requiring exact prepared bytes, journals only
/// successful writes for exact-once reverse restore, removes only the two live agent
/// keys from settings, and on the successful metadata commit point prunes the agent
/// component scope (dormant) before dropping both guards and removing the staged
/// targets. A pre-commit failure rolls staged targets back and prunes only the
/// still-staged (logically absent) targets. Blocker diagnostics run after every guard
/// is released.
pub(crate) async fn delete_agent_matrix_inner<H>(
    app: &AppHandle,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    settings: &SettingsState,
    project_path: &str,
    agent_path: &str,
    activation: Option<ManifestActivationToken>,
    after_project_before_team: H,
) -> Result<(), String>
where
    H: FnOnce(&Path) + Send + 'static,
{
    let base = selected_ac_root(Path::new(project_path))?;

    // Preflight under a short preliminary team guard, dropped before any wait.
    let plan = {
        let prelim_guard = acquire_team_config_mutation_guard_async(&base).await?;
        let plan = collect_agent_delete_plan_guarded(&base, Path::new(agent_path), &prelim_guard)?;
        drop(prelim_guard);
        plan
    };
    // Public live-session preflight + target validation (guards released).
    preflight_agent_delete(&plan, session_mgr).await?;
    // Owned settings/team metadata snapshot; the async settings read guard is dropped.
    let metadata = prepare_agent_delete_metadata(&plan, settings).await?;

    // Blocking transaction: project -> manager snapshot -> team -> settings.
    let project_root = Path::new(project_path).to_path_buf();
    let base_for_block = base.clone();
    let plan_for_block = plan.clone();
    let metadata_for_block = metadata.clone();
    let settings_for_block = settings.clone();
    let session_mgr_for_block = session_mgr.clone();
    let agent_name = plan.agent_name.clone();
    let outcome = crate::config::seed_manifest::run_blocking_owned(move || {
        run_agent_delete_transaction(
            &base_for_block,
            &project_root,
            &plan_for_block,
            &metadata_for_block,
            &settings_for_block,
            &session_mgr_for_block,
            &agent_name,
            activation.as_ref(),
            after_project_before_team,
            crate::config::settings::save_settings,
        )
    })
    .await
    .map_err(|e| format!("Agent delete blocking preparation failed: {e}"))?;

    match outcome {
        AgentDeleteTransactionOutcome::Committed {
            settings_changed,
            cleanup,
        } => {
            // The agent was removed; a hidden cleanup-dir error still surfaces to keep
            // the existing success/error contract.
            cleanup?;
            log::info!(
                "[entity_creation] Deleted agent matrix identity '{}' at '{}' (targets={}, \
                 team_configs={})",
                plan.agent_ref,
                plan.origin_dir.display(),
                plan.targets.len(),
                metadata.team_mutations.len()
            );
            if settings_changed {
                let _ = app.emit("coding_agent_profiles_updated", serde_json::json!({}));
            }
            emit_coordinator_refresh(app, session_mgr).await;
            Ok(())
        }
        AgentDeleteTransactionOutcome::Blocked { target, raw_error } => {
            // Blocker diagnostics run after every synchronous guard is released.
            log::info!(
                "[entity_creation] delete_agent_matrix: file-in-use detected for '{}' on rename \
                 probe",
                target.label
            );
            let report = crate::commands::wg_delete_diagnostic::diagnose_delete_root_blockers(
                &target.original_path,
                &target.label,
                &raw_error,
                session_mgr,
            )
            .await;
            let json = serde_json::to_string(&report).map_err(|se| {
                format!(
                    "Failed to serialize blocker report: {}; original error: {}",
                    se, raw_error
                )
            })?;
            Err(format!("BLOCKERS:{}", json))
        }
        AgentDeleteTransactionOutcome::Aborted { error } => Err(error),
    }
}

/// Outcome of the Agent Matrix delete blocking transaction (#1063). `Send + 'static`
/// so it can cross the `run_blocking_owned` boundary; the async caller formats it
/// after every synchronous guard has been released.
pub(super) enum AgentDeleteTransactionOutcome {
    /// Metadata persistence reached its irreversible commit point; the manifest
    /// prune already ran under the gate. `cleanup` is the hidden staged-target
    /// removal result (an error keeps the existing success/error contract).
    Committed {
        settings_changed: bool,
        cleanup: Result<(), String>,
    },
    /// A rename probe was blocked by an open handle; the async caller runs the
    /// blocker diagnostic on the still-intact tree after the gate is released.
    Blocked {
        target: AgentDeleteTarget,
        raw_error: String,
    },
    /// Any pre-commit failure (gate/stage/live-recheck/team/metadata). Staged
    /// targets were rolled back and only still-staged scopes were pruned already.
    Aborted { error: String },
}

/// The Agent Matrix delete blocking transaction (#1063). Runs the project →
/// pending-inclusive manager snapshot → team → settings sequence to completion in
/// blocking work; production drives it inside `run_blocking_owned`, and tests call
/// it directly with an injected `save_settings_fn` and lock-order barrier. The
/// `after_project_before_team` barrier fires while the project gate is held and
/// before the team guard is taken.
#[allow(clippy::too_many_arguments)]
pub(super) fn run_agent_delete_transaction(
    base: &Path,
    project_root: &Path,
    plan: &AgentDeletePlan,
    metadata: &PreparedAgentDeleteMetadata,
    settings: &SettingsState,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    agent_name: &str,
    activation: Option<&ManifestActivationToken>,
    after_project_before_team: impl FnOnce(&Path),
    save_settings_fn: impl Fn(&AppSettings) -> Result<AppSettings, String>,
) -> AgentDeleteTransactionOutcome {
    let mut project_gate = match acquire_lifecycle_project_gate(project_root) {
        Ok(gate) => gate,
        Err(error) => return AgentDeleteTransactionOutcome::Aborted { error },
    };
    after_project_before_team(base);

    // Stage targets; `stage_agent_delete_targets_with_rename` rolls back its own
    // staged set on failure, so no targets remain staged on Err.
    let staged = match stage_agent_delete_targets_with_rename(
        plan,
        |from: &Path, to: &Path| std::fs::rename(from, to),
        session_mgr,
    ) {
        Ok(staged) => staged,
        Err(AgentDeleteStageError::Blocked { target, raw_error }) => {
            drop(project_gate);
            return AgentDeleteTransactionOutcome::Blocked { target, raw_error };
        }
        Err(AgentDeleteStageError::Other(error))
        | Err(AgentDeleteStageError::RollbackFailed(error)) => {
            drop(project_gate);
            return AgentDeleteTransactionOutcome::Aborted { error };
        }
    };

    // Deletion-only pending-inclusive snapshot: outer manager read then one inner
    // state read, both released before the team guard.
    let live_workdirs = {
        let manager = session_mgr.blocking_read();
        manager.live_working_directories_for_deletion_blocking()
    };
    if live_workdirs
        .iter()
        .any(|workdir| workdir_blocks_delete(workdir, &plan.target_keys))
    {
        let still = rollback_staged_targets_reporting(&staged);
        prune_still_staged_targets(project_gate.as_mut(), activation, &still);
        drop(project_gate);
        return AgentDeleteTransactionOutcome::Aborted {
            error: format_post_stage_live_error(&still),
        };
    }

    // Reacquire the #1056 team guard after the project gate and manager reads.
    let team_guard = match TeamConfigMutationGuard::acquire(base) {
        Ok(guard) => guard,
        Err(error) => {
            let still = rollback_staged_targets_reporting(&staged);
            prune_still_staged_targets(project_gate.as_mut(), activation, &still);
            drop(project_gate);
            return AgentDeleteTransactionOutcome::Aborted {
                error: format!("{}{}", error, still_staged_suffix(&still)),
            };
        }
    };

    match persist_agent_delete_metadata_blocking(metadata, settings, &team_guard, save_settings_fn)
    {
        Ok(()) => {
            // Irreversible commit point reached. Prune the agent component scope
            // under the gate, then release both guards and delete.
            prune_agent_component_scope(project_gate.as_mut(), activation, agent_name);
            drop(team_guard);
            drop(project_gate);
            let cleanup = remove_staged_agent_delete_targets(&staged);
            AgentDeleteTransactionOutcome::Committed {
                settings_changed: metadata.settings_changed.load(Ordering::SeqCst),
                cleanup,
            }
        }
        Err(persist_error) => {
            // Metadata restore already ran inside the persist helper. Roll back
            // staged targets and prune only the still-staged ones.
            drop(team_guard);
            let still = rollback_staged_targets_reporting(&staged);
            prune_still_staged_targets(project_gate.as_mut(), activation, &still);
            drop(project_gate);
            AgentDeleteTransactionOutcome::Aborted {
                error: format!("{}{}", persist_error, still_staged_suffix(&still)),
            }
        }
    }
}

/// Whether a session working directory sits at or under any delete target key.
pub(super) fn workdir_blocks_delete(workdir: &str, target_keys: &[String]) -> bool {
    let working_key = path_key_for_delete(Path::new(workdir));
    target_keys.iter().any(|root_key| {
        working_key == *root_key || working_key.starts_with(&(root_key.clone() + "/"))
    })
}

/// Generic retryable post-stage delete error (#1063). It never reveals pending
/// session ids, names, statuses, or working directories.
pub(super) fn format_post_stage_live_error(still_staged: &[StagedAgentDeleteTarget]) -> String {
    format!(
        "Cannot delete agent while a session is active under a delete target; retry after it \
         exits.{}",
        still_staged_suffix(still_staged)
    )
}

/// Suffix describing hidden cleanup dirs that a failed rollback left staged away.
pub(super) fn still_staged_suffix(still_staged: &[StagedAgentDeleteTarget]) -> String {
    if still_staged.is_empty() {
        return String::new();
    }
    let labels = still_staged
        .iter()
        .map(|target| format!("{} [{}]", target.label, target.original_key))
        .collect::<Vec<_>>()
        .join(", ");
    format!(" Hidden cleanup dir(s) remain: {}", labels)
}

/// Roll back staged agent-delete targets in reverse order and return the targets
/// that could NOT be restored (their `.deleting-*` rename-back failed, so the
/// logical path is absent). The caller prunes only these still-staged scopes; a
/// fully restored rollback returns an empty vec and prunes nothing (plan 5.4).
pub(super) fn rollback_staged_targets_reporting(
    staged: &[StagedAgentDeleteTarget],
) -> Vec<StagedAgentDeleteTarget> {
    let mut still_staged = Vec::new();
    for target in staged.iter().rev() {
        if let Err(e) = std::fs::rename(&target.staged_path, &target.original_path) {
            log::warn!(
                "[entity_creation] agent-delete rollback left '{}' [{}] staged: {}",
                target.label,
                target.original_key,
                e
            );
            still_staged.push(target.clone());
        }
    }
    still_staged
}

/// Extract `(workgroup, agent)` for a replica delete target so its config scope
/// can be pruned. Returns `None` for the origin `_agent_*` target, which has no
/// v1 manifest row.
pub(super) fn replica_prefix_for_target(original_path: &Path) -> Option<(String, String)> {
    let agent = original_path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("__agent_"))?;
    let workgroup = original_path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())?;
    Some((workgroup.to_string(), agent.to_string()))
}

/// Prune only the still-staged (logically absent) replica scopes after a pre-commit
/// rollback (#1063). Dormant in production; the origin `_agent_*` target is skipped.
pub(super) fn prune_still_staged_targets(
    mut gate: Option<&mut ProjectSeedManifestGuard>,
    activation: Option<&ManifestActivationToken>,
    still_staged: &[StagedAgentDeleteTarget],
) {
    for target in still_staged {
        if let Some((workgroup, agent)) = replica_prefix_for_target(&target.original_path) {
            prune_replica_config_scope(gate.as_deref_mut(), activation, &workgroup, &agent);
        }
    }
}

/// Read a team `config.json`'s exact current bytes for the #1063 pre-write precheck.
pub(super) fn read_team_config_bytes(config_path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(config_path).map_err(|e| format!("{}: {}", config_path.display(), e))
}

/// Blocking Agent Matrix metadata commit under the reacquired project→team guards
/// (#1063, plan sections 5.4/11.39). It runs the exact-byte precheck, writes each
/// planned team config while journaling successes, removes only the two live agent
/// keys under `SettingsState::blocking_write`, and on any failure restores the
/// journaled entries once in reverse order (byte-checked) before returning.
pub(super) fn persist_agent_delete_metadata_blocking(
    metadata: &PreparedAgentDeleteMetadata,
    settings: &SettingsState,
    team_guard: &TeamConfigMutationGuard,
    save_settings_fn: impl Fn(&AppSettings) -> Result<AppSettings, String>,
) -> Result<(), String> {
    team_guard.require_ac_root(&metadata.ac_root)?;
    metadata.settings_changed.store(false, Ordering::SeqCst);

    // Exact-byte precheck under the reacquired #1056 guard: require the exact
    // prepared bytes before any write. The byte check is additional to the guard,
    // never a substitute for it.
    for mutation in &metadata.team_mutations {
        let current = read_team_config_bytes(&mutation.config_path)?;
        if current != mutation.before_json {
            return Err(format!(
                "Team '{}' config changed under the lock before agent delete; aborting without \
                 writes",
                mutation.team_name
            ));
        }
    }

    // Write each planned config, journaling only successful writes for reverse restore.
    let mut journal: Vec<usize> = Vec::new();
    for (index, mutation) in metadata.team_mutations.iter().enumerate() {
        if let Err(e) = write_team_config_json_atomic(
            &metadata.ac_root,
            team_guard,
            &mutation.config_path,
            &mutation.after_json,
        ) {
            let restore = restore_journaled_team_configs(metadata, team_guard, &journal);
            return Err(format!(
                "Failed to write team '{}' config during agent delete: {}{}",
                mutation.team_name,
                e,
                format_restore_suffix(restore)
            ));
        }
        journal.push(index);
    }

    // Settings commit: remove only the two live agent keys under blocking_write.
    let settings_result = {
        let mut guard = settings.blocking_write();
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
        let restore = restore_journaled_team_configs(metadata, team_guard, &journal);
        return Err(format!("{}{}", e, format_restore_suffix(restore)));
    }
    Ok(())
}

/// Restore journaled team configs once, in reverse order, only when the current
/// bytes still equal our written `after_json` (else a lock-unaware writer changed
/// it after our write; leave the newer content and report a typed conflict).
pub(super) fn restore_journaled_team_configs(
    metadata: &PreparedAgentDeleteMetadata,
    team_guard: &TeamConfigMutationGuard,
    journal: &[usize],
) -> Result<(), String> {
    team_guard.require_ac_root(&metadata.ac_root)?;
    let mut errors = Vec::new();
    for &index in journal.iter().rev() {
        let mutation = &metadata.team_mutations[index];
        match read_team_config_bytes(&mutation.config_path) {
            Ok(current) if current == mutation.after_json => {
                if let Err(e) = write_team_config_json_atomic(
                    &metadata.ac_root,
                    team_guard,
                    &mutation.config_path,
                    &mutation.before_json,
                ) {
                    errors.push(format!("{}: {}", mutation.config_path.display(), e));
                }
            }
            Ok(_) => errors.push(format!(
                "{}: restore conflict (content changed under the lock after write)",
                mutation.config_path.display()
            )),
            Err(e) => errors.push(e),
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

#[derive(Debug, Clone)]
pub(super) struct StagedAgentDeleteTarget {
    pub(super) original_path: PathBuf,
    pub(super) original_key: String,
    pub(super) staged_path: PathBuf,
    pub(super) label: String,
}

#[derive(Debug)]
pub(super) enum AgentDeleteStageError {
    Blocked {
        target: AgentDeleteTarget,
        raw_error: String,
    },
    Other(String),
    RollbackFailed(String),
}

#[cfg(test)]
pub(super) async fn stage_agent_delete_targets(
    plan: &AgentDeletePlan,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
) -> Result<Vec<StagedAgentDeleteTarget>, String> {
    match stage_agent_delete_targets_with_rename(
        plan,
        |from: &Path, to: &Path| std::fs::rename(from, to),
        session_mgr,
    ) {
        Ok(staged) => Ok(staged),
        Err(AgentDeleteStageError::Blocked { target, raw_error }) => {
            log::info!(
                "[entity_creation] delete_agent_matrix: file-in-use detected for '{}' on rename \
                 probe",
                target.label
            );
            let report = crate::commands::wg_delete_diagnostic::diagnose_delete_root_blockers(
                &target.original_path,
                &target.label,
                &raw_error,
                session_mgr,
            )
            .await;
            let json = serde_json::to_string(&report).map_err(|se| {
                format!(
                    "Failed to serialize blocker report: {}; original error: {}",
                    se, raw_error
                )
            })?;
            Err(format!("BLOCKERS:{}", json))
        }
        Err(AgentDeleteStageError::Other(e)) | Err(AgentDeleteStageError::RollbackFailed(e)) => {
            Err(e)
        }
    }
}

pub(super) fn stage_agent_delete_targets_with_rename<R>(
    plan: &AgentDeletePlan,
    mut rename: R,
    _session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
) -> Result<Vec<StagedAgentDeleteTarget>, AgentDeleteStageError>
where
    R: FnMut(&Path, &Path) -> std::io::Result<()>,
{
    let mut staged = Vec::new();
    for target in &plan.targets {
        let parent = target.original_path.parent().ok_or_else(|| {
            AgentDeleteStageError::Other(format!(
                "Agent delete target '{}' has no parent",
                target.original_path.display()
            ))
        })?;
        let original_name = target
            .original_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                AgentDeleteStageError::Other(format!(
                    "Agent delete target '{}' has no filename",
                    target.original_path.display()
                ))
            })?;
        let staged_path = parent.join(format!(
            ".deleting-{}-{}",
            original_name,
            uuid::Uuid::new_v4()
        ));

        match rename(&target.original_path, &staged_path) {
            Ok(()) => staged.push(StagedAgentDeleteTarget {
                original_path: target.original_path.clone(),
                original_key: target.original_key.clone(),
                staged_path,
                label: target.label.clone(),
            }),
            Err(e) => {
                let raw_error = e.to_string();
                let blocked = is_rename_blocked_by_handle(&e);
                let rollback = rollback_staged_agent_delete_targets_with_rename(&staged, rename);
                if let Err(rollback_err) = rollback {
                    return Err(AgentDeleteStageError::RollbackFailed(
                        format_stage_rollback_failed(&staged, &raw_error, rollback_err),
                    ));
                }
                if blocked {
                    return Err(AgentDeleteStageError::Blocked {
                        target: target.clone(),
                        raw_error,
                    });
                }
                return Err(AgentDeleteStageError::Other(format!(
                    "Failed to stage agent delete target '{}': {}",
                    target.original_path.display(),
                    raw_error
                )));
            }
        }
    }
    Ok(staged)
}

#[cfg(test)]
pub(super) fn rollback_staged_agent_delete_targets(
    staged: &[StagedAgentDeleteTarget],
) -> Result<(), String> {
    rollback_staged_agent_delete_targets_with_rename(staged, |from: &Path, to: &Path| {
        std::fs::rename(from, to)
    })
}

pub(super) fn rollback_staged_agent_delete_targets_with_rename<R>(
    staged: &[StagedAgentDeleteTarget],
    mut rename: R,
) -> Result<(), String>
where
    R: FnMut(&Path, &Path) -> std::io::Result<()>,
{
    let mut errors = Vec::new();
    for target in staged.iter().rev() {
        if let Err(e) = rename(&target.staged_path, &target.original_path) {
            errors.push(format!(
                "{} [{}] {} -> {}: {}",
                target.label,
                target.original_key,
                target.staged_path.display(),
                target.original_path.display(),
                e
            ));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Hidden cleanup dirs remain after rollback failure: {}",
            errors.join("; ")
        ))
    }
}

pub(super) fn remove_staged_agent_delete_targets(
    staged: &[StagedAgentDeleteTarget],
) -> Result<(), String> {
    let mut errors = Vec::new();
    for target in staged {
        if let Err(e) = std::fs::remove_dir_all(&target.staged_path) {
            errors.push(format!(
                "{} [{}] {}: {}",
                target.label,
                target.original_key,
                target.staged_path.display(),
                e
            ));
        } else {
            log::debug!(
                "[entity_creation] Removed staged agent delete target '{}' ({})",
                target.label,
                target.original_key
            );
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Agent was removed, but hidden cleanup dir(s) remain: {}",
            errors.join("; ")
        ))
    }
}

pub(super) fn format_stage_rollback_failed(
    staged: &[StagedAgentDeleteTarget],
    original_error: &str,
    rollback_error: String,
) -> String {
    let hidden_paths = staged
        .iter()
        .map(|target| target.staged_path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "Agent delete staging failed and rollback also failed. Hidden cleanup dirs remain: {}. \
         Original error: {}. Rollback error: {}",
        hidden_paths, original_error, rollback_error
    )
}
