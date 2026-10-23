use super::{
    augment_coordinator_project, same_authority, PreparedSnapshotPayload, RequesterProof,
    RequesterSnapshotPermit, SelectedSession, TerminalSnapshotAuditGuard,
    TerminalSnapshotBlockingStage, TerminalSnapshotDisclosure, TerminalSnapshotFinalization,
    TerminalSnapshotState,
};
#[cfg(test)]
use super::{
    TerminalSnapshotHostFinalizerControl, TerminalSnapshotHostFinalizerStage,
    TerminalSnapshotTestTaskOutput,
};
use crate::config::teams::VerifiedTerminalSnapshotRoute;
use crate::pty::context_scrape::ContextSessionLiveness;
use crate::session::manager::{
    SessionManager, TerminalSnapshotRequesterFact, TerminalSnapshotSessionFact,
};
use crate::session::session::{SessionStatus, TEMP_SESSION_PREFIX};
use std::path::Path;
use std::sync::atomic::Ordering;
#[cfg(test)]
use std::sync::Arc;
use std::time::Instant;
use terminal_snapshot_renderer::TerminalSnapshotReasonCode;

pub(super) fn finalize_host_publication<F>(
    success_bytes: Vec<u8>,
    authority: Result<(), TerminalSnapshotReasonCode>,
    publish: F,
) -> Result<(), TerminalSnapshotReasonCode>
where
    F: FnOnce(
        Result<Vec<u8>, TerminalSnapshotReasonCode>,
    ) -> Result<(), TerminalSnapshotReasonCode>,
{
    match authority {
        Ok(()) => publish(Ok(success_bytes)),
        Err(reason) => match publish(Err(reason)) {
            Ok(()) => Err(reason),
            Err(_) => Err(TerminalSnapshotReasonCode::ResponseUnavailable),
        },
    }
}

pub(super) async fn final_revalidate_async(
    finalization: &TerminalSnapshotFinalization,
) -> Result<(), TerminalSnapshotReasonCode> {
    let state = &finalization.state;
    let context = &finalization.context;
    let manager = &finalization.manager;
    let deadline = finalization.deadline;
    let permit = &finalization.permit;
    let audit = &finalization.audit;
    let requester = &finalization.requester;
    let route = &finalization.route;
    let selected = &finalization.selected;
    ensure_before_deadline(deadline, &state.shutdown)?;
    let security = await_deadline(
        deadline,
        crate::config::settings::read_terminal_snapshot_security_settings_strict_offloaded(),
    )
    .await?
    .map_err(|_| TerminalSnapshotReasonCode::AuthorityChanged)?;
    let memory_enabled = await_deadline(deadline, context.settings.current_read())
        .await?
        .map_err(|_| TerminalSnapshotReasonCode::AuthorityChanged)?
        .terminal_snapshots_enabled;
    if !security.terminal_snapshots_enabled
        || !memory_enabled
        || context.restore.0.load(Ordering::SeqCst)
        || context.purge.blocks_agent(&route.target.canonical_fqn)
        || context.purge.blocks_session(selected.fact.id)
    {
        return Err(TerminalSnapshotReasonCode::AuthorityChanged);
    }
    let sender_cwd = requester.fact.working_directory.clone();
    let target_fqn = route.target.canonical_fqn.clone();
    let mut project_paths = security.project_paths;
    let sender_is_root = requester.fact.is_root_agent;
    if !sender_is_root {
        augment_coordinator_project(&mut project_paths, &requester.identity)
            .map_err(|_| TerminalSnapshotReasonCode::AuthorityChanged)?;
    }
    let fresh_route = run_blocking_with_deadline(
        state,
        TerminalSnapshotBlockingStage::FinalVerification,
        deadline,
        permit,
        audit,
        move || {
            crate::config::teams::verify_terminal_snapshot_route(
                std::path::Path::new(&sender_cwd),
                sender_is_root,
                &target_fqn,
                &project_paths,
            )
        },
    )
    .await?
    .map_err(|_| TerminalSnapshotReasonCode::AuthorityChanged)?;
    if !same_authority(&requester.identity, &fresh_route.sender)
        || !same_authority(&route.target, &fresh_route.target)
    {
        return Err(TerminalSnapshotReasonCode::AuthorityChanged);
    }
    if let Some(token) = requester.host_token {
        let token_fact = await_deadline(
            deadline,
            manager.find_unique_live_snapshot_requester_by_token(token),
        )
        .await?
        .map_err(|_| TerminalSnapshotReasonCode::AuthorityChanged)?;
        if token_fact.id != requester.fact.id || token_fact.created_at != requester.fact.created_at
        {
            return Err(TerminalSnapshotReasonCode::AuthorityChanged);
        }
    }
    verify_current_requester_async(deadline, manager, requester).await?;
    verify_current_selected_async(deadline, manager, route, selected).await?;
    ensure_before_deadline(deadline, &state.shutdown)
}

