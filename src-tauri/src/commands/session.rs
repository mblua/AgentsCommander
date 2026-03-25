use std::sync::Arc;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use crate::config::dark_factory;
use crate::config::sessions_persistence::persist_current_state;
use crate::config::settings::SettingsState;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::session::SessionInfo;
use crate::telegram::manager::TelegramBridgeState;
use crate::telegram::types::RepoConfig;
use crate::DetachedSessionsState;

/// Core session creation logic shared by the Tauri command and the restore path.
/// Creates a session record, spawns a PTY, and emits the session_created event.
/// After spawn, schedules a delayed injection of the session token + CLI instructions.
/// If `agent_id` is provided, saves it as lastCodingAgent in the repo's config.
pub async fn create_session_inner(
    app: &AppHandle,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    shell: String,
    shell_args: Vec<String>,
    cwd: String,
    session_name: Option<String>,
    agent_id: Option<String>,
) -> Result<SessionInfo, String> {
    let mgr = session_mgr.read().await;
    let mut session = mgr
        .create_session(shell.clone(), shell_args.clone(), cwd.clone())
        .await
        .map_err(|e| e.to_string())?;

    if let Some(name) = session_name {
        mgr.rename_session(session.id, name.clone())
            .await
            .map_err(|e| e.to_string())?;
        session.name = name;
    }

    let id = session.id;
    let token = session.token;

    pty_mgr
        .lock()
        .unwrap()
        .spawn(id, &shell, &shell_args, &cwd, 120, 30, app.clone())
        .map_err(|e| e.to_string())?;

    let info = SessionInfo::from(&session);
    let _ = app.emit("session_created", info.clone());

    // Save lastCodingAgent if an agent_id was provided
    if let Some(ref aid) = agent_id {
        if let Err(e) = dark_factory::set_last_coding_agent(&cwd, aid) {
            log::warn!("Failed to save lastCodingAgent: {}", e);
        }
    }

    // Only inject init prompt if this repo belongs to a team (has messaging peers)
    let cwd_for_init = cwd.clone();
    let pty_mgr_clone = Arc::clone(pty_mgr);
    tauri::async_runtime::spawn(async move {
        // Check if repo has teams configured
        let config_path = std::path::Path::new(&cwd_for_init)
            .join(".agentscommander")
            .join("config.json");
        let has_teams = if let Ok(content) = tokio::fs::read_to_string(&config_path).await {
            serde_json::from_str::<serde_json::Value>(&content)
                .ok()
                .and_then(|v| v.get("teams")?.as_array().map(|a| !a.is_empty()))
                .unwrap_or(false)
        } else {
            false
        };

        if !has_teams {
            return;
        }

        // Wait for the agent CLI to boot (3s covers most agents)
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        let binary_path = std::env::current_exe()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "agentscommander".to_string());

        let init_prompt = format!(
            concat!(
                "\n",
                "# === AgentsCommander Session Init ===\n",
                "# Your session token: {token}\n",
                "#\n",
                "# Send a message to another agent (fire-and-forget, do NOT use --get-output):\n",
                "#   \"{bin}\" send --token {token} --to \"<agent_name>\" --message \"...\" --mode wake\n",
                "#\n",
                "# The other agent will reply back via your console as a new message.\n",
                "# Do NOT use --get-output — it blocks and is only for non-interactive sessions.\n",
                "# After sending, you can stay idle and wait for the reply to arrive.\n",
                "#\n",
                "# List available peers:\n",
                "#   \"{bin}\" list-peers --token {token}\n",
                "# === End Session Init ===\n",
                "\r",
            ),
            token = token,
            bin = binary_path,
        );

        if let Ok(mgr) = pty_mgr_clone.lock() {
            if let Err(e) = mgr.write(id, init_prompt.as_bytes()) {
                log::warn!("Failed to inject init prompt for session {}: {}", id, e);
            }
        }
    });

    Ok(info)
}

