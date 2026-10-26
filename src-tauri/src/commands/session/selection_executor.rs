//! Native lifecycle execution behind the selection-owned port.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use tauri::{AppHandle, Manager, Runtime};
use uuid::Uuid;

#[cfg(test)]
use crate::session::selection::{
    OpaqueFixtureInput, OpaqueFixtureOutput, ReplacementFixtureControl, ReplacementFixtureOutcome,
};

use super::lifecycle::LifecycleEntryAuthority;
use super::{destroy, lifecycle, restart, root_agent};
use crate::session::selection::{
    DestroyOutcome, DestroyRequest, LifecycleAdmission, LifecycleEffectGuard,
    LifecycleEntryOutcome, LifecycleExecutor, LifecycleExecutorState, LifecycleFuture,
    LifecycleLease, RestartJobRequest, RootJobRequest, SelectionCoordinator,
    SelectionCoordinatorError, SelectionTransaction,
};

fn executor<R: Runtime>(
    app: &AppHandle<R>,
) -> Option<Arc<dyn LifecycleExecutor<SelectionTransaction<R>>>> {
    app.try_state::<LifecycleExecutorState<SelectionTransaction<R>>>()
        .map(|state| Arc::clone(state.get()))
}

pub(crate) async fn poll_response_close<R: Runtime>(app: &AppHandle<R>) {
    crate::session::selection::poll_response_close(app, executor(app)).await;
}

pub(crate) fn start_response_close_polling<R: Runtime>(
    app: AppHandle<R>,
    shutdown: crate::shutdown::ShutdownSignal,
) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::select! {
                biased;
                _ = shutdown.token().cancelled() => break,
                _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {
                    poll_response_close(&app).await;
                }
            }
        }
    })
}

impl SelectionCoordinator {
    pub fn start<R: Runtime>(&self, app: AppHandle<R>) -> Result<(), SelectionCoordinatorError> {
        let executor = executor(&app);
        self.start_with_executor(app, executor)
    }

    #[cfg(test)]
    pub(crate) async fn submit_inline_replacement_fixture(
        &self,
        input: super::FixtureReplacementInput,
        control: Arc<ReplacementFixtureControl>,
    ) -> Option<super::FixtureReplacementResult> {
        let input = OpaqueFixtureInput {
            continuation: input.continuation.clone(),
            payload: Box::new(input),
        };
        let output = self
            .submit_opaque_replacement_fixture(input, control)
            .await?;
        match output.payload.downcast::<super::FixtureReplacementResult>() {
            Ok(result) => Some(*result),
            Err(_) => {
                log::error!("[selection] invalid native fixture result payload");
                None
            }
        }
    }
}

struct NativeLifecycleExecutor;

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
    #[cfg(test)]
    fn execute_fixture<'a>(
        &'a self,
        transaction: &'a SelectionTransaction<R>,
        input: OpaqueFixtureInput,
    ) -> LifecycleFuture<'a, OpaqueFixtureOutput> {
        Box::pin(async move {
            let Ok(input) = input.payload.downcast::<super::FixtureReplacementInput>() else {
                return OpaqueFixtureOutput {
                    outcome: ReplacementFixtureOutcome::Error,
                    payload: Box::new(super::FixtureReplacementResult {
                        outcome: Err("invalid native fixture input payload".into()),
                        ordinary_lease: None,
                    }),
                };
            };
            let result = super::execute_inline_replacement_fixture(transaction, *input).await;
            let outcome = match &result.outcome {
                Ok(super::OwnedReplacementCreateStep::Finalized(_)) => {
                    ReplacementFixtureOutcome::Finalized
                }
                Ok(super::OwnedReplacementCreateStep::Retained) => {
                    ReplacementFixtureOutcome::Retained
                }
                Err(_) => ReplacementFixtureOutcome::Error,
            };
            OpaqueFixtureOutput {
                outcome,
                payload: Box::new(result),
            }
        })
    }

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
                    lifecycle::LifecycleAuthorityAdmission::Ready(authority) => {
                        LifecycleAdmission::Ready(NativeLifecycleLease::wrap(authority))
                    }
                    lifecycle::LifecycleAuthorityAdmission::Waiting => LifecycleAdmission::Waiting,
                    lifecycle::LifecycleAuthorityAdmission::NeedsRootDirectory => {
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
