use std::sync::Arc;

use tauri::{AppHandle, Runtime};
use tokio::sync::mpsc;

use super::envelope::CoordinatorEnvelope;
use super::execute::{
    execute_finalize_create, execute_owned_finalize_create, execute_rollback_create,
    execute_route_loss, execute_transition,
};
use super::payload::SelectionCoordinatorError;
use super::transaction::SelectionTransaction;
use super::{CoordinatorInner, CoordinatorJob};

#[cfg(test)]
use super::replacement_fixture::{
    ReplacementFixtureControl, ReplacementFixtureCut, ReplacementFixtureEnvelopeTrace,
    ReplacementFixtureOutcome,
};

pub(super) async fn worker_loop<R: Runtime>(
    app: AppHandle<R>,
    inner: Arc<CoordinatorInner>,
    mut receiver: mpsc::Receiver<CoordinatorEnvelope>,
) {
    let transaction = SelectionTransaction::new(app, Arc::downgrade(&inner));
    loop {
        let envelope = tokio::select! {
            biased;
            _ = inner.shutdown.cancelled() => {
                receiver.close();
                drain_after_shutdown(&transaction, &mut receiver).await;
                break;
            }
            envelope = receiver.recv() => envelope,
        };
        let Some(envelope) = envelope else {
            break;
        };
        execute_envelope(&transaction, envelope).await;
    }
}

pub(super) async fn drain_after_shutdown<R: Runtime>(
    transaction: &SelectionTransaction<R>,
    receiver: &mut mpsc::Receiver<CoordinatorEnvelope>,
) {
    while let Some(envelope) = receiver.recv().await {
        let (job, mut hold) = envelope.into_parts();
        match job {
            #[cfg(test)]
            CoordinatorJob::InlineReplacementFixture {
                input,
                control,
                response,
            } => {
                control.record(&input.continuation, ReplacementFixtureCut::Drained);
                hold.release();
                control.record(
                    &input.continuation,
                    ReplacementFixtureCut::AdmissionReleased,
                );
                drop(input);
                drop(response);
            }
            CoordinatorJob::FinalizeCreate { request, response } => {
                if let Some(owned) = request.owned_replacement {
                    // Shutdown is not stopped-executor cleanup authority. The
                    // accepted caller observes Retained when this channel ends;
                    // its original pending/finalized binding remains accounted.
                    hold.release();
                    drop(hold._create_ticket.take());
                    if let Ok(mut state) = owned.owner.state.lock() {
                        state.envelope_released = true;
                        state.retained_reason =
                            Some("owned finalizer drained during shutdown".into());
                    }
                    #[cfg(test)]
                    if let Some(control) = owned.fixture.as_ref() {
                        control.record(&owned, ReplacementFixtureCut::TicketDrained);
                    }
                    drop(response);
                    continue;
                }
                execute_rollback_create(transaction, request.binding).await;
                if hold
                    .respond(
                        response,
                        Err(SelectionCoordinatorError::Unavailable.to_string()),
                    )
                    .is_err()
                {
                    log::debug!(
                        "[selection] shutdown finalizer caller dropped session={}",
                        request.binding.session_id()
                    );
                }
            }
            CoordinatorJob::RollbackCreate { binding } => {
                execute_rollback_create(transaction, binding).await;
                hold.release();
            }
            CoordinatorJob::Transition { response, .. } => {
                if hold
                    .respond(
                        response,
                        Err(SelectionCoordinatorError::Unavailable.to_string()),
                    )
                    .is_err()
                {
                    log::debug!("[selection] queued transition caller dropped during shutdown");
                }
            }
            CoordinatorJob::Snapshot { response } => {
                if hold
                    .respond(
                        response,
                        Err(SelectionCoordinatorError::Unavailable.to_string()),
                    )
                    .is_err()
                {
                    log::debug!("[selection] queued snapshot caller dropped during shutdown");
                }
            }
            CoordinatorJob::RouteLoss { response, .. } => {
                if hold
                    .respond(
                        response,
                        Err(SelectionCoordinatorError::Unavailable.to_string()),
                    )
                    .is_err()
                {
                    log::debug!("[selection] queued route-loss caller dropped during shutdown");
                }
            }
            CoordinatorJob::ConditionalResponseClose { .. } => {
                hold.release();
            }
            CoordinatorJob::Destroy { response, .. } => {
                if hold
                    .respond(
                        response,
                        Err(SelectionCoordinatorError::Unavailable.to_string()),
                    )
                    .is_err()
                {
                    log::debug!("[selection] queued destroy caller dropped during shutdown");
                }
            }
            CoordinatorJob::RestartLifecycle { response, .. } => {
                if hold
                    .respond(
                        response,
                        Err(SelectionCoordinatorError::Unavailable.to_string()),
                    )
                    .is_err()
                {
                    log::debug!("[selection] queued restart caller dropped during shutdown");
                }
            }
            CoordinatorJob::RootLifecycle { response, .. } => {
                if hold
                    .respond(
                        response,
                        Err(SelectionCoordinatorError::Unavailable.to_string()),
                    )
                    .is_err()
                {
                    log::debug!("[selection] queued Root caller dropped during shutdown");
                }
            }
            CoordinatorJob::ResourceKill { response, .. } => {
                if hold
                    .respond(
                        response,
                        Err(SelectionCoordinatorError::Unavailable.to_string()),
                    )
                    .is_err()
                {
                    log::debug!("[selection] queued resource-kill caller dropped during shutdown");
                }
            }
            CoordinatorJob::Detach { response, .. } => {
                if hold
                    .respond(
                        response,
                        Err(SelectionCoordinatorError::Unavailable.to_string()),
                    )
                    .is_err()
                {
                    log::debug!("[selection] queued detach caller dropped during shutdown");
                }
            }
            CoordinatorJob::Attach { response, .. } => {
                if hold
                    .respond(
                        response,
                        Err(SelectionCoordinatorError::Unavailable.to_string()),
                    )
                    .is_err()
                {
                    log::debug!("[selection] queued attach caller dropped during shutdown");
                }
            }
            CoordinatorJob::RestoreBarrier { started, .. } => {
                if started.send(()).is_err() {
                    log::debug!("[selection] restore submitter dropped during shutdown");
                }
                hold.release();
            }
        }
        debug_assert!(
            hold.is_released(),
            "#1580: a coordinator job arm finished without releasing admission and critical key"
        );
    }
}

