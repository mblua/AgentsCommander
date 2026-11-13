use std::sync::{Arc, Mutex};

use uuid::Uuid;

use super::super::replacement::{
    LifecycleEffectGuard, OwnedEffectState, OwnedLaunchReceipt, OwnedReplacementContinuation,
    OwnedReplacementDescriptor, ReplacementCleanupAuthority, ReplacementStage,
};
use crate::config::sessions_persistence::PublishedCarrier;
use crate::pty::manager::PtyManager;
use crate::session::manager::PendingCreateBinding;

#[cfg(test)]
use super::super::replacement_fixture::ReplacementFixtureCut;

impl OwnedReplacementContinuation {
    pub(super) async fn begin_checkpoint_publication(
        &self,
    ) -> Option<(
        &super::super::ResolvedReplacement,
        PendingCreateBinding,
        LifecycleEffectGuard,
    )> {
        if !self.validate_source().await {
            return None;
        }
        let (descriptor, binding, rename) = self.checkpoint_publication()?;
        #[cfg(test)]
        if !self
            .fixture_reached(ReplacementFixtureCut::BeforePublication)
            .await
        {
            return None;
        }
        Some((descriptor, binding, rename))
    }

    pub(super) fn record_pending_carrier(
        &self,
        carrier: impl FnOnce() -> PublishedCarrier,
    ) -> bool {
        match self.owner.state.lock() {
            Ok(mut state) => {
                if let Some(progress) = state.replacement.as_mut() {
                    progress.pending_carrier = Some(carrier());
                }
            }
            _ => {
                return false;
            }
        }
        true
    }

    pub(super) fn capture_transferred_work(
        &self,
    ) -> Option<(PublishedCarrier, OwnedLaunchReceipt, Uuid)> {
        self.owner.state.lock().ok().and_then(|state| {
            let progress = state.replacement.as_ref()?;
            let launch = progress.launch.as_ref()?;
            if state.executor_nonce != self.executor_nonce
                || state.stage != ReplacementStage::FinalizedPublish
                || !progress.work_transferred
                || !progress.preparing_released
                || !progress.reservations_released
                || !progress.retired
                || !progress.retirement_cleanup_completed
                || !progress.activity_completed
                || progress.initial_b.is_none()
                || !matches!(
                    progress.context_prerequisite,
                    None | Some(OwnedEffectState::Completed { .. })
                )
                || progress.create_reservation.is_some()
                || progress.work_reservation.is_some()
                || !matches!(state.completion.as_ref(),
                    Some(Ok(info)) if info.id == launch.binding.session_id().to_string())
                || !state.effects.iter().all(|effect| {
                    matches!(
                        effect,
                        OwnedEffectState::NotStarted | OwnedEffectState::Completed { .. }
                    )
                })
            {
                return None;
            }
            let OwnedReplacementDescriptor::Replacement(descriptor) = &self.owner.descriptor else {
                return None;
            };
            Some((
                progress.current_checkpoint.clone()?,
                launch.clone(),
                descriptor.previous.id,
            ))
        })
    }

    pub(super) fn pending_cleanup_runtime_absent(
        &self,
        binding: PendingCreateBinding,
        pty: &Arc<Mutex<PtyManager>>,
    ) -> bool {
        pty.try_lock()
            .is_ok_and(|pty| self.pending_cleanup_runtime_matches(binding, None, &pty))
    }

    pub(super) fn pending_cleanup_is_claimed(&self, cleanup_nonce: Uuid) -> bool {
        self.owner.state.lock().is_ok_and(|state| {
            state.executor_nonce == self.executor_nonce
                && state.replacement.as_ref().is_some_and(|progress| {
                    progress.cleanup == ReplacementCleanupAuthority::Claimed(cleanup_nonce)
                        && progress.committed.is_none()
                })
        })
    }
}
