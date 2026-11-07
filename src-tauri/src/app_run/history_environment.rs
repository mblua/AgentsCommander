//! Production project environment of context history (#4001).
//!
//! The composition side owns the concrete adapter so `history_runtime`
//! depends only on its port, never on `config::teams` or `config::root_agent`.

use std::path::Path;

use crate::context_history::config::{load_history_config, HistoryConfig, LoadedHistoryConfig};
use crate::context_history::observation::TeamMembers;
use crate::context_history::store::{HistoryWriter, StoreError};
use crate::history_runtime::scopes::{HistoryAppend, ProjectEnvironment};

/// Production environment: the real config owner, team discovery and store.
pub(crate) struct DiskProjectEnvironment;

impl ProjectEnvironment for DiskProjectEnvironment {
    fn load_config(&self, ac_root: &Path) -> LoadedHistoryConfig {
        load_history_config(ac_root)
    }

    fn authoritative_ac_root(&self, ac_root: &Path) -> bool {
        crate::config::ac_root::ensure_authoritative_ac_root(ac_root).is_ok()
    }

    fn teams(&self, project_path: &Path) -> Vec<TeamMembers> {
        let mut teams = Vec::new();
        crate::config::teams::discover_teams_in_project_filtered(project_path, &mut teams, |_| {
            true
        });
        teams
            .into_iter()
            .map(|team| TeamMembers {
                team_id: team.name,
                member_paths: team
                    .agent_paths
                    .into_iter()
                    .flatten()
                    .chain(team.coordinator_path)
                    .filter_map(|path| path.to_str().map(str::to_string))
                    .collect(),
            })
            .collect()
    }

    fn open_writer(
        &self,
        ac_root: &Path,
        config: &HistoryConfig,
    ) -> Result<Box<dyn HistoryAppend>, StoreError> {
        Ok(Box::new(HistoryWriter::open(ac_root, config)?))
    }

    fn is_root_directory(&self, working_directory: &str) -> bool {
        crate::config::root_agent::is_root_agent_dir_name(working_directory)
            || crate::config::root_agent::is_root_agent_path(working_directory)
    }
}
