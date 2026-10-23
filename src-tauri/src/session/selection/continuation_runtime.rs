use std::sync::{Arc, Mutex};

use tauri::{Manager, Runtime};

use crate::pty::manager::PtyManager;
use crate::session::manager::{CommitDecision, CommitResult, LifecycleMutations, SessionManager};
use crate::session::session::SessionInfo;

use super::payload::{SelectionCause, SessionSelection};
use super::replacement::{
    InitialFinalizationReceipt, LifecycleEffectSlot, OwnedReplacementContinuation,
    OwnedReplacementDescriptor, ReplacementCleanupAuthority, ReplacementStage,
};
use super::transaction::SelectionTransaction;

#[cfg(test)]
use super::replacement_fixture::{
    ReplacementFixtureAction, ReplacementFixtureControl, ReplacementFixtureCut,
};

impl OwnedReplacementContinuation {
    /// None is an internal retained outcome. It is never translated into a
    /// successful create or ordinary pending-create error/rollback.
    pub(super) async fn commit_finalized<R: Runtime>(
        &self,
        transaction: &SelectionTransaction<R>,
        decision: CommitDecision,
        cause: SelectionCause,
        mutations: LifecycleMutations,
    ) -> Option<Result<CommitResult, String>> {
        if !self.validate_source().await {
            return None;
        }
        let manager = transaction.manager().await;
        let pty = transaction
            .app()
            .state::<Arc<Mutex<PtyManager>>>()
            .inner()
            .clone();
        #[cfg(test)]
        if !self
            .fixture_reached(ReplacementFixtureCut::BeforeCommit)
            .await
        {
            return None;
        }
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        let launch = self.owner.state.lock().ok().and_then(|state| {
            if state.executor_nonce != self.executor_nonce
                || state.stage != ReplacementStage::PendingCreate
            {
                return None;
            }
            let progress = state.replacement.as_ref()?;
            (progress.cleanup == ReplacementCleanupAuthority::Executor(self.executor_nonce))
                .then(|| progress.launch.clone())
                .flatten()
        })?;
        let authorized = manager
            .with_current_carrier_snapshot(&permit, |current| {
                let tracker = pty.try_lock().ok().map(|pty| pty.response_close_state());
                tracker.is_some_and(|tracker| {
                    self.validate_create_region(&permit, current, &tracker, &pty)
                }) && Self::launch_is_current(&launch, current, &pty)
            })
            .await;
        if !authorized {
            return None;
        }
        #[cfg(test)]
        let panic_before_disarm = self
            .fixture
            .as_ref()
            .and_then(|control| {
                control.take_action(self, |action| {
                    matches!(
                        action,
                        ReplacementFixtureAction::CommitBeforeDisarmPanicOnce
                    )
                })
            })
            .is_some();
        let committed = match manager
            .commit_selection_transition_with_carrier_permit(
                &permit,
                &transaction.capability,
                decision,
                cause,
                mutations,
            )
            .await
        {
            Ok(committed) => committed,
            Err(error) => return Some(Err(error)),
        };
        // Actual commit -> synchronous disarm. No append, allocation, await or
        // fallible row lookup intervenes in this ownership transition.
        {
            #[cfg(test)]
            if panic_before_disarm {
                panic!("selected Replacement commit-before-disarm unwind");
            }
            let mut state = self.owner.state.lock().ok()?;
            let progress = state.replacement.as_mut()?;
            progress.cleanup = ReplacementCleanupAuthority::Disarmed;
            progress.committed = Some(committed);
            state.stage = ReplacementStage::FinalizedCarry;
        }
        #[cfg(test)]
        {
            if !self
                .fixture_reached(ReplacementFixtureCut::AfterDisarm)
                .await
            {
                return None;
            }
            self.fixture_stop_exact_b(&pty, &launch, &permit);
        }
        let baseline = manager
            .with_current_carrier_snapshot(&permit, |current| {
                if !Self::launch_is_current(&launch, current, &pty)
                    || current.pending_ids.contains(&launch.binding.session_id())
                {
                    return false;
                }
                let Some(version) = current
                    .versions
                    .get(&launch.binding.session_id())
                    .copied()
                    .filter(|version| *version < u64::MAX)
                else {
                    return false;
                };
                let Ok(mut state) = self.owner.state.lock() else {
                    return false;
                };
                let Some(progress) = state.replacement.as_mut() else {
                    return false;
                };
                if progress.initial_b.is_some() {
                    return false;
                }
                progress.initial_b = Some(InitialFinalizationReceipt {
                    operation: self.owner.operation,
                    preparing_nonce: progress.preparing_nonce,
                    binding: launch.binding,
                    token: launch.token,
                    carrier_version: version,
                    launch_generation: launch.generation,
                });
                true
            })
            .await;
        drop(permit);
        let committed = self.retained_commit_copy()?;
        {
            let mut state = self.owner.state.lock().ok()?;
            let progress = state.replacement.as_mut()?;
            if progress.activity_started {
                return None;
            }
            progress.activity_started = true;
        }
        crate::config::activity_log::append_batch(&committed.activity);
        if let Ok(mut state) = self.owner.state.lock() {
            if let Some(progress) = state.replacement.as_mut() {
                progress.activity_completed = true;
            }
            if !baseline {
                state.stage = ReplacementStage::Ambiguous;
                state.retained_reason =
                    Some("initial B receipt unavailable before releasing finalization U".into());
            }
        }
        #[cfg(test)]
        if baseline {
            if let Some(control) = &self.fixture {
                control.record(self, ReplacementFixtureCut::Baseline);
                if !ReplacementFixtureControl::wait_gate(&control.baseline_gate).await {
                    self.note_retained_work("fixture baseline observer lost".into());
                    return None;
                }
            }
        }
        baseline.then_some(Ok(committed))
    }

