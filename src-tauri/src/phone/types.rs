use std::fmt;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::{Uuid, Version};

/// Version of the privileged PTY-input wire and result contracts.
pub const PTY_INPUT_VERSION: u32 = 1;
/// Host request lifetime from `issuedAt`.
pub const PTY_INPUT_TTL_SECS: i64 = 10 * 60;
/// Maximum accepted future skew for a host request.
pub const PTY_INPUT_FUTURE_SKEW_SECS: i64 = 30;
/// Bound for a raw host request, including worst-case JSON escaping.
pub const PTY_INPUT_HOST_ENVELOPE_MAX_BYTES: usize =
    6 * crate::pty::backend::PTY_INPUT_MAX_BYTES + (16 * 1024);
/// Bound for a metadata-only marker, result, or HTTP response.
pub const PTY_INPUT_METADATA_MAX_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PtyInputEnterMode {
    AgentSubmit,
    #[serde(other)]
    Unsupported,
}

impl fmt::Debug for PtyInputEnterMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::AgentSubmit => "AgentSubmit",
            Self::Unsupported => "Unsupported",
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PtyInputSourcePlane {
    HostCli,
    ContainerApi,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PtyInputPublicStatus {
    Queued,
    Actuating,
    Injected,
    Rejected,
    Indeterminate,
}

impl PtyInputPublicStatus {
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Injected | Self::Rejected | Self::Indeterminate)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PtyInputReasonCode {
    InvalidEnvelope,
    MixedPayload,
    UnsupportedVersion,
    InvalidEnterMode,
    InvalidId,
    InvalidNonce,
    InvalidTimestamp,
    Expired,
    InvalidTarget,
    InvalidText,
    PayloadTooLarge,
    IdempotencyConflict,
    CapacityExceeded,
    SessionTokenRequired,
    InvalidSessionToken,
    AmbiguousSessionToken,
    SenderSessionNotLive,
    SenderBackendNotLocal,
    SenderIdentityInvalid,
    SenderNotCoordinator,
    RootIdentityInvalid,
    TargetNotMember,
    TargetIsCoordinator,
    TargetOutOfScope,
    UnsafePath,
    ApiScopeRequired,
    ApiClientUnbound,
    ApiClientStale,
    ApiBindingMismatch,
    AuthorityChanged,
    Busy,
    ResizeUnsettled,
    UntrackedReadiness,
    UnsupportedSession,
    NonpersistentLiveSession,
    InconsistentSession,
    UnsupportedProfile,
    ReadinessTimeout,
    StoreCorrupt,
    RestoreInProgress,
    PurgeInProgress,
    SessionRace,
    LeaseLost,
    SpawnFailedSafe,
    StoreTransient,
    FinalRevalidationFailed,
    TextWriteFailed,
    RequiredEnterFailed,
    DaemonRestartAfterActuation,
    RuntimeActuationOrphan,
    TerminalStoreFailed,
    RedundantEnterFailed,
    BoundaryMetadataFailed,
    ArtifactUnclaimed,
}

pub const fn pty_input_reason_code_name(code: PtyInputReasonCode) -> &'static str {
    use PtyInputReasonCode as C;
    match code {
        C::InvalidEnvelope => "invalid_envelope",
        C::MixedPayload => "mixed_payload",
        C::UnsupportedVersion => "unsupported_version",
        C::InvalidEnterMode => "invalid_enter_mode",
        C::InvalidId => "invalid_id",
        C::InvalidNonce => "invalid_nonce",
        C::InvalidTimestamp => "invalid_timestamp",
        C::Expired => "expired",
        C::InvalidTarget => "invalid_target",
        C::InvalidText => "invalid_text",
        C::PayloadTooLarge => "payload_too_large",
        C::IdempotencyConflict => "idempotency_conflict",
        C::CapacityExceeded => "capacity_exceeded",
        C::SessionTokenRequired => "session_token_required",
        C::InvalidSessionToken => "invalid_session_token",
        C::AmbiguousSessionToken => "ambiguous_session_token",
        C::SenderSessionNotLive => "sender_session_not_live",
        C::SenderBackendNotLocal => "sender_backend_not_local",
        C::SenderIdentityInvalid => "sender_identity_invalid",
        C::SenderNotCoordinator => "sender_not_coordinator",
        C::RootIdentityInvalid => "root_identity_invalid",
        C::TargetNotMember => "target_not_member",
        C::TargetIsCoordinator => "target_is_coordinator",
        C::TargetOutOfScope => "target_out_of_scope",
        C::UnsafePath => "unsafe_path",
        C::ApiScopeRequired => "api_scope_required",
        C::ApiClientUnbound => "api_client_unbound",
        C::ApiClientStale => "api_client_stale",
        C::ApiBindingMismatch => "api_binding_mismatch",
        C::AuthorityChanged => "authority_changed",
        C::Busy => "busy",
        C::ResizeUnsettled => "resize_unsettled",
        C::UntrackedReadiness => "untracked_readiness",
        C::UnsupportedSession => "unsupported_session",
        C::NonpersistentLiveSession => "nonpersistent_live_session",
        C::InconsistentSession => "inconsistent_session",
        C::UnsupportedProfile => "unsupported_profile",
        C::ReadinessTimeout => "readiness_timeout",
        C::StoreCorrupt => "store_corrupt",
        C::RestoreInProgress => "restore_in_progress",
        C::PurgeInProgress => "purge_in_progress",
        C::SessionRace => "session_race",
        C::LeaseLost => "lease_lost",
        C::SpawnFailedSafe => "spawn_failed_safe",
        C::StoreTransient => "store_transient",
        C::FinalRevalidationFailed => "final_revalidation_failed",
        C::TextWriteFailed => "text_write_failed",
        C::RequiredEnterFailed => "required_enter_failed",
        C::DaemonRestartAfterActuation => "daemon_restart_after_actuation",
        C::RuntimeActuationOrphan => "runtime_actuation_orphan",
        C::TerminalStoreFailed => "terminal_store_failed",
        C::RedundantEnterFailed => "redundant_enter_failed",
        C::BoundaryMetadataFailed => "boundary_metadata_failed",
        C::ArtifactUnclaimed => "artifact_unclaimed",
    }
}

