use tauri::{AppHandle, Emitter, State};

use crate::config::settings::{
    save_settings, validate_and_repair_settings, AppSettings, CodingAgentEnv,
    CodingAgentProfilesConfig, SettingsState,
};

use super::CodingAgentProfileResolutionResult;

#[tauri::command]
pub async fn update_coding_agent_profiles(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    profiles: CodingAgentProfilesConfig,
) -> Result<(), String> {
    persist_coding_agent_profiles_update(settings.inner(), profiles).await?;
    let _ = app.emit("coding_agent_profiles_updated", serde_json::json!({}));
    Ok(())
}

pub(crate) async fn persist_coding_agent_profiles_update(
    settings: &SettingsState,
    profiles: CodingAgentProfilesConfig,
) -> Result<(), String> {
    let mut s = settings.write().await;
    let mut candidate = s.clone();
    candidate.coding_agent_profiles = profiles;
    validate_and_repair_settings(&mut candidate)?;
    let written = save_settings(&candidate)?;
    *s = written;
    Ok(())
}

#[tauri::command]
pub async fn update_coding_agent_env_settings(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    agent_id: String,
    envs: Vec<CodingAgentEnv>,
    isolated_home: bool,
) -> Result<(), String> {
    persist_coding_agent_env_settings_update(settings.inner(), &agent_id, envs, isolated_home)
        .await?;
    let _ = app.emit(
        "coding_agent_env_settings_updated",
        serde_json::json!({ "agentId": agent_id }),
    );
    Ok(())
}

pub(crate) async fn persist_coding_agent_env_settings_update(
    settings: &SettingsState,
    agent_id: &str,
    envs: Vec<CodingAgentEnv>,
    isolated_home: bool,
) -> Result<(), String> {
    persist_coding_agent_env_settings_update_with_saver(
        settings,
        agent_id,
        envs,
        isolated_home,
        save_settings,
    )
    .await
}

pub(crate) async fn persist_coding_agent_env_settings_update_with_saver(
    settings: &SettingsState,
    agent_id: &str,
    envs: Vec<CodingAgentEnv>,
    isolated_home: bool,
    save: impl FnOnce(&AppSettings) -> Result<AppSettings, String>,
) -> Result<(), String> {
    let mut s = settings.write().await;
    let mut candidate = s.clone();
    let agent = candidate
        .agents
        .iter_mut()
        .find(|agent| agent.id == agent_id)
        .ok_or_else(|| format!("Agent '{}' is not configured", agent_id))?;
    agent.envs = envs;
    agent.isolated_home = isolated_home;
    validate_and_repair_settings(&mut candidate)?;
    let written = save(&candidate)?;
    *s = written;
    Ok(())
}

#[tauri::command]
pub async fn set_agent_default_profile(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    agent_path: String,
    profile: String,
) -> Result<(), String> {
    let payload = set_agent_default_profile_inner(settings.inner(), &agent_path, &profile).await?;
    let _ = app.emit("coding_agent_profile_selection_updated", payload);
    Ok(())
}

/// Persist an origin agent's default profile and return the
/// `coding_agent_profile_selection_updated` event payload for the caller to
/// emit (desktop) or broadcast to web clients (browser transport). Shared by
/// the desktop command and the web dispatcher.
pub(crate) async fn set_agent_default_profile_inner(
    settings: &SettingsState,
    agent_path: &str,
    profile: &str,
) -> Result<serde_json::Value, String> {
    let snapshot = settings.read().await.clone();
    crate::config::coding_agent_profiles::set_agent_default_profile(
        &snapshot,
        std::path::Path::new(agent_path),
        profile,
    )?;
    Ok(serde_json::json!({ "agentPath": agent_path, "profile": profile, "scope": "default" }))
}

#[tauri::command]
pub async fn set_instance_profile_override(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    agent_path: String,
    profile: Option<String>,
) -> Result<(), String> {
    // #1940 - settings snapshot, guarded profile write and event publication are
    // one owned selection operation, so this override cannot interleave with a
    // broad apply or a self-switch persist/restart.
    let app_handle = app.clone();
    let settings = settings.inner().clone();
    crate::session::selection::run_owned_selection_operation(move || {
        let app = app_handle.clone();
        async move {
            let payload =
                set_instance_profile_override_inner(&settings, &agent_path, profile.as_deref())
                    .await?;
            let _ = app.emit("coding_agent_profile_selection_updated", payload);
            Ok(())
        }
    })
    .await
}

/// Persist a replica's instance-level profile override (or clear it when
/// `profile` is `None`) and return the `coding_agent_profile_selection_updated`
/// event payload for the caller to emit or broadcast. Shared by the desktop
/// command and the web dispatcher.
pub(crate) async fn set_instance_profile_override_inner(
    settings: &SettingsState,
    agent_path: &str,
    profile: Option<&str>,
) -> Result<serde_json::Value, String> {
    let snapshot = settings.read().await.clone();
    crate::config::coding_agent_profiles::set_instance_profile_override(
        &snapshot,
        std::path::Path::new(agent_path),
        profile,
    )?;
    Ok(serde_json::json!({ "agentPath": agent_path, "profile": profile, "scope": "instance" }))
}

#[tauri::command]
pub async fn resolve_coding_agent_profile(
    settings: State<'_, SettingsState>,
    agent_path: Option<String>,
    agent_id: String,
    requested_profile: Option<String>,
) -> Result<CodingAgentProfileResolutionResult, String> {
    resolve_coding_agent_profile_inner(
        settings.inner(),
        agent_path.as_deref(),
        &agent_id,
        requested_profile.as_deref(),
    )
    .await
}

/// Resolve the effective coding-agent profile for an agent/replica. Shared by
/// the desktop command and the web dispatcher; reads settings only.
pub(crate) async fn resolve_coding_agent_profile_inner(
    settings: &SettingsState,
    agent_path: Option<&str>,
    agent_id: &str,
    requested_profile: Option<&str>,
) -> Result<CodingAgentProfileResolutionResult, String> {
    let snapshot = settings.read().await.clone();
    let agent_path = agent_path
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(std::path::PathBuf::from);
    let details = crate::config::coding_agent_profiles::resolve_profile_selection(
        &snapshot,
        agent_path.as_deref(),
        agent_id,
        requested_profile,
    )?;

    Ok(CodingAgentProfileResolutionResult {
        requested_profile: details.resolution.requested_profile,
        effective_profile: details.resolution.effective_profile,
        fallback_chain: details.resolution.fallback_chain,
        fallback_applied: details.resolution.fallback_applied,
        requested_profile_input: details.requested_profile_input,
        instance_profile_override: details.instance_profile_override,
        origin_default_profile: details.origin_default_profile,
        agent_default_profile: details.agent_default_profile,
        warnings: details.resolution.warnings,
    })
}
