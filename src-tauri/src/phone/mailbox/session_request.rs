mod purge;

#[cfg(not(test))]
use super::handoff::SELF_CLEAR_MAX_DEFER;
#[cfg(not(test))]
use super::handoff::SELF_CLEAR_POLL;
#[cfg(not(test))]
use super::handoff::SELF_CLEAR_SETTLE;
use super::handoff::{ARCHIVE_TIMESTAMP_FORMAT, PURGE_WG_ACTION, SELF_HANDOFF_ROOT_NAME};
use super::notice::derive_project_from_outbox_path;
use super::poller::{evaluate_gate, MailboxPoller};
use super::settle::{archive_root_md, capture_self_forget_summary};
use crate::config::sessions_persistence::RaiseHandPersistOutcome;
use crate::config::settings::SettingsState;
use crate::filesystem::phone_mailbox as mailbox_disk;
use crate::phone::types::OutboxMessage;
use crate::pty::inject::LogicalPtyCommand;
use crate::session::maintenance::{MaintenanceError, PendingSelfClear};
use crate::session::manager::SessionManager;
use crate::session::session::SessionInfo;
use crate::session::session::SessionStatus;
use std::sync::Arc;
use tauri::Manager;
use uuid::Uuid;

impl MailboxPoller {
    /// (#885) Handle `purge-wg`: purge every peer in the caller's own workgroup.
    ///
    /// Sequence is non-reorderable. Steps 1-14 per plan §5.5c, consensus round 1.
    pub(crate) async fn handle_purge_wg<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &std::path::Path,
        msg: &OutboxMessage,
        is_app_outbox: bool,
    ) -> Result<(), String> {
        // 1. Identity gate — already enforced by the caller (§5.5b).
        debug_assert!(
            msg.token.is_some(),
            "purge-room requires a session token; caller must enforce"
        );

        // 2. Resolve WG scope and authorization, one call (§3.6).
        let effective_paths = {
            let mut paths = {
                let cfg = app.state::<SettingsState>();
                let c = cfg.read().await;
                c.project_paths.clone()
            };
            match derive_project_from_outbox_path(path) {
                Ok(Some(root_project)) => {
                    let canon = crate::filesystem::projects::canonical_refresh_project(
                        std::path::Path::new(&root_project),
                    )
                    .ok();
                    let already = paths.iter().any(|p| match &canon {
                        Some(ct) => {
                            crate::filesystem::projects::canonical_refresh_project(
                                std::path::Path::new(p),
                            )
                            .ok()
                            .as_ref()
                                == Some(ct)
                        }
                        None => p == &root_project,
                    });
                    if !already {
                        paths.push(root_project);
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    return self.reject_message(path, msg, &e).await;
                }
            }
            paths
        };
        let wg =
            match crate::config::teams::verified_wg_coordinator_target(&msg.from, &effective_paths)
            {
                Some(wg) => wg,
                None => {
                    return self
                        .reject_message(
                            path,
                            msg,
                            &format!(
                                "Not authorized: '{}' is not the verified orchestrator of its room",
                                msg.from
                            ),
                        )
                        .await;
                }
            };

        // 3. --wg assertion (§3.6: interlock, not selector).
        if let Some(ref t) = msg.target {
            if t != &wg.wg_name {
                return self
                    .reject_message(
                        path,
                        msg,
                        &format!(
                            "purge-room: --wg assertion '{}' does not match resolved room '{}'",
                            t, wg.wg_name
                        ),
                    )
                    .await;
            }
        }

        // 4. Enumerate peers (Guard A). Only `__agent_*` dirs under the WG dir.
        //    The coordinator does not purge itself.
        let wg_dir = match wg.replica_dir.parent() {
            Some(d) => d,
            None => {
                return self
                    .reject_message(path, msg, "purge-room: cannot resolve Room directory")
                    .await;
            }
        };
        let peer_fqns = purge::peer_fqns(wg_dir, &wg.agent_name, &wg.project, &wg.wg_name);

        // 5. Take the mirror snapshot ONCE (F-4). One list_sessions(), one
        //    PTY-liveness pass. Per-peer filtering reuses the pure predicate.
        let sessions: Vec<SessionInfo> = {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            mgr.list_sessions().await
        };
        let pty_live: std::collections::HashSet<Uuid> = {
            let pty = app.state::<Arc<std::sync::Mutex<crate::pty::manager::PtyManager>>>();
            let mgr = pty.lock().unwrap();
            sessions
                .iter()
                .filter_map(|s| Uuid::parse_str(&s.id).ok())
                .filter(|id| mgr.has_session(*id))
                .collect()
        };

        // 6. Restore-in-progress guard (F-2: exit 3, not 0).
        {
            let restore_flag = app.state::<Arc<crate::RestoreInProgress>>();
            if restore_flag.0.load(std::sync::atomic::Ordering::SeqCst) {
                let response = serde_json::json!({
                    "action": PURGE_WG_ACTION,
                    "workgroup": format!("{}:{}", wg.project, wg.wg_name),
                    "status": "restore_in_progress",
                    "requested_by": msg.from,
                    "peers": [],
                });
                return self
                    .write_purge_response_and_deliver(app, path, msg, is_app_outbox, &response)
                    .await;
            }
        }

        // 7. Liveness and Guard B (F-3: !Exited && has_pty; F-12: root guard).
        //    Collect live session info; the readiness snapshot (step 8) is
        //    correlated into gate_peers after.
        let (peer_infos, all_live_session_ids) = match purge::peer_live_info(
            &peer_fqns,
            &sessions,
            &pty_live,
            &wg.project,
            &wg.wg_name,
            msg,
        ) {
            Ok(snapshot) => snapshot,
            Err(response) => {
                return self
                    .write_purge_response_and_deliver(app, path, msg, is_app_outbox, &response)
                    .await
            }
        };

        // 8. Readiness snapshot and the four-leg gate (§2.3.3, F-1).
        let quiet = std::time::Duration::from_millis(
            msg.quiet_period_ms.unwrap_or(3000).max(
                crate::session::profile::IdleTuning::DEFAULT
                    .idle_threshold
                    .as_millis() as u64,
            ),
        );
        let readiness_map: std::collections::HashMap<
            Uuid,
            crate::pty::idle_detector::PurgeReadiness,
        > = {
            let idle = app.state::<Arc<crate::pty::idle_detector::IdleDetector>>();
            let readiness = idle.purge_readiness(&all_live_session_ids);
            readiness.into_iter().map(|r| (r.session_id, r)).collect()
        };
        let gate_peers = purge::gate_peers(&peer_infos, &readiness_map);

        let decision = evaluate_gate(&gate_peers, quiet);

        // 9. Dry-run exit, BEFORE the gate rejection (F-9).
        if msg.dry_run == Some(true) {
            let response = purge::dry_run_response(&wg.project, &wg.wg_name, msg, quiet, &decision);
            return self
                .write_purge_response_and_deliver(app, path, msg, is_app_outbox, &response)
                .await;
        }

        // 10. The gate. If any peer is not purgeable: reject, destroy nothing.
        if !decision.passed {
            let response = purge::busy_response(&wg.project, &wg.wg_name, msg, quiet, &decision);
            return self
                .write_purge_response_and_deliver(app, path, msg, is_app_outbox, &response)
                .await;
        }

        // 11. Acquire the lease (F-14: clone Arc into a named local first).
        let mut target_sids: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
        let mut target_fqns: std::collections::HashSet<String> = std::collections::HashSet::new();
        for peer in &gate_peers {
            for session in &peer.live {
                target_sids.insert(session.session_id);
            }
            target_fqns.insert(peer.fqn.clone());
        }
        // Also include non-live session IDs (Exited records) for destruction.
        for peer in &gate_peers {
            for sid_str in &peer.all_session_ids {
                if let Ok(sid) = Uuid::parse_str(sid_str) {
                    target_sids.insert(sid);
                }
            }
        }
        let purge_guard: Arc<crate::session::purge_guard::PurgeGuard> = app
            .state::<Arc<crate::session::purge_guard::PurgeGuard>>()
            .inner()
            .clone();
        let force = msg.force.unwrap_or(true);
        let timeout_secs = msg.timeout_secs.unwrap_or(5);
        let budget = super::teardown::observation_budget(force, timeout_secs)
            .ok_or_else(|| "purge observation budget overflow".to_string())?;
        let mut ids = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for peer in &gate_peers {
            for id in &peer.all_session_ids {
                if let Ok(sid) = Uuid::parse_str(id) {
                    if seen.insert(sid) {
                        ids.push(sid);
                    }
                }
            }
        }
        let progress = Arc::new(super::teardown::PurgeProgress::new(ids.clone()));
        let targets = super::teardown::PurgeTargets {
            ids,
            session_ids: target_sids,
            fqns: target_fqns,
        };
        let owned_app = app.clone();
        let poller = self.delivery_poller();
        let worker = tokio::spawn(super::teardown::run_purge_batch(
            purge_guard,
            targets,
            progress.clone(),
            budget,
            move |sid| {
                let app = owned_app.clone();
                let poller = poller.delivery_poller();
                async move {
                    poller
                        .close_session_attempt(
                            &app,
                            sid,
                            force,
                            timeout_secs,
                            Arc::new(std::sync::atomic::AtomicBool::new(false)),
                        )
                        .await
                }
            },
        ));
        let snapshot = super::teardown::observe_purge(progress, worker).await?;
        let response =
            purge::completion_response(&wg.project, &wg.wg_name, msg, quiet, &decision, &snapshot);

        self.write_purge_response_and_deliver(app, path, msg, is_app_outbox, &response)
            .await
    }

    /// Returns fast: the 30s sustained-idle wait runs in a detached task so the
    /// poll loop is never blocked. Idempotent: a second request while one is
    /// pending is a no-op ("already_queued").
    pub(crate) async fn handle_self_clear<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &std::path::Path,
        msg: &OutboxMessage,
        is_app_outbox: bool,
    ) -> Result<(), String> {
        // 1. Resolve the caller's OWN session from the token (the sole authority).
        let token_uuid = match msg.token.as_deref().and_then(|t| Uuid::parse_str(t).ok()) {
            Some(u) => u,
            None => {
                return self
                    .reject_message(
                        path,
                        msg,
                        "self-clear requires a valid session token; restart or respawn the session",
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
                        "self-clear: no live session owns this token; restart or respawn the \
                        session",
                    )
                    .await;
            }
        };
        let session_id = match Uuid::parse_str(&session.id) {
            Ok(u) => u,
            Err(_) => {
                return self
                    .reject_message(path, msg, "self-clear: internal error resolving session id")
                    .await;
            }
        };

        // Resolve before the handoff existence gate so an unsupported source
        // cannot archive or queue self-maintenance state.
        let clear_text = match crate::pty::inject::resolve_logical_command_text(
            &session.shell,
            LogicalPtyCommand::Clear,
        ) {
            Some(text) => text,
            None => {
                return self
                    .reject_message(
                        path,
                        msg,
                        &format!(
                            "self-handoff-and-clear: session shell '{}' has no verified \
                            logical-clear mapping. Claude / Codex / Antigravity / Cursor agent \
                            direct shells use /clear; exact Pi uses /new. cmd / pwsh outer \
                            wrappers remain unsupported.",
                            session.shell
                        ),
                    )
                    .await;
            }
        };

        let pending = app.state::<Arc<PendingSelfClear>>().inner().clone();
        if msg.action.as_deref() == Some(super::PLAIN_SELF_CLEAR_ACTION) {
            #[cfg(not(test))]
            let inject = {
                let app = app.clone();
                move |sid, text: String| async move {
                    crate::pty::inject::inject_text_into_session(&app, sid, &text).await
                }
            };
            #[cfg(test)]
            let inject = |_sid, _text: String| async {
                Err("self-clear synthetic test injector: no PTY input".to_string())
            };
            let boundary_app = app.clone();
            let note_boundary = move |sid| async move {
                crate::commands::pty::stamp_fresh_boundary_to_session(&boundary_app, sid).await;
            };
            let transport = || self.move_to_delivered(app, path, msg);
            let launch = Self::launch_self_clear_driver::<super::self_clear::Driver>;
            return self
                .handle_plain_self_clear(
                    path,
                    msg,
                    session,
                    clear_text,
                    pending,
                    (launch, transport, inject, note_boundary),
                )
                .await;
        }

        // 2b. #626 existence gate - REFUSE if the agent did not write its handoff notes. Clearing
        // with
        // no SELF-HANDOFF.md would wipe context with no way to resume (the agent would post-clear
        // read a nonexistent file = blank). Queue-time intent guard (not transactional): the real
        //     read-time safety net is the handoff prompt's "if missing or empty, wait" clause
        //     (self_clear_handoff_base_prompt). Use .is_file() so a stray directory named
        //     SELF-HANDOFF.md does not pass. Runs BEFORE the idempotency insert and the archive, so
        //     nothing is queued/archived with nothing to resume.
        let handoff_path =
            std::path::Path::new(&session.working_directory).join(SELF_HANDOFF_ROOT_NAME);
        if !mailbox_disk::mailbox_path_is_file(&handoff_path) {
            return self
                .reject_message(
                    path,
                    msg,
                    &format!(
                        "self-handoff-and-clear: SELF-HANDOFF.md not found in your root ({}); \
                         write it before requesting self-handoff-and-clear.",
                        session.working_directory
                    ),
                )
                .await;
        }

        // 3. Idempotency - atomic check-and-set. insert() returns false if already present.
        let reservation = pending.reserve_legacy(session_id);
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

        // 4. Spawn the deferred two-phase sustained-idle gate (detached; never blocks the poller).
        //    MED-4: gated under #[cfg(not(test))] so handle_self_clear unit tests assert
        //    queue + idempotency WITHOUT launching a live 500ms poll task. The gate's
        //    decision policy is covered separately by the pure `self_clear_gate_advance` tests.
        //    Durations are passed in so a future integration test can drive it fast.
        if newly_inserted {
            // #626 - archive SELF-FORGET.md so the next cycle starts fresh. Best-effort: a failure
            // must not block the clear/handoff. The agent already wrote SELF-HANDOFF.md
            // (existence-gated above), so SELF-FORGET.md is present iff the agent kept one. FOLD-3:
            // runs in ALL cfgs (NOT inside the cfg(not(test)) spawn) so the harness archive
            // assertion can pass. Decoupled from clear success (queue-time): an abandoned cycle
            // leaves SELF-FORGET archived but uncleared; content is preserved in
            // self-clear/<ts>_SELF-FORGET.md, re-issue continues normally.
            let root = std::path::Path::new(&session.working_directory);
            let forgotten_summary = capture_self_forget_summary(root);
            let ts = chrono::Local::now()
                .format(ARCHIVE_TIMESTAMP_FORMAT)
                .to_string();
            match archive_root_md(root, "SELF-FORGET", &ts) {
                Ok(Some(p)) => log::info!(
                    "[mailbox] self-handoff-and-clear: archived SELF-FORGET.md -> {}",
                    p.display()
                ),
                Ok(None) => {} // no SELF-FORGET.md; nothing to archive
                Err(e) => log::warn!(
                    "[mailbox] self-handoff-and-clear: SELF-FORGET.md archive failed for session \
                        {} (non-fatal): {}",
                    session_id,
                    e
                ),
            }

            #[cfg(not(test))]
            {
                let app_clone = app.clone();
                // #749 - the root travels with the task (same queue-time snapshot the
                // SELF-FORGET archive above used) so Phase 2 can archive the handoff
                // without a mid-flight session-manager lookup. A session's working
                // directory never changes while its id stays alive.
                let root = root.to_path_buf();
                Self::launch_self_clear_driver(async move {
                    Self::run_self_clear_after_sustained_idle(
                        &app_clone,
                        session_id,
                        clear_text,
                        root,
                        forgotten_summary,
                        SELF_CLEAR_SETTLE,
                        SELF_CLEAR_POLL,
                        SELF_CLEAR_MAX_DEFER,
                    )
                    .await;
                });
            }
            #[cfg(test)]
            let _ = (clear_text, &forgotten_summary);
            log::info!(
                "[mailbox] self-handoff-and-clear queued for session {} (from '{}')",
                session_id,
                msg.from
            );
            #[cfg(test)]
            let _ = app;
        } else {
            log::info!(
                "[mailbox] self-handoff-and-clear already pending for session {} (from '{}')",
                session_id,
                msg.from
            );
        }

        // 5. Write the queue-ack response, then move the message to delivered/.
        self.write_self_clear_response(app, path, msg, session_id, status, is_app_outbox)
            .await
    }

    fn session_has_visible_raise_hand_slot(session: &SessionInfo) -> bool {
        if !session.is_coordinator || matches!(&session.status, SessionStatus::Exited(_)) {
            return false;
        }
        let Some(task_path) =
            crate::session::session::find_workgroup_task_path_for_cwd(&session.working_directory)
        else {
            return false;
        };
        let Ok(content) = mailbox_disk::read_mailbox_task_text(&task_path) else {
            return false;
        };
        crate::commands::entity_creation::parse_task_title(&content).is_some()
    }

    pub(crate) async fn handle_raise_hand<R: tauri::Runtime>(
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
                        "raise-hand requires a valid session token; restart or respawn the session",
                    )
                    .await;
            }
        };
        let mgr = {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let guard = session_mgr.read().await;
            guard.clone()
        };
        let Some(session) = mgr.find_by_token(token_uuid).await else {
            return self
                .reject_message(
                    path,
                    msg,
                    "raise-hand: no live session owns this token; restart or respawn the session",
                )
                .await;
        };
        let session_id = match Uuid::parse_str(&session.id) {
            Ok(u) => u,
            Err(_) => {
                return self
                    .reject_message(path, msg, "raise-hand: internal error resolving session id")
                    .await;
            }
        };

        if !Self::session_has_visible_raise_hand_slot(&session) {
            return self
                .write_raise_hand_response(
                    app,
                    path,
                    msg,
                    session_id,
                    false,
                    "not_visible",
                    is_app_outbox,
                )
                .await;
        }

        // #698 - raise the hand and persist its snapshot atomically with respect
        // to all session persistence (see `raise_hand_and_persist_result`). The
        // mutation, snapshot, and save happen under one global save lock, so no
        // concurrent persist can durably write the raised state before this
        // snapshot lands. The snapshot is durable before we emit the UI event or
        // write the success response, so a peer reading `list-sessions` sees
        // `raisedHand: true` only after it survived. On persist failure the helper
        // has already rolled back the live communication under the same lock, so
        // we just reject the message rather than reporting a success that did not
        // survive.
        let outcome = crate::config::sessions_persistence::raise_hand_and_persist_result(
            &mgr,
            session_id,
            chrono::Utc::now(),
        )
        .await;
        let (raised, status, changed_communication) = match outcome {
            Ok(RaiseHandPersistOutcome::Raised(communication)) => {
                (true, "raised", Some(communication))
            }
            Ok(RaiseHandPersistOutcome::AlreadyVisible) => (true, "already_visible", None),
            Ok(RaiseHandPersistOutcome::NotRaisable) => (false, "not_visible", None),
            Err(e) => {
                return self
                    .reject_message(
                        path,
                        msg,
                        &format!("raise-hand: failed to persist state: {}", e),
                    )
                    .await;
            }
        };

        if let Some(communication) = changed_communication {
            crate::session::selection::publish_session_communication(
                app,
                session_id,
                Some(&communication),
            );
        }

        self.write_raise_hand_response(app, path, msg, session_id, raised, status, is_app_outbox)
            .await
    }
}

