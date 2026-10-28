use super::matrix_layout::{
    agent_matches, repo_dir_name_from_url, selected_ac_root, validate_existing_name,
};
use super::team_config::{
    acquire_team_config_mutation_guard_async, normalize_context_alert_percentages,
    normalize_team_config_for_project, read_team_config, read_team_config_classified,
    resolve_context_alert_argument, write_existing_team_config_guarded, ContextAlertWriteMode,
};
use super::{RepoAssignment, SyncError, SyncResult, TeamConfigResult};
use crate::commands::ac_discovery::DiscoveryBranchWatcher;
use crate::config::replica_identity::{
    normalize_wg_replica_context_entries, repair_wg_replica_config_value, ROLE_MD_FILENAME,
    WG_REPLICA_REQUIRED_CONTEXT,
};
use crate::pty::git_watcher::{CoordinatorChangedPayload, GitWatcher};
use crate::session::manager::SessionManager;
use crate::session::session::SessionRepo;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

/// Update an existing team's config.json in {project_path}/.ac/_team_{name}/
// Tauri command: State<> injections push us over clippy's 7-arg threshold.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn update_team(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    git_watcher: State<'_, Arc<GitWatcher>>,
    discovery_watcher: State<'_, Arc<DiscoveryBranchWatcher>>,
    project_path: String,
    team_name: String,
    agents: Vec<String>,
    coordinator: String,
    repos: Vec<RepoAssignment>,
    context_alert_percentages: Option<Vec<u8>>,
) -> Result<(), String> {
    validate_existing_name(&team_name, "Team")?;
    let base = selected_ac_root(Path::new(&project_path))?;
    let supplied_context_alert_percentages = context_alert_percentages
        .map(|values| normalize_context_alert_percentages(&values))
        .transpose()?;
    let guard = acquire_team_config_mutation_guard_async(&base).await?;

    let team_dir = base.join(format!("_team_{}", team_name));
    if !team_dir.exists() {
        return Err(format!("Team '{}' not found", team_name));
    }

    let context_alert_percentages = match supplied_context_alert_percentages {
        Some(values) => {
            resolve_context_alert_argument(ContextAlertWriteMode::Update, Some(values), None)?
        }
        None => {
            let current =
                read_team_config_classified(&base, &team_name).map_err(|e| e.to_string())?;
            resolve_context_alert_argument(
                ContextAlertWriteMode::Update,
                None,
                Some(&current.context_alert_percentages),
            )?
        }
    };
    let config = normalize_team_config_for_project(
        &base,
        &TeamConfigResult {
            agents,
            coordinator,
            repos,
            context_alert_percentages,
        },
    )?;
    write_existing_team_config_guarded(&base, &team_name, &config, &guard)?;
    drop(guard);

    log::info!("[entity_creation] Updated team: {}", team_name);

    // Propagate repo changes to existing workgroups (async now — awaits SessionManager refresh).
    match sync_workgroup_repos_inner(
        &base,
        &team_name,
        &config.repos,
        session_mgr.inner(),
        git_watcher.inner(),
        discovery_watcher.inner(),
        &app,
    )
    .await
    {
        Ok(result) => {
            log::info!(
                "[entity_creation] Synced {} workgroups, {} replicas for team '{}' ({} errors)",
                result.workgroups_updated,
                result.replicas_updated,
                team_name,
                result.errors.len()
            );
        }
        Err(e) => {
            log::warn!("[entity_creation] Failed to sync workgroup repos: {}", e);
            // Non-fatal: team config was saved successfully
        }
    }

    // Refresh coordinator flags — a team edit can add/remove the coordinator or change its
    // target.
    emit_coordinator_refresh(&app, session_mgr.inner()).await;

    Ok(())
}

/// Canonicalize an absolute or relative repo path and derive (label, absolute_path).
/// Mirrors ac_discovery.rs's source_path production so `Vec<SessionRepo>` equality
/// between the two writers holds (order and path shape both matter).
pub(super) fn build_session_repo(replica_dir: &Path, rel: &str) -> Option<SessionRepo> {
    let resolved = replica_dir.join(rel);
    let abs = crate::config::projects::canonical_repo_source(&resolved).ok()?;
    let source_path = crate::path_utils::path_to_string_without_windows_verbatim_prefix(&abs);
    let dir = source_path
        .replace('\\', "/")
        .split('/')
        .next_back()
        .unwrap_or("")
        .to_string();
    let label = dir.strip_prefix("repo-").map(str::to_string).unwrap_or(dir);
    Some(SessionRepo {
        label,
        source_path,
        // #1028 - undetected, like `branch` above. This literal is emitted straight to
        // the UI by `sync_workgroup_repos_inner`, so a user-initiated sync shows a
        // violet badge until the next GitWatcher tick (<=5s, forced by the
        // `invalidate_session_cache` call beside that emit). Same bound `branch`
        // already has there.
        branch: None,
        dirty: None,
    })
}

