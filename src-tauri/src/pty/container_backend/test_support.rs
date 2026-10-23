use super::{
    ActiveSession, ContainerCredentialBinding, ContainerSessionState, ContainerTransportBackend,
    HostToBridgeFrame,
};
use crate::pty::container_runtime::ContainerRuntimeHandle;
use crate::pty::output::PtyOutputTarget;
use tokio::sync::mpsc;
use uuid::Uuid;

impl ContainerTransportBackend {
    pub(crate) fn last_issued_ticket_for_test(&self, session_id: Uuid) -> Option<String> {
        self.issued_tickets_for_test
            .lock()
            .unwrap()
            .get(&session_id)
            .cloned()
    }

    pub(crate) fn insert_active_runtime_handle_for_test(
        &self,
        handle: ContainerRuntimeHandle,
    ) -> mpsc::Receiver<HostToBridgeFrame> {
        let session_id = handle.session_id;
        let (sender, receiver) = mpsc::channel(8);
        self.sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(
                session_id,
                ContainerSessionState::Active(ActiveSession {
                    sender,
                    rows: 30,
                    cols: 120,
                    runtime_handle: Some(handle),
                    api_client_id: None,
                    credential_binding: None,
                    logical_resource_slot: None,
                    container_credential_path: None,
                }),
            );
        receiver
    }

    pub(crate) fn install_protocol_snapshot_session_for_test(
        &self,
        session_id: Uuid,
        binding: ContainerCredentialBinding,
        rows: u16,
        cols: u16,
        output: Vec<u8>,
    ) -> mpsc::Receiver<HostToBridgeFrame> {
        let (sender, receiver) = mpsc::channel(8);
        self.sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(
                session_id,
                ContainerSessionState::Active(ActiveSession {
                    sender,
                    rows,
                    cols,
                    runtime_handle: None,
                    api_client_id: Some(binding.client_id.clone()),
                    credential_binding: Some(binding),
                    logical_resource_slot: None,
                    container_credential_path: None,
                }),
            );
        let target = PtyOutputTarget::noop();
        self.fanout
            .register_session(
                session_id,
                crate::session::profile::IdleTuning::DEFAULT,
                rows,
                cols,
                target,
            )
            .expect("register protocol snapshot session");
        self.handle_bridge_output(session_id, output)
            .expect("protocol snapshot test output");
        receiver
    }

    pub(crate) fn contains_transport_state_for_test(&self, session_id: Uuid) -> bool {
        self.sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains_key(&session_id)
    }

    pub(crate) fn detached_cleanup_count_for_test(&self) -> usize {
        self.shutdown_work.snapshot().2
    }

    pub(crate) fn shutdown_work_state_for_test(&self) -> (bool, usize, usize) {
        self.shutdown_work.snapshot()
    }

    pub(crate) fn shutdown_observation_for_test(&self) -> (bool, usize, usize, usize) {
        self.shutdown_work.observation()
    }

    pub(crate) fn inject_shutdown_worker_spawn_failure_for_test(&self) {
        self.shutdown_work.inject_worker_spawn_failure();
    }

    pub(crate) fn clear_shutdown_worker_spawn_failure_for_test(&self) {
        self.shutdown_work.clear_worker_spawn_failure();
    }

    pub(crate) fn shutdown_worker_count_for_test(&self) -> usize {
        self.shutdown_work.worker_count()
    }

    pub(crate) fn retained_cleanup_sessions_for_test(&self) -> Vec<Uuid> {
        let mut sessions = self
            .cleanup_ownership
            .entries()
            .into_iter()
            .map(|entry| entry.session_id)
            .collect::<Vec<_>>();
        sessions.sort();
        sessions
    }

    pub(crate) fn retained_runtime_cleanup_sessions_for_test(&self) -> Vec<Uuid> {
        let mut sessions = self
            .cleanup_ownership
            .entries()
            .into_iter()
            .filter(|entry| entry.runtime_handle.is_some())
            .map(|entry| entry.session_id)
            .collect::<Vec<_>>();
        sessions.sort();
        sessions
    }

    pub(crate) fn retained_cleanup_contexts_for_test(&self) -> Vec<String> {
        self.cleanup_ownership
            .contexts()
            .into_iter()
            .map(|context| context.diagnostic())
            .collect()
    }

    pub(crate) fn set_runtime_settings_for_test(
        &mut self,
        settings: crate::config::settings::AppSettings,
    ) {
        self.runtime_settings_override = Some(settings);
    }
}
