use std::collections::HashMap;
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
use crate::{AppOutbox, MasterToken};

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
    /// Remote command to execute on agent's PTY (e.g., "clear", "compact")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

/// Tracks delivery attempts for a single outbox message.
struct RetryState {
    attempt_count: u32,
    logged: bool,
}

const MAX_DELIVERY_ATTEMPTS: u32 = 10;
const ERR_UNRESOLVABLE_AGENT: &str = "Could not resolve inbox for agent";

/// The MailboxPoller runs as a background tokio task. It polls outbox directories
/// for all known agent repos, validates messages, and delivers them according to mode.
pub struct MailboxPoller {
    poll_interval: std::time::Duration,
    retry_tracker: HashMap<PathBuf, RetryState>,
}

impl MailboxPoller {
    pub fn new() -> Self {
        Self {
            poll_interval: std::time::Duration::from_secs(3),
            retry_tracker: HashMap::new(),
        }
    }

    /// Start the poller as a background task.
    pub fn start(mut self, app: tauri::AppHandle) {
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
    async fn poll(&mut self, app: &tauri::AppHandle) -> Result<(), String> {
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
        for (_, dir, _, _) in &session_dirs {
            if !all_paths.contains(dir) {
                all_paths.push(dir.clone());
            }
        }

        // Include the instance-private app outbox
        let app_outbox = app.state::<AppOutbox>();
        let app_outbox_path = app_outbox.path().to_string();

        // Collect all outbox directories to scan
        let mut outbox_dirs: Vec<PathBuf> = all_paths
            .iter()
            .map(|p| Path::new(p).join(crate::config::agent_local_dir_name()).join("outbox"))
            .collect();
        outbox_dirs.push(PathBuf::from(&app_outbox_path));

        for outbox_dir in &outbox_dirs {
            if !outbox_dir.is_dir() {
                continue;
            }

            let entries: Vec<PathBuf> = match std::fs::read_dir(outbox_dir) {
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

            let is_app_outbox = outbox_dir.as_path() == Path::new(&app_outbox_path);
            for path in entries {
                match self.process_message(app, &path, is_app_outbox).await {
                    Ok(()) => {
                        self.retry_tracker.remove(&path);
                    }
                    Err(e) => {
                        let is_permanent = e.contains(ERR_UNRESOLVABLE_AGENT);
                        let should_reject = is_permanent || {
                            let state = self.retry_tracker
                                .entry(path.clone())
                                .or_insert(RetryState { attempt_count: 0, logged: false });
                            state.attempt_count += 1;

                            if !state.logged {
                                log::warn!(
                                    "Failed to process outbox message {:?} (attempt {}): {}",
                                    path, state.attempt_count, e
                                );
                                state.logged = true;
                            } else {
                                log::debug!(
                                    "Retry {} for outbox message {:?}: {}",
                                    state.attempt_count, path, e
                                );
                            }

                            state.attempt_count >= MAX_DELIVERY_ATTEMPTS
                        };

                        if should_reject {
                            let reason = if is_permanent {
                                e.clone()
                            } else {
                                let attempts = self.retry_tracker.get(&path)
                                    .map(|s| s.attempt_count).unwrap_or(0);
                                format!("Undeliverable after {} attempts. Last error: {}", attempts, e)
                            };

                            let rejected = if let Ok(content) = std::fs::read_to_string(&path) {
                                if let Ok(msg) = serde_json::from_str::<OutboxMessage>(&content) {
                                    self.reject_message(&path, &msg, &reason).await.is_ok()
                                } else {
                                    Self::reject_raw_file(&path, &reason).is_ok()
                                }
                            } else {
                                false
                            };

                            if rejected {
                                self.retry_tracker.remove(&path);
                            } else {
                                log::error!(
                                    "Failed to reject outbox message {:?} — will retry",
                                    path
                                );
                            }
                        }
                    }
                }
            }
        }

        // Prune tracker entries for files that no longer exist
        self.retry_tracker.retain(|path, _| path.exists());

        // Poll session-requests directory (from create-agent CLI)
        self.poll_session_requests(app).await;

        Ok(())
    }

    /// Process a single outbox message file.
    /// `is_app_outbox`: true if the message came from the instance-private outbox (master token path).
    async fn process_message(&self, app: &tauri::AppHandle, path: &Path, is_app_outbox: bool) -> Result<(), String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read outbox file: {}", e))?;

        let msg: OutboxMessage = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse outbox message: {}", e))?;

        log::info!(
            "[mailbox] Processing message {} from='{}' to='{}' mode='{}'",
            msg.id, msg.from, msg.to, msg.mode
        );

        // For repo outboxes (not app-outbox), validate that msg.from matches the outbox owner.
        // This prevents tokenless spoofing: a message in repo X's outbox must claim to be from repo X.
        if !is_app_outbox {
            let outbox_dir = path.parent().unwrap_or(Path::new(""));
            // outbox_dir is <repo>/.agentscommander/outbox — go up 2 levels to get the repo path
            if let Some(repo_path) = outbox_dir.parent().and_then(|p| p.parent()) {
                let expected_from = self.agent_name_from_path(&repo_path.to_string_lossy());
                if expected_from != msg.from {
                    return self.reject_message(
                        path,
                        &msg,
                        &format!(
                            "Outbox-sender mismatch: outbox belongs to '{}' but message claims '{}'",
                            expected_from, msg.from
                        ),
                    )
                    .await;
                }
            }
        }

        // Check if token is the master token or root token (bypasses anti-spoofing + team validation)
        let is_master = if let Some(ref token_str) = msg.token {
            let master = app.state::<MasterToken>();
            if master.matches(token_str) {
                true
            } else {
                let settings = crate::config::settings::load_settings();
                settings.root_token.as_deref() == Some(token_str.as_str())
            }
        } else {
            false
        };

        if is_master {
            log::info!("[mailbox] Master token used — bypassing team validation for {} → {}", msg.from, msg.to);
        } else {
            // Validate session token if present (anti-spoofing)
            if let Some(ref token_str) = msg.token {
                let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let mgr = session_mgr.read().await;

                if let Ok(token_uuid) = Uuid::parse_str(token_str) {
                    match mgr.find_by_token(token_uuid).await {
                        None => {
                            // Token is stale/invalid. Try to find the sender's active session
                            // by CWD match — if found, the sender is legit (verified by outbox
                            // anti-spoofing above), so refresh their token and continue.
                            drop(mgr);
                            if let Some(session_id) = self.find_active_session(app, &msg.from).await {
                                log::info!(
                                    "[mailbox] Stale token from '{}' — found active session {}, refreshing token",
                                    msg.from, session_id
                                );
                                self.inject_fresh_token(app, session_id).await;
                                // Continue processing — sender verified by CWD match
                            } else {
                                return self.reject_message(path, &msg, "Invalid session token and no active session to refresh").await;
                            }
                        }
                        Some(session) => {
                            // Anti-spoofing: verify msg.from matches the token's session working_directory
                            let session_name = self.agent_name_from_path(&session.working_directory);
                            if session_name != msg.from {
                                log::warn!(
                                    "[mailbox] Token-root mismatch: token session='{}' but from='{}'",
                                    session_name, msg.from
                                );
                                return self.reject_message(
                                    path,
                                    &msg,
                                    &format!(
                                        "Token-root mismatch: session is '{}' but message claims '{}'",
                                        session_name, msg.from
                                    ),
                                )
                                .await;
                            }
                        }
                    }
                } else {
                    // Token is not a valid UUID (e.g. "none"). Treat like a stale token:
                    // try to find the sender's active session by CWD and refresh.
                    drop(mgr);
                    if let Some(session_id) = self.find_active_session(app, &msg.from).await {
                        log::info!(
                            "[mailbox] Malformed token from '{}' — found active session {}, refreshing token",
                            msg.from, session_id
                        );
                        self.inject_fresh_token(app, session_id).await;
                    } else {
                        return self.reject_message(path, &msg, "Malformed token and no active session to refresh").await;
                    }
                }
            }

            // Validate peer visibility (team membership) — skipped for master token
            let dark_factory = dark_factory::load_dark_factory();
            if !self.can_reach(&msg.from, &msg.to, &dark_factory) {
                return self.reject_message(path, &msg, "Sender cannot reach destination").await;
            }
        }

        // Deliver based on mode — all modes require immediate delivery or rejection
        let mode = if msg.mode.is_empty() { "wake" } else { msg.mode.as_str() };
        match mode {
            "active-only" => self.deliver_active_only(app, &msg).await?,
            "wake" => self.deliver_wake(app, &msg).await?,
            "wake-and-sleep" => self.deliver_wake_and_sleep(app, &msg).await?,
            _ => {
                return self.reject_message(
                    path,
                    &msg,
                    &format!("Unsupported delivery mode '{}'. Valid: active-only, wake, wake-and-sleep", mode),
                ).await;
            }
        }

        // Move to delivered/ with token stripped
        self.move_to_delivered(path, &msg).await
    }

    /// Deliver mode: active-only — inject into PTY if agent is active and not idle, else reject.
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
                log::info!("[mailbox] active-only: conditions not met, rejecting");
                return Err("Destination agent session is not active or is waiting for input".to_string());
            }
        }
        Err("No active session found for destination agent".to_string())
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
                log::info!("[mailbox] wake: session not idle, rejecting");
                return Err("Destination agent session is active but not idle (waiting for input)".to_string());
            } else {
                log::warn!("[mailbox] wake: session {} not in list_sessions", session_id);
            }
        }
        Err("No active session found for destination agent".to_string())
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
            // Session exists but is busy — reject
            return Err("Destination agent session exists but is busy (not idle)".to_string());
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
                None, // No agent label for temp sessions
                true,  // Skip tooling save for temp sessions
                false, // not a restore
                None,  // git_branch_source
                None,  // git_branch_prefix
            )
            .await
            {
                Ok(info) => {
                    let session_id = Uuid::parse_str(&info.id)
                        .map_err(|e| format!("Failed to parse session id: {}", e))?;

                    // Wait for agent boot + init prompt injection (init fires at 3s, needs time to process)
                    tokio::time::sleep(std::time::Duration::from_secs(6)).await;

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
                    Err(format!("Failed to spawn temporary session for delivery: {}", e))
                }
            }
        } else {
            log::warn!("wake-and-sleep: no agent command found for {}", msg.to);
            Err(format!("No agent command resolved for '{}' — cannot spawn temporary session", msg.to))
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

        // ── Remote command path: write /<command>\r directly, no framing ──
        if let Some(ref command) = msg.command {
            const ALLOWED_COMMANDS: &[&str] = &["clear", "compact"];
            if !ALLOWED_COMMANDS.contains(&command.as_str()) {
                return Err(format!("Unsupported remote command '{}'", command));
            }

            // Precondition: agent must be idle (waiting_for_input)
            {
                let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let mgr = session_mgr.read().await;
                let sessions = mgr.list_sessions().await;
                match sessions.iter().find(|s| s.id == session_id.to_string()) {
                    None => return Err(format!(
                        "Session {} not found — cannot execute remote command '{}'",
                        session_id, command
                    )),
                    Some(s) if !s.waiting_for_input => return Err(format!(
                        "Cannot execute remote command '{}': agent is busy (not idle)",
                        command
                    )),
                    _ => {} // idle — proceed
                }
            }

            // Write /<command>\r directly to PTY stdin.
            // Note: there is a small race window between the idle check above and
            // this write — the agent could become busy on a separate task. This is
            // inherent to a polling-based idle model and is acceptable for /clear
            // and /compact which Claude Code processes even if mid-prompt.
            let cmd_bytes = format!("/{}\r", command);
            {
                let mgr = pty_mgr.lock().map_err(|e| format!("PTY lock failed: {}", e))?;
                mgr.write(session_id, cmd_bytes.as_bytes())
                    .map_err(|e| format!("PTY write failed for remote command: {}", e))?;
            }

            // Record in transcript
            {
                let transcript = app.state::<crate::pty::transcript::TranscriptWriter>();
                transcript.record_inject(
                    session_id,
                    cmd_bytes.as_bytes(),
                    crate::pty::transcript::InjectReason::RemoteCommand,
                    Some(msg.from.clone()),
                    true,
                );
            }

            log::info!(
                "Executed remote command '{}' on session {} (from: {})",
                command, session_id, msg.from
            );

            let _ = tauri::Emitter::emit(
                app,
                "message_delivered",
                serde_json::json!({
                    "id": msg.id,
                    "from": msg.from,
                    "to": msg.to,
                    "mode": msg.mode,
                    "command": command,
                    "injected": true
                }),
            );

            // If body is also present, spawn follow-up as background task.
            // Command delivery is already complete — don't block the delivery pipeline.
            if !msg.body.is_empty() {
                let app_clone = app.clone();
                let msg_clone = msg.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = Self::inject_followup_after_idle_static(&app_clone, session_id, &msg_clone).await {
                        log::warn!("Follow-up injection after remote command failed: {}", e);
                    }
                });
            }

            return Ok(());
        }

        // ── Standard message path ──
        // Only use response markers for non-interactive sessions
        let use_markers = msg.get_output && !interactive;

        // Resolve binary path for reply instructions
        let bin_path = crate::resolve_bin_label();

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
                        .join(crate::config::agent_local_dir_name())
                        .join("responses");
                    let mgr = pty_mgr.lock().map_err(|e| format!("PTY lock failed: {}", e))?;
                    mgr.register_response_watcher(session_id, rid.clone(), response_dir);
                    drop(mgr);
                }
            }
        }

        crate::pty::inject::inject_text_into_session(app, session_id, &payload, true, crate::pty::transcript::InjectReason::MessageDelivery, Some(msg.from.clone())).await?;

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

    /// Wait for agent to become idle after a remote command, then inject body as follow-up.
    /// Static method — can be spawned as a detached task without borrowing self.
    async fn inject_followup_after_idle_static(
        app: &tauri::AppHandle,
        session_id: Uuid,
        msg: &OutboxMessage,
    ) -> Result<(), String> {
        let max_wait = std::time::Duration::from_secs(30);
        let poll = std::time::Duration::from_millis(500);
        let start = std::time::Instant::now();

        // Wait for idle (waiting_for_input = true)
        loop {
            if start.elapsed() >= max_wait {
                return Err(format!(
                    "Timeout waiting for agent to become idle after remote command ({}s)",
                    max_wait.as_secs()
                ));
            }
            tokio::time::sleep(poll).await;

            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            let sessions = mgr.list_sessions().await;
            match sessions.iter().find(|s| s.id == session_id.to_string()) {
                Some(s) if s.waiting_for_input => break,
                Some(_) => {} // busy — keep polling
                None => return Err(format!(
                    "Session {} destroyed before follow-up could be injected",
                    session_id
                )),
            }
        }

        // Inject the follow-up body as a standard interactive message.
        // Note: same TOCTOU race as the command path — agent could become busy
        // between the idle check above and this write. Acceptable for this use case.
        let bin_path = crate::resolve_bin_label();
        let payload = format!(
            concat!(
                "\n[Message from {from}] {body}\n",
                "(To reply, run: \"{bin}\" send --token <your_token> --to \"{from}\" --message \"your reply\" --mode wake)\n\r",
            ),
            from = msg.from,
            body = msg.body,
            bin = bin_path,
        );
        crate::pty::inject::inject_text_into_session(
            app,
            session_id,
            &payload,
            true,
            crate::pty::transcript::InjectReason::MessageDelivery,
            Some(msg.from.clone()),
        ).await
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
            dirs.iter().map(|(id, cwd, _, _)| format!("{}={}", id, cwd)).collect::<Vec<_>>()
        );

        for (id, cwd, _, _) in &dirs {
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

    /// Resolve the full filesystem path for an agent name.
    async fn resolve_repo_path(&self, agent_name: &str, app: &tauri::AppHandle) -> Option<String> {
        // Check session CWDs first
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        let dirs = mgr.get_sessions_directories().await;

        for (_, cwd, _, _) in &dirs {
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

        // Check WG replicas: "wg-name/agent" → scan repo_paths for .ac-new/wg-name/__agent_agent/
        if agent_name.starts_with("wg-") {
            if let Some((wg_name, agent_short)) = agent_name.split_once('/') {
                let replica_dir = format!("__agent_{}", agent_short);
                for rp in &cfg.repo_paths {
                    let base = std::path::Path::new(rp);
                    if !base.is_dir() {
                        continue;
                    }
                    let mut dirs_to_check = vec![base.to_path_buf()];
                    if let Ok(entries) = std::fs::read_dir(base) {
                        for entry in entries.flatten() {
                            let p = entry.path();
                            if p.is_dir() {
                                dirs_to_check.push(p);
                            }
                        }
                    }
                    for dir in dirs_to_check {
                        let candidate = dir.join(".ac-new").join(wg_name).join(&replica_dir);
                        if candidate.is_dir() {
                            return Some(candidate.to_string_lossy().to_string());
                        }
                    }
                }
            }
        }

        None
    }

    /// Derive agent name (parent/folder) from a path, stripping `__agent_`/`_agent_` prefixes.
    fn agent_name_from_path(&self, path: &str) -> String {
        let normalized = path.replace('\\', "/");
        let components: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();
        if components.len() >= 2 {
            let parent = components[components.len() - 2];
            let last = components[components.len() - 1];
            let stripped = last
                .strip_prefix("__agent_")
                .or_else(|| last.strip_prefix("_agent_"))
                .unwrap_or(last);
            format!("{}/{}", parent, stripped)
        } else {
            normalized
        }
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
                .join(crate::config::agent_local_dir_name())
                .join("config.json");
            if let Ok(content) = std::fs::read_to_string(&config_path) {
                if let Ok(local_config) = serde_json::from_str::<crate::config::dark_factory::AgentLocalConfig>(&content) {
                    if let Some(last_agent) = local_config.tooling.last_coding_agent.as_deref() {
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

    /// Reject a message: move to outbox/rejected/ with reason, and notify the sender.
    async fn reject_message(&self, path: &Path, msg: &OutboxMessage, reason: &str) -> Result<(), String> {
        let rejected_dir = path
            .parent()
            .ok_or("No parent dir")?
            .join("rejected");
        std::fs::create_dir_all(&rejected_dir)
            .map_err(|e| format!("Failed to create rejected dir: {}", e))?;

        // Write reason file FIRST — the CLI polls for this file to detect rejection
        let reason_path = rejected_dir.join(format!("{}.reason.txt", msg.id));
        std::fs::write(&reason_path, reason)
            .map_err(|_| "Failed to write reason file".to_string())?;

        // Then write the stripped message JSON
        let mut stripped = msg.clone();
        stripped.token = None;

        let dest = rejected_dir.join(format!("{}.json", msg.id));
        let json = serde_json::to_string_pretty(&stripped)
            .map_err(|e| format!("Failed to serialize: {}", e))?;
        std::fs::write(&dest, json)
            .map_err(|e| format!("Failed to write rejected file: {}", e))?;

        // Remove original
        std::fs::remove_file(path)
            .map_err(|e| format!("Failed to remove outbox file: {}", e))?;

        log::warn!("Rejected message {}: {}", msg.id, reason);
        Ok(())
    }

    /// Reject a raw file that cannot be parsed as OutboxMessage.
    fn reject_raw_file(path: &Path, reason: &str) -> Result<(), String> {
        let rejected_dir = path
            .parent()
            .ok_or("No parent dir")?
            .join("rejected");
        std::fs::create_dir_all(&rejected_dir)
            .map_err(|e| format!("Failed to create rejected dir: {}", e))?;

        let filename = path.file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("unknown.json");

        let dest = rejected_dir.join(filename);
        std::fs::rename(path, &dest)
            .or_else(|_| {
                std::fs::copy(path, &dest)
                    .and_then(|_| std::fs::remove_file(path))
            })
            .map_err(|e| format!("Failed to move file to rejected: {}", e))?;

        let stem = Path::new(filename).file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");
        let reason_path = rejected_dir.join(format!("{}.reason.txt", stem));
        std::fs::write(&reason_path, reason)
            .map_err(|_| "Failed to write reason file".to_string())?;

        log::warn!("Rejected raw file {:?}: {}", path, reason);
        Ok(())
    }

    /// Poll ~/.agentscommander/session-requests/ for launch requests from the CLI.
    async fn poll_session_requests(&self, app: &tauri::AppHandle) {
        let config_dir = match crate::config::config_dir() {
            Some(d) => d,
            None => return,
        };
        let requests_dir = config_dir.join("session-requests");
        if !requests_dir.is_dir() {
            return;
        }

        let entries: Vec<PathBuf> = match std::fs::read_dir(&requests_dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("json"))
                .collect(),
            Err(_) => return,
        };

        for path in entries {
            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(e) => {
                    log::warn!("[session-requests] Failed to read {:?}: {}", path, e);
                    continue;
                }
            };

            let request: crate::cli::create_agent::SessionRequest = match serde_json::from_str(&content) {
                Ok(r) => r,
                Err(e) => {
                    log::warn!("[session-requests] Failed to parse {:?}: {}", path, e);
                    // Delete malformed file
                    let _ = std::fs::remove_file(&path);
                    continue;
                }
            };

            log::info!(
                "[session-requests] Processing: name='{}' cwd='{}' agent='{}'",
                request.session_name, request.cwd, request.agent_id
            );

            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();

            match crate::commands::session::create_session_inner(
                app,
                session_mgr.inner(),
                pty_mgr.inner(),
                request.shell.clone(),
                request.shell_args.clone(),
                request.cwd.clone(),
                Some(request.session_name.clone()),
                Some(request.agent_id.clone()),
                None,  // No agent label — auto-detected from shell
                false, // Persist tooling
                false, // not a restore
                None,  // git_branch_source
                None,  // git_branch_prefix
            )
            .await
            {
                Ok(info) => {
                    log::info!(
                        "[session-requests] Created session '{}' (id={})",
                        request.session_name, info.id
                    );
                }
                Err(e) => {
                    log::error!(
                        "[session-requests] Failed to create session '{}': {}",
                        request.session_name, e
                    );
                }
            }

            // Delete processed request file regardless of success/failure
            let _ = std::fs::remove_file(&path);
        }
    }

    /// Inject the current valid token into a session's PTY so the agent can update its credentials.
    /// Called when we detect the agent is using a stale token.
    async fn inject_fresh_token(&self, app: &tauri::AppHandle, session_id: Uuid) {
        // Extract session data under the read-lock, then drop before acquiring PtyManager mutex.
        // This follows the same lock ordering pattern as inject_into_pty / deliver_wake.
        let notice = {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            let sessions = mgr.list_sessions().await;

            match sessions.iter().find(|s| s.id == session_id.to_string()) {
                Some(session) => format!(
                    "\n\
                    # === TOKEN REFRESHED ===\n\
                    # Your previous token was invalid. Here is your updated token:\n\
                    # New session token: {token}\n\
                    #\n\
                    # Updated send command:\n\
                    #   \"{exe}\" send --token {token} --root \"{root}\" --to \"<agent_name>\" --message \"...\" --mode wake\n\
                    # === End Token Refresh ===\n\
                    \r",
                    exe = crate::config::profile::exe_name(),
                    token = session.token,
                    root = session.working_directory,
                ),
                None => {
                    log::warn!("[mailbox] inject_fresh_token: session {} not found", session_id);
                    return;
                }
            }
            // SessionManager read-lock dropped here
        };

        match crate::pty::inject::inject_text_into_session(app, session_id, &notice, false, crate::pty::transcript::InjectReason::TokenRefresh, None).await {
            Ok(()) => log::info!("[mailbox] Fresh token injected into session {}", session_id),
            Err(e) => log::warn!("[mailbox] Failed to inject fresh token into session {}: {}", session_id, e),
        }
    }
}
