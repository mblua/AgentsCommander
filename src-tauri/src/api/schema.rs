//! Versioned request/response DTOs for the in-daemon control-plane API (#791).
//!
//! Every request DTO layer carries `#[serde(deny_unknown_fields)]` so any
//! client-supplied field outside the declared shape (notably the forbidden
//! `from` / `root` / `token` / `command` / `action`) is rejected at parse time
//! with a 400. All DTOs use camelCase to match the IPC convention
//! (`OutboxMessage`, `phone/types.rs`).

use serde::{Deserialize, Serialize};

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
    /// The message payload. Exactly one of `send` (increment 1) or `inline` ([H]).
    pub message: SendMessage,
}

/// The `message` one-of. Increment 1 accepts only `send` (a bare filename that
/// already exists in the sender's `<workgroup>/messaging/`); `inline` is
/// reserved in the schema but rejected with 400 in v1.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SendMessage {
    /// Bare filename (no path separators) present in `<workgroup>/messaging/`.
    #[serde(default)]
    pub send: Option<String>,
    /// Reserved [H]: inline message text. Rejected with 400 in increment 1.
    #[serde(default)]
    pub inline: Option<String>,
}

/// `POST /api/v1/send` response.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendResponse {
    pub api_version: String,
    pub op_id: String,
    /// `"delivered"` on success, `"rejected"` when the engine refused delivery.
    pub status: String,
    /// Canonical target FQN the message was routed to.
    pub to: String,
    /// Human-readable reason on `rejected`; `null` on `delivered`.
    pub detail: Option<String>,
}

impl SendResponse {
    /// Build the `delivered` response for a target FQN.
    pub fn delivered(op_id: &str, to: &str) -> Self {
        Self {
            api_version: API_VERSION.to_string(),
            op_id: op_id.to_string(),
            status: "delivered".to_string(),
            to: to.to_string(),
            detail: None,
        }
    }
}

/// `GET /api/v1/peers` response (mirrors `list-peers-lean`).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeersResponse {
    pub api_version: String,
    /// Server-generated correlation id (`list-peers-lean` is not deduped).
    pub op_id: String,
    pub peers: Vec<crate::cli::list_peers::LeanPeerInfo>,
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
mod tests {
    use super::*;

    #[test]
    fn send_request_rejects_unknown_top_level_field() {
        // The forbidden identity fields are "unknown" to the DTO, so
        // deny_unknown_fields rejects them at parse time.
        for forbidden in ["from", "root", "token", "command", "action"] {
            let json = format!(
                r#"{{"apiVersion":"1","opId":"o","to":"a/b","message":{{"send":"f.md"}},"{}":"x"}}"#,
                forbidden
            );
            let parsed: Result<SendRequest, _> = serde_json::from_str(&json);
            assert!(
                parsed.is_err(),
                "body field '{}' must be rejected by deny_unknown_fields",
                forbidden
            );
        }
    }

    #[test]
    fn send_message_rejects_unknown_field() {
        let json = r#"{"apiVersion":"1","opId":"o","to":"a/b","message":{"send":"f.md","bogus":1}}"#;
        let parsed: Result<SendRequest, _> = serde_json::from_str(json);
        assert!(parsed.is_err(), "unknown message field must be rejected");
    }

    #[test]
    fn send_request_parses_minimal_valid_body() {
        let json = r#"{"apiVersion":"1","opId":"o","to":"proj/agent","message":{"send":"20260101-x.md"}}"#;
        let req: SendRequest = serde_json::from_str(json).expect("valid body should parse");
        assert_eq!(req.api_version, "1");
        assert_eq!(req.op_id, "o");
        assert_eq!(req.to, "proj/agent");
        assert_eq!(req.message.send.as_deref(), Some("20260101-x.md"));
        assert!(req.message.inline.is_none());
    }

    #[test]
    fn health_response_is_exactly_ok_true() {
        let json = serde_json::to_value(HealthResponse { ok: true }).unwrap();
        assert_eq!(json, serde_json::json!({ "ok": true }));
        // No other field may serialize (§0.5 G9).
        assert_eq!(json.as_object().unwrap().len(), 1);
    }

    #[test]
    fn send_response_delivered_has_null_detail_and_camel_case() {
        let json = serde_json::to_value(SendResponse::delivered("op1", "proj/agent")).unwrap();
        assert_eq!(json["apiVersion"], "1");
        assert_eq!(json["opId"], "op1");
        assert_eq!(json["status"], "delivered");
        assert_eq!(json["to"], "proj/agent");
        assert!(json["detail"].is_null());
    }
}