// A stage is owned only after create_new succeeds. Cleanup never touches the destination.
struct PlainReceiptStage {
    path: std::path::PathBuf,
    written: Vec<u8>,
}

impl Drop for PlainReceiptStage {
    fn drop(&mut self) {
        match mailbox_disk::read_mailbox_message(&self.path) {
            Ok(bytes) if bytes == self.written => {
                if let Err(error) = mailbox_disk::remove_mailbox_artifact(&self.path) {
                    log::warn!("[mailbox] self-clear stage cleanup failed: {}", error);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            _ => log::warn!("[mailbox] self-clear stage changed; preserving it"),
        }
    }
}

impl MailboxPoller {
    fn launch_self_clear_driver<F>(driver: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        tauri::async_runtime::spawn(driver);
    }

    async fn handle_plain_self_clear<L, T, TF, I, IF, B, BF>(
        &self,
        path: &std::path::Path,
        msg: &OutboxMessage,
        session: SessionInfo,
        clear_text: &'static str,
        pending: Arc<PendingSelfClear>,
        callbacks: (L, T, I, B),
    ) -> Result<(), String>
    where
        L: FnOnce(super::self_clear::Driver),
        T: FnOnce() -> TF,
        TF: std::future::Future<Output = Result<(), String>>,
        I: FnOnce(Uuid, String) -> IF + Send + 'static,
        IF: std::future::Future<Output = Result<(), String>> + Send + 'static,
        B: FnOnce(Uuid) -> BF + Send + 'static,
        BF: std::future::Future<Output = ()> + Send + 'static,
    {
        use super::self_clear::{accept, Callbacks, PlainError, Prepared, Request};
        use crate::session::maintenance::PlainIdentity;
        let (launch, transport, inject, note_boundary) = callbacks;
        let request_id = match msg
            .request_id
            .as_deref()
            .and_then(|s| Uuid::parse_str(s).ok())
        {
            Some(id) => id,
            None => {
                return self
                    .reject_message(path, msg, "self-clear requires request_id UUID")
                    .await
            }
        };
        let identity = PlainIdentity {
            session_id: Uuid::parse_str(&session.id).map_err(|e| e.to_string())?,
            request_id,
            action: super::PLAIN_SELF_CLEAR_ACTION.into(),
            requested_by: msg.from.clone(),
        };
        let canonical = super::notice::sender_name_for_session_cwd_with_root_flag(
            &session.working_directory,
            session.is_root_agent
                || crate::config::root_agent::is_root_agent_path(&session.working_directory),
        );
        if let Err(error) = super::self_clear::validate_caller(
            &identity,
            &canonical,
            &msg.to,
            msg.target.as_deref(),
        ) {
            return self.reject_message(path, msg, &error).await;
        }
        let request = Request {
            identity,
            root: session.working_directory.into(),
            local_dir: crate::config::agent_local_dir_name(),
            clear_text,
        };
        let result = accept(
            request,
            pending,
            Callbacks {
                read_receipt: |destination: &std::path::Path| {
                    match mailbox_disk::read_mailbox_message(destination) {
                        Ok(bytes) => Ok(Some(bytes)),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                        Err(error) => Err(error.to_string()),
                    }
                },
                prepare_receipt: |destination: &std::path::Path, bytes: &[u8], _lease: &_| {
                    let parent = destination
                        .parent()
                        .ok_or("self-clear: no responses directory")?;
                    mailbox_disk::ensure_mailbox_archive_directory(parent)
                        .map_err(|e| e.to_string())?;
                    let stage_path = parent.join(format!("{}.stage", Uuid::new_v4()));
                    let mut file = mailbox_disk::create_plain_mailbox_receipt_stage(&stage_path)
                        .map_err(|e| e.to_string())?;
                    let mut stage = PlainReceiptStage {
                        path: stage_path,
                        written: Vec::new(),
                    };
                    while stage.written.len() < bytes.len() {
                        let remaining = &bytes[stage.written.len()..];
                        let count =
                            mailbox_disk::write_plain_mailbox_receipt_chunk(&mut file, remaining)
                                .map_err(|e| e.to_string())?;
                        if count == 0 {
                            return Err("self-clear: stage write made no progress".into());
                        }
                        stage.written.extend_from_slice(&remaining[..count]);
                    }
                    drop(file);
                    let bytes = mailbox_disk::read_mailbox_message(&stage.path)
                        .map_err(|e| e.to_string())?;
                    Ok(Prepared { stage, bytes })
                },
                publish_prepared: |stage: &PlainReceiptStage, destination: &std::path::Path| {
                    crate::path_identity::publish_new_file_atomic(&stage.path, destination)
                },
                launch,
                transport,
                inject,
                note_boundary,
            },
        )
        .await;
        match result {
            Ok(()) => Ok(()),
            Err(PlainError::Rejected(error)) => self.reject_message(path, msg, &error).await,
            Err(PlainError::Retryable(error) | PlainError::Transport(error)) => Err(error),
        }
    }
}
