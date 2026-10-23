use super::REQUEST_KIND;
use super::REQUEST_VERSION;
use serde::Deserialize;
use serde::Serialize;
use terminal_snapshot_renderer::TerminalSnapshotFormat;
use terminal_snapshot_renderer::TerminalSnapshotReasonCode;
use uuid::Uuid;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct HostTerminalSnapshotRequest {
    pub kind: String,
    pub version: u32,
    pub request_id: String,
    pub token: String,
    pub from: String,
    pub to: String,
    pub format: TerminalSnapshotFormat,
    pub issued_at: String,
    pub expires_at: String,
    pub nonce: String,
    pub confirmation_tag: String,
}

impl std::fmt::Debug for HostTerminalSnapshotRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HostTerminalSnapshotRequest")
            .field("version", &self.version)
            .field("format", &self.format)
            .finish_non_exhaustive()
    }
}

impl HostTerminalSnapshotRequest {
    pub(super) fn validate_correlation(
        &self,
        filename_request_id: &str,
    ) -> Result<Uuid, TerminalSnapshotReasonCode> {
        let request_id = terminal_snapshot_renderer::validate_uuid(&self.request_id, Some(4))
            .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?;
        if self.request_id != filename_request_id {
            return Err(TerminalSnapshotReasonCode::InvalidRequest);
        }
        terminal_snapshot_renderer::validate_hex(&self.nonce, 64)
            .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?;
        terminal_snapshot_renderer::validate_hex(&self.confirmation_tag, 64)
            .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?;
        if confirmation_tag(self) != self.confirmation_tag {
            return Err(TerminalSnapshotReasonCode::InvalidRequest);
        }
        Ok(request_id)
    }

    pub(crate) fn validate(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<(Uuid, Uuid, chrono::DateTime<chrono::Utc>), TerminalSnapshotReasonCode> {
        if self.kind != REQUEST_KIND || self.version != REQUEST_VERSION {
            return Err(TerminalSnapshotReasonCode::InvalidRequest);
        }
        let request_id = terminal_snapshot_renderer::validate_uuid(&self.request_id, Some(4))
            .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?;
        let token = terminal_snapshot_renderer::validate_uuid(&self.token, Some(4))
            .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?;
        terminal_snapshot_renderer::validate_requester_identity(&self.from, true)
            .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?;
        terminal_snapshot_renderer::validate_target_syntax(&self.to)
            .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?;
        terminal_snapshot_renderer::validate_hex(&self.nonce, 64)
            .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?;
        terminal_snapshot_renderer::validate_hex(&self.confirmation_tag, 64)
            .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?;
        let issued = terminal_snapshot_renderer::validate_timestamp(&self.issued_at)
            .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?;
        let expires = terminal_snapshot_renderer::validate_timestamp(&self.expires_at)
            .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?;
        let lifetime = expires.signed_duration_since(issued);
        let lifetime_ms = lifetime.num_milliseconds();
        if !(5_000..=30_000).contains(&lifetime_ms)
            || lifetime_ms % 1_000 != 0
            || issued > now + chrono::Duration::seconds(5)
            || now >= expires
            || confirmation_tag(self) != self.confirmation_tag
        {
            return Err(TerminalSnapshotReasonCode::InvalidRequest);
        }
        Ok((request_id, token, expires))
    }
}

pub(crate) fn confirmation_tag(request: &HostTerminalSnapshotRequest) -> String {
    use sha2::{Digest, Sha256};
    let format = request.format.to_string();
    let fields: [&[u8]; 8] = [
        b"ac-terminal-snapshot-confirmation-v1",
        request.request_id.as_bytes(),
        request.nonce.as_bytes(),
        request.from.as_bytes(),
        request.to.as_bytes(),
        format.as_bytes(),
        request.issued_at.as_bytes(),
        request.expires_at.as_bytes(),
    ];
    let mut digest = Sha256::new();
    for field in fields {
        digest.update((field.len() as u64).to_be_bytes());
        digest.update(field);
    }
    format!("{:x}", digest.finalize())
}
