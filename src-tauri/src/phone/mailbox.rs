use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::Manager;
use uuid::Uuid;

use crate::config::dark_factory::{self, DarkFactoryConfig};
use crate::config::settings::SettingsState;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::session::SessionStatus;

/// Message format in outbox files. All new fields are Option/default for backwards compat.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboxMessage {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    pub from: String,
    pub to: String,
    pub body: String,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub get_output: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_agent: Option<String>,
    #[serde(default)]
    pub preferred_agent: String,
    #[serde(default)]
    pub priority: String,
    pub timestamp: String,
}

/// The MailboxPoller runs as a background tokio task. It polls outbox directories
/// for all known agent repos, validates messages, and delivers them according to mode.
pub struct MailboxPoller {
    poll_interval: std::time::Duration,
}

impl MailboxPoller {
    pub fn new() -> Self {
        Self {
            poll_interval: std::time::Duration::from_secs(3),
        }
    }

    /// Start the poller as a background task.
    pub fn start(self, app: tauri::AppHandle) {
        tauri::async_runtime::spawn(async move {
            loop {
                if let Err(e) = self.poll(&app).await {
                    log::warn!("MailboxPoller error: {}", e);
                }
                tokio::time::sleep(self.poll_interval).await;
            }
        });
    }

    /// One poll cycle: scan all repo outbox dirs, process each message.
    async fn poll(&self, app: &tauri::AppHandle) -> Result<(), String> {
        let settings = app.state::<SettingsState>();
        let repo_paths = {
            let cfg = settings.read().await;
            cfg.repo_paths.clone()
        };

        // Also scan CWDs of active sessions for repos not in settings
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let session_dirs = {
            let mgr = session_mgr.read().await;
            mgr.get_sessions_directories().await
        };

        let mut all_paths: Vec<String> = repo_paths;
        for (_, dir) in &session_dirs {
            if !all_paths.contains(dir) {
                all_paths.push(dir.clone());
            }
        }

        for repo_path in &all_paths {
            let outbox_dir = Path::new(repo_path).join(".agentscommander").join("outbox");
            if !outbox_dir.is_dir() {
                continue;
            }

            let entries: Vec<PathBuf> = match std::fs::read_dir(&outbox_dir) {
                Ok(rd) => rd
                    .filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("json"))
                    .filter(|p| {
                        // Skip files in subdirectories (delivered/, rejected/)
                        p.parent() == Some(outbox_dir.as_path())
                    })
                    .collect(),
                Err(_) => continue,
            };

            for path in entries {
                if let Err(e) = self.process_message(app, &path).await {
                    log::warn!("Failed to process outbox message {:?}: {}", path, e);
                }
            }
        }

