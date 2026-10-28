use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Weak;

use tokio::sync::{mpsc, oneshot, OwnedSemaphorePermit};
use uuid::Uuid;

use crate::pty::container_lifecycle::ContainerLifecycle;
pub use crate::pty::container_lifecycle::ContainerLifecycleSender;
use crate::session::manager::PendingCreateBinding;
use crate::session::session::SessionInfo;
use crate::session::warnings::SessionWarning;

use super::admission::CriticalAdmissionOutcome;
use super::envelope::CoordinatorEnvelope;
use super::payload::{SelectionCause, SelectionCoordinatorError};
use super::replacement::OwnedReplacementContinuation;
use super::{CoordinatorInner, CoordinatorJob, FinalizeCreateRequest, SelectionCoordinator};

#[cfg(test)]
use super::replacement_fixture::ReplacementFixtureCut;

pub(crate) struct AutoCloseBatchTicket {
    pub(super) slot: Option<mpsc::OwnedPermit<CoordinatorEnvelope>>,
    pub(super) admission: Option<OwnedSemaphorePermit>,
}

impl AutoCloseBatchTicket {
    pub async fn finalize(
        mut self,
        ids: Vec<Uuid>,
    ) -> Result<crate::session::selection::DestroyOutcome, String> {
        let slot = self
            .slot
            .take()
            .ok_or_else(|| SelectionCoordinatorError::Unavailable.to_string())?;
        let admission = self
            .admission
            .take()
            .ok_or_else(|| SelectionCoordinatorError::Unavailable.to_string())?;
        let (response, receiver) = oneshot::channel();
        drop(slot.send(CoordinatorEnvelope {
            job: CoordinatorJob::Destroy {
                request: crate::session::selection::DestroyRequest {
                    ids,
                    source: crate::session::selection::DestructionSource::AutoClose,
                    force_destroy_root: false,
                },
                response,
            },
            _admission: admission,
            _create_ticket: None,
            _critical_admission: None,
        }));
        receiver
            .await
            .map_err(|_| SelectionCoordinatorError::Unavailable.to_string())?
    }
}

pub struct CreateFinalizationTicket {
    pub(super) owned_replacement: Option<OwnedReplacementContinuation>,
    pub(super) slot: Option<mpsc::OwnedPermit<CoordinatorEnvelope>>,
    pub(super) admission: Option<OwnedSemaphorePermit>,
    pub(super) create_ticket: Option<OwnedSemaphorePermit>,
    pub(super) binding: Option<PendingCreateBinding>,
    pub(super) cause: SelectionCause,
    pub(super) auto_select_precondition: Option<(String, u64)>,
    pub(super) completed: bool,
}

impl fmt::Debug for CreateFinalizationTicket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreateFinalizationTicket")
            .field("binding", &self.binding)
            .field("auto_select_precondition", &self.auto_select_precondition)
            .field("completed", &self.completed)
            .finish_non_exhaustive()
    }
}

impl CreateFinalizationTicket {
    pub(crate) fn retain_owned_replacement(
        &mut self,
        continuation: OwnedReplacementContinuation,
    ) -> Result<(), String> {
        self.check_unbound()?;
        if self.owned_replacement.is_some() {
            return Err("create finalization ticket was already bound".to_string());
        }
        self.auto_select_precondition = None;
        self.owned_replacement = Some(continuation);
        Ok(())
    }
    /// Keep the original selection precondition and binding metadata, while a
    /// blocked accepted create releases scarce finalizer capacity.
    pub(crate) fn release_capacity_for_wait(&mut self) -> bool {
        drop(self.slot.take());
        drop(self.admission.take());
        drop(self.create_ticket.take());
        !self.completed && self.binding.is_none()
    }

