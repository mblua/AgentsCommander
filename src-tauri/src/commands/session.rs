use std::sync::Arc;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use crate::config::agent_config::{self, AgentLocalConfig};
use crate::config::sessions_persistence::persist_current_state;
use crate::config::settings::{AppSettings, SettingsState};
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::session::{SessionInfo, SessionRepo};
use crate::telegram::manager::TelegramBridgeState;
use crate::DetachedSessionsState;

fn token_has_unclosed_quote(token: &str, quote: char) -> bool {
    token.chars().filter(|c| *c == quote).count() % 2 == 1
}

fn advance_past_config_value(tokens: &[&str], start: usize) -> usize {
    if start >= tokens.len() {
        return start;
    }

    let mut idx = start;
    let mut in_single = false;
    let mut in_double = false;

    while idx < tokens.len() {
        let token = tokens[idx];
        if token_has_unclosed_quote(token, '\'') {
            in_single = !in_single;
        }
        if token_has_unclosed_quote(token, '"') {
            in_double = !in_double;
        }
        idx += 1;
        if !in_single && !in_double {
            break;
        }
    }

    idx
}

fn codex_option_takes_value(token: &str) -> bool {
    matches!(
        token.to_ascii_lowercase().as_str(),
        "-c" | "--config"
            | "--enable"
            | "--disable"
            | "--remote"
            | "--remote-auth-token-env"
            | "-i"
            | "--image"
            | "-m"
            | "--model"
            | "--local-provider"
            | "-p"
            | "--profile"
            | "-s"
            | "--sandbox"
            | "-a"
            | "--ask-for-approval"
            | "--cd"
            | "--add-dir"
    )
}

fn codex_has_explicit_subcommand(tokens: &[&str], start: usize) -> bool {
    const CODEX_SUBCOMMANDS: &[&str] = &[
        "exec",
        "e",
        "review",
        "login",
        "logout",
        "mcp",
        "marketplace",
        "mcp-server",
        "app-server",
        "completion",
        "sandbox",
        "debug",
        "apply",
        "a",
        "resume",
        "fork",
        "cloud",
        "exec-server",
        "features",
        "help",
    ];

    let mut idx = start;
    while idx < tokens.len() {
        let token = tokens[idx];
        if token.eq_ignore_ascii_case("-c") || token.eq_ignore_ascii_case("--config") {
            idx = advance_past_config_value(tokens, idx + 1);
            continue;
        }
        if codex_option_takes_value(token) {
            idx += 2;
            continue;
        }
        if token.starts_with('-') {
            idx += 1;
            continue;
        }
        return CODEX_SUBCOMMANDS
            .iter()
            .any(|subcommand| token.eq_ignore_ascii_case(subcommand));
    }

    false
}

fn codex_tokens_have_resume(tokens: &[&str], start: usize) -> bool {
    let mut idx = start;
    while idx < tokens.len() {
        let token = tokens[idx];
        if token.eq_ignore_ascii_case("-c") || token.eq_ignore_ascii_case("--config") {
            idx = advance_past_config_value(tokens, idx + 1);
            continue;
        }
        if token.eq_ignore_ascii_case("resume") || token.eq_ignore_ascii_case("--last") {
            return true;
        }
        idx += 1;
    }
    false
}

fn inject_codex_resume(shell: &str, shell_args: &mut Vec<String>) -> bool {
    match executable_basename(shell).as_str() {
        "codex" => {
            let tokens: Vec<&str> = shell_args.iter().map(|arg| arg.as_str()).collect();
            if codex_tokens_have_resume(&tokens, 0) || codex_has_explicit_subcommand(&tokens, 0) {
                return false;
            }
            shell_args.insert(0, "resume".to_string());
            shell_args.insert(1, "--last".to_string());
            true
        }
        "cmd" => {
            if let Some(idx) = shell_args
                .iter()
                .position(|arg| executable_basename(arg) == "codex")
            {
                let tokens: Vec<&str> = shell_args.iter().map(|arg| arg.as_str()).collect();
                if codex_tokens_have_resume(&tokens, idx + 1)
                    || codex_has_explicit_subcommand(&tokens, idx + 1)
                {
                    return false;
                }
                shell_args.insert(idx + 1, "resume".to_string());
                shell_args.insert(idx + 2, "--last".to_string());
                return true;
            }

            for arg in shell_args.iter_mut() {
                let mut tokens: Vec<String> = arg
                    .split_whitespace()
                    .map(|token| token.to_string())
                    .collect();
                if let Some(idx) = tokens
                    .iter()
                    .position(|token| executable_basename(token) == "codex")
                {
                    let token_refs: Vec<&str> = tokens.iter().map(|token| token.as_str()).collect();
                    if codex_tokens_have_resume(&token_refs, idx + 1)
                        || codex_has_explicit_subcommand(&token_refs, idx + 1)
                    {
                        return false;
                    }
                    tokens.insert(idx + 1, "resume".to_string());
                    tokens.insert(idx + 2, "--last".to_string());
                    *arg = tokens.join(" ");
                    return true;
                }
            }

            false
        }
        _ => false,
    }
}

