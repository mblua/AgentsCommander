use serde::Serialize;

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
