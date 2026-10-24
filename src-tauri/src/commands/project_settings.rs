use std::path::Path;

use crate::config::project_settings::{
    load_workgroup_groups, save_workgroup_groups, WorkgroupGroupsConfig,
};
use crate::web::broadcast::WsBroadcaster;
use crate::web::event_broadcast::broadcast_all;
use serde_json::{json, Value};
use tauri::{AppHandle, State};

pub(crate) const PROJECT_GROUPS_UPDATED_EVENT: &str = "project_groups_updated";

pub(crate) fn project_groups_updated_payload(
    project_path: &str,
    config: &WorkgroupGroupsConfig,
) -> Value {
    json!({ "projectPath": project_path, "config": config })
}

pub(crate) fn get_project_groups_inner(path: &str) -> Result<WorkgroupGroupsConfig, String> {
    load_workgroup_groups(Path::new(path))
}

#[tauri::command]
pub async fn get_project_groups(path: String) -> Result<WorkgroupGroupsConfig, String> {
    get_project_groups_inner(&path)
}

pub(crate) fn update_project_groups_inner(
    path: &str,
    config: WorkgroupGroupsConfig,
) -> Result<WorkgroupGroupsConfig, String> {
    save_workgroup_groups(Path::new(path), config)
}

#[tauri::command]
pub async fn update_project_groups(
    app: AppHandle,
    broadcaster: State<'_, WsBroadcaster>,
    path: String,
    config: WorkgroupGroupsConfig,
) -> Result<WorkgroupGroupsConfig, String> {
    let result = update_project_groups_inner(&path, config)?;
    let payload = project_groups_updated_payload(&path, &result);
    broadcast_all(
        &app,
        broadcaster.inner(),
        PROJECT_GROUPS_UPDATED_EVENT,
        &payload,
    );
    Ok(result)
}

#[cfg(test)]
mod tests;
