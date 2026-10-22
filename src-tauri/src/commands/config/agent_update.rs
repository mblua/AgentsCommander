use std::sync::Arc;
use tauri::{AppHandle, State};

use crate::config::settings::SettingsState;

use super::preferences::persist_narrow_settings_update;

/// Issue #609 - return the pending "npm update available" info, or null if the
/// running build is current / the check has not finished / it is disabled.
/// Pure read of the cached `UpdateCheckState` (set by the startup task in
/// `update_check::run_startup_check`). The sidebar queries this on mount to
/// cover the case where the startup emit fired before its listener registered.
#[tauri::command]
pub async fn get_update_status(
    cache: State<'_, crate::UpdateCheckState>,
) -> Result<Option<crate::update_check::UpdateInfo>, String> {
    Ok(cache.get().cloned())
}

/// #1327 - snapshot of the startup coding-agent update run (in progress flag +
/// the currently registered-but-unanswered prompt + per-command results). The
/// sidebar queries it on mount to cover events that fired before its listeners
/// registered (mirrors `get_update_status`).
#[tauri::command]
pub async fn get_agent_update_status(
    gate: State<'_, Arc<crate::agent_update::AgentUpdateGate>>,
) -> Result<crate::agent_update::AgentUpdateStatus, String> {
    Ok(gate.snapshot())
}

/// #1327/#1551 - the user answered the startup prompt for one command, from any
/// surface. The gate's serial section classifies, persists, claims the pending
/// prompt, announces `agent_update_prompt_closed` to every surface and only then
/// releases the prompt loop. Returns Ok(true) ONLY when THIS call resolved the
/// pending prompt (its choice was persisted and this boot acts on it); Ok(false)
/// when the prompt had already expired (persisted for future boots, this boot
/// unaffected) or an answer was already recorded this boot (nothing persisted,
/// the earlier answer stands). Unknown commands are rejected.
#[tauri::command]
pub async fn agent_update_answer(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    command: String,
    enabled: bool,
) -> Result<bool, String> {
    agent_update_answer_inner(&app, settings.inner(), command, enabled).await
}

#[tauri::command]
pub async fn agent_update_cancel(
    app: AppHandle,
    command: String,
) -> Result<crate::agent_update::AgentUpdateCancelResponse, String> {
    agent_update_cancel_inner(&app, command).await
}

#[tauri::command]
pub async fn agent_updates_cancel_all(
    app: AppHandle,
) -> Result<crate::agent_update::AgentUpdateCancelAllResponse, String> {
    agent_updates_cancel_all_inner(&app).await
}

/// #1551 - the managed startup gate, or a plain error string when an unmanaged
/// (test) app asks for it, so a missing state never panics a command.
pub(crate) fn managed_agent_update_gate(
    app: &AppHandle,
) -> Result<State<'_, Arc<crate::agent_update::AgentUpdateGate>>, String> {
    tauri::Manager::try_state::<Arc<crate::agent_update::AgentUpdateGate>>(app)
        .ok_or_else(|| "agent update gate is not managed".to_string())
}

/// #1551 - shared by the Tauri command and the WebSocket router. `try_state` so an
/// unmanaged test app errors instead of panicking.
pub fn agent_update_status_inner(
    app: &AppHandle,
) -> Result<crate::agent_update::AgentUpdateStatus, String> {
    Ok(managed_agent_update_gate(app)?.snapshot())
}

/// #1551 - the only answer path: `agent_update::answer_prompt` with the existing
/// narrow persist injected. Classification, persist, and settlement all happen
/// inside the gate's serial section.
pub async fn agent_update_answer_inner(
    app: &AppHandle,
    settings: &SettingsState,
    command: String,
    enabled: bool,
) -> Result<bool, String> {
    let gate = managed_agent_update_gate(app)?;
    crate::agent_update::answer_prompt(app, &gate, &command, enabled, || {
        persist_narrow_settings_update(settings, |candidate| {
            candidate
                .agent_auto_update_by_command
                .insert(command.clone(), enabled);
        })
    })
    .await
}

pub async fn agent_update_cancel_inner(
    app: &AppHandle,
    command: String,
) -> Result<crate::agent_update::AgentUpdateCancelResponse, String> {
    let gate = managed_agent_update_gate(app)?;
    Ok(crate::agent_update::cancel_update(app, &gate, command).await)
}

pub async fn agent_updates_cancel_all_inner(
    app: &AppHandle,
) -> Result<crate::agent_update::AgentUpdateCancelAllResponse, String> {
    let gate = managed_agent_update_gate(app)?;
    Ok(crate::agent_update::cancel_all_updates(app, &gate).await)
}

/// #1551 - instant read of the Settings "Auto-update" table. Never awaits a probe;
/// probes are scheduled in the background only once the startup pass is finished.
/// #1967 P4 - `Err` when the persisted catalog is unavailable (nothing is probed).
pub async fn agent_update_overview_inner(
    app: &AppHandle,
    settings: &SettingsState,
) -> Result<Vec<crate::agent_update::AgentUpdateOverviewRow>, String> {
    let gate = managed_agent_update_gate(app)?;
    let cache = tauri::Manager::try_state::<Arc<crate::agent_version::AgentInstallCache>>(app)
        .ok_or_else(|| "agent install cache is not managed".to_string())?;
    crate::agent_update::update_overview(app, settings, &gate, &cache)
        .await
        .map_err(|unavailable| unavailable.to_string())
}

/// #1551 - one row per update-capable catalog entry with its configured policy,
/// installed version, and this boot's live state.
#[tauri::command]
pub async fn get_agent_update_overview(
    app: AppHandle,
    settings: State<'_, SettingsState>,
) -> Result<Vec<crate::agent_update::AgentUpdateOverviewRow>, String> {
    agent_update_overview_inner(&app, settings.inner()).await
}