/// Fixed public text for a PTY-input reason. No caller, OS, parser, or backend
/// error text is retained by this mapping.
pub const fn safe_detail(code: PtyInputReasonCode) -> &'static str {
    use PtyInputReasonCode as C;
    match code {
        C::InvalidEnvelope => "The PTY input envelope is invalid.",
        C::MixedPayload => "PTY input cannot be mixed with another payload type.",
        C::UnsupportedVersion => "The PTY input contract version is unsupported.",
        C::InvalidEnterMode => "The PTY input Enter mode is invalid.",
        C::InvalidId => "A canonical UUID version 4 operation ID is required.",
        C::InvalidNonce => "A distinct canonical UUID version 4 nonce is required.",
        C::InvalidTimestamp => "The PTY input timestamp is invalid.",
        C::Expired => "The PTY input operation expired before actuation.",
        C::InvalidTarget => "An exact canonical room target is required.",
        C::InvalidText => "The PTY input text contains a forbidden scalar or is empty.",
        C::PayloadTooLarge => "The PTY input text exceeds 65,536 UTF-8 bytes.",
        C::IdempotencyConflict => "The operation ID was already used with different semantics.",
        C::CapacityExceeded => "The privileged PTY input queue is at capacity.",
        C::SessionTokenRequired => "A live session token is required for PTY input.",
        C::InvalidSessionToken => "The PTY input session token is invalid or stale.",
        C::AmbiguousSessionToken => "The PTY input session token is not unique.",
        C::SenderSessionNotLive => "The sender session is not live.",
        C::SenderBackendNotLocal => "The filesystem PTY input plane requires a local sender.",
        C::SenderIdentityInvalid => "The sender replica identity could not be verified.",
        C::SenderNotCoordinator => "The sender is not the verified room orchestrator.",
        C::RootIdentityInvalid => "The live Root Agent identity could not be verified.",
        C::TargetNotMember => "The target is not a verified member of the sender room.",
        C::TargetIsCoordinator => "An orchestrator cannot target an orchestrator on this route.",
        C::TargetOutOfScope => "The target is outside the sender's PTY input authority.",
        C::UnsafePath => "A privileged path failed confinement or link safety checks.",
        C::ApiScopeRequired => "The API token is not scoped for PTY input.",
        C::ApiClientUnbound => "A live automatically bound container credential is required.",
        C::ApiClientStale => "The bound API credential is revoked, expired, or stale.",
        C::ApiBindingMismatch => "The API credential does not match the live container route.",
        C::AuthorityChanged => "Sender or target authority changed before actuation.",
        C::Busy => "The target coding agent is busy.",
        C::ResizeUnsettled => "The target terminal resize state is not settled.",
        C::UntrackedReadiness => "The target readiness state is incomplete.",
        C::UnsupportedSession => "The target session is not a trusted coding-agent session.",
        C::NonpersistentLiveSession => "A live nonpersistent target session prevents actuation.",
        C::InconsistentSession => "The target session lifecycle is inconsistent.",
        C::UnsupportedProfile => "No trusted supported coding-agent profile is available.",
        C::ReadinessTimeout => "The spawned coding agent did not become ready in time.",
        C::StoreCorrupt => "The privileged operation store failed an integrity check.",
        C::RestoreInProgress => "Session restore temporarily blocks target preparation.",
        C::PurgeInProgress => "Room purge temporarily blocks target preparation.",
        C::SessionRace => "The target session changed during preparation.",
        C::LeaseLost => "The preparation lease was lost before actuation.",
        C::SpawnFailedSafe => "Target spawn failed without leaving an ambiguous session.",
        C::StoreTransient => "A transient operation-store failure prevented actuation.",
        C::FinalRevalidationFailed => {
            "Final authority or readiness validation failed after the no-replay boundary."
        }
        C::TextWriteFailed => "The exact text write could not be durably proven.",
        C::RequiredEnterFailed => "The required Enter write could not be durably proven.",
        C::DaemonRestartAfterActuation => "The daemon restarted after the no-replay boundary.",
        C::RuntimeActuationOrphan => {
            "The actuation owner disappeared after the no-replay boundary."
        }
        C::TerminalStoreFailed => "The terminal result could not be durably recorded.",
        C::RedundantEnterFailed => "The redundant second Enter failed after successful submission.",
        C::BoundaryMetadataFailed => {
            "Submission succeeded but boundary metadata could not be completed."
        }
        C::ArtifactUnclaimed => "A host terminal artifact was not confirmed before compaction.",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PtyInputFailureClass {
    RetryBeforeBoundary,
    RejectBeforeBoundary,
    IndeterminateAfterBoundary,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PtyInputFailure {
    pub code: PtyInputReasonCode,
    pub class: PtyInputFailureClass,
}

impl PtyInputFailure {
    pub const fn reject(code: PtyInputReasonCode) -> Self {
        Self {
            code,
            class: PtyInputFailureClass::RejectBeforeBoundary,
        }
    }

    pub const fn retry(code: PtyInputReasonCode) -> Self {
        Self {
            code,
            class: PtyInputFailureClass::RetryBeforeBoundary,
        }
    }

    pub const fn indeterminate(code: PtyInputReasonCode) -> Self {
        Self {
            code,
            class: PtyInputFailureClass::IndeterminateAfterBoundary,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PtyInputWirePayload {
    pub version: u32,
    pub text: String,
    pub enter: PtyInputEnterMode,
    pub injection_id: String,
    pub op_id: String,
    pub issued_at: String,
    pub expires_at: String,
    pub nonce: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
}

impl fmt::Debug for PtyInputWirePayload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PtyInputWirePayload")
            .field("version", &self.version)
            .field("text_bytes", &self.text.len())
            .field("text_sha256", &sha256_hex(self.text.as_bytes()))
            .field("enter", &self.enter)
            .field("injection_id", &self.injection_id)
            .field("op_id", &self.op_id)
            .field("issued_at", &self.issued_at)
            .field("expires_at", &self.expires_at)
            .field("nonce", &"[REDACTED]")
            .field("agent_id", &self.agent_id)
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PtyInputHostEnvelope {
    pub id: String,
    pub token: String,
    pub from: String,
    pub to: String,
    pub body: String,
    pub mode: String,
    pub get_output: bool,
    pub preferred_agent: String,
    pub priority: String,
    pub timestamp: String,
    pub action: String,
    pub pty_input: PtyInputWirePayload,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PtyInputQueueMarker {
    pub kind: String,
    pub version: u32,
    pub injection_id: String,
    pub op_id: String,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PtyInputReason {
    pub code: PtyInputReasonCode,
    pub detail: String,
}

impl fmt::Debug for PtyInputReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PtyInputReason")
            .field("code", &self.code)
            .field("detail", &safe_detail(self.code))
            .finish()
    }
}

impl PtyInputReason {
    pub fn from_code(code: PtyInputReasonCode) -> Self {
        Self {
            code,
            detail: safe_detail(code).to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PtyInputResult {
    pub version: u32,
    pub injection_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub op_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub status: PtyInputPublicStatus,
    pub terminal: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_plane: Option<PtyInputSourcePlane>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_backend: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issued_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queued_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actuating_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<PtyInputReason>,
}

impl PtyInputResult {
    pub fn new(injection_id: String, status: PtyInputPublicStatus) -> Self {
        Self {
            version: PTY_INPUT_VERSION,
            injection_id,
            op_id: None,
            sender: None,
            target: None,
            status,
            terminal: status.is_terminal(),
            payload_bytes: None,
            payload_sha256: None,
            source_plane: None,
            selected_session_id: None,
            selected_backend: None,
            issued_at: None,
            expires_at: None,
            queued_at: None,
            actuating_at: None,
            terminal_at: None,
            reason: None,
        }
    }
}

pub const fn pty_input_reason_allowed_for_status(
    status: PtyInputPublicStatus,
    reason: Option<PtyInputReasonCode>,
) -> bool {
    use PtyInputPublicStatus as S;
    use PtyInputReasonCode as C;
    match status {
        S::Queued => matches!(
            reason,
            None | Some(
                C::RestoreInProgress
                    | C::PurgeInProgress
                    | C::SessionRace
                    | C::LeaseLost
                    | C::SpawnFailedSafe
                    | C::StoreTransient
            )
        ),
        S::Actuating => reason.is_none(),
        S::Injected => matches!(
            reason,
            None | Some(C::RedundantEnterFailed | C::BoundaryMetadataFailed)
        ),
        S::Rejected => {
            reason.is_some()
                && !matches!(
                    reason,
                    Some(
                        C::FinalRevalidationFailed
                            | C::TextWriteFailed
                            | C::RequiredEnterFailed
                            | C::DaemonRestartAfterActuation
                            | C::RuntimeActuationOrphan
                            | C::TerminalStoreFailed
                            | C::RedundantEnterFailed
                            | C::BoundaryMetadataFailed
                            | C::ArtifactUnclaimed
                    )
                )
        }
        S::Indeterminate => matches!(
            reason,
            Some(
                C::FinalRevalidationFailed
                    | C::TextWriteFailed
                    | C::RequiredEnterFailed
                    | C::DaemonRestartAfterActuation
                    | C::RuntimeActuationOrphan
                    | C::TerminalStoreFailed
            )
        ),
    }
}

pub(crate) fn is_lower_hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Validate the complete metadata-only DTO returned for an enqueued operation.
/// This deliberately inspects no payload bytes and accepts only fixed reason
/// detail, canonical identifiers/timestamps, and coherent public state.
pub fn validate_enqueued_pty_input_result(
    result: &PtyInputResult,
) -> Result<(), PtyInputReasonCode> {
    use PtyInputPublicStatus as S;
    use PtyInputReasonCode as C;

    if result.version != PTY_INPUT_VERSION
        || parse_canonical_uuid_v4(&result.injection_id).is_err()
        || result
            .op_id
            .as_deref()
            .is_none_or(|value| parse_canonical_uuid_v4(value).is_err())
        || result.sender.as_deref().is_none_or(|value| {
            value.is_empty() || value.chars().any(|character| character.is_control())
        })
        || result.target.as_deref().is_none_or(|value| {
            value.is_empty() || value.chars().any(|character| character.is_control())
        })
        || !matches!(result.payload_bytes, Some(1..=65_536))
        || result
            .payload_sha256
            .as_deref()
            .is_none_or(|value| !is_lower_hex_digest(value))
        || result.source_plane.is_none()
        || result.terminal != result.status.is_terminal()
        || result.selected_session_id.is_some() != result.selected_backend.is_some()
        || result
            .selected_session_id
            .as_deref()
            .is_some_and(|value| parse_canonical_uuid_v4(value).is_err())
        || result
            .selected_backend
            .as_deref()
            .is_some_and(|value| !matches!(value, "localProcess" | "containerTransport"))
    {
        return Err(C::StoreCorrupt);
    }

    let issued = result
        .issued_at
        .as_deref()
        .ok_or(C::StoreCorrupt)
        .and_then(parse_canonical_pty_timestamp)?;
    let expires = result
        .expires_at
        .as_deref()
        .ok_or(C::StoreCorrupt)
        .and_then(parse_canonical_pty_timestamp)?;
    let queued = result
        .queued_at
        .as_deref()
        .ok_or(C::StoreCorrupt)
        .and_then(parse_canonical_pty_timestamp)?;
    let actuating = result
        .actuating_at
        .as_deref()
        .map(parse_canonical_pty_timestamp)
        .transpose()?;
    let terminal = result
        .terminal_at
        .as_deref()
        .map(parse_canonical_pty_timestamp)
        .transpose()?;
    if expires - issued != chrono::Duration::seconds(PTY_INPUT_TTL_SECS)
        || queued < issued
        || queued >= expires
        || actuating.is_some_and(|value| value < queued || value >= expires)
        || terminal.is_some_and(|value| value < queued || actuating.is_some_and(|at| value < at))
    {
        return Err(C::StoreCorrupt);
    }

    let reason_code = result.reason.as_ref().map(|reason| reason.code);
    if result
        .reason
        .as_ref()
        .is_some_and(|reason| reason.detail != safe_detail(reason.code))
        || !pty_input_reason_allowed_for_status(result.status, reason_code)
    {
        return Err(C::StoreCorrupt);
    }

    let state_valid = match result.status {
        S::Queued => {
            actuating.is_none() && terminal.is_none() && result.selected_session_id.is_none()
        }
        S::Actuating => {
            actuating.is_some() && terminal.is_none() && result.selected_session_id.is_some()
        }
        S::Injected | S::Indeterminate => {
            actuating.is_some() && terminal.is_some() && result.selected_session_id.is_some()
        }
        S::Rejected => {
            actuating.is_none() && terminal.is_some() && result.selected_session_id.is_none()
        }
    };
    if !state_valid {
        return Err(C::StoreCorrupt);
    }
    if result.source_plane == Some(PtyInputSourcePlane::HostCli)
        && result.op_id.as_deref() != Some(result.injection_id.as_str())
    {
        return Err(C::StoreCorrupt);
    }
    Ok(())
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PtyInputHostArtifact {
    pub result: PtyInputResult,
    pub confirmation_tag: String,
}

impl fmt::Debug for PtyInputHostArtifact {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PtyInputHostArtifact")
            .field("result", &self.result)
            .field("confirmation_tag", &"[REDACTED]")
            .finish()
    }
}

/// Message format in outbox files. Shared between CLI verbs and MailboxPoller.
/// New fields remain optional for backward compatibility.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboxMessage {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    pub from: String,
    pub to: String,
    pub body: String,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub get_output: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_agent: Option<String>,
    #[serde(default)]
    pub preferred_agent: String,
    #[serde(default)]
    pub priority: String,
    pub timestamp: String,
    /// Logical PTY action (`clear` or `compact`); provider text is resolved by
    /// the mailbox and is not part of the wire value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub switch_coding_agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub switch_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dry_run: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quiet_period_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pty_input: Option<PtyInputWirePayload>,
}

impl fmt::Debug for OutboxMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OutboxMessage")
            .field("id", &self.id)
            .field("token", &self.token.as_ref().map(|_| "[REDACTED]"))
            .field("from", &self.from)
            .field("to", &self.to)
            .field("body_bytes", &self.body.len())
            .field("body_sha256", &sha256_hex(self.body.as_bytes()))
            .field("mode", &self.mode)
            .field("get_output", &self.get_output)
            .field("request_id", &self.request_id)
            .field("sender_agent", &self.sender_agent)
            .field("preferred_agent", &self.preferred_agent)
            .field("priority", &self.priority)
            .field("timestamp", &self.timestamp)
            .field("command", &self.command)
            .field("action", &self.action)
            .field("target", &self.target)
            .field("pty_input", &self.pty_input)
            .finish()
    }
}

pub fn canonical_pty_timestamp(now: DateTime<Utc>) -> String {
    now.to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn parse_canonical_pty_timestamp(raw: &str) -> Result<DateTime<Utc>, PtyInputReasonCode> {
    let parsed = DateTime::parse_from_rfc3339(raw)
        .map_err(|_| PtyInputReasonCode::InvalidTimestamp)?
        .with_timezone(&Utc);
    if canonical_pty_timestamp(parsed) != raw {
        return Err(PtyInputReasonCode::InvalidTimestamp);
    }
    Ok(parsed)
}

pub fn parse_canonical_uuid_v4(raw: &str) -> Result<Uuid, PtyInputReasonCode> {
    let id = Uuid::parse_str(raw).map_err(|_| PtyInputReasonCode::InvalidId)?;
    if id.is_nil()
        || id.get_version() != Some(Version::Random)
        || id.hyphenated().to_string() != raw
    {
        return Err(PtyInputReasonCode::InvalidId);
    }
    Ok(id)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("{digest:x}")
}

fn update_len_prefixed(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

pub fn pty_input_confirmation_tag(injection_id: &str, op_id: &str, nonce: &str) -> String {
    let mut hasher = Sha256::new();
    update_len_prefixed(&mut hasher, b"ac-pty-input-confirmation-v1");
    update_len_prefixed(&mut hasher, injection_id.as_bytes());
    update_len_prefixed(&mut hasher, op_id.as_bytes());
    update_len_prefixed(&mut hasher, nonce.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn pty_input_request_fingerprint(fields: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    update_len_prefixed(&mut hasher, b"ac-pty-input-request-v1");
    for field in fields {
        update_len_prefixed(&mut hasher, field);
    }
    format!("{:x}", hasher.finalize())
}

pub struct PtyInputHostFingerprint<'a> {
    pub injection_id: &'a str,
    pub op_id: &'a str,
    pub token: &'a str,
    pub sender_fqn: &'a str,
    pub target_fqn: &'a str,
    pub nonce: &'a str,
    pub issued_at: &'a str,
    pub expires_at: &'a str,
    pub text: &'a str,
    pub agent_id: Option<&'a str>,
    pub confirmation_tag: &'a str,
}

pub fn pty_input_host_request_fingerprint(input: &PtyInputHostFingerprint<'_>) -> String {
    let token_sha256 = sha256_hex(input.token.as_bytes());
    let nonce_sha256 = sha256_hex(input.nonce.as_bytes());
    let payload_sha256 = sha256_hex(input.text.as_bytes());
    pty_input_request_fingerprint(&[
        b"host_cli",
        input.injection_id.as_bytes(),
        input.op_id.as_bytes(),
        token_sha256.as_bytes(),
        input.sender_fqn.as_bytes(),
        input.target_fqn.as_bytes(),
        nonce_sha256.as_bytes(),
        input.issued_at.as_bytes(),
        input.expires_at.as_bytes(),
        b"1",
        b"agent-submit",
        payload_sha256.as_bytes(),
        &(input.text.len() as u64).to_be_bytes(),
        input.agent_id.unwrap_or("").as_bytes(),
        input.confirmation_tag.as_bytes(),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_message() -> OutboxMessage {
        OutboxMessage {
            id: "msg-1".into(),
            token: None,
            from: "from".into(),
            to: "to".into(),
            body: String::new(),
            mode: "wake".into(),
            get_output: false,
            request_id: None,
            sender_agent: None,
            preferred_agent: String::new(),
            priority: "normal".into(),
            timestamp: "2026-06-28T00:00:00Z".into(),
            command: None,
            action: None,
            target: None,
            force: None,
            timeout_secs: None,
            switch_coding_agent: None,
            switch_profile: None,
            dry_run: None,
            quiet_period_ms: None,
            pty_input: None,
        }
    }

    #[test]
    fn outbox_switch_fields_serialize_as_camel_case() {
        let mut msg = base_message();
        msg.switch_coding_agent = Some("codex-main".into());
        msg.switch_profile = Some("B".into());
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["switchCodingAgent"], "codex-main");
        assert_eq!(json["switchProfile"], "B");
    }

    #[test]
    fn old_outbox_defaults_privileged_payload_to_none() {
        let json = serde_json::json!({
            "id": "msg-1", "from": "from", "to": "to", "body": "",
            "timestamp": "2026-06-28T00:00:00Z"
        });
        let msg: OutboxMessage = serde_json::from_value(json).unwrap();
        assert!(msg.pty_input.is_none());
    }

    #[test]
    fn privileged_debug_redacts_all_plaintext_and_tokens() {
        let sentinel = "PTY-PLAINTEXT-SENTINEL";
        let token = "TOKEN-SENTINEL";
        let mut msg = base_message();
        msg.token = Some(token.into());
        msg.body = sentinel.into();
        msg.pty_input = Some(PtyInputWirePayload {
            version: 1,
            text: sentinel.into(),
            enter: PtyInputEnterMode::AgentSubmit,
            injection_id: Uuid::new_v4().to_string(),
            op_id: Uuid::new_v4().to_string(),
            issued_at: "2026-07-19T00:00:00.000Z".into(),
            expires_at: "2026-07-19T00:10:00.000Z".into(),
            nonce: Uuid::new_v4().to_string(),
            agent_id: None,
        });
        let rendered = format!("{msg:?}");
        assert!(!rendered.contains(sentinel));
        assert!(!rendered.contains(token));
        assert!(rendered.contains("[REDACTED]"));

        let reason = PtyInputReason {
            code: PtyInputReasonCode::InvalidEnvelope,
            detail: sentinel.to_string(),
        };
        let reason_debug = format!("{reason:?}");
        assert!(!reason_debug.contains(sentinel));
        assert!(reason_debug.contains(safe_detail(PtyInputReasonCode::InvalidEnvelope)));
    }

    #[test]
    fn canonical_timestamp_and_uuid_are_strict() {
        let now = DateTime::parse_from_rfc3339("2026-07-19T00:00:00.123Z")
            .unwrap()
            .with_timezone(&Utc);
        let canonical = canonical_pty_timestamp(now);
        assert_eq!(canonical, "2026-07-19T00:00:00.123Z");
        assert!(parse_canonical_pty_timestamp(&canonical).is_ok());
        assert!(parse_canonical_pty_timestamp("2026-07-19T00:00:00Z").is_err());
        let id = Uuid::new_v4().to_string();
        assert!(parse_canonical_uuid_v4(&id).is_ok());
        assert!(parse_canonical_uuid_v4(&id.to_uppercase()).is_err());
        assert!(parse_canonical_uuid_v4(&Uuid::nil().to_string()).is_err());
    }

    #[test]
    fn every_reason_has_fixed_nonempty_detail() {
        let json = serde_json::to_value(PtyInputReasonCode::ArtifactUnclaimed).unwrap();
        assert_eq!(json, "artifact_unclaimed");
        assert!(!safe_detail(PtyInputReasonCode::ArtifactUnclaimed).is_empty());
    }
}
