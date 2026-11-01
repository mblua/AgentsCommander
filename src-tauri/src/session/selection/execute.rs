use crate::session::warnings::SessionWarning;

use super::replacement::OwnedReplacementContinuation;

use std::sync::{Arc, Mutex};

use tauri::{Manager, Runtime};
use uuid::Uuid;

use crate::pty::manager::PtyManager;
use crate::session::manager::{
    CommitDecision, LifecycleMutations, PendingCreateBinding, SessionManager,
};
use crate::session::session::{SessionInfo, SessionStatus};
use crate::session::warnings::emit_session_warning;
use crate::DetachedSessionsState;

use super::payload::{
    DormantRuntimeWitness, LiveRuntimeWitness, SelectionCause, SelectionCoordinatorError,
    SelectionMode, SelectionSource, SessionSelection,
};
use super::replacement::{
    EntryCreateStage, LifecycleEffectSlot, OwnedEffectState, OwnedReplacementDescriptor,
    ReplacementStage,
};
use super::shutdown_cleanup::{execute_rollback_create_parts, retained_entry_owns_binding};
use super::transaction::SelectionTransaction;
use super::{DormantRestoreRequest, FinalizeCreateRequest, SelectionRequest};

#[cfg(test)]
use super::replacement_fixture::{ReplacementFixtureAction, ReplacementFixtureCut};

pub(super) async fn execute_transition<R: Runtime>(
    transaction: &SelectionTransaction<R>,
    request: SelectionRequest,
) -> Result<Option<SessionSelection>, String> {
    match request {
        SelectionRequest::UserSwitch { session_id } => {
            let aggregate = transaction.aggregate_snapshot().await;
            let target = aggregate
                .sessions
                .iter()
                .find(|session| session.id == session_id)
                .cloned()
                .ok_or_else(|| format!("Session not found: {session_id}"))?;
            let runtime = transaction.runtime_snapshot(session_id);
            if runtime.detached {
                let label = format!("terminal-{}", session_id.to_string().replace('-', ""));
                if let Some(window) = transaction.app().get_webview_window(&label) {
                    if let Err(error) = window.set_focus() {
                        log::warn!(
                            "[selection] failed to focus detached target session={} \
                                source=userSwitch: {}",
                            session_id,
                            error
                        );
                    }
                }
                return Ok(None);
            }

            let decision = match target.status {
                SessionStatus::Exited(_) => CommitDecision::Dormant(
                    DormantRuntimeWitness::from_snapshot(runtime)
                        .ok_or_else(|| "Session is detached".to_string())?,
                ),
                _ if runtime.has_pty => CommitDecision::Live(
                    LiveRuntimeWitness::from_snapshot(runtime)
                        .ok_or_else(|| "Session has no live PTY".to_string())?,
                ),
                _ => {
                    if aggregate.selection.id() == Some(session_id) {
                        let repair = transaction
                            .commit(
                                CommitDecision::Clear,
                                SelectionCause::LivenessReconcile,
                                LifecycleMutations::default(),
                            )
                            .await?;
                        if let Some(payload) = repair.selection.as_ref() {
                            transaction
                                .persist(SelectionSource::LivenessReconcile, Some(session_id))
                                .await;
                            transaction.publish_selection(payload);
                        }
                    }
                    return Err("Session has no live PTY".to_string());
                }
            };
            let committed = transaction
                .commit(
                    decision,
                    SelectionCause::UserSwitch,
                    LifecycleMutations::default(),
                )
                .await?;
            transaction.publish_view_requested(session_id);
            if let Some(payload) = committed.selection.as_ref() {
                transaction
                    .persist(SelectionSource::UserSwitch, Some(session_id))
                    .await;
                transaction.publish_selection(payload);
            }
            Ok(committed.selection)
        }
        SelectionRequest::Restore { persisted_target } => {
            let aggregate = transaction.aggregate_snapshot().await;
            let exact = persisted_target.and_then(|target_id| {
                let record = aggregate
                    .sessions
                    .iter()
                    .find(|record| record.id == target_id)?;
                let runtime = transaction.runtime_snapshot(target_id);
                if runtime.detached {
                    return None;
                }
                match record.status {
                    SessionStatus::Exited(_) => transaction.dormant_decision(target_id),
                    _ if runtime.has_pty => transaction.live_decision(target_id),
                    _ => None,
                }
            });
            let decision = exact
                .or_else(|| {
                    aggregate.sessions.iter().find_map(|record| {
                        if matches!(record.status, SessionStatus::Exited(_)) {
                            return None;
                        }
                        transaction.live_decision(record.id)
                    })
                })
                .unwrap_or(CommitDecision::Clear);
            let committed = transaction
                .commit(
                    decision,
                    SelectionCause::Restore,
                    LifecycleMutations::default(),
                )
                .await?;
            if let Some(payload) = committed.selection.as_ref() {
                transaction
                    .persist(SelectionSource::Restore, payload.id())
                    .await;
                transaction.publish_selection(payload);
            }
            Ok(committed.selection)
        }
    }
}

