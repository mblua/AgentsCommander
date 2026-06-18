use std::sync::Arc;

use tauri::State;
use uuid::Uuid;

use crate::config::settings::SettingsState;
use crate::resource_monitor::types::{
    ResourceKillRequest, ResourceKillResult, ResourceLimits, ResourceSnapshot,
};
use crate::resource_monitor::ResourceMonitorState;

#[tauri::command]
pub async fn get_resource_snapshot(
    monitor: State<'_, Arc<ResourceMonitorState>>,
    settings: State<'_, SettingsState>,
) -> Result<ResourceSnapshot, String> {
    let cfg = settings.read().await.clone();
    let limits = ResourceLimits::from(&cfg);
    let monitor = Arc::clone(&monitor);
    tokio::task::spawn_blocking(move || monitor.snapshot(limits))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn kill_resource_group(
    request: ResourceKillRequest,
    monitor: State<'_, Arc<ResourceMonitorState>>,
) -> Result<ResourceKillResult, String> {
    let session_id = Uuid::parse_str(&request.session_id).map_err(|e| e.to_string())?;
    let monitor = Arc::clone(&monitor);
    tokio::task::spawn_blocking(move || monitor.kill_group(session_id, request.reason))
        .await
        .map_err(|e| e.to_string())?
}
