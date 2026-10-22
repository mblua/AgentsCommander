use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use tokio::sync::{oneshot, OwnedSemaphorePermit};
use uuid::Uuid;

use crate::session::manager::PendingCreateBinding;

use super::admission::{CoordinatorPhase, CriticalAdmissionGuard};
use super::replacement::ReplacementOwner;
use super::CoordinatorJob;

pub(super) struct CoordinatorEnvelope {
    // #1580: admission and critical key are declared before `job` so an envelope
    // dropped without execution frees both before `job` drops the caller's sender.
    pub(super) _admission: OwnedSemaphorePermit,
    pub(super) _critical_admission: Option<CriticalAdmissionGuard>,
    pub(super) job: CoordinatorJob,
    pub(super) _create_ticket: Option<OwnedSemaphorePermit>,
}

/// #1580: the guards an executing envelope still owns after its job moved out.
pub(super) struct EnvelopeHold {
    pub(super) admission: Option<OwnedSemaphorePermit>,
    pub(super) critical_admission: Option<CriticalAdmissionGuard>,
    pub(super) _create_ticket: Option<OwnedSemaphorePermit>,
}

impl CoordinatorEnvelope {
    pub(super) fn into_parts(self) -> (CoordinatorJob, EnvelopeHold) {
        let CoordinatorEnvelope {
            _admission: admission,
            _critical_admission: critical_admission,
            job,
            _create_ticket: create_ticket,
        } = self;
        (
            job,
            EnvelopeHold {
                admission: Some(admission),
                critical_admission,
                _create_ticket: create_ticket,
            },
        )
    }
}

impl EnvelopeHold {
    /// Capacity first, then the dedup key. Runs before every completion send.
    /// The create ticket is not released here; it drops with the hold.
    pub(super) fn release(&mut self) {
        drop(self.admission.take());
        drop(self.critical_admission.take());
    }

    pub(super) fn is_released(&self) -> bool {
        self.admission.is_none() && self.critical_admission.is_none()
    }

    /// Releases, then sends. Use only for a completion send.
    pub(super) fn respond<T>(&mut self, response: oneshot::Sender<T>, value: T) -> Result<(), T> {
        self.release();
        response.send(value)
    }
}

/// A canceled admission must not strand startup in Restoring. Never overwrite
/// a concurrent Closing transition when returning the unaccepted admission.
pub(super) struct RestoreSubmissionPhaseGuard<'a> {
    pub(super) phase: &'a AtomicU8,
}

impl Drop for RestoreSubmissionPhaseGuard<'_> {
    fn drop(&mut self) {
        let _ = self.phase.compare_exchange(
            CoordinatorPhase::Restoring as u8,
            CoordinatorPhase::Bootstrapping as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
}

/// Constructed only by the owned continuation after its initial B receipt and
/// the current A/Preparing/launch relation have been checked under the same U.
/// This capability never lives in an owner record or a serialized value.
pub(in crate::session) struct FinalizedOwnedCarryCapability {
    pub(super) owner: Arc<ReplacementOwner>,
    pub(super) operation: Uuid,
    pub(super) preparing_nonce: Uuid,
    pub(super) previous: Uuid,
    pub(super) binding: PendingCreateBinding,
    pub(super) token: Uuid,
    pub(super) expected_version: u64,
    pub(super) telegram_bot_id: Option<String>,
    pub(super) communication: Option<crate::session::session::SessionCommunication>,
    pub(super) start_fresh_on_restore: bool,
}

impl FinalizedOwnedCarryCapability {
    pub(in crate::session) fn bound_operation(&self) -> bool {
        self.operation == self.owner.operation
            && self.preparing_nonce == self.operation
            && self.previous != self.binding.session_id()
    }
    pub(in crate::session) fn session_id(&self) -> Uuid {
        self.binding.session_id()
    }
    pub(in crate::session) fn token(&self) -> Uuid {
        self.token
    }
    pub(in crate::session) fn expected_version(&self) -> u64 {
        self.expected_version
    }
    pub(in crate::session) fn telegram_bot_id(&self) -> &Option<String> {
        &self.telegram_bot_id
    }
    pub(in crate::session) fn communication(
        &self,
    ) -> &Option<crate::session::session::SessionCommunication> {
        &self.communication
    }
    pub(in crate::session) fn start_fresh_on_restore(&self) -> bool {
        self.start_fresh_on_restore
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(in crate::session) enum FinalizedOwnedCarryOutcome {
    Applied,
    Recapture,
}
