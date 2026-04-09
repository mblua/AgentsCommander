use std::sync::Arc;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use crate::config::agent_config::{self, AgentLocalConfig};
use crate::config::sessions_persistence::persist_current_state;
use crate::config::settings::{AppSettings, SettingsState};
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::session::SessionInfo;
use crate::telegram::manager::TelegramBridgeState;
use crate::DetachedSessionsState;

/// Core session creation logic shared by the Tauri command and the restore path.
/// Creates a session record, spawns a PTY, and emits the session_created event.
/// Auto-detects agent from shell command if not provided, and auto-injects --continue
/// for Claude when a prior conversation exists (~/.claude/projects/{mangled-cwd}/).
/// If `skip_tooling_save` is true, skips writing to the repo's config.json (for temp sessions).
/// If `skip_continue` is true, suppresses `--continue` auto-injection (used by restart_session
/// to ensure a fresh conversation even when prior history exists).
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
    git_branch_source: Option<String>,
    git_branch_prefix: Option<String>,
    skip_continue: bool,
) -> Result<SessionInfo, String> {
    let mgr = session_mgr.read().await;
    let mut session = mgr
        .create_session(shell.clone(), shell_args.clone(), cwd.clone(), git_branch_source, git_branch_prefix)
        .await
        .map_err(|e| e.to_string())?;

    if let Some(name) = session_name {
        mgr.rename_session(session.id, name.clone())
            .await
            .map_err(|e| e.to_string())?;
        session.name = name;
    }

    let id = session.id;

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
    let is_claude = cmd_basenames.iter().any(|b| b.starts_with("claude"));
    let is_codex = cmd_basenames.iter().any(|b| b.starts_with("codex"));

    // Persist is_claude flag in the SessionManager AND the local clone.
    // The manager update ensures get_session() returns the correct flag (for telegram_attach).
    // The local clone update ensures SessionInfo.is_claude is correct (for auto-attach sites).
    if is_claude {
        mgr.set_is_claude(id, true).await;
        session.is_claude = true;
    }

    // Auto-inject --continue for Claude agents when a prior conversation exists
    // Only if ~/.claude/projects/{mangled-cwd}/ exists (prior conversation exists)
    let claude_project_exists = {
        if let Some(home) = dirs::home_dir() {
            let mangled = crate::session::session::mangle_cwd_for_claude(&cwd);
            home.join(".claude").join("projects").join(&mangled).is_dir()
        } else {
            false
        }
    };
    if is_claude && claude_project_exists && !skip_continue {
        if let Some(ref aid) = agent_id {
            let already_has_continue = full_cmd.split_whitespace().any(|t| {
                let lower = t.to_lowercase();
                lower == "--continue" || lower == "-c"
            });
            if !already_has_continue {
                if executable_basename(&shell) == "cmd" {
                    if let Some(last) = shell_args.last_mut() {
                        if executable_basename(last) == "claude" || last.to_lowercase().contains("claude") {
                            *last = format!("{} --continue", last);
                            log::info!("Auto-injected --continue for agent '{}' (prior conversation exists, cmd path)", aid);
                        }
                    }
                } else {
                    shell_args.push("--continue".to_string());
                    log::info!("Auto-injected --continue for agent '{}' (prior conversation exists)", aid);
                }
            }
        }
    }

    // Auto-inject --append-system-prompt-file for Claude sessions.
    // Replica context (config.json context[]) takes priority over global-only context.
    if is_claude {
        let context_path = match crate::config::session_context::build_replica_context(&cwd) {
            Ok(Some(combined_path)) => {
                log::info!("Using replica combined context for Claude session: {}", combined_path);
                Some(combined_path)
            }
            Ok(None) => {
                // No replica context[] — use global context only
                match crate::config::session_context::ensure_global_context() {
                    Ok(path) => Some(path),
                    Err(e) => {
                        log::warn!("Failed to ensure AgentsCommanderContext.md: {}", e);
                        None
                    }
                }
            }
            Err(e) => {
                // Missing context files — show error dialog and abort launch
                log::error!("Replica context validation failed: {}", e);
                use tauri_plugin_dialog::DialogExt;
                let dialog_msg = format!("Cannot launch session — context files missing:\n\n{}", e);
                // Use non-blocking show() — blocking_show() panics in async context
                app.dialog()
                    .message(&dialog_msg)
                    .title("Context File Error")
                    .show(|_| {});
                // Abort: destroy the session we just created and emit switch if auto-activated
                let mgr2 = session_mgr.read().await;
                if let Ok(Some(new_id)) = mgr2.destroy_session(id).await {
                    let _ = app.emit(
                        "session_switched",
                        serde_json::json!({ "id": new_id.to_string() }),
                    );
                }
                return Err(e);
            }
        };

        if let Some(context_path) = context_path {
            if executable_basename(&shell) == "cmd" {
                if let Some(last) = shell_args.last_mut() {
                    if last.to_lowercase().contains("claude") {
                        *last = format!("{} --append-system-prompt-file \"{}\"", last, context_path);
                        log::info!("Injected --append-system-prompt-file for Claude (cmd path)");
                    }
                }
            } else {
                shell_args.push("--append-system-prompt-file".to_string());
                shell_args.push(context_path);
                log::info!("Injected --append-system-prompt-file for Claude session");
            }
        }
    }

    // Auto-inject developer_instructions for Codex sessions (global user config)
    if is_codex {
        match crate::config::session_context::ensure_codex_context() {
            Ok(()) => {
                log::info!("Injected developer_instructions into ~/.codex/config.toml for Codex session");
            }
            Err(e) => {
                log::warn!("Failed to inject Codex context: {}", e);
            }
        }
    }

    pty_mgr
        .lock()
        .unwrap()
        .spawn(id, &shell, &shell_args, &cwd, 120, 30, app.clone())
        .map_err(|e| e.to_string())?;

    // Auto-inject credentials for agent sessions after PTY spawn.
    // Wait for Claude to become idle (ready for input) instead of fixed delay.
    // Mirrors the pattern in mailbox.rs inject_followup_after_idle_static.
    if agent_id.is_some() {
        let app_clone = app.clone();
        let session_id = id;
        let token = session.token.clone();
        let cwd_clone = cwd.clone();
        tokio::spawn(async move {
            let max_wait = std::time::Duration::from_secs(30);
            let poll = std::time::Duration::from_millis(500);
            let start = std::time::Instant::now();

            loop {
                if start.elapsed() >= max_wait {
                    log::warn!("[session] Timeout waiting for idle before credential injection for session {}", session_id);
                    break; // inject anyway as fallback
                }
                tokio::time::sleep(poll).await;

                let session_mgr = app_clone.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let mgr = session_mgr.read().await;
                let sessions = mgr.list_sessions().await;
                match sessions.iter().find(|s| s.id == session_id.to_string()) {
                    Some(s) if s.waiting_for_input => break, // ready
                    Some(_) => {} // still busy, keep polling
                    None => {
                        log::warn!("[session] Session {} gone before credential injection", session_id);
                        return; // session destroyed, nothing to inject
                    }
                }
            }

            let exe = std::env::current_exe().ok();
            let binary_name = exe.as_ref()
                .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
                .unwrap_or_else(|| "agentscommander".to_string());
            let binary_path = {
                let raw = exe.as_ref()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|| "agentscommander.exe".to_string());
                raw.strip_prefix(r"\\?\").unwrap_or(&raw).to_string()
            };
            let local_dir = exe.as_ref()
                .and_then(|p| p.parent())
                .map(|parent| parent.join(format!(".{}", &binary_name)).to_string_lossy().to_string())
                .unwrap_or_else(|| format!(".{}", &binary_name));
            let local_dir = local_dir.strip_prefix(r"\\?\").unwrap_or(&local_dir).to_string();

            let cred_block = format!(
                concat!(
                    "\n",
                    "# === Session Credentials ===\n",
                    "# Token: {token}\n",
                    "# Root: {root}\n",
                    "# Binary: {binary}\n",
                    "# BinaryPath: {binary_path}\n",
                    "# LocalDir: {local_dir}\n",
                    "# === End Credentials ===\n",
                ),
                token = token,
                root = cwd_clone,
                binary = binary_name,
                binary_path = binary_path,
                local_dir = local_dir,
            );

            match crate::pty::inject::inject_text_into_session(
                &app_clone,
                session_id,
                &cred_block,
                true,
            ).await {
                Ok(()) => {
                    log::info!("[session] Credentials auto-injected for session {}", session_id);
                }
                Err(e) => {
                    log::warn!("[session] Failed to auto-inject credentials for {}: {}", session_id, e);
                }
            }
        });
    }

    let info = SessionInfo::from(&session);
    let _ = app.emit("session_created", info.clone());

    // Show the terminal window when a session is created
    if let Some(win) = app.get_webview_window("terminal") {
        if let Err(e) = win.show() {
            log::warn!("[session] Failed to show terminal window: {}", e);
        }
    }

    // Save lastCodingAgent + codingAgents (skip for temp sessions)
    if !skip_tooling_save {
        if let Some(ref aid) = agent_id {
            // Resolve label: use provided agent_label, or look up from settings by agent_id.
            // Without this fallback, callers that pass agent_id but no label (session-requests,
            // web remote) would write app: "Unknown" into the per-instance config.json.
            let resolved_label = match agent_label.as_deref() {
                Some(l) => l.to_string(),
                None => {
                    let settings = app.state::<SettingsState>();
                    let cfg = settings.read().await;
                    cfg.agents
                        .iter()
                        .find(|a| a.id == *aid)
                        .map(|a| a.label.clone())
                        .unwrap_or_else(|| {
                            log::warn!("Could not resolve label for agent_id='{}' — defaulting to 'Unknown'", aid);
                            "Unknown".to_string()
                        })
                }
            };
            let session_id_str = id.to_string();
            if let Err(e) = agent_config::set_last_coding_agent(&cwd, aid, &resolved_label, Some(&session_id_str)) {
                log::warn!("Failed to save lastCodingAgent: {}", e);
            }
        }
    }

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
    git_branch_source: Option<String>,
    git_branch_prefix: Option<String>,
) -> Result<SessionInfo, String> {
    let cfg = settings.read().await;

    let cwd = cwd.unwrap_or_else(|| {
        dirs::home_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "C:\\".to_string())
    });

    // If agentId provided and shell not explicitly set, use that agent's command
    let (shell, shell_args, agent_label) = match (&shell, &agent_id) {
        (None, Some(aid)) => {
            if let Some(agent) = cfg.agents.iter().find(|a| a.id == *aid) {
                log::info!("[session] Agent resolved: id={:?}, label={:?}, command={:?}", agent.id, agent.label, agent.command);
                let parts: Vec<String> = agent.command.split_whitespace().map(|s| s.to_string()).collect();
                if let Some((cmd, args)) = parts.split_first() {
                    (cmd.clone(), args.to_vec(), Some(agent.label.clone()))
                } else {
                    (cfg.default_shell.clone(), cfg.default_shell_args.clone(), Some(agent.label.clone()))
                }
            } else {
                log::warn!("[session] Agent NOT found for aid={:?}. Falling back to default shell.", aid);
                (cfg.default_shell.clone(), cfg.default_shell_args.clone(), None)
            }
        }
        _ => {
            let s = shell.unwrap_or_else(|| cfg.default_shell.clone());
            let sa = shell_args.unwrap_or_else(|| cfg.default_shell_args.clone());
            let al = agent_id.as_ref().and_then(|aid| {
                cfg.agents.iter().find(|a| a.id == *aid).map(|a| a.label.clone())
            });
            (s, sa, al)
        }
    };

    log::info!("[session] FINAL resolved: shell={:?}, args={:?}, label={:?}", shell, shell_args, agent_label);

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
        git_branch_source,
        git_branch_prefix,
        false, // skip_continue
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
        .join(crate::config::agent_local_dir_name())
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
                    let jsonl_cwd = if info.is_claude { Some(cwd.clone()) } else { None };
                    let mut tg = tg_mgr.lock().await;
                    if let Ok(bridge_info) = tg.attach(id, &bot, pty_arc, app.clone(), jsonl_cwd) {
                        let _ = app.emit("telegram_bridge_attached", bridge_info);
                    }
                }
            }
        }
    }

    Ok(info)
}