pub(super) fn final_revalidate_blocking(
    finalization: &TerminalSnapshotFinalization,
) -> Result<(), TerminalSnapshotReasonCode> {
    let context = &finalization.context;
    let manager = &finalization.manager;
    let requester = &finalization.requester;
    let route = &finalization.route;
    let selected = &finalization.selected;
    ensure_host_finalizer_before_deadline(finalization)?;
    let security = crate::config::settings::read_terminal_snapshot_security_settings_strict()
        .map_err(|_| TerminalSnapshotReasonCode::AuthorityChanged)?;
    let memory_enabled = context
        .settings
        .blocking_current_read()
        .map_err(|_| TerminalSnapshotReasonCode::AuthorityChanged)?
        .terminal_snapshots_enabled;
    if !security.terminal_snapshots_enabled
        || !memory_enabled
        || context.restore.0.load(Ordering::SeqCst)
        || context.purge.blocks_agent(&route.target.canonical_fqn)
        || context.purge.blocks_session(selected.fact.id)
    {
        return Err(TerminalSnapshotReasonCode::AuthorityChanged);
    }
    let mut project_paths = security.project_paths;
    if !requester.fact.is_root_agent {
        augment_coordinator_project(&mut project_paths, &requester.identity)
            .map_err(|_| TerminalSnapshotReasonCode::AuthorityChanged)?;
    }
    #[cfg(test)]
    finalization.run_host_finalizer_control(TerminalSnapshotHostFinalizerStage::RouteVerification);
    let fresh_route = crate::config::teams::verify_terminal_snapshot_route(
        Path::new(&requester.fact.working_directory),
        requester.fact.is_root_agent,
        &route.target.canonical_fqn,
        &project_paths,
    )
    .map_err(|_| TerminalSnapshotReasonCode::AuthorityChanged)?;
    if !same_authority(&requester.identity, &fresh_route.sender)
        || !same_authority(&route.target, &fresh_route.target)
    {
        return Err(TerminalSnapshotReasonCode::AuthorityChanged);
    }
    if let Some(token) = requester.host_token {
        let token_fact = manager
            .find_unique_live_snapshot_requester_by_token_blocking(token)
            .map_err(|_| TerminalSnapshotReasonCode::AuthorityChanged)?;
        if token_fact.id != requester.fact.id || token_fact.created_at != requester.fact.created_at
        {
            return Err(TerminalSnapshotReasonCode::AuthorityChanged);
        }
    }
    verify_current_requester_blocking(manager, requester)?;
    verify_current_selected_blocking(manager, route, selected)?;
    if finalization
        .host_wall_deadline
        .is_some_and(|wall_deadline| chrono::Utc::now() >= wall_deadline)
    {
        return Err(TerminalSnapshotReasonCode::SnapshotTimeout);
    }
    #[cfg(test)]
    finalization.run_host_finalizer_control(TerminalSnapshotHostFinalizerStage::FinalDeadline);
    ensure_host_finalizer_before_deadline(finalization)
}

