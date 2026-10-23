use super::*;

impl MessageStore {
    pub async fn enqueue_pty_input_offloaded(
        &self,
        req: PtyInputEnqueueRequest,
    ) -> Result<PtyInputEnqueueResult, MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.enqueue_pty_input(req))
            .await
            .map_err(|error| MessageStoreError::BlockingTask(error.to_string()))?
    }

    pub async fn record_host_pty_input_rejection_offloaded(
        &self,
        request: HostPtyInputRejectionRequest,
        now: DateTime<Utc>,
    ) -> Result<crate::phone::types::PtyInputResult, MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.record_host_pty_input_rejection(request, now))
            .await
            .map_err(|error| MessageStoreError::BlockingTask(error.to_string()))?
    }

    pub async fn query_pty_input_offloaded(
        &self,
        sender_fqn: String,
        op_id: String,
        sender_incarnation_fingerprint: String,
    ) -> Result<Option<crate::phone::types::PtyInputResult>, MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            store.query_pty_input(&sender_fqn, &op_id, &sender_incarnation_fingerprint)
        })
        .await
        .map_err(|error| MessageStoreError::BlockingTask(error.to_string()))?
    }

    pub async fn recover_pty_input_runtime_offloaded(
        &self,
        active: HashSet<String>,
        now: DateTime<Utc>,
    ) -> Result<usize, MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.recover_pty_input_runtime(&active, now))
            .await
            .map_err(|error| MessageStoreError::BlockingTask(error.to_string()))?
    }

    pub(crate) async fn due_container_pty_input_candidates_fair_offloaded(
        &self,
        now: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<DuePtyInputCandidate>, MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            store.due_container_pty_input_candidates_fair(now, limit)
        })
        .await
        .map_err(|error| MessageStoreError::BlockingTask(error.to_string()))?
    }

    pub async fn claim_pty_input_offloaded(
        &self,
        source_plane: crate::phone::types::PtyInputSourcePlane,
        exact_injection_id: Option<String>,
        lease_owner: String,
        now: DateTime<Utc>,
    ) -> Result<Option<ClaimedPtyInputOperation>, MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            store.claim_pty_input(
                source_plane,
                exact_injection_id.as_deref(),
                &lease_owner,
                now,
            )
        })
        .await
        .map_err(|error| MessageStoreError::BlockingTask(error.to_string()))?
    }

    pub async fn renew_pty_input_lease_offloaded(
        &self,
        injection_id: String,
        lease_owner: String,
        now: DateTime<Utc>,
    ) -> Result<bool, MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            store.renew_pty_input_lease(&injection_id, &lease_owner, now)
        })
        .await
        .map_err(|error| MessageStoreError::BlockingTask(error.to_string()))?
    }

    pub async fn begin_pty_actuating_offloaded(
        &self,
        injection_id: String,
        lease_owner: String,
        selected_session_id: String,
        selected_backend: String,
        now: DateTime<Utc>,
    ) -> Result<Vec<u8>, MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            store.begin_pty_actuating(
                &injection_id,
                &lease_owner,
                &selected_session_id,
                &selected_backend,
                now,
            )
        })
        .await
        .map_err(|error| MessageStoreError::BlockingTask(error.to_string()))?
    }

    pub async fn retry_pty_input_offloaded(
        &self,
        injection_id: String,
        lease_owner: String,
        code: crate::phone::types::PtyInputReasonCode,
        now: DateTime<Utc>,
    ) -> Result<(), MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            store.retry_pty_input(&injection_id, &lease_owner, code, now)
        })
        .await
        .map_err(|error| MessageStoreError::BlockingTask(error.to_string()))?
    }

    pub async fn terminalize_pty_input_offloaded(
        &self,
        injection_id: String,
        status: crate::phone::types::PtyInputPublicStatus,
        reason: Option<crate::phone::types::PtyInputReasonCode>,
        now: DateTime<Utc>,
    ) -> Result<crate::phone::types::PtyInputResult, MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            store.terminalize_pty_input(&injection_id, status, reason, now)
        })
        .await
        .map_err(|error| MessageStoreError::BlockingTask(error.to_string()))?
    }

    pub async fn query_pty_input_by_injection_offloaded(
        &self,
        injection_id: String,
    ) -> Result<Option<crate::phone::types::PtyInputResult>, MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.query_pty_input_by_injection(&injection_id))
            .await
            .map_err(|error| MessageStoreError::BlockingTask(error.to_string()))?
    }

    pub async fn host_confirmation_tag_offloaded(
        &self,
        injection_id: String,
    ) -> Result<Option<String>, MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.host_confirmation_tag(&injection_id))
            .await
            .map_err(|error| MessageStoreError::BlockingTask(error.to_string()))?
    }

    pub async fn mark_host_artifact_offloaded(
        &self,
        injection_id: String,
        now: DateTime<Utc>,
    ) -> Result<(), MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.mark_host_artifact(&injection_id, now))
            .await
            .map_err(|error| MessageStoreError::BlockingTask(error.to_string()))?
    }

    pub async fn compact_pty_terminal_maintenance_offloaded(
        &self,
        now: DateTime<Utc>,
        limit: usize,
    ) -> Result<usize, MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.compact_pty_terminal_maintenance(now, limit))
            .await
            .map_err(|error| MessageStoreError::BlockingTask(error.to_string()))?
    }
}
