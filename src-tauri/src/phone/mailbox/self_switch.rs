use super::handoff::{
    display_profile_for_log, ForgottenSummary, SelfHandoffFlavor, SelfRestartRecipe,
    SelfSwitchTargets, RAISE_HAND_ACTION, SELF_CLEAR_ACTION, SELF_CLEAR_SETTLE_SECS,
    SELF_RESTART_ACTION, SELF_SWITCH_ACTION,
};
use super::notice::{
    resolve_wake_agent_command_from_sources, retain_unarchived_session_dirs,
    ResolvedWakeAgentCommand, ResolvedWakeSpawnPlan,
};
use super::poller::MailboxPoller;
#[cfg(test)]
use super::pty_claim::MailboxTestEvent;
use super::self_handoff::failure::{SelfHandoffFailure, SelfHandoffOutcome, CONTINUATION_FAILURE};
use super::settle::{
    inject_handoff_prompt_with_archive, resolve_wg_path_from_session_dirs, self_clear_gate_advance,
    SelfClearBoundary, SelfClearGateAction, SelfClearGateState,
};
use crate::config::settings::SettingsState;
use crate::config::teams;
use crate::phone::types::OutboxMessage;
use crate::session::maintenance::PendingSelfClear;
use crate::session::manager::SessionManager;
use crate::session::session::SessionStatus;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::Manager;
use uuid::Uuid;

