pub(crate) mod failure;
#[cfg(not(test))]
use super::handoff::encode_restart_profile_for_seam;
#[cfg(not(test))]
use super::handoff::SELF_CLEAR_MAX_DEFER;
#[cfg(not(test))]
use super::handoff::SELF_CLEAR_POLL;
#[cfg(not(test))]
use super::handoff::SELF_CLEAR_SETTLE;
use super::handoff::{
    build_self_clear_handoff_prompt, decode_restart_profile_from_seam, resolve_self_restart_recipe,
    resolve_switch_targets, validate_self_switch_spawn, validate_self_switch_wg_replica,
    ForgottenSummary, SelfHandoffFlavor, ARCHIVE_TIMESTAMP_FORMAT, SELF_CLEAR_ACTION,
    SELF_HANDOFF_FLAVOR_RESTART, SELF_HANDOFF_FLAVOR_SWITCH, SELF_HANDOFF_ROOT_NAME,
};
use super::poller::MailboxPoller;
use super::settle::{
    archive_root_md, capture_self_forget_summary, inject_handoff_prompt_with_archive,
    self_clear_gate_advance, SelfClearBoundary, SelfClearGateAction, SelfClearGateState,
};
use crate::config::settings::SettingsState;
use crate::phone::types::OutboxMessage;
use crate::pty::manager::PtyManager;
use crate::session::maintenance::{MaintenanceError, PendingSelfClear};
use crate::session::manager::SessionManager;
use failure::{consume_outcome, SelfHandoffFailure, SelfHandoffOutcome, CONTINUATION_FAILURE};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use tauri::Manager;
use uuid::Uuid;