pub(super) fn ensure_host_finalizer_before_deadline(
    finalization: &TerminalSnapshotFinalization,
) -> Result<(), TerminalSnapshotReasonCode> {
    #[cfg(test)]
    if finalization
        .host_finalizer_control
        .as_ref()
        .is_some_and(|control| control.deadline_expired())
    {
        return Err(TerminalSnapshotReasonCode::SnapshotTimeout);
    }
    ensure_before_deadline(finalization.deadline, &finalization.state.shutdown)
}

pub(super) fn map_envelope_error(
    error: terminal_snapshot_renderer::ProtocolError,
) -> TerminalSnapshotReasonCode {
    match error {
        terminal_snapshot_renderer::ProtocolError::TooLarge => {
            TerminalSnapshotReasonCode::SnapshotTooLarge
        }
        _ => TerminalSnapshotReasonCode::Internal,
    }
}

pub(super) async fn verify_current_requester_async(
    deadline: Instant,
    manager: &SessionManager,
    requester: &RequesterProof,
) -> Result<(), TerminalSnapshotReasonCode> {
    let current = await_deadline(
        deadline,
        manager.live_snapshot_requester_by_id(requester.fact.id),
    )
    .await?
    .ok_or(TerminalSnapshotReasonCode::AuthorityChanged)?;
    verify_requester_fact(requester, &current)
}

pub(super) fn verify_current_requester_blocking(
    manager: &SessionManager,
    requester: &RequesterProof,
) -> Result<(), TerminalSnapshotReasonCode> {
    let current = manager
        .live_snapshot_requester_by_id_blocking(requester.fact.id)
        .ok_or(TerminalSnapshotReasonCode::AuthorityChanged)?;
    verify_requester_fact(requester, &current)
}

pub(super) fn verify_requester_fact(
    requester: &RequesterProof,
    current: &TerminalSnapshotRequesterFact,
) -> Result<(), TerminalSnapshotReasonCode> {
    if current.created_at != requester.fact.created_at
        || current.working_directory != requester.fact.working_directory
        || current.backend_kind != requester.fact.backend_kind
        || current.is_root_agent != requester.fact.is_root_agent
        || current.is_coordinator != requester.fact.is_coordinator
        || requester.route.liveness() != ContextSessionLiveness::Live
        || !requester.route.matches_requester_route(
            requester.fact.backend_kind,
            &requester.cwd_identity,
            if requester.fact.is_root_agent {
                None
            } else {
                Some(&requester.identity.replica_identity)
            },
        )
    {
        return Err(TerminalSnapshotReasonCode::AuthorityChanged);
    }
    Ok(())
}

pub(super) async fn verify_current_selected_async(
    deadline: Instant,
    manager: &SessionManager,
    route: &VerifiedTerminalSnapshotRoute,
    selected: &SelectedSession,
) -> Result<(), TerminalSnapshotReasonCode> {
    let current = await_deadline(
        deadline,
        manager.terminal_snapshot_session_fact_by_id(selected.fact.id),
    )
    .await?
    .ok_or(TerminalSnapshotReasonCode::AuthorityChanged)?;
    verify_selected_fact(route, selected, &current)
}

pub(super) fn verify_current_selected_blocking(
    manager: &SessionManager,
    route: &VerifiedTerminalSnapshotRoute,
    selected: &SelectedSession,
) -> Result<(), TerminalSnapshotReasonCode> {
    let current = manager
        .terminal_snapshot_session_fact_by_id_blocking(selected.fact.id)
        .ok_or(TerminalSnapshotReasonCode::AuthorityChanged)?;
    verify_selected_fact(route, selected, &current)
}

