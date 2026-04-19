use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tauri::Manager;
use uuid::Uuid;

use crate::config::agent_config::AgentLocalConfig;
use crate::config::settings::SettingsState;
use crate::config::teams;
use crate::phone::types::OutboxMessage;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::session::SessionStatus;
use crate::{AppOutbox, MasterToken};

/// Tracks delivery attempts for a single outbox message.
struct RetryState {
    attempt_count: u32,
    logged: bool,
}

const MAX_DELIVERY_ATTEMPTS: u32 = 10;
const ERR_UNRESOLVABLE_AGENT: &str = "Could not resolve inbox for agent";

/// Decision made by `deliver_wake` for an existing session.
#[derive(Debug, PartialEq)]
pub(crate) enum WakeAction {
    /// Session is live — inject into stdin, regardless of whether the agent
    /// is waiting for input or mid-turn. Bias toward delivery.
    Inject,
    /// Session is Exited — destroy it and fall through to spawn a fresh one.
    RespawnExited,
}

/// Pure decision given a session's status. Extracted so the decision table is
/// unit-testable without a tauri runtime. `deliver_wake` calls this and acts
/// on the result; any future restoration of a busy-gate would require editing
/// this fn (and its tests below), not a lone `if` inside `deliver_wake`.
pub(crate) fn wake_action_for(status: &SessionStatus) -> WakeAction {
    if matches!(status, SessionStatus::Exited(_)) {
        WakeAction::RespawnExited
    } else {
        WakeAction::Inject
    }
}

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
    pub fn start(mut self, app: tauri::AppHandle, shutdown: crate::shutdown::ShutdownSignal) {
        tauri::async_runtime::spawn(async move {
            // Initial poll without delay (matches original behavior)
            if let Err(e) = self.poll(&app).await {
                log::warn!("MailboxPoller error: {}", e);
            }
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown.token().cancelled() => {
                        log::info!("[MailboxPoller] Shutdown signal received, stopping");
                        break;
                    }
                    _ = tokio::time::sleep(self.poll_interval) => {
                        if let Err(e) = self.poll(&app).await {
                            log::warn!("MailboxPoller error: {}", e);
                        }
                    }
                }
            }
        });
    }

    /// One poll cycle: scan all repo outbox dirs, process each message.
    async fn poll(&mut self, app: &tauri::AppHandle) -> Result<(), String> {
        let settings = app.state::<SettingsState>();
        let repo_paths = {
            let cfg = settings.read().await;
            cfg.project_paths.clone()
        };

        // Also scan CWDs of active sessions for repos not in settings
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let session_dirs = {
            let mgr = session_mgr.read().await;
            mgr.get_sessions_working_dirs().await
        };

        let mut all_paths: Vec<String> = repo_paths;
        for (_, dir) in &session_dirs {
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
            .map(|p| {
                Path::new(p)
                    .join(crate::config::agent_local_dir_name())
                    .join("outbox")
            })
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
                            let state =
                                self.retry_tracker
                                    .entry(path.clone())
                                    .or_insert(RetryState {
                                        attempt_count: 0,
                                        logged: false,
                                    });
                            state.attempt_count += 1;

                            if !state.logged {
                                log::warn!(
                                    "Failed to process outbox message {:?} (attempt {}): {}",
                                    path,
                                    state.attempt_count,
                                    e
                                );
                                state.logged = true;
                            } else {
                                log::debug!(
                                    "Retry {} for outbox message {:?}: {}",
                                    state.attempt_count,
                                    path,
                                    e
                                );
                            }

                            state.attempt_count >= MAX_DELIVERY_ATTEMPTS
                        };

                        if should_reject {
                            let reason = if is_permanent {
                                e.clone()
                            } else {
                                let attempts = self
                                    .retry_tracker
                                    .get(&path)
                                    .map(|s| s.attempt_count)
                                    .unwrap_or(0);
                                format!(
                                    "Undeliverable after {} attempts. Last error: {}",
                                    attempts, e
                                )
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
    async fn process_message(
        &self,
        app: &tauri::AppHandle,
        path: &Path,
        is_app_outbox: bool,
    ) -> Result<(), String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read outbox file: {}", e))?;

        let msg: OutboxMessage = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse outbox message: {}", e))?;

        log::info!(
            "[mailbox] Processing message {} from='{}' to='{}' mode='{}'",
            msg.id,
            msg.from,
            msg.to,
            msg.mode
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
            log::info!(
                "[mailbox] Master token used — bypassing team validation for {} → {}",
                msg.from,
                msg.to
            );
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
                            if let Some(session_id) = self.find_active_session(app, &msg.from).await
                            {
                                log::info!(
                                    "[mailbox] Stale token from '{}' — found active session {}, refreshing token",
                                    msg.from, session_id
                                );
                                self.inject_fresh_token(app, session_id).await;
                                // Continue processing — sender verified by CWD match
                            } else {
                                return self
                                    .reject_message(
                                        path,
                                        &msg,
                                        "Invalid session token and no active session to refresh",
                                    )
                                    .await;
                            }
                        }
                        Some(session) => {
                            // Anti-spoofing: verify msg.from matches the token's session working_directory
                            let session_name =
                                self.agent_name_from_path(&session.working_directory);
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
                        return self
                            .reject_message(
                                path,
                                &msg,
                                "Malformed token and no active session to refresh",
                            )
                            .await;
                    }
                }
            }

            // Validate peer visibility (team membership) — skipped for master token
            let discovered_teams = teams::discover_teams();
            if !self.can_reach(&msg.from, &msg.to, &discovered_teams) {
                log::warn!(
                    "[mailbox] Routing check FAILED: '{}' cannot reach '{}'",
                    msg.from,
                    msg.to
                );
                return self
                    .reject_message(path, &msg, "Sender cannot reach destination")
                    .await;
            }
            log::info!(
                "[mailbox] Routing check passed: '{}' → '{}'",
                msg.from,
                msg.to
            );
        }

        // Action-based dispatch (close-session, etc.) — handled before mode-based delivery
        if let Some(ref action) = msg.action {
            match action.as_str() {
                "close-session" => {
                    return self.handle_close_session(app, path, &msg).await;
                }
                _ => {
                    return self
                        .reject_message(path, &msg, &format!("Unknown action '{}'", action))
                        .await;
                }
            }
        }

        // Deliver based on mode — all modes require immediate delivery or rejection
        let mode = if msg.mode.is_empty() {
            "wake"
        } else {
            msg.mode.as_str()
        };
        // Only `wake` is supported. Defensive check for malformed outbox files
        // that might arrive from external (root-token) write paths.
        if mode != "wake" {
            return self
                .reject_message(
                    path,
                    &msg,
                    &format!("Unsupported delivery mode '{}'. Valid: wake", mode),
                )
                .await;
        }
        self.deliver_wake(app, &msg).await?;

        // Move to delivered/ with token stripped
        self.move_to_delivered(path, &msg).await
    }

    /// Deliver mode: wake — inject into the recipient's PTY for any non-Exited
    /// session; destroy and respawn if Exited; spawn persistent if none. Always
    /// delivers (no busy-gate — stdin buffer absorbs input while the agent is
    /// mid-turn).
    async fn deliver_wake(
        &self,
        app: &tauri::AppHandle,
        msg: &OutboxMessage,
    ) -> Result<(), String> {
        if let Some(session_id) = self.find_active_session(app, &msg.to).await {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            let sessions = mgr.list_sessions().await;
            let session = sessions.iter().find(|s| s.id == session_id.to_string());

            if let Some(s) = session {
                log::info!(
                    "[mailbox] wake: session {} status={:?} waiting_for_input={}",
                    session_id,
                    s.status,
                    s.waiting_for_input
                );
                match wake_action_for(&s.status) {
                    WakeAction::Inject => {
                        // Always inject — PTY stdin buffer holds the input
                        // until the agent finishes the current turn.
                        drop(mgr);
                        return self.inject_into_pty(app, session_id, msg, true).await;
                    }
                    WakeAction::RespawnExited => {
                        log::info!(
                            "[mailbox] wake: session {} is Exited, destroying before respawn",
                            session_id
                        );
                        // Drop read lock before destroy call — release promptly
                        // (destroy acquires its own read lock).
                        drop(mgr);
                        if let Err(e) =
                            crate::commands::session::destroy_session_inner(app, session_id).await
                        {
                            log::error!(
                                "[mailbox] wake: failed to destroy exited session {}: {}",
                                session_id,
                                e
                            );
                        }
                        // Fall through to spawn-persistent.
                    }
                }
            } else {
                log::warn!(
                    "[mailbox] wake: session {} not in list_sessions",
                    session_id
                );
                drop(mgr);
            }
        }

        // ── No active session (or only Exited) — spawn a persistent one ──
        log::info!(
            "[mailbox] wake: no active session for '{}', spawning persistent session",
            msg.to
        );

        let agent_command = self.resolve_agent_command(app, msg).await;
        let (shell, shell_args) = agent_command.ok_or_else(|| {
            format!("No agent command resolved for '{}' — cannot spawn session. Configure lastCodingAgent or agents in settings.", msg.to)
        })?;

        let dest_path = self.resolve_repo_path(&msg.to, app).await;
        let cwd = match dest_path {
            Some(path) => path,
            None => {
                // Fallback: for WG agents (wg-name/agent), derive path from sibling session CWDs
                self.resolve_wg_path_from_sessions(app, &msg.to)
                    .await
                    .ok_or_else(|| {
                        format!(
                            "Cannot resolve repo path for '{}' — cannot spawn session",
                            msg.to
                        )
                    })?
            }
        };

        // Resolve agent_id and label from lastCodingAgent config
        let (agent_id, agent_label) = self.resolve_agent_id_and_label(app, &cwd).await;

        let session_name = msg.to.clone();

        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();

        let info = crate::commands::session::create_session_inner(
            app,
            session_mgr.inner(),
            pty_mgr.inner(),
            shell,
            shell_args,
            cwd,
            Some(session_name), // readable name, no [temp] prefix
            agent_id,           // links to agent config
            agent_label,        // human-readable label
            false,              // skip_tooling_save = false → persist lastCodingAgent
            Vec::new(),         // git_repos
            false,              // skip_auto_resume = false → allow provider auto-resume
        )
        .await
        .map_err(|e| format!("Failed to spawn session for '{}': {}", msg.to, e))?;

        let session_id =
            Uuid::parse_str(&info.id).map_err(|e| format!("Failed to parse session id: {}", e))?;

        // Wait for agent to boot and become idle (ready for input)
        let max_wait = std::time::Duration::from_secs(90);
        let poll = std::time::Duration::from_millis(500);
        let start = std::time::Instant::now();

        loop {
            if start.elapsed() >= max_wait {
                log::warn!(
                    "[mailbox] wake: timeout waiting for session {} to become idle",
                    session_id
                );
                break; // inject anyway as fallback
            }
            tokio::time::sleep(poll).await;

            let mgr = session_mgr.read().await;
            let sessions = mgr.list_sessions().await;
            match sessions.iter().find(|s| s.id == session_id.to_string()) {
                Some(s) if s.waiting_for_input => {
                    log::info!(
                        "[mailbox] wake: session {} is idle, injecting message",
                        session_id
                    );
                    drop(mgr);
                    break;
                }
                Some(_) => {} // still booting
                None => {
                    return Err(format!(
                        "Session {} was destroyed before message injection",
                        session_id
                    ));
                }
            }
            drop(mgr);
        }

        // Inject message — interactive mode (session persists, user sees reply instructions)
        self.inject_into_pty(app, session_id, msg, true).await
    }

    /// Inject a message into a session's PTY stdin.
    /// `interactive` = true (all remaining callers): live interactive `wake`
    /// delivery — plain message only, no response markers, no watcher.
    /// `interactive` = false is currently unreachable (the former
    /// `wake-and-sleep` non-interactive path was removed in 0.7.0). The
    /// `use_markers=true` branch below is retained for future non-interactive
    /// consumers; see _plans/delete-modes.md §2.4.
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
                    None => {
                        return Err(format!(
                            "Session {} not found — cannot execute remote command '{}'",
                            session_id, command
                        ))
                    }
                    Some(s) if !s.waiting_for_input => {
                        return Err(format!(
                            "Cannot execute remote command '{}': agent is busy (not idle)",
                            command
                        ))
                    }
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
                let mgr = pty_mgr
                    .lock()
                    .map_err(|e| format!("PTY lock failed: {}", e))?;
                mgr.write(session_id, cmd_bytes.as_bytes())
                    .map_err(|e| format!("PTY write failed for remote command: {}", e))?;
            }

            log::info!(
                "Executed remote command '{}' on session {} (from: {})",
                command,
                session_id,
                msg.from
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
                    if let Err(e) =
                        Self::inject_followup_after_idle_static(&app_clone, session_id, &msg_clone)
                            .await
                    {
                        log::warn!("Follow-up injection after remote command failed: {}", e);
                    }
                });
            }

            return Ok(());
        }

        // ── Standard message path ──
        // Only use response markers for non-interactive sessions
        let use_markers = msg.get_output && !interactive;

        // Interactive and marker-less paths share the minimal PTY wrap via
        // `format_pty_wrap` (single source with `PTY_WRAP_FIXED` used by the
        // CLI clamp). Only the `--get-output` + `request_id` case wraps the
        // payload with response markers.
        let payload = match (use_markers, msg.request_id.as_ref()) {
            (true, Some(rid)) => format!(
                "\n[Message from {}] {}\n(Reply between markers: %%AC_RESPONSE::{}::START%% ... %%AC_RESPONSE::{}::END%%)\n\r",
                msg.from, msg.body, rid, rid
            ),
            _ => crate::phone::messaging::format_pty_wrap(&msg.from, &msg.body),
        };

        // Register response watcher only for non-interactive sessions
        if use_markers {
            if let Some(ref rid) = msg.request_id {
                // Response file goes to the SENDER's .agentscommander/responses/
                if let Some(sender_path) = self.resolve_repo_path(&msg.from, app).await {
                    let response_dir = std::path::PathBuf::from(sender_path)
                        .join(crate::config::agent_local_dir_name())
                        .join("responses");
                    let mgr = pty_mgr
                        .lock()
                        .map_err(|e| format!("PTY lock failed: {}", e))?;
                    mgr.register_response_watcher(session_id, rid.clone(), response_dir);
                    drop(mgr);
                }
            }
        }

        log::debug!(
            "[mailbox] Injecting into PTY session={} msg={} payload_len={} first_100={:?}",
            session_id,
            msg.id,
            payload.len(),
            payload.chars().take(100).collect::<String>()
        );
        crate::pty::inject::inject_text_into_session(app, session_id, &payload, true)
            .await
            .map_err(|e| {
                log::error!(
                    "[mailbox] PTY injection FAILED session={} msg={}: {}",
                    session_id,
                    msg.id,
                    e
                );
                e
            })?;

        log::info!(
            "[mailbox] PTY injection SUCCESS session={} msg={}",
            session_id,
            msg.id
        );
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
                None => {
                    return Err(format!(
                        "Session {} destroyed before follow-up could be injected",
                        session_id
                    ))
                }
            }
        }

        // Inject the follow-up body as a standard interactive message.
        // Note: same TOCTOU race as the command path — agent could become busy
        // between the idle check above and this write. Acceptable for this use case.
        let payload = crate::phone::messaging::format_pty_wrap(&msg.from, &msg.body);
        crate::pty::inject::inject_text_into_session(app, session_id, &payload, true).await
    }

    /// Find the best session for a given agent name (matches by working directory).
    /// Prefers active/running non-temp sessions over idle/exited ones.
    async fn find_active_session(&self, app: &tauri::AppHandle, agent_name: &str) -> Option<Uuid> {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        let sessions = mgr.list_sessions().await;

        log::info!(
            "[mailbox] find_active_session for '{}' — {} sessions: {:?}",
            agent_name,
            sessions.len(),
            sessions
                .iter()
                .map(|s| format!(
                    "{}={} status={:?} name={}",
                    s.id, s.working_directory, s.status, s.name
                ))
                .collect::<Vec<_>>()
        );

        // Collect all CWD-matching sessions (also match via agent_name_from_path for WG replicas
        // where CWD contains __agent_ prefix that doesn't appear in the logical agent name)
        let mut matches: Vec<&crate::session::session::SessionInfo> = sessions
            .iter()
            .filter(|s| {
                let normalized = s.working_directory.replace('\\', "/");
                self.agent_name_from_path(&s.working_directory) == agent_name
                    || normalized.ends_with(agent_name)
                    || normalized.contains(&format!("/{}", agent_name))
            })
            .collect();

        if matches.is_empty() {
            log::warn!("[mailbox] No session matched for '{}'", agent_name);
            return None;
        }

        log::info!(
            "[mailbox] {} CWD matches for '{}': {:?}",
            matches.len(),
            agent_name,
            matches
                .iter()
                .map(|s| format!("{}({})", s.id, s.name))
                .collect::<Vec<_>>()
        );

        // Sort: non-temp first (false < true), then Active/Running before Idle before Exited
        matches.sort_by_key(|s| {
            let is_temp = s
                .name
                .starts_with(crate::session::session::TEMP_SESSION_PREFIX);
            let status = match s.status {
                SessionStatus::Active | SessionStatus::Running => 0u8,
                SessionStatus::Idle => 1,
                SessionStatus::Exited(_) => 2,
            };
            (is_temp, status)
        });

        let best = &matches[0];
        log::info!(
            "[mailbox] Best match for '{}': session {} (name='{}', status={:?})",
            agent_name,
            best.id,
            best.name,
            best.status
        );
        Uuid::parse_str(&best.id).ok()
    }

    /// Find ALL sessions matching an agent name (by working directory).
    /// Returns all matching session UUIDs, not just the "best" one.
    async fn find_all_sessions(&self, app: &tauri::AppHandle, agent_name: &str) -> Vec<Uuid> {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        let sessions = mgr.list_sessions().await;

        sessions
            .iter()
            .filter(|s| {
                let normalized = s.working_directory.replace('\\', "/");
                self.agent_name_from_path(&s.working_directory) == agent_name
                    || normalized.ends_with(agent_name)
                    || normalized.contains(&format!("/{}", agent_name))
            })
            .filter_map(|s| Uuid::parse_str(&s.id).ok())
            .collect()
    }

    /// Handle close-session action: validate coordinator auth, find target sessions, destroy them.
    async fn handle_close_session(
        &self,
        app: &tauri::AppHandle,
        path: &std::path::Path,
        msg: &OutboxMessage,
    ) -> Result<(), String> {
        let target = msg
            .target
            .as_deref()
            .ok_or_else(|| "close-session requires 'target' field".to_string())?;

        // Re-check master token for coordinator auth bypass (independent of routing bypass above)
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

        if !is_master {
            let discovered = teams::discover_teams();
            if !teams::is_coordinator_of(&msg.from, target, &discovered) {
                return self
                    .reject_message(
                        path,
                        msg,
                        &format!(
                            "Not authorized: '{}' is not a coordinator of '{}' team",
                            msg.from, target
                        ),
                    )
                    .await;
            }
        }

        // Find all sessions for the target agent
        let session_ids = self.find_all_sessions(app, target).await;
        if session_ids.is_empty() {
            return self
                .reject_message(
                    path,
                    msg,
                    &format!("No active session found for '{}'", target),
                )
                .await;
        }

        let force = msg.force.unwrap_or(false);
        let timeout_secs = msg.timeout_secs.unwrap_or(30);

        log::info!(
            "[mailbox] close-session: {} {} session(s) for '{}' (requested by '{}', timeout={}s)",
            if force {
                "force-killing"
            } else {
                "gracefully closing"
            },
            session_ids.len(),
            target,
            msg.from,
            timeout_secs
        );

        let mut closed_ids: Vec<String> = Vec::new();
        for sid in &session_ids {
            let success = if force {
                self.force_close_session(app, *sid).await
            } else {
                self.graceful_close_session(app, *sid, timeout_secs).await
            };
            if success {
                closed_ids.push(sid.to_string());
            }
        }

        // Write response with details to sender's responses/ dir.
        // If closed_ids is empty, sessions were found but already exited/destroyed
        // between find and destroy (race condition) — report as already_closed, not error.
        if let Some(ref rid) = msg.request_id {
            let status = if closed_ids.is_empty() {
                "already_closed"
            } else {
                "closed"
            };
            let response = serde_json::json!({
                "action": "close-session",
                "target": target,
                "status": status,
                "sessions_closed": closed_ids.len(),
                "session_ids": closed_ids,
                "requested_by": msg.from,
            });

            if let Some(sender_path) = self.resolve_repo_path(&msg.from, app).await {
                let responses_dir = std::path::PathBuf::from(sender_path)
                    .join(crate::config::agent_local_dir_name())
                    .join("responses");
                let _ = std::fs::create_dir_all(&responses_dir);
                let response_path = responses_dir.join(format!("{}.json", rid));
                if let Ok(json) = serde_json::to_string_pretty(&response) {
                    if let Err(e) = std::fs::write(&response_path, json) {
                        log::warn!("[mailbox] Failed to write close-session response: {}", e);
                    }
                }
            }
        }

        // Move original message to delivered/
        self.move_to_delivered(path, msg).await
    }

    /// Force-close a session immediately via destroy_session_inner.
    async fn force_close_session(&self, app: &tauri::AppHandle, sid: Uuid) -> bool {
        match crate::commands::session::destroy_session_inner(app, sid).await {
            Ok(()) => {
                log::info!("[mailbox] close-session: force-destroyed session {}", sid);
                true
            }
            Err(e) => {
                log::warn!(
                    "[mailbox] close-session: failed to force-destroy session {}: {}",
                    sid,
                    e
                );
                false
            }
        }
    }

    /// Gracefully close a session: inject exit command, poll for Exited, fallback to force on timeout.
    async fn graceful_close_session(
        &self,
        app: &tauri::AppHandle,
        sid: Uuid,
        timeout_secs: u32,
    ) -> bool {
        // Get session info to determine agent type
        let exit_cmd = {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            let sessions = mgr.list_sessions().await;
            match sessions.iter().find(|s| s.id == sid.to_string()) {
                Some(s) => Self::resolve_exit_command(&s.shell, &s.shell_args),
                None => {
                    log::warn!(
                        "[mailbox] close-session: session {} not found for graceful close",
                        sid
                    );
                    return false;
                }
            }
        };

        log::info!(
            "[mailbox] close-session: injecting '{}' into session {}",
            exit_cmd.escape_debug(),
            sid
        );

        // Inject exit command into PTY.
        // Clone the Arc so the State borrow is released, then lock+write+drop guard before any .await.
        let pty_arc = app
            .state::<Arc<std::sync::Mutex<crate::pty::manager::PtyManager>>>()
            .inner()
            .clone();
        let inject_result = match pty_arc.lock() {
            Ok(mgr) => {
                let res = mgr
                    .write(sid, exit_cmd.as_bytes())
                    .map_err(|e| e.to_string());
                drop(mgr);
                res
            }
            Err(e) => Err(format!("PTY lock failed: {}", e)),
        };
        if let Err(e) = inject_result {
            log::warn!(
                "[mailbox] close-session: PTY inject failed for {}: {}, falling back to force",
                sid,
                e
            );
            return self.force_close_session(app, sid).await;
        }

        // Poll for SessionStatus::Exited
        let timeout = std::time::Duration::from_secs(timeout_secs as u64);
        let poll_interval = std::time::Duration::from_secs(1);
        let start = std::time::Instant::now();

        loop {
            if start.elapsed() >= timeout {
                log::warn!(
                    "[mailbox] close-session: graceful timeout ({}s) for session {}, falling back to force",
                    timeout_secs, sid
                );
                return self.force_close_session(app, sid).await;
            }

            tokio::time::sleep(poll_interval).await;

            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            let sessions = mgr.list_sessions().await;
            match sessions.iter().find(|s| s.id == sid.to_string()) {
                Some(s) if matches!(s.status, SessionStatus::Exited(_)) => {
                    log::info!("[mailbox] close-session: session {} exited gracefully", sid);
                    drop(mgr);
                    // Clean up the exited session
                    return self.force_close_session(app, sid).await;
                }
                None => {
                    // Session already removed
                    log::info!("[mailbox] close-session: session {} already gone", sid);
                    return true;
                }
                _ => {} // still running, keep polling
            }
        }
    }

    /// Determine the exit command to inject based on the session's shell/agent type.
    /// Claude Code -> "/exit\r", generic shell/codex -> "exit\r"
    fn resolve_exit_command(shell: &str, shell_args: &[String]) -> String {
        let full_cmd = format!("{} {}", shell, shell_args.join(" "));
        let basenames: Vec<String> = full_cmd
            .split_whitespace()
            .map(|t| crate::commands::session::executable_basename(t))
            .collect();

        if basenames.iter().any(|b| b == "claude" || b == "aider") {
            "/exit\r".to_string()
        } else {
            // Codex, generic shell, and other CLIs
            "exit\r".to_string()
        }
    }

    /// Resolve the full filesystem path for an agent name.
    async fn resolve_repo_path(&self, agent_name: &str, app: &tauri::AppHandle) -> Option<String> {
        // Check session CWDs first
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        let dirs = mgr.get_sessions_working_dirs().await;

        for (_, cwd) in &dirs {
            let normalized = cwd.replace('\\', "/");
            if self.agent_name_from_path(cwd) == agent_name
                || normalized.ends_with(agent_name)
                || normalized.contains(&format!("/{}", agent_name))
            {
                return Some(cwd.clone());
            }
        }

        // Check settings project_paths
        let settings = app.state::<SettingsState>();
        let cfg = settings.read().await;
        for rp in &cfg.project_paths {
            let normalized = rp.replace('\\', "/");
            if self.agent_name_from_path(rp) == agent_name
                || normalized.ends_with(agent_name)
                || normalized.contains(&format!("/{}", agent_name))
            {
                return Some(rp.clone());
            }
        }

        // Check discovered teams for member paths
        let discovered_teams = teams::discover_teams();
        for team in &discovered_teams {
            for agent_path in team.agent_paths.iter().flatten() {
                let path_str = agent_path.to_string_lossy().to_string();
                let normalized = path_str.replace('\\', "/");
                if self.agent_name_from_path(&path_str) == agent_name
                    || normalized.ends_with(agent_name)
                    || normalized.contains(&format!("/{}", agent_name))
                {
                    return Some(path_str);
                }
            }
        }

        // Check WG replicas: "wg-name/agent" → scan project_paths for .ac-new/wg-name/__agent_agent/
        if agent_name.starts_with("wg-") {
            if let Some((wg_name, agent_short)) = agent_name.split_once('/') {
                let replica_dir = format!("__agent_{}", agent_short);
                for rp in &cfg.project_paths {
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
    fn can_reach(&self, from: &str, to: &str, discovered_teams: &[teams::DiscoveredTeam]) -> bool {
        crate::config::teams::can_communicate(from, to, discovered_teams)
    }

    /// Resolve which agent CLI to spawn when `deliver_wake` needs a new
    /// persistent session for the destination agent.
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
                if let Ok(local_config) = serde_json::from_str::<AgentLocalConfig>(&content) {
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

    /// Resolve agent_id and agent_label for a destination agent.
    /// Reads lastCodingAgent from the dest's config.json, then looks up the label in settings.
    async fn resolve_agent_id_and_label(
        &self,
        app: &tauri::AppHandle,
        cwd: &str,
    ) -> (Option<String>, Option<String>) {
        let config_path = std::path::Path::new(cwd)
            .join(crate::config::agent_local_dir_name())
            .join("config.json");

        let last_agent_id = std::fs::read_to_string(&config_path)
            .ok()
            .and_then(|content| serde_json::from_str::<AgentLocalConfig>(&content).ok())
            .and_then(|cfg| cfg.tooling.last_coding_agent);

        let Some(ref agent_id) = last_agent_id else {
            return (None, None);
        };

        let settings = app.state::<SettingsState>();
        let cfg = settings.read().await;
        let label = cfg
            .agents
            .iter()
            .find(|a| a.id == *agent_id)
            .map(|a| a.label.clone());

        (last_agent_id, label)
    }

    /// Fallback path resolution for WG agents: find a sibling session in the same WG,
    /// derive the WG directory from its CWD, and construct the target agent path.
    async fn resolve_wg_path_from_sessions(
        &self,
        app: &tauri::AppHandle,
        agent_name: &str,
    ) -> Option<String> {
        let (wg_name, agent_short) = agent_name.split_once('/')?;
        if !wg_name.starts_with("wg-") {
            return None;
        }

        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        let dirs = mgr.get_sessions_working_dirs().await;
        drop(mgr);

        let wg_marker = format!("/{}/", wg_name);
        for (_, cwd) in &dirs {
            let normalized = cwd.replace('\\', "/");
            if let Some(wg_pos) = normalized.rfind(&wg_marker) {
                let wg_dir = &normalized[..wg_pos + 1 + wg_name.len()];
                let candidate = format!("{}/__agent_{}", wg_dir, agent_short);
                if std::path::Path::new(&candidate).is_dir() {
                    log::info!(
                        "[mailbox] wake: resolved WG agent path from sibling session: {}",
                        candidate
                    );
                    return Some(
                        std::path::PathBuf::from(&candidate)
                            .to_string_lossy()
                            .to_string(),
                    );
                }
            }
        }

        None
    }

    /// Move an outbox message to outbox/delivered/ with token stripped.
    async fn move_to_delivered(&self, path: &Path, msg: &OutboxMessage) -> Result<(), String> {
        let delivered_dir = path.parent().ok_or("No parent dir")?.join("delivered");
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
        std::fs::remove_file(path).map_err(|e| format!("Failed to remove outbox file: {}", e))?;

        log::info!("[mailbox] Message {} moved to delivered/", msg.id);
        Ok(())
    }

    /// Reject a message: move to outbox/rejected/ with reason, and notify the sender.
    async fn reject_message(
        &self,
        path: &Path,
        msg: &OutboxMessage,
        reason: &str,
    ) -> Result<(), String> {
        let rejected_dir = path.parent().ok_or("No parent dir")?.join("rejected");
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
        std::fs::write(&dest, json).map_err(|e| format!("Failed to write rejected file: {}", e))?;

        // Remove original
        std::fs::remove_file(path).map_err(|e| format!("Failed to remove outbox file: {}", e))?;

        log::warn!(
            "[mailbox] Message {} moved to rejected/: {}",
            msg.id,
            reason
        );
        Ok(())
    }

    /// Reject a raw file that cannot be parsed as OutboxMessage.
    fn reject_raw_file(path: &Path, reason: &str) -> Result<(), String> {
        let rejected_dir = path.parent().ok_or("No parent dir")?.join("rejected");
        std::fs::create_dir_all(&rejected_dir)
            .map_err(|e| format!("Failed to create rejected dir: {}", e))?;

        let filename = path
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("unknown.json");

        let dest = rejected_dir.join(filename);
        std::fs::rename(path, &dest)
            .or_else(|_| std::fs::copy(path, &dest).and_then(|_| std::fs::remove_file(path)))
            .map_err(|e| format!("Failed to move file to rejected: {}", e))?;

        let stem = Path::new(filename)
            .file_stem()
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

            let request: crate::cli::create_agent::SessionRequest =
                match serde_json::from_str(&content) {
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
                request.session_name,
                request.cwd,
                request.agent_id
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
                Vec::new(), // git_repos
                false, // skip_auto_resume
            )
            .await
            {
                Ok(info) => {
                    log::info!(
                        "[session-requests] Created session '{}' (id={})",
                        request.session_name,
                        info.id
                    );
                }
                Err(e) => {
                    log::error!(
                        "[session-requests] Failed to create session '{}': {}",
                        request.session_name,
                        e
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
                    #   \"{exe}\" send --token {token} --root \"{root}\" --to \"<agent_name>\" --send <filename> --mode wake\n\
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

        match crate::pty::inject::inject_text_into_session(app, session_id, &notice, false).await {
            Ok(()) => log::info!("[mailbox] Fresh token injected into session {}", session_id),
            Err(e) => log::warn!(
                "[mailbox] Failed to inject fresh token into session {}: {}",
                session_id,
                e
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wake_action_running_injects() {
        assert_eq!(wake_action_for(&SessionStatus::Running), WakeAction::Inject);
    }

    #[test]
    fn wake_action_active_injects() {
        assert_eq!(wake_action_for(&SessionStatus::Active), WakeAction::Inject);
    }

    #[test]
    fn wake_action_idle_injects() {
        assert_eq!(wake_action_for(&SessionStatus::Idle), WakeAction::Inject);
    }

    #[test]
    fn wake_action_exited_respawns() {
        assert_eq!(
            wake_action_for(&SessionStatus::Exited(0)),
            WakeAction::RespawnExited
        );
        // Non-zero exit codes take the same path.
        assert_eq!(
            wake_action_for(&SessionStatus::Exited(1)),
            WakeAction::RespawnExited
        );
        assert_eq!(
            wake_action_for(&SessionStatus::Exited(-1)),
            WakeAction::RespawnExited
        );
    }
}
