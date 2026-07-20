use serde::{Deserialize, Serialize};

/// Message format in outbox files. Shared between CLI (send, close-session) and MailboxPoller.
/// All new fields are Option/default for backwards compatibility with existing outbox messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// Action type for non-message operations (e.g., "close-session")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// Target agent name for action-based operations (e.g., close-session target)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Force mode for close-session (true = immediate kill, false = graceful)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force: Option<bool>,
    /// Timeout in seconds for graceful shutdown before fallback to force-kill
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u32>,
    /// self-handoff-and-switch target configured coding-agent entry id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub switch_coding_agent: Option<String>,
    /// self-handoff-and-switch target profile slot letter A-Z.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub switch_profile: Option<String>,
    /// (#885) purge-wg: evaluate the gate and report, destroy nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dry_run: Option<bool>,
    /// (#885) purge-wg: printable-silence a peer must show to be purgeable.
    /// CLAMPED daemon-side to a floor of `IdleTuning::DEFAULT.idle_threshold`
    /// (2500 ms). The outbox is a trust boundary; a hand-crafted message must
    /// not be able to set this to 0 and defeat the busy gate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quiet_period_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhoneMessage {
    pub id: String,
    pub from: String,
    pub to: String,
    pub team: String,
    pub content: String,
    pub timestamp: String,
    /// "pending", "delivered", "error"
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub id: String,
    pub participants: Vec<String>,
    pub created_at: String,
    pub messages: Vec<PhoneMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    pub name: String,
    pub path: String,
    pub teams: Vec<String>,
    pub is_coordinator_of: Vec<String>,
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
        assert!(json.get("switch_coding_agent").is_none());
    }

    #[test]
    fn outbox_switch_fields_default_when_missing() {
        let json = serde_json::json!({
            "id": "msg-1",
            "from": "from",
            "to": "to",
            "body": "",
            "timestamp": "2026-06-28T00:00:00Z"
        });

        let msg: OutboxMessage = serde_json::from_value(json).unwrap();

        assert_eq!(msg.switch_coding_agent, None);
        assert_eq!(msg.switch_profile, None);
    }
}