    pub(crate) fn resume_unbound_capacity(
        &mut self,
        coordinator: &SelectionCoordinator,
    ) -> Result<(), SelectionCoordinatorError> {
        if self.slot.is_some() && self.admission.is_some() && self.create_ticket.is_some() {
            return Ok(());
        }
        if self.completed
            || self.binding.is_some()
            || self.slot.is_some()
            || self.admission.is_some()
            || self.create_ticket.is_some()
        {
            return Err(SelectionCoordinatorError::Unavailable);
        }
        coordinator.check_external_submission()?;
        let create_ticket = coordinator
            .inner
            .create_tickets
            .clone()
            .try_acquire_owned()
            .map_err(|_| SelectionCoordinatorError::Busy)?;
        let admission = coordinator
            .inner
            .admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| SelectionCoordinatorError::Busy)?;
        let slot = coordinator
            .inner
            .sender
            .clone()
            .try_reserve_owned()
            .map_err(|error| match error {
                mpsc::error::TrySendError::Closed(_) => SelectionCoordinatorError::Unavailable,
                mpsc::error::TrySendError::Full(_) => SelectionCoordinatorError::Busy,
            })?;
        self.slot = Some(slot);
        self.admission = Some(admission);
        self.create_ticket = Some(create_ticket);
        Ok(())
    }

    fn check_unbound(&self) -> Result<(), String> {
        if self.completed || self.binding.is_some() {
            return Err("create finalization ticket was already bound".to_string());
        }
        Ok(())
    }

    pub(in crate::session) fn bind(
        &mut self,
        session_id: Uuid,
    ) -> Result<PendingCreateBinding, String> {
        self.check_unbound()?;
        self.bind_prepared(PendingCreateBinding::new(session_id, Uuid::new_v4()))
    }

    pub(crate) fn bind_prepared(
        &mut self,
        binding: PendingCreateBinding,
    ) -> Result<PendingCreateBinding, String> {
        self.check_unbound()?;
        self.binding = Some(binding);
        Ok(binding)
    }

    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
        )
    )]
    pub(crate) fn binding(&self) -> Option<PendingCreateBinding> {
        self.binding
    }

    pub async fn finalize(self, warnings: Vec<SessionWarning>) -> Result<SessionInfo, String> {
        match self.finalize_with_owned_outcome(warnings).await {
            Some(result) => result,
            None => std::future::pending().await,
        }
    }

    pub(crate) async fn finalize_with_owned_outcome(
        mut self,
        warnings: Vec<SessionWarning>,
    ) -> Option<Result<SessionInfo, String>> {
        let owned = self.owned_replacement.is_some();
        #[cfg(test)]
        let fixture_owned = self.owned_replacement.clone();
        let queued = (|| {
            let slot = self
                .slot
                .take()
                .ok_or_else(|| SelectionCoordinatorError::Unavailable.to_string())?;
            let admission = self
                .admission
                .take()
                .ok_or_else(|| SelectionCoordinatorError::Unavailable.to_string())?;
            let create_ticket = self
                .create_ticket
                .take()
                .ok_or_else(|| SelectionCoordinatorError::Unavailable.to_string())?;
            let binding = self
                .binding
                .take()
                .ok_or_else(|| "create finalization ticket was not bound".to_string())?;
            let (response, receiver) = oneshot::channel();
            drop(slot.send(CoordinatorEnvelope {
                job: CoordinatorJob::FinalizeCreate {
                    request: FinalizeCreateRequest {
                        owned_replacement: self.owned_replacement.take(),
                        binding,
                        cause: self.cause,
                        auto_select_precondition: self.auto_select_precondition.take(),
                        warnings,
                    },
                    response,
                },
                _admission: admission,
                _create_ticket: Some(create_ticket),
                _critical_admission: None,
            }));
            self.completed = true;
            Ok::<_, String>(receiver)
        })();
        let receiver = match queued {
            Ok(receiver) => receiver,
            Err(error) => return Some(Err(error)),
        };
        #[cfg(test)]
        if let Some(continuation) = fixture_owned.as_ref() {
            if let Some(control) = continuation.fixture.as_ref() {
                control.record(continuation, ReplacementFixtureCut::TicketSubmitted);
                return control.receive(continuation, receiver).await;
            }
        }
        match receiver.await {
            Ok(result) => Some(result),
            Err(_) if owned => None,
            Err(_) => Some(Err(SelectionCoordinatorError::Unavailable.to_string())),
        }
    }
}

impl super::seal::Sealed for CreateFinalizationTicket {}

impl super::authority::CreateBindingTicket<PendingCreateBinding> for CreateFinalizationTicket {
    fn bind(&mut self, session_id: Uuid) -> Result<PendingCreateBinding, String> {
        CreateFinalizationTicket::bind(self, session_id)
    }
}

impl Drop for CreateFinalizationTicket {
    fn drop(&mut self) {
        // A dropped waiter/task is never explicit stopped-executor authority.
        // The retained owner, including finalized B, survives this ticket.
        if self.owned_replacement.is_some() {
            return;
        }

        if self.completed {
            return;
        }
        let Some(binding) = self.binding.take() else {
            return;
        };
        let (Some(slot), Some(admission), Some(create_ticket)) = (
            self.slot.take(),
            self.admission.take(),
            self.create_ticket.take(),
        ) else {
            log::error!(
                "[selection] bound create ticket dropped without its reserved finalization \
                    capacity session={}",
                binding.session_id()
            );
            return;
        };
        drop(slot.send(CoordinatorEnvelope {
            job: CoordinatorJob::RollbackCreate { binding },
            _admission: admission,
            _create_ticket: Some(create_ticket),
            _critical_admission: None,
        }));
        self.completed = true;
    }
}

pub(crate) struct WeakContainerLifecycle {
    pub(crate) inner: Weak<CoordinatorInner>,
}

impl ContainerLifecycle for WeakContainerLifecycle {
    fn route_lost(
        &self,
        session_id: Uuid,
        exit_code: i32,
    ) -> Pin<Box<dyn Future<Output = Result<CriticalAdmissionOutcome<()>, String>> + Send + '_>>
    {
        Box::pin(async move {
            let inner = self
                .inner
                .upgrade()
                .ok_or_else(|| SelectionCoordinatorError::Unavailable.to_string())?;
            SelectionCoordinator { inner }
                .submit_route_loss(session_id, exit_code)
                .await
        })
    }
}

#[cfg(test)]
mod tests;