pub(super) async fn execute_owned_finalize_create<R: Runtime>(
    transaction: &SelectionTransaction<R>,
    request: FinalizeCreateRequest,
) -> Option<Result<SessionInfo, String>> {
    let continuation = request.owned_replacement?;
    #[cfg(test)]
    if !continuation
        .fixture_reached(ReplacementFixtureCut::BeforeFinalizer)
        .await
    {
        return None;
    }
    let manager = transaction.manager().await;
    if manager.get_pending_session(request.binding).await.is_none() {
        return Some(Err("created session pending record is unavailable".into()));
    }
    let runtime = transaction.runtime_snapshot(request.binding.session_id());
    let live = match LiveRuntimeWitness::from_snapshot(runtime) {
        Some(live) => live,
        None => return Some(Err("created session is not displayable".into())),
    };
    {
        let mut state = continuation.owner.state.lock().ok()?;
        state.replacement.as_mut()?.warnings = request.warnings;
    }
    let mut mutations = LifecycleMutations::default();
    mutations.finalize_live(request.binding, live);
    #[cfg(test)]
    if continuation
        .fixture
        .as_ref()
        .and_then(|control| {
            control.take_action(&continuation, |action| {
                matches!(action, ReplacementFixtureAction::SimulatedPrecommitFailure)
            })
        })
        .is_some()
    {
        return Some(Err("simulated fixture precommit setup failure".into()));
    }
    // Owned replacement never auto-selects B. The later R5 commit alone
    // removes A with current Clear/Keep; generic None stays literal below.
    match continuation
        .commit_finalized(transaction, CommitDecision::Keep, request.cause, mutations)
        .await?
    {
        Ok(_) => {}
        Err(error) => return Some(Err(error)),
    }
    let (info, selection) = continuation.complete_finalized_create(transaction).await?;
    let publication = continuation.start_effect(LifecycleEffectSlot::Publication)?;
    let (warnings, previous) = {
        let state = continuation.owner.state.lock().ok()?;
        let progress = state.replacement.as_ref()?;
        let OwnedReplacementDescriptor::Replacement(descriptor) = &continuation.owner.descriptor
        else {
            return None;
        };
        (progress.warnings.clone(), descriptor.previous.id)
    };
    // Owned checkpoint already persisted current B. No generic stale save.
    transaction.publish_created(&info);
    transaction.publish_destroyed(previous);
    transaction.publish_selection(&selection);
    for warning in warnings {
        emit_session_warning(transaction.app(), warning);
    }
    publication.complete(Ok(Some(info.id.clone())));
    Some(Ok(info))
}

pub(super) async fn execute_finalize_create<R: Runtime>(
    transaction: &SelectionTransaction<R>,
    request: FinalizeCreateRequest,
) -> Result<SessionInfo, String> {
    let session_id = request.binding.session_id();
    if transaction
        .manager()
        .await
        .get_pending_session(request.binding)
        .await
        .is_none()
    {
        execute_rollback_create(transaction, request.binding).await;
        return Err("created session pending record is unavailable".to_string());
    }
    let runtime = transaction.runtime_snapshot(session_id);
    let live = LiveRuntimeWitness::from_snapshot(runtime)
        .ok_or_else(|| "created session is not displayable".to_string());
    let live = match live {
        Ok(live) => live,
        Err(error) => {
            execute_rollback_create(transaction, request.binding).await;
            return Err(error);
        }
    };
    let current = transaction.manager().await.selection_payload().await;
    let should_select =
        request
            .auto_select_precondition
            .as_ref()
            .is_some_and(|(epoch, revision)| {
                current.mode() == SelectionMode::None
                    && current.epoch() == epoch
                    && current.revision() == *revision
            });
    let mut mutations = LifecycleMutations::default();
    mutations.finalize_live(request.binding, live);
    let decision = if should_select {
        CommitDecision::Live(live)
    } else {
        CommitDecision::Keep
    };
    let committed = match transaction
        .commit_entry_create(request.binding, decision, request.cause, mutations)
        .await
    {
        Ok(committed) => committed,
        Err(error) => {
            log::warn!(
                "[selection] create finalization commit rejected session={}: {}",
                session_id,
                error
            );
            execute_rollback_create(transaction, request.binding).await;
            return Err(error);
        }
    };
    let info = committed
        .finalized_rows
        .iter()
        .find(|row| row.id == session_id.to_string())
        .cloned()
        .ok_or_else(|| "create finalization did not publish a row".to_string())?;
    let publication = transaction.start_lifecycle_effect(LifecycleEffectSlot::Publication);
    if transaction.has_lifecycle_entry() && publication.is_none() {
        return Err(SelectionCoordinatorError::Unavailable.to_string());
    }
    transaction
        .persist(SelectionSource::SessionCreated, Some(session_id))
        .await;
    transaction.publish_created(&info);
    if let Some(payload) = committed.selection.as_ref() {
        transaction.publish_selection(payload);
    }
    for warning in request.warnings {
        emit_session_warning(transaction.app(), warning);
    }
    if let Some(publication) = publication {
        publication.complete(Ok(Some(info.id.clone())));
    }
    Ok(info)
}

