use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Mangle a CWD path the same way Claude Code does for its project directories.
/// Non-alphanumeric, non-hyphen characters are replaced with '-'.
/// Used by session creation (--continue detection) and the JSONL watcher.
pub fn mangle_cwd_for_claude(cwd: &str) -> String {
    cwd.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Prefix used historically by wake-and-sleep delivery (removed in 0.7.0).
/// Retained for defensive purge of legacy temp sessions persisted under
/// older versions, and as a sort-key tiebreaker in `find_active_session`
/// (non-temp sessions preferred).
pub const TEMP_SESSION_PREFIX: &str = "[temp]";

/// One repo watched inside a session, rendered as a single sidebar badge "<label>/<branch>".
/// Populated at session creation time from the replica's `repoPaths`; `branch` is filled
/// and refreshed by `GitWatcher` on each poll.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionRepo {
    /// Repo dir name with leading "repo-" stripped (e.g. "AgentsCommander").
    pub label: String,
    /// Absolute path to the repo root. Branch detection runs `git rev-parse` in this dir.
    pub source_path: String,
    /// Current branch. `None` until first watcher tick, or when detection fails.
    #[serde(default)]
    pub branch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: Uuid,
    pub name: String,
    pub shell: String,
    pub shell_args: Vec<String>,
    /// Effective arg vector actually handed to portable-pty at spawn time,
    /// including dynamic injections (`--continue`, `codex resume --last`,
    /// `--append-system-prompt-file <path>`). `None` until the PTY is
    /// spawned for this session; set once by `create_session_inner` right
    /// before `pty_mgr.spawn`. Runtime-only — NOT persisted to `sessions.toml`
    /// (configured args in `shell_args` are the persistence recipe; the
    /// effective args are re-derived at every spawn from current settings).
    #[serde(skip)]
    pub effective_shell_args: Option<Vec<String>>,
    pub created_at: DateTime<Utc>,
    pub working_directory: String,
    pub status: SessionStatus,
    pub waiting_for_input: bool,
    /// Frontend-only: true when agent finished but user hasn't focused yet
    #[serde(default)]
    pub pending_review: bool,
    pub last_prompt: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub agent_label: Option<String>,
    /// Repos watched by this session. Empty = no repo badge rendered.
    /// Order = replica config.json `repos` array order. Never sort, never dedupe,
    /// never rebuild from a map — equality comparisons in `GitWatcher` depend on order.
    #[serde(default)]
    pub git_repos: Vec<SessionRepo>,
    /// Whether this session's agent is a coordinator of any discovered team.
    /// Controls repo-badge visibility on the sidebar. Recomputed after every discovery.
    #[serde(default)]
    pub is_coordinator: bool,
    /// Monotonic generation counter for `git_repos`. Bumped on every refresh/watcher write.
    /// Used for compare-and-swap in `set_git_repos_if_gen` so an in-flight watcher poll
    /// cannot overwrite a refresh that landed during its detection window. Runtime-only;
    /// never persisted and never exposed via SessionInfo.
    #[serde(skip)]
    pub git_repos_gen: u64,
    /// Unique token for CLI authentication. Passed to agents via init prompt.
    pub token: Uuid,
    /// True if this session runs Claude Code (detected at creation time).
    /// Used by the Telegram bridge to choose JSONL watcher vs PTY pipeline.
    #[serde(default)]
    pub is_claude: bool,
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
    /// See `Session::effective_shell_args`. `None` means "not yet registered"
    /// (dormant or pre-spawn). On the wire, serializes as `null`.
    #[serde(default)]
    pub effective_shell_args: Option<Vec<String>>,
    pub created_at: String,
    pub working_directory: String,
    pub status: SessionStatus,
    pub waiting_for_input: bool,
    #[serde(default)]
    pub pending_review: bool,
    pub last_prompt: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub agent_label: Option<String>,
    #[serde(default)]
    pub git_repos: Vec<SessionRepo>,
    #[serde(default)]
    pub is_coordinator: bool,
    pub token: String,
    #[serde(default)]
    pub is_claude: bool,
}

impl From<&Session> for SessionInfo {
    fn from(s: &Session) -> Self {
        SessionInfo {
            id: s.id.to_string(),
            name: s.name.clone(),
            shell: s.shell.clone(),
            shell_args: s.shell_args.clone(),
            effective_shell_args: s.effective_shell_args.clone(),
            created_at: s.created_at.to_rfc3339(),
            working_directory: s.working_directory.clone(),
            status: s.status.clone(),
            waiting_for_input: s.waiting_for_input,
            pending_review: false,
            last_prompt: s.last_prompt.clone(),
            agent_id: s.agent_id.clone(),
            agent_label: s.agent_label.clone(),
            git_repos: s.git_repos.clone(),
            is_coordinator: s.is_coordinator,
            token: s.token.to_string(),
            is_claude: s.is_claude,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_session(effective: Option<Vec<String>>) -> Session {
        Session {
            id: Uuid::nil(),
            name: "Session 1".to_string(),
            shell: "claude-mb".to_string(),
            shell_args: vec!["--dangerously-skip-permissions".to_string()],
            effective_shell_args: effective,
            created_at: Utc::now(),
            working_directory: "C:\\tmp".to_string(),
            status: SessionStatus::Running,
            waiting_for_input: false,
            pending_review: false,
            last_prompt: None,
            agent_id: None,
            agent_label: None,
            git_repos: Vec::new(),
            is_coordinator: false,
            git_repos_gen: 0,
            token: Uuid::nil(),
            is_claude: false,
        }
    }

    #[test]
    fn session_info_from_session_copies_effective_shell_args_some() {
        let s = sample_session(Some(vec![
            "--dangerously-skip-permissions".to_string(),
            "--continue".to_string(),
        ]));
        let info = SessionInfo::from(&s);
        assert_eq!(
            info.effective_shell_args,
            Some(vec![
                "--dangerously-skip-permissions".to_string(),
                "--continue".to_string()
            ])
        );
    }

    #[test]
    fn session_info_from_session_copies_effective_shell_args_none() {
        let s = sample_session(None);
        let info = SessionInfo::from(&s);
        assert_eq!(info.effective_shell_args, None);
    }

    #[test]
    fn session_info_from_session_copies_effective_shell_args_empty() {
        let s = sample_session(Some(Vec::new()));
        let info = SessionInfo::from(&s);
        assert_eq!(info.effective_shell_args, Some(Vec::new()));
    }
}
