use serde::{Deserialize, Serialize};
use std::path::PathBuf;

mod agent_delete_plan;
mod agent_matrix;
mod delete_guard;
mod matrix_copy;
mod matrix_layout;
mod repo_clone;
mod room_number;
mod room_task;
mod team_config;
mod team_create;
mod team_delete;
mod team_update;
mod workgroup;
mod workgroup_delete;

pub use agent_matrix::{
    __cmd__create_agent_matrix, __cmd__delete_agent_matrix,
    __tauri_command_name_create_agent_matrix, __tauri_command_name_delete_agent_matrix,
    create_agent_matrix, delete_agent_matrix,
};
#[cfg(test)]
pub(crate) use delete_guard::is_file_in_use_error;
pub(crate) use delete_guard::{
    metadata_is_link_or_reparse, validate_delete_root_not_link_or_reparse,
};
#[cfg(test)]
pub(crate) use matrix_layout::build_role_content;
pub(crate) use matrix_layout::{
    create_agent_matrix_from_role, create_agent_matrix_layout, create_agent_matrix_on_disk,
    sanitize_name, validate_existing_name, CreateAgentMatrixDiskArgs,
    CreateAgentMatrixFromRoleArgs,
};
pub(crate) use room_number::{room_dir_name, room_number_pad_width};
pub(crate) use room_task::{parse_task_title, read_room_task_snapshot_json};
pub(crate) use team_config::{
    create_new_team_config_on_disk, normalize_team_config_for_project, read_team_config,
    read_team_config_classified, resolve_agent_ref, write_team_config_guarded,
    TeamConfigMutationGuard, TeamConfigReadError,
};
pub use team_create::{
    __cmd__create_team, __cmd__create_workgroup, __cmd__list_all_agents,
    __tauri_command_name_create_team, __tauri_command_name_create_workgroup,
    __tauri_command_name_list_all_agents, create_team, create_workgroup, list_all_agents,
};
pub use team_delete::{__cmd__delete_team, __tauri_command_name_delete_team, delete_team};
pub use team_update::{
    __cmd__get_team_config, __cmd__update_team, __tauri_command_name_get_team_config,
    __tauri_command_name_update_team, get_team_config, update_team,
};
pub(crate) use workgroup::{
    acquire_lifecycle_project_gate, agent_ref_bare_name, clone_missing_repos_for_workgroup,
    create_or_update_replica_on_disk, create_workgroup_on_disk, list_workgroup_dirs,
    parse_team_from_workgroup_name, prune_replica_config_scope, prune_workgroup_config_scope,
    remove_replica_dir, ReplicaRemovalOutcome,
};
pub use workgroup_delete::{
    __cmd__delete_workgroup, __tauri_command_name_delete_workgroup, delete_workgroup,
};
pub(crate) use workgroup_delete::{
    check_workgroup_repos_dirty, try_atomic_delete_wg, WgDeleteOutcome,
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedEntityResult {
    /// Absolute path to the created directory
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    pub name: String,
    pub description: String,
    pub path: String,
    pub project_name: String,
    pub project_path: String,
    pub is_local: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamAgentSource {
    pub agent_ref: String,
    pub source_project_path: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoAssignment {
    pub url: String,
    pub agents: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamConfigResult {
    #[serde(default)]
    pub agents: Vec<String>,
    #[serde(default)]
    pub coordinator: String,
    #[serde(default)]
    pub repos: Vec<RepoAssignment>,
    #[serde(default)]
    pub context_alert_percentages: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkgroupCloneResult {
    /// Absolute path to the created workgroup directory
    pub path: String,
    /// Repos that failed to clone (url + error message). Empty = all succeeded.
    pub clone_errors: Vec<CloneError>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloneError {
    pub url: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncError {
    pub replica: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncResult {
    pub workgroups_updated: u32,
    pub replicas_updated: u32,
    pub errors: Vec<SyncError>,
}

#[derive(Debug, Clone)]
pub(crate) struct WorkgroupDiskCreateArgs {
    pub project_path: PathBuf,
    pub team_name: String,
    pub task_title: String,
    // Legacy provisioning path. Ordinary callers should pass None and empty
    // vectors so workgroup creation activates an existing team config.
    pub coordinator: Option<String>,
    pub agents: Vec<String>,
    pub repos: Vec<RepoAssignment>,
}

#[derive(Debug, Clone)]
pub(crate) struct ReplicaDiskCreateArgs {
    pub ac_root: PathBuf,
    pub wg_dir: PathBuf,
    pub agent_path: String,
    pub team_repos: Vec<RepoAssignment>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Agent Matrix / replica directory contract (issue #209)
// ---------------------------------------------------------------------------
//
// Agent Matrices own canonical state (`memory/`, `plans/`, `skills/`) plus
// mailboxes. Workgroup replicas own only mailboxes; their config points back
// to the origin matrix for canonical state.
pub(crate) const AGENT_MATRIX_DIRS: &[&str] = &["memory", "plans", "skills", "inbox", "outbox"];

pub(crate) const AGENT_REPLICA_DIRS: &[&str] = &["inbox", "outbox"];

#[cfg(test)]
mod tests;
