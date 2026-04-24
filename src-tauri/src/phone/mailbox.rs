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

/// §DR5 anti-spoof accept rule. Outbox-sender check passes when `msg_from`
/// equals `expected_from` exactly, OR when `msg_from` is unqualified (legacy)
/// AND its local part matches `expected_from`'s local part. Qualified-but-
/// wrong-project `msg_from` is always rejected.
pub(crate) fn anti_spoof_accept(msg_from: &str, expected_from: &str) -> bool {
    if msg_from == expected_from {
        return true;
    }
    let (_, exp_local) = crate::config::teams::split_project_prefix(expected_from);
    let (msg_proj, msg_local) = crate::config::teams::split_project_prefix(msg_from);
    msg_proj.is_none() && exp_local == msg_local
}

/// §AR2-norm step (1): upgrade a legacy-unqualified `msg_from` to
/// `expected_from` when expected_from is FQN. Returns true if the upgrade
/// happened. No-op when msg_from is already qualified, expected_from is
/// None, or expected_from itself is unqualified.
pub(crate) fn canonicalize_msg_from_in_place(
    msg_from: &mut String,
    expected_from: Option<&str>,
) -> bool {
    let Some(exp) = expected_from else {
        return false;
    };
    let (exp_proj, _) = crate::config::teams::split_project_prefix(exp);
    if exp_proj.is_none() {
        return false;
    }
    let (msg_proj, _) = crate::config::teams::split_project_prefix(msg_from);
    if msg_proj.is_some() {
        return false;
    }
    *msg_from = exp.to_string();
    true
}

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

        // `let mut msg`: §AR2-norm below mutates `msg.from` / `msg.to` in place
        // as the SINGLE POINT OF TRUTH for canonicalization. Downstream code
        // (routing, action dispatch, injection, archival) reads the canonical
        // form without re-mutation.
        let mut msg: OutboxMessage = serde_json::from_str(&content)
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
        //
        // §DR5 lenient fallback: if `msg.from` is unqualified (legacy) and its LOCAL
        // part matches `expected_from`'s local part, accept — §AR2-norm below then
        // upgrades `msg.from` to the canonical FQN. Cross-project qualified names
        // are rejected (different `project:` prefix).
        //
        // `expected_from` is hoisted (§DR2-3) so §AR2-norm can upgrade `msg.from`
        // even when this block has returned to outer scope.
        let mut expected_from: Option<String> = None;
        if !is_app_outbox {
            let outbox_dir = path.parent().unwrap_or(Path::new(""));
            // outbox_dir is <repo>/.agentscommander/outbox — go up 2 levels to get the repo path
            if let Some(repo_path) = outbox_dir.parent().and_then(|p| p.parent()) {
                let derived = crate::config::teams::agent_fqn_from_path(
                    &repo_path.to_string_lossy(),
                );
                if !anti_spoof_accept(&msg.from, &derived) {
                    return self
                        .reject_message(
                            path,
                            &msg,
                            &format!(
                                "Outbox-sender mismatch: outbox belongs to '{}' but message claims '{}'",
                                derived, msg.from
                            ),
                        )
                        .await;
                }
                expected_from = Some(derived);
            }
        }

        // ── §AR2-norm — SINGLE POINT OF TRUTH: msg.from / msg.to canonicalization ──
        //
        // Runs AFTER anti-spoof and BEFORE token validation / routing / action dispatch.
        // Every downstream read of msg.from / msg.to sees the canonical FQN (or bare
        // legacy form when canonicalization wasn't possible). Downstream code MUST
        // NOT re-mutate msg.from or msg.to.

        // (1) Upgrade a legacy-unqualified msg.from to the anti-spoof-derived
        // expected_from FQN (closes grinch §G5: resolve_repo_path(&msg.from)
        // for response-dir lookup now sees a canonical input).
        let original_from_for_log = msg.from.clone();
        if canonicalize_msg_from_in_place(&mut msg.from, expected_from.as_deref()) {
            log::info!(
                "[mailbox] canonicalized legacy msg.from '{}' → '{}'",
                original_from_for_log, msg.from
            );
        }

        // (2) Canonicalize msg.to via the shared resolver. Empty `to` is allowed for
        // action-dispatch paths (e.g. close-session may set an empty to); skip in
        // that case. Reject-on-ambiguity semantics match the CLI (Decision 2 rule 2c).
        if !msg.to.is_empty() {
            let paths = {
                let cfg = app.state::<SettingsState>();
                let c = cfg.read().await;
                c.project_paths.clone()
            };
            match crate::config::teams::resolve_agent_target(&msg.to, &paths) {
                Ok(fqn) => {
                    if fqn != msg.to {
                        log::info!(
                            "[mailbox] canonicalized msg.to '{}' → '{}'",
                            msg.to, fqn
                        );
                        msg.to = fqn;
                    }
                }
                Err(e) => {
                    return self
                        .reject_message(
                            path,
                            &msg,
                            &format!("Unresolvable target: {}", e),
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
                            // Anti-spoofing: verify msg.from matches the token's session CWD.
                            // Post-§AR2-norm, msg.from is canonical FQN (or legacy unqualified
                            // if expected_from was unavailable). Session-derived name uses the
                            // canonical helper; comparison is exact equality.
                            let session_name = crate::config::teams::agent_fqn_from_path(
                                &session.working_directory,
                            );
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

        // §AR2-session-name: strip optional `<project>:` prefix from the display
        // name so the sidebar label stays short (e.g. "wg-1-devs/tech-lead" not
        // "proj-a:wg-1-devs/tech-lead"). The canonical FQN stays recoverable via
        // `agent_fqn_from_path(&cwd)` at any list-sessions time.
        let session_name = {
            let (_, local) = crate::config::teams::split_project_prefix(&msg.to);
            local.to_string()
        };

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

            // Post-command background work:
            //  - For `/clear` on an agent session: re-inject credentials (Claude wipes
            //    them with the context), then the body follow-up if present.
            //  - For `/compact` (or `/clear` on a plain shell): body follow-up only.
            // Never block the delivery pipeline — spawn as a detached task.
            let is_clear = command == "clear";
            let app_clone = app.clone();
            let msg_clone = msg.clone();
            let command_owned = command.clone();
            tauri::async_runtime::spawn(async move {
                if is_clear {
                    match Self::reinject_credentials_after_clear_static(
                        &app_clone,
                        session_id,
                    )
                    .await
                    {
                        Ok(()) => { /* creds in — fall through to body */ }
                        Err(e) => {
                            log::warn!(
                                "[mailbox] Cred re-inject after /clear failed (session={}): {} \
                                 — skipping body follow-up to avoid delivering a message the \
                                 agent cannot reply to",
                                session_id,
                                e
                            );
                            return;
                        }
                    }
                }
                if !msg_clone.body.is_empty() {
                    if let Err(e) = Self::inject_followup_after_idle_static(
                        &app_clone,
                        session_id,
                        &msg_clone,
                    )
                    .await
                    {
                        log::warn!(
                            "[mailbox] Follow-up body injection after /{} failed (session={}): {}",
                            command_owned,
                            session_id,
                            e
                        );
                    }
                }
            });

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

        // SECURITY: this `first_100` log MUST NOT see credential blocks.
        // Cred re-inject (reinject_credentials_after_clear_static) and spawn-path
        // cred inject call inject_text_into_session directly, bypassing this
        // branch. Do not refactor without re-verifying.
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

    /// Wait for agent to become idle after `/clear`, then re-inject the
    /// credentials block so the agent keeps its Token/Root/BinaryPath/
    /// LocalDir after its context window was wiped. Best-effort only.
    ///
    /// Gated to agent sessions (`session.agent_id.is_some()`). Plain shell
    /// sessions skip silently with `Ok(())`.
    ///
    /// Static — safe to spawn as a detached task without borrowing self.
    async fn reinject_credentials_after_clear_static(
        app: &tauri::AppHandle,
        session_id: Uuid,
    ) -> Result<(), String> {
        // Resolve the session once up front. We need `agent_id`, `token`,
        // and `working_directory`. Fields are immutable for the lifetime
        // of the session; the idle-poll loop below uses `list_sessions`
        // to detect destruction tick-by-tick.
        let (token, cwd) = {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            match mgr.get_session(session_id).await {
                Some(s) if s.agent_id.is_some() => (s.token, s.working_directory.clone()),
                Some(_) => {
                    // Not an agent session — nothing to re-inject.
                    return Ok(());
                }
                None => {
                    return Err(format!(
                        "Session {} gone before credential re-inject",
                        session_id
                    ));
                }
            }
        };

        let max_wait = std::time::Duration::from_secs(30);
        let poll = std::time::Duration::from_millis(500);
        let start = std::time::Instant::now();

        // Wait for idle (waiting_for_input = true)
        loop {
            if start.elapsed() >= max_wait {
                return Err(format!(
                    "Timeout waiting for idle before credential re-inject ({}s)",
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
                        "Session {} destroyed during credential re-inject poll",
                        session_id
                    ));
                }
            }
        }

        // Build + inject. Same call shape as spawn path.
        let cred_block = crate::pty::credentials::build_credentials_block(&token, &cwd);
        crate::pty::inject::inject_text_into_session(app, session_id, &cred_block, true).await?;

        log::info!(
            "[mailbox] Credentials re-injected after /clear (session={})",
            session_id
        );
        Ok(())
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

        // §AR2-G2: exact-FQN filter. Post-§AR2-norm, `agent_name` is canonical
        // (or a legacy form that genuinely matches only one project's CWD). The
        // substring/suffix fuzziness from the pre-fix code is gone — cross-project
        // leakage is impossible at this layer.
        let mut matches: Vec<&crate::session::session::SessionInfo> = sessions
            .iter()
            .filter(|s| {
                crate::config::teams::agent_fqn_from_path(&s.working_directory)
                    == agent_name
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
    ///
    /// §AR2-G2: exact-FQN filter (same simplification as `find_active_session`).
    async fn find_all_sessions(&self, app: &tauri::AppHandle, agent_name: &str) -> Vec<Uuid> {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        let sessions = mgr.list_sessions().await;

        sessions
            .iter()
            .filter(|s| {
                crate::config::teams::agent_fqn_from_path(&s.working_directory)
                    == agent_name
            })
            .filter_map(|s| Uuid::parse_str(&s.id).ok())
            .collect()
    }

    /// Handle close-session action: validate coordinator auth, find target sessions, destroy them.
    ///
    /// NEW ACTION HANDLERS: resolve user-supplied target fields via
    /// `config::teams::resolve_agent_target` BEFORE privileged operations.
    /// The outbox is a trust boundary — any new destructive action must
    /// canonicalize its target here, not rely on CLI-side resolution.
    async fn handle_close_session(
        &self,
        app: &tauri::AppHandle,
        path: &std::path::Path,
        msg: &OutboxMessage,
    ) -> Result<(), String> {
        let raw_target = msg
            .target
            .as_deref()
            .ok_or_else(|| "close-session requires 'target' field".to_string())?;

        // §AR2-G1: resolve the target to a canonical FQN BEFORE authorization.
        // Even if the CLI skipped resolution (direct outbox write, old client,
        // hand-crafted JSON), the mailbox is the authoritative gate.
        let resolved_target = {
            let paths = {
                let cfg = app.state::<SettingsState>();
                let c = cfg.read().await;
                c.project_paths.clone()
            };
            match crate::config::teams::resolve_agent_target(raw_target, &paths) {
                Ok(fqn) => fqn,
                Err(e) => {
                    return self
                        .reject_message(
                            path,
                            msg,
                            &format!("close-session target unresolvable: {}", e),
                        )
                        .await;
                }
            }
        };
        let target = resolved_target.as_str();

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
    ///
    /// §AR2-G4: collector pattern. For qualified inputs, an FQN matches at most
    /// one CWD/path/team-member entry per iteration (by construction) — the
    /// dedupe is defense-in-depth for redundant registrations. For unqualified
    /// inputs (legacy), local-part matches across multiple projects return
    /// `None` rather than arbitrarily picking one.
    ///
    /// §AR2-G3: WG fallback seed honors the target project filter. Combined
    /// with §DR2-4 composition: `matches.push(...); break;` within a single
    /// `rp` iteration (FQN can only match one replica dir per project) while
    /// the outer loop continues so cross-project ambiguity is still detected.
    async fn resolve_repo_path(&self, agent_name: &str, app: &tauri::AppHandle) -> Option<String> {
        let (target_project, target_local) =
            crate::config::teams::split_project_prefix(agent_name);
        let is_qualified = target_project.is_some();
        let mut matches: Vec<String> = Vec::new();

        let record_match = |path_str: &str, out: &mut Vec<String>| {
            if !out.iter().any(|m| m == path_str) {
                out.push(path_str.to_string());
            }
        };

        let hits_agent = |cwd: &str| -> bool {
            let path_fqn = crate::config::teams::agent_fqn_from_path(cwd);
            if is_qualified {
                path_fqn == agent_name
            } else {
                let (_, path_local) = crate::config::teams::split_project_prefix(&path_fqn);
                path_local == target_local
            }
        };

        // Loop 1: session CWDs
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        let dirs = mgr.get_sessions_working_dirs().await;
        for (_, cwd) in &dirs {
            if hits_agent(cwd) {
                record_match(cwd, &mut matches);
            }
        }
        drop(mgr);

        // Loop 2: settings project_paths
        let settings = app.state::<SettingsState>();
        let cfg = settings.read().await;
        for rp in &cfg.project_paths {
            if hits_agent(rp) {
                record_match(rp, &mut matches);
            }
        }

        // Loop 3: discovered team member paths. Short-circuit by project when
        // the target is qualified (team.project matches target_project).
        let discovered_teams = teams::discover_teams();
        for team in &discovered_teams {
            if let Some(want) = target_project {
                if team.project != want {
                    continue;
                }
            }
            for agent_path in team.agent_paths.iter().flatten() {
                let path_str = agent_path.to_string_lossy().to_string();
                if hits_agent(&path_str) {
                    record_match(&path_str, &mut matches);
                }
            }
        }

        // Loop 4: WG replica fallback. Scan `.ac-new/<wg>/__agent_<short>` under
        // project_paths (base + immediate non-dot children), honoring the target
        // project filter. §DR2-4 composition: push + break within a single `rp`
        // (an FQN matches at most one replica dir per project) but continue the
        // outer loop so ambiguity across projects is detected.
        if target_local.starts_with("wg-") {
            if let Some((wg_name, agent_short)) = target_local.split_once('/') {
                let replica_dir = format!("__agent_{}", agent_short);

                let project_matches = |dir_name: &str| -> bool {
                    match target_project {
                        Some(want) => dir_name == want,
                        None => true,
                    }
                };

                for rp in &cfg.project_paths {
                    let base = std::path::Path::new(rp);
                    if !base.is_dir() {
                        continue;
                    }
                    let base_name = base.file_name().and_then(|n| n.to_str()).unwrap_or("");

                    let mut dirs_to_check: Vec<std::path::PathBuf> = Vec::new();
                    if project_matches(base_name) {
                        dirs_to_check.push(base.to_path_buf());
                    }
                    if let Ok(entries) = std::fs::read_dir(base) {
                        for entry in entries.flatten() {
                            let p = entry.path();
                            if !p.is_dir() {
                                continue;
                            }
                            let dir_name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                            if dir_name.starts_with('.') {
                                continue;
                            }
                            if !project_matches(dir_name) {
                                continue;
                            }
                            dirs_to_check.push(p);
                        }
                    }

                    for dir in dirs_to_check {
                        let candidate = dir.join(".ac-new").join(wg_name).join(&replica_dir);
                        if candidate.is_dir() {
                            record_match(&candidate.to_string_lossy(), &mut matches);
                            // Within a single `rp`, first hit is the unique hit —
                            // an FQN matches one replica dir per project. Continue
                            // OUTER loop to detect cross-project ambiguity (§DR2-4).
                            break;
                        }
                    }
                }
            }
        }

        match matches.len() {
            0 => None,
            1 => Some(matches.pop().unwrap()),
            _ => {
                log::warn!(
                    "[mailbox] resolve_repo_path('{}'): {} candidates, refusing arbitrary pick: {:?}",
                    agent_name, matches.len(), matches
                );
                None
            }
        }
    }

    // Shadow `agent_name_from_path` removed — all mailbox call sites now use
    // `crate::config::teams::agent_fqn_from_path` per §AR2 (§DR2 consolidation).

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
    ///
    /// §4.4: peel optional `<project>:` prefix before splitting the local part.
    /// If the target is qualified, the returned candidate must also be in the
    /// same project (checked via derived FQN).
    async fn resolve_wg_path_from_sessions(
        &self,
        app: &tauri::AppHandle,
        agent_name: &str,
    ) -> Option<String> {
        let (target_project, local) = crate::config::teams::split_project_prefix(agent_name);
        let (wg_name, agent_short) = local.split_once('/')?;
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
                    // If target is qualified, enforce same project on the candidate.
                    if let Some(want) = target_project {
                        let candidate_fqn =
                            crate::config::teams::agent_fqn_from_path(&candidate);
                        let (cand_project, _) =
                            crate::config::teams::split_project_prefix(&candidate_fqn);
                        if cand_project != Some(want) {
                            continue;
                        }
                    }
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

    // ── Anti-spoof / canonicalization pure-logic tests (AR2-tests 22, 23 + DR2-5) ──

    /// §DR7 / AR2-tests #22: legacy-unqualified msg.from is accepted when its
    /// local part matches the anti-spoof expected_from (local-only fallback).
    #[test]
    fn anti_spoof_legacy_msg_from_accepted_by_local_match() {
        // expected_from derived from repo path (canonical FQN).
        let expected = "proj-a:wg-1-devs/tech-lead";
        // msg.from is legacy unqualified with same local part.
        let msg_from = "wg-1-devs/tech-lead";
        assert!(anti_spoof_accept(msg_from, expected));
    }

    /// §DR7 / AR2-tests #23: qualified-but-wrong-project msg.from is rejected.
    /// A naïve suffix match would accept this — the LOCAL-only fallback rejects.
    #[test]
    fn anti_spoof_cross_project_qualified_msg_from_rejected() {
        let expected = "proj-a:wg-1-devs/tech-lead";
        let spoofed = "proj-b:wg-1-devs/tech-lead";
        assert!(!anti_spoof_accept(spoofed, expected));
    }

    /// Exact-FQN match is trivially accepted (baseline).
    #[test]
    fn anti_spoof_exact_fqn_match_accepted() {
        let expected = "proj-a:wg-1-devs/tech-lead";
        assert!(anti_spoof_accept(expected, expected));
    }

    /// §DR2-5 / AR2-tests #25: §AR2-norm step (1) upgrades a legacy-unqualified
    /// msg.from to the anti-spoof expected_from FQN. Without this upgrade,
    /// grinch §G5's `resolve_repo_path(&msg.from)` response-dir lookup could
    /// receive an ambiguous-local-part input.
    #[test]
    fn process_message_canonicalizes_legacy_msg_from() {
        let mut msg_from = "wg-1-devs/tech-lead".to_string();
        let expected_from = "proj-a:wg-1-devs/tech-lead";
        let upgraded = canonicalize_msg_from_in_place(&mut msg_from, Some(expected_from));
        assert!(upgraded);
        assert_eq!(msg_from, "proj-a:wg-1-devs/tech-lead");
    }

    /// Already-qualified msg.from is NOT overwritten by canonicalization.
    #[test]
    fn canonicalize_noop_for_already_qualified_msg_from() {
        let mut msg_from = "proj-a:wg-1-devs/tech-lead".to_string();
        let upgraded = canonicalize_msg_from_in_place(
            &mut msg_from,
            Some("proj-b:wg-1-devs/tech-lead"), // different project — don't overwrite!
        );
        assert!(!upgraded);
        assert_eq!(msg_from, "proj-a:wg-1-devs/tech-lead");
    }

    /// No expected_from (app outbox path) → canonicalization is a no-op.
    #[test]
    fn canonicalize_noop_when_expected_from_absent() {
        let mut msg_from = "wg-1-devs/alice".to_string();
        let upgraded = canonicalize_msg_from_in_place(&mut msg_from, None);
        assert!(!upgraded);
        assert_eq!(msg_from, "wg-1-devs/alice");
    }

    // ── Full mailbox-pipeline tests marked [INT] — placeholders for a future
    // two-project fixture harness. Acknowledged to ship with the fix per
    // tech-lead's must-apply directive; the pure-logic tests above cover the
    // §G1, §G2, §G5 regression surface at the unit level, and §AR2-G1's
    // close-session resolver gate is covered by the §AR2-shared resolver
    // tests (config::teams::tests::resolve_agent_target_*). ──

    /// §G9#1 / AR2-tests #17 — close-session with unqualified target from a
    /// direct outbox write MUST NOT destroy sessions in an unauthorized
    /// project. Covered at the resolver layer by
    /// `resolve_agent_target_rejects_ambiguous` and
    /// `resolve_agent_target_two_level_scan` in `config/teams.rs::tests`;
    /// `handle_close_session`'s §AR2-G1 gate calls `resolve_agent_target`
    /// before authorization, so rejecting Ambiguous at that layer blocks the
    /// attack before any session is touched. Full end-to-end fixture needs a
    /// Tauri `AppHandle` harness — follow-up.
    #[test]
    #[ignore = "integration: needs two-project Tauri AppHandle fixture"]
    fn close_session_rejects_direct_outbox_write_with_unqualified_target() {
        // Full-pipeline assertion stub; logic-layer coverage lives in:
        //   - config::teams::tests::resolve_agent_target_rejects_ambiguous
        //   - config::teams::tests::resolve_agent_target_two_level_scan
        //   - config::teams::tests::is_coordinator_rejects_legacy_unqualified_from
    }

    /// §G9#2 / AR2-tests #18 — wake with ambiguous unqualified `to` MUST be
    /// rejected, not silently routed. Covered at the resolver layer by the
    /// same `resolve_agent_target_rejects_ambiguous` test; `process_message`
    /// calls `resolve_agent_target` on `msg.to` at §AR2-norm before mode
    /// dispatch, so ambiguous `msg.to` becomes a rejected outbox message.
    /// Full end-to-end fixture needs an AppHandle harness.
    #[test]
    #[ignore = "integration: needs two-project Tauri AppHandle fixture"]
    fn deliver_wake_rejects_unqualified_to_with_cross_project_matches() {
        // Full-pipeline assertion stub; logic-layer coverage lives in:
        //   - config::teams::tests::resolve_agent_target_rejects_ambiguous
        //   - config::teams::tests::resolve_agent_target_two_level_scan
    }

    /// §G9#3 / AR2-tests #19 — `resolve_repo_path` WG fallback with a
    /// qualified target honors `target_project` (no cross-project leak even
    /// when the base dir `rp` is another project's root). Logic covered by
    /// the `project_matches` closure + `dirs_to_check` seeding in the
    /// §AR2-G3 block; a full fixture-based test needs filesystem setup
    /// under an AppHandle harness.
    #[test]
    #[ignore = "integration: needs filesystem fixture + AppHandle"]
    fn resolve_repo_path_wg_fallback_honors_target_project() {}

    /// §G9#4 / AR2-tests #20 — `resolve_repo_path` with an unqualified target
    /// matching multiple projects returns `None` (refuses arbitrary pick).
    /// §AR2-G4 collector pattern logic is covered by inspection — the
    /// `matches.len()` match arm returns None on `_`. Full integration test
    /// needs AppHandle + session-CWDs fixture.
    #[test]
    #[ignore = "integration: needs filesystem fixture + AppHandle"]
    fn resolve_repo_path_returns_none_on_ambiguous_unqualified() {}

    /// §G9#8 / AR2-tests #21 — a session spawned by `deliver_wake` from an
    /// FQN `msg.to` has a sidebar `Session.name` WITHOUT the `:` prefix.
    /// §AR2-session-name handles this at mailbox.rs (spawn path) via
    /// `split_project_prefix(&msg.to).1`. The logic is one line; a full
    /// integration test needs a Tauri runtime.
    #[test]
    #[ignore = "integration: needs Tauri runtime + session manager fixture"]
    fn deliver_wake_spawned_session_name_has_no_colon() {}

    /// §G9#9 / AR2-tests #24 — full round-trip CLI send → mailbox route →
    /// reply. Intentionally integration-level; no unit scaffolding.
    #[test]
    #[ignore = "integration: full CLI + mailbox + two-project fixture"]
    fn resolve_to_target_round_trip_integration() {}
}