/// Core session destruction logic shared by the Tauri command and the MailboxPoller.
/// Kills PTY, detaches Telegram bridge, removes from SessionManager, persists, and emits events.
pub async fn destroy_session_inner(app: &AppHandle, uuid: Uuid) -> Result<(), String> {
    let id = uuid.to_string();

    // Remove from detached set
    {
        let detached = app.state::<DetachedSessionsState>();
        let mut detached_set = detached.lock().unwrap();
        detached_set.remove(&uuid);
    }

    // Auto-detach Telegram bridge if active
    {
        let tg_mgr = app.state::<TelegramBridgeState>();
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
    {
        let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();
        pty_mgr
            .lock()
            .unwrap()
            .kill(uuid)
            .map_err(|e| e.to_string())?;
    }

    let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
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

    // Hide the terminal window when no sessions remain
    if mgr.list_sessions().await.is_empty() {
        if let Some(win) = app.get_webview_window("terminal") {
            if let Err(e) = win.hide() {
                log::warn!("[session] Failed to hide terminal window: {}", e);
            }
        }
    }

    Ok(())
}

#[tauri::command]
pub async fn destroy_session(
    app: AppHandle,
    _session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    _pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    _tg_mgr: State<'_, TelegramBridgeState>,
    _detached: State<'_, DetachedSessionsState>,
    id: String,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    destroy_session_inner(&app, uuid).await
}

/// Restart a session: destroy the existing one and recreate it with the same
/// configuration but a fresh PTY (no `--continue`). The restarted session is
/// automatically activated, Telegram bridges are re-attached, and state is persisted.
#[tauri::command]
pub async fn restart_session(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    tg_mgr: State<'_, TelegramBridgeState>,
    settings: State<'_, SettingsState>,
    id: String,
) -> Result<SessionInfo, String> {
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;

    // 1. Read config from existing session BEFORE destroying it
    let (shell, shell_args, cwd, name, git_branch_source, git_branch_prefix) = {
        let mgr = session_mgr.read().await;
        let session = mgr.get_session(uuid).await.ok_or("Session not found")?;
        (
            session.shell.clone(),
            session.shell_args.clone(),
            session.working_directory.clone(),
            session.name.clone(),
            session.git_branch_source.clone(),
            session.git_branch_prefix.clone(),
        )
    };

    // 2. Strip auto-injected args (--continue + --append-system-prompt-file and its value)
    let clean_args = crate::config::sessions_persistence::strip_auto_injected_args(&shell, &shell_args);

    // 3. Destroy the old session (resolves all State<> internally from app)
    destroy_session_inner(&app, uuid).await?;

    // 4. Create new session with same config, skip_continue = true
    let session_info = create_session_inner(
        &app,
        session_mgr.inner(),
        pty_mgr.inner(),
        shell,
        clean_args,
        cwd.clone(),
        Some(name),
        None,  // agent_id — auto-detection from shell command
        None,  // agent_label — resolved from settings during creation
        false, // skip_tooling_save
        git_branch_source,
        git_branch_prefix,
        true,  // skip_continue — the whole point of restart
    )
    .await?;

    // 5. Explicitly activate the new session.
    //    destroy_session_inner may have auto-activated a sibling.
    //    create_session_inner only auto-activates if active.is_none().
    //    With multiple sessions, the new session would NOT be active without this.
    let new_uuid = Uuid::parse_str(&session_info.id).map_err(|e| e.to_string())?;
    {
        let mgr = session_mgr.read().await;
        let _ = mgr.switch_session(new_uuid).await;
    }
    let _ = app.emit("session_switched", serde_json::json!({ "id": session_info.id }));

    // 6. Re-attach Telegram bridge if the repo config has one
    let config_path = std::path::Path::new(&cwd)
        .join(crate::config::agent_local_dir_name())
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
                    let jsonl_cwd = if session_info.is_claude { Some(cwd.clone()) } else { None };
                    let mut tg = tg_mgr.lock().await;
                    if let Ok(bridge_info) = tg.attach(new_uuid, &bot, pty_arc, app.clone(), jsonl_cwd) {
                        let _ = app.emit("telegram_bridge_attached", bridge_info);
                    }
                }
            }
        }
    }

    // 7. Persist state — create_session_inner does NOT persist
    {
        let mgr = session_mgr.read().await;
        persist_current_state(&mgr).await;
    }

    Ok(session_info)
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
pub(crate) fn executable_basename(s: &str) -> String {
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

/// Create or reuse a root agent session.
/// Derives the root agent path from the current binary name:
///   {exe_dir}/.{binary_name}/ac-root-agent
/// If a session already exists at that path, switches to it instead.
/// Uses the first configured coding agent from settings.
/// Injects session credentials immediately after creation.
#[tauri::command]
pub async fn create_root_agent_session(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    tg_mgr: State<'_, TelegramBridgeState>,
    settings: State<'_, SettingsState>,
) -> Result<SessionInfo, String> {
    // Derive root agent path from binary name
    let exe_path = std::env::current_exe()
        .map_err(|e| format!("Failed to get current exe path: {}", e))?;
    let binary_name = exe_path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("Failed to extract binary name")?
        .to_string();

    let exe_dir = exe_path
        .parent()
        .ok_or("Failed to get exe parent directory")?;
    let root_agent_path = exe_dir
        .join(format!(".{}", binary_name))
        .join("ac-root-agent")
        .to_string_lossy()
        .to_string();

    // Check if a session already exists at this path — reuse it
    {
        let mgr = session_mgr.read().await;
        let sessions = mgr.list_sessions().await;
        if let Some(existing) = sessions.iter().find(|s| s.working_directory == root_agent_path) {
            log::info!("[root-agent] Reusing existing session {} at {}", existing.id, root_agent_path);
            return Ok(existing.clone());
        }
    }

    // Create directory if it doesn't exist
    std::fs::create_dir_all(&root_agent_path)
        .map_err(|e| format!("Failed to create root agent directory: {}", e))?;

    // Get the first configured agent from settings
    let cfg = settings.read().await;
    let (agent_id, shell, shell_args, agent_label) = if let Some(agent) = cfg.agents.first() {
        let parts: Vec<String> = agent.command.split_whitespace().map(|s| s.to_string()).collect();
        if let Some((cmd, args)) = parts.split_first() {
            (Some(agent.id.clone()), cmd.clone(), args.to_vec(), Some(agent.label.clone()))
        } else {
            (None, cfg.default_shell.clone(), cfg.default_shell_args.clone(), None)
        }
    } else {
        (None, cfg.default_shell.clone(), cfg.default_shell_args.clone(), None)
    };
    drop(cfg);

    let info = create_session_inner(
        &app,
        session_mgr.inner(),
        pty_mgr.inner(),
        shell,
        shell_args,
        root_agent_path.clone(),
        Some("Root Agent".to_string()),
        agent_id,
        agent_label,
        false,
        None,
        None,
        false, // skip_continue
    )
    .await?;

    // Persist after creation
    {
        let mgr = session_mgr.read().await;
        persist_current_state(&mgr).await;
    }

    // Auto-attach Telegram bot if configured
    let id = Uuid::parse_str(&info.id).map_err(|e| format!("Invalid session UUID: {}", e))?;
    let config_path = std::path::Path::new(&root_agent_path)
        .join(crate::config::agent_local_dir_name())
        .join("config.json");
    if let Ok(contents) = tokio::fs::read_to_string(&config_path).await {
        if let Ok(local_config) = serde_json::from_str::<AgentLocalConfig>(&contents) {
            if let Some(bot_label) = local_config.tooling.telegram_bot {
                let cfg = settings.read().await;
                let bot = cfg.telegram_bots.iter().find(|b| b.label == bot_label).cloned();
                drop(cfg);
                if let Some(bot) = bot {
                    let pty_arc = pty_mgr.inner().clone();
                    let jsonl_cwd = if info.is_claude { Some(root_agent_path.clone()) } else { None };
                    let mut tg = tg_mgr.lock().await;
                    if let Ok(bridge_info) = tg.attach(id, &bot, pty_arc, app.clone(), jsonl_cwd) {
                        let _ = app.emit("telegram_bridge_attached", bridge_info);
                    }
                }
            }
        }
    }

    // Credentials are auto-injected by create_session_inner for all Claude sessions.

    Ok(info)
}
