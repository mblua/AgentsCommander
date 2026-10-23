use super::sha256_hex;

#[derive(Debug, Clone)]
pub struct EnqueueRequest {
    pub sender_fqn: String,
    pub target_fqn: String,
    pub op_id: String,
    pub content_type: String,
    pub body: String,
    pub source_plane: String,
    pub source_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnqueueResult {
    pub message_id: String,
    pub sender_fqn: String,
    pub target_fqn: String,
    pub op_id: String,
    pub status: String,
    pub duplicate: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeasedMessage {
    pub message_id: String,
    pub sender_fqn: String,
    pub target_fqn: String,
    pub op_id: String,
    pub content_type: String,
    pub body: String,
    pub attempt: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DuePtyInputCandidate {
    pub injection_id: String,
    pub target_fqn: String,
}

pub struct PtyInputEnqueueRequest {
    pub injection_id: String,
    pub sender_fqn: String,
    pub target_fqn: String,
    pub op_id: String,
    pub nonce_sha256: String,
    pub request_fingerprint: String,
    pub confirmation_tag: Option<String>,
    pub requested_agent_id: Option<String>,
    pub payload: Vec<u8>,
    pub source_plane: crate::phone::types::PtyInputSourcePlane,
    pub sender_incarnation_fingerprint: String,
    pub sender_identity_fingerprint: String,
    pub target_identity_fingerprint: String,
    pub authority_session_id: String,
    pub authority_client_id: Option<String>,
    pub authority_client_generation: Option<String>,
    pub issued_at: String,
    pub expires_at: String,
}

impl std::fmt::Debug for PtyInputEnqueueRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PtyInputEnqueueRequest")
            .field("injection_id", &self.injection_id)
            .field("sender_fqn", &self.sender_fqn)
            .field("target_fqn", &self.target_fqn)
            .field("op_id", &self.op_id)
            .field("payload_bytes", &self.payload.len())
            .field("payload_sha256", &sha256_hex(&self.payload))
            .field("source_plane", &self.source_plane)
            .finish()
    }
}

pub struct HostPtyInputRejectionRequest {
    pub injection_id: String,
    pub sender_fqn: String,
    pub target_fqn: String,
    pub op_id: String,
    pub nonce_sha256: String,
    pub request_fingerprint: String,
    pub confirmation_tag: String,
    pub sender_incarnation_fingerprint: String,
    pub payload_sha256: String,
    pub payload_bytes: u64,
    pub issued_at: String,
    pub expires_at: String,
    pub reason: crate::phone::types::PtyInputReasonCode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtyInputEnqueueResult {
    pub result: crate::phone::types::PtyInputResult,
    pub duplicate: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedPtyInputOperation {
    pub injection_id: String,
    pub sender_fqn: String,
    pub target_fqn: String,
    pub op_id: String,
    pub source_plane: crate::phone::types::PtyInputSourcePlane,
    pub lease_owner: String,
    pub attempt: i64,
    pub authority_session_id: String,
    pub authority_client_id: Option<String>,
    pub authority_client_generation: Option<String>,
    pub sender_identity_fingerprint: String,
    pub target_identity_fingerprint: String,
    pub requested_agent_id: Option<String>,
    pub expires_at: String,
}
