use super::root_key;
use super::{
    resources_from_state, ActiveSession, AttachingSession, ContainerSessionState,
    ContainerTransportBackend, HostToBridgeFrame, HostToBridgeTextFrame, RemovedSessionResources,
    RouteRemovalError, TransportAttachError, TransportTicketError, CLEANUP_TASK_TIMEOUT,
    CONTAINER_SHUTDOWN_POLL, MAX_TRANSPORT_FRAME_BYTES, TRANSPORT_LOST_EXIT_CODE,
    TRANSPORT_PROTOCOL_VERSION,
};
use crate::errors::AppError;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::TryLockError;
use std::time::Instant;
use tokio::sync::mpsc;
use uuid::Uuid;

impl ContainerTransportBackend {
    pub(crate) fn consume_ticket(
        &self,
        session_id: Uuid,
        bound_root: &str,
        ticket: &str,
    ) -> Result<(), TransportTicketError> {
        let bound_key = root_key(bound_root);
        let ticket_hash = crate::api::auth::hash_token(ticket);
        let now = Instant::now();

        let mut sessions = self.sessions.lock().unwrap();
        let Some(state) = sessions.remove(&session_id) else {
            return Err(TransportTicketError::Invalid);
        };

        let pending = match state {
            ContainerSessionState::Pending(pending) => pending,
            other => {
                sessions.insert(session_id, other);
                return Err(TransportTicketError::Invalid);
            }
        };

        if pending.root_key != bound_key
            || pending.ticket_expires_at <= now
            || !crate::api::auth::constant_time_eq(&pending.ticket_hash, &ticket_hash)
        {
            sessions.insert(session_id, ContainerSessionState::Pending(pending));
            return Err(TransportTicketError::Invalid);
        }

        let attaching = AttachingSession {
            root_key: pending.root_key,
            output_target: pending.output_target,
            idle_tuning: pending.idle_tuning,
            rows: pending.rows,
            cols: pending.cols,
            runtime_handle: pending.runtime_handle,
            api_client_id: pending.api_client_id,
            credential_binding: pending.credential_binding,
            logical_resource_slot: pending.logical_resource_slot,
            attach_notify: pending.attach_notify,
            container_credential_path: pending.container_credential_path,
        };
        sessions.insert(session_id, ContainerSessionState::Attaching(attaching));
        Ok(())
    }

    pub(crate) fn complete_hello(
        &self,
        session_id: Uuid,
        bridge_root: &str,
        sender: mpsc::Sender<HostToBridgeFrame>,
    ) -> Result<(), TransportAttachError> {
        let bridge_key = root_key(bridge_root);
        let attach = {
            let mut sessions = self.sessions.lock().unwrap();
            let Some(state) = sessions.remove(&session_id) else {
                return Err(TransportAttachError::Invalid);
            };

            let attach = match state {
                ContainerSessionState::Attaching(attach) => attach,
                other => {
                    sessions.insert(session_id, other);
                    return Err(TransportAttachError::Invalid);
                }
            };
            if attach.root_key != bridge_key {
                sessions.insert(session_id, ContainerSessionState::Attaching(attach));
                return Err(TransportAttachError::Invalid);
            }

            attach
        };

        let AttachingSession {
            output_target,
            idle_tuning,
            rows,
            cols,
            runtime_handle,
            api_client_id,
            credential_binding,
            logical_resource_slot,
            attach_notify,
            container_credential_path,
            ..
        } = attach;
        if self
            .fanout
            .register_session(session_id, idle_tuning, rows, cols, output_target)
            .is_err()
        {
            return Err(TransportAttachError::Invalid);
        }
        self.sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(
                session_id,
                ContainerSessionState::Active(ActiveSession {
                    sender,
                    rows,
                    cols,
                    runtime_handle,
                    api_client_id,
                    credential_binding,
                    logical_resource_slot,
                    container_credential_path,
                }),
            );
        if let Some(notify) = attach_notify {
            notify.notify_waiters();
        }
        log::info!(
            "[container-transport] attached bridge for session {}",
            session_id
        );
        Ok(())
    }

