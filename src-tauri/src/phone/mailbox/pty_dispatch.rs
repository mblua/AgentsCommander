mod context;
mod select;
use crate::phone::mailbox::poller::MailboxPoller;
#[cfg(test)]
use crate::phone::mailbox::pty_claim::MailboxTestHooks;
use crate::phone::mailbox::pty_claim::{
    await_pty_input_before_deadline, finish_pty_input_before_boundary,
    pty_input_lease_failure_code, reject_pty_input_before_boundary, PtyInputPreBoundaryStop,
};
use crate::phone::mailbox::settle::{finish_owned_pty_work, OwnedPtyWorkTransfer};
use crate::pty::backend::SessionBackendKind;
use crate::pty::manager::PtyManager;
use context::{
    BoundaryAuthority, DispatchSelection, InitialDispatchContext, PtyDispatchContext,
    PtyDispatchGuards, SelectedPtyTarget, SelectionCandidates, SpawnAttempt,
};
use select::BoundaryWriteGuards;
use std::sync::Arc;
use tauri::Manager;
use uuid::Uuid;

/// Payload and authority validated together before the first write.
#[derive(Clone, Copy)]
struct FinalDispatchInput<'a> {
    final_authority: &'a crate::config::teams::VerifiedPtyInputRoute,
    payload: &'a [u8],
}