pub(super) async fn execute_restore_dormant<R: Runtime>(
    transaction: &SelectionTransaction<R>,
    request: DormantRestoreRequest,
) -> Result<SessionInfo, String> {
    let id = Uuid::new_v4();
    let binding = PendingCreateBinding::new(id, Uuid::new_v4());
    let persisted = &request.persisted;
    let retained_key = crate::config::sessions_persistence::prompt_key(
        &request.working_directory,
        persisted.agent_id.as_deref(),
        persisted.requested_profile.as_deref(),
        persisted.effective_profile.as_deref(),
    );
    let coding_agent_binding = persisted.binding_metadata()?;
    let exit_code = match persisted.status {
        Some(SessionStatus::Exited(code)) => code,
        _ => 0,
    };
    let session = restored_dormant_session(request, id);
    let manager = transaction.manager().await;
    crate::config::sessions_persistence::initialize_prompt_retention(&manager).await;
    let initial_prompt = session.last_prompt.clone();
    let fresh = session.start_fresh_on_restore;
    let verified_key = crate::config::sessions_persistence::prompt_key(
        &session.working_directory,
        session.agent_id.as_deref(),
        session.requested_profile.as_deref(),
        session.effective_profile.as_deref(),
    );
    let retained_key = retained_key.filter(|key| Some(key) == verified_key.as_ref());
    let carrier_permit = crate::session::manager::acquire_carrier_mutation_permit().await;
    manager
        .insert_transaction_pending_record_with_carrier_permit(&carrier_permit, session, binding)
        .await?;
    if let Err(error) = manager
        .initialize_pending_prompt_with_carrier_permit(
            &carrier_permit,
            binding,
            retained_key,
            initial_prompt,
            true,
            fresh,
            false,
        )
        .await
    {
        manager
            .rollback_pending_create_with_carrier_permit(&carrier_permit, binding)
            .await?;
        return Err(error);
    }
    if let Err(error) = manager
        .install_pending_coding_agent_binding_with_carrier_permit(
            &carrier_permit,
            binding,
            coding_agent_binding,
        )
        .await
    {
        manager
            .rollback_pending_create_with_carrier_permit(&carrier_permit, binding)
            .await?;
        return Err(error);
    }
    drop(carrier_permit);
    let witness = DormantRuntimeWitness::from_snapshot(transaction.runtime_snapshot(id))
        .ok_or_else(|| "restored dormant record is runtime-detached".to_string())?;
    let mut mutations = LifecycleMutations::default();
    mutations.finalize_dormant(binding, witness, exit_code);
    let committed = transaction
        .commit(CommitDecision::Keep, SelectionCause::Restore, mutations)
        .await?;
    let info = committed
        .finalized_rows
        .into_iter()
        .find(|row| row.id == id.to_string())
        .ok_or_else(|| "dormant restore finalization did not publish a row".to_string())?;
    transaction
        .persist(SelectionSource::Restore, Some(id))
        .await;
    transaction.publish_created(&info);
    Ok(info)
}

