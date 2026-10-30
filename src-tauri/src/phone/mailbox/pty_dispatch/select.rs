use super::{
    BoundaryAuthority, DispatchSelection, FinalDispatchInput, PtyDispatchContext,
    PtyDispatchGuards, PtyDispatcher, SelectedPtyTarget, SelectionCandidates, SpawnAttempt,
};
use crate::config::settings::SettingsState;
use crate::config::teams::VerifiedPtyInputIdentity;
use crate::phone::events::PtyInputEvents;
#[cfg(test)]
use crate::phone::mailbox::pty_claim::MailboxTestHooks;
use crate::phone::mailbox::pty_claim::{
    await_pty_input_before_deadline, finish_pty_input_before_boundary,
    reacquire_pty_dispatch_target, reject_pty_input_before_boundary, PtyInputPreBoundaryStop,
    PtyReacquireGoal,
};
use crate::phone::mailbox::settle::{
    finish_owned_pty_work, session_has_current_pty_submission_provenance, PtySpawnStep,
};
use crate::phone::types::{PtyInputPublicStatus as S, PtyInputReasonCode as C};
use crate::pty::backend::SessionBackendKind;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::session::SessionInfo;
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
use tauri::Manager;
use uuid::Uuid;

/// Mutable guards retained through the final validation and write boundary.
pub(super) struct BoundaryWriteGuards<'a> {
    pub(super) permit: &'a mut Option<crate::pty::manager::PtyInputPermit>,
    pub(super) final_api_guard: &'a mut Option<crate::api::auth::ApiClientFreshGuard>,
}