/// Core session creation logic shared by the Tauri command and the restore path.
/// Creates a session record, spawns a PTY, and emits the session_created event.
/// Auto-detects agent from shell command if not provided, and auto-injects provider-specific
/// resume flags (`claude --continue`, `codex resume --last`) when appropriate.
/// If `skip_tooling_save` is true, skips writing to the repo's config.json (for temp sessions).
/// If `skip_auto_resume` is true, suppresses provider-specific auto-resume injection (used by
/// restart_session to ensure a fresh conversation even when prior history exists).
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
    git_repos: Vec<SessionRepo>,
    skip_auto_resume: bool,
) -> Result<SessionInfo, String> {
    let (agent_id, agent_label) = {
        let settings_state = app.state::<SettingsState>();
        let cfg = settings_state.read().await;
        resolve_actual_agent(
            &shell,
            &shell_args,
            agent_id.as_deref(),
            agent_label.as_deref(),
            &cfg,
        )
    };

    // Recompute is_coordinator from the current team snapshot. One source of truth —
    // every caller of create_session_inner gets the same computation.
    let teams = crate::config::teams::discover_teams();
    let is_coordinator = crate::config::teams::is_coordinator_for_cwd(&cwd, &teams);

    let mgr = session_mgr.read().await;
    let mut session = mgr
        .create_session(
            shell.clone(),
            shell_args.clone(),
            cwd.clone(),
            agent_id.clone(),
            agent_label.clone(),
            git_repos,
            is_coordinator,
        )
        .await
        .map_err(|e| e.to_string())?;

    if let Some(name) = session_name {
        mgr.rename_session(session.id, name.clone())
            .await
            .map_err(|e| e.to_string())?;
        session.name = name;
    }

    let id = session.id;

    // Detect coding agent families so we can materialize provider-specific context files.
    let mut shell_args = shell_args;
    let full_cmd = format!("{} {}", shell, shell_args.join(" "));
    let cmd_basenames: Vec<String> = full_cmd
        .split_whitespace()
        .map(|t| executable_basename(t))
        .collect();
    let is_claude = cmd_basenames.iter().any(|b| b.starts_with("claude"));
    let is_codex = cmd_basenames.iter().any(|b| b.starts_with("codex"));
    let is_gemini = cmd_basenames.iter().any(|b| b.starts_with("gemini"));
    let context_target = if is_claude {
        Some(crate::config::session_context::ManagedContextTarget::Claude)
    } else if is_codex {
        Some(crate::config::session_context::ManagedContextTarget::Codex)
    } else if is_gemini {
        Some(crate::config::session_context::ManagedContextTarget::Gemini)
    } else {
        None
    };

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
            home.join(".claude")
                .join("projects")
                .join(&mangled)
                .is_dir()
        } else {
            false
        }
    };
    if is_claude && claude_project_exists && !skip_auto_resume {
        if let Some(ref aid) = agent_id {
            let already_has_continue = full_cmd.split_whitespace().any(|t| {
                let lower = t.to_lowercase();
                lower == "--continue" || lower == "-c"
            });
            if !already_has_continue {
                if executable_basename(&shell) == "cmd" {
                    if let Some(last) = shell_args.last_mut() {
                        if executable_basename(last) == "claude"
                            || last.to_lowercase().contains("claude")
                        {
                            *last = format!("{} --continue", last);
                            log::info!("Auto-injected --continue for agent '{}' (prior conversation exists, cmd path)", aid);
                        }
                    }
                } else {
                    shell_args.push("--continue".to_string());
                    log::info!(
                        "Auto-injected --continue for agent '{}' (prior conversation exists)",
                        aid
                    );
                }
            }
        }
    }

    if is_codex && !skip_auto_resume {
        if let Some(ref aid) = agent_id {
            if inject_codex_resume(&shell, &mut shell_args) {
                log::info!("Auto-injected `codex resume --last` for agent '{}'", aid);
            }
        }
    }

    let materialized_context_path = if let Some(target) = context_target {
        match crate::config::session_context::materialize_agent_context_file(&cwd, target) {
            Ok(context) => context,
            Err(e) => {
                log::error!("Replica context validation failed: {}", e);
                use tauri_plugin_dialog::DialogExt;
                let dialog_msg = format!("Cannot launch session — context files missing:\n\n{}", e);
                app.dialog()
                    .message(&dialog_msg)
                    .title("Context File Error")
                    .show(|_| {});
                let mgr2 = session_mgr.read().await;
                if let Ok(Some(new_id)) = mgr2.destroy_session(id).await {
                    let _ = app.emit(
                        "session_switched",
                        serde_json::json!({ "id": new_id.to_string() }),
                    );
                }
                return Err(e);
            }
        }
    } else {
        None
    };

    // Claude consumes the materialized CLAUDE.md via --append-system-prompt-file.
    if is_claude {
        if let Some(context_path) = materialized_context_path.as_ref() {
            if executable_basename(&shell) == "cmd" {
                if let Some(last) = shell_args.last_mut() {
                    if last.to_lowercase().contains("claude") {
                        *last =
                            format!("{} --append-system-prompt-file \"{}\"", last, context_path);
                        log::info!("Injected --append-system-prompt-file for Claude (cmd path)");
                    }
                }
            } else {
                shell_args.push("--append-system-prompt-file".to_string());
                shell_args.push(context_path.to_string());
                log::info!("Injected --append-system-prompt-file for Claude session");
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
                    Some(_) => {}                            // still busy, keep polling
                    None => {
                        log::warn!(
                            "[session] Session {} gone before credential injection",
                            session_id
                        );
                        return; // session destroyed, nothing to inject
                    }
                }
            }

            let cred_block =
                crate::pty::credentials::build_credentials_block(&token, &cwd_clone);

            match crate::pty::inject::inject_text_into_session(
                &app_clone,
                session_id,
                &cred_block,
                true,
            )
            .await
            {
                Ok(()) => {
                    log::info!(
                        "[session] Credentials auto-injected for session {}",
                        session_id
                    );
                }
                Err(e) => {
                    log::warn!(
                        "[session] Failed to auto-inject credentials for {}: {}",
                        session_id,
                        e
                    );
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
                    resolve_agent_label(aid, &cfg).unwrap_or_else(|| {
                        log::warn!(
                            "Could not resolve label for agent_id='{}' — defaulting to 'Unknown'",
                            aid
                        );
                        "Unknown".to_string()
                    })
                }
            };
            let session_id_str = id.to_string();
            if let Err(e) = agent_config::set_last_coding_agent(
                &cwd,
                aid,
                &resolved_label,
                Some(&session_id_str),
            ) {
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
    git_repos: Option<Vec<SessionRepo>>,
) -> Result<SessionInfo, String> {
    let cfg = settings.read().await;

    let cwd = cwd.unwrap_or_else(|| {
        dirs::home_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "C:\\".to_string())
    });

    // If agentId provided and shell not explicitly set, use that agent's command
    let (shell, shell_args, agent_label) = match (&shell, &agent_id) {
        (None, Some(aid)) => resolve_agent_command(aid, &cfg),
        _ => {
            let s = shell.unwrap_or_else(|| cfg.default_shell.clone());
            let sa = shell_args.unwrap_or_else(|| cfg.default_shell_args.clone());
            let al = agent_id.as_ref().and_then(|aid| {
                cfg.agents
                    .iter()
                    .find(|a| a.id == *aid)
                    .map(|a| a.label.clone())
            });
            (s, sa, al)
        }
    };

    log::info!(
        "[session] FINAL resolved: shell={:?}, args={:?}, label={:?}",
        shell,
        shell_args,
        agent_label
    );

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
        git_repos.unwrap_or_default(),
        false, // skip_auto_resume
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
                    let jsonl_cwd = if info.is_claude {
                        Some(cwd.clone())
                    } else {
                        None
                    };
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
    let new_active = mgr.destroy_session(uuid).await.map_err(|e| e.to_string())?;

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
/// configuration but a fresh PTY (no provider auto-resume). The restarted session is
/// automatically activated, Telegram bridges are re-attached, and state is persisted.
#[tauri::command]
pub async fn restart_session(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    tg_mgr: State<'_, TelegramBridgeState>,
    settings: State<'_, SettingsState>,
    id: String,
    agent_id: Option<String>,
) -> Result<SessionInfo, String> {
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;

    // 1. Read config from existing session BEFORE destroying it
    let (
        shell,
        shell_args,
        cwd,
        name,
        stored_agent_id,
        stored_agent_label,
        git_repos,
    ) = {
        let mgr = session_mgr.read().await;
        let session = mgr.get_session(uuid).await.ok_or("Session not found")?;
        (
            session.shell.clone(),
            session.shell_args.clone(),
            session.working_directory.clone(),
            session.name.clone(),
            session.agent_id.clone(),
            session.agent_label.clone(),
            session.git_repos.clone(),
        )
    };

    // 2. Strip auto-injected args before restart so the new session starts from the saved recipe.
    let clean_args =
        crate::config::sessions_persistence::strip_auto_injected_args(&shell, &shell_args);

    let requested_agent_id = agent_id;
    let (shell, shell_args, agent_label) = if let Some(ref aid) = requested_agent_id {
        let cfg = settings.read().await;
        let resolved = resolve_agent_command(aid, &cfg);
        drop(cfg);
        resolved
    } else {
        (shell, clean_args, stored_agent_label)
    };

    // 3. Destroy the old session (resolves all State<> internally from app)
    destroy_session_inner(&app, uuid).await?;

    // 4. Create new session with same config, or switch to the selected coding agent.
    let session_info = create_session_inner(
        &app,
        session_mgr.inner(),
        pty_mgr.inner(),
        shell,
        shell_args,
        cwd.clone(),
        Some(name),
        requested_agent_id.or(stored_agent_id),
        agent_label,
        false, // skip_tooling_save
        git_repos,
        true, // skip_auto_resume — the whole point of restart
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
    let _ = app.emit(
        "session_switched",
        serde_json::json!({ "id": session_info.id }),
    );

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
                    let jsonl_cwd = if session_info.is_claude {
                        Some(cwd.clone())
                    } else {
                        None
                    };
                    let mut tg = tg_mgr.lock().await;
                    if let Ok(bridge_info) =
                        tg.attach(new_uuid, &bot, pty_arc, app.clone(), jsonl_cwd)
                    {
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
    mgr.switch_session(uuid).await.map_err(|e| e.to_string())?;

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

fn resolve_agent_command(
    agent_id: &str,
    settings: &AppSettings,
) -> (String, Vec<String>, Option<String>) {
    if let Some(agent) = settings.agents.iter().find(|a| a.id == agent_id) {
        log::info!(
            "[session] Agent resolved: id={:?}, label={:?}, command={:?}",
            agent.id,
            agent.label,
            agent.command
        );
        let parts: Vec<String> = agent
            .command
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();
        if let Some((cmd, args)) = parts.split_first() {
            (cmd.clone(), args.to_vec(), Some(agent.label.clone()))
        } else {
            (
                settings.default_shell.clone(),
                settings.default_shell_args.clone(),
                Some(agent.label.clone()),
            )
        }
    } else {
        log::warn!(
            "[session] Agent NOT found for aid={:?}. Falling back to default shell.",
            agent_id
        );
        (
            settings.default_shell.clone(),
            settings.default_shell_args.clone(),
            None,
        )
    }
}

fn resolve_agent_label(agent_id: &str, settings: &AppSettings) -> Option<String> {
    settings
        .agents
        .iter()
        .find(|a| a.id == agent_id)
        .map(|a| a.label.clone())
}

fn resolve_actual_agent(
    shell: &str,
    shell_args: &[String],
    requested_agent_id: Option<&str>,
    requested_agent_label: Option<&str>,
    settings: &AppSettings,
) -> (Option<String>, Option<String>) {
    let detected = resolve_agent_from_shell(shell, shell_args, settings);

    if let Some(agent_id) = requested_agent_id {
        match detected.0.as_deref() {
            Some(detected_id) if detected_id == agent_id => {
                return (
                    detected.0,
                    requested_agent_label
                        .map(ToString::to_string)
                        .or(detected.1),
                )
            }
            Some(detected_id) => {
                log::warn!(
                    "[session] Requested agent_id='{}' did not match final shell-resolved agent '{}'; storing resolved agent instead",
                    agent_id,
                    detected_id
                );
                return detected;
            }
            None => {
                log::warn!(
                    "[session] Requested agent_id='{}' did not validate against final launched shell; clearing actual agent metadata",
                    agent_id
                );
                return (None, None);
            }
        }
    }

    detected
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
            log::info!(
                "Auto-detected agent '{}' ({}) from shell command",
                agent.id,
                agent.label
            );
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
    let exe_path =
        std::env::current_exe().map_err(|e| format!("Failed to get current exe path: {}", e))?;
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
        if let Some(existing) = sessions
            .iter()
            .find(|s| s.working_directory == root_agent_path)
        {
            log::info!(
                "[root-agent] Reusing existing session {} at {}",
                existing.id,
                root_agent_path
            );
            return Ok(existing.clone());
        }
    }

    // Create directory if it doesn't exist
    std::fs::create_dir_all(&root_agent_path)
        .map_err(|e| format!("Failed to create root agent directory: {}", e))?;

    // Get the first configured agent from settings
    let cfg = settings.read().await;
    let (agent_id, shell, shell_args, agent_label) = if let Some(agent) = cfg.agents.first() {
        let parts: Vec<String> = agent
            .command
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();
        if let Some((cmd, args)) = parts.split_first() {
            (
                Some(agent.id.clone()),
                cmd.clone(),
                args.to_vec(),
                Some(agent.label.clone()),
            )
        } else {
            (
                None,
                cfg.default_shell.clone(),
                cfg.default_shell_args.clone(),
                None,
            )
        }
    } else {
        (
            None,
            cfg.default_shell.clone(),
            cfg.default_shell_args.clone(),
            None,
        )
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
        Vec::new(),
        false, // skip_auto_resume
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
                let bot = cfg
                    .telegram_bots
                    .iter()
                    .find(|b| b.label == bot_label)
                    .cloned();
                drop(cfg);
                if let Some(bot) = bot {
                    let pty_arc = pty_mgr.inner().clone();
                    let jsonl_cwd = if info.is_claude {
                        Some(root_agent_path.clone())
                    } else {
                        None
                    };
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

#[cfg(test)]
mod tests {
    use super::{inject_codex_resume, resolve_actual_agent};
    use crate::config::settings::{AgentConfig, AppSettings};

    fn test_settings() -> AppSettings {
        let mut settings = AppSettings::default();
        settings.agents = vec![
            AgentConfig {
                id: "claude".to_string(),
                label: "Claude Code".to_string(),
                command: "claude".to_string(),
                color: "#d97706".to_string(),
                git_pull_before: false,
                exclude_global_claude_md: false,
            },
            AgentConfig {
                id: "codex".to_string(),
                label: "Codex".to_string(),
                command: "codex".to_string(),
                color: "#10b981".to_string(),
                git_pull_before: false,
                exclude_global_claude_md: false,
            },
        ];
        settings
    }

    #[test]
    fn inject_codex_resume_prefixes_direct_codex_args() {
        let mut args = vec![
            "-m".to_string(),
            "gpt-5".to_string(),
            "-c".to_string(),
            "model_reasoning_effort=\"high\"".to_string(),
        ];

        assert!(inject_codex_resume("codex", &mut args));
        assert_eq!(
            args,
            vec![
                "resume".to_string(),
                "--last".to_string(),
                "-m".to_string(),
                "gpt-5".to_string(),
                "-c".to_string(),
                "model_reasoning_effort=\"high\"".to_string(),
            ]
        );
    }

    #[test]
    fn inject_codex_resume_inserts_into_cmd_tokenized_wrapper() {
        let mut args = vec![
            "/C".to_string(),
            "codex".to_string(),
            "-m".to_string(),
            "gpt-5".to_string(),
        ];

        assert!(inject_codex_resume("cmd.exe", &mut args));
        assert_eq!(
            args,
            vec![
                "/C".to_string(),
                "codex".to_string(),
                "resume".to_string(),
                "--last".to_string(),
                "-m".to_string(),
                "gpt-5".to_string(),
            ]
        );
    }

    #[test]
    fn inject_codex_resume_inserts_into_embedded_cmd_wrapper() {
        let mut args = vec!["/K".to_string(), "git pull && codex -m gpt-5".to_string()];

        assert!(inject_codex_resume("cmd.exe", &mut args));
        assert_eq!(
            args,
            vec![
                "/K".to_string(),
                "git pull && codex resume --last -m gpt-5".to_string(),
            ]
        );
    }

    #[test]
    fn inject_codex_resume_skips_existing_resume_tokens() {
        let mut args = vec![
            "resume".to_string(),
            "--last".to_string(),
            "-m".to_string(),
            "gpt-5".to_string(),
        ];

        assert!(!inject_codex_resume("codex", &mut args));
        assert_eq!(
            args,
            vec![
                "resume".to_string(),
                "--last".to_string(),
                "-m".to_string(),
                "gpt-5".to_string(),
            ]
        );
    }

    #[test]
    fn inject_codex_resume_skips_explicit_fork_subcommand() {
        let mut args = vec!["fork".to_string(), "--last".to_string()];

        assert!(!inject_codex_resume("codex", &mut args));
        assert_eq!(args, vec!["fork".to_string(), "--last".to_string()]);
    }

    #[test]
    fn inject_codex_resume_skips_explicit_exec_subcommand_after_options() {
        let mut args = vec![
            "-m".to_string(),
            "gpt-5".to_string(),
            "exec".to_string(),
            "--json".to_string(),
        ];

        assert!(!inject_codex_resume("codex", &mut args));
        assert_eq!(
            args,
            vec![
                "-m".to_string(),
                "gpt-5".to_string(),
                "exec".to_string(),
                "--json".to_string(),
            ]
        );
    }

    #[test]
    fn inject_codex_resume_skips_explicit_help_subcommand_in_cmd_wrapper() {
        let mut args = vec!["/C".to_string(), "codex".to_string(), "help".to_string()];

        assert!(!inject_codex_resume("cmd.exe", &mut args));
        assert_eq!(
            args,
            vec!["/C".to_string(), "codex".to_string(), "help".to_string()]
        );
    }

    #[test]
    fn inject_codex_resume_ignores_resume_text_inside_config_value() {
        let mut args = vec![
            "-c".to_string(),
            "instruction=\"resume later\"".to_string(),
            "--search".to_string(),
        ];

        assert!(inject_codex_resume("codex", &mut args));
        assert_eq!(
            args,
            vec![
                "resume".to_string(),
                "--last".to_string(),
                "-c".to_string(),
                "instruction=\"resume later\"".to_string(),
                "--search".to_string(),
            ]
        );
    }

    #[test]
    fn resolve_actual_agent_keeps_requested_agent_when_shell_validates_it() {
        let settings = test_settings();

        let resolved = resolve_actual_agent(
            "codex",
            &["-m".to_string(), "gpt-5".to_string()],
            Some("codex"),
            Some("Codex Stable"),
            &settings,
        );

        assert_eq!(
            resolved,
            (Some("codex".to_string()), Some("Codex Stable".to_string()))
        );
    }

    #[test]
    fn resolve_actual_agent_falls_back_to_detected_label_when_validated_match_has_no_stored_label() {
        let settings = test_settings();

        let resolved = resolve_actual_agent(
            "codex",
            &["-m".to_string(), "gpt-5".to_string()],
            Some("codex"),
            None,
            &settings,
        );

        assert_eq!(
            resolved,
            (Some("codex".to_string()), Some("Codex".to_string()))
        );
    }

    #[test]
    fn resolve_actual_agent_clears_requested_agent_when_shell_is_unresolved() {
        let settings = test_settings();

        let resolved = resolve_actual_agent(
            "powershell.exe",
            &["-NoLogo".to_string()],
            Some("codex"),
            Some("Codex"),
            &settings,
        );

        assert_eq!(resolved, (None, None));
    }

    #[test]
    fn resolve_actual_agent_uses_shell_resolved_agent_on_mismatch() {
        let settings = test_settings();

        let resolved = resolve_actual_agent(
            "claude",
            &[],
            Some("codex"),
            Some("Codex"),
            &settings,
        );

        assert_eq!(
            resolved,
            (Some("claude".to_string()), Some("Claude Code".to_string()))
        );
    }
}