impl MailboxPoller {
    /// #1632 - despite the name, this drives BOTH self-handoff flavors: pass
    /// `SELF_HANDOFF_FLAVOR_SWITCH` for self-handoff-and-switch and
    /// `SELF_HANDOFF_FLAVOR_RESTART` for self-handoff-and-restart. The two differ only in the four
    /// `SelfHandoffFlavor` fields (three `&'static str` plus one prompt-builder `fn` pointer); the
    /// two-phase gate, the pending-set alias bookkeeping, and the archive-first phase-2 tail are
    /// shared and must not fork. The name is kept so a rename does not churn fifteen call sites
    /// (two production wrappers plus thirteen tests) for zero behavioral value.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn drive_self_switch_after_sustained_idle<
        SessionState,
        SessionFut,
        Persist,
        PersistFut,
        Restart,
        RestartFut,
        Inject,
        InjectFut,
        NoteBoundary,
        NoteFut,
    >(
        original_session_id: Uuid,
        cwd: PathBuf,
        target_agent: String,
        target_profile: String,
        forgotten_summary: Option<ForgottenSummary>,
        flavor: SelfHandoffFlavor,
        pending: Arc<PendingSelfClear>,
        settle: std::time::Duration,
        poll: std::time::Duration,
        max_defer: std::time::Duration,
        mut session_state: SessionState,
        mut persist: Persist,
        mut restart: Restart,
        mut inject: Inject,
        mut note_boundary: NoteBoundary,
    ) -> SelfHandoffOutcome
    where
        SessionState: FnMut(Uuid) -> SessionFut + Send + 'static,
        SessionFut: std::future::Future<Output = (bool, bool)> + Send,
        Persist: FnMut(PathBuf, String, String) -> PersistFut + Send + 'static,
        PersistFut: std::future::Future<Output = Result<(), String>> + Send,
        Restart: FnMut(Uuid, String, String) -> RestartFut + Send + 'static,
        RestartFut: std::future::Future<Output = Result<String, String>> + Send,
        Inject: FnMut(Uuid, String) -> InjectFut + Send + 'static,
        InjectFut: std::future::Future<Output = Result<(), String>> + Send,
        NoteBoundary: FnMut(Uuid, SelfClearBoundary) -> NoteFut + Send + 'static,
        NoteFut: std::future::Future<Output = ()> + Send,
    {
        let Some(lease) = pending.legacy_lease(original_session_id) else {
            log::warn!(
                "[mailbox] switch driver missing legacy reservation for {}",
                original_session_id
            );
            return SelfHandoffOutcome::Failed(SelfHandoffFailure {
                action: flavor.action,
                attempt: Uuid::new_v4(),
                original: original_session_id,
                current: Some(original_session_id),
                root: cwd,
                continuation: false,
                reason: format!(
                    "switch driver missing legacy reservation for {original_session_id}"
                ),
                forgotten: forgotten_summary.is_some(),
            });
        };
        let attempt = Uuid::new_v4();
        let mut continuation = false;
        let mut current = Some(original_session_id);
        let mut state = SelfClearGateState::new(std::time::Instant::now());
        let mut session_id = original_session_id;

        let reason = loop {
            tokio::time::sleep(poll).await;
            let (present, waiting) = session_state(session_id).await;
            let (next, action) = self_clear_gate_advance(
                state,
                present,
                waiting,
                std::time::Instant::now(),
                settle,
                max_defer,
            );
            state = next;

            match action {
                SelfClearGateAction::Wait => continue,
                SelfClearGateAction::InjectClear => {
                    log::info!(
                        "[mailbox] {}: session {} idle >={}s; {} with coding agent '{}' profile \
                        '{}' (phase 1)",
                        flavor.action,
                        session_id,
                        settle.as_secs(),
                        flavor.phase1_verb,
                        target_agent,
                        display_profile_for_log(&target_profile)
                    );
                    // #1940 - acquire the shared selection operation turn only here:
                    // the sustained-idle wait above stays outside it. The guard is
                    // held across the persist, the adjacent restart await and the
                    // pending-alias bookkeeping, and is released when this arm ends,
                    // before the phase-2 settling and handoff injection.
                    let _turn =
                        match crate::session::selection::acquire_selection_operation_turn().await {
                            Ok(turn) => turn,
                            Err(error) => {
                                log::warn!(
                                "[mailbox] {}: selection operation turn unavailable for session \
                                {}: {}",
                                flavor.action,
                                session_id,
                                error
                            );
                                break Some(error.to_string());
                            }
                        };
                    if let Err(e) =
                        persist(cwd.clone(), target_agent.clone(), target_profile.clone()).await
                    {
                        log::warn!(
                            "[mailbox] {}: persist failed for session {}: {}",
                            flavor.action,
                            original_session_id,
                            e
                        );
                        break Some(e);
                    }
                    let restarted =
                        restart(session_id, target_agent.clone(), target_profile.clone()).await;
                    let new_id = match restarted {
                        Ok(id) => match Uuid::parse_str(&id) {
                            Ok(uuid) => uuid,
                            Err(e) => {
                                log::warn!(
                                    "[mailbox] {}: restarted session id '{}' could not be parsed: \
                                    {}",
                                    flavor.action,
                                    id,
                                    e
                                );
                                current = None;
                                break Some(e.to_string());
                            }
                        },
                        Err(e) => {
                            log::warn!(
                                "[mailbox] {}: restart failed for session {}: {}",
                                flavor.action,
                                original_session_id,
                                e
                            );
                            break Some(e);
                        }
                    };
                    if let Err(error) = pending.alias(&lease, new_id) {
                        log::warn!(
                            "[mailbox] {}: new session {} alias refused: {:?}",
                            flavor.action,
                            new_id,
                            error
                        );
                    }
                    session_id = new_id;
                    current = Some(new_id);
                    continuation = true;
                    continue;
                }
                SelfClearGateAction::InjectHandoff => {
                    log::info!(
                        "[mailbox] {}: session {} idle >={}s {}; injecting handoff prompt (phase \
                        2)",
                        flavor.action,
                        session_id,
                        settle.as_secs(),
                        flavor.phase2_verb
                    );
                    // #749 - archive-first contract; see inject_handoff_prompt_with_archive.
                    // (#756) NOTE: the switch phase 1 fires NO Cleared event; it is a
                    // restart through restart_session_inner_with_activation (skip
                    // Some(true)), which stamps record + mirror itself via C3. The
                    // handoff lands on the CURRENT session_id (the post-switch id).
                    let injected = inject_handoff_prompt_with_archive(
                        &cwd,
                        session_id,
                        flavor.action,
                        flavor.build_prompt,
                        forgotten_summary.as_ref(),
                        &mut inject,
                    )
                    .await;
                    if injected {
                        // (#756) switch handoff prompt is post-boundary content.
                        note_boundary(session_id, SelfClearBoundary::ContentInjected).await;
                    }
                    break (!injected).then(|| CONTINUATION_FAILURE.to_string());
                }
                SelfClearGateAction::Abandon(reason) => {
                    log::warn!(
                        "[mailbox] {} ABANDONED for original session {} current session {}: {} \
                        (agent may re-issue)",
                        flavor.action,
                        original_session_id,
                        session_id,
                        reason
                    );
                    break Some(reason.to_string());
                }
            }
        };

        pending.release(&lease);
        match reason {
            None => SelfHandoffOutcome::Completed,
            Some(reason) => SelfHandoffOutcome::Failed(SelfHandoffFailure {
                action: flavor.action,
                attempt,
                original: original_session_id,
                current,
                root: cwd,
                continuation,
                reason,
                forgotten: forgotten_summary.is_some(),
            }),
        }
    }