impl<R: tauri::Runtime> PtyDispatcher<'_, tauri::AppHandle<R>> {
    pub(super) async fn select_pty_dispatch_session(
        &self,
        context: &PtyDispatchContext<'_>,
        mut guards: PtyDispatchGuards<'_>,
    ) -> Result<SelectedPtyTarget, bool> {
        let app = self.app;

        use crate::phone::types::PtyInputReasonCode as C;

        let mut prepared_replacement = None;
        loop {
            let verified_route = self.dispatch_selection_route(context, &mut guards).await?;
            let verified_sender = verified_route.sender;
            let verified_target = verified_route.target;

            let session_manager = app
                .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
                .inner()
                .clone();
            let pty_manager = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
            let SelectionCandidates {
                matching,
                eligible,
                saw_supported_busy,
                saw_live_unsupported,
                saw_live_inconsistent,
                saw_live_temporary,
            } = super::context::dispatch_selection_candidates(
                context.claimed,
                &session_manager,
                &pty_manager,
                &verified_target,
                || async { app.state::<SettingsState>().read().await.clone() },
            )
            .await;
            let (selected, spawned) = if let Some(selected) = eligible.into_iter().next() {
                (selected, false)
            } else if saw_supported_busy {
                let observed = reject_pty_input_before_boundary(
                    context.store,
                    guards.heartbeat,
                    context.injection_id,
                    C::Busy,
                )
                .await;
                drop(guards.target_guard.take());
                drop(guards.target_stripe.take());
                finish_owned_pty_work(app, guards.owned_work, observed).await;
                return Err(false);
            } else if saw_live_temporary {
                let observed = reject_pty_input_before_boundary(
                    context.store,
                    guards.heartbeat,
                    context.injection_id,
                    C::NonpersistentLiveSession,
                )
                .await;
                drop(guards.target_guard.take());
                drop(guards.target_stripe.take());
                finish_owned_pty_work(app, guards.owned_work, observed).await;
                return Err(false);
            } else if saw_live_inconsistent {
                if context.claimed.attempt < 5 {
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
                } else {
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
                }
                return Err(false);
            } else if saw_live_unsupported {
                let observed = reject_pty_input_before_boundary(
                    context.store,
                    guards.heartbeat,
                    context.injection_id,
                    C::UnsupportedSession,
                )
                .await;
                drop(guards.target_guard.take());
                drop(guards.target_stripe.take());
                finish_owned_pty_work(app, guards.owned_work, observed).await;
                return Err(false);
            } else {
                let spawn = self
                    .dispatch_selection_spawn(
                        context,
                        &mut guards,
                        &matching,
                        &verified_sender,
                        &verified_target,
                        &mut prepared_replacement,
                    )
                    .await?;
                match self
                    .dispatch_selection_spawn_result(
                        context,
                        &mut guards,
                        spawn,
                        &verified_target,
                        &mut prepared_replacement,
                    )
                    .await?
                {
                    Some(selected) => selected,
                    None => continue,
                }
            };
            let selected_id = Uuid::parse_str(&selected.id).ok();
            if guards.owned_work.is_none()
                && selected_id.is_some_and(|id| {
                    crate::config::sessions_persistence::unconfirmed_replacement_work_waits(
                        &context.claimed.target_fqn,
                        id,
                    )
                })
            {
                drop(guards.target_guard.take());
                drop(guards.target_stripe.take());
                let Some(id) = selected_id else {
                    guards.heartbeat.finish().await;
                    return Err(false);
                };
                let Some((stripe, exact)) = reacquire_pty_dispatch_target(
                    app,
                    (context.store, context.initial, context.claimed),
                    (
                        context.target_gate,
                        &verified_target,
                        PtyReacquireGoal::Published(id),
                    ),
                    (context.expires, context.shutdown),
                    guards.heartbeat,
                )
                .await
                else {
                    return Err(false);
                };
                *guards.target_stripe = Some(stripe);
                *guards.target_guard = Some(exact);
                continue;
            }
            return Ok((
                selected,
                spawned,
                verified_target,
                session_manager,
                pty_manager,
            ));
        }
    }
    async fn dispatch_selection_route(
        &self,
        context: &PtyDispatchContext<'_>,
        guards: &mut PtyDispatchGuards<'_>,
    ) -> Result<crate::config::teams::VerifiedPtyInputRoute, bool> {
        let app = self.app;

        use crate::phone::types::PtyInputReasonCode as C;
        if app
            .try_state::<Arc<crate::RestoreInProgress>>()
            .is_some_and(|flag| flag.0.load(std::sync::atomic::Ordering::SeqCst))
        {
            let observed = finish_pty_input_before_boundary(
                context.store,
                guards.heartbeat,
                context.injection_id,
                context.lease_owner,
                C::RestoreInProgress,
            )
            .await;
            drop(guards.target_guard.take());
            drop(guards.target_stripe.take());
            finish_owned_pty_work(app, guards.owned_work, observed).await;
            return Err(false);
        }
        if app
            .try_state::<Arc<crate::session::purge_guard::PurgeGuard>>()
            .is_some_and(|guard| guard.blocks_agent(&context.claimed.target_fqn))
        {
            let observed = finish_pty_input_before_boundary(
                context.store,
                guards.heartbeat,
                context.injection_id,
                context.lease_owner,
                C::PurgeInProgress,
            )
            .await;
            drop(guards.target_guard.take());
            drop(guards.target_stripe.take());
            finish_owned_pty_work(app, guards.owned_work, observed).await;
            return Err(false);
        }

        let verified_route =
            if context.source_plane == crate::phone::types::PtyInputSourcePlane::HostCli {
                self.validate_claimed_host_authority(app, context.claimed)
                    .await
            } else {
                match self
                    .validate_claimed_api_authority(app, context.claimed, context.api_client_store)
                    .await
                {
                    Ok(route) => route,
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
                        finish_owned_pty_work(app, guards.owned_work, observed).await;
                        return Err(false);
                    }
                }
            };
        let Some(verified_route) = verified_route else {
            let code = if context.source_plane == crate::phone::types::PtyInputSourcePlane::HostCli
            {
                C::AuthorityChanged
            } else {
                C::ApiBindingMismatch
            };
            let observed = reject_pty_input_before_boundary(
                context.store,
                guards.heartbeat,
                context.injection_id,
                code,
            )
            .await;
            drop(guards.target_guard.take());
            drop(guards.target_stripe.take());
            finish_owned_pty_work(app, guards.owned_work, observed).await;
            return Err(false);
        };
        if verified_route.target.canonical_fqn != *context.target {
            let observed = reject_pty_input_before_boundary(
                context.store,
                guards.heartbeat,
                context.injection_id,
                C::AuthorityChanged,
            )
            .await;
            drop(guards.target_guard.take());
            drop(guards.target_stripe.take());
            finish_owned_pty_work(app, guards.owned_work, observed).await;
            return Err(false);
        }
        Ok(verified_route)
    }
    async fn dispatch_selection_spawn(
        &self,
        context: &PtyDispatchContext<'_>,
        guards: &mut PtyDispatchGuards<'_>,
        matching: &[SessionInfo],
        verified_sender: &VerifiedPtyInputIdentity,
        verified_target: &VerifiedPtyInputIdentity,
        prepared_replacement: &mut Option<crate::session::selection::PreparedOwnedReplacement>,
    ) -> Result<SpawnAttempt, bool> {
        let app = self.app;

        #[cfg(test)]
        let fixture_control = self
            .test_hooks
            .as_ref()
            .and_then(MailboxTestHooks::replacement_fixture_control)
            .filter(|control| {
                !control
                    .ticket_only
                    .load(std::sync::atomic::Ordering::Acquire)
            });
        #[cfg(test)]
        let fixture_lease = if fixture_control.is_some() {
            let (Some(exact), Some(stripe)) =
                (guards.target_guard.take(), guards.target_stripe.take())
            else {
                guards.heartbeat.finish().await;
                return Err(false);
            };
            Some(Arc::new(crate::commands::session::FixturePtyCutLease {
                gate: context.target_gate.clone(),
                target: context.target.clone(),
                exact,
                stripe,
            }))
        } else {
            None
        };
        let spawn = {
            #[cfg(test)]
            let (stripe, exact) = match fixture_lease.as_ref() {
                Some(lease) => (Some(&lease.stripe), Some(&lease.exact)),
                None => (guards.target_stripe.as_ref(), guards.target_guard.as_ref()),
            };
            #[cfg(not(test))]
            let (stripe, exact) = (guards.target_stripe.as_ref(), guards.target_guard.as_ref());
            let (Some(stripe), Some(exact)) = (stripe, exact) else {
                guards.heartbeat.finish().await;
                return Err(false);
            };
            let target_ownership =
                match context
                    .target_gate
                    .target_ownership(context.target, stripe, exact)
                {
                    Ok(proof) => proof,
                    Err(_) => {
                        guards.heartbeat.finish().await;
                        return Err(false);
                    }
                };
            await_pty_input_before_deadline(
                context.expires,
                context.shutdown,
                self.spawn_pty_input_target(
                    app,
                    context.claimed,
                    matching,
                    verified_sender,
                    verified_target,
                    context.source_plane,
                    context.api_client_store,
                    &target_ownership,
                    context.initial,
                    prepared_replacement.take(),
                    guards.source_executor,
                    #[cfg(test)]
                    fixture_lease.as_ref(),
                ),
            )
            .await
        };
        #[cfg(test)]
        if let Some(lease) = fixture_lease {
            let restored = Arc::try_unwrap(lease);
            let worker_still_holds_clone = restored.is_err();
            let receipt_missing = fixture_control.as_ref().is_some_and(|control| {
                control.submitted.load(std::sync::atomic::Ordering::Acquire)
                    && !control.received.load(std::sync::atomic::Ordering::Acquire)
            });
            match restored {
                Ok(lease) if !receipt_missing => {
                    *guards.target_guard = Some(lease.exact);
                    *guards.target_stripe = Some(lease.stripe);
                }
                other => {
                    drop(other);
                    if let Some(control) = fixture_control.as_ref() {
                        control.restoration_shared.store(
                            worker_still_holds_clone,
                            std::sync::atomic::Ordering::Release,
                        );
                        control
                            .restoration_failed
                            .store(true, std::sync::atomic::Ordering::Release);
                    }
                    // This cut precedes every Retained/timeout/error
                    // handoff, even if the owner already has a stop or
                    // rollback receipt. A submitted job without actual
                    // receive has no source-side work authority.
                    guards.heartbeat.finish().await;
                    return Err(false);
                }
            }
        }
        Ok(spawn)
    }
    async fn dispatch_selection_spawn_result(
        &self,
        context: &PtyDispatchContext<'_>,
        guards: &mut PtyDispatchGuards<'_>,
        spawn: SpawnAttempt,
        verified_target: &VerifiedPtyInputIdentity,
        prepared_replacement: &mut Option<crate::session::selection::PreparedOwnedReplacement>,
    ) -> Result<Option<(SessionInfo, bool)>, bool> {
        let app = self.app;

        use crate::phone::types::PtyInputReasonCode as C;
        let selected = match spawn {
            Ok(Ok(PtySpawnStep::Created(session))) => (session, true),
            Ok(Ok(PtySpawnStep::OwnedCreated(session, continuation))) => {
                let Some(effect) = continuation
                    .start_effect(crate::session::selection::LifecycleEffectSlot::WorkTransfer)
                else {
                    drop(guards.target_guard.take());
                    drop(guards.target_stripe.take());
                    guards.heartbeat.finish().await;
                    return Err(false);
                };
                *guards.owned_work = Some((continuation, effect));
                (session, true)
            }
            Ok(Ok(PtySpawnStep::Retained {
                continuation,
                failed,
            })) => {
                drop(guards.target_guard.take());
                drop(guards.target_stripe.take());
                let stop = continuation.source_stop();
                let rolled_back = continuation.precommit_rollback_completed();
                if stop.is_none() && !rolled_back {
                    guards.heartbeat.finish().await;
                    return Err(failed);
                }
                let effect = continuation
                    .start_effect(crate::session::selection::LifecycleEffectSlot::WorkTransfer);
                use crate::session::selection::OwnedSourceStop as Stop;
                let observed = match stop {
                    Some(Stop::Terminal) => {
                        guards.heartbeat.finish().await;
                        true
                    }
                    reason => {
                        let code = match reason {
                            Some(Stop::AuthorityChanged) => C::AuthorityChanged,
                            Some(Stop::StoreTransient) => C::StoreTransient,
                            Some(Stop::Expired) => C::Expired,
                            Some(Stop::LeaseLost) => C::LeaseLost,
                            Some(Stop::Terminal) => unreachable!(),
                            None => C::SpawnFailedSafe,
                        };
                        finish_pty_input_before_boundary(
                            context.store,
                            guards.heartbeat,
                            context.injection_id,
                            context.lease_owner,
                            code,
                        )
                        .await
                    }
                };
                if let Some(effect) = effect {
                    let manager = app
                        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
                        .read()
                        .await
                        .clone();
                    let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
                    if rolled_back {
                        continuation
                            .finish_rolled_back_work(&manager, &pty, effect, observed)
                            .await;
                    } else {
                        continuation
                            .finish_observed_work_transfer(&manager, &pty, effect, observed)
                            .await;
                    }
                }
                return Err(failed);
            }
            Ok(Ok(PtySpawnStep::RetainedUnowned)) => {
                drop(guards.target_guard.take());
                drop(guards.target_stripe.take());
                guards.heartbeat.finish().await;
                return Err(false);
            }
            Ok(Ok(PtySpawnStep::NeedReacquire(prepared))) => {
                *prepared_replacement = prepared;
                // The spawn block ended the proof borrow above.
                drop(guards.target_guard.take());
                drop(guards.target_stripe.take());
                let Some((stripe, exact)) = reacquire_pty_dispatch_target(
                    app,
                    (context.store, context.initial, context.claimed),
                    (
                        context.target_gate,
                        verified_target,
                        PtyReacquireGoal::Spawn,
                    ),
                    (context.expires, context.shutdown),
                    guards.heartbeat,
                )
                .await
                else {
                    return Err(false);
                };
                *guards.target_stripe = Some(stripe);
                *guards.target_guard = Some(exact);
                return Ok(None);
            }
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
        };
        Ok(Some(selected))
    }

    pub(super) async fn dispatch_boundary_authority(
        &self,
        context: &PtyDispatchContext<'_>,
        guards: &mut PtyDispatchGuards<'_>,
        selection: &DispatchSelection<'_>,
        permit: &mut Option<crate::pty::manager::PtyInputPermit>,
        final_api_guard: &mut Option<crate::api::auth::ApiClientFreshGuard>,
    ) -> Result<BoundaryAuthority, bool> {
        let app = self.app;

        use crate::phone::types::{PtyInputPublicStatus as S, PtyInputReasonCode as C};
        let manager = {
            let guard = selection.session_manager.read().await;
            guard.clone()
        };
        let idle_detector = app
            .state::<Arc<crate::pty::idle_detector::IdleDetector>>()
            .inner()
            .clone();
        let authority_id = match crate::phone::types::parse_canonical_uuid_v4(
            &context.claimed.authority_session_id,
        ) {
            Ok(id) => id,
            Err(_) => {
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
                drop(permit.take());
                finish_owned_pty_work(app, guards.owned_work, observed).await;
                if terminal.is_err() {
                    log::error!(
                        "[pty-input] boundary terminalization failed id={} \
                        code=terminal_store_failed",
                        context.injection_id
                    );
                }
                return Err(false);
            }
        };
        let authority_backend = match context.source_plane {
            crate::phone::types::PtyInputSourcePlane::HostCli => SessionBackendKind::LocalProcess,
            crate::phone::types::PtyInputSourcePlane::ContainerApi => {
                SessionBackendKind::ContainerTransport
            }
        };
        let authority_container_backend = selection
            .pty_manager
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .container_backend();
        let authority_route =
            match PtyManager::authority_route_proof(selection.pty_manager, authority_id) {
                Ok(proof) => proof,
                Err(_) => {
                    let terminal = context.store.terminalize_pty_input(
                        context.injection_id,
                        S::Indeterminate,
                        Some(C::FinalRevalidationFailed),
                        chrono::Utc::now(),
                    );
                    let observed = terminal.as_ref().is_ok_and(|result| result.terminal);
                    drop(final_api_guard.take());
                    drop(guards.target_guard.take());
                    drop(guards.target_stripe.take());
                    drop(permit.take());
                    finish_owned_pty_work(app, guards.owned_work, observed).await;
                    if terminal.is_err() {
                        log::error!(
                            "[pty-input] boundary terminalization failed id={} \
                        code=terminal_store_failed",
                            context.injection_id
                        );
                    }
                    return Err(false);
                }
            };
        let expected_api_binding = final_api_guard.as_ref().and_then(|fresh| {
            let root =
                crate::path_identity::verify_directory(Path::new(&fresh.client.bound_root)).ok()?;
            Some((
                fresh.client.client_id.clone(),
                fresh.client.credential_generation.clone()?,
                fresh.client.bound_session_id.clone()?,
                root.object_id,
                fresh.presented_token_hash.clone(),
            ))
        });
        if context.source_plane == crate::phone::types::PtyInputSourcePlane::ContainerApi
            && expected_api_binding.is_none()
        {
            let terminal = context.store.terminalize_pty_input(
                context.injection_id,
                S::Indeterminate,
                Some(C::FinalRevalidationFailed),
                chrono::Utc::now(),
            );
            let observed = terminal.as_ref().is_ok_and(|result| result.terminal);
            drop(final_api_guard.take());
            drop(guards.target_guard.take());
            drop(guards.target_stripe.take());
            drop(permit.take());
            finish_owned_pty_work(app, guards.owned_work, observed).await;
            if terminal.is_err() {
                log::error!(
                    "[pty-input] boundary terminalization failed id={} code=terminal_store_failed",
                    context.injection_id
                );
            }
            return Err(false);
        }

        Ok(BoundaryAuthority {
            manager,
            idle_detector,
            authority_id,
            authority_backend,
            authority_container_backend,
            authority_route,
            expected_api_binding,
        })
    }
    pub(super) async fn dispatch_boundary_write(
        &self,
        context: &PtyDispatchContext<'_>,
        guards: &mut PtyDispatchGuards<'_>,
        selection: &DispatchSelection<'_>,
        write_guards: BoundaryWriteGuards<'_>,
        input: FinalDispatchInput<'_>,
        authority: &BoundaryAuthority,
    ) -> Result<Result<bool, bool>, bool> {
        let BoundaryWriteGuards {
            permit,
            final_api_guard,
        } = write_guards;
        let FinalDispatchInput {
            final_authority,
            payload,
        } = input;
        let app = self.app;

        let BoundaryAuthority {
            manager,
            idle_detector,
            authority_id,
            authority_backend,
            authority_container_backend,
            authority_route,
            expected_api_binding,
            ..
        } = authority;
        use crate::phone::types::{PtyInputPublicStatus as S, PtyInputReasonCode as C};
        let boundary_settings = app.state::<SettingsState>().inner().clone();
        let owned_route = guards.owned_work.as_ref().and_then(|_| {
            PtyManager::snapshot_route_proof(selection.pty_manager, selection.selected_id).ok()
        });
        let boundary_carrier = crate::session::manager::acquire_carrier_mutation_permit().await;
        let owned_token = guards
            .owned_work
            .as_ref()
            .and_then(|(continuation, _)| continuation.launch_token());
        let owned_current = match guards.owned_work.as_ref() {
            Some((continuation, _)) => {
                manager
                    .with_current_carrier_snapshot(&boundary_carrier, |current| {
                        owned_token.is_some()
                            && continuation.current_completion_region(
                                &boundary_carrier,
                                current,
                                selection.pty_manager,
                            )
                    })
                    .await
            }
            None => true,
        };
        if !owned_current {
            drop(boundary_carrier);
            let observed = context
                .store
                .terminalize_pty_input_offloaded(
                    context.injection_id.to_string(),
                    S::Indeterminate,
                    Some(C::FinalRevalidationFailed),
                    chrono::Utc::now(),
                )
                .await
                .is_ok_and(|result| result.terminal);
            drop(final_api_guard.take());
            drop(guards.target_guard.take());
            drop(guards.target_stripe.take());
            drop(permit.take());
            finish_owned_pty_work(app, guards.owned_work, observed).await;
            return Err(false);
        }
        let boundary_write = match manager
            .prepare_pty_input_boundary_with_carrier_permit(
                &boundary_carrier,
                selection.selected_id,
                selection.verified_target,
                selection.selected.backend_kind,
                *authority_id,
                &final_authority.sender,
                *authority_backend,
                authority_route,
                permit.as_ref().expect("dispatch input permit"),
                idle_detector,
                &boundary_settings,
                |session, settings| {
                    owned_token.is_none_or(|token| session.token == token)
                        && guards.owned_work.as_ref().is_none_or(|(continuation, _)| {
                            owned_route.as_ref().is_some_and(|proof| {
                                continuation.matches_captured_launch_route(proof)
                            })
                        })
                        && session_has_current_pty_submission_provenance(
                            &SessionInfo::from(session),
                            settings,
                        )
                },
                || {
                    if context.expires <= chrono::Utc::now()
                        || app
                            .try_state::<Arc<crate::RestoreInProgress>>()
                            .is_some_and(|flag| flag.0.load(std::sync::atomic::Ordering::SeqCst))
                        || app
                            .try_state::<Arc<crate::session::purge_guard::PurgeGuard>>()
                            .is_some_and(|guard| guard.blocks_agent(&context.claimed.target_fqn))
                    {
                        return false;
                    }
                    match (&context.source_plane, expected_api_binding.as_ref()) {
                        (crate::phone::types::PtyInputSourcePlane::HostCli, None) => true,
                        (
                            crate::phone::types::PtyInputSourcePlane::ContainerApi,
                            Some((
                                client_id,
                                generation,
                                bound_session_id,
                                root_object,
                                token_hash,
                            )),
                        ) => {
                            let binding =
                                authority_container_backend.credential_binding(*authority_id);
                            binding.is_some_and(|binding| {
                                binding.client_id == *client_id
                                    && binding.credential_generation == *generation
                                    && binding.bound_session_id == *bound_session_id
                                    && binding.bound_root_object_id == *root_object
                                    && crate::api::auth::constant_time_eq(
                                        &binding.credential_token_hash,
                                        token_hash,
                                    )
                            })
                        }
                        _ => false,
                    }
                },
            )
            .await
        {
            Ok(guard) => {
                drop(boundary_carrier);
                Ok(crate::pty::inject::write_exact_agent_input_first(
                    guard, payload,
                ))
            }
            Err(_) => {
                drop(boundary_carrier);
                // The guard-bearing Result ends in this match. Only the
                // observed terminal boolean survives to the work handoff await.
                let terminal = context.store.terminalize_pty_input(
                    context.injection_id,
                    S::Indeterminate,
                    Some(C::FinalRevalidationFailed),
                    chrono::Utc::now(),
                );
                if terminal.is_err() {
                    log::error!(
                        "[pty-input] boundary terminalization failed id={} \
                        code=terminal_store_failed",
                        context.injection_id
                    );
                }
                Err(terminal.as_ref().is_ok_and(|result| result.terminal))
            }
        };

        Ok(boundary_write)
    }
    pub(super) async fn dispatch_boundary_result(
        &self,
        context: &PtyDispatchContext<'_>,
        guards: &mut PtyDispatchGuards<'_>,
        selection: &DispatchSelection<'_>,
        write_guards: BoundaryWriteGuards<'_>,
        payload: &[u8],
        boundary_write: Result<bool, bool>,
    ) -> bool {
        let BoundaryWriteGuards {
            permit,
            final_api_guard,
        } = write_guards;
        let app = self.app;

        let text_write_succeeded = match boundary_write {
            Ok(written) => written,
            Err(observed) => {
                drop(final_api_guard.take());
                drop(guards.target_guard.take());
                drop(guards.target_stripe.take());
                drop(permit.take());
                finish_owned_pty_work(app, guards.owned_work, observed).await;
                return false;
            }
        };
        // Keep the fresh registry lock through the synchronous first-write
        // boundary so a concurrent revocation linearizes before or after it,
        // never between the final authority check and the text write.
        drop(final_api_guard.take());
        let outcome = crate::pty::inject::submit_exact_agent_input_with_permit(
            permit.as_ref().expect("dispatch input permit"),
            text_write_succeeded,
        )
        .await;
        let (status, reason) = self
            .pty_submit_terminal_status(payload, selection.selected_id, outcome)
            .await;
        let result = match context
            .store
            .terminalize_pty_input_offloaded(
                context.injection_id.to_string(),
                status,
                reason,
                chrono::Utc::now(),
            )
            .await
        {
            Ok(result) => result,
            Err(_) => {
                log::error!(
                    "[pty-input] terminal persistence failed id={} code=terminal_store_failed",
                    context.injection_id
                );
                return false;
            }
        };
        crate::api::audit::record_pty_input_result("terminal", &result);
        drop(guards.target_guard.take());
        drop(guards.target_stripe.take());
        drop(permit.take());
        finish_owned_pty_work(app, guards.owned_work, result.terminal).await;
        if !app.pty_input_status(&result) {
            log::warn!(
                "[pty-input] status event failed id={} code=boundary_metadata_failed",
                context.injection_id
            );
        }

        false
    }
    pub(super) async fn pty_submit_terminal_status(
        &self,
        payload: &[u8],
        selected_id: Uuid,
        outcome: crate::pty::inject::AgentSubmitOutcome,
    ) -> (S, Option<C>) {
        let app = self.app;

        match outcome {
            crate::pty::inject::AgentSubmitOutcome::TextWriteFailed => {
                (S::Indeterminate, Some(C::TextWriteFailed))
            }
            crate::pty::inject::AgentSubmitOutcome::RequiredEnterFailed => {
                (S::Indeterminate, Some(C::RequiredEnterFailed))
            }
            crate::pty::inject::AgentSubmitOutcome::Submitted {
                redundant_enter_failed,
            } => {
                let metadata = if payload == b"/clear" {
                    crate::commands::pty::stamp_fresh_boundary_to_session(app, selected_id).await
                } else if payload == b"/compact" {
                    crate::commands::pty::BoundaryMetadataOutcome::Unchanged
                } else {
                    crate::commands::pty::note_post_boundary_content_to_session(app, selected_id)
                        .await
                };
                let reason = if metadata == crate::commands::pty::BoundaryMetadataOutcome::Failed {
                    Some(C::BoundaryMetadataFailed)
                } else {
                    redundant_enter_failed.then_some(C::RedundantEnterFailed)
                };
                (S::Injected, reason)
            }
        }
    }
}
