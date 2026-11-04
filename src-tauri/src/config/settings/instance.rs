//! Instance settings location and typed access to its existing persistence operations.

use crate::config::instance_artifacts::SETTINGS_FILE_NAME;
use std::path::PathBuf;

#[cfg(test)]
use super::{AgentHelpFile, BlockingMenusFile, ProjectPathsRepairStage};
use super::{AppSettings, ProjectPathsWriteTransaction};
#[cfg(test)]
use chrono::{DateTime, Utc};
use serde_json::{Map, Value};
use std::path::Path;

pub(crate) fn settings_path() -> Option<PathBuf> {
    super::super::config_dir().map(|d| d.join(SETTINGS_FILE_NAME))
}

#[derive(Clone)]
pub(crate) struct SettingsFileTarget {
    path: PathBuf,
}

pub(crate) fn instance_settings_target() -> Option<SettingsFileTarget> {
    settings_path().map(|path| SettingsFileTarget { path })
}

#[cfg(test)]
pub(crate) fn instance_settings_display_path() -> Option<String> {
    settings_path().map(|path| path.display().to_string())
}

pub(crate) fn persisted_instance_token_matches(directory: &Path, token: &str) -> bool {
    let settings_token = crate::path_identity::read_bounded_regular(
        &directory.join(SETTINGS_FILE_NAME),
        1024 * 1024,
    )
    .ok()
    .and_then(|(bytes, _)| crate::path_identity::parse_json_no_duplicates(&bytes).ok())
    .and_then(|value| {
        value
            .get("rootToken")
            .and_then(Value::as_str)
            .map(str::to_owned)
    });
    settings_token.as_deref() == Some(token)
}

pub(crate) struct SettingsProjectTransaction {
    transaction: ProjectPathsWriteTransaction,
}

pub(crate) struct SettingsProjectCheckpoint {
    projection: Map<String, Value>,
}

impl SettingsFileTarget {
    pub(crate) fn begin_project_paths(
        &self,
        current: &AppSettings,
    ) -> Result<SettingsProjectTransaction, String> {
        super::begin_project_paths_transaction_at_path(current, &self.path)
            .map(|transaction| SettingsProjectTransaction { transaction })
    }

    #[cfg(test)]
    pub(crate) fn repair_project_paths(
        &self,
        current: &mut AppSettings,
    ) -> Result<(), (ProjectPathsRepairStage, String)> {
        super::repair_fresh_project_paths_at_path(current, &self.path)
    }

    #[cfg(test)]
    pub(crate) fn read_agent_help_layers(&self) -> (AgentHelpFile, Option<String>, AgentHelpFile) {
        let (local, error) = super::load_local_agent_help_file(&self.path);
        let remote = super::load_remote_agent_help_file(&self.path);
        (local, error, remote)
    }

    #[cfg(test)]
    pub(crate) fn read_remote_agent_help_check_stamp(&self) -> Option<DateTime<Utc>> {
        super::read_remote_agent_help_check_stamp(&self.path)
    }

    #[cfg(test)]
    pub(crate) fn read_remote_blocking_menus_check_stamp(&self) -> Option<DateTime<Utc>> {
        super::read_remote_blocking_menus_check_stamp(&self.path)
    }

    #[cfg(test)]
    pub(crate) fn write_remote_agent_help_check_stamp(
        &self,
        at: DateTime<Utc>,
    ) -> Result<(), String> {
        super::write_remote_agent_help_check_stamp(&self.path, at)
    }

    #[cfg(test)]
    pub(crate) fn write_remote_blocking_menus_check_stamp(
        &self,
        at: DateTime<Utc>,
    ) -> Result<(), String> {
        super::write_remote_blocking_menus_check_stamp(&self.path, at)
    }

    #[cfg(test)]
    pub(crate) fn write_remote_agent_help_cache(
        &self,
        file: AgentHelpFile,
        source_url: &str,
        fetched_at: DateTime<Utc>,
    ) -> Result<(), String> {
        super::write_remote_agent_help_cache(&self.path, file, source_url, fetched_at)
    }

    #[cfg(test)]
    pub(crate) fn write_remote_blocking_menus_cache(
        &self,
        file: BlockingMenusFile,
        source_url: &str,
        fetched_at: DateTime<Utc>,
    ) -> Result<(), String> {
        super::write_remote_blocking_menus_cache(&self.path, file, source_url, fetched_at)
    }

    #[cfg(test)]
    pub(crate) fn for_test_path(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
        }
    }
}

impl SettingsProjectTransaction {
    pub(crate) fn fresh_settings(&self) -> &AppSettings {
        self.transaction.fresh_settings()
    }

    pub(crate) fn refresh_raw_projects(&self, current: &mut AppSettings) -> Result<(), String> {
        self.transaction.refresh_raw_projects(current)
    }

    pub(crate) fn commit_project_groups(
        &mut self,
        candidate: &AppSettings,
        active: bool,
        archived: bool,
    ) -> Result<AppSettings, String> {
        self.transaction
            .commit_project_groups(candidate, active, archived)
    }

    pub(crate) fn checkpoint(&self) -> Result<SettingsProjectCheckpoint, String> {
        self.transaction
            .raw_project_projection()
            .map(|projection| SettingsProjectCheckpoint { projection })
    }

    pub(crate) fn restore_checkpoint(
        &mut self,
        checkpoint: &SettingsProjectCheckpoint,
    ) -> Result<AppSettings, String> {
        self.transaction
            .restore_raw_project_projection(&checkpoint.projection)
    }
}

#[cfg(test)]
pub(crate) fn settings_fixture_path(directory: &Path) -> PathBuf {
    directory.join(SETTINGS_FILE_NAME)
}

#[cfg(test)]
mod tests;