/// Create a new session. Optionally override shell/args/cwd/name (for action buttons).
/// Falls back to settings defaults when not provided.
#[tauri::command]
pub async fn create_session(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    tg_mgr: State<'_, TelegramBridgeState>,
    settings: State<'_, SettingsState>,
    shell: Option<String>,
    shell_args: Option<Vec<String>>,
    cwd: Option<String>,
    session_name: Option<String>,
    agent_id: Option<String>,
) -> Result<SessionInfo, String> {
    let cfg = settings.read().await;

    let shell = shell.unwrap_or_else(|| cfg.default_shell.clone());
    let shell_args = shell_args.unwrap_or_else(|| cfg.default_shell_args.clone());
    let cwd = cwd.unwrap_or_else(|| {
        dirs::home_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "C:\\".to_string())
    });

    drop(cfg);

    let info = create_session_inner(
        &app,
        session_mgr.inner(),
        pty_mgr.inner(),
        shell,
        shell_args,
        cwd.clone(),
        session_name,
        agent_id,
    )
    .await?;

    // Persist after creation
    {
        let mgr = session_mgr.read().await;
        persist_current_state(&mgr).await;
    }

    // Auto-attach Telegram bot if repo has .agentscommander/config.json
    let id = Uuid::parse_str(&info.id).unwrap();
    let config_path = std::path::Path::new(&cwd)
        .join(".agentscommander")
        .join("config.json");
    if let Ok(contents) = tokio::fs::read_to_string(&config_path).await {
        if let Ok(repo_config) = serde_json::from_str::<RepoConfig>(&contents) {
            if let Some(bot_label) = repo_config.telegram_bot {
                let cfg = settings.read().await;
                let bot = cfg
                    .telegram_bots
                    .iter()
                    .find(|b| b.label == bot_label)
                    .cloned();
                drop(cfg);

                if let Some(bot) = bot {
                    let pty_arc = pty_mgr.inner().clone();
                    let mut tg = tg_mgr.lock().await;
                    if let Ok(bridge_info) = tg.attach(id, &bot, pty_arc, app.clone()) {
                        let _ = app.emit("telegram_bridge_attached", bridge_info);
                    }
                }
            }
        }
    }

    Ok(info)
}

#[tauri::command]
pub async fn destroy_session(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    tg_mgr: State<'_, TelegramBridgeState>,
    detached: State<'_, DetachedSessionsState>,
    id: String,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;

    // Remove from detached set
    {
        let mut detached_set = detached.lock().unwrap();
        detached_set.remove(&uuid);
    }

    // Auto-detach Telegram bridge if active
    {
        let mut tg = tg_mgr.lock().await;
        if tg.has_bridge(uuid) {
            let _ = tg.detach(uuid);
            let _ = app.emit(
                "telegram_bridge_detached",
                serde_json::json!({ "sessionId": id }),
            );
        }
    }

    // Kill the PTY first
    pty_mgr
        .lock()
        .unwrap()
        .kill(uuid)
        .map_err(|e| e.to_string())?;

    let mgr = session_mgr.read().await;
    let new_active = mgr
        .destroy_session(uuid)
        .await
        .map_err(|e| e.to_string())?;

    // Persist after destruction
    persist_current_state(&mgr).await;

    let _ = app.emit("session_destroyed", serde_json::json!({ "id": id }));

    // Close any detached terminal window for this session
    let detached_label = format!("terminal-{}", id.replace('-', ""));
    if let Some(detached_win) = app.get_webview_window(&detached_label) {
        let _ = detached_win.close();
    }

    // If a new session was auto-activated, emit switch event
    if let Some(new_id) = new_active {
        let _ = app.emit(
            "session_switched",
            serde_json::json!({ "id": new_id.to_string() }),
        );
    }

    Ok(())
}

#[tauri::command]
pub async fn switch_session(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    detached: State<'_, DetachedSessionsState>,
    id: String,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;

    // If this session is detached, focus its window instead of switching the main terminal
    {
        let detached_set = detached.lock().unwrap();
        if detached_set.contains(&uuid) {
            let label = format!("terminal-{}", id.replace('-', ""));
            if let Some(win) = app.get_webview_window(&label) {
                let _ = win.set_focus();
            }
            return Ok(());
        }
    }

    let mgr = session_mgr.read().await;
    mgr.switch_session(uuid)
        .await
        .map_err(|e| e.to_string())?;

    // Persist after switch (updates was_active)
    persist_current_state(&mgr).await;

    let _ = app.emit("session_switched", serde_json::json!({ "id": id }));

    Ok(())
}

#[tauri::command]
pub async fn rename_session(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    id: String,
    name: String,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;

    let mgr = session_mgr.read().await;
    mgr.rename_session(uuid, name.clone())
        .await
        .map_err(|e| e.to_string())?;

    // Persist after rename
    persist_current_state(&mgr).await;

    let _ = app.emit(
        "session_renamed",
        serde_json::json!({ "id": id, "name": name }),
    );

    Ok(())
}

#[tauri::command]
pub async fn list_sessions(
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
) -> Result<Vec<SessionInfo>, String> {
    let mgr = session_mgr.read().await;
    Ok(mgr.list_sessions().await)
}

#[tauri::command]
pub async fn set_last_prompt(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    id: String,
    text: String,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    let mgr = session_mgr.read().await;
    mgr.set_last_prompt(uuid, text.clone()).await;
    let _ = app.emit(
        "last_prompt",
        serde_json::json!({ "sessionId": id, "text": text }),
    );
    Ok(())
}

#[tauri::command]
pub async fn get_active_session(
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
) -> Result<Option<String>, String> {
    let mgr = session_mgr.read().await;
    Ok(mgr.get_active().await.map(|id| id.to_string()))
}