/// Core sync logic — updates repos and context in all replica configs for a team's workgroups.
/// After successful per-replica writes, pushes the new `git_repos` to any matching live session
/// via `refresh_git_repos_for_sessions` + watcher cache invalidation + `session_git_repos` emit.
/// Async so it can await the RwLock on `SessionManager`.
/// #2816 (C3) - site 7 lifted out of `sync_workgroup_repos_inner`, body
/// verbatim, so its refusal of an absent `config.json` and its no-file
/// guarantee are provable by a unit test.
pub(super) fn apply_wg_replica_repos(
    replica_dir: &Path,
    assigned_repos: &[String],
) -> Result<crate::config::replica_identity::WgReplicaIdentity, String> {
    // Read existing config, preserving identity/tooling/other runtime fields
    let config_path = replica_dir.join("config.json");
    let mut repaired_identity = None;
    crate::config::agent_config::update_existing_agent_config(&config_path, |obj, _state| {
        let mut config = serde_json::Value::Object(std::mem::take(obj));
        let identity =
            repair_wg_replica_config_value(replica_dir, &mut config, WG_REPLICA_REQUIRED_CONTEXT)?;

        // Update repos
        config["repos"] = serde_json::json!(assigned_repos);

        // Context merge: prepend required tokens to maintain consistent ordering
        // with create_workgroup() (which writes [$AC_CONTEXT, $REPOS_INFO] first).
        // Preserve custom non-Role entries while replacing identity-derived Role.md
        // entries with the repaired same-workspace identity.
        let existing_context: Vec<String> = config
            .get("context")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        config["context"] = serde_json::json!(normalize_wg_replica_context_entries(
            &existing_context,
            &["$AGENTSCOMMANDER_CONTEXT"],
            &identity.identity,
            identity.matrix_dir.join(ROLE_MD_FILENAME).exists(),
        ));

        let final_obj = config.as_object_mut().ok_or_else(|| {
            format!(
                "Replica config {} must be a JSON object",
                config_path.display()
            )
        })?;
        *obj = std::mem::take(final_obj);
        repaired_identity = Some(identity);
        Ok(())
    })?;
    Ok(repaired_identity.expect("identity repaired before successful write"))
}

