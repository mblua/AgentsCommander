use super::creation_trace::{CreationTrace, Origin, Outcome, Reason, Stage, StageGuard};
use super::matrix_layout::{
    agent_matches, create_agent_replica_layout, repo_dir_name_from_url, sanitize_name,
    selected_ac_root, validate_existing_name,
};
use super::repo_clone::git_clone_async;
use super::room_number::{determine_next_wg_number, room_dir_name, room_number_pad_width};
use super::room_task::{build_task_content, initialize_new_room, validate_task_title};
use super::team_config::{
    acquire_team_config_mutation_guard_async, create_new_team_config_on_disk_guarded,
    normalize_team_config_for_project, read_team_config, resolve_agent_ref,
};
use super::{
    CloneError, ReplicaDiskCreateArgs, RepoAssignment, TeamConfigResult, WorkgroupCloneResult,
    WorkgroupDiskCreateArgs,
};
use crate::config::coding_agent_profiles::{
    apply_replica_selection_default, read_replica_selection_default,
};
use crate::config::entity_creation::directories::{
    self, clone_target_present, replica_role_present,
};
use crate::config::replica_identity::{
    normalize_wg_replica_context_entries, repair_wg_replica_config_value, ROLE_MD_FILENAME,
    WG_REPLICA_REQUIRED_CONTEXT,
};
use crate::config::seed_manifest::{
    ManifestActivationToken, ManifestLifecycleFilter, ProjectSeedManifestGuard, SeedManifestError,
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub(super) fn allocate_disk_room(
    base: &Path,
    safe_team: &str,
    task_title: &str,
    write_task: impl FnOnce(&Path, &str) -> std::io::Result<()>,
) -> Result<(PathBuf, u32), String> {
    let wg_number = determine_next_wg_number(base);
    let wg_name = room_dir_name(wg_number, safe_team, room_number_pad_width());
    let wg_dir = base.join(&wg_name);
    if directories::room_directory_present(&wg_dir) {
        return Err(format!("Room directory already exists: {}", wg_name));
    }
    directories::create_room_directory(&wg_dir)
        .map_err(|e| format!("Failed to create room directory: {}", e))?;
    directories::create_room_messaging_directory(
        &wg_dir.join(crate::phone::messaging::MESSAGING_DIR_NAME),
    )
    .map_err(|e| format!("Failed to create messaging directory: {}", e))?;
    // #1795: `room-shared/` is a sibling of `messaging/` and `TASK.md`, and it is
    // created with the same hard `?` they use. A half-created room must not be
    // reported as created.
    crate::config::shared_locations::create_room_shared_dir(&wg_dir)
        .map_err(|e| format!("Failed to create room shared directory: {}", e))?;
    let task_content = build_task_content(task_title);
    write_task(&wg_dir, &task_content).map_err(|e| format!("Failed to write TASK.md: {}", e))?;

    Ok::<_, String>((wg_dir, wg_number))
}

pub(crate) fn list_workgroup_dirs(ac_root: &Path) -> Vec<PathBuf> {
    crate::config::entity_creation::directories::workgroup_candidates(ac_root)
}

pub(crate) fn parse_team_from_workgroup_name(workgroup_name: &str) -> Result<String, String> {
    crate::config::entity_creation::directories::parse_team_from_workgroup_name(workgroup_name)
}

pub(crate) fn agent_ref_bare_name(agent_ref: &str) -> String {
    let dir_name = Path::new(agent_ref)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(agent_ref);
    dir_name
        .strip_prefix("_agent_")
        .unwrap_or(dir_name)
        .to_string()
}

// Compatibility entry point for callers without an existing operation.
#[allow(dead_code)]
pub(crate) async fn create_workgroup_on_disk(
    args: WorkgroupDiskCreateArgs,
) -> Result<WorkgroupCloneResult, String> {
    let trace = CreationTrace::new(Origin::OnDisk);
    let mut operation = trace.operation();
    let result = create_workgroup_on_disk_traced(args, &mut operation).await;
    if let Ok(created) = &result {
        operation.clone_errors(created.clone_errors.len());
        operation.finish(Outcome::clones(created.clone_errors.len()), None);
    }
    operation.result(result)
}

pub(crate) async fn create_workgroup_on_disk_traced(
    args: WorkgroupDiskCreateArgs,
    operation: &mut StageGuard<'_>,
) -> Result<WorkgroupCloneResult, String> {
    let mut physical = operation.child(Stage::Physical);
    let result = async {
        let mut prepare = physical.child(Stage::Prepare);
        let prepared = async {
            let safe_team = sanitize_name(&args.team_name)?;
            let task_title = args.task_title.trim().to_string();
            validate_task_title(&task_title)?;
            let base = selected_ac_root(&args.project_path)?;
            prepare.project(&base);
            physical.project(&base);
            operation.project(&base);

            if let Err(e) = crate::commands::ac_discovery::ensure_ac_root_gitignore(&base) {
                log::warn!(
                    "[create_workgroup] Failed to ensure Project AC Root .gitignore: {}",
                    e
                );
            }

            let team_config = if !args.agents.is_empty()
                || args.coordinator.is_some()
                || !args.repos.is_empty()
            {
                let coordinator = args.coordinator.clone().ok_or_else(|| {
                    "Orchestrator is required when provisioning team config".to_string()
                })?;
                if !args.agents.contains(&coordinator) {
                    return Err("Orchestrator must be one of the selected agents".to_string());
                }
                let config = TeamConfigResult {
                    agents: args.agents.clone(),
                    coordinator,
                    repos: args.repos.clone(),
                    context_alert_percentages: Vec::new(),
                };
                let config = normalize_team_config_for_project(&base, &config)?;
                let guard = prepare
                    .child(Stage::TeamGuard)
                    .run_async(acquire_team_config_mutation_guard_async(&base))
                    .await?;
                create_new_team_config_on_disk_guarded(&base, &safe_team, &config, &guard)?;
                drop(guard);
                config
            } else {
                read_team_config(&base, &safe_team)?
            };

            Ok::<_, String>((safe_team, task_title, base, team_config))
        }
        .await;
        let (safe_team, task_title, base, team_config) = prepare.result(prepared)?;

        let allocate = physical.child(Stage::Allocate);
        let (wg_dir, wg_number) = allocate
            .run(|| allocate_disk_room(&base, &safe_team, &task_title, initialize_new_room))?;
        physical.room(wg_number);
        operation.room(wg_number);

        let mut replicas = physical.child(Stage::Replicas);
        replicas.count(team_config.agents.len());
        let replica_result = (|| {
            for (index, agent_path) in team_config.agents.iter().enumerate() {
                let replica = replicas.indexed_child(Stage::Replica, index + 1);
                replica.run(|| {
                    create_or_update_replica_on_disk(ReplicaDiskCreateArgs {
                        ac_root: base.clone(),
                        wg_dir: wg_dir.clone(),
                        agent_path: agent_path.clone(),
                        team_repos: team_config.repos.clone(),
                    })
                    .map(|_| ())
                })?;
            }
            Ok::<_, String>(())
        })();
        replicas.result(replica_result)?;

        let clone_errors =
            clone_missing_repos_for_workgroup_traced(&wg_dir, &team_config.repos, Some(&physical))
                .await;
        let result_path =
            crate::path_utils::path_to_string_without_windows_verbatim_prefix(&wg_dir);
        log::info!(
            "[entity_creation] Created workgroup: {} ({} clone errors)",
            result_path,
            clone_errors.len()
        );
        Ok(WorkgroupCloneResult {
            path: result_path,
            clone_errors,
        })
    }
    .await;
    if let Ok(created) = &result {
        physical.clone_errors(created.clone_errors.len());
        physical.finish(Outcome::clones(created.clone_errors.len()), None);
    }
    physical.result(result)
}

pub(crate) fn create_or_update_replica_on_disk(
    args: ReplicaDiskCreateArgs,
) -> Result<PathBuf, String> {
    let agent_ref = resolve_agent_ref(&args.ac_root, &args.agent_path)?;
    let agent_name = agent_ref_bare_name(&agent_ref);
    validate_existing_name(&agent_name, "Agent")?;
    let replica_dir = args.wg_dir.join(format!("__agent_{}", agent_name));

    create_agent_replica_layout(&replica_dir).map_err(|(sub, e)| match sub {
        "replica_dir" => format!("Failed to create replica dir for {}: {}", agent_name, e),
        _ => format!("Failed to create {} for {}: {}", sub, agent_name, e),
    })?;

    let assigned_repos: Vec<String> = args
        .team_repos
        .iter()
        .filter(|r| r.agents.iter().any(|a| agent_matches(a, &agent_name)))
        .map(|r| {
            let dir_name = format!("repo-{}", repo_dir_name_from_url(&r.url));
            format!("../{}", dir_name)
        })
        .collect();

    initialize_replica_config_on_disk(&replica_dir, &assigned_repos)?;
    Ok(replica_dir)
}

/// #1939 - guarded, lossless initialization of one Room replica config.
///
/// Shared by `create_or_update_replica_on_disk` and the GUI workgroup loop so
/// both publish through the #1938 sidecar with one merge: repair identity and
/// context, refresh `repos`, and preserve tooling, custom context and unknown
/// keys. A missing config file inside the guard is the first-creation boundary
/// (`try_exists`, so an inspection error is an error, never absence); only then
/// is the Matrix creation default read and materialized. An existing config
/// never reads the default, so a later invalid default cannot block a valid
/// retry, and a concurrent winner's bytes are merged, not overwritten.
pub(super) fn initialize_replica_config_on_disk(
    replica_dir: &Path,
    assigned_repos: &[String],
) -> Result<(), String> {
    let config_path = replica_dir.join("config.json");
    crate::config::agent_config::update_agent_config(&config_path, |obj, _state| {
        let had_config = config_path.try_exists().map_err(|error| {
            format!(
                "Failed to inspect replica config {}: {}",
                config_path.display(),
                error
            )
        })?;
        let mut config = serde_json::Value::Object(std::mem::take(obj));
        if !had_config {
            // First creation: establish the canonical identity the repair below
            // then validates, exactly like any other identity.
            let expected =
                crate::config::replica_identity::expected_wg_replica_identity(replica_dir)?;
            config["identity"] = serde_json::json!(expected.identity);
        }
        let identity =
            repair_wg_replica_config_value(replica_dir, &mut config, WG_REPLICA_REQUIRED_CONTEXT)?;
        config["repos"] = serde_json::json!(assigned_repos);
        let existing_context: Vec<String> = config
            .get("context")
            .and_then(|value| value.as_array())
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| entry.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        config["context"] = serde_json::json!(normalize_wg_replica_context_entries(
            &existing_context,
            &["$AGENTSCOMMANDER_CONTEXT"],
            &identity.identity,
            replica_role_present(&identity.matrix_dir.join(ROLE_MD_FILENAME)),
        ));
        if !had_config {
            if let Some(default) = read_replica_selection_default(&identity.matrix_dir)? {
                apply_replica_selection_default(&mut config, &default)?;
            }
        }
        let final_object = config.as_object_mut().ok_or_else(|| {
            format!(
                "Replica config {} must be a JSON object",
                config_path.display()
            )
        })?;
        *obj = std::mem::take(final_object);
        Ok(())
    })?;
    Ok(())
}

/// #1939 - the GUI workgroup replica loop. Kept as one function so the GUI path
/// and the on-disk path provably share the same guarded initializer instead of
/// a second destructive writer.
// Compatibility entry point for callers without creation diagnostics.
#[allow(dead_code)]
pub(super) fn initialize_workgroup_replicas_on_disk(
    ac_root: &Path,
    wg_dir: &Path,
    agents: &[String],
    repos: &[RepoAssignment],
) -> Result<(), String> {
    initialize_workgroup_replicas_on_disk_traced(ac_root, wg_dir, agents, repos, None)
}

pub(super) fn initialize_workgroup_replicas_on_disk_traced(
    ac_root: &Path,
    wg_dir: &Path,
    agents: &[String],
    repos: &[RepoAssignment],
    parent: Option<&StageGuard<'_>>,
) -> Result<(), String> {
    let mut replicas = parent.map(|p| p.child(Stage::Replicas));
    if let Some(stage) = &mut replicas {
        stage.count(agents.len());
    }
    let result = (|| {
        for (index, agent_path) in agents.iter().enumerate() {
            let replica = replicas
                .as_ref()
                .map(|p| p.indexed_child(Stage::Replica, index + 1));
            let result = (|| {
                let agent_ref = resolve_agent_ref(ac_root, agent_path)?;
                let agent_name = agent_ref_bare_name(&agent_ref);
                let replica_dir = wg_dir.join(format!("__agent_{}", agent_name));
                create_agent_replica_layout(&replica_dir).map_err(|(sub, error)| match sub {
                    "replica_dir" => {
                        format!("Failed to create replica dir for {}: {}", agent_name, error)
                    }
                    _ => format!("Failed to create {} for {}: {}", sub, agent_name, error),
                })?;
                let assigned_repos: Vec<String> = repos
                    .iter()
                    .filter(|repo| repo.agents.iter().any(|a| agent_matches(a, &agent_name)))
                    .map(|repo| {
                        let dir_name = format!("repo-{}", repo_dir_name_from_url(&repo.url));
                        format!("../{}", dir_name)
                    })
                    .collect();
                initialize_replica_config_on_disk(&replica_dir, &assigned_repos)
            })();
            if let Some(stage) = replica {
                stage.result(result)?;
            } else {
                result?;
            }
        }
        Ok(())
    })();
    if let Some(stage) = replicas {
        stage.result(result)
    } else {
        result
    }
}

pub(crate) use crate::config::entity_creation::directories::ReplicaRemovalOutcome;

pub(crate) fn remove_replica_dir(replica_dir: &Path) -> ReplicaRemovalOutcome {
    crate::config::entity_creation::directories::remove_replica_directory(replica_dir)
}

// ===========================================================================
// #1063 lifecycle-removal seam (#1065 Stage F: activated recorder).
//
// Lifecycle deletion acquires the project seed-manifest gate so it serializes
// against config-seed publication (plan sections 5.4/6.3): the lock ordering is
// production-active. The manifest prune requires a `ManifestActivationToken`;
// Stage F adds the sole production constructor (`ManifestActivationToken::production`)
// and each production deletion entry point now threads `Some(&token)`, so the
// recorder call is compiled and reached in a production build and the manifest is
// pruned after the logical removal commits. Emission still stays a no-op when the
// gate degraded before contention (`None` gate) or a caller intentionally passes
// no token (`None`), preserving the fail-soft deletion contract. Tests thread a
// token to exercise the real prune, staged-delete rollback, and single final
// snapshot on the same lifecycle path.
// ===========================================================================

/// Acquire the project seed-manifest gate for a still-reversible lifecycle
/// removal.
///
/// * `Ok(Some(guard))` holds the gate through logical removal, rollback, and the
///   (dormant) prune.
/// * `Ok(None)` is a pre-contention lock-capability/I-O degradation: preserve
///   today's untracked deletion behavior and warn (plan section 6.1 lifecycle row).
/// * `Err(msg)` is a retryable busy timeout after observed contention, or an
///   unsafe/reparse project path that must fail closed before mutation.
pub(crate) fn acquire_lifecycle_project_gate(
    project_root: &Path,
) -> Result<Option<ProjectSeedManifestGuard>, String> {
    match ProjectSeedManifestGuard::acquire(project_root) {
        Ok(guard) => Ok(Some(guard)),
        Err(SeedManifestError::UnsafePath { path, reason }) => Err(format!(
            "Refusing deletion: unsafe project seed-manifest path {}: {}",
            path.display(),
            reason
        )),
        Err(SeedManifestError::BusyTimeout { path, waited }) => Err(format!(
            "Project is busy; retry the deletion (seed-manifest lock at {} stayed busy for {:?})",
            path.display(),
            waited
        )),
        Err(SeedManifestError::LockIo {
            saw_contention: true,
            path,
            source,
        }) => Err(format!(
            "Project is busy; retry the deletion (seed-manifest lock I/O at {} after contention: \
             {})",
            path.display(),
            source
        )),
        Err(other) => {
            // Pre-contention capability or other lock-acquisition error: preserve
            // the existing deletion behavior as untracked degradation.
            log::warn!(
                "[entity_creation] #1063 lifecycle removal proceeding without seed-manifest gate \
                 (untracked): {}",
                other
            );
            Ok(None)
        }
    }
}

/// Lifecycle prune (#1063, #1065 Stage F activated). The recorder call is compiled
/// in production; it runs only when a gate is held and an activation token was
/// threaded, so a pre-contention gate degradation or an unactivated caller stays a
/// no-op.
///
/// The `gate` is `None` when the pre-contention degradation above skipped the
/// lock. The `make_filter` closure builds the exact committed-intent
/// `ManifestLifecycleFilter` lazily so no filter is constructed on the no-op path.
pub(super) fn prune_lifecycle_scope_if_activated(
    gate: Option<&mut ProjectSeedManifestGuard>,
    activation: Option<&ManifestActivationToken>,
    make_filter: impl FnOnce() -> Result<ManifestLifecycleFilter, SeedManifestError>,
) {
    if let (Some(guard), Some(token)) = (gate, activation) {
        match make_filter() {
            Ok(filter) => {
                let outcome = guard
                    .publication_permit()
                    .apply_lifecycle_filter(token, filter);
                log::debug!(
                    "[entity_creation] #1063 lifecycle prune outcome: {:?}",
                    outcome
                );
            }
            Err(error) => log::warn!(
                "[entity_creation] #1063 lifecycle filter rejected before prune: {}",
                error
            ),
        }
    }
}

/// Activation-gated prune of the config scope of a single replica `.ac/<workgroup>/__agent_<name>`
/// (#1063, plan section 5.4 "AC removes one replica"). Used by CLI team-member removal.
pub(crate) fn prune_replica_config_scope(
    gate: Option<&mut ProjectSeedManifestGuard>,
    activation: Option<&ManifestActivationToken>,
    workgroup: &str,
    agent_name: &str,
) {
    let relative = Path::new(".ac")
        .join(workgroup)
        .join(format!("__agent_{}", agent_name));
    prune_lifecycle_scope_if_activated(gate, activation, move || {
        let identity =
            crate::config::seed_manifest::ManifestPathIdentity::from_relative_path(&relative)?;
        ManifestLifecycleFilter::config_path_prefix(identity)
    });
}

/// Activation-gated prune of every replica config scope under one workgroup `.ac/<workgroup>`
/// (#1063, plan section 5.4 "AC removes a workgroup"). Used by GUI/CLI workgroup deletion.
pub(crate) fn prune_workgroup_config_scope(
    gate: Option<&mut ProjectSeedManifestGuard>,
    activation: Option<&ManifestActivationToken>,
    workgroup: &str,
) {
    let relative = Path::new(".ac").join(workgroup);
    prune_lifecycle_scope_if_activated(gate, activation, move || {
        let identity =
            crate::config::seed_manifest::ManifestPathIdentity::from_relative_path(&relative)?;
        ManifestLifecycleFilter::config_path_prefix(identity)
    });
}

/// Activation-gated prune of every replica config scope for a set of workgroups (#1063, plan
/// section 5.4 "AC deletes a team"). Used by GUI team deletion for the committed
/// intent: successfully removed workgroups plus valid matching team workgroup
/// identities that were already absent at the explicit delete event.
pub(crate) fn prune_team_workgroups_scope(
    gate: Option<&mut ProjectSeedManifestGuard>,
    activation: Option<&ManifestActivationToken>,
    workgroups: Vec<String>,
) {
    if workgroups.is_empty() {
        return;
    }
    prune_lifecycle_scope_if_activated(gate, activation, move || {
        ManifestLifecycleFilter::workgroup_components(workgroups)
    });
}

/// Activation-gated prune of every replica config scope for one exact agent component
/// `__agent_<name>` across all workgroups (#1063, plan section 5.4 "AC deletes an
/// Agent Matrix and cascaded replicas"). Used by Agent Matrix deletion.
pub(crate) fn prune_agent_component_scope(
    gate: Option<&mut ProjectSeedManifestGuard>,
    activation: Option<&ManifestActivationToken>,
    agent_name: &str,
) {
    let component = format!("__agent_{}", agent_name);
    prune_lifecycle_scope_if_activated(gate, activation, move || {
        ManifestLifecycleFilter::replica_component(component)
    });
}

pub(crate) async fn clone_missing_repos_for_workgroup(
    wg_dir: &Path,
    repos: &[RepoAssignment],
) -> Vec<CloneError> {
    clone_missing_repos_for_workgroup_traced(wg_dir, repos, None).await
}

async fn clone_missing_repos_for_workgroup_traced(
    wg_dir: &Path,
    repos: &[RepoAssignment],
    parent: Option<&StageGuard<'_>>,
) -> Vec<CloneError> {
    let mut repos_stage = parent.map(|p| p.child(Stage::Repos));
    if let Some(stage) = &mut repos_stage {
        stage.count(repos.len());
    }
    let mut clone_errors = Vec::new();
    let mut seen_urls = HashSet::new();
    for (index, repo) in repos.iter().enumerate() {
        if !seen_urls.insert(repo.url.clone()) {
            if let Some(stage) = &repos_stage {
                stage.skip_repo(index + 1, Reason::DuplicateUrl);
            }
            continue;
        }
        let dir_name = format!("repo-{}", repo_dir_name_from_url(&repo.url));
        let target = wg_dir.join(&dir_name);
        if clone_target_present(&target) {
            if let Some(stage) = &repos_stage {
                stage.skip_repo(index + 1, Reason::TargetExists);
            }
            continue;
        }
        let mut repo_stage = repos_stage
            .as_ref()
            .map(|p| p.indexed_child(Stage::Repo, index + 1));
        let result = git_clone_async(&repo.url, &target, repo_stage.as_ref()).await;
        if let Some(stage) = &mut repo_stage {
            if result.is_ok() {
                stage.finish(Outcome::Ok, None);
            } else {
                stage.finish(Outcome::Error, Some(Reason::Failed));
            }
        }
        match result {
            Ok(_) => log::info!(
                "[entity_creation] Cloned {} -> {}",
                repo.url,
                target.display()
            ),
            Err(e) => {
                log::error!("[entity_creation] Failed to clone {}: {}", repo.url, e);
                clone_errors.push(CloneError {
                    url: repo.url.clone(),
                    error: e,
                });
            }
        }
    }
    if let Some(stage) = &mut repos_stage {
        stage.clone_errors(clone_errors.len());
        stage.finish(Outcome::clones(clone_errors.len()), None);
    }
    clone_errors
}
