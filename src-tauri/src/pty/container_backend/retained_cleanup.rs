use super::{lock_mutex_until, RemovedSessionResources, RouteRemovalError, RouteRemover};
use crate::errors::AppError;
use crate::pty::container_runtime::{
    ContainerRuntime, ContainerRuntimeControl, ContainerRuntimeHandle,
    RetainedContainerOwnerContext, CONTAINER_STOP_TIMEOUT,
};
use crate::pty::container_tokens::ContainerApiTokenManager;
use crate::resource_monitor::ResourceLogicalAgentSlot;
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, TryLockError};
use std::time::Instant;
use uuid::Uuid;

#[derive(Default)]
pub(super) struct RetainedContainerCleanupRegistry {
    pub(super) next_id: AtomicU64,
    pub(super) entries: Mutex<HashMap<u64, Arc<RetainedContainerCleanup>>>,
}

pub(super) struct RetainedContainerCleanup {
    pub(super) id: u64,
    pub(super) session_id: Uuid,
    pub(super) reason: &'static str,
    pub(super) runtime: Option<Arc<dyn ContainerRuntime>>,
    pub(super) runtime_handle: Option<ContainerRuntimeHandle>,
    pub(super) route_removal: Mutex<RetainedRouteRemoval>,
    pub(super) non_runtime: Mutex<Option<RetainedNonRuntimeCleanup>>,
    pub(super) attempt: Mutex<RetainedContainerCleanupAttempt>,
}

pub(super) enum RetainedRouteRemoval {
    NotRequired,
    Pending(RouteRemover),
    Unavailable(String),
}

pub(super) struct RetainedNonRuntimeCleanup {
    pub(super) token_manager: Option<ContainerApiTokenManager>,
    pub(super) api_client_id: Option<String>,
    pub(super) logical_resource_slot: Option<ResourceLogicalAgentSlot>,
    pub(super) container_credential_path: Option<PathBuf>,
}

#[derive(Default)]
pub(super) struct RetainedContainerCleanupAttempt {
    pub(super) in_flight: bool,
    pub(super) last_epoch: Option<u64>,
    pub(super) last_error: Option<String>,
}

impl RetainedContainerCleanup {
    pub(super) fn require_route_removal(
        &self,
        remover: Result<Option<RouteRemover>, RouteRemovalError>,
    ) {
        let state = match remover {
            Ok(Some(remover)) => RetainedRouteRemoval::Pending(remover),
            Ok(None) => RetainedRouteRemoval::NotRequired,
            Err(error) => RetainedRouteRemoval::Unavailable(error.to_string()),
        };
        match self.route_removal.lock() {
            Ok(mut route_removal) => *route_removal = state,
            Err(error) => {
                log::error!(
                    "[container-transport] route-removal ownership lock poisoned session={} \
                        reason={} state=retained",
                    self.session_id,
                    self.reason
                );
                *error.into_inner() = RetainedRouteRemoval::Unavailable(
                    "route-removal ownership lock poisoned".to_string(),
                );
            }
        }
    }

    pub(super) fn remove_route_before_cleanup(
        &self,
        control: &ContainerRuntimeControl,
    ) -> Result<(), AppError> {
        let remover = match self.route_removal.lock() {
            Ok(route_removal) => match &*route_removal {
                RetainedRouteRemoval::NotRequired => return Ok(()),
                RetainedRouteRemoval::Pending(remover) => Arc::clone(remover),
                RetainedRouteRemoval::Unavailable(error) => {
                    return Err(AppError::Other(format!(
                        "route removal unavailable: {error}"
                    )))
                }
            },
            Err(_) => {
                return Err(AppError::Other(
                    "route-removal ownership lock poisoned".to_string(),
                ))
            }
        };
        let deadline = control.shutdown_deadline().ok_or_else(|| {
            AppError::Other("route removal missing global-sweep deadline".to_string())
        })?;
        if Instant::now() >= deadline {
            return Err(AppError::Other(
                RouteRemovalError::Deadline("globalSweep").to_string(),
            ));
        }
        remover(self.session_id, deadline)
            .map_err(|error| AppError::Other(format!("route removal failed: {error}")))?;
        match self.route_removal.lock() {
            Ok(mut route_removal) => {
                *route_removal = RetainedRouteRemoval::NotRequired;
                Ok(())
            }
            Err(_) => Err(AppError::Other(
                "route-removal ownership lock poisoned after removal".to_string(),
            )),
        }
    }
}