impl MailboxPoller {
    /// #668 - queue a deferred self switch for the session that owns `msg.token`.
    /// It shares the self context pending set with self-clear, so clear and switch
    /// requests cannot stack on the same live session.
    pub(crate) async fn handle_self_handoff_switch<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &std::path::Path,
        msg: &OutboxMessage,
        is_app_outbox: bool,
    ) -> Result<(), String> {
        let token_uuid = match msg.token.as_deref().and_then(|t| Uuid::parse_str(t).ok()) {
            Some(u) => u,
            None => {
                return self
                    .reject_message(
                        path,
                        msg,
                        "self-handoff-and-switch requires a valid session token; restart or \
                        respawn the session",
                    )
                    .await;
            }
        };
        let session = {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            mgr.find_by_token(token_uuid).await
        };
        let session = match session {
            Some(s) => s,
            None => {
                return self
                    .reject_message(
                        path,
                        msg,
                        "self-handoff-and-switch: no live session owns this token; restart or \
                        respawn the session",
                    )
                    .await;
            }
        };
        let session_id = match Uuid::parse_str(&session.id) {
            Ok(u) => u,
            Err(_) => {
                return self
                    .reject_message(
                        path,
                        msg,
                        "self-handoff-and-switch: internal error resolving session id",
                    )
                    .await;
            }
        };

        if !crate::pty::inject::supports_self_handoff_switch(&session.shell) {
            return self
                .reject_message(
                    path,
                    msg,
                    &format!(
                        "self-handoff-and-switch: session shell '{}' is not a supported source \
                        shell (Claude / Codex / Antigravity direct family, Cursor agent, or Pi); \
                        switch is not supported here",
                        session.shell
                    ),
                )
                .await;
        }

        let settings_snapshot = {
            let settings = app.state::<SettingsState>();
            let snapshot = settings.read().await.clone();
            snapshot
        };
        let session_cwd = PathBuf::from(&session.working_directory);
        let replica_path = match validate_self_switch_wg_replica(&settings_snapshot, &session_cwd) {
            Ok(path) => path,
            Err(reason) => return self.reject_message(path, msg, &reason).await,
        };
        let targets = match resolve_switch_targets(
            &settings_snapshot,
            &replica_path,
            msg.switch_coding_agent.as_deref(),
            msg.switch_profile.as_deref(),
            session.agent_id.as_deref(),
            session.effective_profile.as_deref(),
            session.requested_profile.as_deref(),
        ) {
            Ok(targets) => targets,
            Err(reason) => return self.reject_message(path, msg, &reason).await,
        };
        if let Err(reason) = validate_self_switch_spawn(&settings_snapshot, &replica_path, &targets)
        {
            return self.reject_message(path, msg, &reason).await;
        }

        let handoff_path = replica_path.join(SELF_HANDOFF_ROOT_NAME);
        if !handoff_path.is_file() {
            return self
                .reject_message(
                    path,
                    msg,
                    &format!(
                        "self-handoff-and-switch: SELF-HANDOFF.md not found in your root ({}); \
                        write it before requesting self-handoff-and-switch.",
                        replica_path.display()
                    ),
                )
                .await;
        }

        let reservation = {
            let pending = app.state::<Arc<PendingSelfClear>>();
            pending.reserve_legacy(session_id)
        };
        let newly_inserted = match reservation {
            Ok(_) => true,
            Err(MaintenanceError::AlreadyLegacy) => false,
            Err(error) => {
                return self.reject_message(path, msg, &error.to_string()).await;
            }
        };
        let status = if newly_inserted {
            "queued"
        } else {
            "already_queued"
        };

        if newly_inserted {
            let forgotten_summary = capture_self_forget_summary(&replica_path);
            let ts = chrono::Local::now()
                .format(ARCHIVE_TIMESTAMP_FORMAT)
                .to_string();
            match archive_root_md(&replica_path, "SELF-FORGET", &ts) {
                Ok(Some(p)) => log::info!(
                    "[mailbox] self-handoff-and-switch: archived SELF-FORGET.md -> {}",
                    p.display()
                ),
                Ok(None) => {}
                Err(e) => log::warn!(
                    "[mailbox] self-handoff-and-switch: SELF-FORGET.md archive failed for session \
                    {} (non-fatal): {}",
                    session_id,
                    e
                ),
            }

            #[cfg(not(test))]
            {
                let app_clone = app.clone();
                let target_agent = targets.coding_agent.clone();
                let target_profile = targets.profile.clone();
                let replica_path = replica_path.clone();
                tauri::async_runtime::spawn(async move {
                    Self::run_self_switch_after_sustained_idle(
                        &app_clone,
                        session_id,
                        replica_path,
                        target_agent,
                        target_profile,
                        forgotten_summary,
                        SELF_HANDOFF_FLAVOR_SWITCH,
                        SELF_CLEAR_SETTLE,
                        SELF_CLEAR_POLL,
                        SELF_CLEAR_MAX_DEFER,
                    )
                    .await;
                });
            }
            #[cfg(test)]
            let _ = &forgotten_summary;
            log::info!(
                "[mailbox] self-handoff-and-switch queued for session {} target coding agent '{}' \
                profile '{}' (from '{}')",
                session_id,
                targets.coding_agent,
                targets.profile,
                msg.from
            );
        } else {
            log::info!(
                "[mailbox] self-handoff-and-switch already pending for session {} (from '{}')",
                session_id,
                msg.from
            );
        }

        self.write_self_switch_response(app, path, msg, session_id, status, &targets, is_app_outbox)
            .await
    }

    /// #1632 - self-handoff-and-restart: hand off through `SELF-HANDOFF.md`, then respawn the
    /// caller's own session on the SAME configured coding agent and profile letter it is already
    /// running, then resume in the NEW session. Scope is every agent: Room replicas, origin Agent
    /// Matrix agents, and the Root Agent.
    ///
    /// Gate order is load-bearing: identity (4) before existence (5) before launchability (6)
    /// before the pending insert (7) and the SELF-FORGET archive (8). The property the contract
    /// rests on is that EVERY rejection happens with nothing queued and nothing archived. Step 6
    /// is NOT read-only (the spawn builder can `create_dir_all` an isolated `CODEX_HOME` and an
    /// absolute `OPENCODE_CONFIG_DIR`), which is parity with the landed switch handler's
    /// `validate_self_switch_spawn` call; that is exactly why the pure `is_file()` existence read
    /// runs first, so the commonest rejection creates nothing.
    ///
    /// NOTE the gate order differs deliberately from `handle_self_handoff_switch`, which is
    /// launchability then existence. Neither handler is aligned to the other: aligning switch
    /// would be an unrequested behavior edit to a landed handler, and aligning restart would
    /// reinstate the defect this order removes. Consequence: for a request that is both missing
    /// its handoff file and unlaunchable, switch reports the launchability failure and restart
    /// reports the missing file.
    pub(crate) async fn handle_self_restart<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &std::path::Path,
        msg: &OutboxMessage,
        is_app_outbox: bool,
    ) -> Result<(), String> {
        // 1. Token parse.
        let token_uuid = match msg.token.as_deref().and_then(|t| Uuid::parse_str(t).ok()) {
            Some(u) => u,
            None => {
                return self
                    .reject_message(
                        path,
                        msg,
                        "self-handoff-and-restart requires a valid session token; restart or \
                        respawn the session",
                    )
                    .await;
            }
        };
        // 2. Sole authorization: the session that owns the presented token.
        let session = {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            mgr.find_by_token(token_uuid).await
        };
        let session = match session {
            Some(s) => s,
            None => {
                return self
                    .reject_message(
                        path,
                        msg,
                        "self-handoff-and-restart: no live session owns this token; restart or \
                        respawn the session",
                    )
                    .await;
            }
        };
        // 3. Session id.
        let session_id = match Uuid::parse_str(&session.id) {
            Ok(u) => u,
            Err(_) => {
                return self
                    .reject_message(
                        path,
                        msg,
                        "self-handoff-and-restart: internal error resolving session id",
                    )
                    .await;
            }
        };

        // 4. D1 identity gate. No coding-agent identity means no recipe to respawn, and passing
        //    None would hand the choice back to `resolve_restart_selected_agent_id`.
        let recipe = match resolve_self_restart_recipe(
            session.agent_id.as_deref(),
            session.effective_profile.as_deref(),
            session.requested_profile.as_deref(),
        ) {
            Ok(recipe) => recipe,
            Err(reason) => return self.reject_message(path, msg, &reason).await,
        };

        // 5. Existence gate on the session's OWN cwd. Restart runs no replica validation, so the
        //    session cwd is the only correct source for a Root Agent or an origin matrix agent.
        let handoff_path =
            std::path::Path::new(&session.working_directory).join(SELF_HANDOFF_ROOT_NAME);
        if !handoff_path.is_file() {
            return self
                .reject_message(
                    path,
                    msg,
                    &format!(
                        "self-handoff-and-restart: SELF-HANDOFF.md not found in your root ({}); \
                        write it before requesting self-handoff-and-restart.",
                        session.working_directory
                    ),
                )
                .await;
        }

        // 6. Launchability pre-flight on the pinned recipe, through the VERY function the restart
        //    transaction calls (`commands/session.rs`), so the pre-flight cannot disagree with the
        //    real spawn; a re-implementation could.
        let settings_snapshot = {
            let settings = app.state::<SettingsState>();
            let snapshot = settings.read().await.clone();
            snapshot
        };
        // #1632: mirror the restart transaction's Root substitution so the pre-flight cannot
        // resolve a different profile cell or placeholder root than the spawn it is predicting.
        // Read-only: `root_agent_dir()` returns exactly the string `ensure_root_agent_dir()`
        // returns, without provisioning anything. On Err, fall back to the raw cwd rather than
        // deny: `is_root_agent_path` itself returns false on that same error, so the only route
        // to the fallback is `session.is_root_agent` alone, and in that case the transaction's own
        // `ensure_root_agent_dir()?` fails pre-teardown on the identical root cause. Denying
        // instead would convert a recoverable pre-teardown failure into a synchronous exit 1 for
        // the Root Agent.
        let is_root_agent = session.is_root_agent
            || crate::config::root_agent::is_root_agent_path(&session.working_directory);
        let preflight_cwd = if is_root_agent {
            crate::config::root_agent::root_agent_dir()
                .unwrap_or_else(|_| session.working_directory.clone())
        } else {
            session.working_directory.clone()
        };
        match crate::commands::session::build_configured_agent_spawn_for_cwd(
            &settings_snapshot,
            &recipe.agent,
            &preflight_cwd,
            recipe.profile.as_deref(),
        ) {
            // `Ok(Some(_))` is launchable. `Ok(None)` means the pinned agent id is no longer in
            // `settings.agents`, so the respawn falls back to the session's stored shell and args,
            // byte-identically to what the UI Restart button does today for the same session:
            // proceed, do NOT reject.
            Ok(_) => {}
            // The pinned recipe does not build a launchable command: a profile letter that no
            // longer resolves, or a base command that no longer tokenizes. Reject synchronously so
            // the agent is told, rather than warning 30s later after the CLI already returned 0.
            Err(e) => {
                return self
                    .reject_message(
                        path,
                        msg,
                        &format!(
                            "self-handoff-and-restart: coding agent '{}' profile '{}' is not \
                            launchable from '{}': {}",
                            recipe.agent,
                            recipe.profile.as_deref().unwrap_or("<inherited>"),
                            preflight_cwd,
                            e,
                        ),
                    )
                    .await;
            }
        }

        // 7. Idempotency - atomic check-and-set. The set is SHARED with clear and switch, so the
        //    three operations cannot race on one session.
        let reservation = {
            let pending = app.state::<Arc<PendingSelfClear>>();
            pending.reserve_legacy(session_id)
        };
        let newly_inserted = match reservation {
            Ok(_) => true,
            Err(MaintenanceError::AlreadyLegacy) => false,
            Err(error) => {
                return self.reject_message(path, msg, &error.to_string()).await;
            }
        };
        let status = if newly_inserted {
            "queued"
        } else {
            "already_queued"
        };

        if newly_inserted {
            // 8. Archive SELF-FORGET.md so the next cycle starts fresh. Best-effort, log-only on
            //    failure, in ALL cfgs (as `handle_self_clear` does) so the harness can assert it.
            let root = std::path::Path::new(&session.working_directory);
            let forgotten_summary = capture_self_forget_summary(root);
            let ts = chrono::Local::now()
                .format(ARCHIVE_TIMESTAMP_FORMAT)
                .to_string();
            match archive_root_md(root, "SELF-FORGET", &ts) {
                Ok(Some(p)) => log::info!(
                    "[mailbox] self-handoff-and-restart: archived SELF-FORGET.md -> {}",
                    p.display()
                ),
                Ok(None) => {} // no SELF-FORGET.md; nothing to archive
                Err(e) => log::warn!(
                    "[mailbox] self-handoff-and-restart: SELF-FORGET.md archive failed for session \
                    {} (non-fatal): {}",
                    session_id,
                    e
                ),
            }

            // 9. Spawn the deferred two-phase sustained-idle gate (detached; never blocks the
            //    poller). Gated under cfg(not(test)) exactly like the other two handlers.
            #[cfg(not(test))]
            {
                let app_clone = app.clone();
                let restart_agent = recipe.agent.clone();
                let restart_profile = encode_restart_profile_for_seam(recipe.profile.as_deref());
                let root = root.to_path_buf();
                tauri::async_runtime::spawn(async move {
                    Self::run_self_switch_after_sustained_idle(
                        &app_clone,
                        session_id,
                        root,
                        restart_agent,
                        restart_profile,
                        forgotten_summary,
                        SELF_HANDOFF_FLAVOR_RESTART,
                        SELF_CLEAR_SETTLE,
                        SELF_CLEAR_POLL,
                        SELF_CLEAR_MAX_DEFER,
                    )
                    .await;
                });
            }
            #[cfg(test)]
            let _ = &forgotten_summary;
            log::info!(
                "[mailbox] self-handoff-and-restart queued for session {} coding agent '{}' \
                profile '{}' (from '{}')",
                session_id,
                recipe.agent,
                recipe.profile.as_deref().unwrap_or("<inherited>"),
                msg.from
            );
        } else {
            log::info!(
                "[mailbox] self-handoff-and-restart already pending for session {} (from '{}')",
                session_id,
                msg.from
            );
        }

        // 10.
        self.write_self_restart_response(app, path, msg, session_id, status, &recipe, is_app_outbox)
            .await
    }

    /// #626 - thin timer driver around `self_clear_gate_advance`. Fire-and-forget. Drives both
    /// phases on the stable `session_id`, injecting provider-resolved logical-clear text, then
    /// (#749) archiving `SELF-HANDOFF.md` and injecting the handoff prompt that names the archived
    /// path, and ALWAYS de-registers on exit. No "inject anyway" fallback - a busy or never-idle
    /// session is never cleared (the user-approved "30s sustained idle" semantic).
    #[allow(clippy::too_many_arguments)]
    #[cfg_attr(test, allow(dead_code))]
    pub(crate) async fn run_self_clear_after_sustained_idle<R: tauri::Runtime>(
        app: &tauri::AppHandle<R>,
        session_id: Uuid,
        clear_text: &'static str,
        root: PathBuf,
        forgotten_summary: Option<ForgottenSummary>,
        settle: std::time::Duration,
        poll: std::time::Duration,
        max_defer: std::time::Duration,
    ) {
        let pending = app.state::<Arc<PendingSelfClear>>().inner().clone();

        let app_for_state = app.clone();
        let session_state = move |session_id: Uuid| {
            let app = app_for_state.clone();
            async move {
                let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let mgr = session_mgr.read().await;
                let sessions = mgr.list_sessions().await;
                match sessions.iter().find(|s| s.id == session_id.to_string()) {
                    Some(s) => (true, s.waiting_for_input),
                    None => (false, false),
                }
            }
        };

        let app_for_inject = app.clone();
        let inject = move |session_id: Uuid, prompt: String| {
            use crate::pty::inject::inject_text_into_session;
            let app = app_for_inject.clone();
            async move { inject_text_into_session(&app, session_id, &prompt).await }
        };

        let app_for_boundary = app.clone();
        let note_boundary = move |session_id: Uuid, boundary: SelfClearBoundary| {
            let app = app_for_boundary.clone();
            async move {
                match boundary {
                    SelfClearBoundary::Cleared => {
                        crate::commands::pty::stamp_fresh_boundary_to_session(&app, session_id)
                            .await
                    }
                    SelfClearBoundary::ContentInjected => {
                        crate::commands::pty::note_post_boundary_content_to_session(
                            &app, session_id,
                        )
                        .await
                    }
                };
            }
        };

        consume_outcome(
            Self::drive_self_clear_after_sustained_idle(
                session_id,
                clear_text,
                root,
                pending,
                forgotten_summary,
                settle,
                poll,
                max_defer,
                session_state,
                inject,
                note_boundary,
            ),
            |failure| Self::report_self_handoff_failure(app, failure),
        )
        .await;
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn drive_self_clear_after_sustained_idle<
        SessionState,
        SessionFut,
        Inject,
        InjectFut,
        NoteBoundary,
        NoteFut,
    >(
        session_id: Uuid,
        clear_text: &'static str,
        root: PathBuf,
        pending: Arc<PendingSelfClear>,
        forgotten_summary: Option<ForgottenSummary>,
        settle: std::time::Duration,
        poll: std::time::Duration,
        max_defer: std::time::Duration,
        mut session_state: SessionState,
        mut inject: Inject,
        mut note_boundary: NoteBoundary,
    ) -> SelfHandoffOutcome
    where
        SessionState: FnMut(Uuid) -> SessionFut + Send + 'static,
        SessionFut: std::future::Future<Output = (bool, bool)> + Send,
        Inject: FnMut(Uuid, String) -> InjectFut + Send + 'static,
        InjectFut: std::future::Future<Output = Result<(), String>> + Send,
        NoteBoundary: FnMut(Uuid, SelfClearBoundary) -> NoteFut + Send + 'static,
        NoteFut: std::future::Future<Output = ()> + Send,
    {
        let Some(lease) = pending.legacy_lease(session_id) else {
            log::warn!(
                "[mailbox] clear driver missing legacy reservation for {}",
                session_id
            );
            return SelfHandoffOutcome::Failed(SelfHandoffFailure {
                action: SELF_CLEAR_ACTION,
                attempt: Uuid::new_v4(),
                original: session_id,
                current: Some(session_id),
                root,
                continuation: false,
                reason: format!("clear driver missing legacy reservation for {session_id}"),
                forgotten: forgotten_summary.is_some(),
            });
        };
        let attempt = Uuid::new_v4();
        let mut continuation = false;
        let mut state = SelfClearGateState::new(std::time::Instant::now());

        let reason = loop {
            tokio::time::sleep(poll).await;

            // Presence + waiting flag under a short-lived lock; never held across .await.
            let (present, waiting) = session_state(session_id).await;

            let (next, action) = self_clear_gate_advance(
                state,
                present,
                waiting,
                std::time::Instant::now(),
                settle,
                max_defer,
            );
            state = next; // adopt the advanced state (the Clear->Handoff reset is already applied)

            match action {
                SelfClearGateAction::Wait => continue,
                SelfClearGateAction::InjectClear => {
                    log::info!(
                        "[mailbox] self-handoff-and-clear: session {} idle >={}s; injecting {} \
                        (phase 1)",
                        session_id,
                        settle.as_secs(),
                        clear_text
                    );
                    // The idle-to-final-Enter race matches the remote logical-clear path.
                    if let Err(e) = inject(session_id, clear_text.to_string()).await {
                        log::warn!(
                            "[mailbox] self-handoff-and-clear: {} injection failed for session {}: \
                            {}",
                            clear_text,
                            session_id,
                            e
                        );
                        break Some(e); // clear may have been partially submitted
                    }
                    // Logical clear reached the PTY; stamp before phase 2
                    // can possibly drop (stamp-then-drop ordering).
                    note_boundary(session_id, SelfClearBoundary::Cleared).await;
                    continuation = true;
                    continue; // state is already Phase 2 with reset clocks
                }
                SelfClearGateAction::InjectHandoff => {
                    log::info!(
                        "[mailbox] self-handoff-and-clear: session {} idle >={}s post-clear; \
                        injecting handoff prompt (phase 2)",
                        session_id,
                        settle.as_secs()
                    );
                    // #749 - archive-first contract; see inject_handoff_prompt_with_archive.
                    let injected = inject_handoff_prompt_with_archive(
                        &root,
                        session_id,
                        SELF_CLEAR_ACTION,
                        build_self_clear_handoff_prompt,
                        forgotten_summary.as_ref(),
                        &mut inject,
                    )
                    .await;
                    if injected {
                        // (#756) handoff prompt is post-boundary content.
                        note_boundary(session_id, SelfClearBoundary::ContentInjected).await;
                    }
                    break (!injected).then(|| CONTINUATION_FAILURE.to_string());
                }
                SelfClearGateAction::Abandon(reason) => {
                    // Greppable abandon line so a silently-dropped clear/handoff is diagnosable.
                    // The CLI already warned the caller it is best-effort.
                    log::warn!(
                        "[mailbox] self-handoff-and-clear ABANDONED for session {}: {} (agent may \
                        re-issue)",
                        session_id,
                        reason
                    );
                    break Some(reason.to_string());
                }
            }
        };

        // Always de-register (handoff injected / destroy / cap-expiry / clear-inject-fail all land
        // here).
        pending.release(&lease);
        match reason {
            None => SelfHandoffOutcome::Completed,
            Some(reason) => SelfHandoffOutcome::Failed(SelfHandoffFailure {
                action: SELF_CLEAR_ACTION,
                attempt,
                original: session_id,
                current: Some(session_id),
                root,
                continuation,
                reason,
                forgotten: forgotten_summary.is_some(),
            }),
        }
    }

    #[allow(clippy::too_many_arguments)]
    #[cfg_attr(test, allow(dead_code))]
    async fn run_self_switch_after_sustained_idle<R: tauri::Runtime>(
        app: &tauri::AppHandle<R>,
        original_session_id: Uuid,
        cwd: PathBuf,
        target_agent: String,
        target_profile: String,
        forgotten_summary: Option<ForgottenSummary>,
        flavor: SelfHandoffFlavor,
        settle: std::time::Duration,
        poll: std::time::Duration,
        max_defer: std::time::Duration,
    ) {
        let pending = app.state::<Arc<PendingSelfClear>>().inner().clone();

        let app_for_state = app.clone();
        let session_state = move |session_id: Uuid| {
            let app = app_for_state.clone();
            async move {
                let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let mgr = session_mgr.read().await;
                let sessions = mgr.list_sessions().await;
                match sessions.iter().find(|s| s.id == session_id.to_string()) {
                    Some(s) => (true, s.waiting_for_input),
                    None => (false, false),
                }
            }
        };

        let app_for_persist = app.clone();
        let persist = move |cwd: PathBuf, agent: String, profile: String| {
            let app = app_for_persist.clone();
            async move {
                if flavor.action == SELF_HANDOFF_FLAVOR_RESTART.action {
                    return Ok(());
                }
                let settings_snapshot = {
                    let settings = app.state::<SettingsState>();
                    let snapshot = settings.read().await.clone();
                    snapshot
                };
                crate::config::coding_agent_profiles::set_replica_coding_agent_selection(
                    &settings_snapshot,
                    &cwd,
                    &agent,
                    &profile,
                )
            }
        };

        let app_for_restart = app.clone();
        let restart = move |session_id: Uuid, agent: String, profile: String| {
            let app = app_for_restart.clone();
            async move {
                let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();
                let settings = app.state::<SettingsState>();
                // (#747) The self-clear restart is agent-initiated, not user
                // input: a pending raised hand must survive it (the injected
                // handoff continues the same work). Capture before the
                // destroy; restore_communication re-gates kind, visibility,
                // and coordinator, so no pre-filter is needed here.
                let carried_communication = {
                    let mgr = session_mgr.read().await;
                    mgr.get_session(session_id)
                        .await
                        .and_then(|s| s.communication.clone())
                };
                let result = crate::commands::session::restart_session_inner_with_intent(
                    &app,
                    session_mgr.inner(),
                    pty_mgr.inner(),
                    settings.inner(),
                    session_id,
                    Some(agent),
                    if flavor.action == SELF_HANDOFF_FLAVOR_SWITCH.action {
                        Some(profile)
                    } else {
                        decode_restart_profile_from_seam(&profile)
                    },
                    Some(true),
                    true,
                    crate::session::selection::TrustedRestartIntent::Background,
                    carried_communication,
                    // §1295 6.6 / 5.1b: the mailbox wake-restart passes Enforce
                    // explicitly, unlike the default-preferring user restart.
                    crate::config::sessions_persistence::CreationGateEnforcement::Enforce,
                )
                .await;
                result.map(|info| info.id)
            }
        };

        let app_for_inject = app.clone();
        let inject = move |session_id: Uuid, prompt: String| {
            use crate::pty::inject::inject_text_into_session;
            let app = app_for_inject.clone();
            async move { inject_text_into_session(&app, session_id, &prompt).await }
        };

        let app_for_boundary = app.clone();
        let note_boundary = move |session_id: Uuid, boundary: SelfClearBoundary| {
            let app = app_for_boundary.clone();
            async move {
                match boundary {
                    SelfClearBoundary::Cleared => {
                        crate::commands::pty::stamp_fresh_boundary_to_session(&app, session_id)
                            .await
                    }
                    SelfClearBoundary::ContentInjected => {
                        crate::commands::pty::note_post_boundary_content_to_session(
                            &app, session_id,
                        )
                        .await
                    }
                };
            }
        };

        consume_outcome(
            Self::drive_self_switch_after_sustained_idle(
                original_session_id,
                cwd,
                target_agent,
                target_profile,
                forgotten_summary,
                flavor,
                pending,
                settle,
                poll,
                max_defer,
                session_state,
                persist,
                restart,
                inject,
                note_boundary,
            ),
            |failure| Self::report_self_handoff_failure(app, failure),
        )
        .await;
    }

    pub(crate) async fn report_self_handoff_failure<R: tauri::Runtime>(
        app: &tauri::AppHandle<R>,
        failure: SelfHandoffFailure,
    ) {
        use crate::session::warnings::{emit_session_warning, SessionWarning};
        use failure::{retry_notice, NoticeDelivery, ERR_SELF_HANDOFF_NOTICE_NOT_WAITING};
        const ERR_SELF_HANDOFF_NOTICE_PENDING: &str = "self-handoff-notice-pending";
        let observed = chrono::Utc::now().to_rfc3339();
        let message = failure.message(&observed);
        let diagnostic = failure.diagnostic_target();
        let warn = |suffix: &str, text: String| {
            emit_session_warning(
                app,
                SessionWarning::new(
                    diagnostic,
                    failure.key(suffix),
                    failure::WARNING_KIND_SELF_HANDOFF_FAILED,
                    text,
                ),
            );
        };
        warn("failure", message.clone());
        let Some(target) = failure.current else {
            warn(
                "delivery:unavailable",
                "The agent could not be notified: current UUID unknown".into(),
            );
            return;
        };
        let manager = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let pending = app.state::<Arc<crate::PendingSelfClear>>();
        let is_pending = || pending.contains(target);
        let body = format!("[AgentsCommander self-handoff failure] {message}");
        let status = retry_notice(
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(30),
            is_pending,
            || async {
                let session = { manager.read().await.get_session(target).await };
                let Some(session) = session else {
                    return Err(NoticeDelivery::Unavailable("session missing".into()));
                };
                if matches!(
                    session.status,
                    crate::session::session::SessionStatus::Exited(_)
                ) || session.is_root_agent
                    || session.agent_id.is_none()
                    || !crate::pty::inject::needs_explicit_enter(&session.shell)
                {
                    return Err(NoticeDelivery::Unavailable(
                        "session exited, root, agentless or unsupported shell".into(),
                    ));
                }
                if !session.waiting_for_input {
                    return Err(NoticeDelivery::Failed(
                        ERR_SELF_HANDOFF_NOTICE_NOT_WAITING.into(),
                    ));
                }
                #[cfg(test)]
                failure::signal_inject(target);
                crate::pty::inject::inject_internal_notice_into_supported_agent_session(
                    app,
                    target,
                    &body,
                    |snapshot| {
                        if is_pending() {
                            return Err(ERR_SELF_HANDOFF_NOTICE_PENDING.into());
                        }
                        if !snapshot.waiting_for_input {
                            return Err(ERR_SELF_HANDOFF_NOTICE_NOT_WAITING.into());
                        }
                        Ok(())
                    },
                )
                .await
                .map_err(NoticeDelivery::Failed)
            },
            |error| {
                error.starts_with(ERR_SELF_HANDOFF_NOTICE_NOT_WAITING)
                    || error.starts_with(crate::pty::menu_guard::ERR_MENU_GUARD_DEFERRED)
                    || error.starts_with(crate::pty::menu_guard::ERR_TYPING_HOLD_DEFERRED)
            },
            ERR_SELF_HANDOFF_NOTICE_PENDING,
        )
        .await;
        let (suffix, reason) = match status {
            NoticeDelivery::Delivered => {
                crate::commands::pty::note_post_boundary_content_to_session(app, target).await;
                app.state::<Arc<crate::pty::idle_detector::IdleDetector>>()
                    .touch_silence(target);
                return;
            }
            NoticeDelivery::Cancelled => (
                "cancelled",
                "notification cancelled: another self operation is pending".to_string(),
            ),
            NoticeDelivery::Unavailable(reason) => ("unavailable", reason),
            NoticeDelivery::Failed(reason) => (
                "failed",
                format!("{reason}; submission may be partial; agent receipt is not confirmed"),
            ),
            NoticeDelivery::Exhausted => (
                "exhausted",
                "no further attempt will start after the 30-second retry window".to_string(),
            ),
        };
        warn(
            &format!("delivery:{suffix}"),
            format!("The agent could not be notified: {reason}"),
        );
    }
}
