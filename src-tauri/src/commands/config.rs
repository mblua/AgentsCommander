use std::collections::BTreeMap;
use std::sync::Arc;
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
    purge_sessions_after_settings_update(&saved);
    Ok(())
}

#[tauri::command]
pub async fn save_settings_draft(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    draft: AppSettings,
) -> Result<(), String> {
    let (saved, events) = persist_settings_draft_update(settings.inner(), draft).await?;
    purge_sessions_after_settings_update(&saved);
    emit_settings_draft_update_events(&app, &events);
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

pub(crate) fn purge_sessions_after_settings_update(saved: &AppSettings) {
    if let Err(e) = crate::config::sessions_persistence::purge_sessions_outside_project_paths(
        &saved.project_paths,
    ) {
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
        profiles_changed: before.coding_agent_profiles != after.coding_agent_profiles,
        env_agent_ids,
    }
}

fn agent_env_settings_by_id(
    settings: &AppSettings,
) -> BTreeMap<String, (Vec<CodingAgentEnv>, bool)> {
    settings
        .agents
        .iter()
        .map(|agent| {
            (
                agent.id.clone(),
                (agent.envs.clone(), agent.isolate_codex_home),
            )
        })
        .collect()
}

fn emit_settings_draft_update_events(app: &AppHandle, events: &SettingsDraftUpdateEvents) {
    if events.profiles_changed {
        let _ = app.emit("coding_agent_profiles_updated", serde_json::json!({}));
    }

    for agent_id in &events.env_agent_ids {
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
    isolate_codex_home: bool,
) -> Result<(), String> {
    persist_coding_agent_env_settings_update(settings.inner(), &agent_id, envs, isolate_codex_home)
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
    isolate_codex_home: bool,
) -> Result<(), String> {
    let mut s = settings.write().await;
    let mut candidate = s.clone();
    let agent = candidate
        .agents
        .iter_mut()
        .find(|agent| agent.id == agent_id)
        .ok_or_else(|| format!("Agent '{}' is not configured", agent_id))?;
    agent.envs = envs;
    agent.isolate_codex_home = isolate_codex_home;
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

/// Narrow setter for `inject_rtk_hook`. Holds the SettingsState
/// write lock through `save_settings` so the in-memory mutation, the cloned
/// snapshot, and the disk write happen atomically with respect to each other
/// (issue #120, grinch H3 + N1). The explicit `drop(s)` after `save_settings`
/// makes the guard scope visually unambiguous: lock-then-write-then-release.
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
    let mut s = settings.write().await;
    s.inject_rtk_hook = value;
    let snapshot = s.clone();
    save_settings(&snapshot)?;
    drop(s); // explicit; lock released AFTER the disk write completes
    Ok(())
}

/// Narrow setter for `rtk_prompt_dismissed`. Same lock-held-through-save
/// pattern as `set_inject_rtk_hook` (issue #120, grinch H3 + N1).
#[tauri::command]
pub async fn set_rtk_prompt_dismissed(
    settings: State<'_, SettingsState>,
    value: bool,
) -> Result<(), String> {
    let mut s = settings.write().await;
    s.rtk_prompt_dismissed = value;
    let snapshot = s.clone();
    save_settings(&snapshot)?;
    drop(s); // explicit; lock released AFTER the disk write completes
    Ok(())
}

/// Narrow setter for `sounds_enabled`. Same lock-held-through-save
/// pattern as `set_inject_rtk_hook` (issue #158). Replaces the toolbar's
/// previous full-object `update_settings(next)` call, which could clobber
/// unrelated fields from a stale `settingsStore.current` snapshot.
#[tauri::command]
pub async fn set_sounds_enabled(
    settings: State<'_, SettingsState>,
    value: bool,
) -> Result<(), String> {
    let mut s = settings.write().await;
    s.sounds_enabled = value;
    let snapshot = s.clone();
    save_settings(&snapshot)?;
    drop(s); // explicit; lock released AFTER the disk write completes
    Ok(())
}

/// Narrow setter for `theme_light`. Same lock-held-through-save
/// pattern as `set_sounds_enabled` (issue #289). Lets the UI persist the
/// user's light/dark mode choice without going through `update_settings`,
/// which could clobber unrelated fields from a stale snapshot.
#[tauri::command]
pub async fn set_theme_light(
    settings: State<'_, SettingsState>,
    value: bool,
) -> Result<(), String> {
    let mut s = settings.write().await;
    s.theme_light = value;
    let snapshot = s.clone();
    save_settings(&snapshot)?;
    drop(s); // explicit; lock released AFTER the disk write completes
    Ok(())
}

/// Sweep every AC-managed agent directory and apply
/// `ensure_rtk_pretool_hook(dir, enabled)`. Best-effort per directory:
/// per-dir failures are logged + appended to `errors` and the sweep
/// continues. Reads `project_paths` from the live `SettingsState` (avoids a
/// disk-read race against `save_settings`).
///
/// Acquires `RtkSweepLockState` for the entire loop — eliminates the
/// in-process race vs. concurrent `ensure_claude_md_excludes` /
/// `ensure_rtk_pretool_hook` calls from `entity_creation` /
/// `agent_creator` (issue #120, grinch M8). Cross-process races (two AC
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
        persist_protected_settings_update_with_saver, persist_settings_draft_update_with_saver,
        RtkSweepError, RtkSweepResult,
    };
    use crate::config::settings::{
        AgentConfig, AppSettings, CodingAgentEnv, CodingAgentEnvSource, ProfileCellConfig,
        SettingsState,
    };
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
                git_pull_before: false,
                exclude_global_claude_md: false,
                envs: Vec::new(),
                isolate_codex_home: false,
            }],
            ..AppSettings::default()
        }
    }

    fn state_for(settings: AppSettings) -> SettingsState {
        Arc::new(RwLock::new(settings))
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
        original.agents[0].isolate_codex_home = true;
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
            live.agents[0].isolate_codex_home,
            original.agents[0].isolate_codex_home
        );
    }

    #[tokio::test]
    async fn invalid_profile_settings_update_leaves_live_settings_unchanged() {
        let original = settings_with_single_agent();
        let mut invalid_profiles = original.coding_agent_profiles.clone();
        invalid_profiles
            .matrix
            .entry("agent-0".to_string())
            .or_default()
            .insert(
                "A".to_string(),
                ProfileCellConfig {
                    enabled: true,
                    argv: Vec::new(),
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
        original.agents[0].isolate_codex_home = true;
        original
            .coding_agent_profiles
            .matrix
            .entry("agent-0".to_string())
            .or_default()
            .insert(
                "B".to_string(),
                ProfileCellConfig {
                    enabled: true,
                    argv: vec!["--current-profile".to_string()],
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
        draft.agents[0].isolate_codex_home = false;
        draft
            .coding_agent_profiles
            .matrix
            .entry("agent-0".to_string())
            .or_default()
            .insert(
                "C".to_string(),
                ProfileCellConfig {
                    enabled: true,
                    argv: vec!["--draft-profile".to_string()],
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
            live.agents[0].isolate_codex_home,
            original.agents[0].isolate_codex_home
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
        original.agents[0].isolate_codex_home = true;
        let state = state_for(original.clone());

        let mut draft = original.clone();
        draft.sidebar_style = "command-center".to_string();
        draft.agents[0].envs = vec![CodingAgentEnv {
            key: "OPENAI_API_BASE".to_string(),
            value: "draft".to_string(),
            source: CodingAgentEnvSource::User,
            enabled: true,
        }];
        draft.agents[0].isolate_codex_home = false;

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
            live.agents[0].isolate_codex_home,
            original.agents[0].isolate_codex_home
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
        current.agents[0].isolate_codex_home = true;
        current
            .coding_agent_profiles
            .matrix
            .entry("agent-0".to_string())
            .or_default()
            .insert(
                "B".to_string(),
                ProfileCellConfig {
                    enabled: true,
                    argv: vec!["--current-profile".to_string()],
                    env: BTreeMap::new(),
                    notes: String::new(),
                },
            );
        let state = state_for(current.clone());

        let mut stale = current.clone();
        stale.sidebar_style = "command-center".to_string();
        stale.agents[0].envs.clear();
        stale.agents[0].isolate_codex_home = false;
        stale.coding_agent_profiles.matrix.clear();

        let saved = persist_protected_settings_update_with_saver(&state, stale, |_| Ok(()))
            .await
            .unwrap();

        assert_eq!(saved.sidebar_style, "command-center");
        assert_eq!(saved.agents[0].envs, current.agents[0].envs);
        assert_eq!(
            saved.agents[0].isolate_codex_home,
            current.agents[0].isolate_codex_home
        );
        assert!(saved
            .coding_agent_profiles
            .matrix
            .get("agent-0")
            .is_some_and(|cells| cells.contains_key("B")));
        let live = state.read().await;
        assert_eq!(live.sidebar_style, "command-center");
        assert_eq!(live.agents[0].envs, current.agents[0].envs);
        assert_eq!(
            live.agents[0].isolate_codex_home,
            current.agents[0].isolate_codex_home
        );
        assert!(live
            .coding_agent_profiles
            .matrix
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