    async fn retire_previous<R: Runtime>(
        &self,
        transaction: &SelectionTransaction<R>,
        manager: &SessionManager,
        pty: &Arc<Mutex<PtyManager>>,
    ) -> Option<bool> {
        if !self.validate_source().await {
            return None;
        }
        let OwnedReplacementDescriptor::Replacement(descriptor) = &self.owner.descriptor else {
            return None;
        };
        let effect = self.start_effect(LifecycleEffectSlot::Retirement)?;
        #[cfg(test)]
        if !self
            .fixture_reached(ReplacementFixtureCut::BeforeRetirement)
            .await
        {
            return None;
        }
        let carrier = self
            .owner
            .state
            .lock()
            .ok()?
            .replacement
            .as_ref()?
            .current_checkpoint
            .clone()?;
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        let decision = manager
            .with_current_carrier_snapshot(&permit, |current| {
                if !self.publication_region_current(current, pty)
                    || crate::config::sessions_persistence::renew_carrier_replacement(
                        &permit,
                        &carrier,
                        self.owner.operation,
                        descriptor.previous.id,
                        carrier.session_id,
                        current,
                    )
                    .is_none()
                {
                    return None;
                }
                Some(if current.selection.id() == Some(descriptor.previous.id) {
                    CommitDecision::Clear
                } else {
                    CommitDecision::Keep
                })
            })
            .await?;
        let mut mutations = LifecycleMutations::default();
        mutations.remove(descriptor.previous.id);
        let retired = match manager
            .commit_selection_transition_with_carrier_permit(
                &permit,
                &transaction.capability,
                decision,
                SelectionCause::BackgroundCleanup,
                mutations,
            )
            .await
        {
            Ok(retired) => retired,
            Err(error) => {
                effect.complete(Err(error));
                return None;
            }
        };
        // Record the actual removal before any cleanup await. A missing A is
        // admissible on recovery only through this same-operation receipt.
        {
            let mut state = self.owner.state.lock().ok()?;
            let progress = state.replacement.as_mut()?;
            progress.retired = retired
                .removed_rows
                .iter()
                .any(|row| row.id == descriptor.previous.id.to_string());
            progress.retirement_result = Some(retired);
            if !progress.retired {
                return None;
            }
        }
        // The version is observed after manager Drop under the SAME retirement U.
        let reusable = manager
            .with_current_carrier_snapshot(&permit, |current| {
                crate::config::sessions_persistence::renew_carrier_replacement(
                    &permit,
                    &carrier,
                    self.owner.operation,
                    descriptor.previous.id,
                    carrier.session_id,
                    current,
                )
            })
            .await;
        drop(permit);
        let activity = self
            .owner
            .state
            .lock()
            .ok()?
            .replacement
            .as_ref()?
            .retirement_result
            .as_ref()?
            .activity
            .clone();
        crate::config::activity_log::append_batch(&activity);
        crate::commands::session::complete_owned_replacement_retirement(
            transaction,
            descriptor.previous.id,
        )
        .await;
        {
            let mut state = self.owner.state.lock().ok()?;
            let progress = state.replacement.as_mut()?;
            progress.retirement_cleanup_completed = true;
            if let Some(current) = reusable.as_ref() {
                progress.current_checkpoint = Some(current.clone());
            }
        }
        effect.complete(Ok(Some(descriptor.previous.id.to_string())));
        Some(reusable.is_some())
    }

