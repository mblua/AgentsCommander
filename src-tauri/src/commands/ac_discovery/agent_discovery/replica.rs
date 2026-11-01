use super::super::model::AcAgentReplica;
use super::super::project_git::detect_repo_branch;
use super::super::replica_read::{
    origin_project_for_replica_identity, projected_path_string, read_preferred_agent_id,
    read_replica_config_with_valid_identity, replica_selection_wire_fields,
};
use crate::config::coding_agent_profiles::{
    read_replica_current_coding_agent, read_replica_profile_result,
};
use crate::config::settings::AppSettings;
use crate::filesystem::ac_discovery::canonical_discovery_repo;
use std::{collections::HashMap, path::Path};
pub(crate) struct DiscoveryContext<'a> {
    pub(crate) project_folder: &'a str,
    pub(crate) ac_root: &'a Path,
    pub(crate) cfg: &'a AppSettings,
    pub(crate) clocks_snapshot: &'a HashMap<String, crate::config::coordinator_clocks::ClockEntry>,
    pub(crate) call_id: u64,
}

pub(crate) async fn discover_replica(
    wg_path: &Path,
    wg_dir_name: &str,
    dir_name: &str,
    path: &Path,
    context: &DiscoveryContext<'_>,
) -> AcAgentReplica {
    let DiscoveryContext {
        project_folder,
        ac_root,
        cfg,
        clocks_snapshot,
        call_id,
    } = context;
    let replica_name = wg_dir_name
        .strip_prefix("__agent_")
        .unwrap_or(wg_dir_name)
        .to_string();

    let identity_read = read_replica_config_with_valid_identity(wg_path);

    let identity_path = identity_read
        .identity
        .as_ref()
        .map(|identity| identity.identity.clone());

    let origin_project = origin_project_for_replica_identity(project_folder, &identity_read);

    // Resolve identity to matrix dir and read its lastCodingAgent
    let preferred_agent_id = identity_read
        .identity
        .as_ref()
        .and_then(|identity| read_preferred_agent_id(&identity.matrix_dir, cfg));
    let current_coding_agent_id = read_replica_current_coding_agent(wg_path)
        .filter(|id| cfg.agents.iter().any(|agent| agent.id == *id));
    let profile_read = read_replica_profile_result(wg_path);
    if let Some(warning) = &profile_read.warning {
        log::warn!("[ac-discovery] {}", warning);
    }
    let current_profile = profile_read.profile;

    // Extract repos from config.json and resolve to absolute paths
    let repo_paths = discover_replica_repos(wg_path, identity_read.config.as_ref());

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
    let is_coordinator = crate::config::teams::resolve_wg_coordinator_replica(ac_root, path)
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
    let fqn = format!("{}:{}/{}", project_folder, dir_name, replica_name);
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
    let (saved_pair, selection_state, selection_error) = replica_selection_wire_fields(wg_path);

    AcAgentReplica {
        name: replica_name,
        path: projected_path_string(wg_path),
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
    }
}

fn discover_replica_repos(wg_path: &Path, config: Option<&serde_json::Value>) -> Vec<String> {
    config
        .and_then(|v| v.get("repos")?.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|r| r.as_str())
        .filter_map(|rel| {
            let resolved = wg_path.join(rel);
            canonical_discovery_repo(&resolved)
                .ok()
                .map(|p| projected_path_string(&p))
        })
        .collect()
}
