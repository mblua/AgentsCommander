use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramBotConfig {
    pub id: String,
    pub label: String,
    pub token: String,
    pub chat_id: i64,
    pub color: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeInfo {
    pub bot_id: String,
    pub bot_label: String,
    pub session_id: String,
    pub status: BridgeStatus,
    pub color: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BridgeStatus {
    Active,
    Error(String),
    Detaching,
}

/// Per-repo config read from .summongate/config.json
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoConfig {
    pub telegram_bot: Option<String>,
}
