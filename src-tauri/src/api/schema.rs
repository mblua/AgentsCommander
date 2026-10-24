//! Versioned request/response DTOs for the in-daemon control-plane API (#791).
//!
//! Every request DTO layer carries `#[serde(deny_unknown_fields)]` so any
//! client-supplied field outside the declared shape (notably the forbidden
//! `from` / `root` / `token` / `command` / `action`) is rejected at parse time
//! with a 400. All DTOs use camelCase to match the IPC convention
//! (`OutboxMessage`, `phone/types.rs`).

use serde::{Deserialize, Serialize};

pub use terminal_snapshot_renderer::{TerminalSnapshotApiError, TerminalSnapshotApiRequest};

/// Contract version. URL-versioned (`/api/v1`); this echo is a redundant,
/// explicit assertion field for clients.
pub const API_VERSION: &str = "1";

/// `POST /api/v1/send` request envelope.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SendRequest {
    /// Client-asserted contract version (redundant with the URL). Required so a
    /// malformed client that omits it is rejected.
    pub api_version: String,
    /// Client-generated idempotency key. A replay returns the prior result.
    pub op_id: String,
    /// Destination agent canonical FQN.
    pub to: String,
    /// The message payload. Exactly one of `send` or `inline`.
    pub message: SendMessage,
}

/// The `message` one-of. `send` is a bare filename that already exists in the
/// sender's `<workgroup>/messaging/`; the handler ingests its content into the
/// DB as inline text. `inline` carries direct message text.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SendMessage {
    /// Bare filename (no path separators) present in `<workgroup>/messaging/`.
    #[serde(default)]
    pub send: Option<String>,
    /// Inline message text. Mutually exclusive with `send`.
    #[serde(default)]
    pub inline: Option<String>,
    /// MIME-like content type for the inline payload.
    #[serde(default)]
    pub content_type: Option<String>,
}

/// `POST /api/v1/send` response.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendResponse {
    pub api_version: String,
    pub op_id: String,
    /// `"queued"` once the durable DB row exists.
    pub status: String,
    /// Canonical target FQN the message was routed to.
    pub to: String,
    /// Durable DB message id.
    pub message_id: String,
    /// Human-readable detail; `null` on normal queued responses.
    pub detail: Option<String>,
}

impl SendResponse {
    /// Build the `queued` response for a target FQN.
    pub fn queued(op_id: &str, to: &str, message_id: &str) -> Self {
        Self {
            api_version: API_VERSION.to_string(),
            op_id: op_id.to_string(),
            status: "queued".to_string(),
            to: to.to_string(),
            message_id: message_id.to_string(),
            detail: None,
        }
    }
}

/// `POST /api/v1/pty-input` strict request envelope. Payload-bearing request
/// types intentionally implement no `Debug`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PtyInputRequest {
    pub api_version: String,
    pub op_id: String,
    pub to: String,
    pub pty_input: PtyInputRequestPayload,
    #[serde(default)]
    pub agent_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PtyInputRequestPayload {
    pub version: u32,
    pub text: String,
    pub enter: crate::phone::types::PtyInputEnterMode,
}

/// serde predicate: omit a `false` flag from the JSON entirely (#1669).
pub(crate) fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Lean peer projection emitted by `list-peers-lean`. Made `pub` (#791 §8.2)
/// so the in-daemon API can reuse the exact projection via `lean_peers`; only
/// this lean view crosses the module boundary, `PeerInfo` stays private.
pub struct LeanPeerInfo {
    /// Canonical FQN. Same value as `PeerInfo.name`.
    pub name: String,
    /// Same predicate as `PeerInfo.working`.
    pub working: bool,
    /// Same domain as `PeerInfo.session_status`.
    pub session_status: String,
    /// Same value as `PeerInfo.waiting_for_input`.
    pub waiting_for_input: bool,
    /// #1669 - same value as `PeerInfo.blocked_menu`. Omitted entirely when
    /// the matched session is not parked on a blocking terminal menu.
    #[serde(skip_serializing_if = "is_false")]
    pub blocked_menu: bool,
    /// #1669 - same value as `PeerInfo.blocked_menu_message`. Omitted when the
    /// session is not blocked or the row carries no message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_menu_message: Option<String>,
    /// Same value as `PeerInfo.reachable`.
    pub reachable: bool,
    /// Same value as `PeerInfo.teams`.
    pub teams: Vec<String>,
    /// Short single-line summary derived from `PeerInfo.role`, ≤80 chars
    /// total (including any trailing `…`). Omitted when empty.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub role_summary: String,
    /// #1088 - the peer's live context-usage percent (0-100), the same figure
    /// the Sidebar badge shows. Same value as `PeerInfo.context_percent`.
    /// Omitted when there is no reading.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_percent: Option<u8>,
    /// Effective coding-agent id of the peer's chosen live session. Omitted
    /// when the peer has no live session.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Effective profile letter of that session (post-fallback). Omitted when
    /// the peer has no live session or the session is not an agent session.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

/// `GET /api/v1/peers` response (mirrors `list-peers-lean`).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeersResponse {
    pub api_version: String,
    /// Server-generated correlation id (`list-peers-lean` is not deduped).
    pub op_id: String,
    pub peers: Vec<LeanPeerInfo>,
}

/// `GET /api/v1/healthz` response. Pinned to exactly `{"ok":true}` (§0.5 G9):
/// no version, bind, or client-count is exposed on the unauthenticated route.
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub ok: bool,
}

/// Error body returned for every non-2xx response. Never contains host
/// absolute paths (scrubbed in `error.rs`).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorBody {
    pub api_version: String,
    /// Stable machine-readable code, e.g. `"unauthorized"`, `"forbidden"`.
    pub error: String,
    /// Human-readable, host-path-scrubbed detail.
    pub detail: String,
}

#[cfg(test)]
mod tests;
