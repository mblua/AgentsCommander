use crate::config::replica_identity::agent_bare_name_from_ref;
use crate::config::replica_identity::read_and_repair_wg_replica_config;
use crate::config::replica_identity::WG_REPLICA_REQUIRED_CONTEXT;
use crate::filesystem::ac_discovery;
use std::path::Path;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WgCoordinatorReplica {
    pub project: String,
    pub team: String,
    pub wg_name: String,
    pub agent_name: String,
    pub replica_dir: PathBuf,
}

pub fn resolve_wg_coordinator_replica(
    ac_root: &Path,
    wg_dir: &Path,
) -> Option<WgCoordinatorReplica> {
    let project = ac_root.parent()?.file_name()?.to_str()?.to_string();
    let wg_name = wg_dir.file_name()?.to_str()?.to_string();
    let team = crate::config::entity_prefix::strip_entity_prefix(&wg_name)
        .and_then(|s| s.split_once('-').map(|(_, rest)| rest.to_string()))?;

    let team_dir = ac_root.join(format!("_team_{team}"));
    let team_config: serde_json::Value =
        ac_discovery::read_team_discovery_config(&crate::config::config_file_path(&team_dir))
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())?;
    let coordinator_ref = team_config.get("coordinator").and_then(|c| c.as_str())?;
    let coordinator_name = agent_bare_name_from_ref(coordinator_ref).ok()?;
    if !ac_discovery::discovery_path_is_dir(&ac_root.join(format!("_agent_{coordinator_name}"))) {
        return None;
    }

    for (replica_dir, _) in ac_discovery::team_discovery_entries(wg_dir).ok()?.flatten() {
        if !ac_discovery::discovery_path_is_dir(&replica_dir) {
            continue;
        }
        let _dir_name = match replica_dir.file_name().and_then(|n| n.to_str()) {
            Some(n) if n.starts_with("__agent_") => n,
            _ => continue,
        };
        let Ok((_config, identity)) =
            read_and_repair_wg_replica_config(&replica_dir, WG_REPLICA_REQUIRED_CONTEXT)
        else {
            continue;
        };
        if identity.agent_name == coordinator_name {
            return Some(WgCoordinatorReplica {
                project,
                team,
                wg_name,
                agent_name: identity.agent_name,
                replica_dir,
            });
        }
    }

    None
}
