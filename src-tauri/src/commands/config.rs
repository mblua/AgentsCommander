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
    let current = settings.read().await.clone();
    let mut to_save = merge_protected_coding_agent_settings(&current, new_settings);
    // Preserve existing root token — frontend cannot overwrite it
    to_save.root_token = current.root_token.clone();
    validate_and_repair_settings(&mut to_save)?;
    save_settings(&to_save)?;
    if let Err(e) = crate::config::sessions_persistence::purge_sessions_outside_project_paths(
        &to_save.project_paths,
    ) {
        log::warn!(
            "[settings] Failed to purge sessions outside current projectPaths after settings update: {}",
            e
        );
    }
    let mut s = settings.write().await;
    *s = to_save;
    Ok(())
}

#[tauri::command]
pub async fn update_coding_agent_profiles(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    profiles: CodingAgentProfilesConfig,
) -> Result<(), String> {
    let mut s = settings.write().await;
    s.coding_agent_profiles = profiles;
    validate_and_repair_settings(&mut s)?;
    let snapshot = s.clone();
    save_settings(&snapshot)?;
    drop(s);
    let _ = app.emit("coding_agent_profiles_updated", serde_json::json!({}));
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
    let mut s = settings.write().await;
    let agent = s
        .agents
        .iter_mut()
        .find(|agent| agent.id == agent_id)
        .ok_or_else(|| format!("Agent '{}' is not configured", agent_id))?;
    agent.envs = envs;
    agent.isolate_codex_home = isolate_codex_home;
    validate_and_repair_settings(&mut s)?;
    let snapshot = s.clone();
    save_settings(&snapshot)?;
    drop(s);
    let _ = app.emit(
        "coding_agent_env_settings_updated",
        serde_json::json!({ "agentId": agent_id }),
    );
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

/// Narrow setter — flips ONLY `inject_rtk_hook`. Holds the SettingsState
/// write lock through `save_settings` so the in-memory mutation, the cloned
/// snapshot, and the disk write happen atomically with respect to each other
/// (issue #120, grinch H3 + N1). The explicit `drop(s)` after `save_settings`
/// makes the guard scope visually unambiguous: lock-then-write-then-release.
///
/// **Caveat (out of scope per plan §7.5).** The pre-existing `update_settings`
/// command writes to disk OUTSIDE the SettingsState write lock (it does
/// `save_settings` then acquires the lock to assign in-memory). A concurrent
/// `update_settings` whose draft has the OLD `inject_rtk_hook` value can
/// therefore still produce on-disk / in-memory divergence interleaved with
/// this setter. Closing that race requires re-shaping `update_settings`
/// itself; flagged in the plan as a follow-up. For the banner flow which
/// drove this setter, the divergence window is small and idempotently
/// repaired by the next user toggle.
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

/// Narrow setter — flips ONLY `rtk_prompt_dismissed`. Same lock-held-through-save
/// pattern as `set_inject_rtk_hook` (issue #120, grinch H3 + N1). The same
/// `update_settings` caveat applies — see `set_inject_rtk_hook` doc for
/// details.
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

/// Narrow setter — flips ONLY `sounds_enabled`. Same lock-held-through-save
/// pattern as `set_inject_rtk_hook` (issue #158). Replaces the toolbar's
/// previous full-object `update_settings(next)` call, which could clobber
/// unrelated fields from a stale `settingsStore.current` snapshot.
/// The `update_settings` caveat documented on `set_inject_rtk_hook` applies
/// here too.
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

/// Narrow setter — flips ONLY `theme_light`. Same lock-held-through-save
/// pattern as `set_sounds_enabled` (issue #289). Lets the UI persist the
/// user's light/dark mode choice without going through `update_settings`,
/// which could clobber unrelated fields from a stale snapshot. The
/// `update_settings` caveat documented on `set_inject_rtk_hook` applies here
/// too.
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
    use super::{RtkSweepError, RtkSweepResult};

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
