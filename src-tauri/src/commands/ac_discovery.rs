mod agent_discovery;
mod branch_watcher;
mod context_templates;
mod gitignore;
mod model;
mod project_archive;
mod project_create;
mod project_discovery;
mod project_git;
mod project_registry;
mod replica_read;
mod task_snapshot;

#[cfg(test)]
use crate::config::settings::SettingsState;
#[cfg(test)]
use crate::pty::manager::{PendingSpawn, PtyManager};
#[cfg(test)]
use crate::session::manager::SessionManager;
#[cfg(test)]
use crate::session::session::{SessionInfo, SessionStatus};
#[cfg(test)]
use std::collections::HashMap;
#[cfg(test)]
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
#[cfg(test)]
use std::sync::{Arc, Mutex};
#[cfg(test)]
use std::time::Duration;
#[cfg(test)]
use tauri::Manager;
#[cfg(test)]
use uuid::Uuid;

pub use agent_discovery::{
    __cmd__discover_ac_agents, __tauri_command_name_discover_ac_agents, discover_ac_agents,
};
pub use branch_watcher::DiscoveryBranchWatcher;
#[cfg(test)]
use context_templates::overwrite_context_template_recorded;
pub use context_templates::{
    __cmd__get_replica_context_files, __cmd__keep_custom_context_template,
    __cmd__overwrite_context_template_with_default, __cmd__set_replica_context_files,
    __tauri_command_name_get_replica_context_files,
    __tauri_command_name_keep_custom_context_template,
    __tauri_command_name_overwrite_context_template_with_default,
    __tauri_command_name_set_replica_context_files, get_replica_context_files,
    keep_custom_context_template, overwrite_context_template_with_default,
    set_replica_context_files,
};
pub(crate) use gitignore::ensure_ac_root_gitignore;
#[cfg(test)]
pub(crate) use gitignore::ensure_ac_root_gitignore_with_names;
pub use model::{
    AcAgentMatrix, AcAgentReplica, AcDiscoveryResult, AcReplicaSavedPair, AcTeam, AcWorkgroup,
};
pub use project_archive::{
    __cmd__archive_project, __cmd__list_archived_projects, __cmd__unarchive_project,
    __tauri_command_name_archive_project, __tauri_command_name_list_archived_projects,
    __tauri_command_name_unarchive_project, archive_project, list_archived_projects,
    unarchive_project,
};
#[cfg(test)]
use project_archive::{archive_blocked_message, archive_project_inner_with_settings_path};
#[cfg(test)]
pub(crate) use project_archive::{archive_blockers, ArchiveSnapshot};
pub(crate) use project_archive::{
    archive_project_inner, list_archived_projects_inner, unarchive_project_inner,
};
pub(crate) use project_create::check_project_path_inner;
pub use project_create::{
    __cmd__check_project_path, __cmd__create_ac_project, __tauri_command_name_check_project_path,
    __tauri_command_name_create_ac_project, check_project_path, create_ac_project,
};
#[cfg(test)]
use project_create::{create_ac_project_impl, create_ac_project_impl_with_hook};
pub(crate) use project_discovery::discover_project_inner;
pub use project_discovery::{
    __cmd__discover_project, __tauri_command_name_discover_project, discover_project,
};
pub use project_git::{
    __cmd__get_project_file_difference, __tauri_command_name_get_project_file_difference,
    get_project_file_difference, get_project_file_difference_inner, ProjectFileDifference,
};
#[cfg(test)]
use project_git::{
    parse_nul_records, read_project_file_difference, strip_line_terminator,
    PROJECT_GIT_REDIRECT_ENV,
};
pub use project_registry::{
    __cmd__new_project, __cmd__open_project, __cmd__remove_project,
    __tauri_command_name_new_project, __tauri_command_name_open_project,
    __tauri_command_name_remove_project, new_project, open_project, remove_project,
};
#[cfg(test)]
use project_registry::{
    new_project_inner_with_event_and_settings_path, new_project_inner_with_settings_path,
    new_project_inner_with_settings_path_and_hooks,
    open_project_inner_with_event_and_settings_path, open_project_inner_with_settings_path,
    remove_project_inner_with_settings_path, NewProjectTransactionHooks,
};
pub(crate) use project_registry::{open_project_inner, remove_project_inner};
#[cfg(test)]
pub(crate) use project_registry::{ArchiveChangeReason, ProjectArchiveChanged};
#[cfg(all(test, windows))]
use replica_read::projected_path_string;
#[cfg(test)]
use replica_read::{
    extract_task_first_line, origin_project_for_replica_identity, read_preferred_agent_id,
    read_preferred_agent_id_logged, read_replica_config_with_valid_identity,
};
pub(crate) use task_snapshot::task_snapshot_inner;
pub use task_snapshot::{
    __cmd__task_get_snapshot, __cmd__task_get_snapshot_at, __tauri_command_name_task_get_snapshot,
    __tauri_command_name_task_get_snapshot_at, task_get_snapshot, task_get_snapshot_at,
};
#[cfg(test)]
use task_snapshot::{
    deliver_poll_task, poll_task_snapshot, validate_snapshot_room, DiscoveryBranchPayload,
    ReplicaBranchEntry,
};

/// Per-call monotonic ID partitioning concurrent `discover_*` invocations in the log
/// stream. See plan §3 A0 (round-2 G5 + round-3 H1 placement-fix). Consumed only by
/// `format!`-into-log; `Relaxed` is canonical for an observed-but-not-synchronizing counter.
static DISCOVERY_CALL_ID: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
mod tests;