impl RetainedContainerCleanupRegistry {
    pub(super) fn retain(
        self: &Arc<Self>,
        token_manager: Option<ContainerApiTokenManager>,
        runtime: Option<Arc<dyn ContainerRuntime>>,
        resources: RemovedSessionResources,
        reason: &'static str,
    ) -> Arc<RetainedContainerCleanup> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let entry = Arc::new(RetainedContainerCleanup {
            id,
            session_id: resources.session_id,
            reason,
            runtime,
            runtime_handle: resources.runtime_handle,
            route_removal: Mutex::new(RetainedRouteRemoval::NotRequired),
            non_runtime: Mutex::new(Some(RetainedNonRuntimeCleanup {
                token_manager,
                api_client_id: resources.api_client_id,
                logical_resource_slot: resources.logical_resource_slot,
                container_credential_path: resources.container_credential_path,
            })),
            attempt: Mutex::new(RetainedContainerCleanupAttempt::default()),
        });
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(id, Arc::clone(&entry));
        entry
    }

    #[cfg(test)]
    pub(super) fn entries(&self) -> Vec<Arc<RetainedContainerCleanup>> {
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .values()
            .cloned()
            .collect()
    }

    pub(super) fn is_empty_until(&self, deadline: Instant) -> Option<bool> {
        lock_mutex_until(&self.entries, deadline).map(|entries| entries.is_empty())
    }

    pub(super) fn entries_for_sweep(
        &self,
        deadline: Instant,
        limit: usize,
    ) -> Vec<Arc<RetainedContainerCleanup>> {
        let Some(entries) = lock_mutex_until(&self.entries, deadline) else {
            return Vec::new();
        };
        let mut fresh = Vec::with_capacity(limit);
        let mut retries = Vec::with_capacity(limit);
        for entry in entries.values() {
            if Instant::now() >= deadline {
                break;
            }
            let attempt = match entry.attempt.try_lock() {
                Ok(attempt) => attempt,
                Err(TryLockError::Poisoned(error)) => error.into_inner(),
                Err(TryLockError::WouldBlock) => continue,
            };
            if attempt.in_flight {
                continue;
            }
            if attempt.last_error.is_none() && fresh.len() < limit {
                fresh.push(Arc::clone(entry));
            } else if retries.len() < limit {
                retries.push(Arc::clone(entry));
            }
        }
        let retry_capacity = limit.saturating_sub(fresh.len());
        fresh.extend(retries.into_iter().take(retry_capacity));
        fresh
    }

    pub(super) fn contexts(&self) -> Vec<RetainedContainerOwnerContext> {
        let (entries, registry_error) = match self.entries.try_lock() {
            Ok(entries) => (entries, None),
            Err(TryLockError::Poisoned(error)) => (
                error.into_inner(),
                Some("retained cleanup registry lock poisoned".to_string()),
            ),
            Err(TryLockError::WouldBlock) => {
                return vec![RetainedContainerOwnerContext {
                    owner: "containerCleanupRegistry",
                    session_id: None,
                    reason: "diagnostic-snapshot".to_string(),
                    program: None,
                    runtime_handle: None,
                    state: "retained",
                    in_flight: true,
                    last_error: Some("retained cleanup registry lock unavailable".to_string()),
                }]
            }
        };
        let mut contexts = entries
            .values()
            .map(|entry| {
                let (in_flight, attempt_error) = match entry.attempt.try_lock() {
                    Ok(attempt) => (attempt.in_flight, attempt.last_error.clone()),
                    Err(TryLockError::Poisoned(error)) => {
                        let attempt = error.into_inner();
                        (
                            attempt.in_flight,
                            Some(
                                attempt
                                    .last_error
                                    .as_deref()
                                    .map(|last_error| {
                                        format!("{last_error}; cleanup attempt state lock poisoned")
                                    })
                                    .unwrap_or_else(|| {
                                        "cleanup attempt state lock poisoned".to_string()
                                    }),
                            ),
                        )
                    }
                    Err(TryLockError::WouldBlock) => (
                        true,
                        Some("cleanup attempt state lock unavailable".to_string()),
                    ),
                };
                let route_error = match entry.route_removal.try_lock() {
                    Ok(route_removal) => match &*route_removal {
                        RetainedRouteRemoval::Unavailable(error) => {
                            Some(format!("route removal unavailable: {error}"))
                        }
                        RetainedRouteRemoval::NotRequired | RetainedRouteRemoval::Pending(_) => {
                            None
                        }
                    },
                    Err(TryLockError::Poisoned(_)) => {
                        Some("route-removal ownership lock poisoned".to_string())
                    }
                    Err(TryLockError::WouldBlock) => {
                        Some("route-removal ownership lock unavailable".to_string())
                    }
                };
                let last_error = match (attempt_error, route_error) {
                    (Some(attempt), Some(route)) => Some(format!("{attempt}; {route}")),
                    (Some(error), None) | (None, Some(error)) => Some(error),
                    (None, None) => None,
                };
                RetainedContainerOwnerContext {
                    owner: "containerCleanup",
                    session_id: Some(entry.session_id),
                    reason: entry.reason.to_string(),
                    program: None,
                    runtime_handle: Some(entry.runtime_handle.is_some()),
                    state: if in_flight { "inFlight" } else { "retained" },
                    in_flight,
                    last_error,
                }
            })
            .collect::<Vec<_>>();
        if let Some(last_error) = registry_error {
            contexts.push(RetainedContainerOwnerContext {
                owner: "containerCleanupRegistry",
                session_id: None,
                reason: "diagnostic-snapshot".to_string(),
                program: None,
                runtime_handle: None,
                state: "retained",
                in_flight: false,
                last_error: Some(last_error),
            });
        }
        contexts
    }

    pub(super) fn attempt(
        self: &Arc<Self>,
        entry: Arc<RetainedContainerCleanup>,
        epoch: u64,
        control: &ContainerRuntimeControl,
    ) {
        {
            let mut attempt = entry
                .attempt
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if attempt.in_flight || attempt.last_epoch == Some(epoch) {
                return;
            }
            attempt.in_flight = true;
            attempt.last_epoch = Some(epoch);
        }

        let result = catch_unwind(AssertUnwindSafe(|| {
            entry.remove_route_before_cleanup(control)?;
            if let Some(non_runtime) = entry
                .non_runtime
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take()
            {
                if let Some(path) = non_runtime.container_credential_path.as_deref() {
                    crate::pty::container_credentials::remove_copied(path);
                }
                if let (Some(manager), Some(client_id)) =
                    (non_runtime.token_manager, non_runtime.api_client_id)
                {
                    manager.revoke(&client_id);
                }
                drop(non_runtime.logical_resource_slot);
            }

            match (&entry.runtime, &entry.runtime_handle) {
                (Some(runtime), Some(handle)) => {
                    runtime.stop(handle, CONTAINER_STOP_TIMEOUT, control)
                }
                (None, Some(_)) => Err(AppError::Other(
                    "container runtime handle retained without a configured runtime".to_string(),
                )),
                (_, None) => Ok(()),
            }
        }));

        let terminal = match result {
            Ok(Ok(())) => true,
            Ok(Err(error)) => {
                let message = error.to_string();
                log::warn!(
                    "[container-transport] retained cleanup failed session={} reason={} \
                        state=retained error={}",
                    entry.session_id,
                    entry.reason,
                    crate::pty::container_runtime::redact_container_diagnostic_text(&message)
                );
                let mut attempt = entry
                    .attempt
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                attempt.in_flight = false;
                attempt.last_error = Some(message);
                false
            }
            Err(_) => {
                log::error!(
                    "[container-transport] retained cleanup panicked session={} reason={} \
                        state=retained",
                    entry.session_id,
                    entry.reason
                );
                let mut attempt = entry
                    .attempt
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                attempt.in_flight = false;
                attempt.last_error = Some("cleanup panicked".to_string());
                false
            }
        };

        if terminal {
            {
                let mut attempt = entry
                    .attempt
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                attempt.in_flight = false;
                attempt.last_error = None;
            }
            let mut entries = self
                .entries
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if entries
                .get(&entry.id)
                .is_some_and(|owned| Arc::ptr_eq(owned, &entry))
            {
                entries.remove(&entry.id);
            }
            log::debug!(
                "[container-transport] retained cleanup terminal session={} reason={}",
                entry.session_id,
                entry.reason
            );
        }
    }
}