    /// #617 - write the self-clear queue-ack response, then move the message to
    /// delivered/. Mirrors the close-session dual-write (self-contained copy; the
    /// landed close-session block is intentionally left untouched, blast radius 0).
    pub(crate) async fn write_self_clear_response<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &std::path::Path,
        msg: &OutboxMessage,
        session_id: Uuid,
        status: &str,
        is_app_outbox: bool,
    ) -> Result<(), String> {
        if let Some(ref rid) = msg.request_id {
            let response = serde_json::json!({
                "action": SELF_CLEAR_ACTION,
                "status": status,                 // "queued" | "already_queued"
                "session_id": session_id.to_string(),
                "settle_secs": SELF_CLEAR_SETTLE_SECS,   // single-sourced const
                "requested_by": msg.from,
            });
            if let Ok(json) = serde_json::to_string_pretty(&response) {
                // (1) outbox-relative <ac_dir>/responses/<rid>.json - skip for app-outbox.
                if !is_app_outbox {
                    if let Some(responses_dir) = path
                        .parent()
                        .and_then(|p| p.parent())
                        .map(|ac| ac.join("responses"))
                    {
                        let _ = std::fs::create_dir_all(&responses_dir);
                        let _ = std::fs::write(responses_dir.join(format!("{}.json", rid)), &json);
                    }
                }
                // (2) resolved-sender path (best-effort).
                if let Some(sender_path) = self.resolve_repo_path(&msg.from, app).await {
                    let responses_dir = std::path::PathBuf::from(sender_path)
                        .join(crate::config::agent_local_dir_name())
                        .join("responses");
                    let _ = std::fs::create_dir_all(&responses_dir);
                    let _ = std::fs::write(responses_dir.join(format!("{}.json", rid)), &json);
                }
            }
        }
        self.move_to_delivered(app, path, msg).await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn write_self_switch_response<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &std::path::Path,
        msg: &OutboxMessage,
        session_id: Uuid,
        status: &str,
        targets: &SelfSwitchTargets,
        is_app_outbox: bool,
    ) -> Result<(), String> {
        if let Some(ref rid) = msg.request_id {
            let response = serde_json::json!({
                "action": SELF_SWITCH_ACTION,
                "status": status,
                "session_id": session_id.to_string(),
                "settle_secs": SELF_CLEAR_SETTLE_SECS,
                "requested_by": msg.from,
                "target_coding_agent": targets.coding_agent,
                "target_profile": targets.profile,
            });
            if let Ok(json) = serde_json::to_string_pretty(&response) {
                if !is_app_outbox {
                    if let Some(responses_dir) = path
                        .parent()
                        .and_then(|p| p.parent())
                        .map(|ac| ac.join("responses"))
                    {
                        let _ = std::fs::create_dir_all(&responses_dir);
                        let _ = std::fs::write(responses_dir.join(format!("{}.json", rid)), &json);
                    }
                }
                if let Some(sender_path) = self.resolve_repo_path(&msg.from, app).await {
                    let responses_dir = std::path::PathBuf::from(sender_path)
                        .join(crate::config::agent_local_dir_name())
                        .join("responses");
                    let _ = std::fs::create_dir_all(&responses_dir);
                    let _ = std::fs::write(responses_dir.join(format!("{}.json", rid)), &json);
                }
            }
        }
        self.move_to_delivered(app, path, msg).await
    }

    /// #1632 - write the self-handoff-and-restart queue-ack response, then move the message to
    /// delivered/. `profile` serializes as JSON `null` when the session inherited its letter: the
    /// recipe pins nothing there, and reporting an invented `"A"` would be a lie about the pin.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn write_self_restart_response<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &std::path::Path,
        msg: &OutboxMessage,
        session_id: Uuid,
        status: &str,
        recipe: &SelfRestartRecipe,
        is_app_outbox: bool,
    ) -> Result<(), String> {
        if let Some(ref rid) = msg.request_id {
            let response = serde_json::json!({
                "action": SELF_RESTART_ACTION,
                "status": status,
                "session_id": session_id.to_string(),
                "settle_secs": SELF_CLEAR_SETTLE_SECS,
                "requested_by": msg.from,
                "coding_agent": recipe.agent,
                "profile": recipe.profile,
            });
            if let Ok(json) = serde_json::to_string_pretty(&response) {
                if !is_app_outbox {
                    if let Some(responses_dir) = path
                        .parent()
                        .and_then(|p| p.parent())
                        .map(|ac| ac.join("responses"))
                    {
                        let _ = std::fs::create_dir_all(&responses_dir);
                        let _ = std::fs::write(responses_dir.join(format!("{}.json", rid)), &json);
                    }
                }
                if let Some(sender_path) = self.resolve_repo_path(&msg.from, app).await {
                    let responses_dir = std::path::PathBuf::from(sender_path)
                        .join(crate::config::agent_local_dir_name())
                        .join("responses");
                    let _ = std::fs::create_dir_all(&responses_dir);
                    let _ = std::fs::write(responses_dir.join(format!("{}.json", rid)), &json);
                }
            }
        }
        self.move_to_delivered(app, path, msg).await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn write_raise_hand_response<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &std::path::Path,
        msg: &OutboxMessage,
        session_id: Uuid,
        raised: bool,
        status: &str,
        is_app_outbox: bool,
    ) -> Result<(), String> {
        if let Some(ref rid) = msg.request_id {
            let response = serde_json::json!({
                "action": RAISE_HAND_ACTION,
                "status": status,
                "raised": raised,
                "session_id": session_id.to_string(),
                "requested_by": msg.from,
            });
            if let Ok(json) = serde_json::to_string_pretty(&response) {
                if !is_app_outbox {
                    if let Some(responses_dir) = path
                        .parent()
                        .and_then(|p| p.parent())
                        .map(|ac| ac.join("responses"))
                    {
                        let _ = std::fs::create_dir_all(&responses_dir);
                        let _ = std::fs::write(responses_dir.join(format!("{}.json", rid)), &json);
                    }
                }
                if let Some(sender_path) = self.resolve_repo_path(&msg.from, app).await {
                    let responses_dir = std::path::PathBuf::from(sender_path)
                        .join(crate::config::agent_local_dir_name())
                        .join("responses");
                    let _ = std::fs::create_dir_all(&responses_dir);
                    let _ = std::fs::write(responses_dir.join(format!("{}.json", rid)), &json);
                }
            }
        }
        self.move_to_delivered(app, path, msg).await
    }

    /// Force-close a session immediately via destroy_session_inner.
    pub(crate) async fn force_close_session<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        sid: Uuid,
    ) -> bool {
        #[cfg(test)]
        {
            // (#885 E-2) Emit a Destroy event so purge tests can assert that
            // the destroy loop was or was not reached. Without this, the
            // "zero Destroy events" assertions in the F-7 and root-guard
            // tests are tautologies: the only emit site was in the wake
            // deferred-destroy path, which the purge never reaches.
            if let Some(hooks) = &self.test_hooks {
                {
                    let mut calls = hooks.destroy_calls.lock().unwrap();
                    calls.push(sid);
                }
                {
                    let mut events = hooks.events.lock().unwrap();
                    events.push(MailboxTestEvent::Destroy(sid));
                }
            }
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            return mgr.destroy_session(sid).await.is_ok();
        }

        #[cfg(not(test))]
        {
            match crate::commands::session::background_destroy_session_inner(app, sid).await {
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
    }

    /// Gracefully close a session: inject exit command, poll for Exited, fallback to force on
    /// timeout.
    pub(crate) async fn graceful_close_session<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
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

        // Graceful exit shares the same per-route input serialization as user,
        // automated, and privileged writers.
        let pty_arc = app
            .state::<Arc<std::sync::Mutex<crate::pty::manager::PtyManager>>>()
            .inner()
            .clone();
        let inject_result =
            match crate::pty::manager::PtyManager::acquire_input_writer(&pty_arc, sid).await {
                Ok(permit) => {
                    let result = crate::pty::manager::PtyManager::write_with_permit(
                        &permit,
                        exit_cmd.as_bytes(),
                    )
                    .map_err(|error| error.to_string());
                    if result.is_ok() {
                        crate::commands::pty::mark_successful_pty_write_busy(
                            app,
                            sid,
                            exit_cmd.len(),
                        )
                        .await;
                    }
                    result
                }
                Err(error) => Err(error.to_string()),
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
                    "[mailbox] close-session: graceful timeout ({}s) for session {}, falling back \
                    to force",
                    timeout_secs,
                    sid
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
            .map(crate::commands::session::executable_basename)
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
    pub(crate) async fn resolve_repo_path<R: tauri::Runtime>(
        &self,
        agent_name: &str,
        app: &tauri::AppHandle<R>,
    ) -> Option<String> {
        let (target_project, target_local) = crate::config::teams::split_project_prefix(agent_name);
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

        let settings = app.state::<SettingsState>();
        let (project_paths, archived) = {
            let cfg = settings.read().await;
            (
                cfg.project_paths.clone(),
                cfg.archived_project_paths.clone(),
            )
        };

        // Loop 1: session CWDs
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        let dirs = mgr.get_sessions_working_dirs().await;
        drop(mgr);
        let roots = crate::config::sessions_persistence::normalize_project_roots(&archived);
        let dirs = retain_unarchived_session_dirs(dirs, &roots);
        for (_, cwd) in &dirs {
            if hits_agent(cwd) {
                record_match(cwd, &mut matches);
            }
        }

        // Loop 2: settings project_paths
        for rp in &project_paths {
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

        // Loop 4: WG replica fallback. Scan `<workspace>/<wg>/__agent_<short>` under
        // project_paths (base + immediate non-dot children), honoring the target
        // project filter. §DR2-4 composition: push + break within a single `rp`
        // (an FQN matches at most one replica dir per project) but continue the
        // outer loop so ambiguity across projects is detected.
        if crate::config::entity_prefix::has_entity_prefix(target_local) {
            if let Some((wg_name, agent_short)) = target_local.split_once('/') {
                let replica_dir = format!("__agent_{}", agent_short);

                let project_matches = |dir_name: &str| -> bool {
                    match target_project {
                        Some(want) => dir_name == want,
                        None => true,
                    }
                };

                for rp in &project_paths {
                    let base = std::path::Path::new(rp);
                    if !crate::filesystem::projects::project_path_is_dir(base) {
                        continue;
                    }
                    let base_name = base.file_name().and_then(|n| n.to_str()).unwrap_or("");

                    let mut dirs_to_check: Vec<std::path::PathBuf> = Vec::new();
                    if project_matches(base_name) {
                        dirs_to_check.push(base.to_path_buf());
                    }
                    let _ =
                        crate::filesystem::projects::visit_project_kind_children(base, |item| {
                            let Ok(p) = item else {
                                return std::ops::ControlFlow::Continue(());
                            };
                            if !crate::filesystem::projects::project_path_is_dir(&p) {
                                return std::ops::ControlFlow::Continue(());
                            }
                            let dir_name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                            if dir_name.starts_with('.') {
                                return std::ops::ControlFlow::Continue(());
                            }
                            if !project_matches(dir_name) {
                                return std::ops::ControlFlow::Continue(());
                            }
                            dirs_to_check.push(p);
                            std::ops::ControlFlow::Continue(())
                        });

                    for dir in dirs_to_check {
                        let Some(ac_root) = crate::config::ac_root::existing_ac_root(&dir) else {
                            continue;
                        };
                        let candidate = ac_root.join(wg_name).join(&replica_dir);
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
                    "[mailbox] resolve_repo_path('{}'): {} candidates, refusing arbitrary pick: \
                    {:?}",
                    agent_name,
                    matches.len(),
                    matches
                );
                None
            }
        }
    }

    /// Check if sender can reach destination via team membership.
    /// Only agents in the same team can communicate — no parent directory fallback.
    pub(crate) fn can_reach(
        &self,
        from: &str,
        to: &str,
        discovered_teams: &[teams::DiscoveredTeam],
    ) -> bool {
        crate::config::teams::can_communicate(from, to, discovered_teams)
    }

    /// Resolve a wake fallback completely without preparing directories or
    /// mutating recipient lifecycle state. The returned command is carried
    /// unchanged through capability preflight and the later spawn.
    pub(crate) async fn resolve_wake_spawn_plan<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        msg: &OutboxMessage,
    ) -> Result<ResolvedWakeSpawnPlan, String> {
        let resolved_command = self.resolve_agent_command(app, msg).await?;
        let resolved_command = resolved_command.ok_or_else(|| {
            format!(
                "No agent command resolved for '{}'; preferredAgent={:?}. Configure \
                lastCodingAgent or agents in settings.",
                msg.to, msg.preferred_agent
            )
        })?;

        let cwd = match self.resolve_repo_path(&msg.to, app).await {
            Some(path) => path,
            None => self
                .resolve_wg_path_from_sessions(app, &msg.to)
                .await
                .ok_or_else(|| {
                    format!(
                        "Cannot resolve repo path for '{}' - cannot spawn session",
                        msg.to
                    )
                })?,
        };
        let session_name = {
            let (_, local) = crate::config::teams::split_project_prefix(&msg.to);
            local.to_string()
        };
        let (configured_spawn, resolved_agent_host_shell) =
            if let Some(agent_id) = resolved_command.agent_id.as_deref() {
                let settings = app.state::<SettingsState>();
                let cfg = settings.read().await;
                let spawn = crate::commands::session::resolve_configured_agent_spawn_for_cwd(
                    &cfg,
                    agent_id,
                    &cwd,
                    msg.requested_profile.as_deref(),
                    // #1635-2 D2: the dispatch request outranks the replica
                    // pin for this spawn only.
                    msg.requested_profile.is_some(),
                )?;
                // #1271 - same-guard host shell: copied from the exact snapshot
                // that built the spawn, before any await.
                let host_shell = if spawn.is_some() {
                    Some(crate::pty::backend::ResolvedAgentHostShell {
                        program: cfg.default_shell.clone(),
                        args: cfg.default_shell_args.clone(),
                    })
                } else {
                    None
                };
                (spawn, host_shell)
            } else {
                (None, None)
            };
        let (spawn_shell, spawn_args, spawn_label) = if let Some(spawn) = configured_spawn.as_ref()
        {
            (
                spawn.shell.clone(),
                spawn.shell_args.clone(),
                Some(spawn.trusted_agent_label.clone()),
            )
        } else {
            (
                resolved_command.shell.clone(),
                resolved_command.shell_args.clone(),
                resolved_command.agent_label.clone(),
            )
        };

        Ok(ResolvedWakeSpawnPlan {
            resolved_command,
            cwd,
            session_name,
            spawn_shell,
            spawn_args,
            spawn_label,
            configured_spawn,
            resolved_agent_host_shell,
        })
    }

    /// Resolve which agent CLI to spawn when `deliver_wake` needs a new
    /// persistent session for the destination agent.
    pub(crate) async fn resolve_agent_command<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        msg: &OutboxMessage,
    ) -> Result<Option<ResolvedWakeAgentCommand>, String> {
        let agents = {
            let settings = app.state::<SettingsState>();
            let cfg = settings.read().await;
            // #2716 (B3, option C) row 7: an unreadable agents layer is refused and
            // reported, never resolved against a list that is not the user's.
            if let Some(path) = cfg.agents_layer.unreadable_path() {
                return Err(crate::config::settings::agents_unreadable_session_error(
                    path,
                ));
            }
            cfg.agents.clone()
        };

        let mut destination_current_agent: Option<String> = None;
        let mut destination_last_agent: Option<String> = None;
        let mut destination_config_path: Option<PathBuf> = None;

        if let Some(dest_path) = self.resolve_repo_path(&msg.to, app).await {
            let dest = Path::new(&dest_path);
            // The Selection UI assignment (currentCodingAgent), including the
            // "Entire Workgroup" path, is written to the replica's top-level
            // config.json by set_replica_coding_agent_selection, not to the
            // per-instance agent_local_dir config that holds lastCodingAgent.
            destination_current_agent =
                crate::config::coding_agent_profiles::read_replica_current_coding_agent(dest);

            let config_dir = dest.join(crate::config::agent_local_dir_name());
            destination_config_path = Some(config_dir.join("config.json"));

            if let Some(local_config) =
                crate::config::agent_config::read_agent_local_config(&config_dir)
            {
                destination_last_agent = local_config.tooling.last_coding_agent;
            }
        }

        resolve_wake_agent_command_from_sources(
            &agents,
            &msg.preferred_agent,
            destination_current_agent.as_deref(),
            destination_last_agent.as_deref(),
            destination_config_path.as_deref(),
            msg.sender_agent.as_deref(),
        )
    }

    /// Fallback path resolution for WG agents: find a sibling session in the same WG,
    /// derive the WG directory from its CWD, and construct the target agent path.
    ///
    /// §4.4: peel optional `<project>:` prefix before splitting the local part.
    /// If the target is qualified, the returned candidate must also be in the
    /// same project (checked via derived FQN).
    pub(crate) async fn resolve_wg_path_from_sessions<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        agent_name: &str,
    ) -> Option<String> {
        let archived = {
            let settings = app.state::<SettingsState>();
            let cfg = settings.read().await;
            cfg.archived_project_paths.clone()
        };
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        let dirs = mgr.get_sessions_working_dirs().await;
        drop(mgr);
        let roots = crate::config::sessions_persistence::normalize_project_roots(&archived);
        let dirs = retain_unarchived_session_dirs(dirs, &roots);

        resolve_wg_path_from_session_dirs(&dirs, agent_name)
    }
}
