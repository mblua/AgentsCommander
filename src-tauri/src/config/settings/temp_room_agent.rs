use serde::{Deserialize, Serialize};

/// #4779: coding agent remembered for new temporary rooms.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TempRoomAgentChoice {
    pub agent_id: String,
    #[serde(default)]
    pub requested_profile: Option<String>,
}
