use serde::{Deserialize, Serialize};

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