pub(super) fn verify_selected_fact(
    route: &VerifiedTerminalSnapshotRoute,
    selected: &SelectedSession,
    current: &TerminalSnapshotSessionFact,
) -> Result<(), TerminalSnapshotReasonCode> {
    if current.created_at != selected.fact.created_at
        || current.working_directory != selected.fact.working_directory
        || current.backend_kind != selected.fact.backend_kind
        || current.name != selected.fact.name
        || current.name.starts_with(TEMP_SESSION_PREFIX)
        || matches!(current.status, SessionStatus::Exited(_))
        || selected.proof.liveness() != ContextSessionLiveness::Live
        || !selected.proof.matches_current(
            selected.fact.backend_kind,
            &selected.cwd_identity,
            &route.target.replica_identity,
        )
    {
        return Err(TerminalSnapshotReasonCode::AuthorityChanged);
    }
    Ok(())
}

pub(super) fn ensure_before_deadline(
    deadline: Instant,
    shutdown: &crate::shutdown::ShutdownSignal,
) -> Result<(), TerminalSnapshotReasonCode> {
    if shutdown.is_cancelled() {
        return Err(TerminalSnapshotReasonCode::ServiceUnavailable);
    }
    if Instant::now() >= deadline {
        return Err(TerminalSnapshotReasonCode::SnapshotTimeout);
    }
    Ok(())
}

pub(super) async fn await_deadline<F, T>(
    deadline: Instant,
    future: F,
) -> Result<T, TerminalSnapshotReasonCode>
where
    F: std::future::Future<Output = T>,
{
    let remaining = deadline.saturating_duration_since(Instant::now());
    tokio::time::timeout(remaining, future)
        .await
        .map_err(|_| TerminalSnapshotReasonCode::SnapshotTimeout)
}

pub(super) async fn run_blocking_with_deadline<F, T>(
    state: &TerminalSnapshotState,
    stage: TerminalSnapshotBlockingStage,
    deadline: Instant,
    permit: &RequesterSnapshotPermit,
    audit: &TerminalSnapshotAuditGuard,
    operation: F,
) -> Result<T, TerminalSnapshotReasonCode>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    #[cfg(test)]
    let control = state.take_blocking_control(stage);
    #[cfg(not(test))]
    let _ = (state, stage);

    let permit = permit.clone();
    let audit = audit.clone();
    #[cfg(test)]
    let worker_control = control.clone();
    #[cfg(test)]
    let completion_control = control.clone();
    #[cfg(test)]
    let mut handle = tokio::task::spawn_blocking(move || {
        let outcome = crate::logging::catch_payload_unwind(move || {
            let _permit = permit;
            let _audit = audit;
            if let Some(control) = worker_control {
                control.enter_worker();
            }
            operation()
        });
        TerminalSnapshotTestTaskOutput {
            outcome: Some(outcome),
            control: completion_control,
        }
    });
    #[cfg(not(test))]
    let mut handle = tokio::task::spawn_blocking(move || {
        crate::logging::catch_payload_unwind(move || {
            let _permit = permit;
            let _audit = audit;
            operation()
        })
    });

    #[cfg(test)]
    let joined = if let Some(control) = control {
        let remaining = deadline.saturating_duration_since(Instant::now());
        tokio::select! {
            joined = &mut handle => joined,
            _ = tokio::time::sleep(remaining) => {
                return Err(TerminalSnapshotReasonCode::SnapshotTimeout);
            }
            _ = control.deadline_expired() => {
                return Err(TerminalSnapshotReasonCode::SnapshotTimeout);
            }
        }
    } else {
        await_deadline(deadline, &mut handle).await?
    };
    #[cfg(not(test))]
    let joined = await_deadline(deadline, &mut handle).await?;
    #[cfg(test)]
    let joined = joined.map(TerminalSnapshotTestTaskOutput::into_outcome);

    match crate::logging::collapse_payload_task(joined) {
        Ok(value) => Ok(value),
        Err(_) => {
            log::error!("[terminal-snapshot] stage=blocking_task code=internal");
            Err(TerminalSnapshotReasonCode::Internal)
        }
    }
}

impl TerminalSnapshotFinalization {
    #[cfg(test)]
    pub(crate) fn install_host_finalizer_control(
        &mut self,
        control: Arc<TerminalSnapshotHostFinalizerControl>,
    ) {
        self.host_finalizer_control = Some(control);
    }

