//! Production project environment of context history (#4001).
//!
//! The composition side owns the concrete adapter so `history_runtime`
//! depends only on its port, never on `config::teams` or `config::root_agent`.
//! History configuration is the app-wide Settings snapshot (#4049); no project
//! file is read for it.

use std::path::Path;

use crate::config::settings::SettingsState;
use crate::context_history::config::{ConfigStatus, HistoryConfig, LoadedHistoryConfig};
use crate::context_history::observation::TeamMembers;
use crate::context_history::store::{HistoryWriter, StoreError};
use crate::history_runtime::scopes::{HistoryAppend, ProjectEnvironment};

/// Fixed, path-free diagnostic when the Settings owner cannot be read.
const SETTINGS_UNAVAILABLE: &str = "settings_unavailable";

/// Production environment: the Settings owner, team discovery and store.
pub(crate) struct DiskProjectEnvironment {
    settings: SettingsState,
}

impl DiskProjectEnvironment {
    pub(crate) fn new(settings: SettingsState) -> Self {
        Self { settings }
    }

    /// An adapter over default Settings, for tests that need no real profile.
    #[cfg(test)]
    pub(crate) fn for_tests() -> Self {
        Self::new(crate::config::settings::new_settings_state(
            crate::config::settings::AppSettings::default(),
        ))
    }
}

impl ProjectEnvironment for DiskProjectEnvironment {
    /// Copy the two history fields from the current Settings snapshot and drop
    /// the guard before returning; every root gets the same configuration. The
    /// runtime calls this on a blocking thread. An unavailable owner fails closed.
    fn load_config(&self, _ac_root: &Path) -> LoadedHistoryConfig {
        match self.settings.blocking_current_read() {
            Ok(snapshot) => LoadedHistoryConfig {
                config: HistoryConfig {
                    enabled: snapshot.context_history_enabled,
                    retention_days: snapshot.context_history_retention_days.days(),
                },
                status: ConfigStatus::Loaded,
            },
            Err(_) => LoadedHistoryConfig {
                config: HistoryConfig::DISABLED,
                status: ConfigStatus::Invalid(SETTINGS_UNAVAILABLE.to_string()),
            },
        }
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

#[cfg(test)]
mod tests;
