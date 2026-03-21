use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: Uuid,
    pub name: String,
    pub shell: String,
    pub shell_args: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub working_directory: String,
    pub status: SessionStatus,
    pub waiting_for_input: bool,
    pub last_prompt: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum SessionStatus {
    Active,
    Running,
    Idle,
    Exited(i32),
}

/// Info sent to the frontend via IPC
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: String,
    pub name: String,
    pub shell: String,
    pub shell_args: Vec<String>,
    pub created_at: String,
    pub working_directory: String,
    pub status: SessionStatus,
    pub waiting_for_input: bool,
    pub last_prompt: Option<String>,
}

impl From<&Session> for SessionInfo {
    fn from(s: &Session) -> Self {
        SessionInfo {
            id: s.id.to_string(),
            name: s.name.clone(),
            shell: s.shell.clone(),
            shell_args: s.shell_args.clone(),
            created_at: s.created_at.to_rfc3339(),
            working_directory: s.working_directory.clone(),
            status: s.status.clone(),
            waiting_for_input: s.waiting_for_input,
            last_prompt: s.last_prompt.clone(),
        }
    }
}