    #[cfg(test)]
    pub(super) fn run_host_finalizer_control(&self, stage: TerminalSnapshotHostFinalizerStage) {
        if let Some(control) = self.host_finalizer_control.as_ref() {
            control.enter_stage(stage);
        }
    }

    pub(crate) async fn build_api_response(
        &self,
        payload: PreparedSnapshotPayload,
    ) -> Result<Vec<u8>, TerminalSnapshotReasonCode> {
        #[cfg(test)]
        self.state.mark_blocking_payload_retention(
            TerminalSnapshotBlockingStage::ApiEnvelope,
            payload.retained_content_bytes(),
        );
        run_blocking_with_deadline(
            &self.state,
            TerminalSnapshotBlockingStage::ApiEnvelope,
            self.deadline,
            &self.permit,
            &self.audit,
            move || payload.encode_api().map_err(map_envelope_error),
        )
        .await?
    }

    pub(crate) async fn build_host_response(
        &self,
        payload: PreparedSnapshotPayload,
        request_id: String,
        confirmation_tag: String,
        expires_at: String,
    ) -> Result<Vec<u8>, TerminalSnapshotReasonCode> {
        #[cfg(test)]
        self.state.mark_blocking_payload_retention(
            TerminalSnapshotBlockingStage::HostEnvelope,
            payload.retained_content_bytes(),
        );
        run_blocking_with_deadline(
            &self.state,
            TerminalSnapshotBlockingStage::HostEnvelope,
            self.deadline,
            &self.permit,
            &self.audit,
            move || {
                payload
                    .encode_host(&request_id, &confirmation_tag, &expires_at)
                    .map_err(map_envelope_error)
            },
        )
        .await?
    }

    pub(crate) async fn revalidate_api(
        self,
    ) -> Result<TerminalSnapshotDisclosure, TerminalSnapshotReasonCode> {
        if self.host_wall_deadline.is_some() {
            self.audit
                .finalize_failure(TerminalSnapshotReasonCode::Internal);
            return Err(TerminalSnapshotReasonCode::Internal);
        }
        if let Err(reason) = final_revalidate_async(&self).await {
            self.audit.finalize_failure(reason);
            return Err(reason);
        }
        Ok(TerminalSnapshotDisclosure {
            state: self.state,
            permit: self.permit,
            audit: self.audit,
            deadline: self.deadline,
        })
    }

    pub(crate) fn fail_host<F>(
        self,
        reason: TerminalSnapshotReasonCode,
        publish: F,
    ) -> Result<(), TerminalSnapshotReasonCode>
    where
        F: FnOnce(TerminalSnapshotReasonCode) -> Result<(), TerminalSnapshotReasonCode>,
    {
        let result = match publish(reason) {
            Ok(()) => Err(reason),
            Err(_) => Err(TerminalSnapshotReasonCode::ResponseUnavailable),
        };
        if let Err(final_reason) = result {
            self.audit.finalize_failure(final_reason);
        }
        drop(self.permit);
        result
    }

    pub(crate) fn finalize_host<F>(
        self,
        success_bytes: Vec<u8>,
        publish: F,
    ) -> Result<(), TerminalSnapshotReasonCode>
    where
        F: FnOnce(
            Result<Vec<u8>, TerminalSnapshotReasonCode>,
        ) -> Result<(), TerminalSnapshotReasonCode>,
    {
        #[cfg(test)]
        if let Some(control) = self.host_finalizer_control.as_ref() {
            control.set_retained_response_bytes(success_bytes.len());
        }
        #[cfg(test)]
        self.state.run_host_final_handoff_hook();
        #[cfg(test)]
        self.run_host_finalizer_control(TerminalSnapshotHostFinalizerStage::RevalidationEntry);
        let result =
            finalize_host_publication(success_bytes, final_revalidate_blocking(&self), publish);
        match result {
            Ok(()) => self.audit.finalize("succeeded", None),
            Err(reason) => self.audit.finalize_failure(reason),
        }
        drop(self.permit);
        result
    }
}