struct PtyDispatcher<'a, A> {
    poller: &'a MailboxPoller,
    app: &'a A,
}
impl<A> std::ops::Deref for PtyDispatcher<'_, A> {
    type Target = MailboxPoller;
    fn deref(&self) -> &MailboxPoller {
        self.poller
    }
}
impl MailboxPoller {
    pub(crate) async fn dispatch_pty_input_operation_core<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        state: &crate::api::message_store::MessageStoreState,
        injection_id: &str,
        source_plane: crate::phone::types::PtyInputSourcePlane,
        api_client_store: Option<&Arc<crate::api::auth::ApiClientStore>>,
    ) {
        let dispatcher = PtyDispatcher { poller: self, app };
        let mut source_executor = crate::session::selection::SourceExecutorGuard::unbound();
        #[cfg(test)]
        if let Some(control) = self
            .test_hooks
            .as_ref()
            .and_then(MailboxTestHooks::replacement_fixture_control)
        {
            if !control.after_pty_core_accept(injection_id).await {
                return;
            }
        }
        // Borrow the lexical observer through the whole existing dispatch
        // body. This is polled inline, with no new task or accepted core.
        let failed = dispatcher
            .dispatch_accepted_operation(
                state,
                injection_id,
                source_plane,
                api_client_store,
                &mut source_executor,
            )
            .await;
        source_executor.returned(failed);
    }
}
impl<R: tauri::Runtime> PtyDispatcher<'_, tauri::AppHandle<R>> {
    async fn dispatch_accepted_operation(
        &self,
        state: &crate::api::message_store::MessageStoreState,
        injection_id: &str,
        source_plane: crate::phone::types::PtyInputSourcePlane,
        api_client_store: Option<&Arc<crate::api::auth::ApiClientStore>>,
        source_executor: &mut crate::session::selection::SourceExecutorGuard,
    ) -> bool {
        let app = self.app;
        let store = match &state.store {
            Ok(store) => Arc::clone(store),
            Err(_) => return false,
        };
        let shutdown = app
            .try_state::<crate::shutdown::ShutdownSignal>()
            .map(|signal| signal.token().clone())
            .unwrap_or_else(tokio_util::sync::CancellationToken::new);
        let _operation_lock = match store.try_operation_lock(injection_id) {
            Ok(Some(lock)) => lock,
            _ => return false,
        };
        let _active = match state.active_operations.try_register(injection_id) {
            Some(active) => active,
            None => return false,
        };
        let initial = match store
            .query_pty_input_by_injection_offloaded(injection_id.to_string())
            .await
        {
            Ok(Some(result)) if !result.terminal => result,
            _ => return false,
        };
        let target = match initial.target.clone() {
            Some(target) => target,
            None => return false,
        };
        let _response_delivery = match store.begin_response_delivery(&target) {
            Ok(reservation) => reservation,
            Err(_) => return false,
        };
        let target_gate = match &state.target_gate {
            Ok(gate) => Arc::clone(gate),
            Err(_) => return false,
        };
        if !self.wait_for_dispatch_restore().await {
            return false;
        }
        let mut target_stripe = Some(match target_gate.try_target_lock(&target) {
            Ok(Some(lock)) => lock,
            _ => return false,
        });
        let mut target_guard = Some(
            match self
                .acquire_initial_dispatch_target(
                    &store,
                    &initial,
                    injection_id,
                    &target_gate,
                    &target,
                    &shutdown,
                )
                .await
            {
                Ok(guard) => guard,
                Err(failed) => return failed,
            },
        );
        {
            let (Some(stripe), Some(exact)) = (target_stripe.as_ref(), target_guard.as_ref())
            else {
                return false;
            };
            if target_gate
                .target_ownership(&target, stripe, exact)
                .is_err()
            {
                return false;
            }
        }
        self.dispatch_locked_operation(
            InitialDispatchContext {
                store: &store,
                initial: &initial,
                injection_id,
                source_plane,
                api_client_store,
                target_gate: &target_gate,
                target: &target,
                shutdown: &shutdown,
            },
            &mut target_guard,
            &mut target_stripe,
            source_executor,
        )
        .await
    }
    async fn wait_for_dispatch_restore(&self) -> bool {
        let app = self.app;
        // Startup restore must finish before this dispatch owns target guards.
        // No claim or payload has been consumed at this point.
        if let Some(coordinator) =
            app.try_state::<crate::session::selection::SelectionCoordinator>()
        {
            if coordinator.wait_for_restore_release().await.is_err() {
                return false;
            }
        }
        true
    }
    async fn dispatch_locked_operation(
        &self,
        initial_context: InitialDispatchContext<'_>,
        target_guard: &mut Option<crate::api::message_store::PtyInputTargetGuard>,
        target_stripe: &mut Option<crate::api::message_store::PtyInputStripeGuard>,
        source_executor: &mut crate::session::selection::SourceExecutorGuard,
    ) -> bool {
        let prepared = match context::prepare_dispatch_claim(&initial_context).await {
            Some(prepared) => prepared,
            None => return false,
        };
        let context::PreparedDispatchClaim {
            lease_owner,
            claimed,
            expires,
            mut heartbeat,
        } = prepared;
        let mut owned_work: Option<OwnedPtyWorkTransfer> = None;
        let context = PtyDispatchContext {
            store: initial_context.store,
            initial: initial_context.initial,
            claimed: &claimed,
            injection_id: initial_context.injection_id,
            lease_owner: &lease_owner,
            source_plane: initial_context.source_plane,
            api_client_store: initial_context.api_client_store,
            target_gate: initial_context.target_gate,
            target: initial_context.target,
            expires,
            shutdown: initial_context.shutdown,
        };
        let mut guards = PtyDispatchGuards {
            target_guard,
            target_stripe,
            heartbeat: &mut heartbeat,
            owned_work: &mut owned_work,
            source_executor,
        };
        self.dispatch_claimed_operation(&context, &mut guards).await
    }
    async fn dispatch_claimed_operation(
        &self,
        context: &PtyDispatchContext<'_>,
        guards: &mut PtyDispatchGuards<'_>,
    ) -> bool {
        let (selected, spawned, verified_target, session_manager, pty_manager) = match self
            .select_pty_dispatch_session(context, guards.reborrow())
            .await
        {
            Ok(value) => value,
            Err(failed) => return failed,
        };
        let mut selection = DispatchSelection {
            selected: &selected,
            spawned,
            verified_target: &verified_target,
            session_manager: &session_manager,
            pty_manager: &pty_manager,
            selected_id: Uuid::nil(),
        };
        selection.selected_id = match self.ready_dispatch(context, guards, &selection).await {
            Ok(id) => id,
            Err(failed) => return failed,
        };
        let permit = match self.dispatch_writer(context, guards, &selection).await {
            Ok(value) => value,
            Err(failed) => return failed,
        };
        let write_guards = match self.dispatch_fresh_binding(context, guards, permit).await {
            Ok(value) => value,
            Err(failed) => return failed,
        };
        let (final_authority, write_guards) = match self
            .dispatch_final_authority(context, guards, &selection, write_guards)
            .await
        {
            Ok(value) => value,
            Err(failed) => return failed,
        };
        let (payload, write_guards) = match self
            .dispatch_payload(context, guards, &selection, write_guards)
            .await
        {
            Ok(value) => value,
            Err(failed) => return failed,
        };
        let write_guards = match self
            .dispatch_post_authority(
                context,
                guards,
                &selection,
                write_guards,
                FinalDispatchInput {
                    final_authority: &final_authority,
                    payload: &payload,
                },
            )
            .await
        {
            Ok(value) => value,
            Err(failed) => return failed,
        };
        self.dispatch_boundary(
            context,
            guards,
            &selection,
            write_guards,
            FinalDispatchInput {
                final_authority: &final_authority,
                payload: &payload,
            },
        )
        .await
    }
    async fn acquire_initial_dispatch_target(
        &self,
        store: &Arc<crate::api::message_store::MessageStore>,
        initial: &crate::phone::types::PtyInputResult,
        injection_id: &str,
        target_gate: &Arc<crate::api::message_store::PtyInputTargetGate>,
        target: &str,
        shutdown: &tokio_util::sync::CancellationToken,
    ) -> Result<crate::api::message_store::PtyInputTargetGuard, bool> {
        use crate::phone::types::{PtyInputPublicStatus as S, PtyInputReasonCode as C};
        let initial_expires = match initial
            .expires_at
            .as_deref()
            .and_then(|value| crate::phone::types::parse_canonical_pty_timestamp(value).ok())
        {
            Some(expires) => expires,
            None => {
                if store
                    .terminalize_pty_input_offloaded(
                        injection_id.to_string(),
                        S::Rejected,
                        Some(C::StoreCorrupt),
                        chrono::Utc::now(),
                    )
                    .await
                    .is_err()
                {
                    log::warn!(
                        "[pty-input] invalid initial deadline id={} code=terminal_store_failed",
                        injection_id
                    );
                }
                return Err(false);
            }
        };
        let guard = match await_pty_input_before_deadline(
            initial_expires,
            shutdown,
            target_gate.acquire_exact(target),
        )
        .await
        {
            Ok(guard) => guard,
            Err(PtyInputPreBoundaryStop::Shutdown) => return Err(false),
            Err(PtyInputPreBoundaryStop::Expired) => {
                if store
                    .terminalize_pty_input_offloaded(
                        injection_id.to_string(),
                        S::Rejected,
                        Some(C::Expired),
                        chrono::Utc::now(),
                    )
                    .await
                    .is_err()
                {
                    log::warn!(
                        "[pty-input] target-wait expiry failed id={} code=terminal_store_failed",
                        injection_id
                    );
                }
                return Err(false);
            }
        };

        Ok(guard)
    }
    async fn dispatch_fresh_binding(
        &self,
        context: &PtyDispatchContext<'_>,
        guards: &mut PtyDispatchGuards<'_>,
        permit: crate::pty::manager::PtyInputPermit,
    ) -> Result<BoundaryWriteGuards, bool> {
        let app = self.app;

        use crate::phone::types::PtyInputReasonCode as C;
        let fresh =
            if context.source_plane == crate::phone::types::PtyInputSourcePlane::ContainerApi {
                let Some(client_store) = context.api_client_store else {
                    let observed = reject_pty_input_before_boundary(
                        context.store,
                        guards.heartbeat,
                        context.injection_id,
                        C::AuthorityChanged,
                    )
                    .await;
                    drop(guards.target_guard.take());
                    drop(guards.target_stripe.take());
                    drop(permit);
                    finish_owned_pty_work(app, guards.owned_work, observed).await;
                    return Err(false);
                };
                let (Some(client_id), Some(generation)) = (
                    context.claimed.authority_client_id.as_deref(),
                    context.claimed.authority_client_generation.as_deref(),
                ) else {
                    let observed = reject_pty_input_before_boundary(
                        context.store,
                        guards.heartbeat,
                        context.injection_id,
                        C::AuthorityChanged,
                    )
                    .await;
                    drop(guards.target_guard.take());
                    drop(guards.target_stripe.take());
                    drop(permit);
                    finish_owned_pty_work(app, guards.owned_work, observed).await;
                    return Err(false);
                };
                match client_store
                    .load_active_binding_fresh_offloaded(
                        client_id.to_string(),
                        generation.to_string(),
                        crate::api::auth::SCOPE_PTY_INPUT,
                    )
                    .await
                {
                    Ok(Some(guard)) => Some(guard),
                    Ok(None) => {
                        let observed = reject_pty_input_before_boundary(
                            context.store,
                            guards.heartbeat,
                            context.injection_id,
                            C::AuthorityChanged,
                        )
                        .await;
                        drop(guards.target_guard.take());
                        drop(guards.target_stripe.take());
                        drop(permit);
                        finish_owned_pty_work(app, guards.owned_work, observed).await;
                        return Err(false);
                    }
                    Err(_) => {
                        let observed = finish_pty_input_before_boundary(
                            context.store,
                            guards.heartbeat,
                            context.injection_id,
                            context.lease_owner,
                            C::StoreTransient,
                        )
                        .await;
                        drop(guards.target_guard.take());
                        drop(guards.target_stripe.take());
                        drop(permit);
                        finish_owned_pty_work(app, guards.owned_work, observed).await;
                        return Err(false);
                    }
                }
            } else {
                None
            };

        Ok(BoundaryWriteGuards {
            final_api_guard: fresh,
            permit,
        })
    }
    async fn dispatch_final_authority(
        &self,
        context: &PtyDispatchContext<'_>,
        guards: &mut PtyDispatchGuards<'_>,
        selection: &DispatchSelection<'_>,
        write_guards: BoundaryWriteGuards,
    ) -> Result<
        (
            crate::config::teams::VerifiedPtyInputRoute,
            BoundaryWriteGuards,
        ),
        bool,
    > {
        let app = self.app;

        use crate::phone::types::PtyInputReasonCode as C;
        let final_authority = if let Some(fresh) = write_guards.final_api_guard.as_ref() {
            self.validate_claimed_api_authority_with_fresh(app, context.claimed, fresh)
                .await
        } else {
            self.validate_claimed_host_authority(app, context.claimed)
                .await
        };
        let Some(final_authority) = final_authority else {
            let observed = reject_pty_input_before_boundary(
                context.store,
                guards.heartbeat,
                context.injection_id,
                C::AuthorityChanged,
            )
            .await;
            drop(guards.target_guard.take());
            drop(guards.target_stripe.take());
            drop(write_guards.permit);
            finish_owned_pty_work(app, guards.owned_work, observed).await;
            return Err(false);
        };
        if final_authority.target.authority_fingerprint
            != selection.verified_target.authority_fingerprint
            || !crate::path_identity::same_object(
                &final_authority.target.replica_identity,
                &selection.verified_target.replica_identity,
            )
        {
            let observed = reject_pty_input_before_boundary(
                context.store,
                guards.heartbeat,
                context.injection_id,
                C::AuthorityChanged,
            )
            .await;
            drop(guards.target_guard.take());
            drop(guards.target_stripe.take());
            drop(write_guards.permit);
            finish_owned_pty_work(app, guards.owned_work, observed).await;
            return Err(false);
        }
        if self
            .validate_selected_pty_target(
                app,
                context.claimed,
                selection.verified_target,
                selection.selected_id,
                selection.selected.backend_kind,
            )
            .await
            .is_none()
        {
            let observed = finish_pty_input_before_boundary(
                context.store,
                guards.heartbeat,
                context.injection_id,
                context.lease_owner,
                C::SessionRace,
            )
            .await;
            drop(guards.target_guard.take());
            drop(guards.target_stripe.take());
            drop(write_guards.permit);
            finish_owned_pty_work(app, guards.owned_work, observed).await;
            return Err(false);
        }
        if let Some(code) = Self::dispatch_final_lease_failure(context, guards.heartbeat).await {
            let observed = finish_pty_input_before_boundary(
                context.store,
                guards.heartbeat,
                context.injection_id,
                context.lease_owner,
                code,
            )
            .await;
            drop(guards.target_guard.take());
            drop(guards.target_stripe.take());
            drop(write_guards.permit);
            finish_owned_pty_work(app, guards.owned_work, observed).await;
            return Err(false);
        }

        Ok((final_authority, write_guards))
    }
    async fn dispatch_final_lease_failure(
        context: &PtyDispatchContext<'_>,
        heartbeat: &mut crate::api::message_store::PreparationHeartbeatGuard,
    ) -> Option<crate::phone::types::PtyInputReasonCode> {
        use crate::phone::types::PtyInputReasonCode as C;
        if heartbeat.failed() {
            return Some(if context.expires <= chrono::Utc::now() {
                C::Expired
            } else {
                C::LeaseLost
            });
        }
        if !matches!(
            context
                .store
                .renew_pty_input_lease_offloaded(
                    context.injection_id.to_string(),
                    context.lease_owner.to_string(),
                    chrono::Utc::now(),
                )
                .await,
            Ok(true)
        ) {
            return Some(pty_input_lease_failure_code(context.expires));
        }
        if !heartbeat.finish().await {
            return Some(pty_input_lease_failure_code(context.expires));
        }
        None
    }
    async fn parse_dispatch_session_id(
        &self,
        context: &PtyDispatchContext<'_>,
        guards: &mut PtyDispatchGuards<'_>,
        selection: &DispatchSelection<'_>,
    ) -> Result<Uuid, bool> {
        let app = self.app;
        use crate::phone::types::PtyInputReasonCode as C;
        match Uuid::parse_str(&selection.selected.id) {
            Ok(id) => Ok(id),
            Err(_) => {
                let observed = reject_pty_input_before_boundary(
                    context.store,
                    guards.heartbeat,
                    context.injection_id,
                    C::InconsistentSession,
                )
                .await;
                drop(guards.target_guard.take());
                drop(guards.target_stripe.take());
                finish_owned_pty_work(app, guards.owned_work, observed).await;
                Err(false)
            }
        }
    }
    async fn ready_dispatch(
        &self,
        context: &PtyDispatchContext<'_>,
        guards: &mut PtyDispatchGuards<'_>,
        selection: &DispatchSelection<'_>,
    ) -> Result<Uuid, bool> {
        let app = self.app;

        use crate::phone::types::PtyInputReasonCode as C;
        let selected_id = self
            .parse_dispatch_session_id(context, guards, selection)
            .await?;
        let readiness = await_pty_input_before_deadline(
            context.expires,
            context.shutdown,
            self.wait_for_pty_input_ready(
                app,
                context.claimed,
                selection.verified_target,
                selected_id,
                selection.spawned,
                context.source_plane,
                context.api_client_store,
                guards.heartbeat,
            ),
        )
        .await;
        match readiness {
            Ok(Ok(())) => {}
            Ok(Err(code)) => {
                let observed = finish_pty_input_before_boundary(
                    context.store,
                    guards.heartbeat,
                    context.injection_id,
                    context.lease_owner,
                    code,
                )
                .await;
                drop(guards.target_guard.take());
                drop(guards.target_stripe.take());
                finish_owned_pty_work(app, guards.owned_work, observed).await;
                return Err(false);
            }
            Err(PtyInputPreBoundaryStop::Expired) => {
                let observed = reject_pty_input_before_boundary(
                    context.store,
                    guards.heartbeat,
                    context.injection_id,
                    C::Expired,
                )
                .await;
                drop(guards.target_guard.take());
                drop(guards.target_stripe.take());
                finish_owned_pty_work(app, guards.owned_work, observed).await;
                return Err(false);
            }
            Err(PtyInputPreBoundaryStop::Shutdown) => {
                guards.heartbeat.finish().await;
                return Err(false);
            }
        }
        if !matches!(
            context
                .store
                .renew_pty_input_lease_offloaded(
                    context.injection_id.to_string(),
                    context.lease_owner.to_string(),
                    chrono::Utc::now(),
                )
                .await,
            Ok(true)
        ) {
            let observed = finish_pty_input_before_boundary(
                context.store,
                guards.heartbeat,
                context.injection_id,
                context.lease_owner,
                pty_input_lease_failure_code(context.expires),
            )
            .await;
            drop(guards.target_guard.take());
            drop(guards.target_stripe.take());
            finish_owned_pty_work(app, guards.owned_work, observed).await;
            return Err(false);
        }

        Ok(selected_id)
    }
    async fn dispatch_writer(
        &self,
        context: &PtyDispatchContext<'_>,
        guards: &mut PtyDispatchGuards<'_>,
        selection: &DispatchSelection<'_>,
    ) -> Result<crate::pty::manager::PtyInputPermit, bool> {
        let app = self.app;

        use crate::phone::types::PtyInputReasonCode as C;
        let permit = match await_pty_input_before_deadline(
            context.expires,
            context.shutdown,
            PtyManager::acquire_input_writer(selection.pty_manager, selection.selected_id),
        )
        .await
        {
            Ok(Ok(permit)) => permit,
            Ok(Err(_)) => {
                let observed = finish_pty_input_before_boundary(
                    context.store,
                    guards.heartbeat,
                    context.injection_id,
                    context.lease_owner,
                    C::SessionRace,
                )
                .await;
                drop(guards.target_guard.take());
                drop(guards.target_stripe.take());
                finish_owned_pty_work(app, guards.owned_work, observed).await;
                return Err(false);
            }
            Err(PtyInputPreBoundaryStop::Expired) => {
                let observed = reject_pty_input_before_boundary(
                    context.store,
                    guards.heartbeat,
                    context.injection_id,
                    C::Expired,
                )
                .await;
                drop(guards.target_guard.take());
                drop(guards.target_stripe.take());
                finish_owned_pty_work(app, guards.owned_work, observed).await;
                return Err(false);
            }
            Err(PtyInputPreBoundaryStop::Shutdown) => {
                guards.heartbeat.finish().await;
                return Err(false);
            }
        };

        Ok(permit)
    }
    async fn dispatch_payload(
        &self,
        context: &PtyDispatchContext<'_>,
        guards: &mut PtyDispatchGuards<'_>,
        selection: &DispatchSelection<'_>,
        write_guards: BoundaryWriteGuards,
    ) -> Result<(Vec<u8>, BoundaryWriteGuards), bool> {
        let BoundaryWriteGuards {
            permit,
            mut final_api_guard,
        } = write_guards;
        let app = self.app;

        use crate::phone::types::{PtyInputPublicStatus as S, PtyInputReasonCode as C};
        let backend_name = match selection.selected.backend_kind {
            SessionBackendKind::LocalProcess => "localProcess",
            SessionBackendKind::ContainerTransport => "containerTransport",
        };
        let payload = match context
            .store
            .begin_pty_actuating_offloaded(
                context.injection_id.to_string(),
                context.lease_owner.to_string(),
                selection.selected.id.clone(),
                backend_name.to_string(),
                chrono::Utc::now(),
            )
            .await
        {
            Ok(payload) => payload,
            Err(crate::api::message_store::MessageStoreError::ActuationCommitAmbiguous) => {
                let terminal = context
                    .store
                    .terminalize_pty_input_offloaded(
                        context.injection_id.to_string(),
                        S::Indeterminate,
                        Some(C::TerminalStoreFailed),
                        chrono::Utc::now(),
                    )
                    .await;
                let observed = terminal.as_ref().is_ok_and(|result| result.terminal);
                drop(final_api_guard.take());
                drop(guards.target_guard.take());
                drop(guards.target_stripe.take());
                drop(permit);
                finish_owned_pty_work(app, guards.owned_work, observed).await;
                if terminal.is_err() {
                    log::error!(
                        "[pty-input] ambiguous boundary reconciliation failed id={} \
                        code=terminal_store_failed",
                        context.injection_id
                    );
                }
                return Err(false);
            }
            Err(_) => {
                reject_pty_input_before_boundary(
                    context.store,
                    guards.heartbeat,
                    context.injection_id,
                    C::StoreTransient,
                )
                .await;
                return Err(false);
            }
        };

        Ok((
            payload,
            BoundaryWriteGuards {
                final_api_guard,
                permit,
            },
        ))
    }
    async fn dispatch_post_authority(
        &self,
        context: &PtyDispatchContext<'_>,
        guards: &mut PtyDispatchGuards<'_>,
        selection: &DispatchSelection<'_>,
        write_guards: BoundaryWriteGuards,
        input: FinalDispatchInput<'_>,
    ) -> Result<BoundaryWriteGuards, bool> {
        let BoundaryWriteGuards {
            permit,
            mut final_api_guard,
        } = write_guards;
        let FinalDispatchInput {
            final_authority,
            payload,
        } = input;
        let app = self.app;

        use crate::phone::types::{PtyInputPublicStatus as S, PtyInputReasonCode as C};
        let post_authority = if let Some(fresh) = final_api_guard.as_ref() {
            self.validate_claimed_api_authority_with_fresh(app, context.claimed, fresh)
                .await
        } else {
            self.validate_claimed_host_authority(app, context.claimed)
                .await
        };
        let payload_matches = context.initial.payload_bytes == Some(payload.len() as u64)
            && context.initial.payload_sha256.as_deref()
                == Some(crate::phone::types::sha256_hex(payload).as_str());
        let post_valid = payload_matches
            && post_authority.is_some_and(|route| {
                route.sender.authority_fingerprint == final_authority.sender.authority_fingerprint
                    && route.target.authority_fingerprint
                        == selection.verified_target.authority_fingerprint
                    && crate::path_identity::same_object(
                        &route.target.replica_identity,
                        &selection.verified_target.replica_identity,
                    )
            })
            && self
                .validate_selected_pty_target(
                    app,
                    context.claimed,
                    selection.verified_target,
                    selection.selected_id,
                    selection.selected.backend_kind,
                )
                .await
                .is_some();
        if !post_valid {
            let terminal = context
                .store
                .terminalize_pty_input_offloaded(
                    context.injection_id.to_string(),
                    S::Indeterminate,
                    Some(C::FinalRevalidationFailed),
                    chrono::Utc::now(),
                )
                .await;
            let observed = terminal.as_ref().is_ok_and(|result| result.terminal);
            drop(final_api_guard.take());
            drop(guards.target_guard.take());
            drop(guards.target_stripe.take());
            drop(permit);
            finish_owned_pty_work(app, guards.owned_work, observed).await;
            if terminal.is_err() {
                log::error!(
                    "[pty-input] post-boundary terminalization failed id={} \
                    code=terminal_store_failed",
                    context.injection_id
                );
            }
            return Err(false);
        }

        Ok(BoundaryWriteGuards {
            final_api_guard,
            permit,
        })
    }
    async fn dispatch_boundary(
        &self,
        context: &PtyDispatchContext<'_>,
        guards: &mut PtyDispatchGuards<'_>,
        selection: &DispatchSelection<'_>,
        write_guards: BoundaryWriteGuards,
        input: FinalDispatchInput<'_>,
    ) -> bool {
        let (authority, write_guards) = match self
            .dispatch_boundary_authority(context, guards, selection, write_guards)
            .await
        {
            Ok(value) => value,
            Err(failed) => return failed,
        };
        let (boundary_write, write_guards) = match self
            .dispatch_boundary_write(context, guards, selection, write_guards, input, &authority)
            .await
        {
            Ok(value) => value,
            Err(failed) => return failed,
        };
        self.dispatch_boundary_result(
            context,
            guards,
            selection,
            write_guards,
            input.payload,
            boundary_write,
        )
        .await
    }
}
