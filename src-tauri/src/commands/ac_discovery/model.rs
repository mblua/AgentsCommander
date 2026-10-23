use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcAgentMatrix {
    /// Display name: "{project_folder}/{agent_name}" with _agent_ prefix stripped
    pub name: String,
    /// Absolute path to the agent matrix directory
    pub path: String,
    /// Whether Role.md exists in the agent directory
    pub role_exists: bool,
    /// Preferred coding agent ID from config.json tooling.lastCodingAgent
    pub preferred_agent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcTeam {
    /// Team directory name with _team_ prefix stripped
    pub name: String,
    /// Agent display names belonging to this team
    pub agents: Vec<String>,
    /// Coordinator agent display name, if any
    pub coordinator: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcAgentReplica {
    /// Display name: agent dir name with __agent_ prefix stripped
    pub name: String,
    /// Absolute path to the replica agent directory
    pub path: String,
    /// Resolved identity path from config.json "identity" field
    pub identity_path: Option<String>,
    /// Project folder where the identity (matrix agent) lives
    pub origin_project: Option<String>,
    /// Preferred coding agent ID inherited from the identity matrix
    pub preferred_agent_id: Option<String>,
    /// Persisted selection UI coding agent ID for this replica
    pub current_coding_agent_id: Option<String>,
    /// Persisted selection UI profile for this replica
    pub current_profile: Option<String>,
    /// Absolute paths to repos this replica works on (resolved from config.json "repos")
    pub repo_paths: Vec<String>,
    /// Git branch of the first repo (if exactly one repo), for sidebar display
    pub repo_branch: Option<String>,
    /// True if this replica is a coordinator of any discovered team.
    /// Computed at construction against a fresh `config::teams` snapshot;
    /// covers WG-aware suffix matching that simple `originProject/name`
    /// comparison on the frontend misses. See issue #69.
    pub is_coordinator: bool,
    /// #552/#580 RFC3339 timestamp of the unified team-idle anchor
    /// `max(last_user_message_at, last_activity_at)` for this coordinator, or
    /// None. Only meaningful when `is_coordinator`. The badge displays and
    /// auto-close triggers on this value (closed time counts). The field keeps
    /// its #552 name (rename to `idleSinceAt` deferred, Decision C). Read from
    /// the persisted CoordinatorClocks store keyed by FQN.
    pub last_user_message_at: Option<String>,
    /// #552 RFC3339 timestamp this coordinator's team was auto-closed for
    /// inactivity, or None. Only meaningful when `is_coordinator`. Drives the
    /// "auto-closed" pill; cleared on reopen. Read from the same store entry.
    pub auto_closed_at: Option<String>,
    /// #588 RFC3339 time this coordinator was manually closed, or None. Only
    /// meaningful when `is_coordinator`. Drives the MANUALLY-CLOSED pill;
    /// cleared on reopen. Read from the same persisted store entry.
    pub manually_closed_at: Option<String>,
    /// #1941 - the stored selection pair from the strict #1939 read, or `null`
    /// when no complete pair exists (including an invalid state).
    pub saved_pair: Option<AcReplicaSavedPair>,
    /// #1941 - `unlocked` | `locked` | `invalid`, from the strict state read.
    /// Never a synthesized default: an unreadable/malformed state is `invalid`.
    pub selection_state: Option<String>,
    /// #1941 - the strict-read diagnostic when `selection_state` is `invalid`.
    pub selection_error: Option<String>,
}

/// #1941 - the selection pair as discovery exposes it. A strict complete pair
/// fills both fields; the option type leaves room for legacy/partial data.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcReplicaSavedPair {
    pub coding_agent_id: Option<String>,
    pub requested_profile: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcWorkgroup {
    /// Workgroup name (wg-* dir name)
    pub name: String,
    /// Absolute path to the workgroup directory
    pub path: String,
    /// First line of TASK.md (if exists)
    pub task: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_title: Option<String>,
    /// Replica agents inside this workgroup
    pub agents: Vec<AcAgentReplica>,
    /// Absolute path to the first repo-* directory found (for CWD)
    pub repo_path: Option<String>,
    /// Team this workgroup belongs to (matched by replica membership)
    pub team_name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcDiscoveryResult {
    pub agents: Vec<AcAgentMatrix>,
    pub teams: Vec<AcTeam>,
    pub workgroups: Vec<AcWorkgroup>,
    pub loops: Vec<crate::config::loops::AcLoopSummary>,
    pub context_template_updates:
        Vec<crate::config::seeded_context_templates::ContextTemplateUpdate>,
}