    pub(super) async fn complete_finalized_create<R: Runtime>(
        &self,
        transaction: &SelectionTransaction<R>,
    ) -> Option<(SessionInfo, SessionSelection)> {
        let manager = transaction.manager().await;
        let pty = transaction
            .app()
            .state::<Arc<Mutex<PtyManager>>>()
            .inner()
            .clone();
        let settings = transaction
            .app()
            .state::<crate::config::settings::SettingsState>()
            .read()
            .await
            .clone();
        let roots =
            crate::config::sessions_persistence::session_retention_project_paths(&settings).ok()?;
        if !self.apply_finalized_carry(&manager, &pty).await {
            return None;
        }
        let carrier = self
            .owner
            .state
            .lock()
            .ok()?
            .replacement
            .as_ref()?
            .previous_carrier
            .clone();
        if !self
            .publish_checkpoint(&manager, &pty, &roots, carrier)
            .await
        {
            return None;
        }
        {
            let mut state = self.owner.state.lock().ok()?;
            state.stage = ReplacementStage::FinalizedRetire;
        }
        let reusable = self.retire_previous(transaction, &manager, &pty).await?;
        if !reusable {
            let carrier = {
                let mut state = self.owner.state.lock().ok()?;
                state.stage = ReplacementStage::FinalizedRecheckpoint;
                state.replacement.as_ref()?.current_checkpoint.clone()?
            };
            if !self
                .publish_checkpoint(&manager, &pty, &roots, carrier)
                .await
            {
                return None;
            }
        }
        let carrier = self
            .owner
            .state
            .lock()
            .ok()?
            .replacement
            .as_ref()?
            .current_checkpoint
            .clone()?;
        let previous = match &self.owner.descriptor {
            OwnedReplacementDescriptor::Replacement(descriptor) => descriptor.previous.id,
            _ => return None,
        };
        if !self.validate_source().await {
            return None;
        }
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        let current = manager
            .with_current_carrier_snapshot(&permit, |current| {
                if !self.publication_region_current(current, &pty)
                    || !crate::config::sessions_persistence::finish_carrier_replacement(
                        &permit,
                        &carrier,
                        self.owner.operation,
                        previous,
                        current,
                    )
                {
                    return None;
                }
                let key = crate::config::sessions_persistence::current_protected_carrier(
                    &permit, &carrier, current,
                )?;
                let row = current
                    .rows
                    .iter()
                    .find(|row| row.id == key.session_id && row.token == key.token)?;
                Some((SessionInfo::from(row), current.selection.clone(), key))
            })
            .await?;
        {
            let mut state = self.owner.state.lock().ok()?;
            let progress = state.replacement.as_mut()?;
            progress.preparing_released = true;
            progress.current_checkpoint = Some(current.2);
            state.stage = ReplacementStage::FinalizedPublish;
        }
        drop(permit);
        Some((current.0, current.1))
    }
}
