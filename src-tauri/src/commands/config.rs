use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};

use crate::config::claude_settings::{ensure_rtk_pretool_hook, enumerate_managed_agent_dirs};
use crate::config::settings::{
    load_settings, merge_protected_coding_agent_settings, save_settings,
    validate_and_repair_settings, AppSettings, CodingAgentEnv, CodingAgentProfilesConfig,
    SettingsState,
};
use crate::network::OutboundNetwork;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::session::SessionInfo;
use crate::web::auth::WebAccessToken;
use crate::web::broadcast::WsBroadcaster;
use crate::{RtkStartupModeState, RtkSweepLockState, WebServerHandle};

const HOME_MARKDOWN_URL: &str =
    "https://raw.githubusercontent.com/mblua/AgentsCommander/main/docs/home-en.md";

const HOME_MARKDOWN_MAX_BYTES: usize = 256 * 1024; // 256 KB
const HOME_MARKDOWN_TIMEOUT_SECS: u64 = 5;
const WEB_STATUS_CONNECT_TIMEOUT_MS: u64 = 500;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RtkSweepResult {
    pub total: u32,
    pub succeeded: u32,
    pub errors: Vec<RtkSweepError>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RtkSweepError {
    pub path: String,
    pub error: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodingAgentProfileResolutionResult {
    pub requested_profile: String,
    pub effective_profile: String,
    pub fallback_chain: Vec<String>,
    pub fallback_applied: bool,
    pub requested_profile_input: Option<String>,
    pub instance_profile_override: Option<String>,
    pub origin_default_profile: Option<String>,
    pub agent_default_profile: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SettingsDraftUpdateEvents {
    pub profiles_changed: bool,
    pub env_agent_ids: Vec<String>,
}

#[tauri::command]
pub async fn save_debug_logs(content: String) -> Result<(), String> {
    let path = crate::config::config_dir()
        .ok_or("No config dir")?
        .join("debug-logs.txt");
    tokio::fs::write(&path, &content)
        .await
        .map_err(|e| format!("Failed to write logs: {}", e))?;
    log::info!("Debug logs saved to {:?} ({} bytes)", path, content.len());
    Ok(())
}

/// #264 — read-and-clear the buffered ERROR-level log entries for the UI error
/// modal. The frontend calls this once on `ErrorModal` mount (to collect errors
/// logged before the webview was listening) and again on every `error_log_event`
/// ping. Sync (no I/O, no `.await`) — a sub-microsecond mutex take.
#[tauri::command]
pub fn drain_error_logs() -> Vec<crate::logging::ErrorLogEntry> {
    crate::logging::error_sink().drain()
}

#[tauri::command]
pub async fn get_settings(settings: State<'_, SettingsState>) -> Result<AppSettings, String> {
    let s = settings.read().await;
    let mut result = s.clone();
    result.root_token = None; // never expose root token to frontend
    Ok(result)
}

#[tauri::command]
pub async fn update_settings(
    settings: State<'_, SettingsState>,
    new_settings: AppSettings,
) -> Result<(), String> {
    let saved = persist_protected_settings_update(settings.inner(), new_settings).await?;
    purge_sessions_after_settings_update(&saved).await;
    Ok(())
}

#[tauri::command]
pub async fn save_settings_draft(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    draft: AppSettings,
) -> Result<(), String> {
    let (saved, events) = persist_settings_draft_update(settings.inner(), draft).await?;
    purge_sessions_after_settings_update(&saved).await;
    emit_settings_draft_update_events(&app, &events);
    // #612 apply the (possibly changed) log level live + broadcast so every
    // webview re-applies its console gate. Idempotent and cheap; runs only on an
    // explicit Settings Save. `saved` is the binding already destructured from
    // the persist call above, so this does NOT re-persist.
    let level = crate::logging::normalize_log_level(saved.log_level.as_deref().unwrap_or("info"))
        .unwrap_or("info");
    apply_and_broadcast_log_level(&app, level);
    Ok(())
}

pub(crate) async fn persist_protected_settings_update(
    settings: &SettingsState,
    new_settings: AppSettings,
) -> Result<AppSettings, String> {
    persist_protected_settings_update_with_saver(settings, new_settings, save_settings).await
}

async fn persist_protected_settings_update_with_saver(
    settings: &SettingsState,
    new_settings: AppSettings,
    save: impl FnOnce(&AppSettings) -> Result<(), String>,
) -> Result<AppSettings, String> {
    let mut s = settings.write().await;
    let current = s.clone();
    let candidate = build_protected_settings_candidate(&current, new_settings)?;
    save(&candidate)?;
    *s = candidate.clone();
    Ok(candidate)
}

fn build_protected_settings_candidate(
    current: &AppSettings,
    new_settings: AppSettings,
) -> Result<AppSettings, String> {
    let mut candidate = merge_protected_coding_agent_settings(current, new_settings);
    // Preserve existing root token. Frontend settings payloads cannot overwrite it.
    candidate.root_token = current.root_token.clone();
    validate_and_repair_settings(&mut candidate)?;
    Ok(candidate)
}

pub(crate) async fn persist_settings_draft_update(
    settings: &SettingsState,
    draft: AppSettings,
) -> Result<(AppSettings, SettingsDraftUpdateEvents), String> {
    persist_settings_draft_update_with_saver(settings, draft, save_settings).await
}

async fn persist_settings_draft_update_with_saver(
    settings: &SettingsState,
    mut draft: AppSettings,
    save: impl FnOnce(&AppSettings) -> Result<(), String>,
) -> Result<(AppSettings, SettingsDraftUpdateEvents), String> {
    let mut s = settings.write().await;
    let current = s.clone();
    draft.root_token = current.root_token.clone();
    validate_and_repair_settings(&mut draft)?;
    let events = settings_draft_update_events(&current, &draft);
    save(&draft)?;
    *s = draft.clone();
    Ok((draft, events))
}

pub(crate) async fn purge_sessions_after_settings_update(saved: &AppSettings) {
    if let Err(e) = crate::config::sessions_persistence::purge_sessions_outside_project_paths(
        &saved.project_paths,
    )
    .await
    {
        log::warn!(
            "[settings] Failed to purge sessions outside current projectPaths after settings update: {}",
            e
        );
    }
}

fn settings_draft_update_events(
    before: &AppSettings,
    after: &AppSettings,
) -> SettingsDraftUpdateEvents {
    let before_envs = agent_env_settings_by_id(before);
    let after_envs = agent_env_settings_by_id(after);
    let mut env_agent_ids = Vec::new();

    for agent_id in before_envs.keys().chain(after_envs.keys()) {
        if env_agent_ids.iter().any(|existing| existing == agent_id) {
            continue;
        }
        if before_envs.get(agent_id) != after_envs.get(agent_id) {
            env_agent_ids.push(agent_id.clone());
        }
    }

    SettingsDraftUpdateEvents {
        // #592/#597 - the drift content-hash fingerprints the EFFECTIVE command
        // (agent base command + cell command, via compose_effective_command), so a
        // bare agent-base-command edit (e.g. `claude` -> `claude-amp -c ...`) is real
        // drift even when the profile cells are untouched. It is neither a
        // coding_agent_profiles change nor an env change, so without folding it in
        // here the SettingsModal save path emits NO refresh event and the outdated
        // badge never updates. coding_agent_profiles_updated drives
        // refreshProfileOutdated in the sidebar, so route base-command edits through it.
        profiles_changed: before.coding_agent_profiles != after.coding_agent_profiles
            || agent_commands_by_id(before) != agent_commands_by_id(after),
        env_agent_ids,
    }
}

fn agent_env_settings_by_id(
    settings: &AppSettings,
) -> BTreeMap<String, (Vec<CodingAgentEnv>, bool)> {
    settings
        .agents
        .iter()
        .map(|agent| (agent.id.clone(), (agent.envs.clone(), agent.isolated_home)))
        .collect()
}

/// #592/#597 - per-agent base command, the other half of the drift hash input
/// (`compose_effective_command(agent.command, cell.command)`). A change here must
/// trigger a drift refresh just like a profile-cell edit.
fn agent_commands_by_id(settings: &AppSettings) -> BTreeMap<String, String> {
    settings
        .agents
        .iter()
        .map(|agent| (agent.id.clone(), agent.command.clone()))
        .collect()
}

fn emit_settings_draft_update_events(app: &AppHandle, events: &SettingsDraftUpdateEvents) {
    if events.profiles_changed {
        // #592/#597: the SettingsModal "Save" persists profile cells AND agent base
        // commands through save_settings_draft (NOT update_coding_agent_profiles), so
        // THIS is the emit point the user's normal edit flow actually reaches. Kept at
        // debug to confirm the refresh fired without spamming app.log.
        log::debug!(
            "[profile-hash] profile-save (save_settings_draft): coding-agent config changed; emitting coding_agent_profiles_updated"
        );
        let _ = app.emit("coding_agent_profiles_updated", serde_json::json!({}));
    }

    for agent_id in &events.env_agent_ids {
        log::debug!(
            "[profile-hash] profile-save (save_settings_draft): agent env changed for {}; emitting coding_agent_env_settings_updated",
            agent_id
        );
        let _ = app.emit(
            "coding_agent_env_settings_updated",
            serde_json::json!({ "agentId": agent_id }),
        );
    }
}

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

async fn persist_coding_agent_profiles_update(
    settings: &SettingsState,
    profiles: CodingAgentProfilesConfig,
) -> Result<(), String> {
    let mut s = settings.write().await;
    let mut candidate = s.clone();
    candidate.coding_agent_profiles = profiles;
    validate_and_repair_settings(&mut candidate)?;
    save_settings(&candidate)?;
    *s = candidate;
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

async fn persist_coding_agent_env_settings_update(
    settings: &SettingsState,
    agent_id: &str,
    envs: Vec<CodingAgentEnv>,
    isolated_home: bool,
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
    save_settings(&candidate)?;
    *s = candidate;
    Ok(())
}

#[tauri::command]
pub async fn set_agent_default_profile(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    agent_path: String,
    profile: String,
) -> Result<(), String> {
    let snapshot = settings.read().await.clone();
    crate::config::coding_agent_profiles::set_agent_default_profile(
        &snapshot,
        std::path::Path::new(&agent_path),
        &profile,
    )?;
    let _ = app.emit(
        "coding_agent_profile_selection_updated",
        serde_json::json!({ "agentPath": agent_path, "profile": profile, "scope": "default" }),
    );
    Ok(())
}

#[tauri::command]
pub async fn set_instance_profile_override(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    agent_path: String,
    profile: Option<String>,
) -> Result<(), String> {
    let snapshot = settings.read().await.clone();
    crate::config::coding_agent_profiles::set_instance_profile_override(
        &snapshot,
        std::path::Path::new(&agent_path),
        profile.as_deref(),
    )?;
    let _ = app.emit(
        "coding_agent_profile_selection_updated",
        serde_json::json!({ "agentPath": agent_path, "profile": profile, "scope": "instance" }),
    );
    Ok(())
}

#[tauri::command]
pub async fn resolve_coding_agent_profile(
    settings: State<'_, SettingsState>,
    agent_path: Option<String>,
    agent_id: String,
    requested_profile: Option<String>,
) -> Result<CodingAgentProfileResolutionResult, String> {
    let snapshot = settings.read().await.clone();
    let agent_path = agent_path
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(std::path::PathBuf::from);
    let details = crate::config::coding_agent_profiles::resolve_profile_selection(
        &snapshot,
        agent_path.as_deref(),
        &agent_id,
        requested_profile.as_deref(),
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

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum ProfileAssignmentScope {
    Replica,
    Kind,
    Workgroup,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileAssignmentTarget {
    pub workgroup_name: String,
    pub workgroup_path: String,
    pub replica_name: String,
    pub replica_path: String,
    pub identity_path: String,
    pub origin_project: Option<String>,
    pub live_session_ids: Vec<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewCodingAgentProfileSelectionRequest {
    pub target_replica_path: String,
    pub coding_agent_id: String,
    pub profile: String,
    pub scope: ProfileAssignmentScope,
    #[serde(default)]
    pub restart_sessions: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewCodingAgentProfileSelectionResult {
    pub scope: ProfileAssignmentScope,
    pub target_count: usize,
    pub live_session_count: usize,
    pub target_fingerprint: String,
    pub requires_explicit_confirmation: bool,
    pub targets: Vec<ProfileAssignmentTarget>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyCodingAgentProfileSelectionRequest {
    pub target_replica_path: String,
    pub coding_agent_id: String,
    pub profile: String,
    pub scope: ProfileAssignmentScope,
    pub restart_sessions: bool,
    pub confirmed_target_fingerprint: Option<String>,
    pub typed_confirmation: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileAssignmentError {
    pub code: String,
    pub message: String,
    pub session_ids: Vec<String>,
    pub replica_paths: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyCodingAgentProfileSelectionResult {
    pub scope: ProfileAssignmentScope,
    pub updated_count: usize,
    pub restarted_count: usize,
    pub updated_replica_paths: Vec<String>,
    pub restarted_session_ids: Vec<String>,
    pub destroyed_but_not_recreated_session_ids: Vec<String>,
    pub target_fingerprint: String,
    pub warnings: Vec<String>,
    pub errors: Vec<ProfileAssignmentError>,
}

#[tauri::command]
pub async fn preview_coding_agent_profile_selection(
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    settings: State<'_, SettingsState>,
    request: PreviewCodingAgentProfileSelectionRequest,
) -> Result<PreviewCodingAgentProfileSelectionResult, String> {
    let settings_snapshot = settings.read().await.clone();
    validate_profile_assignment_request(
        &settings_snapshot,
        &request.coding_agent_id,
        &request.profile,
    )?;
    let sessions = { session_mgr.read().await.list_sessions().await };
    let enumeration = enumerate_profile_assignment_targets(
        &settings_snapshot,
        Path::new(&request.target_replica_path),
        &request.scope,
        &sessions,
    )?;
    let normalized_profile = normalize_profile_letter_for_assignment(&request.profile)?;
    let target_fingerprint = profile_assignment_fingerprint(
        &request.coding_agent_id,
        &normalized_profile,
        request.restart_sessions,
        &enumeration.canonical_target_paths,
    );
    let live_session_count = enumeration
        .targets
        .iter()
        .map(|target| target.live_session_ids.len())
        .sum();
    Ok(PreviewCodingAgentProfileSelectionResult {
        scope: request.scope.clone(),
        target_count: enumeration.targets.len(),
        live_session_count,
        target_fingerprint,
        requires_explicit_confirmation: profile_assignment_requires_explicit_confirmation(
            &request.scope,
        ),
        targets: enumeration.targets,
        warnings: enumeration.warnings,
    })
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn apply_coding_agent_profile_selection(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    pty_mgr: State<'_, Arc<std::sync::Mutex<PtyManager>>>,
    settings: State<'_, SettingsState>,
    request: ApplyCodingAgentProfileSelectionRequest,
) -> Result<ApplyCodingAgentProfileSelectionResult, String> {
    let apply_lock = broad_profile_apply_lock().lock().await;
    let settings_snapshot = settings.read().await.clone();
    validate_profile_assignment_request(
        &settings_snapshot,
        &request.coding_agent_id,
        &request.profile,
    )?;
    let normalized_profile = normalize_profile_letter_for_assignment(&request.profile)?;
    let sessions = { session_mgr.read().await.list_sessions().await };
    let enumeration = enumerate_profile_assignment_targets(
        &settings_snapshot,
        Path::new(&request.target_replica_path),
        &request.scope,
        &sessions,
    )?;
    let target_fingerprint = profile_assignment_fingerprint(
        &request.coding_agent_id,
        &normalized_profile,
        request.restart_sessions,
        &enumeration.canonical_target_paths,
    );
    validate_profile_assignment_confirmation(&request, &target_fingerprint)?;

    if request.restart_sessions {
        prevalidate_profile_assignment_restarts(
            &settings_snapshot,
            &request.coding_agent_id,
            &normalized_profile,
            &enumeration.targets,
        )?;
    }

    let mut updated_replica_paths = Vec::new();
    let mut write_succeeded_keys = BTreeSet::new();
    let mut errors = Vec::new();
    for target in &enumeration.targets {
        match crate::config::coding_agent_profiles::set_replica_coding_agent_selection(
            &settings_snapshot,
            Path::new(&target.replica_path),
            &request.coding_agent_id,
            &normalized_profile,
        ) {
            Ok(()) => {
                updated_replica_paths.push(target.replica_path.clone());
                write_succeeded_keys.insert(canonical_compare_key(Path::new(&target.replica_path)));
            }
            Err(e) => errors.push(ProfileAssignmentError {
                code: "configWriteFailed".to_string(),
                message: e,
                session_ids: target.live_session_ids.clone(),
                replica_paths: vec![target.replica_path.clone()],
            }),
        }
    }
    drop(apply_lock);

    let mut restarted_session_ids = Vec::new();
    let mut destroyed_but_not_recreated_session_ids = Vec::new();
    if request.restart_sessions && !write_succeeded_keys.is_empty() {
        for target in &enumeration.targets {
            if !write_succeeded_keys
                .contains(&canonical_compare_key(Path::new(&target.replica_path)))
            {
                continue;
            }
            for session_id in &target.live_session_ids {
                let Ok(uuid) = uuid::Uuid::parse_str(session_id) else {
                    continue;
                };
                match crate::commands::session::restart_session_inner_with_activation(
                    &app,
                    session_mgr.inner(),
                    pty_mgr.inner(),
                    settings.inner(),
                    uuid,
                    Some(request.coding_agent_id.clone()),
                    Some(normalized_profile.clone()),
                    Some(true),
                    false,
                )
                .await
                {
                    Ok(_) => restarted_session_ids.push(session_id.clone()),
                    Err(e) => {
                        let (code, destroyed_but_not_recreated) =
                            classify_restart_failure(session_mgr.inner(), uuid).await;
                        errors.push(ProfileAssignmentError {
                            code: code.to_string(),
                            message: e,
                            session_ids: vec![session_id.clone()],
                            replica_paths: vec![target.replica_path.clone()],
                        });
                        if destroyed_but_not_recreated {
                            destroyed_but_not_recreated_session_ids.push(session_id.clone());
                        }
                    }
                }
            }
        }
    }

    let result = ApplyCodingAgentProfileSelectionResult {
        scope: request.scope.clone(),
        updated_count: updated_replica_paths.len(),
        restarted_count: restarted_session_ids.len(),
        updated_replica_paths,
        restarted_session_ids,
        destroyed_but_not_recreated_session_ids,
        target_fingerprint,
        warnings: enumeration.warnings,
        errors,
    };
    let _ = app.emit(
        "coding_agent_profile_selection_updated",
        serde_json::json!({
            "scope": request.scope,
            "codingAgentId": request.coding_agent_id,
            "profile": normalized_profile,
            "updatedCount": result.updated_count,
            "restartedCount": result.restarted_count,
            "targetFingerprint": &result.target_fingerprint,
            "errors": &result.errors,
        }),
    );
    Ok(result)
}

struct ProfileTargetEnumeration {
    targets: Vec<ProfileAssignmentTarget>,
    canonical_target_paths: Vec<String>,
    warnings: Vec<String>,
}

fn broad_profile_apply_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn validate_profile_assignment_request(
    settings: &AppSettings,
    coding_agent_id: &str,
    profile: &str,
) -> Result<(), String> {
    if !settings
        .agents
        .iter()
        .any(|agent| agent.id == coding_agent_id)
    {
        return Err(format!("Agent '{}' is not configured", coding_agent_id));
    }
    normalize_profile_letter_for_assignment(profile)?;
    Ok(())
}

fn normalize_profile_letter_for_assignment(profile: &str) -> Result<String, String> {
    crate::config::settings::normalize_profile_letter(profile)
        .ok_or_else(|| "Profile must be a single letter A through Z".to_string())
}

fn profile_assignment_requires_explicit_confirmation(scope: &ProfileAssignmentScope) -> bool {
    *scope != ProfileAssignmentScope::Replica
}

fn validate_profile_assignment_confirmation(
    request: &ApplyCodingAgentProfileSelectionRequest,
    fingerprint: &str,
) -> Result<(), String> {
    if request.scope != ProfileAssignmentScope::Replica {
        match request.confirmed_target_fingerprint.as_deref() {
            Some(value) if value == fingerprint => {}
            _ => {
                return Err(
                    "Target selection changed. Rerun preview before applying profile selection."
                        .to_string(),
                )
            }
        }
    } else if let Some(value) = request.confirmed_target_fingerprint.as_deref() {
        if value != fingerprint {
            return Err(
                "Target selection changed. Rerun preview before applying profile selection."
                    .to_string(),
            );
        }
    }
    Ok(())
}

fn prevalidate_profile_assignment_restarts(
    settings: &AppSettings,
    coding_agent_id: &str,
    profile: &str,
    targets: &[ProfileAssignmentTarget],
) -> Result<(), String> {
    for target in targets {
        if target.live_session_ids.is_empty() {
            continue;
        }
        crate::config::agent_command::build_agent_spawn_command(
            settings,
            coding_agent_id,
            Some(Path::new(&target.replica_path)),
            Some(profile),
        )?;
    }
    Ok(())
}

async fn classify_restart_failure(
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    uuid: uuid::Uuid,
) -> (&'static str, bool) {
    let session_survived = {
        let mgr = session_mgr.read().await;
        mgr.get_session(uuid).await.is_some()
    };
    if session_survived {
        ("restartFailed", false)
    } else {
        ("destroyedButNotRecreated", true)
    }
}

fn enumerate_profile_assignment_targets(
    settings: &AppSettings,
    target_replica_path: &Path,
    scope: &ProfileAssignmentScope,
    sessions: &[SessionInfo],
) -> Result<ProfileTargetEnumeration, String> {
    let target_replica = canonical_real_dir(target_replica_path, "target replica")?;
    validate_wg_replica_path(&target_replica)?;
    let (_target_config, target_identity) =
        crate::config::replica_identity::read_wg_replica_config_read_only(&target_replica)?;
    let mut warnings = Vec::new();
    let mut candidate_dirs = Vec::new();
    match scope {
        ProfileAssignmentScope::Replica => candidate_dirs.push(target_replica.clone()),
        ProfileAssignmentScope::Workgroup => {
            let wg_dir = target_replica
                .parent()
                .ok_or_else(|| "Target replica has no workgroup parent".to_string())?;
            collect_replica_dirs_in_workgroup(wg_dir, &mut candidate_dirs)?;
        }
        ProfileAssignmentScope::Kind => {
            for workspace in
                crate::config::coding_agent_profiles::configured_workspace_dirs(settings)
            {
                collect_kind_replica_dirs(&workspace, &mut candidate_dirs)?;
            }
        }
    }

    let live_by_cwd = live_sessions_by_cwd(sessions);
    let mut seen = BTreeSet::new();
    let mut targets = Vec::new();
    for candidate in candidate_dirs {
        let Ok(replica_dir) = canonical_real_dir(&candidate, "candidate replica") else {
            warnings.push(format!(
                "Skipping unreadable replica '{}'",
                candidate.display()
            ));
            continue;
        };
        let key = canonical_compare_key(&replica_dir);
        if !seen.insert(key) {
            continue;
        }
        let Ok((_, identity)) =
            crate::config::replica_identity::read_wg_replica_config_read_only(&replica_dir)
        else {
            warnings.push(format!(
                "Skipping invalid replica '{}'",
                replica_dir.display()
            ));
            continue;
        };
        if *scope == ProfileAssignmentScope::Kind
            && canonical_compare_key(&identity.matrix_dir)
                != canonical_compare_key(&target_identity.matrix_dir)
        {
            continue;
        }
        let target = build_profile_assignment_target(&replica_dir, &identity, &live_by_cwd);
        targets.push(target);
    }
    targets.sort_by(|a, b| {
        a.workgroup_name
            .cmp(&b.workgroup_name)
            .then_with(|| a.replica_name.cmp(&b.replica_name))
            .then_with(|| a.replica_path.cmp(&b.replica_path))
    });
    let canonical_target_paths = targets
        .iter()
        .map(|target| canonical_compare_key(Path::new(&target.replica_path)))
        .collect();
    Ok(ProfileTargetEnumeration {
        targets,
        canonical_target_paths,
        warnings,
    })
}

pub(crate) fn canonical_compare_key(path: &Path) -> String {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut text = path
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('\\', "/");
    if cfg!(windows) {
        text = text.to_ascii_lowercase();
    }
    text
}

fn canonical_real_dir(path: &Path, label: &str) -> Result<PathBuf, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|e| format!("{} '{}' is not readable: {}", label, path.display(), e))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!(
            "{} '{}' is not a real directory",
            label,
            path.display()
        ));
    }
    std::fs::canonicalize(path)
        .map(|path| {
            let text = path.to_string_lossy();
            text.strip_prefix(r"\\?\")
                .map(PathBuf::from)
                .unwrap_or(path)
        })
        .map_err(|e| {
            format!(
                "Failed to canonicalize {} '{}': {}",
                label,
                path.display(),
                e
            )
        })
}

fn validate_wg_replica_path(path: &Path) -> Result<(), String> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if !name.starts_with("__agent_") {
        return Err(format!("Target '{}' is not a WG replica", path.display()));
    }
    let wg_name = path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if !wg_name.starts_with("wg-") {
        return Err(format!(
            "Target '{}' is not inside a wg-* workgroup",
            path.display()
        ));
    }
    Ok(())
}

fn collect_replica_dirs_in_workgroup(wg_dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(wg_dir)
        .map_err(|e| format!("Failed to read workgroup '{}': {}", wg_dir.display(), e))?;
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let name = entry.file_name();
        if name.to_string_lossy().starts_with("__agent_") {
            out.push(entry.path());
        }
    }
    Ok(())
}

fn collect_kind_replica_dirs(workspace: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(workspace)
        .map_err(|e| format!("Failed to read workspace '{}': {}", workspace.display(), e))?;
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() || !entry.file_name().to_string_lossy().starts_with("wg-") {
            continue;
        }
        collect_replica_dirs_in_workgroup(&entry.path(), out)?;
    }
    Ok(())
}

fn live_sessions_by_cwd(sessions: &[SessionInfo]) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for session in sessions {
        if matches!(
            session.status,
            crate::session::session::SessionStatus::Exited(_)
        ) {
            continue;
        }
        out.entry(canonical_compare_key(Path::new(&session.working_directory)))
            .or_default()
            .push(session.id.clone());
    }
    out
}

fn build_profile_assignment_target(
    replica_dir: &Path,
    identity: &crate::config::replica_identity::WgReplicaIdentity,
    live_by_cwd: &BTreeMap<String, Vec<String>>,
) -> ProfileAssignmentTarget {
    let wg_dir = replica_dir.parent().unwrap_or_else(|| Path::new(""));
    let workgroup_name = wg_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_string();
    let replica_name = replica_dir
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("__agent_"))
        .unwrap_or("")
        .to_string();
    let origin_project = identity
        .workspace_dir
        .parent()
        .and_then(|project| project.file_name())
        .and_then(|name| name.to_str())
        .map(str::to_string);
    ProfileAssignmentTarget {
        workgroup_name,
        workgroup_path: wg_dir.to_string_lossy().to_string(),
        replica_name,
        replica_path: replica_dir.to_string_lossy().to_string(),
        identity_path: identity.identity.clone(),
        origin_project,
        live_session_ids: live_by_cwd
            .get(&canonical_compare_key(replica_dir))
            .cloned()
            .unwrap_or_default(),
    }
}

fn profile_assignment_fingerprint(
    coding_agent_id: &str,
    profile: &str,
    restart_sessions: bool,
    canonical_target_paths: &[String],
) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    coding_agent_id.hash(&mut hasher);
    profile.hash(&mut hasher);
    restart_sessions.hash(&mut hasher);
    canonical_target_paths.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[tauri::command]
pub async fn open_web_remote() -> Result<(), String> {
    let settings = load_settings();
    if !settings.web_server_enabled {
        return Err("Web server is not enabled".into());
    }

    let token_path = crate::config::config_dir()
        .ok_or("No config dir")?
        .join("web-token.txt");

    let token = std::fs::read_to_string(&token_path)
        .map_err(|e| format!("Cannot read web token: {}", e))?;

    let url = format!(
        "http://{}:{}/?window=browser&remoteToken={}",
        settings.web_server_bind,
        settings.web_server_port,
        token.trim()
    );

    open::that(&url).map_err(|e| format!("Failed to open browser: {}", e))?;
    Ok(())
}

// Tauri command: State<> injections push us over clippy's 7-arg threshold.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn start_web_server(
    app_handle: tauri::AppHandle,
    ws_handle: State<'_, WebServerHandle>,
    settings: State<'_, SettingsState>,
    web_token: State<'_, Arc<WebAccessToken>>,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    pty_mgr: State<'_, Arc<std::sync::Mutex<PtyManager>>>,
    broadcaster: State<'_, WsBroadcaster>,
    shutdown: State<'_, crate::shutdown::ShutdownSignal>,
) -> Result<bool, String> {
    let s = settings.read().await;
    let bind = s.web_server_bind.clone();
    let port = s.web_server_port;
    drop(s);

    // Check if already listening
    let addr = format!("{}:{}", bind, port);
    if is_tcp_listening(&addr).await {
        return Ok(false); // already running
    }

    let join_handle = crate::web::start_server(
        bind,
        port,
        Arc::clone(&web_token),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
        Arc::clone(&settings),
        (*broadcaster).clone(),
        app_handle,
        shutdown.inner().clone(),
    );

    *ws_handle.lock().unwrap() = Some(join_handle);
    log::info!("[web-server] Started via command");
    Ok(true)
}

#[tauri::command]
pub async fn stop_web_server(ws_handle: State<'_, WebServerHandle>) -> Result<bool, String> {
    let mut guard = ws_handle.lock().unwrap();
    if let Some(handle) = guard.take() {
        handle.abort();
        log::info!("[web-server] Stopped via command");
        Ok(true)
    } else {
        Ok(false)
    }
}

#[tauri::command]
pub async fn get_web_server_status(settings: State<'_, SettingsState>) -> Result<bool, String> {
    let s = settings.read().await;
    let addr = format!("{}:{}", s.web_server_bind, s.web_server_port);
    drop(s);
    Ok(is_tcp_listening(&addr).await)
}

async fn is_tcp_listening(addr: &str) -> bool {
    matches!(
        tokio::time::timeout(
            Duration::from_millis(WEB_STATUS_CONNECT_TIMEOUT_MS),
            tokio::net::TcpStream::connect(addr),
        )
        .await,
        Ok(Ok(_))
    )
}

/// Returns the runtime instance label for the titlebar badge.
/// E.g. "STAGE", "STANDALONE", or "" for prod (no badge).
#[tauri::command]
pub fn get_instance_label() -> String {
    crate::config::profile::instance_label().to_string()
}

async fn persist_narrow_settings_update(
    settings: &SettingsState,
    mutate_candidate: impl FnOnce(&mut AppSettings),
) -> Result<(), String> {
    persist_narrow_settings_update_with_saver(settings, mutate_candidate, save_settings).await
}

async fn persist_narrow_settings_update_with_saver(
    settings: &SettingsState,
    mutate_candidate: impl FnOnce(&mut AppSettings),
    save: impl FnOnce(&AppSettings) -> Result<(), String>,
) -> Result<(), String> {
    let mut s = settings.write().await;
    let mut candidate = s.clone();
    mutate_candidate(&mut candidate);
    save(&candidate)?;
    *s = candidate;
    Ok(())
}

/// Narrow setter for `inject_rtk_hook`. Holds the SettingsState
/// write lock through `save_settings` and publishes the candidate only after
/// the disk write succeeds (issue #120, grinch H3 + N1).
/// Broad settings updates now use the same write-lock-through-save pattern, so
/// stale full-object payloads cannot interleave with this setter and overwrite
/// the live value after this save publishes.
///
/// Caller is responsible for triggering `sweep_rtk_hook` if disk side-effects
/// on replicas are desired.
#[tauri::command]
pub async fn set_inject_rtk_hook(
    settings: State<'_, SettingsState>,
    value: bool,
) -> Result<(), String> {
    persist_narrow_settings_update(settings.inner(), |candidate| {
        candidate.inject_rtk_hook = value;
    })
    .await
}

/// Narrow setter for `rtk_prompt_dismissed`. Same candidate-save-publish
/// pattern as `set_inject_rtk_hook` (issue #120, grinch H3 + N1).
#[tauri::command]
pub async fn set_rtk_prompt_dismissed(
    settings: State<'_, SettingsState>,
    value: bool,
) -> Result<(), String> {
    persist_narrow_settings_update(settings.inner(), |candidate| {
        candidate.rtk_prompt_dismissed = value;
    })
    .await
}

/// Narrow setter for `sounds_enabled`. Same candidate-save-publish
/// pattern as `set_inject_rtk_hook` (issue #158). Replaces the toolbar's
/// previous full-object `update_settings(next)` call, which could clobber
/// unrelated fields from a stale `settingsStore.current` snapshot.
#[tauri::command]
pub async fn set_sounds_enabled(
    settings: State<'_, SettingsState>,
    value: bool,
) -> Result<(), String> {
    persist_narrow_settings_update(settings.inner(), |candidate| {
        candidate.sounds_enabled = value;
    })
    .await
}

/// Narrow setter for `theme_light`. Same candidate-save-publish
/// pattern as `set_sounds_enabled` (issue #289). Lets the UI persist the
/// user's light/dark mode choice without going through `update_settings`,
/// which could clobber unrelated fields from a stale snapshot.
#[tauri::command]
pub async fn set_theme_light(
    settings: State<'_, SettingsState>,
    value: bool,
) -> Result<(), String> {
    persist_narrow_settings_update(settings.inner(), |candidate| {
        candidate.theme_light = value;
    })
    .await
}

/// Narrow setter for `main_resource_monitor_attached` (#587). Same
/// candidate-save-publish pattern as `set_theme_light`; lets the central-view
/// toggle persist without a get+update round-trip racing SettingsModal.
#[tauri::command]
pub async fn set_main_resource_monitor_attached(
    settings: State<'_, SettingsState>,
    value: bool,
) -> Result<(), String> {
    persist_narrow_settings_update(settings.inner(), |candidate| {
        candidate.main_resource_monitor_attached = value;
    })
    .await
}

/// #612 apply the runtime log level (no-op under RUST_LOG) and broadcast so
/// every webview re-applies its own console gate. Single point that both the
/// dedicated `set_log_level` command and the SettingsModal Save path reuse.
fn apply_and_broadcast_log_level(app: &AppHandle, level: &str) {
    // set_runtime_log_level returns false when the gate is not installed
    // (RUST_LOG dev-override active): the backend stays frozen and only the
    // frontend console gate moves. Emit one debug line so an operator reading
    // app.log isn't confused why the backend verbosity didn't change.
    if !crate::logging::set_runtime_log_level(level) {
        log::debug!(
            "[log-level] RUST_LOG override active; backend frozen, applying '{level}' to frontend only"
        );
    }
    let _ = app.emit("log_level_changed", serde_json::json!({ "level": level }));
}

/// #612 canonical command to change ONLY the log level: validates, persists
/// `logLevel`, applies live, broadcasts. Used by programmatic callers / future
/// quick-switch; the SettingsModal uses the form Save path (`save_settings_draft`).
#[tauri::command]
pub async fn set_log_level(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    level: String,
) -> Result<(), String> {
    let normalized = crate::logging::normalize_log_level(&level)
        .ok_or_else(|| format!("Invalid log level: {level}"))?;
    persist_narrow_settings_update(settings.inner(), |candidate| {
        candidate.log_level = Some(normalized.to_string());
    })
    .await?;
    apply_and_broadcast_log_level(&app, normalized);
    Ok(())
}

/// Sweep every AC-managed agent directory and apply
/// `ensure_rtk_pretool_hook(dir, enabled)`. Best-effort per directory:
/// per-dir failures are logged + appended to `errors` and the sweep
/// continues. Reads `project_paths` from the live `SettingsState` (avoids a
/// disk-read race against `save_settings`).
///
/// Acquires `RtkSweepLockState` for the entire loop — eliminates the
/// in-process race vs. concurrent `ensure_rtk_pretool_hook` calls from
/// `entity_creation` (issue #120, grinch M8). Cross-process races (two AC
/// instances) remain documented in the plan §7.4.
#[tauri::command]
pub async fn sweep_rtk_hook(
    settings: State<'_, SettingsState>,
    sweep_lock: State<'_, RtkSweepLockState>,
    enabled: bool,
) -> Result<RtkSweepResult, String> {
    let _guard = sweep_lock.lock().await;

    let project_paths: Vec<String> = {
        let s = settings.read().await;
        s.project_paths.clone()
    };

    let dirs = enumerate_managed_agent_dirs(&project_paths);
    let total = dirs.len() as u32;
    let mut succeeded: u32 = 0;
    let mut errors: Vec<RtkSweepError> = Vec::new();

    for dir in dirs {
        match ensure_rtk_pretool_hook(&dir, enabled) {
            Ok(()) => {
                succeeded += 1;
            }
            Err(e) => {
                log::warn!(
                    "[rtk-sweep] Failed to apply (enabled={}) to {}: {}",
                    enabled,
                    dir.display(),
                    e
                );
                errors.push(RtkSweepError {
                    path: dir.to_string_lossy().to_string(),
                    error: e,
                });
            }
        }
    }

    log::info!(
        "[rtk-sweep] enabled={} total={} succeeded={} errors={}",
        enabled,
        total,
        succeeded,
        errors.len()
    );

    Ok(RtkSweepResult {
        total,
        succeeded,
        errors,
    })
}

/// Returns the BOOT-TIME RTK startup decision computed by the setup task in
/// `lib.rs::run` and cached in `RtkStartupModeState`. This is the SAME value
/// the setup task emitted via `rtk_startup_status` — so the listener and the
/// getter always agree, even after the auto-disable side-effect mutates
/// settings (issue #120 §18 amendment).
///
/// If called before the setup task has finished (extremely narrow boot
/// window — `which::which` resolve + a state read), returns "silent". The
/// listener will fire shortly after with the actual mode; combined with
/// idempotent `setMode` on the frontend, the banner self-corrects.
///
/// Pure read — does NOT auto-disable, does NOT sweep, does NOT probe PATH.
#[tauri::command]
pub async fn get_rtk_startup_status(
    mode_cache: State<'_, RtkStartupModeState>,
) -> Result<String, String> {
    Ok(mode_cache
        .get()
        .cloned()
        .unwrap_or_else(|| "silent".to_string()))
}

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

/// Fetch the Home screen Markdown source from the public docs URL.
/// Returns the raw Markdown body as a String.
/// Errors are returned as user-facing strings; the frontend renders them in
/// the Home view's error state.
#[tauri::command]
pub async fn fetch_home_markdown(network: State<'_, OutboundNetwork>) -> Result<String, String> {
    let _permit = network.acquire("docs.fetch_home_markdown").await?;
    let resp = tokio::time::timeout(
        Duration::from_secs(HOME_MARKDOWN_TIMEOUT_SECS),
        network
            .general()
            .get(HOME_MARKDOWN_URL)
            .header(
                reqwest::header::USER_AGENT,
                concat!("agentscommander/", env!("CARGO_PKG_VERSION")),
            )
            .send(),
    )
    .await
    .map_err(|_| "Network error: request timed out".to_string())?
    .map_err(|e| format!("Network error: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("Server returned status {}", resp.status().as_u16()));
    }

    // Use bytes() so we can length-check before allocating a String.
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("Failed to read response: {}", e))?;

    if bytes.is_empty() {
        return Err("Server returned empty response".to_string());
    }
    if bytes.len() > HOME_MARKDOWN_MAX_BYTES {
        return Err("Response too large".to_string());
    }

    // Strip a leading UTF-8 BOM if present so it does not render as an
    // invisible character at the top of the document (grinch §M, optional).
    let trimmed: &[u8] = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes);

    String::from_utf8(trimmed.to_vec()).map_err(|_| "Response is not valid UTF-8".to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        persist_coding_agent_env_settings_update, persist_coding_agent_profiles_update,
        persist_narrow_settings_update_with_saver, persist_protected_settings_update_with_saver,
        persist_settings_draft_update_with_saver, RtkSweepError, RtkSweepResult,
    };
    use crate::config::settings::{
        AgentConfig, AppSettings, CodingAgentEnv, CodingAgentEnvSource, ProfileCellConfig,
        SettingsState,
    };
    use crate::session::manager::SessionManager;
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    fn settings_with_single_agent() -> AppSettings {
        AppSettings {
            agents: vec![AgentConfig {
                id: "agent-0".to_string(),
                label: "Codex".to_string(),
                command: "codex".to_string(),
                color: "#000000".to_string(),
                envs: Vec::new(),
                isolated_home: false,
                instructions_filename: None,
                config_seed: None,
            }],
            ..AppSettings::default()
        }
    }

    fn state_for(settings: AppSettings) -> SettingsState {
        Arc::new(RwLock::new(settings))
    }

    fn profile_assignment_request(
        scope: super::ProfileAssignmentScope,
        confirmed_target_fingerprint: Option<&str>,
        typed_confirmation: Option<&str>,
    ) -> super::ApplyCodingAgentProfileSelectionRequest {
        super::ApplyCodingAgentProfileSelectionRequest {
            target_replica_path: "C:/ac/wg-1/__agent_dev-rust".to_string(),
            coding_agent_id: "dev-rust".to_string(),
            profile: "B".to_string(),
            scope,
            restart_sessions: false,
            confirmed_target_fingerprint: confirmed_target_fingerprint
                .map(|value| value.to_string()),
            typed_confirmation: typed_confirmation.map(|value| value.to_string()),
        }
    }

    #[test]
    fn profile_assignment_fingerprint_uses_typed_fields() {
        let targets = vec!["c:/ac/wg-1/__agent_a".to_string()];

        let ab_c = super::profile_assignment_fingerprint("ab", "C", false, &targets);
        let a_bc = super::profile_assignment_fingerprint("a", "BC", false, &targets);
        let restart = super::profile_assignment_fingerprint("ab", "C", true, &targets);
        let other_targets = super::profile_assignment_fingerprint(
            "ab",
            "C",
            false,
            &["c:/ac/wg-1/__agent_b".to_string()],
        );

        assert_ne!(ab_c, a_bc);
        assert_ne!(ab_c, restart);
        assert_ne!(ab_c, other_targets);
    }

    #[test]
    fn profile_assignment_confirmation_hint_tracks_broad_scopes() {
        assert!(!super::profile_assignment_requires_explicit_confirmation(
            &super::ProfileAssignmentScope::Replica
        ));
        assert!(super::profile_assignment_requires_explicit_confirmation(
            &super::ProfileAssignmentScope::Workgroup
        ));
        assert!(super::profile_assignment_requires_explicit_confirmation(
            &super::ProfileAssignmentScope::Kind
        ));
    }

    #[test]
    fn kind_assignment_accepts_fingerprint_without_typed_confirmation() {
        let request =
            profile_assignment_request(super::ProfileAssignmentScope::Kind, Some("fp-kind"), None);

        assert_eq!(
            super::validate_profile_assignment_confirmation(&request, "fp-kind"),
            Ok(())
        );
    }

    #[test]
    fn kind_assignment_ignores_legacy_typed_confirmation() {
        let request = profile_assignment_request(
            super::ProfileAssignmentScope::Kind,
            Some("fp-kind"),
            Some("not the old exact phrase"),
        );

        assert_eq!(
            super::validate_profile_assignment_confirmation(&request, "fp-kind"),
            Ok(())
        );
    }

    #[test]
    fn broad_assignment_rejects_missing_or_stale_fingerprint() {
        for scope in [
            super::ProfileAssignmentScope::Kind,
            super::ProfileAssignmentScope::Workgroup,
        ] {
            let missing = profile_assignment_request(scope.clone(), None, None);
            let err = super::validate_profile_assignment_confirmation(&missing, "current-fp")
                .unwrap_err();
            assert!(err.contains("Target selection changed"), "{err}");

            let stale = profile_assignment_request(scope, Some("old-fp"), None);
            let err =
                super::validate_profile_assignment_confirmation(&stale, "current-fp").unwrap_err();
            assert!(err.contains("Target selection changed"), "{err}");
        }
    }

    #[test]
    fn settings_draft_events_flag_agent_base_command_edit() {
        // #592/#597 - a bare agent base-command edit changes the effective-command
        // drift hash, so save_settings_draft must emit coding_agent_profiles_updated
        // even though the profile cells and envs are untouched. This is the bug the
        // user hit editing `claude` -> `claude-amp -c ...` in Settings -> Coding Agents.
        let before = settings_with_single_agent();
        let mut after = settings_with_single_agent();
        after.agents[0].command = "codex --resume".to_string();

        let events = super::settings_draft_update_events(&before, &after);
        assert!(
            events.profiles_changed,
            "an agent base-command edit must flag a profiles refresh"
        );
        assert!(events.env_agent_ids.is_empty(), "no env changed");
    }

    #[test]
    fn settings_draft_events_quiet_for_non_drift_field_edit() {
        // A non-drift field (label) changing must NOT emit a profiles refresh, so the
        // fix does not over-fire on unrelated settings saves.
        let before = settings_with_single_agent();
        let mut after = settings_with_single_agent();
        after.agents[0].label = "Renamed".to_string();

        let events = super::settings_draft_update_events(&before, &after);
        assert!(!events.profiles_changed, "a label edit is not drift");
        assert!(events.env_agent_ids.is_empty());
    }

    #[test]
    fn settings_draft_events_flag_agent_env_edit() {
        // Regression guard: the pre-existing env path still flags the agent id (the
        // base env is also a drift-hash input, surfaced via the env event).
        let before = settings_with_single_agent();
        let mut after = settings_with_single_agent();
        after.agents[0].envs = vec![CodingAgentEnv {
            key: "FOO".to_string(),
            value: "bar".to_string(),
            source: CodingAgentEnvSource::User,
            enabled: true,
        }];

        let events = super::settings_draft_update_events(&before, &after);
        assert_eq!(events.env_agent_ids, vec!["agent-0".to_string()]);
        assert!(!events.profiles_changed, "env-only edit is not a profiles change");
    }

    #[tokio::test]
    async fn restart_failure_classification_depends_on_session_survival() {
        let manager = SessionManager::new();
        let session = manager
            .create_session(
                "codex".to_string(),
                Vec::new(),
                "C:/work/repo".to_string(),
                Some("codex".to_string()),
                Some("Codex".to_string()),
                Vec::new(),
                false,
            )
            .await
            .unwrap();
        let session_mgr = Arc::new(RwLock::new(manager));

        let (survived_code, survived_destroyed) =
            super::classify_restart_failure(&session_mgr, session.id).await;
        assert_eq!(survived_code, "restartFailed");
        assert!(!survived_destroyed);

        let (missing_code, missing_destroyed) =
            super::classify_restart_failure(&session_mgr, uuid::Uuid::new_v4()).await;
        assert_eq!(missing_code, "destroyedButNotRecreated");
        assert!(missing_destroyed);
    }

    #[tokio::test]
    async fn invalid_env_settings_update_leaves_live_settings_unchanged() {
        let mut original = settings_with_single_agent();
        original.agents[0].envs = vec![CodingAgentEnv {
            key: "OPENAI_API_BASE".to_string(),
            value: "current".to_string(),
            source: CodingAgentEnvSource::User,
            enabled: true,
        }];
        original.agents[0].isolated_home = true;
        let state = state_for(original.clone());

        let err = persist_coding_agent_env_settings_update(
            &state,
            "agent-0",
            vec![CodingAgentEnv {
                key: "AGENTSCOMMANDER_TOKEN".to_string(),
                value: "bad".to_string(),
                source: CodingAgentEnvSource::User,
                enabled: true,
            }],
            false,
        )
        .await
        .unwrap_err();

        assert!(err.contains("reserved by AgentsCommander"), "{err}");
        let live = state.read().await;
        assert_eq!(live.agents[0].envs, original.agents[0].envs);
        assert_eq!(
            live.agents[0].isolated_home,
            original.agents[0].isolated_home
        );
    }

    #[tokio::test]
    async fn invalid_profile_settings_update_leaves_live_settings_unchanged() {
        let original = settings_with_single_agent();
        let mut invalid_profiles = original.coding_agent_profiles.clone();
        invalid_profiles
            .profiles_by_agent
            .entry("agent-0".to_string())
            .or_default()
            .insert(
                "A".to_string(),
                ProfileCellConfig {
                    enabled: true,
                    command: String::new(),
                    env: BTreeMap::from([("AGENTSCOMMANDER_ROOT".to_string(), "bad".to_string())]),
                    notes: String::new(),
                },
            );
        let state = state_for(original.clone());

        let err = persist_coding_agent_profiles_update(&state, invalid_profiles)
            .await
            .unwrap_err();

        assert!(err.contains("reserved by AgentsCommander"), "{err}");
        let live = state.read().await;
        assert_eq!(live.coding_agent_profiles, original.coding_agent_profiles);
    }

    #[tokio::test]
    async fn invalid_settings_draft_transaction_leaves_generic_profiles_and_env_unchanged() {
        let mut original = settings_with_single_agent();
        original.sidebar_style = "noir-minimal".to_string();
        original.agents[0].envs = vec![CodingAgentEnv {
            key: "OPENAI_API_BASE".to_string(),
            value: "current".to_string(),
            source: CodingAgentEnvSource::User,
            enabled: true,
        }];
        original.agents[0].isolated_home = true;
        original
            .coding_agent_profiles
            .profiles_by_agent
            .entry("agent-0".to_string())
            .or_default()
            .insert(
                "B".to_string(),
                ProfileCellConfig {
                    enabled: true,
                    command: "codex --current-profile".to_string(),
                    env: BTreeMap::new(),
                    notes: String::new(),
                },
            );
        let state = state_for(original.clone());

        let mut draft = original.clone();
        draft.sidebar_style = "command-center".to_string();
        draft.agents[0].envs = vec![CodingAgentEnv {
            key: "OPENAI_API_BASE".to_string(),
            value: "draft".to_string(),
            source: CodingAgentEnvSource::User,
            enabled: true,
        }];
        draft.agents[0].isolated_home = false;
        draft
            .coding_agent_profiles
            .profiles_by_agent
            .entry("agent-0".to_string())
            .or_default()
            .insert(
                "C".to_string(),
                ProfileCellConfig {
                    enabled: true,
                    command: "codex --draft-profile".to_string(),
                    env: BTreeMap::from([("AGENTSCOMMANDER_TOKEN".to_string(), "bad".to_string())]),
                    notes: String::new(),
                },
            );

        let err =
            persist_settings_draft_update_with_saver(&state, draft, |_| -> Result<(), String> {
                panic!("save must not run")
            })
            .await
            .unwrap_err();

        assert!(err.contains("reserved by AgentsCommander"), "{err}");
        let live = state.read().await;
        assert_eq!(live.sidebar_style, original.sidebar_style);
        assert_eq!(live.coding_agent_profiles, original.coding_agent_profiles);
        assert_eq!(live.agents[0].envs, original.agents[0].envs);
        assert_eq!(
            live.agents[0].isolated_home,
            original.agents[0].isolated_home
        );
    }

    #[tokio::test]
    async fn settings_draft_transaction_save_failure_leaves_live_settings_unchanged() {
        let mut original = settings_with_single_agent();
        original.agents[0].envs = vec![CodingAgentEnv {
            key: "OPENAI_API_BASE".to_string(),
            value: "current".to_string(),
            source: CodingAgentEnvSource::User,
            enabled: true,
        }];
        original.agents[0].isolated_home = true;
        let state = state_for(original.clone());

        let mut draft = original.clone();
        draft.sidebar_style = "command-center".to_string();
        draft.agents[0].envs = vec![CodingAgentEnv {
            key: "OPENAI_API_BASE".to_string(),
            value: "draft".to_string(),
            source: CodingAgentEnvSource::User,
            enabled: true,
        }];
        draft.agents[0].isolated_home = false;

        let err = persist_settings_draft_update_with_saver(&state, draft, |_| {
            Err("simulated save failure".to_string())
        })
        .await
        .unwrap_err();

        assert_eq!(err, "simulated save failure");
        let live = state.read().await;
        assert_eq!(live.sidebar_style, original.sidebar_style);
        assert_eq!(live.coding_agent_profiles, original.coding_agent_profiles);
        assert_eq!(live.agents[0].envs, original.agents[0].envs);
        assert_eq!(
            live.agents[0].isolated_home,
            original.agents[0].isolated_home
        );
    }

    async fn assert_narrow_setter_save_failure_rolls_back(
        field_name: &'static str,
        mutate_candidate: impl FnOnce(&mut AppSettings) + Send,
        assert_candidate: impl FnOnce(&AppSettings) + Send,
    ) {
        let mut original = settings_with_single_agent();
        original.inject_rtk_hook = false;
        original.rtk_prompt_dismissed = false;
        original.sounds_enabled = true;
        original.theme_light = false;
        let state = state_for(original.clone());
        let expected_err = format!("simulated save failure for {field_name}");

        let err = persist_narrow_settings_update_with_saver(
            &state,
            |candidate| {
                mutate_candidate(candidate);
                assert_candidate(candidate);
            },
            |_| Err(expected_err.clone()),
        )
        .await
        .unwrap_err();

        assert_eq!(err, expected_err);
        let live = state.read().await;
        assert_eq!(
            live.inject_rtk_hook, original.inject_rtk_hook,
            "{field_name}"
        );
        assert_eq!(
            live.rtk_prompt_dismissed, original.rtk_prompt_dismissed,
            "{field_name}"
        );
        assert_eq!(live.sounds_enabled, original.sounds_enabled, "{field_name}");
        assert_eq!(live.theme_light, original.theme_light, "{field_name}");
    }

    #[tokio::test]
    async fn narrow_settings_setter_save_failure_leaves_live_settings_unchanged() {
        assert_narrow_setter_save_failure_rolls_back(
            "inject_rtk_hook",
            |candidate| candidate.inject_rtk_hook = true,
            |candidate| assert!(candidate.inject_rtk_hook),
        )
        .await;
        assert_narrow_setter_save_failure_rolls_back(
            "rtk_prompt_dismissed",
            |candidate| candidate.rtk_prompt_dismissed = true,
            |candidate| assert!(candidate.rtk_prompt_dismissed),
        )
        .await;
        assert_narrow_setter_save_failure_rolls_back(
            "sounds_enabled",
            |candidate| candidate.sounds_enabled = false,
            |candidate| assert!(!candidate.sounds_enabled),
        )
        .await;
        assert_narrow_setter_save_failure_rolls_back(
            "theme_light",
            |candidate| candidate.theme_light = true,
            |candidate| assert!(candidate.theme_light),
        )
        .await;
    }

    #[tokio::test]
    async fn protected_update_settings_transaction_preserves_current_coding_fields() {
        let mut current = settings_with_single_agent();
        current.agents[0].envs = vec![CodingAgentEnv {
            key: "OPENAI_API_BASE".to_string(),
            value: "current".to_string(),
            source: CodingAgentEnvSource::User,
            enabled: true,
        }];
        current.agents[0].isolated_home = true;
        current
            .coding_agent_profiles
            .profiles_by_agent
            .entry("agent-0".to_string())
            .or_default()
            .insert(
                "B".to_string(),
                ProfileCellConfig {
                    enabled: true,
                    command: "codex --current-profile".to_string(),
                    env: BTreeMap::new(),
                    notes: String::new(),
                },
            );
        let state = state_for(current.clone());

        let mut stale = current.clone();
        stale.sidebar_style = "command-center".to_string();
        stale.agents[0].envs.clear();
        stale.agents[0].isolated_home = false;
        stale.coding_agent_profiles.profiles_by_agent.clear();

        let saved = persist_protected_settings_update_with_saver(&state, stale, |_| Ok(()))
            .await
            .unwrap();

        assert_eq!(saved.sidebar_style, "command-center");
        assert_eq!(saved.agents[0].envs, current.agents[0].envs);
        assert_eq!(
            saved.agents[0].isolated_home,
            current.agents[0].isolated_home
        );
        assert!(saved
            .coding_agent_profiles
            .profiles_by_agent
            .get("agent-0")
            .is_some_and(|cells| cells.contains_key("B")));
        let live = state.read().await;
        assert_eq!(live.sidebar_style, "command-center");
        assert_eq!(live.agents[0].envs, current.agents[0].envs);
        assert_eq!(
            live.agents[0].isolated_home,
            current.agents[0].isolated_home
        );
        assert!(live
            .coding_agent_profiles
            .profiles_by_agent
            .get("agent-0")
            .is_some_and(|cells| cells.contains_key("B")));
    }

    /// `RtkSweepResult` and `RtkSweepError` cross the Tauri IPC boundary, so
    /// the `#[serde(rename_all = "camelCase")]` rename is part of the public
    /// contract with the SolidJS frontend types in `src/shared/ipc.ts`.
    /// Removing the rename would still compile and the sweep would still
    /// run, but the banner would render `undefined` for every error.
    #[test]
    fn rtk_sweep_result_serializes_camel_case() {
        let value = RtkSweepResult {
            total: 5,
            succeeded: 4,
            errors: vec![RtkSweepError {
                path: "/some/dir".to_string(),
                error: "boom".to_string(),
            }],
        };
        let json = serde_json::to_string(&value).expect("serialize");
        assert!(json.contains("\"total\":5"), "missing total: {}", json);
        assert!(
            json.contains("\"succeeded\":4"),
            "missing succeeded: {}",
            json
        );
        assert!(
            json.contains("\"errors\":[{\"path\":\"/some/dir\",\"error\":\"boom\"}]"),
            "missing errors with camelCase fields: {}",
            json
        );
        // Negative checks: snake_case / PascalCase variants must not appear.
        assert!(!json.contains("\"Total\""));
        assert!(!json.contains("\"Path\""));
    }
}