    pub(crate) fn handle_bridge_output(
        &self,
        session_id: Uuid,
        data: Vec<u8>,
    ) -> Result<(), AppError> {
        if data.len() > MAX_TRANSPORT_FRAME_BYTES {
            return Err(AppError::PtyError(
                "container transport frame exceeds 64 KiB".to_string(),
            ));
        }

        let active = {
            let sessions = self.sessions.lock().unwrap();
            match sessions.get(&session_id) {
                Some(ContainerSessionState::Active(_)) => true,
                _ => return Err(AppError::SessionNotFound(session_id.to_string())),
            }
        };
        if !active {
            return Err(AppError::SessionNotFound(session_id.to_string()));
        }
        let token = self
            .fanout
            .registration_token_for_session(session_id)
            .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))?;

        let session_id_str = session_id.to_string();
        self.fanout.handle_output(&token, &session_id_str, data);
        Ok(())
    }

    pub(crate) async fn handle_bridge_exit(&self, session_id: Uuid, code: i32) {
        self.close_transport(session_id, Some(code)).await;
    }

    pub(crate) async fn handle_bridge_disconnect(&self, session_id: Uuid) {
        self.close_transport(session_id, Some(TRANSPORT_LOST_EXIT_CODE))
            .await;
    }

    pub(crate) async fn handle_handshake_failed(&self, session_id: Uuid) {
        self.close_transport(session_id, Some(TRANSPORT_LOST_EXIT_CODE))
            .await;
    }

    pub(crate) fn send_ping(&self, session_id: Uuid) -> Result<(), AppError> {
        self.send_text_frame(
            session_id,
            HostToBridgeTextFrame::Ping {
                version: TRANSPORT_PROTOCOL_VERSION,
            },
        )
    }

    pub(super) fn send_text_frame(
        &self,
        session_id: Uuid,
        frame: HostToBridgeTextFrame,
    ) -> Result<(), AppError> {
        self.send_frame(session_id, HostToBridgeFrame::Text(frame))
    }

    pub(super) fn send_frame(
        &self,
        session_id: Uuid,
        frame: HostToBridgeFrame,
    ) -> Result<(), AppError> {
        let sender = {
            let sessions = self.sessions.lock().unwrap();
            match sessions.get(&session_id) {
                Some(ContainerSessionState::Active(active)) => active.sender.clone(),
                _ => return Err(AppError::SessionNotFound(session_id.to_string())),
            }
        };

        match sender.try_send(frame) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.close_transport_from_sync(session_id, TRANSPORT_LOST_EXIT_CODE);
                Err(AppError::PtyError(
                    "container transport outbound queue full".to_string(),
                ))
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.close_transport_from_sync(session_id, TRANSPORT_LOST_EXIT_CODE);
                Err(AppError::SessionNotFound(session_id.to_string()))
            }
        }
    }

    pub(super) async fn close_transport(&self, session_id: Uuid, exit_code: Option<i32>) {
        let resources = self.remove_session_state(session_id);
        let Some(resources) = resources else {
            return;
        };

        self.cleanup_removed_resources_async(resources, "transport-close");
        self.remove_route(session_id);
        if let Some(code) = exit_code {
            if let Some(sender) = self.lifecycle_sender.as_ref() {
                if let Err(error) = sender.route_lost(session_id, code).await {
                    log::warn!(
                        "[container-transport] route-loss reconciliation failed session={}: {}",
                        session_id,
                        error
                    );
                }
            } else {
                log::debug!(
                    "[container-transport] no lifecycle sender installed session={}",
                    session_id
                );
            }
        }
    }

    pub(super) fn close_transport_from_sync(&self, session_id: Uuid, exit_code: i32) {
        let resources = self.remove_session_state(session_id);
        let Some(resources) = resources else {
            return;
        };

        self.cleanup_removed_resources_async(resources, "transport-sync-close");
        let remover = match self.route_remover.try_lock() {
            Ok(route_remover) => route_remover.clone(),
            Err(TryLockError::Poisoned(_)) => {
                log::error!(
                    "[container-transport] deferred route-remover registry lock poisoned \
                        session={} state=retained",
                    session_id
                );
                None
            }
            Err(TryLockError::WouldBlock) => {
                log::warn!(
                    "[container-transport] deferred route-remover registry lock unavailable \
                        session={} state=retained",
                    session_id
                );
                None
            }
        };
        let lifecycle_sender = self.lifecycle_sender.clone();
        tauri::async_runtime::spawn(async move {
            if let Some(remove) = remover {
                let deadline = Instant::now() + CLEANUP_TASK_TIMEOUT;
                match catch_unwind(AssertUnwindSafe(|| remove(session_id, deadline))) {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => log::warn!(
                        "[container-transport] deferred sync route removal failed session={} \
                            error={}",
                        session_id,
                        error
                    ),
                    Err(_) => log::error!(
                        "[container-transport] deferred sync route removal panicked session={}",
                        session_id
                    ),
                }
            }
            if let Some(sender) = lifecycle_sender {
                if let Err(error) = sender.route_lost(session_id, exit_code).await {
                    log::warn!(
                        "[container-transport] deferred sync route-loss reconciliation failed \
                            session={}: {}",
                        session_id,
                        error
                    );
                }
            }
        });
    }

    pub(super) fn remove_session_state(&self, session_id: Uuid) -> Option<RemovedSessionResources> {
        let removed = self
            .sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&session_id);
        if removed.is_some() {
            self.fanout.remove_session(session_id);
        }
        removed.map(|state| resources_from_state(session_id, state))
    }

    pub(super) fn remove_session_state_until(
        &self,
        session_id: Uuid,
        deadline: Instant,
    ) -> Result<Option<RemovedSessionResources>, RouteRemovalError> {
        let removed = loop {
            match self.sessions.try_lock() {
                Ok(mut sessions) => break sessions.remove(&session_id),
                Err(TryLockError::Poisoned(_)) => {
                    return Err(RouteRemovalError::LockPoisoned("containerSessionRegistry"))
                }
                Err(TryLockError::WouldBlock) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err(RouteRemovalError::Deadline("containerSessionRegistry"));
                    }
                    std::thread::sleep(CONTAINER_SHUTDOWN_POLL.min(remaining));
                }
            }
        };
        if removed.is_some() {
            self.fanout.remove_session(session_id);
        }
        Ok(removed.map(|state| resources_from_state(session_id, state)))
    }

    /// #930 F1 - after copy-in, if a concurrent teardown already removed the
    /// session from the map, that teardown's `remove_copied` ran before the file
    /// physically existed (idempotent no-op), so delete the now-orphaned file
    /// here. The shared `sessions` mutex serializes this check against teardown,
    /// so every interleaving converges on a deleted file. Body is identical to
    /// the previous inline recheck; extracted only so the race is unit-testable.
    pub(super) fn remove_credential_if_orphaned(
        &self,
        id: Uuid,
        plan: &crate::pty::container_credentials::ContainerCredentialPlan,
    ) {
        if !self.sessions.lock().unwrap().contains_key(&id) {
            crate::pty::container_credentials::remove_copied(&plan.dest);
        }
    }

    pub(super) fn remove_route(&self, session_id: Uuid) {
        let deadline = Instant::now() + CLEANUP_TASK_TIMEOUT;
        let remover = match self.route_remover_until(deadline) {
            Ok(remover) => remover,
            Err(error) => {
                log::warn!(
                    "[container-transport] route removal unavailable session={} error={}",
                    session_id,
                    error
                );
                return;
            }
        };
        if let Some(remove) = remover {
            match catch_unwind(AssertUnwindSafe(|| remove(session_id, deadline))) {
                Ok(Ok(())) => {}
                Ok(Err(error)) => log::warn!(
                    "[container-transport] route removal failed session={} error={}",
                    session_id,
                    error
                ),
                Err(_) => log::error!(
                    "[container-transport] route removal panicked session={}",
                    session_id
                ),
            }
        }
    }
}