        Ok(())
    }

    /// Process a single outbox message file.
    async fn process_message(&self, app: &tauri::AppHandle, path: &Path) -> Result<(), String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read outbox file: {}", e))?;

        let msg: OutboxMessage = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse outbox message: {}", e))?;

        log::info!(
            "[mailbox] Processing message {} from='{}' to='{}' mode='{}'",
            msg.id, msg.from, msg.to, msg.mode
        );

        // Validate token if present
        if let Some(ref token_str) = msg.token {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;

            if let Ok(token_uuid) = Uuid::parse_str(token_str) {
                if mgr.find_by_token(token_uuid).await.is_none() {
                    // Invalid token — reject
                    return self.reject_message(path, &msg, "Invalid session token").await;
                }
            } else {
                return self.reject_message(path, &msg, "Malformed token").await;
            }
        }

        // Validate peer visibility
        let dark_factory = dark_factory::load_dark_factory();
        if !self.can_reach(&msg.from, &msg.to, &dark_factory) {
            return self.reject_message(path, &msg, "Sender cannot reach destination").await;
        }

        // Deliver based on mode
        let mode = if msg.mode.is_empty() { "queue" } else { msg.mode.as_str() };
        match mode {
            "queue" => self.deliver_queue(app, &msg).await?,
            "active-only" => self.deliver_active_only(app, &msg).await?,
            "wake" => self.deliver_wake(app, &msg).await?,
            "wake-and-sleep" => self.deliver_wake_and_sleep(app, &msg).await?,
            _ => {
                log::warn!("Unknown delivery mode '{}', falling back to queue", mode);
                self.deliver_queue(app, &msg).await?;
            }
        }

        // Move to delivered/ with token stripped
        self.move_to_delivered(path, &msg).await
    }

    /// Deliver mode: queue — write to destination's inbox directory.
    async fn deliver_queue(&self, app: &tauri::AppHandle, msg: &OutboxMessage) -> Result<(), String> {
        log::info!("[mailbox] Delivering {} via QUEUE to {}", msg.id, msg.to);
        let dest_inbox = self.resolve_inbox_dir(&msg.to, app).await?;
        std::fs::create_dir_all(&dest_inbox)
            .map_err(|e| format!("Failed to create inbox dir: {}", e))?;

        let inbox_path = dest_inbox.join(format!("{}.json", msg.id));
        let json = serde_json::to_string_pretty(msg)
            .map_err(|e| format!("Failed to serialize message: {}", e))?;
        std::fs::write(&inbox_path, json)
            .map_err(|e| format!("Failed to write inbox message: {}", e))?;

        log::info!("Delivered message {} to {} (queue)", msg.id, msg.to);
        let _ = tauri::Emitter::emit(
            app,
            "message_delivered",
            serde_json::json!({
                "id": msg.id,
                "from": msg.from,
                "to": msg.to,
                "mode": "queue"
            }),
        );
        Ok(())
    }

    /// Deliver mode: active-only — inject into PTY if agent is active and not idle, else queue.
    async fn deliver_active_only(&self, app: &tauri::AppHandle, msg: &OutboxMessage) -> Result<(), String> {
        if let Some(session_id) = self.find_active_session(app, &msg.to).await {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            let sessions = mgr.list_sessions().await;
            let session = sessions.iter().find(|s| s.id == session_id.to_string());

            if let Some(s) = session {
                log::info!(
                    "[mailbox] active-only: session {} status={:?} waiting_for_input={}",
                    session_id, s.status, s.waiting_for_input
                );
                // Only deliver if session is active/running and NOT waiting for input
                if !s.waiting_for_input && matches!(s.status, SessionStatus::Active | SessionStatus::Running) {
                    return self.inject_into_pty(app, session_id, msg, true).await;
                }
                log::info!("[mailbox] active-only: conditions not met, falling back to queue");
            }
        }
        // Fallback to queue
        self.deliver_queue(app, msg).await
    }

    /// Deliver mode: wake — inject into PTY if agent is idle (waiting for input), else queue.
    async fn deliver_wake(&self, app: &tauri::AppHandle, msg: &OutboxMessage) -> Result<(), String> {
        if let Some(session_id) = self.find_active_session(app, &msg.to).await {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            let sessions = mgr.list_sessions().await;
            let session = sessions.iter().find(|s| s.id == session_id.to_string());

            if let Some(s) = session {
                log::info!(
                    "[mailbox] wake: session {} status={:?} waiting_for_input={}",
                    session_id, s.status, s.waiting_for_input
                );
                if s.waiting_for_input {
                    return self.inject_into_pty(app, session_id, msg, true).await;
                }
                log::info!("[mailbox] wake: session not idle, falling back to queue");
            } else {
                log::warn!("[mailbox] wake: session {} not in list_sessions", session_id);
            }
        }
        // Fallback to queue
        self.deliver_queue(app, msg).await
    }

    /// Deliver mode: wake-and-sleep — spawn temporary session if needed, inject, wait for idle, kill.
    async fn deliver_wake_and_sleep(&self, app: &tauri::AppHandle, msg: &OutboxMessage) -> Result<(), String> {
        // Check if there's already an active session for this destination
        if let Some(session_id) = self.find_active_session(app, &msg.to).await {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            let sessions = mgr.list_sessions().await;
            let session = sessions.iter().find(|s| s.id == session_id.to_string());

            if let Some(s) = session {
                if s.waiting_for_input {
                    // Existing session is interactive — no response markers
                    return self.inject_into_pty(app, session_id, msg, true).await;
                }
            }
            // Session exists but is busy — queue instead
            return self.deliver_queue(app, msg).await;
        }

        // No active session — need to spawn a temporary one.
        // Determine which agent CLI to use.
        let agent_command = self.resolve_agent_command(app, msg).await;

        if let Some((shell, shell_args)) = agent_command {
            let dest_path = self.resolve_repo_path(&msg.to, app).await;
            let cwd = dest_path.unwrap_or_else(|| msg.to.clone());

            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();

            match crate::commands::session::create_session_inner(
                app,
                session_mgr.inner(),
                pty_mgr.inner(),
                shell,
                shell_args,
                cwd,
                Some(format!("[temp] {}", msg.to)),
                None, // Temp session — don't update lastCodingAgent
            )
            .await
            {
                Ok(info) => {
                    let session_id = Uuid::parse_str(&info.id)
                        .map_err(|e| format!("Failed to parse session id: {}", e))?;

                    // Wait for agent boot (3s matches init prompt delay)
                    tokio::time::sleep(std::time::Duration::from_secs(4)).await;

                    // Inject the message — non-interactive one-shot, use markers if get_output
                    self.inject_into_pty(app, session_id, msg, false).await?;

                    // Schedule cleanup: wait for idle then destroy session
                    let app_clone = app.clone();
                    let session_id_clone = session_id;
                    tauri::async_runtime::spawn(async move {
                        // Wait up to 10 minutes for the agent to finish
                        let timeout = std::time::Duration::from_secs(600);
                        let poll = std::time::Duration::from_secs(2);
                        let start = std::time::Instant::now();

                        loop {
                            if start.elapsed() >= timeout {
                                log::warn!("wake-and-sleep timeout for session {}", session_id_clone);
                                break;
                            }

                            let session_mgr = app_clone
                                .state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                            let mgr = session_mgr.read().await;
                            let sessions = mgr.list_sessions().await;

                            if let Some(s) = sessions.iter().find(|s| s.id == session_id_clone.to_string()) {
                                if s.waiting_for_input {
                                    // Agent is done — cleanup
                                    break;
                                }
                            } else {
                                // Session already gone
                                return;
                            }
                            drop(mgr);
                            tokio::time::sleep(poll).await;
                        }

                        // Destroy the temporary session
                        let pty_mgr = app_clone.state::<Arc<Mutex<PtyManager>>>();
                        if let Ok(mgr) = pty_mgr.lock() {
                            let _ = mgr.kill(session_id_clone);
                        }

                        let session_mgr = app_clone
                            .state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                        let mgr = session_mgr.read().await;
                        let _ = mgr.destroy_session(session_id_clone).await;

                        let _ = tauri::Emitter::emit(
                            &app_clone,
                            "session_destroyed",
                            serde_json::json!({ "id": session_id_clone.to_string() }),
                        );
                        log::info!("wake-and-sleep: destroyed temp session {}", session_id_clone);
                    });

                    Ok(())
                }
                Err(e) => {
                    log::warn!("wake-and-sleep: failed to spawn temp session: {}", e);
                    // Fallback to queue
                    self.deliver_queue(app, msg).await
                }
            }
        } else {
            // No agent command resolved — queue
            log::warn!("wake-and-sleep: no agent command found for {}, falling back to queue", msg.to);
            self.deliver_queue(app, msg).await
        }
    }

    /// Inject a message into a session's PTY stdin.
    /// `interactive` = true means the session is a live interactive agent (wake/active-only).
    ///   → plain message only, no response markers, no watcher.
    /// `interactive` = false means it's a non-interactive one-shot (wake-and-sleep).
    ///   → if get_output, include response marker instructions and register watcher.
    async fn inject_into_pty(
        &self,
        app: &tauri::AppHandle,
        session_id: Uuid,
        msg: &OutboxMessage,
        interactive: bool,
    ) -> Result<(), String> {
        let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();

        // Only use response markers for non-interactive sessions
        let use_markers = msg.get_output && !interactive;

        // Resolve binary path for reply instructions
        let bin_path = std::env::current_exe()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "agentscommander".to_string());

        let payload = if use_markers {
            if let Some(ref rid) = msg.request_id {
                format!(
                    "\n[Message from {}] {}\n(Reply between markers: %%AC_RESPONSE::{}::START%% ... %%AC_RESPONSE::{}::END%%)\n\r",
                    msg.from, msg.body, rid, rid
                )
            } else {
                format!("\n[Message from {}] {}\n\r", msg.from, msg.body)
            }
        } else {
            format!(
                concat!(
                    "\n[Message from {from}] {body}\n",
                    "(To reply, run: \"{bin}\" send --token <your_token> --to \"{from}\" --message \"your reply\" --mode wake)\n\r",
                ),
                from = msg.from,
                body = msg.body,
                bin = bin_path,
            )
        };

        // Register response watcher only for non-interactive sessions
        if use_markers {
            if let Some(ref rid) = msg.request_id {
                // Response file goes to the SENDER's .agentscommander/responses/
                if let Some(sender_path) = self.resolve_repo_path(&msg.from, app).await {
                    let response_dir = std::path::PathBuf::from(sender_path)
                        .join(".agentscommander")
                        .join("responses");
                    let mgr = pty_mgr.lock().map_err(|e| format!("PTY lock failed: {}", e))?;
                    mgr.register_response_watcher(session_id, rid.clone(), response_dir);
                    drop(mgr);
                }
            }
        }

        let mgr = pty_mgr.lock().map_err(|e| format!("PTY lock failed: {}", e))?;
        mgr.write(session_id, payload.as_bytes())
            .map_err(|e| format!("PTY write failed: {}", e))?;

        log::info!("Injected message {} into session {} PTY", msg.id, session_id);
        let _ = tauri::Emitter::emit(
            app,
            "message_delivered",
            serde_json::json!({
                "id": msg.id,
                "from": msg.from,
                "to": msg.to,
                "mode": msg.mode,
                "injected": true
            }),
        );
        Ok(())
    }

    /// Find an active session for a given agent name (matches by working directory).
    async fn find_active_session(&self, app: &tauri::AppHandle, agent_name: &str) -> Option<Uuid> {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        let dirs = mgr.get_sessions_directories().await;

        log::info!(
            "[mailbox] find_active_session for '{}' — {} sessions: {:?}",
            agent_name,
            dirs.len(),
            dirs.iter().map(|(id, cwd)| format!("{}={}", id, cwd)).collect::<Vec<_>>()
        );

        for (id, cwd) in &dirs {
            let normalized = cwd.replace('\\', "/");
            if normalized.ends_with(agent_name)
                || normalized.contains(&format!("/{}", agent_name))
            {
                log::info!("[mailbox] Matched session {} (cwd={})", id, cwd);
                return Some(*id);
            }
        }
        log::warn!("[mailbox] No session matched for '{}'", agent_name);
        None
    }

    /// Resolve the inbox directory for a destination agent.
    async fn resolve_inbox_dir(&self, agent_name: &str, app: &tauri::AppHandle) -> Result<PathBuf, String> {
        if let Some(path) = self.resolve_repo_path(agent_name, app).await {
            return Ok(PathBuf::from(path).join(".agentscommander").join("inbox"));
        }
        Err(format!("Could not resolve inbox for agent '{}'", agent_name))
    }

    /// Resolve the full filesystem path for an agent name.
    async fn resolve_repo_path(&self, agent_name: &str, app: &tauri::AppHandle) -> Option<String> {
        // Check session CWDs first
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        let dirs = mgr.get_sessions_directories().await;

        for (_, cwd) in &dirs {
            let normalized = cwd.replace('\\', "/");
            if normalized.ends_with(agent_name) || normalized.contains(&format!("/{}", agent_name)) {
                return Some(cwd.clone());
            }
        }

        // Check settings repo_paths
        let settings = app.state::<SettingsState>();
        let cfg = settings.read().await;
        for rp in &cfg.repo_paths {
            let normalized = rp.replace('\\', "/");
            if normalized.ends_with(agent_name) || normalized.contains(&format!("/{}", agent_name)) {
                return Some(rp.clone());
            }
        }

        // Check teams config for member paths
        let dark_factory = dark_factory::load_dark_factory();
        for team in &dark_factory.teams {
            for member in &team.members {
                let normalized = member.path.replace('\\', "/");
                if normalized.ends_with(agent_name) || normalized.contains(&format!("/{}", agent_name)) {
                    return Some(member.path.clone());
                }
            }
        }

        None
    }

    /// Check if sender can reach destination via team membership.
    /// Only agents in the same team can communicate — no parent directory fallback.
    fn can_reach(&self, from: &str, to: &str, config: &DarkFactoryConfig) -> bool {
        if config.teams.is_empty() {
            return false; // No teams configured → no communication
        }
        crate::phone::manager::can_communicate(from, to, config)
    }

    /// Resolve which agent CLI to use for wake-and-sleep mode.
    async fn resolve_agent_command(
        &self,
        app: &tauri::AppHandle,
        msg: &OutboxMessage,
    ) -> Option<(String, Vec<String>)> {
        let settings = app.state::<SettingsState>();
        let cfg = settings.read().await;

        // If specific agent requested
        if msg.preferred_agent != "auto" {
            if let Some(agent) = cfg.agents.iter().find(|a| a.id == msg.preferred_agent) {
                return Some((agent.command.clone(), vec![]));
            }
        }

        // Try lastCodingAgent from destination's config.json
        if let Some(dest_path) = self.resolve_repo_path(&msg.to, app).await {
            let config_path = Path::new(&dest_path)
                .join(".agentscommander")
                .join("config.json");
            if let Ok(content) = std::fs::read_to_string(&config_path) {
                if let Ok(local_config) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(last_agent) = local_config.get("lastCodingAgent").and_then(|v| v.as_str()) {
                        if let Some(agent) = cfg.agents.iter().find(|a| a.id == last_agent) {
                            return Some((agent.command.clone(), vec![]));
                        }
                    }
                }
            }
        }

        // Fallback: use sender's agent if known
        if let Some(ref sender_agent) = msg.sender_agent {
            if let Some(agent) = cfg.agents.iter().find(|a| a.id == *sender_agent) {
                return Some((agent.command.clone(), vec![]));
            }
        }

        // Last resort: use first configured agent
        cfg.agents.first().map(|a| (a.command.clone(), vec![]))
    }

    /// Move an outbox message to outbox/delivered/ with token stripped.
    async fn move_to_delivered(&self, path: &Path, msg: &OutboxMessage) -> Result<(), String> {
        let delivered_dir = path
            .parent()
            .ok_or("No parent dir")?
            .join("delivered");
        std::fs::create_dir_all(&delivered_dir)
            .map_err(|e| format!("Failed to create delivered dir: {}", e))?;

        // Strip token before storing
        let mut stripped = msg.clone();
        stripped.token = None;

        let dest = delivered_dir.join(format!("{}.json", msg.id));
        let json = serde_json::to_string_pretty(&stripped)
            .map_err(|e| format!("Failed to serialize: {}", e))?;
        std::fs::write(&dest, json)
            .map_err(|e| format!("Failed to write delivered file: {}", e))?;

        // Remove original
        std::fs::remove_file(path)
            .map_err(|e| format!("Failed to remove outbox file: {}", e))?;

        Ok(())
    }

    /// Reject a message: move to outbox/rejected/ with reason.
    async fn reject_message(&self, path: &Path, msg: &OutboxMessage, reason: &str) -> Result<(), String> {
        let rejected_dir = path
            .parent()
            .ok_or("No parent dir")?
            .join("rejected");
        std::fs::create_dir_all(&rejected_dir)
            .map_err(|e| format!("Failed to create rejected dir: {}", e))?;

        // Strip token
        let mut stripped = msg.clone();
        stripped.token = None;

        let dest = rejected_dir.join(format!("{}.json", msg.id));
        let json = serde_json::to_string_pretty(&stripped)
            .map_err(|e| format!("Failed to serialize: {}", e))?;
        std::fs::write(&dest, json)
            .map_err(|e| format!("Failed to write rejected file: {}", e))?;

        // Write reason
        let reason_path = rejected_dir.join(format!("{}.reason.txt", msg.id));
        std::fs::write(&reason_path, reason)
            .map_err(|_| "Failed to write reason file".to_string())?;

        // Remove original
        std::fs::remove_file(path)
            .map_err(|e| format!("Failed to remove outbox file: {}", e))?;

        log::warn!("Rejected message {}: {}", msg.id, reason);
        Ok(())
    }
}