pub(super) async fn execute_envelope<R: Runtime>(
    transaction: &SelectionTransaction<R>,
    envelope: CoordinatorEnvelope,
) {
    let (job, mut hold) = envelope.into_parts();
    match job {
        #[cfg(test)]
        CoordinatorJob::InlineReplacementFixture {
            input,
            control,
            response,
        } => {
            let owned = input.continuation.clone();
            control.record(&owned, ReplacementFixtureCut::WorkerEntered);
            let mut trace = ReplacementFixtureEnvelopeTrace {
                hold: &mut hold,
                control: &control,
                owned: &owned,
                released: false,
                outcome: None,
            };
            let result =
                crate::commands::session::execute_inline_replacement_fixture(transaction, input)
                    .await;
            let outcome = match &result.outcome {
                Ok(crate::commands::session::OwnedReplacementCreateStep::Finalized(_)) => {
                    ReplacementFixtureOutcome::Finalized
                }
                Ok(crate::commands::session::OwnedReplacementCreateStep::Retained) => {
                    ReplacementFixtureOutcome::Retained
                }
                Err(_) => ReplacementFixtureOutcome::Error,
            };
            control.record_outcome(
                &owned,
                ReplacementFixtureCut::AdapterReturned,
                Some(outcome),
            );
            let resumed = ReplacementFixtureControl::wait_gate(&control.before_release_gate).await;
            trace.outcome = Some(outcome);
            trace.release();
            let after_release = control
                .reached(&owned, ReplacementFixtureCut::AfterReleaseBeforeSend)
                .await;
            if resumed && after_release {
                if response.send(result).is_err() {
                    control.record(&owned, ReplacementFixtureCut::ReceiverLost);
                }
            } else {
                control.record(&owned, ReplacementFixtureCut::GateLost);
                drop(result);
                drop(response);
            }
        }
        CoordinatorJob::Transition { request, response } => {
            let result = execute_transition(transaction, request).await;
            if hold.respond(response, result).is_err() {
                log::debug!("[selection] transition caller dropped before result delivery");
            }
        }
        CoordinatorJob::Snapshot { response } => {
            let snapshot = transaction.manager().await.selection_payload().await;
            if hold.respond(response, Ok(snapshot)).is_err() {
                log::debug!("[selection] snapshot caller dropped before result delivery");
            }
        }
        CoordinatorJob::FinalizeCreate { request, response } => {
            let session_id = request.binding.session_id();
            let owned = request.owned_replacement.clone();
            let result = if owned.is_some() {
                execute_owned_finalize_create(transaction, request).await
            } else {
                Some(execute_finalize_create(transaction, request).await)
            };
            match result {
                Some(result) => {
                    let observed = result.clone();
                    hold.release();
                    if owned.is_some() {
                        drop(hold._create_ticket.take());
                    }
                    if let Some(owned) = owned {
                        if let Ok(mut state) = owned.owner.state.lock() {
                            state.completion = Some(observed);
                            state.publication_observed = true;
                            state.envelope_released = true;
                        }
                        owned.owner.changed.notify_waiters();
                    }
                    // No await separates recording the attempt from sending.
                    // Only receipt permits the producer to certify work.
                    let sent = response.send(result);
                    if sent.is_err() {
                        log::debug!(
                            "[selection] create finalizer caller dropped session={}",
                            session_id
                        );
                    }
                }
                None => {
                    hold.release();
                    drop(response);
                }
            }
        }
        CoordinatorJob::RollbackCreate { binding } => {
            execute_rollback_create(transaction, binding).await;
            hold.release();
        }
        CoordinatorJob::RouteLoss {
            session_id,
            exit_code,
            response,
        } => {
            let result = execute_route_loss(transaction, session_id, exit_code).await;
            if hold.respond(response, result).is_err() {
                log::debug!(
                    "[selection] route-loss caller dropped before result delivery session={}",
                    session_id
                );
            }
        }
        CoordinatorJob::ConditionalResponseClose { request } => {
            if let Err(error) = crate::commands::session::execute_response_close_transaction(
                transaction,
                request.ticket,
            )
            .await
            {
                log::warn!("[response-close] conditional operation failed: {}", error);
            }
            hold.release();
        }
        CoordinatorJob::Destroy { request, response } => {
            let result =
                crate::commands::session::execute_destroy_transaction(transaction, request).await;
            if hold.respond(response, result).is_err() {
                log::debug!("[selection] destroy caller dropped before result delivery");
            }
        }
        CoordinatorJob::RestartLifecycle {
            request,
            mut entry,
            response,
        } => {
            let Some(authority) = entry.authority.as_mut() else {
                hold.release();
                drop(response);
                return;
            };
            if !crate::commands::session::validate_lifecycle_entry_authority(
                transaction.app(),
                authority,
            )
            .await
            {
                hold.release();
                drop(response);
                return;
            }
            let transaction = transaction.for_lifecycle_entry(&entry);
            if !transaction.has_accepted_lifecycle_core() {
                hold.release();
                drop(response);
                return;
            }
            let Some(authority) = entry.authority.as_ref() else {
                hold.release();
                drop(response);
                return;
            };
            let result = crate::commands::session::execute_external_restart_transaction(
                &transaction,
                request,
                authority,
            )
            .await;
            let result = match result {
                crate::commands::session::LifecycleEntryOutcome::Returned(result) => result,
                crate::commands::session::LifecycleEntryOutcome::Interrupted => {
                    entry.observe_authority_interruption();
                    drop(transaction);
                    drop(entry);
                    hold.release();
                    drop(response);
                    return;
                }
            };
            entry.observe_return(&result);
            entry.release_lease().await;
            drop(transaction);
            drop(entry);
            if hold.respond(response, result).is_err() {
                log::debug!("[selection] restart caller dropped before result delivery");
            }
        }
        CoordinatorJob::RootLifecycle {
            request,
            mut entry,
            response,
        } => {
            let Some(authority) = entry.authority.as_mut() else {
                hold.release();
                drop(response);
                return;
            };
            if !crate::commands::session::validate_lifecycle_entry_authority(
                transaction.app(),
                authority,
            )
            .await
            {
                hold.release();
                drop(response);
                return;
            }
            let transaction = transaction.for_lifecycle_entry(&entry);
            if !transaction.has_accepted_lifecycle_core() {
                hold.release();
                drop(response);
                return;
            }
            let Some(authority) = entry.authority.as_ref() else {
                hold.release();
                drop(response);
                return;
            };
            let result = crate::commands::session::execute_external_root_transaction(
                &transaction,
                request,
                authority,
            )
            .await;
            let result = match result {
                crate::commands::session::LifecycleEntryOutcome::Returned(result) => result,
                crate::commands::session::LifecycleEntryOutcome::Interrupted => {
                    entry.observe_authority_interruption();
                    drop(transaction);
                    drop(entry);
                    hold.release();
                    drop(response);
                    return;
                }
            };
            entry.observe_return(&result);
            entry.release_lease().await;
            drop(transaction);
            drop(entry);
            if hold.respond(response, result).is_err() {
                log::debug!("[selection] Root caller dropped before result delivery");
            }
        }
        CoordinatorJob::ResourceKill {
            session_id,
            intent,
            response,
        } => {
            let result = crate::commands::resource_monitor::execute_resource_kill_transaction(
                transaction,
                session_id,
                intent,
            )
            .await;
            if hold.respond(response, result).is_err() {
                log::debug!(
                    "[selection] resource-kill caller dropped before result delivery session={}",
                    session_id
                );
            }
        }
        CoordinatorJob::Detach {
            session_id,
            geometry,
            suppress_selection,
            response,
        } => {
            let result = crate::commands::window::execute_detach_transaction(
                transaction,
                session_id,
                geometry,
                suppress_selection,
            )
            .await;
            if hold.respond(response, result).is_err() {
                log::debug!(
                    "[selection] detach caller dropped before result delivery session={}",
                    session_id
                );
            }
        }
        CoordinatorJob::Attach {
            session_id,
            response,
        } => {
            let result =
                crate::commands::window::execute_attach_transaction(transaction, session_id).await;
            if hold.respond(response, result).is_err() {
                log::debug!(
                    "[selection] attach caller dropped before result delivery session={}",
                    session_id
                );
            }
        }
        CoordinatorJob::RestoreBarrier {
            nonce,
            started,
            release,
        } => {
            if !transaction.begin_restore(nonce) {
                drop(started);
                hold.release();
                return;
            }
            if started.send(()).is_err() {
                log::debug!("[selection] restore submitter dropped before barrier start");
            }
            if release.await.is_err() {
                log::warn!("[selection] restore barrier released by dropped owner");
            }
            transaction.release_restore(nonce);
            hold.release();
        }
    }
    debug_assert!(
        hold.is_released(),
        "#1580: a coordinator job arm finished without releasing admission and critical key"
    );
}
