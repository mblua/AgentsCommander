use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Mangle a CWD path the same way Claude Code does for its project directories.
/// Non-alphanumeric, non-hyphen characters are replaced with '-'.
/// Used by session creation (--continue detection) and the JSONL watcher.
pub fn mangle_cwd_for_claude(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .collect()
}

/// Prefix used for temporary sessions spawned by wake-and-sleep delivery.
/// These sessions are ephemeral and must never be persisted across restarts.
pub const TEMP_SESSION_PREFIX: &str = "[temp]";

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
    /// Frontend-only: true when agent finished but user hasn't focused yet
    #[serde(default)]
    pub pending_review: bool,
    pub last_prompt: Option<String>,
    pub git_branch: Option<String>,
    /// Path to detect git branch from (overrides working_directory when set).
    /// Used for replica sessions where the cwd is the agent dir but we want the repo's branch.
    #[serde(default)]
    pub git_branch_source: Option<String>,
    /// Prefix to prepend to the detected branch (e.g., "agentscommander" → "agentscommander/main").
    /// When set without git_branch_source, used as the full static label (e.g., "multi-repo").
    #[serde(default)]
    pub git_branch_prefix: Option<String>,
    /// Unique token for CLI authentication. Passed to agents via init prompt.
    pub token: Uuid,
    /// True if this session runs Claude Code (detected at creation time).
    /// Used by the Telegram bridge to choose JSONL watcher vs PTY pipeline.
    #[serde(default)]
    pub is_claude: bool,
    /// Completion status: "working", "completed", or "hung"
    #[serde(default = "default_completion_status")]
    pub completion_status: String,
}

fn default_completion_status() -> String {
    "working".to_string()
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
    #[serde(default)]
    pub pending_review: bool,
    pub last_prompt: Option<String>,
    pub git_branch: Option<String>,
    #[serde(default)]
    pub git_branch_source: Option<String>,
    #[serde(default)]
    pub git_branch_prefix: Option<String>,
    pub token: String,
    #[serde(default)]
    pub is_claude: bool,
    #[serde(default)]
    pub completion_status: String,
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
            pending_review: false,
            last_prompt: s.last_prompt.clone(),
            git_branch: s.git_branch.clone(),
            git_branch_source: s.git_branch_source.clone(),
            git_branch_prefix: s.git_branch_prefix.clone(),
            token: s.token.to_string(),
            is_claude: s.is_claude,
            completion_status: s.completion_status.clone(),
        }
    }
}
