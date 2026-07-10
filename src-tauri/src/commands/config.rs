use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use crate::api::auth;
use crate::config::settings::{
    load_settings, merge_protected_coding_agent_settings, parse_api_server_socket_addr,
    save_settings, validate_and_repair_settings, AppSettings, CodingAgentEnv,
    CodingAgentProfilesConfig, SettingsState,
};
use crate::network::OutboundNetwork;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::session::SessionInfo;
use crate::web::auth::WebAccessToken;
use crate::web::broadcast::WsBroadcaster;
use crate::{
    ApiServerHandle, ApiServerTask, WebServerHandle,
};

const HOME_MARKDOWN_URL: &str =
    "https://raw.githubusercontent.com/mblua/AgentsCommander/main/docs/home-en.md";

const HOME_MARKDOWN_MAX_BYTES: usize = 256 * 1024; // 256 KB
const HOME_MARKDOWN_TIMEOUT_SECS: u64 = 5;
const WEB_STATUS_CONNECT_TIMEOUT_MS: u64 = 500;
const API_SERVER_STOP_TIMEOUT_MS: u64 = 5_000;
// Default mirrors container API token TTL; manual GUI mints get a longer cap.
const MINT_API_CLIENT_DEFAULT_TTL_HOURS: i64 = 24;
const MINT_API_CLIENT_MAX_TTL_DAYS: i64 = 30;
const MINT_API_CLIENT_NOTE: &str =
    "Store this token now; it is shown only once. The registry keeps only a hash.";

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

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MintApiClientResponse {
    pub client_id: String,
    pub token: String,
    pub bound_fqn: String,
    pub bound_root: String,
    pub scopes: Vec<String>,
    pub expires_at: Option<String>,
    pub note: String,
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

/// #769 Phase 1 - return the externalized coding-agent catalog for the onboarding
/// and Settings pick lists. Contract (§14.2, dev-rust E5): resolves `Ok(Vec)` in
/// the normal AND self-heal cases (a missing or unparseable `agents.json` yields
/// the embedded default IN MEMORY, never a disk write); `Ok([])` is honored
/// verbatim when the user removed every built-in; `Err` only when the config
/// directory cannot be resolved (a genuine environment failure the compiled
/// fallback cannot satisfy). The frontend's never-empty fallback fires only on
/// this `Err`/transport path, so keeping the self-heal on the `Ok` side is load-
/// bearing for that contract.
#[tauri::command]
pub async fn get_coding_agent_catalog(
) -> Result<Vec<crate::config::coding_agents_catalog::CodingAgentDefinition>, String> {
    let config_dir = crate::config::config_dir().ok_or("No config dir")?;
    Ok(crate::config::coding_agents_catalog::load_catalog(
        &config_dir,
    ))
}

/// #769 Phase 2 - the coding-agent command executable basenames that ship a
/// re-seedable default config-folder master (`claude`, `codex`, `opencode`). The
/// frontend enables the "Re-seed default configuration" button only for a catalog
/// def whose command reduces (exact basename) to one of these; the reseed command
/// re-checks server-side.
#[tauri::command]
pub fn list_reseedable_agent_commands() -> Vec<String> {
    crate::config::coding_agents_catalog::reseedable_command_basenames()
}

/// #769 Phase 2 - restore a built-in's shipped default config-folder master (the
/// Settings "Re-seed default configuration" button). Gating is re-checked
/// server-side (exact executable basename, never `starts_with`, so `pi`/`agent`
/// cannot false-match): `command` must map to a built-in that ships a master, else
/// `Err`. On success the current master is `.bak`ed first, then atomically
/// replaced with the embedded default; it takes effect on NEW sessions via the
/// absent-only fill. Running replicas and their live config are untouched.
#[tauri::command]
pub async fn reseed_coding_agent_default(
    command: String,
) -> Result<crate::config::coding_agents_catalog::ReseedResult, String> {
    let config_dir = crate::config::config_dir().ok_or("No config dir")?;
    crate::config::coding_agents_catalog::reseed_master_for_command(&config_dir, &command)
}

#[tauri::command]
pub async fn update_settings(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    new_settings: AppSettings,
) -> Result<(), String> {
    let saved = persist_protected_settings_update(settings.inner(), new_settings).await?;
    purge_sessions_after_settings_update(&saved).await;
    // #714 re-register the (possibly changed) hotkey. Syntax was already validated
    // before persistence; an OS-level registration conflict must NOT turn this
    // successfully-persisted save into an Err. Surface it as a visible event.
    if let Err(e) =
        crate::screenshot::register_configured_hotkey(&app, &saved.screenshot_capture_hotkey)
    {
        let _ = app.emit(
            "screenshot_capture_failed",
            serde_json::json!({
                "message": format!("Screenshot hotkey was saved but could not be registered: {}", e)
            }),
        );
    }
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
    // #714 same non-failing hotkey re-registration as update_settings: persisted
    // save stays successful even if the OS refuses the hotkey.
    if let Err(e) =
        crate::screenshot::register_configured_hotkey(&app, &saved.screenshot_capture_hotkey)
    {
        let _ = app.emit(
            "screenshot_capture_failed",
            serde_json::json!({
                "message": format!("Screenshot hotkey was saved but could not be registered: {}", e)
            }),
        );
    }
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
    save: impl FnOnce(&AppSettings) -> Result<AppSettings, String>,
) -> Result<AppSettings, String> {
    let mut s = settings.write().await;
    let current = s.clone();
    let candidate = build_protected_settings_candidate(&current, new_settings)?;
    let written = save(&candidate)?;
    *s = written.clone();
    Ok(written)
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
    save: impl FnOnce(&AppSettings) -> Result<AppSettings, String>,
) -> Result<(AppSettings, SettingsDraftUpdateEvents), String> {
    let mut s = settings.write().await;
    let current = s.clone();
    draft.root_token = current.root_token.clone();
    validate_and_repair_settings(&mut draft)?;
    let events = settings_draft_update_events(&current, &draft);
    let written = save(&draft)?;
    *s = written.clone();
    Ok((written, events))
}

pub(crate) async fn purge_sessions_after_settings_update(saved: &AppSettings) {
    let Some(dir) = crate::config::config_dir() else {
        log::warn!("Could not determine home directory for session purge after settings update");
        return;
    };

    if let Err(e) = purge_sessions_after_settings_update_in_dir(saved, &dir).await {
        log::warn!(
            "[settings] Failed to purge sessions outside current projectPaths after settings update: {}",
            e
        );
    }
}

async fn purge_sessions_after_settings_update_in_dir(
    saved: &AppSettings,
    dir: &Path,
) -> Result<(), String> {
    crate::config::sessions_persistence::purge_sessions_outside_project_paths_in_dir(
        dir,
        &saved.project_paths,
    )
    .await
    .map(|_| ())
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
    let written = save_settings(&candidate)?;
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
    let payload =
        set_instance_profile_override_inner(settings.inner(), &agent_path, profile.as_deref())
            .await?;
    let _ = app.emit("coding_agent_profile_selection_updated", payload);
    Ok(())
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
    preview_coding_agent_profile_selection_inner(session_mgr.inner(), settings.inner(), request)
        .await
}

/// Enumerate the replicas a broad-scope profile assignment would touch and
/// return a fingerprint the frontend echoes back on apply. Shared by the
/// desktop command and the web dispatcher; reads settings + sessions only.
pub(crate) async fn preview_coding_agent_profile_selection_inner(
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    settings: &SettingsState,
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
    let (result, payload) = apply_coding_agent_profile_selection_inner(
        &app,
        session_mgr.inner(),
        pty_mgr.inner(),
        settings.inner(),
        request,
    )
    .await?;
    let _ = app.emit("coding_agent_profile_selection_updated", payload);
    Ok(result)
}

/// Apply a coding-agent profile assignment across the enumerated replicas
/// (optionally restarting live sessions) and return both the result and the
/// `coding_agent_profile_selection_updated` event payload. The caller emits the
/// payload (desktop) or broadcasts it to web clients. Shared by the desktop
/// command and the web dispatcher.
pub(crate) async fn apply_coding_agent_profile_selection_inner(
    app: &AppHandle,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<std::sync::Mutex<PtyManager>>,
    settings: &SettingsState,
    request: ApplyCodingAgentProfileSelectionRequest,
) -> Result<(ApplyCodingAgentProfileSelectionResult, serde_json::Value), String> {
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
                    app,
                    session_mgr,
                    pty_mgr,
                    settings,
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
                            classify_restart_failure(session_mgr, uuid).await;
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
    let payload = serde_json::json!({
        "scope": request.scope,
        "codingAgentId": request.coding_agent_id,
        "profile": normalized_profile,
        "updatedCount": result.updated_count,
        "restartedCount": result.restarted_count,
        "targetFingerprint": &result.target_fingerprint,
        "errors": &result.errors,
    });
    Ok((result, payload))
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
    let mut text =
        crate::path_utils::path_to_string_without_windows_verbatim_prefix(&path).replace('\\', "/");
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
        .map(|path| crate::path_utils::normalize_windows_verbatim_path_buf(&path))
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
        workgroup_path: crate::path_utils::path_to_string_without_windows_verbatim_prefix(wg_dir),
        replica_name,
        replica_path: crate::path_utils::path_to_string_without_windows_verbatim_prefix(
            replica_dir,
        ),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WebServerOwnershipState {
    OwnedRunning,
    ExternalListening,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebServerOwnedStatus {
    pub listening: bool,
    pub owned: bool,
    pub external_listening: bool,
    pub open_allowed: bool,
    pub bind: String,
    pub port: u16,
    pub state: WebServerOwnershipState,
}

fn ensure_web_remote_open_allowed(
    settings: &AppSettings,
    ws_handle: &WebServerHandle,
) -> Result<(), String> {
    if !settings.web_server_enabled {
        return Err("Web server is not enabled".into());
    }

    if !ws_handle.is_owned_running(&settings.web_server_bind, settings.web_server_port) {
        return Err("Web server is not owned by this app process".into());
    }

    Ok(())
}

fn build_web_server_owned_status(
    bind: String,
    port: u16,
    owned: bool,
    listening: bool,
) -> WebServerOwnedStatus {
    let external_listening = listening && !owned;
    let state = if owned {
        WebServerOwnershipState::OwnedRunning
    } else if external_listening {
        WebServerOwnershipState::ExternalListening
    } else {
        WebServerOwnershipState::Stopped
    };

    WebServerOwnedStatus {
        listening,
        owned,
        external_listening,
        open_allowed: owned,
        bind,
        port,
        state,
    }
}

#[tauri::command]
pub async fn open_web_remote(ws_handle: State<'_, WebServerHandle>) -> Result<(), String> {
    let settings = load_settings();
    ensure_web_remote_open_allowed(&settings, &ws_handle)?;

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

#[tauri::command]
pub async fn start_api_server(
    app_handle: tauri::AppHandle,
    api_handle: State<'_, ApiServerHandle>,
    settings: State<'_, SettingsState>,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    pty_mgr: State<'_, Arc<std::sync::Mutex<PtyManager>>>,
    shutdown: State<'_, crate::shutdown::ShutdownSignal>,
) -> Result<bool, String> {
    if api_handle.has_running()? {
        return Ok(false);
    }

    let s = settings.read().await;
    let bind = s.api_server_bind.clone();
    let port = s.api_server_port;
    drop(s);

    let probe_addr = api_server_probe_addr(&bind, port)?;
    if is_tcp_socket_listening(probe_addr).await {
        return Ok(false);
    }

    let api_shutdown = shutdown.token().child_token();
    let server_start = crate::api::start_server(
        bind,
        port,
        app_handle,
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
        api_shutdown.clone(),
    );
    let bound_addr = match crate::api::wait_for_startup_ready(server_start.readiness).await {
        Ok(bound_addr) => bound_addr,
        Err(err) => {
            api_shutdown.cancel();
            return Err(err);
        }
    };

    if !api_handle.store_if_idle(ApiServerTask::new(
        server_start.join_handle,
        api_shutdown,
        bound_addr,
    ))? {
        return Ok(false);
    }

    log::info!("[api-server] Started via command");
    Ok(true)
}

#[tauri::command]
pub async fn stop_api_server(api_handle: State<'_, ApiServerHandle>) -> Result<bool, String> {
    let stopped = api_handle
        .shutdown_running(Duration::from_millis(API_SERVER_STOP_TIMEOUT_MS))
        .await?;
    if stopped {
        log::info!("[api-server] Stopped via command");
    }
    Ok(stopped)
}

#[tauri::command]
pub async fn api_server_status(
    api_handle: State<'_, ApiServerHandle>,
    settings: State<'_, SettingsState>,
) -> Result<bool, String> {
    if api_handle.has_running()? {
        return Ok(true);
    }

    let probe_addr = {
        let s = settings.read().await;
        api_server_probe_addr(&s.api_server_bind, s.api_server_port)?
    };
    Ok(is_tcp_socket_listening(probe_addr).await)
}

#[tauri::command]
pub async fn mint_api_client(
    root: String,
    scopes: Vec<String>,
    label: Option<String>,
    expires: Option<String>,
) -> Result<MintApiClientResponse, String> {
    let settings = load_settings();
    let path = crate::config::config_dir()
        .ok_or_else(|| "Cannot resolve the host config directory".to_string())?
        .join(auth::REGISTRY_FILENAME);
    mint_api_client_with_path(
        &path,
        &settings,
        root,
        scopes,
        label,
        expires,
        Uuid::new_v4().to_string(),
        Uuid::new_v4().to_string(),
        chrono::Utc::now().to_rfc3339(),
    )
}

#[allow(clippy::too_many_arguments)]
fn mint_api_client_with_path(
    path: &Path,
    settings: &AppSettings,
    root: String,
    scopes: Vec<String>,
    label: Option<String>,
    expires: Option<String>,
    client_id: String,
    secret: String,
    issued_at: String,
) -> Result<MintApiClientResponse, String> {
    if crate::config::root_agent::is_root_agent_path(&root) {
        return Err(
            "Cannot mint an API client bound to the Root Agent; root-agent is API-excluded"
                .to_string(),
        );
    }

    let (bound_root, bound_fqn) = validate_mint_api_client_root(&root, settings)?;
    if crate::config::root_agent::is_root_agent_target(&bound_fqn)
        || bound_fqn == crate::config::root_agent::ROOT_AGENT_SENDER
    {
        return Err(
            "Cannot mint an API client bound to the Root Agent; root-agent is API-excluded"
                .to_string(),
        );
    }

    let scopes: Vec<String> = scopes
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    auth::validate_scopes(&scopes)?;
    let expires_at = bounded_mint_api_client_expiry(expires, &issued_at)?;

    let out = auth::mint(
        path,
        auth::MintRequest {
            client_id,
            secret,
            label: label.unwrap_or_default(),
            bound_root: bound_root.clone(),
            bound_fqn: bound_fqn.clone(),
            scopes: scopes.clone(),
            issued_at,
            expires_at: Some(expires_at.clone()),
        },
    )?;

    crate::api::audit::record(&out.client_id, &bound_fqn, "mint", "ok");
    Ok(MintApiClientResponse {
        client_id: out.client_id,
        token: out.secret,
        bound_fqn,
        bound_root,
        scopes,
        expires_at: Some(expires_at),
        note: MINT_API_CLIENT_NOTE.to_string(),
    })
}

fn validate_mint_api_client_root(
    root: &str,
    settings: &AppSettings,
) -> Result<(String, String), String> {
    let canonical = crate::config::coding_agent_profiles::validate_profile_selection_agent_path(
        settings,
        Path::new(root),
    )
    .map_err(|_| format!("unknown or invalid root: {}", root))?
    .launch_path;

    let is_replica = canonical
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.starts_with("__agent_"))
        .unwrap_or(false);
    if !is_replica {
        return Err(format!("unknown or invalid root: {}", root));
    }

    let bound_root = crate::path_utils::path_to_string_without_windows_verbatim_prefix(&canonical);
    let bound_fqn = crate::config::teams::agent_fqn_from_path(&bound_root);
    Ok((bound_root, bound_fqn))
}

fn bounded_mint_api_client_expiry(
    expires: Option<String>,
    issued_at: &str,
) -> Result<String, String> {
    let issued_at = chrono::DateTime::parse_from_rfc3339(issued_at)
        .map_err(|e| format!("Invalid issuedAt timestamp: {}", e))?
        .with_timezone(&chrono::Utc);
    let max_expires_at = issued_at + chrono::Duration::days(MINT_API_CLIENT_MAX_TTL_DAYS);

    let expires_at = match expires {
        Some(raw) => {
            let trimmed = raw.trim();
            let parsed = chrono::DateTime::parse_from_rfc3339(trimmed)
                .map_err(|e| format!("Invalid expires value; expected RFC3339: {}", e))?
                .with_timezone(&chrono::Utc);
            if parsed <= issued_at {
                return Err("expires must be after the issuedAt timestamp".to_string());
            }
            if parsed > max_expires_at {
                return Err(format!(
                    "expires exceeds maximum API client lifetime of {} days",
                    MINT_API_CLIENT_MAX_TTL_DAYS
                ));
            }
            parsed
        }
        None => issued_at + chrono::Duration::hours(MINT_API_CLIENT_DEFAULT_TTL_HOURS),
    };

    Ok(expires_at.to_rfc3339())
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

    if ws_handle.is_owned_running(&bind, port) {
        return Ok(true);
    }

    let addr = format!("{}:{}", bind, port);
    if is_tcp_listening(&addr).await {
        return Ok(false);
    }

    let start_result = crate::web::start_server(
        bind.clone(),
        port,
        Arc::clone(&web_token),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
        Arc::clone(&settings),
        (*broadcaster).clone(),
        app_handle,
        shutdown.inner().clone(),
    )
    .await;

    match start_result {
        Ok(join_handle) => {
            ws_handle.store_owned(bind, port, join_handle);
            log::info!("[web-server] Started via command");
            Ok(true)
        }
        Err(err) => {
            log::warn!("[web-server] start failed: {}", err);
            Ok(false)
        }
    }
}

#[tauri::command]
pub async fn stop_web_server(ws_handle: State<'_, WebServerHandle>) -> Result<bool, String> {
    if ws_handle.abort_running() {
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

#[tauri::command]
pub async fn get_web_server_owned_status(
    ws_handle: State<'_, WebServerHandle>,
    settings: State<'_, SettingsState>,
) -> Result<WebServerOwnedStatus, String> {
    let s = settings.read().await;
    let bind = s.web_server_bind.clone();
    let port = s.web_server_port;
    drop(s);

    let owned = ws_handle.is_owned_running(&bind, port);
    let addr = format!("{}:{}", bind, port);
    let listening = if owned {
        true
    } else {
        is_tcp_listening(&addr).await
    };

    Ok(build_web_server_owned_status(bind, port, owned, listening))
}

fn api_server_probe_addr(bind: &str, port: u16) -> Result<SocketAddr, String> {
    let configured = parse_api_server_socket_addr(bind, port)?;
    let probe_ip = match configured.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    Ok(SocketAddr::new(probe_ip, configured.port()))
}

async fn is_tcp_socket_listening(addr: SocketAddr) -> bool {
    matches!(
        tokio::time::timeout(
            Duration::from_millis(WEB_STATUS_CONNECT_TIMEOUT_MS),
            tokio::net::TcpStream::connect(addr),
        )
        .await,
        Ok(Ok(_))
    )
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
    save: impl FnOnce(&AppSettings) -> Result<AppSettings, String>,
) -> Result<(), String> {
    let mut s = settings.write().await;
    let mut candidate = s.clone();
    mutate_candidate(&mut candidate);
    let written = save(&candidate)?;
    *s = written;
    Ok(())
}

/// Narrow setter for `sounds_enabled`. Same candidate-save-publish
/// pattern as the other narrow setters (issue #158). Replaces the toolbar's
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
        api_server_probe_addr, api_server_status, build_web_server_owned_status,
        ensure_web_remote_open_allowed, is_tcp_socket_listening, mint_api_client_with_path,
        persist_coding_agent_env_settings_update, persist_coding_agent_profiles_update,
        persist_narrow_settings_update_with_saver, persist_protected_settings_update_with_saver,
        persist_settings_draft_update_with_saver, purge_sessions_after_settings_update_in_dir,
        start_api_server, WebServerOwnershipState,
        MINT_API_CLIENT_DEFAULT_TTL_HOURS, MINT_API_CLIENT_MAX_TTL_DAYS, MINT_API_CLIENT_NOTE,
    };
    #[cfg(windows)]
    use super::{build_profile_assignment_target, canonical_compare_key};
    use crate::api::auth;
    use crate::config::sessions_persistence::PersistedSession;
    use crate::config::settings::{
        AgentConfig, AppSettings, CodingAgentEnv, CodingAgentEnvSource, ProfileCellConfig,
        SettingsState,
    };
    use crate::session::manager::SessionManager;
    use crate::{ApiServerHandle, ApiServerTask, WebServerHandle};
    use std::collections::{BTreeMap, HashMap};
    use std::net::{IpAddr, Ipv4Addr};
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tauri::Manager;
    use tokio::sync::RwLock;
    use tokio_util::sync::CancellationToken;

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
                backend: Default::default(),
            }],
            ..AppSettings::default()
        }
    }

    #[test]
    #[cfg(windows)]
    fn canonical_compare_key_converts_verbatim_unc() {
        assert_eq!(
            canonical_compare_key(Path::new(r"\\?\UNC\server\share\Repo")),
            "//server/share/repo"
        );
    }

    #[test]
    #[cfg(windows)]
    fn profile_assignment_target_paths_convert_verbatim_unc() {
        let live_by_cwd = BTreeMap::new();
        let identity = crate::config::replica_identity::WgReplicaIdentity {
            agent_name: "dev".to_string(),
            workspace_dir: PathBuf::from(r"\\?\UNC\server\share\repo\.ac"),
            matrix_dir: PathBuf::from(r"\\?\UNC\server\share\repo\.ac\_agent_dev"),
            identity: "../../_agent_dev".to_string(),
        };

        let target = build_profile_assignment_target(
            Path::new(r"\\?\UNC\server\share\repo\.ac\wg-1\__agent_dev"),
            &identity,
            &live_by_cwd,
        );

        assert_eq!(target.workgroup_path, r"\\server\share\repo\.ac\wg-1");
        assert_eq!(
            target.replica_path,
            r"\\server\share\repo\.ac\wg-1\__agent_dev"
        );
    }

    fn state_for(settings: AppSettings) -> SettingsState {
        Arc::new(RwLock::new(settings))
    }

    fn write_settings_file(dir: &Path, settings: &AppSettings) {
        let json = serde_json::to_vec_pretty(settings).expect("serialize settings");
        std::fs::write(dir.join("settings.json"), json).expect("write settings.json");
    }

    fn write_sessions_file(dir: &Path, sessions: &[PersistedSession]) {
        let json = serde_json::to_vec_pretty(sessions).expect("serialize sessions");
        std::fs::write(dir.join("sessions.json"), json).expect("write sessions.json");
    }

    fn read_sessions_file(dir: &Path) -> Vec<PersistedSession> {
        let json = std::fs::read_to_string(dir.join("sessions.json")).expect("read sessions.json");
        serde_json::from_str(&json).expect("parse sessions.json")
    }

    fn assert_single_project(settings: &AppSettings, project: &str) {
        assert_eq!(settings.project_paths, vec![project.to_string()]);
        assert_eq!(settings.project_path.as_deref(), Some(project));
    }

    fn api_server_command_test_app(settings: AppSettings) -> tauri::App {
        let session_mgr = Arc::new(RwLock::new(SessionManager::new()));
        let app = tauri::Builder::default()
            .any_thread()
            .manage(ApiServerHandle::default())
            .manage(state_for(settings))
            .manage(Arc::clone(&session_mgr))
            .manage(crate::shutdown::ShutdownSignal::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build test app");
        let idle_detector = crate::pty::idle_detector::IdleDetector::new(|_| {}, |_| {});
        let git_watcher = crate::pty::git_watcher::GitWatcher::new(
            Arc::clone(&session_mgr),
            app.handle().clone(),
        );
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new(
            Arc::new(Mutex::new(HashMap::new())),
            idle_detector,
            git_watcher,
            None,
            session_mgr,
        )));
        app.manage(pty_mgr);
        app
    }

    #[test]
    fn api_server_probe_addr_maps_wildcards_to_loopback() {
        assert_eq!(
            api_server_probe_addr("0.0.0.0", 9906).unwrap().to_string(),
            "127.0.0.1:9906"
        );
        assert_eq!(
            api_server_probe_addr("127.0.0.1", 9906)
                .unwrap()
                .to_string(),
            "127.0.0.1:9906"
        );
        assert_eq!(
            api_server_probe_addr("::", 9906).unwrap().to_string(),
            "[::1]:9906"
        );
        assert_eq!(
            api_server_probe_addr("::1", 9906).unwrap().to_string(),
            "[::1]:9906"
        );
    }

    #[tokio::test]
    async fn api_server_probe_addr_detects_wildcard_listener_via_loopback() {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))
            .await
            .expect("bind wildcard test listener");
        let port = listener.local_addr().expect("listener local addr").port();
        let probe_addr = api_server_probe_addr("0.0.0.0", port).expect("probe addr");

        assert_eq!(probe_addr.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert!(is_tcp_socket_listening(probe_addr).await);
    }

    #[tokio::test]
    async fn api_server_status_reports_running_for_managed_wildcard_server() {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))
            .await
            .expect("bind wildcard test listener");
        let bound_addr = listener.local_addr().expect("listener local addr");
        let settings = AppSettings {
            api_server_bind: "0.0.0.0".to_string(),
            api_server_port: bound_addr.port(),
            ..AppSettings::default()
        };
        let app = tauri::test::mock_builder()
            .manage(ApiServerHandle::default())
            .manage(state_for(settings))
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build test app");

        let shutdown = CancellationToken::new();
        let task_shutdown = shutdown.clone();
        let join = tauri::async_runtime::spawn(async move {
            let _listener = listener;
            task_shutdown.cancelled().await;
        });
        app.state::<ApiServerHandle>()
            .store_if_idle(ApiServerTask::new(join, shutdown, bound_addr))
            .expect("store api server task");

        let running =
            api_server_status(app.state::<ApiServerHandle>(), app.state::<SettingsState>())
                .await
                .expect("status succeeds");

        assert!(running);
        assert!(app
            .state::<ApiServerHandle>()
            .shutdown_running(Duration::from_secs(1))
            .await
            .expect("shutdown stored task"));
    }

    #[tokio::test]
    async fn start_api_server_bind_failure_returns_err_and_status_false() {
        let settings = AppSettings {
            api_server_bind: "192.0.2.1".to_string(),
            api_server_port: 9906,
            ..AppSettings::default()
        };
        let app = api_server_command_test_app(settings);

        let err = start_api_server(
            app.handle().clone(),
            app.state::<ApiServerHandle>(),
            app.state::<SettingsState>(),
            app.state::<Arc<RwLock<SessionManager>>>(),
            app.state::<Arc<Mutex<crate::pty::manager::PtyManager>>>(),
            app.state::<crate::shutdown::ShutdownSignal>(),
        )
        .await
        .expect_err("nonlocal bind should fail readiness");

        assert!(err.contains("API server bind failed"), "{err}");
        assert!(!app.state::<ApiServerHandle>().has_running().unwrap());
        assert!(
            !api_server_status(app.state::<ApiServerHandle>(), app.state::<SettingsState>())
                .await
                .expect("status succeeds")
        );
    }

    struct MintApiClientFixture {
        _temp: tempfile::TempDir,
        path: PathBuf,
        project: PathBuf,
        settings: AppSettings,
        root: String,
        expected_fqn: String,
    }

    fn mint_api_client_fixture() -> MintApiClientFixture {
        let temp = tempfile::tempdir().expect("tempdir");
        let project = temp.path().join("proj-a");
        let workspace = project.join(".ac");
        let matrix = workspace.join("_agent_alice");
        let replica = workspace.join("wg-1-devs").join("__agent_alice");
        std::fs::create_dir_all(&matrix).expect("create matrix");
        std::fs::create_dir_all(&replica).expect("create replica");
        std::fs::write(
            replica.join("config.json"),
            r#"{"identity":"../../_agent_alice"}"#,
        )
        .expect("write replica config");

        let root_path = std::fs::canonicalize(&replica).expect("canonical replica");
        let root = crate::path_utils::path_to_string_without_windows_verbatim_prefix(&root_path);
        let expected_fqn = crate::config::teams::agent_fqn_from_path(&root);
        let settings = AppSettings {
            project_paths: vec![project.to_string_lossy().to_string()],
            ..AppSettings::default()
        };

        MintApiClientFixture {
            path: temp.path().join(auth::REGISTRY_FILENAME),
            _temp: temp,
            project,
            settings,
            root,
            expected_fqn,
        }
    }

    #[test]
    fn mint_api_client_command_helper_returns_token_and_stores_hash_only_with_default_expiry() {
        let fixture = mint_api_client_fixture();
        let issued_at = "2026-01-01T00:00:00Z";
        let expected_expires_at = (chrono::DateTime::parse_from_rfc3339(issued_at)
            .unwrap()
            .with_timezone(&chrono::Utc)
            + chrono::Duration::hours(MINT_API_CLIENT_DEFAULT_TTL_HOURS))
        .to_rfc3339();

        let response = mint_api_client_with_path(
            &fixture.path,
            &fixture.settings,
            fixture.root.clone(),
            vec![
                " send ".to_string(),
                "".to_string(),
                "list-peers-lean".to_string(),
            ],
            Some("gui mint".to_string()),
            None,
            "client-1".to_string(),
            "plain-secret".to_string(),
            issued_at.to_string(),
        )
        .expect("mint succeeds");

        assert_eq!(response.client_id, "client-1");
        assert_eq!(response.token, "plain-secret");
        assert_eq!(response.bound_root, fixture.root);
        assert_eq!(response.bound_fqn, fixture.expected_fqn);
        assert_eq!(response.scopes, vec!["send", "list-peers-lean"]);
        assert_eq!(
            response.expires_at.as_deref(),
            Some(expected_expires_at.as_str())
        );
        assert_eq!(response.note, MINT_API_CLIENT_NOTE);

        let raw = std::fs::read_to_string(&fixture.path).expect("read registry");
        assert!(raw.contains("client-1"));
        assert!(raw.contains("sha256:"));
        assert!(!raw.contains("plain-secret"));

        let registry = auth::list(&fixture.path);
        assert_eq!(registry.clients.len(), 1);
        let client = &registry.clients[0];
        assert_eq!(client.client_id, "client-1");
        assert_eq!(client.label, "gui mint");
        assert_eq!(client.bound_fqn, fixture.expected_fqn);
        assert_eq!(client.bound_root, response.bound_root);
        assert_eq!(client.scopes, response.scopes);
        assert_eq!(client.expires_at, response.expires_at);
        assert_ne!(client.token_hash, "plain-secret");
    }

    #[test]
    fn mint_api_client_rejects_unknown_root() {
        let fixture = mint_api_client_fixture();
        let unknown_root = fixture
            .project
            .join(".ac")
            .join("wg-1-devs")
            .join("__agent_missing");

        let err = mint_api_client_with_path(
            &fixture.path,
            &fixture.settings,
            unknown_root.to_string_lossy().to_string(),
            vec!["send".to_string()],
            None,
            None,
            "client-1".to_string(),
            "plain-secret".to_string(),
            "2026-01-01T00:00:00Z".to_string(),
        )
        .expect_err("unknown root should fail");

        assert!(err.contains("unknown or invalid root"), "{err}");
        assert!(!fixture.path.exists());
    }

    #[test]
    fn mint_api_client_rejects_expiry_beyond_max() {
        let fixture = mint_api_client_fixture();
        let issued_at = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let over_max =
            (issued_at + chrono::Duration::days(MINT_API_CLIENT_MAX_TTL_DAYS + 1)).to_rfc3339();

        let err = mint_api_client_with_path(
            &fixture.path,
            &fixture.settings,
            fixture.root,
            vec!["send".to_string()],
            None,
            Some(over_max),
            "client-1".to_string(),
            "plain-secret".to_string(),
            issued_at.to_rfc3339(),
        )
        .expect_err("over-max expiry should fail");

        assert!(
            err.contains("expires exceeds maximum API client lifetime"),
            "{err}"
        );
        assert!(!fixture.path.exists());
    }

    #[test]
    fn web_server_owned_status_maps_owned_running() {
        let status = build_web_server_owned_status("127.0.0.1".to_string(), 8765, true, true);

        assert!(status.listening);
        assert!(status.owned);
        assert!(!status.external_listening);
        assert!(status.open_allowed);
        assert_eq!(status.bind, "127.0.0.1");
        assert_eq!(status.port, 8765);
        assert_eq!(status.state, WebServerOwnershipState::OwnedRunning);
    }

    #[test]
    fn web_server_owned_status_maps_external_listener() {
        let status = build_web_server_owned_status("127.0.0.1".to_string(), 8765, false, true);

        assert!(status.listening);
        assert!(!status.owned);
        assert!(status.external_listening);
        assert!(!status.open_allowed);
        assert_eq!(status.state, WebServerOwnershipState::ExternalListening);
    }

    #[test]
    fn web_server_owned_status_maps_stopped() {
        let status = build_web_server_owned_status("127.0.0.1".to_string(), 8765, false, false);

        assert!(!status.listening);
        assert!(!status.owned);
        assert!(!status.external_listening);
        assert!(!status.open_allowed);
        assert_eq!(status.state, WebServerOwnershipState::Stopped);
    }

    #[test]
    fn ensure_web_remote_open_allowed_rejects_enabled_settings_without_owned_handle() {
        let settings = AppSettings {
            web_server_enabled: true,
            web_server_bind: "127.0.0.1".to_string(),
            web_server_port: 8765,
            ..AppSettings::default()
        };
        let handle = WebServerHandle::default();

        let err = ensure_web_remote_open_allowed(&settings, &handle).unwrap_err();

        assert_eq!(err, "Web server is not owned by this app process");
    }

    #[tokio::test]
    async fn ensure_web_remote_open_allowed_accepts_owned_running_handle() {
        let settings = AppSettings {
            web_server_enabled: true,
            web_server_bind: "127.0.0.1".to_string(),
            web_server_port: 8765,
            ..AppSettings::default()
        };
        let handle = WebServerHandle::default();
        let task = tauri::async_runtime::spawn(async {
            std::future::pending::<()>().await;
        });

        handle.store_owned("127.0.0.1".to_string(), 8765, task);

        assert!(ensure_web_remote_open_allowed(&settings, &handle).is_ok());
        assert!(handle.abort_running());
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
        assert!(
            !events.profiles_changed,
            "env-only edit is not a profiles change"
        );
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
                crate::pty::backend::SessionBackendKind::LocalProcess,
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

        let err = persist_settings_draft_update_with_saver(
            &state,
            draft,
            |_| -> Result<AppSettings, String> { panic!("save must not run") },
        )
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
        assert_eq!(live.sounds_enabled, original.sounds_enabled, "{field_name}");
        assert_eq!(live.theme_light, original.theme_light, "{field_name}");
    }

    #[tokio::test]
    async fn narrow_settings_setter_save_failure_leaves_live_settings_unchanged() {
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
    async fn settings_update_does_not_clobber_in_memory_project_lists() {
        let temp = tempfile::tempdir().unwrap();
        let settings_path = temp.path().join("settings.json");
        let disk_project = "C:/disk/project-a";
        write_settings_file(
            temp.path(),
            &AppSettings {
                project_paths: vec![disk_project.to_string()],
                project_path: Some(disk_project.to_string()),
                ..AppSettings::default()
            },
        );

        let mut current = settings_with_single_agent();
        current.project_paths = vec!["C:/stale/current".to_string()];
        current.project_path = Some("C:/stale/current".to_string());
        current.root_token = Some("current-root-token".to_string());

        let protected_state = state_for(current.clone());
        let mut stale = current.clone();
        stale.project_paths.clear();
        stale.project_path = None;
        stale.root_token = Some("frontend-root-token".to_string());

        let saved =
            persist_protected_settings_update_with_saver(&protected_state, stale, |candidate| {
                crate::config::settings::save_settings_to_path_preserving_project_paths(
                    candidate,
                    &settings_path,
                )
            })
            .await
            .unwrap();

        assert_single_project(&saved, disk_project);
        assert_eq!(saved.root_token.as_deref(), Some("current-root-token"));
        {
            let live = protected_state.read().await;
            assert_single_project(&live, disk_project);
            assert_eq!(live.root_token.as_deref(), Some("current-root-token"));
        }

        let draft_state = state_for(current.clone());
        let mut draft = current;
        draft.project_paths.clear();
        draft.project_path = None;

        let (saved, _events) =
            persist_settings_draft_update_with_saver(&draft_state, draft, |candidate| {
                crate::config::settings::save_settings_to_path_preserving_project_paths(
                    candidate,
                    &settings_path,
                )
            })
            .await
            .unwrap();

        assert_single_project(&saved, disk_project);
        {
            let live = draft_state.read().await;
            assert_single_project(&live, disk_project);
        }
    }

    #[tokio::test]
    async fn settings_update_returns_written_project_lists_for_session_purge() {
        let temp = tempfile::tempdir().unwrap();
        let settings_path = temp.path().join("settings.json");
        let project = temp.path().join("project-a");
        let kept_workdir = project.join(".ac").join("wg-1").join("__agent_dev");
        let outside = temp.path().join("project-b");
        let outside_workdir = outside.join(".ac").join("wg-1").join("__agent_dev");
        std::fs::create_dir_all(&kept_workdir).expect("create kept workdir");
        std::fs::create_dir_all(&outside_workdir).expect("create outside workdir");

        let project_path = project.to_string_lossy().to_string();
        write_settings_file(
            temp.path(),
            &AppSettings {
                project_paths: vec![project_path.clone()],
                project_path: Some(project_path.clone()),
                ..AppSettings::default()
            },
        );
        write_sessions_file(
            temp.path(),
            &[
                PersistedSession {
                    name: "kept".to_string(),
                    shell: "codex".to_string(),
                    working_directory: kept_workdir.to_string_lossy().to_string(),
                    ..Default::default()
                },
                PersistedSession {
                    name: "outside".to_string(),
                    shell: "codex".to_string(),
                    working_directory: outside_workdir.to_string_lossy().to_string(),
                    ..Default::default()
                },
            ],
        );

        let mut current = settings_with_single_agent();
        current.project_paths.clear();
        current.project_path = None;
        let state = state_for(current.clone());
        let stale = current;

        let saved = persist_protected_settings_update_with_saver(&state, stale, |candidate| {
            crate::config::settings::save_settings_to_path_preserving_project_paths(
                candidate,
                &settings_path,
            )
        })
        .await
        .unwrap();

        purge_sessions_after_settings_update_in_dir(&saved, temp.path())
            .await
            .unwrap();

        let remaining = read_sessions_file(temp.path());
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].name, "kept");
        assert_eq!(
            remaining[0].working_directory,
            kept_workdir.to_string_lossy()
        );
        assert_single_project(&saved, &project_path);
    }

    #[tokio::test]
    async fn settings_update_missing_settings_file_preserves_project_sessions() {
        let temp = tempfile::tempdir().unwrap();
        let settings_path = temp.path().join("settings.json");
        assert!(!settings_path.exists());

        let project = temp.path().join("project-a");
        let kept_workdir = project.join(".ac").join("wg-1").join("__agent_dev");
        std::fs::create_dir_all(&kept_workdir).expect("create kept workdir");
        let project_path = project.to_string_lossy().to_string();

        write_sessions_file(
            temp.path(),
            &[PersistedSession {
                name: "kept".to_string(),
                shell: "codex".to_string(),
                working_directory: kept_workdir.to_string_lossy().to_string(),
                ..Default::default()
            }],
        );

        let mut current = settings_with_single_agent();
        current.project_paths = vec![project_path.clone()];
        current.project_path = Some(project_path.clone());
        let state = state_for(current.clone());

        let mut incoming = current.clone();
        incoming.sidebar_style = "deep-space".to_string();
        let saved = persist_protected_settings_update_with_saver(&state, incoming, |candidate| {
            crate::config::settings::save_settings_to_path_preserving_project_paths(
                candidate,
                &settings_path,
            )
        })
        .await
        .unwrap();

        assert_single_project(&saved, &project_path);
        {
            let live = state.read().await;
            assert_single_project(&live, &project_path);
        }
        assert!(settings_path.exists(), "writer must create settings.json");
        let disk: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
        assert_eq!(
            disk["projectPaths"],
            serde_json::json!([project_path.clone()])
        );
        assert_eq!(disk["projectPath"], serde_json::json!(project_path));

        purge_sessions_after_settings_update_in_dir(&saved, temp.path())
            .await
            .unwrap();

        let remaining = read_sessions_file(temp.path());
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].name, "kept");
        assert_eq!(
            remaining[0].working_directory,
            kept_workdir.to_string_lossy()
        );
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

        let saved = persist_protected_settings_update_with_saver(&state, stale, |candidate| {
            Ok(candidate.clone())
        })
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
}