pub(super) async fn sync_workgroup_repos_inner(
    base: &Path,
    team_name: &str,
    repos: &[RepoAssignment],
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    git_watcher: &Arc<GitWatcher>,
    discovery_watcher: &Arc<DiscoveryBranchWatcher>,
    app: &AppHandle,
) -> Result<SyncResult, String> {
    let mut result = SyncResult {
        workgroups_updated: 0,
        replicas_updated: 0,
        errors: Vec::new(),
    };

    // `updates` is built ONLY from replicas whose config.json write succeeded
    // (Grinch #15 partial-failure filter). In-memory state must match on-disk.
    let mut updates: Vec<(String, Vec<SessionRepo>)> = Vec::new();
    // Replica paths touched successfully — used for `invalidate_replicas` so the next
    // discovery poll re-registers them with fresh data (§3.2.5 / Grinch #17).
    let mut touched_replica_paths: Vec<String> = Vec::new();

    // Find all workgroups for this team (same discovery as delete_team())
    let wg_suffix = format!("-{}", team_name);
    let mut wg_dirs: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(base) {
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            // #1614: the middle segment comes from the STRIPPED remainder, never
            // from an index. The literal 3 was "wg-".len(); "room-".len() is 5,
            // so an index form would mis-slice every Room name and, for a team
            // name long enough, panic on a reversed range.
            if let Some(rest) = crate::config::entity_prefix::strip_entity_prefix(&name_str) {
                if let Some(middle) = rest.strip_suffix(&wg_suffix) {
                    if middle.parse::<u32>().is_ok() {
                        wg_dirs.push(entry.path());
                    }
                }
            }
        }
    }

    for wg_dir in &wg_dirs {
        let mut wg_touched = false;
        let wg_name = wg_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        // List __agent_* directories in this workgroup
        let replica_dirs: Vec<PathBuf> = match std::fs::read_dir(wg_dir) {
            Ok(entries) => entries
                .flatten()
                .filter(|e| {
                    e.path().is_dir() && e.file_name().to_string_lossy().starts_with("__agent_")
                })
                .map(|e| e.path())
                .collect(),
            Err(e) => {
                log::warn!("Failed to read workgroup dir {}: {}", wg_dir.display(), e);
                continue;
            }
        };

        for replica_dir in &replica_dirs {
            let dir_name = replica_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");

            // __agent_dev-rust → dev-rust
            let replica_name = dir_name.strip_prefix("__agent_").unwrap_or(dir_name);

            // Compute assigned repos (relative strings, written to config.json).
            let assigned_repos: Vec<String> = repos
                .iter()
                .filter(|r| r.agents.iter().any(|a| agent_matches(a, replica_name)))
                .map(|r| {
                    let d = format!("repo-{}", repo_dir_name_from_url(&r.url));
                    format!("../{}", d)
                })
                .collect();

            let _identity = match apply_wg_replica_repos(replica_dir, &assigned_repos) {
                Ok(identity) => identity,
                Err(e) => {
                    result.errors.push(SyncError {
                        replica: dir_name.to_string(),
                        error: e,
                    });
                    continue;
                }
            };

            // Write succeeded — record for in-memory refresh. Canonicalize each repo
            // path so source_path matches DiscoveryBranchWatcher's shape. Order of
            // `assigned_repos` = team config `repos` order, preserved via the filter
            // above — do NOT sort or dedupe.
            let session_repos: Vec<SessionRepo> = assigned_repos
                .iter()
                .filter_map(|rel| build_session_repo(replica_dir, rel))
                .collect();
            let session_name = format!("{}/{}", wg_name, replica_name);
            updates.push((session_name, session_repos));
            touched_replica_paths.push(replica_dir.to_string_lossy().to_string());

            result.replicas_updated += 1;
            wg_touched = true;
        }

        if wg_touched {
            result.workgroups_updated += 1;
        }
    }

    if !result.errors.is_empty() {
        log::warn!(
            "[entity_creation] sync_workgroup_repos for '{}': {} replicas updated, {} errors",
            team_name,
            result.replicas_updated,
            result.errors.len()
        );
    }

    // Refresh live sessions' git_repos in-memory so the sidebar updates before the next
    // discovery poll. CAS-guarded via git_repos_gen bump (Grinch #14 race fix).
    if !updates.is_empty() {
        let changed = {
            let mgr = session_mgr.read().await;
            mgr.refresh_git_repos_for_sessions(&updates).await
        };

        // Force DiscoveryBranchWatcher to re-register these replicas with fresh data
        // on the next `discover_project` call (§3.2.5 / Grinch #17).
        discovery_watcher.invalidate_replicas(&touched_replica_paths);

        for (session_id, repos) in changed {
            // Clear GitWatcher's cache slot so the next tick re-emits with detected branches.
            git_watcher.invalidate_session_cache(session_id);
            let _ = app.emit(
                "session_git_repos",
                serde_json::json!({
                    "sessionId": session_id.to_string(),
                    "repos": repos,
                }),
            );
        }
    }

    Ok(result)
}

/// Refresh `is_coordinator` on every live session and emit `session_coordinator_changed`
/// for those whose flag flipped. Called by team-CRUD commands (§2).
pub(crate) async fn emit_coordinator_refresh(
    app: &AppHandle,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
) {
    let teams = crate::config::teams::discover_teams();
    let changes = {
        let mgr = session_mgr.read().await;
        mgr.refresh_coordinator_flags(&teams).await
    };
    for (id, is_coord) in changes {
        let _ = app.emit(
            "session_coordinator_changed",
            CoordinatorChangedPayload {
                session_id: id.to_string(),
                is_coordinator: is_coord,
            },
        );
    }
}

/// Read a team's config.json and return its contents.
#[tauri::command]
pub async fn get_team_config(
    project_path: String,
    team_name: String,
) -> Result<TeamConfigResult, String> {
    validate_existing_name(&team_name, "Team")?;
    let base = selected_ac_root(Path::new(&project_path))?;
    read_team_config(&base, &team_name)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------
