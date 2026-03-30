use std::sync::Arc;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use crate::config::dark_factory::{self, AgentLocalConfig};
use crate::config::sessions_persistence::persist_current_state;
use crate::config::settings::{AppSettings, SettingsState};
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::session::SessionInfo;
use crate::telegram::manager::TelegramBridgeState;
use crate::DetachedSessionsState;

/// Core session creation logic shared by the Tauri command and the restore path.
/// Creates a session record, spawns a PTY, and emits the session_created event.
/// Auto-detects agent from shell command if not provided, and auto-injects --continue for Claude.
/// If `skip_tooling_save` is true, skips writing to the repo's config.json (for temp sessions).
pub async fn create_session_inner(
    app: &AppHandle,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    shell: String,
    shell_args: Vec<String>,
    cwd: String,
    session_name: Option<String>,
    agent_id: Option<String>,
    agent_label: Option<String>,
    skip_tooling_save: bool,
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

    // Auto-detect agent from shell command if not explicitly provided
    let (agent_id, agent_label) = if agent_id.is_some() {
        (agent_id, agent_label)
    } else {
        let settings_state = app.state::<SettingsState>();
        let cfg = settings_state.read().await;
        resolve_agent_from_shell(&shell, &shell_args, &cfg)
    };

    // Detect if this is a Claude session (shared flag for --continue and --append-system-prompt-file)
    let mut shell_args = shell_args;
    let full_cmd = format!("{} {}", shell, shell_args.join(" "));
    let cmd_basenames: Vec<String> = full_cmd.split_whitespace().map(|t| executable_basename(t)).collect();
    let is_claude = cmd_basenames.iter().any(|b| b == "claude");

    // Auto-inject --continue for Claude agents with prior sessions in this repo
    if is_claude {
        if let Some(ref aid) = agent_id {
            let already_has_continue = full_cmd.split_whitespace().any(|t| {
                let lower = t.to_lowercase();
                lower == "--continue" || lower == "-c"
            });
            if !already_has_continue {
                let config_path = std::path::Path::new(&cwd)
                    .join(".agentscommander")
                    .join("config.json");
                let has_prior_session = tokio::fs::read_to_string(&config_path).await
                    .ok()
                    .and_then(|c| serde_json::from_str::<AgentLocalConfig>(&c).ok())
                    .map(|cfg| cfg.tooling.coding_agents.contains_key(aid))
                    .unwrap_or(false);
                if has_prior_session {
                    if executable_basename(&shell) == "cmd" {
                        if let Some(last) = shell_args.last_mut() {
                            if executable_basename(last) == "claude" || last.to_lowercase().contains("claude") {
                                *last = format!("{} --continue", last);
                                log::info!("Auto-injected --continue for agent '{}' (prior session, cmd path)", aid);
                            }
                        }
                    } else {
                        shell_args.push("--continue".to_string());
                        log::info!("Auto-injected --continue for agent '{}' (prior session found)", aid);
                    }
                }
            }
        }
    }

    // Auto-inject --append-system-prompt-file for Claude sessions
    let context_file_injected = if is_claude {
        let binary_path = crate::resolve_bin_label();
        match crate::config::session_context::write_session_context(&cwd, &token.to_string(), &binary_path) {
            Ok(rel_path) => {
                if executable_basename(&shell) == "cmd" {
                    if let Some(last) = shell_args.last_mut() {
                        if last.to_lowercase().contains("claude") {
                            *last = format!("{} --append-system-prompt-file {}", last, rel_path);
                            log::info!("Injected --append-system-prompt-file for Claude (cmd path)");
                        }
                    }
                } else {
                    shell_args.push("--append-system-prompt-file".to_string());
                    shell_args.push(rel_path);
                    log::info!("Injected --append-system-prompt-file for Claude session");
                }
                true
            }
            Err(e) => {
                log::warn!("Failed to write AgentsCommanderContext.md: {}. Falling back to PTY injection.", e);
                false
            }
        }
    } else {
        false
    };

    pty_mgr
        .lock()
        .unwrap()
        .spawn(id, &shell, &shell_args, &cwd, 120, 30, app.clone())
        .map_err(|e| e.to_string())?;

    let info = SessionInfo::from(&session);
    let _ = app.emit("session_created", info.clone());

    // Save lastCodingAgent + codingAgents (skip for temp sessions)
    if !skip_tooling_save {
        if let Some(ref aid) = agent_id {
            let label = agent_label.as_deref().unwrap_or("Unknown");
            let session_id_str = id.to_string();
            if let Err(e) = dark_factory::set_last_coding_agent(&cwd, aid, label, Some(&session_id_str)) {
                log::warn!("Failed to save lastCodingAgent: {}", e);
            }
        }
    }

    // Inject init prompt for agent sessions so they know their token.
    // Skip for plain interactive shells and Claude sessions with context file.
    let shell_lower = shell.to_lowercase();
    let is_interactive_shell = ["powershell", "pwsh", "cmd", "bash", "zsh", "sh", "wsl", "nu"]
        .iter()
        .any(|s| shell_lower == *s || shell_lower.ends_with(&format!("/{}", s)) || shell_lower.ends_with(&format!("\\{}", s)));

    let skip_init_prompt = is_interactive_shell || context_file_injected;
    let cwd_for_init = cwd.clone();
    let app_clone = app.clone();
    if is_interactive_shell {
        log::debug!("Skipping init prompt for interactive shell '{}'", shell);
    } else if context_file_injected {
        log::debug!("Skipping init prompt for Claude (using --append-system-prompt-file)");
    }
    tauri::async_runtime::spawn(async move {
        if skip_init_prompt {
            return;
        }
        // Wait for the agent CLI to boot (3s covers most agents)
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        let binary_path = crate::resolve_bin_label();

        let init_prompt = format!(
            concat!(
                "\n",
                "# === AgentsCommander Session Init ===\n",
                "# Your session token: {token}\n",
                "# Your agent root: {root}\n",
                "#\n",
                "# Send a message to another agent (fire-and-forget, do NOT use --get-output):\n",
                "#   \"{bin}\" send --token {token} --root \"{root}\" --to \"<agent_name>\" --message \"...\" --mode wake\n",
                "#\n",
                "# The other agent will reply back via your console as a new message.\n",
                "# Do NOT use --get-output — it blocks and is only for non-interactive sessions.\n",
                "# After sending, you can stay idle and wait for the reply to arrive.\n",
                "#\n",
                "# List available peers:\n",
                "#   \"{bin}\" list-peers --token {token} --root \"{root}\"\n",
                "# === End Session Init ===\n",
            ),
            token = token,
            bin = binary_path,
            root = cwd_for_init,
        );

        if let Err(e) = crate::pty::inject::inject_text_into_session(&app_clone, id, &init_prompt, true, crate::pty::transcript::InjectReason::InitPrompt, None).await {
            log::warn!("Failed to inject init prompt for session {}: {}", id, e);
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

    // Resolve agent label before dropping cfg
    let agent_label = agent_id.as_ref().and_then(|aid| {
        cfg.agents.iter().find(|a| a.id == *aid).map(|a| a.label.clone())
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
        agent_label,
        false, // persist tooling
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
        if let Ok(local_config) = serde_json::from_str::<AgentLocalConfig>(&contents) {
            if let Some(bot_label) = local_config.tooling.telegram_bot {
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

/// Extract the basename (without extension) from a path or command token.
fn executable_basename(s: &str) -> String {
    std::path::Path::new(s)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(s)
        .to_lowercase()
}

/// Try to match the shell command against configured agents in settings.
/// Returns (Some(agent_id), Some(label)) if a match is found, (None, None) otherwise.
fn resolve_agent_from_shell(
    shell: &str,
    shell_args: &[String],
    settings: &AppSettings,
) -> (Option<String>, Option<String>) {
    // Collect all tokens from shell + args, extract basenames for comparison
    let full_cmd = format!("{} {}", shell, shell_args.join(" "));
    let cmd_basenames: Vec<String> = full_cmd
        .split_whitespace()
        .map(|t| executable_basename(t))
        .collect();

    for agent in &settings.agents {
        let agent_exec = agent.command.split_whitespace().next().unwrap_or("");
        let agent_basename = executable_basename(agent_exec);
        if !agent_basename.is_empty() && cmd_basenames.iter().any(|b| *b == agent_basename) {
            log::info!("Auto-detected agent '{}' ({}) from shell command", agent.id, agent.label);
            return (Some(agent.id.clone()), Some(agent.label.clone()));
        }
    }
    (None, None)
}

#[tauri::command]
pub async fn get_active_session(
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
) -> Result<Option<String>, String> {
    let mgr = session_mgr.read().await;
    Ok(mgr.get_active().await.map(|id| id.to_string()))
}
