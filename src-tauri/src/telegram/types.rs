use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramBotConfig {
    pub id: String,
    pub label: String,
    pub token: String,
    pub chat_id: i64,
    pub color: String,
}

/// Manual `Debug` so `{:?}` never prints the bot token (#4319).
/// Serialization is unchanged: `settings.json` still stores the token.
impl std::fmt::Debug for TelegramBotConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelegramBotConfig")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("token", &"[REDACTED]")
            .field("chat_id", &self.chat_id)
            .field("color", &self.color)
            .finish()
    }
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
