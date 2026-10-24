//! Native lifecycle execution behind the selection-owned port.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use tauri::Runtime;
use uuid::Uuid;

use super::lifecycle::{self, LifecycleAuthorityAdmission, LifecycleEntryAuthority};
use super::{destroy, restart, root_agent};
use crate::session::selection::{
    DestroyOutcome, DestroyRequest, LifecycleAdmission, LifecycleEffectGuard,
    LifecycleEntryOutcome, LifecycleExecutor, LifecycleExecutorState, LifecycleFuture,
    LifecycleLease, RestartJobRequest, RootJobRequest, SelectionTransaction,
};

struct NativeLifecycleExecutor;

#[expect(
    dead_code,
    reason = "P04 switches native dispatch to the execution port"
)]
struct NativeLifecycleLease {
    authority: Option<LifecycleEntryAuthority>,
    release_receipt: Arc<AtomicU8>,
}

impl Drop for NativeLifecycleLease {
    fn drop(&mut self) {
        let released = self
            .authority
            .take()
            .is_some_and(|authority| authority.release());
        self.release_receipt
            .store(if released { 1 } else { 2 }, Ordering::Release);
    }
}

#[expect(
    dead_code,
    reason = "P04 switches native dispatch to the execution port"
)]
impl NativeLifecycleLease {
    fn wrap(authority: LifecycleEntryAuthority) -> LifecycleLease {
        let release_receipt = Arc::new(AtomicU8::new(0));
        LifecycleLease::new(
            Box::new(Self {
                authority: Some(authority),
                release_receipt: Arc::clone(&release_receipt),
            }),
            release_receipt,
        )
    }
}

impl<R: Runtime> LifecycleExecutor<SelectionTransaction<R>> for NativeLifecycleExecutor {
    fn prepare<'a>(
        &'a self,
        transaction: &'a SelectionTransaction<R>,
        operation: Uuid,
        source: Option<Uuid>,
        root: bool,
        directory: Option<&'a str>,
    ) -> LifecycleFuture<'a, Result<LifecycleAdmission, String>> {
        Box::pin(async move {
            Ok(
                match lifecycle::prepare_lifecycle_entry_authority(
                    transaction.app(),
                    operation,
                    source,
                    root,
                    directory,
                )
                .await?
                {
                    LifecycleAuthorityAdmission::Ready(authority) => {
                        LifecycleAdmission::Ready(NativeLifecycleLease::wrap(authority))
                    }
                    LifecycleAuthorityAdmission::Waiting => LifecycleAdmission::Waiting,
                    LifecycleAuthorityAdmission::NeedsRootDirectory => {
                        LifecycleAdmission::NeedsRootDirectory
                    }
                },
            )
        })
    }

    fn validate<'a>(
        &'a self,
        transaction: &'a SelectionTransaction<R>,
        lease: &'a mut LifecycleLease,
    ) -> LifecycleFuture<'a, bool> {
        Box::pin(async move {
            let Some(native) = lease.downcast_mut::<NativeLifecycleLease>() else {
                return false;
            };
            let Some(authority) = native.authority.as_mut() else {
                return false;
            };
            lifecycle::validate_lifecycle_entry_authority(transaction.app(), authority).await
        })
    }

    fn has_claim(&self, lease: &LifecycleLease) -> bool {
        lease
            .downcast_ref::<NativeLifecycleLease>()
            .and_then(|native| native.authority.as_ref())
            .is_some_and(LifecycleEntryAuthority::has_owned_claim)
    }

    fn release(&self, lease: LifecycleLease) -> bool {
        let valid = lease
            .downcast_ref::<NativeLifecycleLease>()
            .is_some_and(|native| native.authority.is_some());
        lease.release() && valid
    }

    fn ensure_root_directory(&self, effect: LifecycleEffectGuard) -> Result<String, String> {
        root_agent::ensure_owned_root_directory(effect)
    }

    fn destroy<'a>(
        &'a self,
        transaction: &'a SelectionTransaction<R>,
        request: DestroyRequest,
    ) -> LifecycleFuture<'a, Result<DestroyOutcome, String>> {
        Box::pin(destroy::execute_destroy_transaction(transaction, request))
    }

    fn restart<'a>(
        &'a self,
        transaction: &'a SelectionTransaction<R>,
        request: RestartJobRequest,
        lease: &'a LifecycleLease,
    ) -> LifecycleFuture<'a, LifecycleEntryOutcome> {
        Box::pin(async move {
            let Some(authority) = lease
                .downcast_ref::<NativeLifecycleLease>()
                .and_then(|native| native.authority.as_ref())
            else {
                return LifecycleEntryOutcome::Interrupted;
            };
            restart::execute_external_restart_transaction(transaction, request, authority).await
        })
    }

    fn root<'a>(
        &'a self,
        transaction: &'a SelectionTransaction<R>,
        request: RootJobRequest,
        lease: &'a LifecycleLease,
    ) -> LifecycleFuture<'a, LifecycleEntryOutcome> {
        Box::pin(async move {
            let Some(authority) = lease
                .downcast_ref::<NativeLifecycleLease>()
                .and_then(|native| native.authority.as_ref())
            else {
                return LifecycleEntryOutcome::Interrupted;
            };
            root_agent::execute_external_root_transaction(transaction, request, authority).await
        })
    }

    fn conditional_close<'a>(
        &'a self,
        transaction: &'a SelectionTransaction<R>,
        ticket: crate::pty::manager::ResponseCloseTicket,
    ) -> LifecycleFuture<'a, Result<(), String>> {
        Box::pin(async move {
            destroy::execute_response_close_transaction(transaction, ticket)
                .await
                .map(|_| ())
        })
    }

    fn reconcile<'a>(
        &'a self,
        transaction: &'a SelectionTransaction<R>,
    ) -> LifecycleFuture<'a, ()> {
        Box::pin(destroy::reconcile_response_closes(transaction.app()))
    }

    fn complete_retirement<'a>(
        &'a self,
        transaction: &'a SelectionTransaction<R>,
        id: Uuid,
    ) -> LifecycleFuture<'a, ()> {
        Box::pin(restart::complete_owned_replacement_retirement(
            transaction,
            id,
        ))
    }
}

pub(crate) fn state<R: Runtime>() -> LifecycleExecutorState<SelectionTransaction<R>> {
    LifecycleExecutorState::new(Arc::new(NativeLifecycleExecutor))
}

pub(crate) fn install<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    builder.manage(state::<R>())
}

#[cfg(test)]
mod tests;
