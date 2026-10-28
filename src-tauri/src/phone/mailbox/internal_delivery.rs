use super::handoff::internal_wake_resumes;
use super::notice::{
    canonical_cwd_owned_by_replica, format_wake_content, internal_system_envelope,
    resolve_wake_agent_command_from_sources, InternalNoticeGuard, InternalSystemNotice,
    InternalSystemTarget, ResolvedWakeAgentCommand, WakeContent, WakeDeliveryOrigin,
};
use super::poller::MailboxPoller;
use super::pty_claim::ResponseWakeWork;
#[cfg(test)]
use super::pty_claim::{InternalSystemBookkeeping, MailboxTestEvent};
use super::settle::{err_is_pty_session_missing, internal_target_fqn_prefilter_matches};
use crate::config::settings::SettingsState;
use crate::phone::consumption::verdict_to_result;
use crate::phone::consumption::ConsumptionVerdict;
use crate::phone::types::OutboxMessage;
use crate::pty::inject::SettleReadiness;
use crate::session::manager::SessionManager;
use crate::session::session::Session;
use crate::session::session::SessionInfo;
use crate::session::session::SessionStatus;
use std::sync::Arc;
use tauri::Manager;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

impl MailboxPoller {
    pub(crate) async fn deliver_internal_system_wake<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        target: InternalSystemTarget,
        notice: InternalSystemNotice,
        cancellation: CancellationToken,
        guard: InternalNoticeGuard,
    ) -> Result<(), String> {
        if cancellation.is_cancelled() {
            return Err("Context alert delivery was canceled".to_string());
        }
        if let Some(purge) = app.try_state::<Arc<crate::session::purge_guard::PurgeGuard>>() {
            if purge.blocks_agent(target.fqn()) {
                return Err(format!(
                    "purge-room in progress for '{}'; context alert deferred",
                    target.fqn()
                ));
            }
        }
        Self::run_internal_guard(Arc::clone(&guard)).await?;

        let envelope = internal_system_envelope(&target);
        let mut exited: Option<SessionInfo> = None;
        for (candidate, has_pty) in self.find_internal_system_candidates(app, &target).await? {
            match candidate.status {
                SessionStatus::Exited(_) => {
                    if exited.is_none() {
                        exited = Some(candidate);
                    }
                }
                _ if has_pty => {
                    let session_id = Uuid::parse_str(&candidate.id).map_err(|e| {
                        format!("Invalid orchestrator session id '{}': {}", candidate.id, e)
                    })?;
                    let blocked = tokio::select! {
                        biased;
                        _ = cancellation.cancelled() => {
                            return Err(
                                "Context alert delivery was canceled before live injection"
                                    .to_string(),
                            );
                        }
                        reason = crate::pty::inject::internal_notice_block_reason(
                            app,
                            session_id,
                        ) => reason,
                    };
                    if cancellation.is_cancelled() {
                        return Err(
                            "Context alert delivery was canceled before live injection".to_string()
                        );
                    }
                    if let Some(reason) = blocked {
                        return Err(reason);
                    }
                    tokio::select! {
                        biased;
                        _ = cancellation.cancelled() => {
                            return Err(
                                "Context alert delivery was canceled during live settle"
                                    .to_string(),
                            );
                        }
                        _ = self.settle_internal_live_before_inject(app, session_id) => {}
                    }
                    if cancellation.is_cancelled() {
                        return Err(
                            "Context alert delivery was canceled before live injection".to_string()
                        );
                    }
                    match self
                        .inject_internal_system_notice(
                            app,
                            session_id,
                            &target,
                            &notice,
                            Arc::clone(&guard),
                        )
                        .await
                    {
                        Ok(()) => return Ok(()),
                        Err(error) if err_is_pty_session_missing(&error) => {
                            log::warn!(
                                "[context-alert] coordinator session {} vanished before injection: \
                                {}",
                                session_id,
                                error
                            );
                        }
                        Err(error) => return Err(error),
                    }
                }
                _ => {}
            }
        }

        let mut spawn_with_resume = false;
        let mut carried_bot: Option<String> = None;
        let mut carried_communication = None;
        if let Some(exited_candidate) = exited {
            if cancellation.is_cancelled() {
                return Err(
                    "Context alert delivery was canceled before exited-session actuation"
                        .to_string(),
                );
            }
            let exited_id = Uuid::parse_str(&exited_candidate.id)
                .map_err(|e| format!("Invalid exited orchestrator session id: {}", e))?;
            self.recheck_internal_exited_candidate(app, exited_id, &target)
                .await?;
            Self::run_internal_guard(Arc::clone(&guard)).await?;
            self.recheck_internal_exited_record(
                app,
                exited_id,
                &target,
                &exited_candidate.working_directory,
            )
            .await?;
            carried_bot = exited_candidate.telegram_bot_id.clone();
            carried_communication = exited_candidate.communication.clone();
            self.destroy_exited_wake_session(app, exited_id)
                .await
                .map_err(|error| {
                    format!(
                        "Failed to destroy stale orchestrator session {}: {}",
                        exited_id, error
                    )
                })?;
            spawn_with_resume = true;
        }

        // Final pre-spawn authorization followed by a fresh exact-path enumeration. A
        // supported recipient that appeared wins; this attempt returns stale rather than
        // creating a duplicate.
        Self::run_internal_guard(Arc::clone(&guard)).await?;
        if !self
            .find_internal_system_candidates(app, &target)
            .await?
            .is_empty()
        {
            return Err(format!(
                "Orchestrator recipient '{}' changed immediately before background spawn",
                target.fqn()
            ));
        }

        let resolved_command = self
            .resolve_internal_agent_command(app, &target)
            .await?
            .ok_or_else(|| {
                format!(
                    "No supported coding-agent command is configured for '{}'",
                    target.fqn()
                )
            })?;
        let agent_id = resolved_command.agent_id.as_deref().ok_or_else(|| {
            format!(
                "Resolved command for '{}' has no trusted coding-agent id",
                target.fqn()
            )
        })?;
        let cwd = target.replica_dir().to_string_lossy().to_string();
        let settings_snapshot = {
            let settings = app.state::<SettingsState>();
            let snapshot = settings.read().await.clone();
            snapshot
        };
        // #1271 - copy the configured default host shell from the SAME snapshot
        // that builds the spawn below, before the snapshot moves into the
        // resolution task (Phase 1 items 1-2: never read one half of the pair
        // after configuration has changed).
        let resolved_agent_host_shell = Some(crate::pty::backend::ResolvedAgentHostShell {
            program: settings_snapshot.default_shell.clone(),
            args: settings_snapshot.default_shell_args.clone(),
        });
        let spawn_agent_id = agent_id.to_string();
        let spawn_cwd = cwd.clone();
        let resolved_spawn = tokio::task::spawn_blocking(move || {
            crate::commands::session::build_configured_agent_spawn_for_cwd(
                &settings_snapshot,
                &spawn_agent_id,
                &spawn_cwd,
                None,
            )
        })
        .await
        .map_err(|error| format!("Internal profile resolution task failed: {}", error))??
        .ok_or_else(|| {
            format!(
                "Configured coding-agent '{}' could not produce a trusted spawn for '{}'",
                agent_id,
                target.fqn()
            )
        })?;
        if resolved_spawn.trusted_agent_id.trim().is_empty()
            || resolved_spawn.trusted_agent_id != agent_id
        {
            return Err(format!(
                "Resolved spawn for '{}' lost its trusted coding-agent identity",
                target.fqn()
            ));
        }
        if !crate::pty::inject::needs_explicit_enter(&resolved_spawn.shell) {
            return Err(format!(
                "Resolved spawn shell '{}' for '{}' is not a supported coding-agent CLI",
                resolved_spawn.shell,
                target.fqn()
            ));
        }

        // Repeat the guard and exact-path absence check immediately before actuation. Command
        // and profile resolution above can perform filesystem work and must not widen this race.
        Self::run_internal_guard(Arc::clone(&guard)).await?;
        if !self
            .find_internal_system_candidates(app, &target)
            .await?
            .is_empty()
        {
            return Err(format!(
                "Orchestrator recipient '{}' changed immediately before background spawn",
                target.fqn()
            ));
        }

        if cancellation.is_cancelled() {
            return Err("Context alert delivery was canceled before background spawn".to_string());
        }
        // #2411 An idle auto-close destroys the record, so a no-session wake of an
        // auto-closed orchestrator resumes its conversation. Same key as create and
        // auto-close. The std mutex is never held across an await.
        let marker_fqn = crate::config::teams::agent_fqn_from_path(&cwd);
        let auto_closed_at = app
            .try_state::<crate::config::coordinator_clocks::CoordinatorClocksState>()
            .and_then(|clocks| {
                clocks
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .auto_closed_at(&marker_fqn)
            });
        let spawn_with_resume = internal_wake_resumes(spawn_with_resume, auto_closed_at.is_some());
        log::info!(
            "[internal-wake] '{}' no-live-session spawn resume={} auto_closed_at={:?}",
            target.fqn(),
            spawn_with_resume,
            auto_closed_at
        );
        let (_, local) = crate::config::teams::split_project_prefix(target.fqn());
        let spawn = self.spawn_wake_session(
            app,
            &envelope,
            &resolved_command,
            cwd,
            local.to_string(),
            spawn_with_resume,
            // Internal-system envelopes never carry dispatch flags (D5).
            false,
            resolved_spawn.shell.clone(),
            resolved_spawn.shell_args.clone(),
            Some(resolved_spawn.trusted_agent_label.clone()),
            Some(resolved_spawn),
            resolved_agent_host_shell,
        );
        tokio::pin!(spawn);
        let spawn_result = tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                return Err(
                    "Context alert delivery was canceled during background spawn".to_string(),
                );
            }
            result = &mut spawn => result,
        };
        let info = match spawn_result {
            Ok(info) => info,
            Err(error) => {
                // #2413 A failed create restores its cleared close markers itself
                // (commands::session::CoordinatorCreateTicket), for every caller.
                return Err(format!(
                    "Failed to spawn supported orchestrator session for '{}': {}",
                    target.fqn(),
                    error
                ));
            }
        };
        let session_id = Uuid::parse_str(&info.id)
            .map_err(|e| format!("Invalid spawned orchestrator session id: {}", e))?;

        if carried_bot.is_some() {
            self.attach_persisted_telegram_for_wake(app, session_id, carried_bot.as_deref())
                .await;
        }
        if let Some(communication) = carried_communication {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let manager = session_mgr.read().await.clone();
            let restored = manager
                .restore_communication(session_id, communication.clone())
                .await;
            if restored {
                crate::session::selection::publish_session_communication(
                    app,
                    session_id,
                    Some(&communication),
                );
            }
        }

        tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                return Err(
                    "Context alert delivery was canceled during spawned-session settle".to_string(),
                );
            }
            result = self.wait_for_spawned_wake_idle(app, session_id) => {
                result?;
            }
        }
        if cancellation.is_cancelled() {
            return Err(
                "Context alert delivery was canceled before spawned-session injection".to_string(),
            );
        }
        self.inject_internal_system_notice(app, session_id, &target, &notice, guard)
            .await
    }

    async fn settle_internal_live_before_inject<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        session_id: Uuid,
    ) {
        #[cfg(test)]
        if let Some(hooks) = &self.test_hooks {
            let gate = hooks.internal_live_settle_gate.lock().unwrap().take();
            if let Some(gate) = gate {
                // notify_one, not notify_waiters: this stores a permit when the canceller
                // has not been polled yet, which on a current_thread runtime is the norm.
                hooks.internal_live_settle_entered.notify_one();
                let _ = gate.await;
                return;
            }
        }
        // #2586 - the internal-notice injector is a non-wake path that reports
        // `Unknown`, so this readiness is not carried.
        let _ = self.settle_live_before_inject(app, session_id).await;
    }

    async fn run_internal_guard(guard: InternalNoticeGuard) -> Result<(), String> {
        tokio::task::spawn_blocking(move || guard())
            .await
            .map_err(|error| format!("Internal notice guard task failed: {}", error))?
    }

    async fn resolve_internal_agent_command<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        target: &InternalSystemTarget,
    ) -> Result<Option<ResolvedWakeAgentCommand>, String> {
        let agents = {
            let settings = app.state::<SettingsState>();
            let settings = settings.read().await;
            // #2716 (B3, option C): the same refusal as `resolve_agent_command`.
            if let Some(path) = settings.agents_layer.unreadable_path() {
                return Err(crate::config::settings::agents_unreadable_session_error(
                    path,
                ));
            }
            settings.agents.clone()
        };
        let replica_dir = target.replica_dir().to_path_buf();
        tokio::task::spawn_blocking(move || {
            let current = crate::config::coding_agent_profiles::read_replica_current_coding_agent(
                &replica_dir,
            );
            let config_dir = replica_dir.join(crate::config::agent_local_dir_name());
            let config_path = config_dir.join("config.json");
            let last = crate::config::agent_config::read_agent_local_config(&config_dir)
                .and_then(|config| config.tooling.last_coding_agent);
            resolve_wake_agent_command_from_sources(
                &agents,
                "auto",
                current.as_deref(),
                last.as_deref(),
                Some(&config_path),
                None,
            )
        })
        .await
        .map_err(|error| format!("Internal command lookup task failed: {}", error))?
    }

    async fn find_internal_system_candidates<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        target: &InternalSystemTarget,
    ) -> Result<Vec<(SessionInfo, bool)>, String> {
        let sessions = {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let manager = session_mgr.read().await.clone();
            manager.list_sessions().await
        };
        let target_fqn = target.fqn().to_string();
        let replica_dir = target.replica_dir().to_path_buf();
        let structural: Vec<SessionInfo> = sessions
            .into_iter()
            .filter(|session| {
                !session.is_root_agent
                    && session.agent_id.is_some()
                    && crate::pty::inject::needs_explicit_enter(&session.shell)
                    && internal_target_fqn_prefilter_matches(
                        &session.working_directory,
                        &target_fqn,
                    )
            })
            .collect();
        let mut owned = tokio::task::spawn_blocking(move || {
            structural
                .into_iter()
                .filter(|session| {
                    canonical_cwd_owned_by_replica(&session.working_directory, &replica_dir)
                        .unwrap_or(false)
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|error| format!("Orchestrator candidate path task failed: {}", error))?;
        owned.sort_by_key(|session| {
            let temporary = session
                .name
                .starts_with(crate::session::session::TEMP_SESSION_PREFIX);
            let status = match session.status {
                SessionStatus::Active | SessionStatus::Running => 0u8,
                SessionStatus::Idle => 1,
                SessionStatus::Exited(_) => 2,
            };
            (temporary, status)
        });

        let mut result = Vec::with_capacity(owned.len());
        for session in owned {
            let id = Uuid::parse_str(&session.id)
                .map_err(|e| format!("Invalid orchestrator session id '{}': {}", session.id, e))?;
            let has_pty = self.has_pty_session_for_wake(app, id).await;
            if matches!(session.status, SessionStatus::Exited(_)) || has_pty {
                result.push((session, has_pty));
            }
        }
        Ok(result)
    }

    async fn recheck_internal_exited_candidate<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        session_id: Uuid,
        target: &InternalSystemTarget,
    ) -> Result<(), String> {
        let session = {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let manager = session_mgr.read().await.clone();
            manager.get_session(session_id).await
        }
        .ok_or_else(|| format!("Exited orchestrator session {} disappeared", session_id))?;
        if !matches!(session.status, SessionStatus::Exited(_))
            || session.is_root_agent
            || session.agent_id.is_none()
            || !crate::pty::inject::needs_explicit_enter(&session.shell)
        {
            return Err(format!(
                "Orchestrator session {} changed before exited-session destruction",
                session_id
            ));
        }
        let cwd = session.working_directory;
        let replica = target.replica_dir().to_path_buf();
        let owned =
            tokio::task::spawn_blocking(move || canonical_cwd_owned_by_replica(&cwd, &replica))
                .await
                .map_err(|error| format!("Exited orchestrator path task failed: {}", error))??;
        if !owned {
            return Err(format!(
                "Orchestrator session {} no longer belongs to target replica",
                session_id
            ));
        }
        Ok(())
    }

    async fn recheck_internal_exited_record<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        session_id: Uuid,
        target: &InternalSystemTarget,
        expected_cwd: &str,
    ) -> Result<(), String> {
        let session = {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let manager = session_mgr.read().await.clone();
            manager.get_session(session_id).await
        }
        .ok_or_else(|| format!("Exited orchestrator session {} disappeared", session_id))?;
        if !matches!(session.status, SessionStatus::Exited(_))
            || session.is_root_agent
            || session.agent_id.is_none()
            || !crate::pty::inject::needs_explicit_enter(&session.shell)
            || session.working_directory != expected_cwd
            || !internal_target_fqn_prefilter_matches(&session.working_directory, target.fqn())
        {
            return Err(format!(
                "Orchestrator session {} restarted or changed immediately before destruction",
                session_id
            ));
        }
        let cwd = session.working_directory;
        let replica = target.replica_dir().to_path_buf();
        let owned =
            tokio::task::spawn_blocking(move || canonical_cwd_owned_by_replica(&cwd, &replica))
                .await
                .map_err(|error| {
                    format!(
                        "Final exited orchestrator path task failed for {}: {}",
                        session_id, error
                    )
                })??;
        if !owned {
            return Err(format!(
                "Orchestrator session {} escaped the canonical target before destruction",
                session_id
            ));
        }
        Ok(())
    }

    async fn inject_internal_system_notice<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        session_id: Uuid,
        target: &InternalSystemTarget,
        notice: &InternalSystemNotice,
        guard: InternalNoticeGuard,
    ) -> Result<(), String> {
        let payload =
            format_wake_content(WakeContent::InternalSystem(notice, self.messages.as_ref()));

        #[cfg(test)]
        if let Some(hooks) = &self.test_hooks {
            let session = {
                let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let manager = session_mgr.read().await.clone();
                manager.get_session(session_id).await
            }
            .ok_or_else(|| format!("Session {} missing before test injection", session_id))?;
            if session.is_root_agent
                || matches!(session.status, SessionStatus::Exited(_))
                || session.agent_id.is_none()
                || !crate::pty::inject::needs_explicit_enter(&session.shell)
                || !canonical_cwd_owned_by_replica(
                    &session.working_directory,
                    target.replica_dir(),
                )?
            {
                return Err(format!(
                    "Session {} failed supported-agent final validation",
                    session_id
                ));
            }
            guard()?;
            if let Some(reason) =
                crate::pty::inject::internal_notice_block_reason(app, session_id).await
            {
                return Err(reason);
            }
            hooks.inject_calls.lock().unwrap().push(session_id);
            hooks
                .internal_payloads
                .lock()
                .unwrap()
                .push(payload.clone());
            hooks
                .events
                .lock()
                .unwrap()
                .push(MailboxTestEvent::Inject(session_id));
            hooks
                .inject_results
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(()))?;
            hooks
                .internal_bookkeeping
                .lock()
                .unwrap()
                .push(InternalSystemBookkeeping {
                    session_id,
                    post_boundary: true,
                    silence_touch: true,
                    set_last_prompt: false,
                    peer_event: false,
                    response_watcher: false,
                    consumption_verdict: false,
                    mailbox_archive: false,
                });
            return Ok(());
        } else {
            crate::pty::inject::inject_internal_notice_into_supported_agent_session(
                app,
                session_id,
                &payload,
                |session: &Session| {
                    if !canonical_cwd_owned_by_replica(
                        &session.working_directory,
                        target.replica_dir(),
                    )? {
                        return Err(format!(
                            "Orchestrator session {} escaped canonical target replica",
                            session_id
                        ));
                    }
                    guard()
                },
            )
            .await?;
        }

        #[cfg(not(test))]
        crate::pty::inject::inject_internal_notice_into_supported_agent_session(
            app,
            session_id,
            &payload,
            |session: &Session| {
                if !canonical_cwd_owned_by_replica(
                    &session.working_directory,
                    target.replica_dir(),
                )? {
                    return Err(format!(
                        "Orchestrator session {} escaped canonical target replica",
                        session_id
                    ));
                }
                guard()
            },
        )
        .await?;

        crate::commands::pty::note_post_boundary_content_to_session(app, session_id).await;
        if let Some(idle) = app.try_state::<Arc<crate::pty::idle_detector::IdleDetector>>() {
            idle.touch_silence(session_id);
        }
        let (project, _) = crate::config::teams::split_project_prefix(target.fqn());
        match notice {
            // Byte-identical to the line this feature shipped, including the
            // field order and the `{:?}` thresholds: operators grep it.
            InternalSystemNotice::ContextAlert {
                member,
                workgroup,
                observed,
                thresholds,
            } => {
                log::info!(
                    "[context-alert] delivered coordinatorSession={} project={} \
                    workgroup={} member={} observed={} thresholds={:?}",
                    session_id,
                    project.unwrap_or(""),
                    workgroup,
                    member,
                    observed,
                    thresholds
                );
            }
            // A NEW prefix, never the context-alert one: two different notices
            // under one prefix would make that grep lie.
            InternalSystemNotice::RemoteActivity {
                kind,
                repo_nwo,
                branch,
                sha7,
                ..
            } => {
                log::info!(
                    "[remote-activity] delivered coordinatorSession={} project={} kind={:?} \
                    repo={} branch={} sha={}",
                    session_id,
                    project.unwrap_or(""),
                    kind,
                    repo_nwo,
                    branch,
                    sha7
                );
            }
        }
        Ok(())
    }

    pub(crate) async fn has_pty_session_for_wake<
        S: super::pty_observation::PtyAuthorityState + ?Sized,
    >(
        &self,
        app: &S,
        id: Uuid,
    ) -> bool {
        #[cfg(test)]
        if let Some(hooks) = &self.test_hooks {
            let scripted = {
                let presence = hooks.pty_presence.lock().unwrap();
                presence.get(&id).copied()
            };
            if let Some(has_pty) = scripted {
                return has_pty;
            }
        }

        let pty_mgr = app.pty_routes();
        let pty = pty_mgr.lock().unwrap();
        pty.has_session(id)
    }

    pub(crate) async fn inject_wake_into_pty<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        session_id: Uuid,
        msg: &OutboxMessage,
        attribution: (WakeDeliveryOrigin, bool),
        settle: SettleReadiness,
        context: (
            Option<&ResponseWakeWork>,
            Option<&crate::session::selection::OwnedReplacementContinuation>,
        ),
    ) -> Result<(), String> {
        let (response, owned_replacement) = context;
        let (origin, comanaged) = attribution;
        #[cfg(test)]
        if let Some(hooks) = self
            .test_hooks
            .as_ref()
            .filter(|_| owned_replacement.is_none())
        {
            {
                let mut calls = hooks.inject_calls.lock().unwrap();
                calls.push(session_id);
            }
            {
                let mut events = hooks.events.lock().unwrap();
                events.push(MailboxTestEvent::Inject(session_id));
            }
            if hooks.panic_on_inject.lock().unwrap().contains(&session_id) {
                panic!("test-scripted wake delivery panic");
            }
            let use_real_inject = hooks
                .real_inject_sessions
                .lock()
                .unwrap()
                .contains(&session_id);
            if !use_real_inject {
                let inject_result = {
                    let mut results = hooks.inject_results.lock().unwrap();
                    results.pop_front()
                }
                .unwrap_or(Ok(()));
                // (#1001 PR1 / G6) On a successful inject, if a consumption
                // verdict is scripted, run the SAME verdict_to_result the
                // production path uses so AC3 covers the real conversion. No
                // scripted verdict => Ok (existing hooked tests unchanged).
                return match inject_result {
                    Ok(()) => match hooks.consumption_results.lock().unwrap().pop_front() {
                        Some(verdict) => verdict_to_result(verdict),
                        None => Ok(()),
                    },
                    Err(e) => Err(e),
                };
            }
        }

        let result = loop {
            let result = self
                .inject_into_pty_with_response(
                    app,
                    session_id,
                    msg,
                    true,
                    origin,
                    comanaged,
                    settle,
                    (response, owned_replacement),
                )
                .await;
            if result
                .as_ref()
                .is_err_and(|error| error == crate::pty::inject::OWNED_WORK_SCOPE_WAIT)
            {
                // The injector has dropped its writer and route guards. An
                // ordinary unowned wake retains its source and attempts here.
                tokio::task::yield_now().await;
                continue;
            }
            break result;
        };
        if result.is_ok() {
            if let Some(attempt) = response.and_then(|work| match work.attempt.lock() {
                Ok(attempt) => attempt.clone(),
                Err(_) => {
                    response_diag_local!(
                        "mark_request_submitted",
                        "attempt_lock_failure",
                        work.diagnostic_message_id
                    );
                    None
                }
            }) {
                if let Ok(candidate) = attempt.candidate.lock() {
                    if let Some(candidate) = candidate.as_ref() {
                        response_diag_local!(
                            "mark_request_submitted",
                            "true_invocation",
                            Uuid::parse_str(&msg.id).ok()
                        );
                        attempt
                            .state
                            .mark_request_submitted(&candidate.context, true);
                    } else {
                        response_diag_local!(
                            "mark_request_submitted",
                            "candidate_absent",
                            None::<Uuid>
                        );
                    }
                } else {
                    response_diag_local!(
                        "mark_request_submitted",
                        "candidate_lock_failure",
                        None::<Uuid>
                    );
                }
            } else {
                response_diag_local!(
                    "mark_request_submitted",
                    "work_or_attempt_absent",
                    None::<Uuid>
                );
            }
        }
        // #552 auto-close: a successful inter-agent wake is activity for the
        // recipient team's silence clock (NOT the badge; inter-agent is not a
        // user message). This single site covers both wake paths (deliver_wake
        // and the spawned-wake path), which both funnel through here. The
        // recipient's own response output would also reset silence via the read
        // loop; recording the delivery makes inter-agent traffic count at once.
        if result.is_ok() {
            if let Some(idle) =
                app.try_state::<std::sync::Arc<crate::pty::idle_detector::IdleDetector>>()
            {
                idle.touch_silence(session_id);
            }
        }
        // (#1001 PR1 / G6) Route the successful-inject return through the
        // shared verdict_to_result so this production site runs the exact
        // conversion the AC3 hooked test exercises. PR1 wires no oracle yet,
        // so the verdict is NotApplicable => Ok(()): today's write-receipt
        // semantics, unchanged. A (PR3) replaces NotApplicable with the
        // observed verdict from the oracle driver.
        match result {
            Ok(()) => verdict_to_result(ConsumptionVerdict::NotApplicable),
            Err(e) => Err(e),
        }
    }

    pub(crate) async fn destroy_exited_wake_session<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        session_id: Uuid,
    ) -> Result<(), String> {
        #[cfg(test)]
        if let Some(hooks) = &self.test_hooks {
            {
                let mut calls = hooks.destroy_calls.lock().unwrap();
                calls.push(session_id);
            }
            {
                let mut events = hooks.events.lock().unwrap();
                events.push(MailboxTestEvent::Destroy(session_id));
            }
            if let Some(result) = hooks.destroy_results.lock().unwrap().pop_front() {
                result?;
            }
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            return mgr
                .destroy_session(session_id)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string());
        }

        crate::commands::session::background_destroy_session_inner(app, session_id).await
    }
}