fn restored_dormant_session(
    request: DormantRestoreRequest,
    id: Uuid,
) -> crate::session::session::Session {
    let persisted = request.persisted;
    crate::session::session::Session {
        id,
        name: persisted.name,
        shell: persisted.shell,
        shell_args: persisted.shell_args,
        backend_kind: crate::pty::backend::SessionBackendKind::LocalProcess,
        effective_shell_args: None,
        created_at: chrono::Utc::now(),
        working_directory: request.working_directory,
        status: SessionStatus::Running,
        waiting_for_input: false,
        communication: persisted.communication,
        pending_review: false,
        last_prompt: if persisted.start_fresh_on_restore {
            None
        } else {
            persisted.last_prompt.clone()
        },
        agent_id: persisted.agent_id,
        agent_label: persisted.agent_label,
        git_repos: persisted.git_repos,
        is_coordinator: request.is_coordinator,
        is_root_agent: request.is_root_agent,
        git_repos_gen: 0,
        agent_turn_armed: false,
        token: Uuid::new_v4(),
        agent_kind: None,
        requested_profile: persisted.requested_profile,
        effective_profile: persisted.effective_profile,
        profile_fallback_chain: Vec::new(),
        profile_fallback_applied: false,
        match_tier: None,
        original_profile_letter: None,
        effective_codex_home: None,
        resolved_claude_projects_dir: None,
        profile_content_hash: None,
        trusted_configured_spawn: false,
        telegram_bot_id: persisted.telegram_bot_id,
        was_detached: persisted.was_detached,
        detached_geometry: persisted.detached_geometry,
        start_fresh_on_restore: persisted.start_fresh_on_restore,
        // #1088 ignored on restore: the scraper re-reads and re-commits on its
        // first post-restart tick (mirrors id/waiting_for_input/created_at above).
        context_percent: None,
    }
}

pub(super) async fn execute_rollback_create<R: Runtime>(
    transaction: &SelectionTransaction<R>,
    binding: PendingCreateBinding,
) {
    let manager = transaction.manager().await;
    let pty = transaction
        .app()
        .try_state::<Arc<Mutex<PtyManager>>>()
        .map(|state| state.inner().clone());
    if transaction.has_lifecycle_entry() {
        execute_entry_rollback_create(transaction, manager, pty, binding).await;
    } else {
        if let Some(inner) = transaction.coordinator.upgrade() {
            if !matches!(retained_entry_owns_binding(&inner, binding), Ok(None)) {
                // An owned or unreadable retained binding is never a generic
                // rollback capability, including a stale queued cleanup job.
                return;
            }
        }
        execute_rollback_create_parts(manager, pty, binding).await;
    }
}

/// An entry's own stopped setup body calls this on its real error exit. Neither
/// a missing pending row nor an Err grants authority to kill a finalized B.
pub(super) async fn execute_entry_rollback_create<R: Runtime>(
    transaction: &SelectionTransaction<R>,
    manager: SessionManager,
    pty: Option<Arc<Mutex<PtyManager>>>,
    binding: PendingCreateBinding,
) {
    let Some((owner, executor)) = transaction.lifecycle_entry.as_ref() else {
        return;
    };
    let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
    let pending = manager.get_pending_session(binding).await;
    let cleanup_nonce = Uuid::new_v4();
    let claimed = owner.state.lock().ok().is_some_and(|mut state| {
        if state.executor_nonce != *executor
            || state.stage != ReplacementStage::Executing
            || state.effects.iter().any(|effect| {
                matches!(
                    effect,
                    OwnedEffectState::Running(_) | OwnedEffectState::Ambiguous(_)
                )
            })
        {
            return false;
        }
        let Some(create) = state.entry_create.as_mut() else {
            return false;
        };
        if create.binding != binding
            || create.stage != EntryCreateStage::Pending
            || pending.is_none()
        {
            return false;
        }
        create.stage = EntryCreateStage::CleanupClaimed(cleanup_nonce);
        true
    });
    drop(permit);
    if !claimed {
        return;
    }
    let effect = transaction.start_lifecycle_effect(LifecycleEffectSlot::PendingRollback);
    let Some(effect) = effect else {
        return;
    };
    let Some(pending) = pending else {
        return;
    };
    let kill = match pty.as_ref().and_then(|pty| pty.lock().ok()) {
        Some(pty) => pty
            .kill_for_kind(binding.session_id(), pending.backend_kind)
            .map_err(|error| error.to_string()),
        None => Err(SelectionCoordinatorError::Unavailable.to_string()),
    };
    if let Err(error) = kill {
        effect.complete(Err(error));
        if let Ok(mut state) = owner.state.lock() {
            state.stage = ReplacementStage::Ambiguous;
            state.retained_reason = Some("entry pending backend rollback failed".into());
        }
        return;
    }
    let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
    let still_claimed = owner.state.lock().ok().is_some_and(|state| {
        state.executor_nonce == *executor
            && state.entry_create.as_ref().is_some_and(|create| {
                create.binding == binding
                    && create.stage == EntryCreateStage::CleanupClaimed(cleanup_nonce)
            })
    });
    if !still_claimed {
        return;
    }
    let result = manager
        .rollback_pending_create_with_carrier_permit(&permit, binding)
        .await;
    if let Ok(mut state) = owner.state.lock() {
        if let Some(create) = state.entry_create.as_mut() {
            if create.binding == binding
                && create.stage == EntryCreateStage::CleanupClaimed(cleanup_nonce)
            {
                create.stage = if result.is_ok() {
                    EntryCreateStage::RolledBack
                } else {
                    EntryCreateStage::Unknown
                };
            }
        }
        if result.is_err() {
            state.stage = ReplacementStage::Ambiguous;
            state.retained_reason = Some("entry pending manager rollback failed".into());
        }
    }
    effect.complete(result.map(|_| None));
    drop(permit);
}

