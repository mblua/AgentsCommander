use super::retained_cleanup::RetainedContainerCleanup;
use super::shutdown_work::ContainerShutdownProducer;
use super::{
    ContainerSessionState, ContainerShutdownReport, ContainerTransportBackend,
    RemovedSessionResources, CLEANUP_TASK_TIMEOUT, TRANSPORT_LOST_EXIT_CODE,
};
use crate::pty::container_runtime::{
    ContainerRuntime, ContainerRuntimeControl, ContainerRuntimeHandle,
};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::oneshot;

impl ContainerTransportBackend {
    pub(super) fn spawn_shutdown_owned_runtime_stop(
        &self,
        producer: &ContainerShutdownProducer,
        runtime: Arc<dyn ContainerRuntime>,
        handle: ContainerRuntimeHandle,
        reason: &'static str,
    ) {
        let session_id = handle.session_id;
        let resources = RemovedSessionResources {
            session_id,
            runtime_handle: Some(handle),
            api_client_id: None,
            logical_resource_slot: None,
            container_credential_path: None,
        };
        let entry = self
            .cleanup_ownership
            .retain(None, Some(runtime), resources, reason);
        self.schedule_retained_cleanup(entry, Some(producer));
        log::debug!(
            "[container-transport] tracking shutdown-owned stop session={} reason={}",
            session_id,
            reason
        );
    }

    pub(super) fn retain_removed_resources(
        &self,
        resources: RemovedSessionResources,
        reason: &'static str,
    ) -> Arc<RetainedContainerCleanup> {
        self.cleanup_ownership.retain(
            self.token_manager.clone(),
            self.runtime.clone(),
            resources,
            reason,
        )
    }

    pub(super) fn schedule_retained_cleanup(
        &self,
        entry: Arc<RetainedContainerCleanup>,
        producer: Option<&ContainerShutdownProducer>,
    ) {
        let epoch = self.cleanup_attempt_epoch.fetch_add(1, Ordering::Relaxed);
        let ownership = Arc::clone(&self.cleanup_ownership);
        let session_id = entry.session_id;
        let reason = entry.reason;
        let work = move |control: &ContainerRuntimeControl| {
            ownership.attempt(entry, epoch, control);
        };
        if let Some(producer) = producer {
            producer.spawn_owned(Some(session_id), reason, work);
        } else if let Some(producer) = self.shutdown_work.register_producer() {
            producer.spawn_owned(Some(session_id), reason, work);
        } else {
            log::warn!(
                "[container-transport] cleanup retained for the explicit global sweep without \
                    reopening sealed shutdown work session={} reason={} state=retained",
                session_id,
                reason
            );
        }
    }

    pub(crate) fn begin_shutdown(&self, deadline: Instant) -> bool {
        self.shutdown_work.begin_shutdown(deadline)
    }

    pub(crate) fn seal_and_drain_shutdown_work_blocking(
        &self,
        deadline: Instant,
    ) -> ContainerShutdownReport {
        self.shutdown_work.seal_and_drain_until(deadline)
    }

    pub(super) fn cleanup_removed_resources_async(
        &self,
        resources: RemovedSessionResources,
        reason: &'static str,
    ) {
        self.cleanup_removed_resources_async_owned(resources, reason, None);
    }

    pub(super) fn cleanup_removed_resources_async_owned(
        &self,
        resources: RemovedSessionResources,
        reason: &'static str,
        producer: Option<&ContainerShutdownProducer>,
    ) {
        let entry = self.retain_removed_resources(resources, reason);
        self.schedule_retained_cleanup(entry, producer);
    }

    pub(super) async fn cleanup_removed_resources_offloaded_owned(
        &self,
        resources: RemovedSessionResources,
        reason: &'static str,
        producer: Option<&ContainerShutdownProducer>,
    ) {
        let entry = self.retain_removed_resources(resources, reason);
        let (completed, completion) = oneshot::channel();
        let session_id = entry.session_id;
        let cleanup_epoch = self.cleanup_attempt_epoch.fetch_add(1, Ordering::Relaxed);
        let ownership = Arc::clone(&self.cleanup_ownership);
        let work = move |control: &ContainerRuntimeControl| {
            ownership.attempt(entry, cleanup_epoch, control);
            if completed.send(()).is_err() {
                log::debug!(
                    "[container-transport] {} cleanup completed after caller cancellation",
                    reason
                );
            }
        };
        if let Some(producer) = producer {
            producer.spawn_owned(Some(session_id), reason, work);
        } else if let Some(producer) = self.shutdown_work.register_producer() {
            producer.spawn_owned(Some(session_id), reason, work);
        } else {
            log::warn!(
                "[container-transport] cleanup retained for the explicit global sweep without \
                    reopening sealed shutdown work session={} reason={} state=retained",
                session_id,
                reason
            );
        }
        match tokio::time::timeout(CLEANUP_TASK_TIMEOUT, completion).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                log::warn!(
                    "[container-transport] {} cleanup task failed: {}",
                    reason,
                    error
                );
            }
            Err(_) => {
                log::warn!(
                    "[container-transport] {} cleanup exceeded {:?}; continuing",
                    reason,
                    CLEANUP_TASK_TIMEOUT
                );
            }
        }
    }

    pub async fn reap_expired_pending_sessions(&self) -> usize {
        let expired = {
            let sessions = self.sessions.lock().unwrap();
            let now = Instant::now();
            sessions
                .iter()
                .filter_map(|(id, state)| match state {
                    ContainerSessionState::Pending(pending) if pending.ticket_expires_at <= now => {
                        Some(*id)
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
        };

        let mut reaped = 0;
        for session_id in expired {
            if let Some(resources) = self.remove_session_state(session_id) {
                self.cleanup_removed_resources_async(resources, "pending-reaper");
                self.remove_route(session_id);
                if let Some(sender) = self.lifecycle_sender.as_ref() {
                    if let Err(error) = sender
                        .route_lost(session_id, TRANSPORT_LOST_EXIT_CODE)
                        .await
                    {
                        log::warn!(
                            "[container-transport] pending-reaper reconciliation failed \
                                session={}: {}",
                            session_id,
                            error
                        );
                    }
                }
                reaped += 1;
            }
        }
        reaped
    }
}
