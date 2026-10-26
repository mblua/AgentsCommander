mod outcome;
mod select;

use super::poller::MailboxPoller;
#[cfg(test)]
use super::pty_claim::MailboxTestHooks;
use super::pty_claim::{
    await_pty_input_before_deadline, finish_pty_input_before_boundary,
    pty_input_lease_failure_code, refresh_pty_claim_for_resume, reject_pty_input_before_boundary,
    PtyClaimRefresh, PtyInputPreBoundaryStop,
};
use super::settle::{
    finish_owned_pty_work, session_has_current_pty_submission_provenance, OwnedPtyWorkTransfer,
};
use crate::config::settings::SettingsState;
use crate::phone::events::PtyInputEvents;
use crate::pty::backend::SessionBackendKind;
use crate::pty::manager::PtyManager;
use crate::session::session::SessionInfo;
use select::{PtyDispatchContext, PtyDispatchGuards};
use std::path::Path;
use std::sync::Arc;
use tauri::Manager;
use uuid::Uuid;

impl MailboxPoller {
    pub(crate) async fn dispatch_pty_input_operation_core<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        state: &crate::api::message_store::MessageStoreState,
        injection_id: &str,
        source_plane: crate::phone::types::PtyInputSourcePlane,
        api_client_store: Option<&Arc<crate::api::auth::ApiClientStore>>,
    ) {
        use crate::phone::types::{PtyInputPublicStatus as S, PtyInputReasonCode as C};
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
        let failed = async {
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
            // Startup restore must finish before this dispatch owns target guards.
            // No claim or payload has been consumed at this point.
            if let Some(coordinator) =
                app.try_state::<crate::session::selection::SelectionCoordinator>()
            {
                if coordinator.wait_for_restore_release().await.is_err() {
                    return false;
                }
            }
            let mut target_stripe = Some(match target_gate.try_target_lock(&target) {
                Ok(Some(lock)) => lock,
                _ => return false,
            });
            let initial_expires =
                match initial.expires_at.as_deref().and_then(|value| {
                    crate::phone::types::parse_canonical_pty_timestamp(value).ok()
                }) {
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
                        return false;
                    }
                };
            let mut target_guard = Some(
                match await_pty_input_before_deadline(
                    initial_expires,
                    &shutdown,
                    target_gate.acquire_exact(&target),
                )
                .await
                {
                    Ok(guard) => guard,
                    Err(PtyInputPreBoundaryStop::Shutdown) => return false,
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
                        return false;
                    }
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
            let lease_owner = Uuid::new_v4().to_string();
            let claimed = match store
                .claim_pty_input_offloaded(
                    source_plane,
                    Some(injection_id.to_string()),
                    lease_owner.clone(),
                    chrono::Utc::now(),
                )
                .await
            {
                Ok(Some(claimed)) => claimed,
                _ => return false,
            };
            let expires =
                match crate::phone::types::parse_canonical_pty_timestamp(&claimed.expires_at) {
                    Ok(expires) => expires,
                    Err(_) => {
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
                                "[pty-input] corrupt-expiry rejection failed id={} \
                        code=terminal_store_failed",
                                injection_id
                            );
                        }
                        return false;
                    }
                };
            let mut heartbeat = crate::api::message_store::PreparationHeartbeatGuard::start(
                Arc::clone(&store),
                injection_id.to_string(),
                lease_owner.clone(),
                expires,
            );
            if expires <= chrono::Utc::now() {
                reject_pty_input_before_boundary(&store, &mut heartbeat, injection_id, C::Expired)
                    .await;
                return false;
            }
            match refresh_pty_claim_for_resume(&store, &initial, &claimed).await {
                Ok(PtyClaimRefresh::Current) => {}
                Ok(PtyClaimRefresh::Terminal) => {
                    heartbeat.finish().await;
                    return false;
                }
                Err(code) => {
                    finish_pty_input_before_boundary(
                        &store,
                        &mut heartbeat,
                        injection_id,
                        &lease_owner,
                        code,
                    )
                    .await;
                    return false;
                }
            }
            let mut owned_work: Option<OwnedPtyWorkTransfer> = None;
            let (selected, spawned, verified_target, session_manager, pty_manager) = match self
                .select_pty_dispatch_session(
                    app,
                    &PtyDispatchContext {
                        store: &store,
                        initial: &initial,
                        claimed: &claimed,
                        injection_id,
                        lease_owner: &lease_owner,
                        source_plane,
                        api_client_store,
                        target_gate: &target_gate,
                        target: &target,
                        expires,
                        shutdown: &shutdown,
                    },
                    PtyDispatchGuards {
                        target_guard: &mut target_guard,
                        target_stripe: &mut target_stripe,
                        heartbeat: &mut heartbeat,
                        owned_work: &mut owned_work,
                        source_executor: &mut source_executor,
                    },
                )
                .await
            {
                Ok(selected) => selected,
                Err(failed) => return failed,
            };
            let selected_id = match Uuid::parse_str(&selected.id) {
                Ok(id) => id,
                Err(_) => {
                    let observed = reject_pty_input_before_boundary(
                        &store,
                        &mut heartbeat,
                        injection_id,
                        C::InconsistentSession,
                    )
                    .await;
                    drop(target_guard.take());
                    drop(target_stripe.take());
                    finish_owned_pty_work(app, &mut owned_work, observed).await;
                    return false;
                }
            };
            let readiness = await_pty_input_before_deadline(
                expires,
                &shutdown,
                self.wait_for_pty_input_ready(
                    app,
                    &claimed,
                    &verified_target,
                    selected_id,
                    spawned,
                    source_plane,
                    api_client_store,
                    &heartbeat,
                ),
            )
            .await;
            match readiness {
                Ok(Ok(())) => {}
                Ok(Err(code)) => {
                    let observed = finish_pty_input_before_boundary(
                        &store,
                        &mut heartbeat,
                        injection_id,
                        &lease_owner,
                        code,
                    )
                    .await;
                    drop(target_guard.take());
                    drop(target_stripe.take());
                    finish_owned_pty_work(app, &mut owned_work, observed).await;
                    return false;
                }
                Err(PtyInputPreBoundaryStop::Expired) => {
                    let observed = reject_pty_input_before_boundary(
                        &store,
                        &mut heartbeat,
                        injection_id,
                        C::Expired,
                    )
                    .await;
                    drop(target_guard.take());
                    drop(target_stripe.take());
                    finish_owned_pty_work(app, &mut owned_work, observed).await;
                    return false;
                }
                Err(PtyInputPreBoundaryStop::Shutdown) => {
                    heartbeat.finish().await;
                    return false;
                }
            }
            if !matches!(
                store
                    .renew_pty_input_lease_offloaded(
                        injection_id.to_string(),
                        lease_owner.clone(),
                        chrono::Utc::now(),
                    )
                    .await,
                Ok(true)
            ) {
                let observed = finish_pty_input_before_boundary(
                    &store,
                    &mut heartbeat,
                    injection_id,
                    &lease_owner,
                    pty_input_lease_failure_code(expires),
                )
                .await;
                drop(target_guard.take());
                drop(target_stripe.take());
                finish_owned_pty_work(app, &mut owned_work, observed).await;
                return false;
            }
            let permit = match await_pty_input_before_deadline(
                expires,
                &shutdown,
                PtyManager::acquire_input_writer(&pty_manager, selected_id),
            )
            .await
            {
                Ok(Ok(permit)) => permit,
                Ok(Err(_)) => {
                    let observed = finish_pty_input_before_boundary(
                        &store,
                        &mut heartbeat,
                        injection_id,
                        &lease_owner,
                        C::SessionRace,
                    )
                    .await;
                    drop(target_guard.take());
                    drop(target_stripe.take());
                    finish_owned_pty_work(app, &mut owned_work, observed).await;
                    return false;
                }
                Err(PtyInputPreBoundaryStop::Expired) => {
                    let observed = reject_pty_input_before_boundary(
                        &store,
                        &mut heartbeat,
                        injection_id,
                        C::Expired,
                    )
                    .await;
                    drop(target_guard.take());
                    drop(target_stripe.take());
                    finish_owned_pty_work(app, &mut owned_work, observed).await;
                    return false;
                }
                Err(PtyInputPreBoundaryStop::Shutdown) => {
                    heartbeat.finish().await;
                    return false;
                }
            };
            let final_api_guard =
                if source_plane == crate::phone::types::PtyInputSourcePlane::ContainerApi {
                    let Some(client_store) = api_client_store else {
                        let observed = reject_pty_input_before_boundary(
                            &store,
                            &mut heartbeat,
                            injection_id,
                            C::AuthorityChanged,
                        )
                        .await;
                        drop(target_guard.take());
                        drop(target_stripe.take());
                        drop(permit);
                        finish_owned_pty_work(app, &mut owned_work, observed).await;
                        return false;
                    };
                    let (Some(client_id), Some(generation)) = (
                        claimed.authority_client_id.as_deref(),
                        claimed.authority_client_generation.as_deref(),
                    ) else {
                        let observed = reject_pty_input_before_boundary(
                            &store,
                            &mut heartbeat,
                            injection_id,
                            C::AuthorityChanged,
                        )
                        .await;
                        drop(target_guard.take());
                        drop(target_stripe.take());
                        drop(permit);
                        finish_owned_pty_work(app, &mut owned_work, observed).await;
                        return false;
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
                                &store,
                                &mut heartbeat,
                                injection_id,
                                C::AuthorityChanged,
                            )
                            .await;
                            drop(target_guard.take());
                            drop(target_stripe.take());
                            drop(permit);
                            finish_owned_pty_work(app, &mut owned_work, observed).await;
                            return false;
                        }
                        Err(_) => {
                            let observed = finish_pty_input_before_boundary(
                                &store,
                                &mut heartbeat,
                                injection_id,
                                &lease_owner,
                                C::StoreTransient,
                            )
                            .await;
                            drop(target_guard.take());
                            drop(target_stripe.take());
                            drop(permit);
                            finish_owned_pty_work(app, &mut owned_work, observed).await;
                            return false;
                        }
                    }
                } else {
                    None
                };
            let final_authority = if let Some(fresh) = final_api_guard.as_ref() {
                self.validate_claimed_api_authority_with_fresh(app, &claimed, fresh)
                    .await
            } else {
                self.validate_claimed_host_authority(app, &claimed).await
            };
            let Some(final_authority) = final_authority else {
                let observed = reject_pty_input_before_boundary(
                    &store,
                    &mut heartbeat,
                    injection_id,
                    C::AuthorityChanged,
                )
                .await;
                drop(target_guard.take());
                drop(target_stripe.take());
                drop(permit);
                finish_owned_pty_work(app, &mut owned_work, observed).await;
                return false;
            };
            if final_authority.target.authority_fingerprint != verified_target.authority_fingerprint
                || !crate::path_identity::same_object(
                    &final_authority.target.replica_identity,
                    &verified_target.replica_identity,
                )
            {
                let observed = reject_pty_input_before_boundary(
                    &store,
                    &mut heartbeat,
                    injection_id,
                    C::AuthorityChanged,
                )
                .await;
                drop(target_guard.take());
                drop(target_stripe.take());
                drop(permit);
                finish_owned_pty_work(app, &mut owned_work, observed).await;
                return false;
            }
            if self
                .validate_selected_pty_target(
                    app,
                    &claimed,
                    &verified_target,
                    selected_id,
                    selected.backend_kind,
                )
                .await
                .is_none()
            {
                let observed = finish_pty_input_before_boundary(
                    &store,
                    &mut heartbeat,
                    injection_id,
                    &lease_owner,
                    C::SessionRace,
                )
                .await;
                drop(target_guard.take());
                drop(target_stripe.take());
                drop(permit);
                finish_owned_pty_work(app, &mut owned_work, observed).await;
                return false;
            }
            if heartbeat.failed() {
                let code = if expires <= chrono::Utc::now() {
                    C::Expired
                } else {
                    C::LeaseLost
                };
                let observed = finish_pty_input_before_boundary(
                    &store,
                    &mut heartbeat,
                    injection_id,
                    &lease_owner,
                    code,
                )
                .await;
                drop(target_guard.take());
                drop(target_stripe.take());
                drop(permit);
                finish_owned_pty_work(app, &mut owned_work, observed).await;
                return false;
            }
            if !matches!(
                store
                    .renew_pty_input_lease_offloaded(
                        injection_id.to_string(),
                        lease_owner.clone(),
                        chrono::Utc::now(),
                    )
                    .await,
                Ok(true)
            ) {
                let observed = finish_pty_input_before_boundary(
                    &store,
                    &mut heartbeat,
                    injection_id,
                    &lease_owner,
                    pty_input_lease_failure_code(expires),
                )
                .await;
                drop(target_guard.take());
                drop(target_stripe.take());
                drop(permit);
                finish_owned_pty_work(app, &mut owned_work, observed).await;
                return false;
            }
            if !heartbeat.finish().await {
                let observed = finish_pty_input_before_boundary(
                    &store,
                    &mut heartbeat,
                    injection_id,
                    &lease_owner,
                    pty_input_lease_failure_code(expires),
                )
                .await;
                drop(target_guard.take());
                drop(target_stripe.take());
                drop(permit);
                finish_owned_pty_work(app, &mut owned_work, observed).await;
                return false;
            }
            let backend_name = match selected.backend_kind {
                SessionBackendKind::LocalProcess => "localProcess",
                SessionBackendKind::ContainerTransport => "containerTransport",
            };
            let payload = match store
                .begin_pty_actuating_offloaded(
                    injection_id.to_string(),
                    lease_owner.clone(),
                    selected.id.clone(),
                    backend_name.to_string(),
                    chrono::Utc::now(),
                )
                .await
            {
                Ok(payload) => payload,
                Err(crate::api::message_store::MessageStoreError::ActuationCommitAmbiguous) => {
                    let terminal = store
                        .terminalize_pty_input_offloaded(
                            injection_id.to_string(),
                            S::Indeterminate,
                            Some(C::TerminalStoreFailed),
                            chrono::Utc::now(),
                        )
                        .await;
                    let observed = terminal.as_ref().is_ok_and(|result| result.terminal);
                    drop(final_api_guard);
                    drop(target_guard.take());
                    drop(target_stripe.take());
                    drop(permit);
                    finish_owned_pty_work(app, &mut owned_work, observed).await;
                    if terminal.is_err() {
                        log::error!(
                            "[pty-input] ambiguous boundary reconciliation failed id={} \
                        code=terminal_store_failed",
                            injection_id
                        );
                    }
                    return false;
                }
                Err(_) => {
                    reject_pty_input_before_boundary(
                        &store,
                        &mut heartbeat,
                        injection_id,
                        C::StoreTransient,
                    )
                    .await;
                    return false;
                }
            };
            let post_authority = if let Some(fresh) = final_api_guard.as_ref() {
                self.validate_claimed_api_authority_with_fresh(app, &claimed, fresh)
                    .await
            } else {
                self.validate_claimed_host_authority(app, &claimed).await
            };
            let payload_matches = initial.payload_bytes == Some(payload.len() as u64)
                && initial.payload_sha256.as_deref()
                    == Some(crate::phone::types::sha256_hex(&payload).as_str());
            let post_valid = payload_matches
                && post_authority.is_some_and(|route| {
                    route.sender.authority_fingerprint
                        == final_authority.sender.authority_fingerprint
                        && route.target.authority_fingerprint
                            == verified_target.authority_fingerprint
                        && crate::path_identity::same_object(
                            &route.target.replica_identity,
                            &verified_target.replica_identity,
                        )
                })
                && self
                    .validate_selected_pty_target(
                        app,
                        &claimed,
                        &verified_target,
                        selected_id,
                        selected.backend_kind,
                    )
                    .await
                    .is_some();
            if !post_valid {
                let terminal = store
                    .terminalize_pty_input_offloaded(
                        injection_id.to_string(),
                        S::Indeterminate,
                        Some(C::FinalRevalidationFailed),
                        chrono::Utc::now(),
                    )
                    .await;
                let observed = terminal.as_ref().is_ok_and(|result| result.terminal);
                drop(final_api_guard);
                drop(target_guard.take());
                drop(target_stripe.take());
                drop(permit);
                finish_owned_pty_work(app, &mut owned_work, observed).await;
                if terminal.is_err() {
                    log::error!(
                        "[pty-input] post-boundary terminalization failed id={} \
                    code=terminal_store_failed",
                        injection_id
                    );
                }
                return false;
            }
            let manager = {
                let guard = session_manager.read().await;
                guard.clone()
            };
            let idle_detector = app
                .state::<Arc<crate::pty::idle_detector::IdleDetector>>()
                .inner()
                .clone();
            let authority_id =
                match crate::phone::types::parse_canonical_uuid_v4(&claimed.authority_session_id) {
                    Ok(id) => id,
                    Err(_) => {
                        let terminal = store
                            .terminalize_pty_input_offloaded(
                                injection_id.to_string(),
                                S::Indeterminate,
                                Some(C::FinalRevalidationFailed),
                                chrono::Utc::now(),
                            )
                            .await;
                        let observed = terminal.as_ref().is_ok_and(|result| result.terminal);
                        drop(final_api_guard);
                        drop(target_guard.take());
                        drop(target_stripe.take());
                        drop(permit);
                        finish_owned_pty_work(app, &mut owned_work, observed).await;
                        if terminal.is_err() {
                            log::error!(
                                "[pty-input] boundary terminalization failed id={} \
                        code=terminal_store_failed",
                                injection_id
                            );
                        }
                        return false;
                    }
                };
            let authority_backend = match source_plane {
                crate::phone::types::PtyInputSourcePlane::HostCli => {
                    SessionBackendKind::LocalProcess
                }
                crate::phone::types::PtyInputSourcePlane::ContainerApi => {
                    SessionBackendKind::ContainerTransport
                }
            };
            let authority_container_backend = pty_manager
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .container_backend();
            let authority_route =
                match PtyManager::authority_route_proof(&pty_manager, authority_id) {
                    Ok(proof) => proof,
                    Err(_) => {
                        let terminal = store.terminalize_pty_input(
                            injection_id,
                            S::Indeterminate,
                            Some(C::FinalRevalidationFailed),
                            chrono::Utc::now(),
                        );
                        let observed = terminal.as_ref().is_ok_and(|result| result.terminal);
                        drop(final_api_guard);
                        drop(target_guard.take());
                        drop(target_stripe.take());
                        drop(permit);
                        finish_owned_pty_work(app, &mut owned_work, observed).await;
                        if terminal.is_err() {
                            log::error!(
                                "[pty-input] boundary terminalization failed id={} \
                        code=terminal_store_failed",
                                injection_id
                            );
                        }
                        return false;
                    }
                };
            let expected_api_binding = final_api_guard.as_ref().and_then(|fresh| {
                let root =
                    crate::path_identity::verify_directory(Path::new(&fresh.client.bound_root))
                        .ok()?;
                Some((
                    fresh.client.client_id.clone(),
                    fresh.client.credential_generation.clone()?,
                    fresh.client.bound_session_id.clone()?,
                    root.object_id,
                    fresh.presented_token_hash.clone(),
                ))
            });
            if source_plane == crate::phone::types::PtyInputSourcePlane::ContainerApi
                && expected_api_binding.is_none()
            {
                let terminal = store.terminalize_pty_input(
                    injection_id,
                    S::Indeterminate,
                    Some(C::FinalRevalidationFailed),
                    chrono::Utc::now(),
                );
                let observed = terminal.as_ref().is_ok_and(|result| result.terminal);
                drop(final_api_guard);
                drop(target_guard.take());
                drop(target_stripe.take());
                drop(permit);
                finish_owned_pty_work(app, &mut owned_work, observed).await;
                if terminal.is_err() {
                    log::error!(
                    "[pty-input] boundary terminalization failed id={} code=terminal_store_failed",
                    injection_id
                );
                }
                return false;
            }
            let boundary_settings = app.state::<SettingsState>().inner().clone();
            let owned_route = owned_work
                .as_ref()
                .and_then(|_| PtyManager::snapshot_route_proof(&pty_manager, selected_id).ok());
            let boundary_carrier = crate::session::manager::acquire_carrier_mutation_permit().await;
            let owned_token = owned_work
                .as_ref()
                .and_then(|(continuation, _)| continuation.launch_token());
            let owned_current = match owned_work.as_ref() {
                Some((continuation, _)) => {
                    manager
                        .with_current_carrier_snapshot(&boundary_carrier, |current| {
                            owned_token.is_some()
                                && continuation.current_completion_region(
                                    &boundary_carrier,
                                    current,
                                    &pty_manager,
                                )
                        })
                        .await
                }
                None => true,
            };
            if !owned_current {
                drop(boundary_carrier);
                let observed = store
                    .terminalize_pty_input_offloaded(
                        injection_id.to_string(),
                        S::Indeterminate,
                        Some(C::FinalRevalidationFailed),
                        chrono::Utc::now(),
                    )
                    .await
                    .is_ok_and(|result| result.terminal);
                drop(final_api_guard);
                drop(target_guard.take());
                drop(target_stripe.take());
                drop(permit);
                finish_owned_pty_work(app, &mut owned_work, observed).await;
                return false;
            }
            let boundary_write = match manager
                .prepare_pty_input_boundary_with_carrier_permit(
                    &boundary_carrier,
                    selected_id,
                    &verified_target,
                    selected.backend_kind,
                    authority_id,
                    &final_authority.sender,
                    authority_backend,
                    &authority_route,
                    &permit,
                    &idle_detector,
                    &boundary_settings,
                    |session, settings| {
                        owned_token.is_none_or(|token| session.token == token)
                            && owned_work.as_ref().is_none_or(|(continuation, _)| {
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
                        if expires <= chrono::Utc::now()
                            || app
                                .try_state::<Arc<crate::RestoreInProgress>>()
                                .is_some_and(|flag| {
                                    flag.0.load(std::sync::atomic::Ordering::SeqCst)
                                })
                            || app
                                .try_state::<Arc<crate::session::purge_guard::PurgeGuard>>()
                                .is_some_and(|guard| guard.blocks_agent(&claimed.target_fqn))
                        {
                            return false;
                        }
                        match (&source_plane, expected_api_binding.as_ref()) {
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
                                    authority_container_backend.credential_binding(authority_id);
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
                        guard, &payload,
                    ))
                }
                Err(_) => {
                    drop(boundary_carrier);
                    // The guard-bearing Result ends in this match. Only the
                    // observed terminal boolean survives to the work handoff await.
                    let terminal = store.terminalize_pty_input(
                        injection_id,
                        S::Indeterminate,
                        Some(C::FinalRevalidationFailed),
                        chrono::Utc::now(),
                    );
                    if terminal.is_err() {
                        log::error!(
                            "[pty-input] boundary terminalization failed id={} \
                        code=terminal_store_failed",
                            injection_id
                        );
                    }
                    Err(terminal.as_ref().is_ok_and(|result| result.terminal))
                }
            };
            let text_write_succeeded = match boundary_write {
                Ok(written) => written,
                Err(observed) => {
                    drop(final_api_guard);
                    drop(target_guard.take());
                    drop(target_stripe.take());
                    drop(permit);
                    finish_owned_pty_work(app, &mut owned_work, observed).await;
                    return false;
                }
            };
            // Keep the fresh registry lock through the synchronous first-write
            // boundary so a concurrent revocation linearizes before or after it,
            // never between the final authority check and the text write.
            drop(final_api_guard);
            let outcome = crate::pty::inject::submit_exact_agent_input_with_permit(
                &permit,
                text_write_succeeded,
            )
            .await;
            let (status, reason) =
                outcome::pty_submit_terminal_status(app, &payload, selected_id, outcome).await;
            let result = match store
                .terminalize_pty_input_offloaded(
                    injection_id.to_string(),
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
                        injection_id
                    );
                    return false;
                }
            };
            crate::api::audit::record_pty_input_result("terminal", &result);
            drop(target_guard.take());
            drop(target_stripe.take());
            drop(permit);
            finish_owned_pty_work(app, &mut owned_work, result.terminal).await;
            if !app.pty_input_status(&result) {
                log::warn!(
                    "[pty-input] status event failed id={} code=boundary_metadata_failed",
                    injection_id
                );
            }
            false
        }
        .await;
        source_executor.returned(failed);
    }
}