pub(super) async fn execute_route_loss<R: Runtime>(
    transaction: &SelectionTransaction<R>,
    session_id: Uuid,
    exit_code: i32,
) -> Result<(), String> {
    let aggregate = transaction.aggregate_snapshot().await;
    if aggregate.pending_ids.contains(&session_id) {
        return Ok(());
    }
    let Some(record) = aggregate
        .sessions
        .iter()
        .find(|record| record.id == session_id)
    else {
        return Ok(());
    };
    if matches!(record.status, SessionStatus::Exited(_)) {
        return Ok(());
    }
    let mut runtime = transaction.runtime_snapshot(session_id);
    if runtime.detached {
        transaction
            .app()
            .state::<DetachedSessionsState>()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&session_id);
        let label = format!("terminal-{}", session_id.to_string().replace('-', ""));
        if let Some(window) = transaction.app().get_webview_window(&label) {
            if let Err(error) = window.destroy() {
                log::warn!(
                    "[selection] route-loss detached window close failed session={}: {}",
                    session_id,
                    error
                );
            }
        }
        runtime = transaction.runtime_snapshot(session_id);
    }
    let decision = if aggregate.selection.id() == Some(session_id) {
        CommitDecision::Dormant(
            DormantRuntimeWitness::from_snapshot(runtime)
                .ok_or_else(|| "route-loss target remained detached".to_string())?,
        )
    } else {
        CommitDecision::Keep
    };
    let mut mutations = LifecycleMutations::default();
    mutations.mark_exited(session_id, exit_code);
    let committed = transaction
        .commit(decision, SelectionCause::LivenessReconcile, mutations)
        .await?;
    if committed.changed_rows.is_empty() {
        return Ok(());
    }
    transaction
        .persist(SelectionSource::LivenessReconcile, Some(session_id))
        .await;
    transaction.publish_destroyed(session_id);
    for row in &committed.changed_rows {
        transaction.publish_created(row);
    }
    for cleared in &committed.cleared_raise_hand_ids {
        transaction.publish_communication_cleared(*cleared);
    }
    if let Some(payload) = committed.selection.as_ref() {
        transaction.publish_selection(payload);
    }
    Ok(())
}

impl<R: Runtime> SelectionTransaction<R> {
    pub(crate) async fn finalize_inline_create(
        &self,
        binding: PendingCreateBinding,
        cause: SelectionCause,
        warnings: Vec<SessionWarning>,
        owned_replacement: Option<OwnedReplacementContinuation>,
    ) -> Option<Result<SessionInfo, String>> {
        let request = FinalizeCreateRequest {
            owned_replacement,
            binding,
            cause,
            auto_select_precondition: None,
            warnings,
        };
        if let Some(owned) = request.owned_replacement.clone() {
            let result = execute_owned_finalize_create(self, request).await;
            if let Some(result) = result.as_ref() {
                if let Ok(mut state) = owned.owner.state.lock() {
                    state.completion = Some(result.clone());
                    state.publication_observed = true;
                    state.envelope_released = true;
                }
                owned.owner.changed.notify_waiters();
            }
            result
        } else {
            Some(execute_finalize_create(self, request).await)
        }
    }

    pub(crate) async fn rollback_inline_create(&self, binding: PendingCreateBinding) {
        execute_rollback_create(self, binding).await;
    }

    pub(crate) async fn reconcile_route_loss_inline(
        &self,
        session_id: Uuid,
        exit_code: i32,
    ) -> Result<(), String> {
        execute_route_loss(self, session_id, exit_code).await
    }

    pub(crate) async fn restore_dormant_inline(
        &self,
        request: DormantRestoreRequest,
    ) -> Result<SessionInfo, String> {
        execute_restore_dormant(self, request).await
    }

    pub(crate) async fn restore_selection_inline(
        &self,
        persisted_target: Option<Uuid>,
    ) -> Result<Option<SessionSelection>, String> {
        execute_transition(self, SelectionRequest::restore(persisted_target)).await
    }
}
