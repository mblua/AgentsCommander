use clap::Args;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::config::ac_root::existing_ac_root;
use crate::config::agent_config::{AgentLocalConfig, CodingAgentEntry};
use crate::config::sessions_persistence::{load_sessions_raw, PersistedSession};
use crate::session::session::{SessionStatus, TEMP_SESSION_PREFIX};

#[derive(Args)]
#[command(after_help = "\
OUTPUT: JSON array of team peers. Each entry contains:\n  \
  name              Agent name to use with `send --to` (e.g., \"repos/my-project\")\n  \
  path              Full filesystem path to the agent's root directory\n  \
  status            Legacy: \"active\" iff working==true, else \"unknown\"\n  \
  working           true iff peer has a Running or Active session not\n                    \
                  waiting for input. For Room peers this matches the\n                    \
                  Sidebar running-peer badge exactly.\n  \
  sessionStatus     One of: \"active\", \"running\", \"idle\", \"waiting\",\n                    \
                  \"exited\", \"none\"\n  \
  sessionId         UUID of the matched session (omitted if no match)\n  \
  waitingForInput   true if the matching session is waiting for user input\n  \
  exitCode          Exit code (only present when sessionStatus == \"exited\")\n  \
  role              Summary extracted from the agent's CLAUDE.md\n  \
  teams             List of shared team names\n  \
  reachable         true if you can directly message this agent, false otherwise\n  \
  lastCodingAgent   Last coding CLI used (e.g., \"claude\", \"codex\"), if known\n\n\
PEER FILTER (--peer):\n  \
  Repeat `--peer <FQN>` to return only the named peers. Matching is by\n  \
  exact canonical FQN (no substring, no case-folding). Duplicate values\n  \
  are silently deduplicated and the output preserves the user-requested\n  \
  order for unique entries. When omitted, all discovered peers are\n  \
  returned (current behavior is byte-for-byte unchanged). If any\n  \
  requested FQN is not present in the discovered peer set the command\n  \
  exits non-zero with a clear stderr error naming the unknown peer(s);\n  \
  no JSON is emitted on the unknown-peer path. Unreachable peers\n  \
  (reachable=false) are still returned when their name matches —\n  \
  filtering is by name only.\n\n\
NOTES:\n  \
  - Canonical Root Agent roots return verified Room orchestrator replicas only.\n    \
    Origin orchestrators and non-orchestrator Room replicas are omitted for Root Agent discovery in #277.\n  \
  - Identity-verified Room orchestrator replicas additionally see a synthetic\n    \
    peer with `name=agentscommander://root-agent`, representing the Root\n    \
    Agent reply target. Pass that name verbatim to `send --to` when replying\n    \
    to a root-originated message. Other replicas do not see this entry.\n    \
    See issue #293.\n  \
  - Working-state visibility is bound to the binary instance writing\n    \
    sessions.json. Peers running under a different AgentsCommander binary\n    \
    (e.g. agentscommander_mb_wg-20.exe vs agentscommander_mb.exe) will\n    \
    always report sessionStatus=\"none\".\n  \
  - `pendingReview` is a frontend-only state, invisible to the CLI. A\n    \
    peer whose agent has finished but the user has not yet acknowledged in\n    \
    the sidebar will be reported as working/running by this command.\n  \
  - Room peers match by session name (`<room>/<agent>`); non-Room peers match\n    \
    by working-directory only.\n  \
  See issue #206 for the full rationale.\n\n\
All agents that belong to your team(s) are listed. Agents you cannot directly\n\
message are included with reachable=false. If you have no teams, the result is an empty array.")]
pub struct ListPeersArgs {
    /// Session token from AGENTSCOMMANDER_TOKEN. Shape-validated only; this verb
    /// reads disk state and does not authorize per-token. See `--help` TOKEN VALIDATION MODEL.
    #[arg(long)]
    pub token: Option<String>,

    /// Agent root directory (required). Your working directory — used to identify you and your teams
    #[arg(long)]
    pub root: Option<String>,

    /// Return only the peers whose canonical FQN exactly matches one of the given
    /// values. Repeat the flag to request multiple peers (e.g. `--peer A --peer B`).
    /// Duplicates are silently deduplicated. When omitted, every discovered peer is
    /// returned. If any requested FQN is absent from the discovered set, the command
    /// exits non-zero and writes a clear error to stderr. See `--help` PEER FILTER.
    #[arg(long = "peer")]
    pub peer: Vec<String>,
}

#[derive(Args)]
#[command(after_help = "\
OUTPUT: JSON array of team peers, compact form for agent coordination.\n\
Each entry contains:\n  \
  name              Canonical FQN. Pass verbatim to `send --to`.\n  \
  working           true iff the peer has a Running/Active session not\n                    \
                  waiting for input. Same predicate as `list-peers`.\n  \
  sessionStatus     One of: \"active\", \"running\", \"idle\", \"waiting\",\n                    \
                  \"exited\", \"none\". Same domain as `list-peers`.\n  \
  waitingForInput   true if the matched session is waiting for user input;\n                    \
                  false when there is no matching session.\n  \
  reachable         true if you can directly message this agent.\n  \
  teams             List of shared team names.\n  \
  roleSummary       Single-line role hint, ≤80 chars total (including any\n                    \
                  trailing `…` ellipsis when truncated). Omitted if\n                    \
                  empty. Best-effort: when a peer has no `## Role`\n                    \
                  section in CLAUDE.md (or no `# Role:` heading in\n                    \
                  Role.md), this field may reflect the role-document\n                    \
                  fallback rather than an explicit role description.\n                    \
                  Treat as a hint, not authoritative.\n\n\
EXCLUDED VS list-peers: path, role (full), codingAgents, lastCodingAgent,\n\
sessionId, exitCode, legacy status. Use `list-peers` if any of those are\n\
needed.\n\n\
PEER SET: identical to `list-peers` for the same --root. The two verbs\n\
share a single discovery function (see issue #252). For canonical Root Agent\n\
roots this set contains verified Room orchestrator replicas only; origin\n\
orchestrators and non-orchestrator Room replicas are omitted in #277.\n\n\
SNAPSHOT TARGETS: `--snapshot-targets` switches only this lean verb to the\n\
identity-only terminal snapshot capability view. Canonical Root lists every\n\
verified Room Orchestrator and member in registered projectPaths. A verified Room\n\
Orchestrator lists non-Orchestrator members of its own room. Workers and\n\
origin agents receive `[]`. This view reads no sessions.json, creates no peer\n\
directories, and grants no authority. Use the returned exact name with\n\
`terminal-snapshot --to`.\n\n\
PEER FILTER (--peer):\n  \
  Repeat `--peer <FQN>` to return only the named peers. Matching is by\n  \
  exact canonical FQN (no substring, no case-folding). Duplicate values\n  \
  are silently deduplicated and the output preserves the user-requested\n  \
  order for unique entries. When omitted, all discovered peers are\n  \
  returned (current behavior is byte-for-byte unchanged). If any\n  \
  requested FQN is not present in the discovered peer set the command\n  \
  exits non-zero with a clear stderr error naming the unknown peer(s);\n  \
  no JSON is emitted on the unknown-peer path. Unreachable peers\n  \
  (reachable=false) are still returned when their name matches —\n  \
  filtering is by name only. Identical semantics to `list-peers --peer`.\n\n\
NOTES:\n  \
  - Working-state visibility is bound to the binary instance that wrote\n    \
    sessions.json (same caveat as `list-peers`).\n  \
  - Identity-verified Room orchestrator replicas additionally see a synthetic\n    \
    peer with `name=agentscommander://root-agent`, representing the Root\n    \
    Agent reply target. Pass that name verbatim to `send --to` when replying\n    \
    to a root-originated message. Other replicas do not see this entry.\n    \
    See issue #293.\n  \
  - Side effect: discovering Room peers creates `inbox/` and `outbox/`\n    \
    subdirectories under each peer's local config dir (inherited from\n    \
    `list-peers`). The verb is read-only w.r.t. the daemon mailbox, NOT\n    \
    filesystem-side-effect-free.\n  \
  - Reachable=false entries are included so callers can see the full team\n    \
    set; filter client-side if needed.\n\
See issue #252 for the full rationale.")]
pub struct ListPeersLeanArgs {
    /// Session token from AGENTSCOMMANDER_TOKEN. Shape-validated only; this verb
    /// reads disk state and does not authorize per-token. See `--help` TOKEN VALIDATION MODEL.
    #[arg(long)]
    pub token: Option<String>,

    /// Agent root directory (required). Your working directory — used to identify you and your teams
    #[arg(long)]
    pub root: Option<String>,

    /// Return only the peers whose canonical FQN exactly matches one of the given
    /// values. Repeat the flag to request multiple peers (e.g. `--peer A --peer B`).
    /// Duplicates are silently deduplicated. When omitted, every discovered peer is
    /// returned. If any requested FQN is absent from the discovered set, the command
    /// exits non-zero and writes a clear error to stderr. See `--help` PEER FILTER.
    #[arg(long = "peer")]
    pub peer: Vec<String>,

    /// Capability-specific, identity-only terminal snapshot target discovery.
    /// This view is side-effect-free, reads no session index, and does not
    /// change ordinary messaging discovery or grant snapshot authority.
    #[arg(long = "snapshot-targets")]
    pub snapshot_targets: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Lean peer projection emitted by `list-peers-lean`. Made `pub` (#791 §8.2)
/// so the in-daemon API can reuse the exact projection via `lean_peers`; only
/// this lean view crosses the module boundary, `PeerInfo` stays private.
pub struct LeanPeerInfo {
    /// Canonical FQN. Same value as `PeerInfo.name`.
    pub name: String,
    /// Same predicate as `PeerInfo.working`.
    pub working: bool,
    /// Same domain as `PeerInfo.session_status`.
    pub session_status: String,
    /// Same value as `PeerInfo.waiting_for_input`.
    pub waiting_for_input: bool,
    /// Same value as `PeerInfo.reachable`.
    pub reachable: bool,
    /// Same value as `PeerInfo.teams`.
    pub teams: Vec<String>,
    /// Short single-line summary derived from `PeerInfo.role`, ≤80 chars
    /// total (including any trailing `…`). Omitted when empty.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub role_summary: String,
    /// #1088 - the peer's live context-usage percent (0-100), the same figure
    /// the Sidebar badge shows. Same value as `PeerInfo.context_percent`.
    /// Omitted when there is no reading.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_percent: Option<u8>,
    /// Effective coding-agent id of the peer's chosen live session. Omitted
    /// when the peer has no live session.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Effective profile letter of that session (post-fallback). Omitted when
    /// the peer has no live session or the session is not an agent session.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

/// Maximum length (in Unicode chars) of `roleSummary`, **including any
/// trailing ellipsis**. Any input longer than this limit is truncated to
/// `ROLE_SUMMARY_MAX - 1` chars plus `…`, so the final string is always
/// `≤ ROLE_SUMMARY_MAX` chars.
///
/// Chosen to be:
///   - long enough to convey a one-line role hint;
///   - short enough that a list of 20 peers stays under ~5 KB of JSON
///     worst-case (multi-byte scripts like CJK), well within the 32 KB
///     notification-line budget agents have to read.
const ROLE_SUMMARY_MAX: usize = 80;

/// Compute a single-line, length-capped summary from a full `role` string.
///
/// - Takes the first non-empty trimmed line.
/// - Returns `""` for empty/whitespace-only input, the documented
///   "no-role" sentinels, or the standard AgentsCommander CLAUDE.md
///   preamble openings (which are uniform across replicas and carry no
///   role-hint signal).
/// - If the surviving line is ≤ `ROLE_SUMMARY_MAX` chars, returns it
///   unchanged.
/// - Otherwise returns the first `ROLE_SUMMARY_MAX - 1` chars followed by
///   `…`, so the result is always ≤ `ROLE_SUMMARY_MAX` chars total.
///
/// Note: `roleSummary` is best-effort. When a peer's role document has no
/// `## Role` section (or no `# Role:` heading for WG replicas), the field
/// reflects the first non-heading lines that `extract_role_section` falls
/// back to — which may not be a true role description. Treat the field as
/// a hint, not authoritative.
fn lean_role_summary(role: &str) -> String {
    const NO_ROLE_SENTINELS: &[&str] = &["No role description available.", "Room replica agent."];
    // Standard AgentsCommander preamble openings. These appear verbatim in
    // every replica's CLAUDE.md by design, so when they surface through the
    // `extract_role_section` fallback they carry no discriminating signal —
    // suppress them rather than ship 20 peers with identical roleSummary.
    const PREAMBLE_PREFIXES: &[&str] = &[
        "You are running inside an AgentsCommander session",
        "# AgentsCommander Context",
    ];

    let line = role
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");

    if line.is_empty() || NO_ROLE_SENTINELS.contains(&line) {
        return String::new();
    }
    if PREAMBLE_PREFIXES.iter().any(|p| line.starts_with(p)) {
        return String::new();
    }

    let char_count = line.chars().count();
    if char_count <= ROLE_SUMMARY_MAX {
        return line.to_string();
    }
    // Truncate to MAX-1 chars + ellipsis → exactly MAX chars total.
    let truncated: String = line.chars().take(ROLE_SUMMARY_MAX - 1).collect();
    format!("{}…", truncated)
}

impl From<&PeerInfo> for LeanPeerInfo {
    fn from(p: &PeerInfo) -> Self {
        LeanPeerInfo {
            name: p.name.clone(),
            working: p.working,
            session_status: p.session_status.clone(),
            waiting_for_input: p.waiting_for_input,
            reachable: p.reachable,
            teams: p.teams.clone(),
            role_summary: lean_role_summary(&p.role),
            context_percent: p.context_percent,
            agent: p.agent.clone(),
            profile: p.profile.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PeerInfo {
    name: String,
    path: String,
    /// Legacy: "active" iff working==true, else "unknown".
    /// Preserved verbatim for callers that string-match the old field.
    /// New callers should read `working` / `sessionStatus`.
    status: String,
    role: String,
    teams: Vec<String>,
    reachable: bool,
    last_coding_agent: Option<String>,

    // ── NEW (issue #206) ────────────────────────────────────────────
    /// True iff the peer has a matching session in Running or Active
    /// state AND `waiting_for_input == false`. Mirrors the sidebar
    /// `running-peer` badge predicate (ProjectPanel.tsx:780-786).
    working: bool,
    /// Fine-grained status. One of:
    ///   "active"   — SessionStatus::Active (focused session)
    ///   "running"  — SessionStatus::Running
    ///   "idle"     — SessionStatus::Idle
    ///   "waiting"  — any matching session has waiting_for_input==true
    ///                (overrides underlying SessionStatus, mirrors
    ///                replicaDotClass() at ProjectPanel.tsx:60)
    ///   "exited"   — SessionStatus::Exited(_)
    ///   "none"     — no session matches this peer (WG peers match by
    ///                name+cwd, non-WG peers match by cwd only)
    session_status: String,
    /// UUID of the matched session, when one was found.
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    /// True if the matched session has waiting_for_input.
    waiting_for_input: bool,
    /// Exit code, present iff session_status == "exited".
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_code: Option<i32>,
    /// #1088 - the peer's live context-usage percent (0-100), the same figure
    /// the Sidebar badge shows, read from the matched session's persisted row.
    /// Omitted when there is no reading (agent has no `context_regex`, the
    /// session was not sampled yet, or no session matches this peer).
    #[serde(skip_serializing_if = "Option::is_none")]
    context_percent: Option<u8>,
    /// Effective coding-agent id of the peer's chosen live session (the
    /// session the peer status was computed from). None when no session matched.
    #[serde(skip_serializing_if = "Option::is_none")]
    agent: Option<String>,
    /// Effective profile letter of that session (post-fallback). None when no
    /// session matched or the session is not an agent session.
    #[serde(skip_serializing_if = "Option::is_none")]
    profile: Option<String>,
    // ────────────────────────────────────────────────────────────────
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    coding_agents: HashMap<String, CodingAgentEntry>,
}

// Shadow `agent_name_from_path`/`strip_agent_prefix` removed — canonical
// helpers live in `config::teams` (§AR2-order step 7 / §DR2). Origin agents
// use `agent_name_from_path` (project/agent); WG replicas use
// `agent_fqn_from_path` (project:wg-N/agent).

/// Extract a role description from markdown content: finds the `## Role` section
/// and returns up to 3 lines. Falls back to the first `fallback_lines` non-heading lines.
fn extract_role_section(content: &str, fallback_lines: usize, default_msg: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let mut in_role = false;
    let mut role_lines: Vec<&str> = Vec::new();

    for line in &lines {
        if line.starts_with("## Role Prompt") || line.starts_with("## Role") {
            in_role = true;
            continue;
        }
        if in_role {
            if line.starts_with("## ") || line.starts_with("---") {
                break;
            }
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                role_lines.push(trimmed);
            }
        }
    }

    if !role_lines.is_empty() {
        return role_lines.into_iter().take(3).collect::<Vec<_>>().join(" ");
    }

    let fallback: Vec<&str> = lines
        .iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .take(fallback_lines)
        .collect();

    if fallback.is_empty() {
        default_msg.to_string()
    } else {
        fallback.join(" ")
    }
}

/// Read role from CLAUDE.md: extract ## Role section, or first 5 lines.
fn read_role(repo_path: &str) -> String {
    let claude_md = Path::new(repo_path).join("CLAUDE.md");
    match std::fs::read_to_string(&claude_md) {
        Ok(content) => extract_role_section(&content, 5, "No role description available."),
        Err(_) => "No role description available.".to_string(),
    }
}

// ── Issue #206: working-state derivation from sessions.json ──────────

struct CandidateSession {
    id: String,
    name: String,
    status: SessionStatus,
    waiting_for_input: bool,
    context_percent: Option<u8>,
    agent_id: Option<String>,
    effective_profile: Option<String>,
}

struct PeerStatus {
    working: bool,
    session_status: &'static str,
    status_legacy: &'static str,
    session_id: Option<String>,
    waiting_for_input: bool,
    exit_code: Option<i32>,
    context_percent: Option<u8>,
    agent_id: Option<String>,
    effective_profile: Option<String>,
}

impl PeerStatus {
    fn none() -> Self {
        PeerStatus {
            working: false,
            session_status: "none",
            status_legacy: "unknown",
            session_id: None,
            waiting_for_input: false,
            exit_code: None,
            context_percent: None,
            agent_id: None,
            effective_profile: None,
        }
    }
}

fn norm_path(path: &str) -> String {
    let stripped = path.strip_prefix(r"\\?\").unwrap_or(path);
    stripped
        .replace('\\', "/")
        .to_lowercase()
        .trim_end_matches('/')
        .to_string()
}

fn canon_or_norm(path: &str) -> String {
    match std::fs::canonicalize(path) {
        Ok(canon) => norm_path(&canon.to_string_lossy()),
        Err(_) => norm_path(path),
    }
}

/// Pure inner: build the cwd → candidate index from a slice of persisted rows.
/// Exposed (private) so unit tests can drive it without touching the filesystem.
fn build_session_index_from(rows: &[PersistedSession]) -> HashMap<String, Vec<CandidateSession>> {
    let mut index: HashMap<String, Vec<CandidateSession>> = HashMap::new();
    for ps in rows {
        if ps.name.starts_with(TEMP_SESSION_PREFIX) {
            continue;
        }
        let (Some(id), Some(status)) = (ps.id.clone(), ps.status.clone()) else {
            continue;
        };
        let key = canon_or_norm(&ps.working_directory);
        index.entry(key).or_default().push(CandidateSession {
            id,
            name: ps.name.clone(),
            status,
            waiting_for_input: ps.waiting_for_input.unwrap_or(false),
            context_percent: ps.context_percent,
            agent_id: ps.agent_id.clone(),
            effective_profile: ps.effective_profile.clone(),
        });
    }
    index
}

/// Production entry point: read sessions.json and build the index.
fn build_session_index() -> HashMap<String, Vec<CandidateSession>> {
    build_session_index_from(&load_sessions_raw())
}

/// Priority: waiting(4) > active(3) > running(2) > idle(1) > exited(0).
/// Uses `match &c.status` to avoid moving the non-Copy `SessionStatus`
/// (matches the proven pattern in `list_sessions.rs:status_tag`).
fn priority(c: &CandidateSession) -> u8 {
    if c.waiting_for_input {
        return 4;
    }
    match &c.status {
        SessionStatus::Active => 3,
        SessionStatus::Running => 2,
        SessionStatus::Idle => 1,
        SessionStatus::Exited(_) => 0,
    }
}

/// Compute a peer's working state.
///
/// `expected_name`:
///   - `Some("wg/agent")` for WG peers → filters candidates by exact session
///     name to mirror the sidebar's `findSessionByName` predicate.
///   - `None` for non-WG peers → cwd-only match (no sidebar predicate to
///     mirror; see §6.1 and §10.8 of the plan).
fn compute_peer_status(
    peer_path: &str,
    expected_name: Option<&str>,
    index: &HashMap<String, Vec<CandidateSession>>,
) -> PeerStatus {
    let key = canon_or_norm(peer_path);
    let Some(candidates) = index.get(&key) else {
        return PeerStatus::none();
    };

    let filtered: Vec<&CandidateSession> = match expected_name {
        Some(name) => candidates.iter().filter(|c| c.name == name).collect(),
        None => candidates.iter().collect(),
    };

    let Some(chosen) = filtered.iter().copied().max_by_key(|c| priority(c)) else {
        return PeerStatus::none();
    };

    let (session_status, status_legacy, working, exit_code): (&str, &str, bool, Option<i32>) =
        if chosen.waiting_for_input {
            ("waiting", "unknown", false, None)
        } else {
            match &chosen.status {
                SessionStatus::Active => ("active", "active", true, None),
                SessionStatus::Running => ("running", "active", true, None),
                SessionStatus::Idle => ("idle", "unknown", false, None),
                SessionStatus::Exited(n) => ("exited", "unknown", false, Some(*n)),
            }
        };

    PeerStatus {
        working,
        session_status,
        status_legacy,
        session_id: Some(chosen.id.clone()),
        waiting_for_input: chosen.waiting_for_input,
        exit_code,
        context_percent: chosen.context_percent,
        agent_id: chosen.agent_id.clone(),
        effective_profile: chosen.effective_profile.clone(),
    }
}

struct WgReplicaInfo {
    my_agent_name: String,
    my_wg_name: String,
    my_wg_dir: PathBuf,
    ac_root: PathBuf,
    /// Project folder name (the dir containing the Project AC Root). Forms the
    /// LHS of the canonical FQN for WG replicas.
    my_project: String,
}

/// Detect if `root` is a WG replica: path matches `*/<project_ac_root>/wg-*/__agent_*/`.
///
/// Thin wrapper over the shared walk-up in
/// `config::ac_root::wg_replica_layout_from_agent_dir` (single source with
/// `send::derive_root_project_dir` and `mailbox::derive_project_from_outbox_path`,
/// see #726). This caller keeps the project FOLDER NAME (`my_project`), the LHS
/// of a WG replica's canonical FQN, rather than the full project path.
fn detect_wg_replica(root: &str) -> Result<Option<WgReplicaInfo>, String> {
    let canon = match std::fs::canonicalize(root) {
        Ok(c) => c,
        Err(_) => return Ok(None),
    };
    let Some(layout) = crate::config::ac_root::wg_replica_layout_from_agent_dir(&canon)? else {
        return Ok(None);
    };
    let Some(my_project) = layout
        .project_dir
        .file_name()
        .and_then(|name| name.to_str())
    else {
        return Ok(None);
    };

    Ok(Some(WgReplicaInfo {
        my_agent_name: layout.agent_name,
        my_wg_name: layout.wg_name,
        my_wg_dir: layout.wg_dir,
        ac_root: layout.ac_root,
        my_project: my_project.to_string(),
    }))
}

/// Resolve the coordinator agent name for a WG by matching replica identity
/// paths against the team coordinator path in Project AC Root `_team_*/config.json`.
/// Only checks the team whose name matches the WG suffix (e.g. `wg-1-ac-devs` → `_team_ac-devs`).
fn resolve_wg_coordinator(ac_root: &Path, wg_dir: &Path) -> Option<String> {
    crate::config::teams::resolve_wg_coordinator_replica(ac_root, wg_dir)
        .map(|replica| replica.agent_name)
}

/// Read role from a WG replica's identity matrix Role.md, falling back to CLAUDE.md.
fn read_wg_role(replica_dir: &Path) -> String {
    let matrix_dir = match crate::config::replica_identity::read_and_repair_wg_replica_config(
        replica_dir,
        crate::config::replica_identity::WG_REPLICA_REQUIRED_CONTEXT,
    ) {
        Ok((_config, identity)) => identity.matrix_dir,
        Err(e) => {
            log::warn!(
                "[list-peers] Rejected invalid WG replica identity for '{}': {}",
                replica_dir.display(),
                e
            );
            return "Room replica agent.".to_string();
        }
    };
    let role_path = matrix_dir.join("Role.md");
    match std::fs::read_to_string(&role_path) {
        Ok(content) => extract_role_section(&content, 3, "Room replica agent."),
        Err(_) => read_role(&matrix_dir.to_string_lossy()),
    }
}

/// Build a PeerInfo for a WG replica directory. Also bootstraps IPC dirs.
/// `project` is the project folder name (dir containing the Project AC Root) and forms
/// the LHS of the canonical FQN.
fn build_wg_peer(
    project: &str,
    agent_name: &str,
    wg_name: &str,
    agent_path: &Path,
    reachable: bool,
    session_index: &HashMap<String, Vec<CandidateSession>>,
) -> PeerInfo {
    let replica_ac = agent_path.join(crate::config::agent_local_dir_name());
    let _ = std::fs::create_dir_all(replica_ac.join("inbox"));
    let _ = std::fs::create_dir_all(replica_ac.join("outbox"));

    let peer_config: AgentLocalConfig = replica_ac
        .join("config.json")
        .to_str()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default();

    let expected_session_name = format!("{}/{}", wg_name, agent_name);
    let ps = compute_peer_status(
        &agent_path.to_string_lossy(),
        Some(&expected_session_name),
        session_index,
    );

    PeerInfo {
        name: format!("{}:{}/{}", project, wg_name, agent_name),
        path: agent_path.to_string_lossy().to_string(),
        status: ps.status_legacy.to_string(),
        role: read_wg_role(agent_path),
        teams: vec![wg_name.to_string()],
        reachable,
        last_coding_agent: peer_config.tooling.last_coding_agent,
        working: ps.working,
        session_status: ps.session_status.to_string(),
        session_id: ps.session_id,
        waiting_for_input: ps.waiting_for_input,
        exit_code: ps.exit_code,
        context_percent: ps.context_percent,
        agent: ps.agent_id,
        profile: ps.effective_profile,
        coding_agents: peer_config.tooling.coding_agents,
    }
}

/// Pure (filesystem-bound) discovery for WG replicas. Returns the same vector
/// that the legacy `execute_wg_discovery` would have serialized. Shared by
/// `execute` and `execute_lean` so the peer set is identical by construction
/// for both verbs (see issue #252).
fn discover_wg_peers(wg: WgReplicaInfo) -> Vec<PeerInfo> {
    let session_index = build_session_index();
    let mut peers: Vec<PeerInfo> = Vec::new();
    let discovered = crate::config::teams::discover_teams();
    // Canonical FQN: `<project>:<wg>/<agent>`. All downstream routing checks
    // (`can_communicate`) compare project-qualified strings.
    let my_full_name = format!("{}:{}/{}", wg.my_project, wg.my_wg_name, wg.my_agent_name);

    let coordinator = resolve_wg_coordinator(&wg.ac_root, &wg.my_wg_dir);
    let i_am_coordinator = coordinator.as_deref() == Some(wg.my_agent_name.as_str());

    if coordinator.is_none() {
        if std::env::var("AC_MACHINE_OUTPUT").is_err() {
            eprintln!(
                "Warning: no orchestrator found for Room '{}', showing all replicas",
                wg.my_wg_name
            );
        } else {
            log::warn!(
                "Warning: no orchestrator found for Room '{}', showing all replicas",
                wg.my_wg_name
            );
        }
    }

    // Collect all replicas in my WG
    let replicas: Vec<(String, PathBuf)> = std::fs::read_dir(&wg.my_wg_dir)
        .into_iter()
        .flat_map(|rd| rd.flatten())
        .filter_map(|e| {
            let p = e.path();
            if !p.is_dir() {
                return None;
            }
            let name = p.file_name()?.to_str()?;
            let agent = name.strip_prefix("__agent_")?.to_string();
            Some((agent, p))
        })
        .collect();

    for (agent_name, agent_path) in &replicas {
        if *agent_name == wg.my_agent_name {
            continue;
        }
        let peer_full_name = format!("{}:{}/{}", wg.my_project, wg.my_wg_name, agent_name);
        let reachable =
            crate::config::teams::can_communicate(&my_full_name, &peer_full_name, &discovered);
        peers.push(build_wg_peer(
            &wg.my_project,
            agent_name,
            &wg.my_wg_name,
            agent_path,
            reachable,
            &session_index,
        ));
    }

    // Coordinator also sees coordinators of other WGs in the same Project AC Root
    // (same project, different WG — still qualified with `wg.my_project`).
    if i_am_coordinator {
        if let Ok(entries) = std::fs::read_dir(&wg.ac_root) {
            for entry in entries.flatten() {
                let other_wg_dir = entry.path();
                if !other_wg_dir.is_dir() {
                    continue;
                }
                let other_wg_name = match other_wg_dir.file_name().and_then(|n| n.to_str()) {
                    Some(n)
                        if crate::config::entity_prefix::has_entity_prefix(n)
                            && n != wg.my_wg_name =>
                    {
                        n.to_string()
                    }
                    _ => continue,
                };

                if let Some(other_coord) = resolve_wg_coordinator(&wg.ac_root, &other_wg_dir) {
                    let coord_dir = other_wg_dir.join(format!("__agent_{}", other_coord));
                    if !coord_dir.is_dir() {
                        continue;
                    }
                    let peer_name = format!("{}:{}/{}", wg.my_project, other_wg_name, other_coord);
                    if peers.iter().any(|p| p.name == peer_name) {
                        continue;
                    }
                    let reachable = crate::config::teams::can_communicate(
                        &my_full_name,
                        &peer_name,
                        &discovered,
                    );
                    peers.push(build_wg_peer(
                        &wg.my_project,
                        &other_coord,
                        &other_wg_name,
                        &coord_dir,
                        reachable,
                        &session_index,
                    ));
                }
            }
        }
    }

    // #293 — synthetic Root Agent reply peer (identity-verified coordinators
    // only). Augment `settings.project_paths` with the caller's own project
    // dir so `verified_wg_coordinator_target` sees the replica even when the
    // project isn't in settings. Mirrors `cli/send.rs::effective_project_paths`
    // (send.rs `derive_root_project_dir` augmentation) and the mailbox
    // root-recipient arm's `derive_project_from_outbox_path` augmentation.
    let mut effective_paths = crate::config::settings::load_settings()
        .project_paths
        .clone();
    if let Some(project_dir) = wg.ac_root.parent() {
        if let Some(project_str) = project_dir.to_str() {
            let p = project_str.to_string();
            let canon_p = std::fs::canonicalize(&p).ok();
            let already_present = effective_paths.iter().any(|x| match &canon_p {
                Some(c) => std::fs::canonicalize(x).ok().as_ref() == Some(c),
                None => x == &p,
            });
            if !already_present {
                effective_paths.push(p);
            }
        }
    }
    if let Some(root_peer) = build_root_agent_synthetic_peer(&wg, &effective_paths) {
        peers.push(root_peer);
    }

    peers
}

/// Discovery for non-WG-replica roots: standard team membership scan + a
/// WG-replica scan across `settings.project_paths`. Returns the same vector
/// that the legacy `execute` body would have serialized. Shared by `execute`
/// and `execute_lean` so the peer set is identical by construction for both
/// verbs (see issue #252).
fn discover_origin_peers(root: &str) -> Vec<PeerInfo> {
    // ── Standard discovery-based peer listing ────────────────────────
    //
    // `execute` is the non-WG-replica path (WG replicas return early above).
    // `root` is an origin matrix agent CWD → `agent_fqn_from_path` gives the
    // origin form `project/agent` (identical to the legacy behavior for
    // non-WG paths). Using the canonical helper eliminates the shadow.
    let my_name = crate::config::teams::agent_fqn_from_path(root);
    let discovered = crate::config::teams::discover_teams();
    let session_index = build_session_index();

    let mut peers: Vec<PeerInfo> = Vec::new();

    // Find teams where I'm a member, then list their other members.
    // Also: if I'm a coordinator, show other coordinators (cross-team).
    let i_am_coordinator = discovered.iter().any(|t| {
        crate::config::teams::is_in_team(&my_name, t)
            && t.coordinator_name
                .as_deref()
                .is_some_and(|cn| cn == my_name)
    });

    for team in &discovered {
        let i_am_in_team = crate::config::teams::is_in_team(&my_name, team);

        if !i_am_in_team && !i_am_coordinator {
            continue;
        }

        for (i, display_name) in team.agent_names.iter().enumerate() {
            let member_path = team.agent_paths.get(i).and_then(|p| p.as_ref());
            let peer_name = member_path
                .map(|p| crate::config::teams::agent_fqn_from_path(&p.to_string_lossy()))
                .unwrap_or_else(|| display_name.clone());

            // Skip ourselves
            if peer_name == my_name || display_name == &my_name {
                continue;
            }

            // Determine reachability using the canonical routing rules
            let reachable =
                crate::config::teams::can_communicate(&my_name, &peer_name, &discovered);

            // Skip duplicates — add team to existing peer, upgrade reachable if needed
            if let Some(existing) = peers.iter_mut().find(|p| p.name == peer_name) {
                if !existing.teams.contains(&team.name) {
                    existing.teams.push(team.name.clone());
                }
                // If reachable via any team, mark as reachable
                if reachable {
                    existing.reachable = true;
                }
                continue;
            }

            let path_str = member_path
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();

            let peer_ac = member_path
                .map(|p| p.join(crate::config::agent_local_dir_name()))
                .unwrap_or_else(|| {
                    PathBuf::from(&path_str).join(crate::config::agent_local_dir_name())
                });

            let peer_config: AgentLocalConfig = peer_ac
                .join("config.json")
                .to_str()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .and_then(|c| serde_json::from_str(&c).ok())
                .unwrap_or_default();

            let ps = compute_peer_status(&path_str, None, &session_index);
            peers.push(PeerInfo {
                name: peer_name,
                path: path_str,
                status: ps.status_legacy.to_string(),
                role: member_path
                    .map(|p| read_role(&p.to_string_lossy()))
                    .unwrap_or_else(|| "No role description available.".to_string()),
                teams: vec![team.name.clone()],
                reachable,
                last_coding_agent: peer_config.tooling.last_coding_agent,
                working: ps.working,
                session_status: ps.session_status.to_string(),
                session_id: ps.session_id,
                waiting_for_input: ps.waiting_for_input,
                exit_code: ps.exit_code,
                context_percent: ps.context_percent,
                agent: ps.agent_id,
                profile: ps.effective_profile,
                coding_agents: peer_config.tooling.coding_agents,
            });
        }
    }

    // ── WG replica discovery ──────────────────────────────────────────────
    // Scan project_paths for Project AC Root wg-*/__agent_* replicas
    let settings = crate::config::settings::load_settings();
    for base_path in &settings.project_paths {
        let base = Path::new(base_path);
        if !base.is_dir() {
            continue;
        }
        // Check base and its immediate children (same pattern as ac_discovery)
        let mut dirs_to_check = vec![base.to_path_buf()];
        if let Ok(entries) = std::fs::read_dir(base) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if !name.starts_with('.') {
                        dirs_to_check.push(p);
                    }
                }
            }
        }
        for repo_dir in dirs_to_check {
            let Some(ac_root) = existing_ac_root(&repo_dir) else {
                continue;
            };
            // Project folder name (parent of Project AC Root) is the LHS of the canonical FQN.
            let project_folder = match repo_dir.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            let wg_entries = match std::fs::read_dir(&ac_root) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for wg_entry in wg_entries.flatten() {
                let wg_path = wg_entry.path();
                if !wg_path.is_dir() {
                    continue;
                }
                let wg_name = match wg_path.file_name().and_then(|n| n.to_str()) {
                    Some(n) if crate::config::entity_prefix::has_entity_prefix(n) => n.to_string(),
                    _ => continue,
                };
                // Derive team name from the entity name: "room-1-ac-devs" or
                // the legacy "wg-1-ac-devs" → "ac-devs"
                let wg_team = crate::config::entity_prefix::strip_entity_prefix(&wg_name)
                    .and_then(|s| s.split_once('-').map(|(_, rest)| rest))
                    .unwrap_or(&wg_name)
                    .to_string();

                let agent_entries = match std::fs::read_dir(&wg_path) {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                for agent_entry in agent_entries.flatten() {
                    let agent_path = agent_entry.path();
                    if !agent_path.is_dir() {
                        continue;
                    }
                    let agent_dir = match agent_path.file_name().and_then(|n| n.to_str()) {
                        Some(n) if n.starts_with("__agent_") => n.to_string(),
                        _ => continue,
                    };
                    let agent_short = agent_dir
                        .strip_prefix("__agent_")
                        .unwrap_or(&agent_dir)
                        .to_string();
                    let peer_name = format!("{}:{}/{}", project_folder, wg_name, agent_short);

                    // Skip self
                    if peer_name == my_name {
                        continue;
                    }
                    // Skip duplicates
                    if peers.iter().any(|p| p.name == peer_name) {
                        continue;
                    }

                    let reachable =
                        crate::config::teams::can_communicate(&my_name, &peer_name, &discovered);
                    let mut peer = build_wg_peer(
                        &project_folder,
                        &agent_short,
                        &wg_name,
                        &agent_path,
                        reachable,
                        &session_index,
                    );
                    peer.teams = vec![wg_team.clone()];
                    peers.push(peer);
                }
            }
        }
    }

    peers
}

fn discover_root_coordinator_peers() -> Vec<PeerInfo> {
    let settings = crate::config::settings::load_settings();
    discover_root_coordinator_peers_from_project_paths(&settings.project_paths)
}

fn discover_root_coordinator_peers_from_project_paths(project_paths: &[String]) -> Vec<PeerInfo> {
    let session_index = build_session_index();
    let mut peers: Vec<PeerInfo> = Vec::new();

    for base_path in project_paths {
        let base = Path::new(base_path);
        if !base.is_dir() {
            continue;
        }

        let mut dirs_to_check = vec![base.to_path_buf()];
        if let Ok(entries) = std::fs::read_dir(base) {
            for entry in entries.flatten() {
                let p = entry.path();
                if !p.is_dir() {
                    continue;
                }
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if !name.starts_with('.') {
                    dirs_to_check.push(p);
                }
            }
        }

        for repo_dir in dirs_to_check {
            let Some(ac_root) = existing_ac_root(&repo_dir) else {
                continue;
            };

            let wg_entries = match std::fs::read_dir(&ac_root) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for wg_entry in wg_entries.flatten() {
                let wg_path = wg_entry.path();
                if !wg_path.is_dir() {
                    continue;
                }
                match wg_path.file_name().and_then(|n| n.to_str()) {
                    Some(n) if crate::config::entity_prefix::has_entity_prefix(n) => {}
                    _ => continue,
                }

                let Some(coord) =
                    crate::config::teams::resolve_wg_coordinator_replica(&ac_root, &wg_path)
                else {
                    continue;
                };
                let peer_name = format!("{}:{}/{}", coord.project, coord.wg_name, coord.agent_name);
                if peers.iter().any(|p| p.name == peer_name) {
                    continue;
                }
                let mut peer = build_wg_peer(
                    &coord.project,
                    &coord.agent_name,
                    &coord.wg_name,
                    &coord.replica_dir,
                    true,
                    &session_index,
                );
                peer.teams = vec![coord.team];
                peers.push(peer);
            }
        }
    }

    peers
}

/// Build a synthetic `PeerInfo` for the canonical Root Agent reply target,
/// or `None` if the caller is not an identity-verified WG coordinator (the
/// only relationship that can address the Root Agent — see #293 §3).
///
/// `wg` is the caller's WG-replica detection result (already computed by
/// `discover_peers`); `project_paths` is the effective settings slice the
/// caller scanned.
fn build_root_agent_synthetic_peer(
    wg: &WgReplicaInfo,
    project_paths: &[String],
) -> Option<PeerInfo> {
    // The caller is a coordinator iff `verified_wg_coordinator_target`
    // accepts its own FQN. Re-uses the same identity-cross-reference path
    // (#277 §2.3) used by the root-sender side.
    let my_fqn = format!("{}:{}/{}", wg.my_project, wg.my_wg_name, wg.my_agent_name);
    crate::config::teams::verified_wg_coordinator_target(&my_fqn, project_paths)?;

    let root_dir = crate::config::root_agent::root_agent_dir().ok()?;
    Some(PeerInfo {
        name: crate::config::root_agent::ROOT_AGENT_SENDER.to_string(),
        path: root_dir,
        // `status`/`session_status` reflect "we don't track it from here".
        // The root session is in a different working-directory namespace
        // than this verb scans — its working state is invisible to
        // `compute_peer_status`.
        status: "unknown".to_string(),
        role: "AgentsCommander Root Agent (canonical reply target).".to_string(),
        teams: Vec::new(),
        reachable: true,
        last_coding_agent: None,
        working: false,
        session_status: "none".to_string(),
        session_id: None,
        waiting_for_input: false,
        exit_code: None,
        // Synthetic root-agent peer: no session in this verb's cwd namespace.
        context_percent: None,
        agent: None,
        profile: None,
        coding_agents: std::collections::HashMap::new(),
    })
}

/// Apply the `--peer` filter to a discovered peer list.
///
/// Consumes `peers` by value so filtering does not require `PeerInfo: Clone`.
fn apply_peer_filter(
    peers: Vec<PeerInfo>,
    requested: &[String],
) -> Result<Vec<PeerInfo>, Vec<String>> {
    if requested.is_empty() {
        return Ok(peers);
    }

    // Dedupe `requested` while preserving the user's order for unique entries.
    let mut unique: Vec<String> = Vec::with_capacity(requested.len());
    for name in requested {
        if !unique.iter().any(|n| n == name) {
            unique.push(name.clone());
        }
    }

    // Move peers into a name→PeerInfo map so emission is O(1) per requested
    // name and we don't need PeerInfo: Clone. Discovery never produces
    // duplicate names (see `discover_origin_peers` / `discover_wg_peers`),
    // so this collapse is lossless.
    let mut by_name: HashMap<String, PeerInfo> =
        peers.into_iter().map(|p| (p.name.clone(), p)).collect();

    let unknown: Vec<String> = unique
        .iter()
        .filter(|n| !by_name.contains_key(n.as_str()))
        .cloned()
        .collect();
    if !unknown.is_empty() {
        return Err(unknown);
    }

    let filtered: Vec<PeerInfo> = unique
        .into_iter()
        .filter_map(|name| by_name.remove(&name))
        .collect();
    Ok(filtered)
}

/// Emit the unknown-peer error to stderr and return exit code 1. Used by
/// both `execute` and `execute_lean` so the message shape is identical.
fn report_unknown_peers(unknown: &[String], available: &[String]) -> i32 {
    eprintln!(
        "Error: unknown peer(s) requested via --peer: {}",
        unknown.join(", ")
    );
    if available.is_empty() {
        eprintln!("No peers were discovered for this --root.");
    } else {
        eprintln!("Available peers: {}", available.join(", "));
    }
    1
}

/// Run the shared discovery — WG-replica path when `root` is a replica, the
/// origin-agent path otherwise. Factored so `execute` and `execute_lean`
/// share the dispatch.
fn discover_peers(root: &str) -> Result<Vec<PeerInfo>, String> {
    if crate::config::root_agent::is_root_agent_path(root) {
        Ok(discover_root_coordinator_peers())
    } else if let Some(wg) = detect_wg_replica(root)? {
        Ok(discover_wg_peers(wg))
    } else {
        Ok(discover_origin_peers(root))
    }
}

/// #791 §8.2 - public in-process `list-peers-lean` for the in-daemon API.
///
/// Runs the SAME discovery as `execute_lean` (WG-replica / origin / root
/// dispatch) and the SAME `reachable` computation (via `can_communicate`),
/// returning the lean projection directly instead of printing to stdout. The
/// API handler calls this with the caller's bound replica root, so an API
/// `peers` response is byte-identical to the CLI's for the same fixture. Keeps
/// `PeerInfo` and `discover_peers` private; only `LeanPeerInfo` is exposed.
pub fn lean_peers(root: &str) -> Result<Vec<LeanPeerInfo>, String> {
    let peers = discover_peers(root)?;
    Ok(peers.iter().map(LeanPeerInfo::from).collect())
}

fn serialize_full_peers(peers: &[PeerInfo]) -> i32 {
    match serde_json::to_string_pretty(peers) {
        Ok(json) => {
            crate::cli_println!("{}", json);
            0
        }
        Err(e) => {
            eprintln!("Error: failed to serialize peers: {}", e);
            1
        }
    }
}

fn serialize_lean_peers(peers: &[PeerInfo]) -> i32 {
    let lean: Vec<LeanPeerInfo> = peers.iter().map(LeanPeerInfo::from).collect();
    match serde_json::to_string_pretty(&lean) {
        Ok(json) => {
            crate::cli_println!("{}", json);
            0
        }
        Err(e) => {
            eprintln!("Error: failed to serialize peers: {}", e);
            1
        }
    }
}

pub fn execute(args: ListPeersArgs) -> i32 {
    // Validate token before any discovery
    if let Err(msg) = crate::cli::validate_cli_token(&args.token) {
        eprintln!("{}", msg);
        return 1;
    }

    let root = match args.root {
        Some(ref r) => r.clone(),
        None => {
            eprintln!("Error: --root is required. Specify your agent's root directory.");
            return 1;
        }
    };

    let peers = match discover_peers(&root) {
        Ok(peers) => peers,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };

    // No-filter fast path: behavior is byte-for-byte unchanged from the
    // pre-filter implementation when --peer is not supplied.
    if args.peer.is_empty() {
        return serialize_full_peers(&peers);
    }

    let available: Vec<String> = peers.iter().map(|p| p.name.clone()).collect();
    match apply_peer_filter(peers, &args.peer) {
        Ok(filtered) => serialize_full_peers(&filtered),
        Err(unknown) => report_unknown_peers(&unknown, &available),
    }
}

fn snapshot_target_projection(
    target: crate::config::teams::TerminalSnapshotTargetIdentity,
    reachable: bool,
) -> LeanPeerInfo {
    LeanPeerInfo {
        name: target.canonical_fqn,
        working: false,
        session_status: "unknown".to_string(),
        waiting_for_input: false,
        reachable,
        teams: vec![target.team],
        role_summary: String::new(),
        context_percent: None,
        agent: None,
        profile: None,
    }
}

fn discover_snapshot_targets(root: &str) -> Result<Vec<LeanPeerInfo>, String> {
    let root_path = Path::new(root);
    if crate::config::root_agent::is_root_agent_path(root) {
        let expected = crate::config::root_agent::verify_live_root_agent_path(root_path)?;
        let supplied = crate::path_identity::verify_directory(root_path)?;
        if !crate::path_identity::same_object(&expected, &supplied) {
            return Err("invalid snapshot discovery root".to_string());
        }
        let project_paths = crate::config::settings::read_terminal_snapshot_project_paths_strict()?;
        let targets =
            crate::config::teams::discover_verified_terminal_snapshot_targets(&project_paths)?;
        return Ok(targets
            .into_iter()
            .map(|target| {
                let reachable = target.is_coordinator;
                snapshot_target_projection(target, reachable)
            })
            .collect());
    }

    let Some(wg) = detect_wg_replica(root)? else {
        return Ok(Vec::new());
    };
    let requester = crate::config::teams::verify_pty_input_replica_cwd(root_path)?;
    let supplied = crate::path_identity::verify_directory(root_path)?;
    if !crate::path_identity::same_object(&requester.replica_identity, &supplied) {
        return Err("invalid snapshot discovery root".to_string());
    }
    if !requester.is_coordinator {
        return Ok(Vec::new());
    }
    let project_dir = wg
        .ac_root
        .parent()
        .ok_or_else(|| "snapshot project path unavailable".to_string())?
        .to_string_lossy()
        .to_string();
    let targets =
        crate::config::teams::discover_verified_terminal_snapshot_targets(&[project_dir])?;
    Ok(targets
        .into_iter()
        .filter(|target| {
            target.project == requester.project
                && target.workgroup == requester.workgroup
                && !target.is_coordinator
        })
        .map(|target| snapshot_target_projection(target, true))
        .collect())
}

fn apply_snapshot_target_filter(
    peers: Vec<LeanPeerInfo>,
    requested: &[String],
) -> Result<Vec<LeanPeerInfo>, Vec<String>> {
    if requested.is_empty() {
        return Ok(peers);
    }
    let mut by_name: HashMap<String, LeanPeerInfo> = peers
        .into_iter()
        .map(|peer| (peer.name.clone(), peer))
        .collect();
    let mut seen = std::collections::HashSet::new();
    let mut filtered = Vec::new();
    let mut unknown = Vec::new();
    for name in requested {
        if !seen.insert(name.clone()) {
            continue;
        }
        match by_name.remove(name) {
            Some(peer) => filtered.push(peer),
            None => unknown.push(name.clone()),
        }
    }
    if unknown.is_empty() {
        Ok(filtered)
    } else {
        Err(unknown)
    }
}

fn serialize_snapshot_targets(peers: &[LeanPeerInfo]) -> i32 {
    match serde_json::to_string_pretty(peers) {
        Ok(json) => {
            crate::cli_println!("{}", json);
            0
        }
        Err(_) => {
            eprintln!("Error: failed to serialize terminal snapshot targets");
            1
        }
    }
}

pub fn execute_lean(args: ListPeersLeanArgs) -> i32 {
    // Snapshot target discovery is shape-only and side-effect-free. In
    // particular, it must not load or repair settings merely to inspect a
    // persisted static token. Default discovery retains its existing model.
    let token_validation = if args.snapshot_targets {
        args.token
            .as_deref()
            .filter(|token| uuid::Uuid::parse_str(token).is_ok())
            .map(|_| ())
            .ok_or_else(|| {
                "Error: --token is required and must be a UUID-shaped session token.".to_string()
            })
    } else {
        crate::cli::validate_cli_token(&args.token).map(|_| ())
    };
    if let Err(message) = token_validation {
        eprintln!("{}", message);
        return 1;
    }

    let root = match args.root {
        Some(ref r) => r.clone(),
        None => {
            eprintln!("Error: --root is required. Specify your agent's root directory.");
            return 1;
        }
    };

    if args.snapshot_targets {
        let peers = match discover_snapshot_targets(&root) {
            Ok(peers) => peers,
            Err(error) => {
                eprintln!("Error: {}", error);
                return 1;
            }
        };
        let available: Vec<String> = peers.iter().map(|peer| peer.name.clone()).collect();
        return match apply_snapshot_target_filter(peers, &args.peer) {
            Ok(filtered) => serialize_snapshot_targets(&filtered),
            Err(unknown) => report_unknown_peers(&unknown, &available),
        };
    }

    let peers = match discover_peers(&root) {
        Ok(peers) => peers,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };

    if args.peer.is_empty() {
        return serialize_lean_peers(&peers);
    }

    let available: Vec<String> = peers.iter().map(|p| p.name.clone()).collect();
    match apply_peer_filter(peers, &args.peer) {
        Ok(filtered) => serialize_lean_peers(&filtered),
        Err(unknown) => report_unknown_peers(&unknown, &available),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::sessions_persistence::PersistedSession;
    use crate::session::session::SessionStatus;

    fn cand(name: &str, status: SessionStatus, waiting: bool) -> CandidateSession {
        CandidateSession {
            id: "11111111-1111-1111-1111-111111111111".to_string(),
            name: name.to_string(),
            status,
            waiting_for_input: waiting,
            context_percent: None,
            agent_id: None,
            effective_profile: None,
        }
    }

    /// Build a minimal PersistedSession for build_session_index_from tests.
    fn ps_row(
        name: &str,
        cwd: &str,
        status: Option<SessionStatus>,
        id_present: bool,
    ) -> PersistedSession {
        PersistedSession {
            name: name.to_string(),
            working_directory: cwd.to_string(),
            id: if id_present {
                Some("11111111-1111-1111-1111-111111111111".to_string())
            } else {
                None
            },
            status,
            waiting_for_input: Some(false),
            ..Default::default()
        }
    }

    fn make_root_discovery_fixture(
        spoofed_coordinator_identity: bool,
    ) -> (tempfile::TempDir, Vec<String>) {
        let temp = tempfile::TempDir::new().unwrap();
        let project = temp.path().join("proj-a");
        let ac_root = project.join(".ac");
        let team_dir = ac_root.join("_team_dev-team");
        let origin_tech_lead = ac_root.join("_agent_tech-lead");
        let origin_dev_rust = ac_root.join("_agent_dev-rust");
        let wg_dir = ac_root.join("wg-1-dev-team");
        let tech_lead_replica = wg_dir.join("__agent_tech-lead");
        let dev_rust_replica = wg_dir.join("__agent_dev-rust");

        for dir in [
            &team_dir,
            &origin_tech_lead,
            &origin_dev_rust,
            &tech_lead_replica,
            &dev_rust_replica,
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }

        std::fs::write(
            team_dir.join("config.json"),
            r#"{"agents":["../_agent_dev-rust"],"coordinator":"../_agent_tech-lead"}"#,
        )
        .unwrap();
        let tech_lead_identity = if spoofed_coordinator_identity {
            "../../_agent_dev-rust"
        } else {
            "../../_agent_tech-lead"
        };
        std::fs::write(
            tech_lead_replica.join("config.json"),
            format!(r#"{{"identity":"{}"}}"#, tech_lead_identity),
        )
        .unwrap();
        std::fs::write(
            dev_rust_replica.join("config.json"),
            r#"{"identity":"../../_agent_dev-rust"}"#,
        )
        .unwrap();

        let paths = vec![temp.path().to_string_lossy().to_string()];
        (temp, paths)
    }

    fn make_portable_cross_wg_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempfile::TempDir::new().unwrap();
        let project = temp.path().join("proj-a");
        let ac_root = project.join(".ac");
        let team_dir = ac_root.join("_team_dev-team");
        let local_tech_lead = ac_root.join("_agent_tech-lead");
        let local_dev_rust = ac_root.join("_agent_dev-rust");
        let origin = temp.path().join("origin-matrix").join(".ac");
        let origin_tech_lead = origin.join("_agent_tech-lead");
        let origin_dev_rust = origin.join("_agent_dev-rust");

        for dir in [
            &team_dir,
            &local_tech_lead,
            &local_dev_rust,
            &origin_tech_lead,
            &origin_dev_rust,
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(
            team_dir.join("config.json"),
            r#"{"agents":["_agent_dev-rust"],"coordinator":"_agent_tech-lead"}"#,
        )
        .unwrap();

        let tech_lead_identity = origin_tech_lead.to_string_lossy().replace('\\', "/");
        let dev_rust_identity = origin_dev_rust.to_string_lossy().replace('\\', "/");
        let mut wg1_tech_lead = PathBuf::new();
        let mut wg1_dev_rust = PathBuf::new();
        for wg_name in ["wg-1-dev-team", "wg-2-dev-team"] {
            let wg_dir = ac_root.join(wg_name);
            let tech_lead_replica = wg_dir.join("__agent_tech-lead");
            let dev_rust_replica = wg_dir.join("__agent_dev-rust");
            std::fs::create_dir_all(&tech_lead_replica).unwrap();
            std::fs::create_dir_all(&dev_rust_replica).unwrap();
            std::fs::write(
                tech_lead_replica.join("config.json"),
                format!(r#"{{"identity":"{}"}}"#, tech_lead_identity),
            )
            .unwrap();
            std::fs::write(
                dev_rust_replica.join("config.json"),
                format!(r#"{{"identity":"{}"}}"#, dev_rust_identity),
            )
            .unwrap();
            if wg_name == "wg-1-dev-team" {
                wg1_tech_lead = tech_lead_replica;
                wg1_dev_rust = dev_rust_replica;
            }
        }

        (temp, wg1_tech_lead, wg1_dev_rust)
    }

    #[test]
    fn detect_wg_replica_accepts_ac_root() {
        let temp = tempfile::TempDir::new().unwrap();
        let project = temp.path().join("proj-a");
        let root = project.join(".ac").join("wg-1-devs").join("__agent_alice");
        std::fs::create_dir_all(&root).unwrap();

        let wg = detect_wg_replica(root.to_str().unwrap())
            .unwrap()
            .expect("Project AC Root should be accepted");
        assert_eq!(wg.my_project, "proj-a");
        assert_eq!(wg.my_wg_name, "wg-1-devs");
        assert_eq!(wg.my_agent_name, "alice");
    }

    #[test]
    fn discover_wg_peers_lists_ac_root_peers() {
        let temp = tempfile::TempDir::new().unwrap();
        let project = temp.path().join("proj-a");
        let wg_dir = project.join(".ac").join("wg-1-devs");
        let root = wg_dir.join("__agent_alice");
        let peer = wg_dir.join("__agent_bob");
        for dir in [&root, &peer] {
            std::fs::create_dir_all(dir).unwrap();
        }

        let wg = detect_wg_replica(root.to_str().unwrap())
            .unwrap()
            .expect("Project AC Root should be accepted");
        let peers = discover_wg_peers(wg);
        let names: Vec<&str> = peers.iter().map(|peer| peer.name.as_str()).collect();

        assert!(names.contains(&"proj-a:wg-1-devs/bob"));
    }

    #[test]
    fn discover_wg_peers_for_portable_verified_coordinator_includes_cross_wg_and_root() {
        let (_temp, wg1_tech_lead, _wg1_dev_rust) = make_portable_cross_wg_fixture();
        let wg = detect_wg_replica(wg1_tech_lead.to_str().unwrap())
            .unwrap()
            .expect("portable coordinator replica");

        let peers = discover_wg_peers(wg);
        let names: Vec<&str> = peers.iter().map(|peer| peer.name.as_str()).collect();

        assert!(names.contains(&"proj-a:wg-1-dev-team/dev-rust"));
        assert!(names.contains(&"proj-a:wg-2-dev-team/tech-lead"));
        assert!(names.contains(&crate::config::root_agent::ROOT_AGENT_SENDER));
        assert!(!names.contains(&"proj-a:wg-2-dev-team/dev-rust"));
    }

    #[test]
    fn discover_wg_peers_for_portable_non_coordinator_stays_wg_local_without_root() {
        let (_temp, _wg1_tech_lead, wg1_dev_rust) = make_portable_cross_wg_fixture();
        let wg = detect_wg_replica(wg1_dev_rust.to_str().unwrap())
            .unwrap()
            .expect("portable non-coordinator replica");

        let peers = discover_wg_peers(wg);
        let names: Vec<&str> = peers.iter().map(|peer| peer.name.as_str()).collect();

        assert!(names.contains(&"proj-a:wg-1-dev-team/tech-lead"));
        assert!(!names.contains(&"proj-a:wg-2-dev-team/tech-lead"));
        assert!(!names.contains(&"proj-a:wg-2-dev-team/dev-rust"));
        assert!(!names.contains(&crate::config::root_agent::ROOT_AGENT_SENDER));
    }

    #[test]
    fn discover_root_coordinator_peers_from_lists_identity_verified_coordinator_only() {
        let (_temp, paths) = make_root_discovery_fixture(false);

        let peers = discover_root_coordinator_peers_from_project_paths(&paths);
        let names: Vec<&str> = peers.iter().map(|p| p.name.as_str()).collect();

        assert_eq!(names, vec!["proj-a:wg-1-dev-team/tech-lead"]);
        assert!(peers[0].reachable);
        assert_eq!(peers[0].teams, vec!["dev-team"]);
    }

    #[test]
    fn discover_root_coordinator_peers_from_omits_spoofed_coordinator_dir_name() {
        let (_temp, paths) = make_root_discovery_fixture(true);

        let peers = discover_root_coordinator_peers_from_project_paths(&paths);

        assert!(peers.is_empty());
    }

    #[test]
    fn discover_root_coordinator_peers_from_omits_origin_coordinator() {
        let (_temp, paths) = make_root_discovery_fixture(false);

        let peers = discover_root_coordinator_peers_from_project_paths(&paths);

        assert!(!peers.iter().any(|p| p.name == "proj-a/tech-lead"));
        assert!(!peers
            .iter()
            .any(|p| p.name == "proj-a:wg-1-dev-team/dev-rust"));
    }

    #[test]
    fn build_root_agent_synthetic_peer_emits_peer_for_verified_coordinator() {
        let (_temp, paths) = make_root_discovery_fixture(false);
        let wg = WgReplicaInfo {
            my_agent_name: "tech-lead".to_string(),
            my_wg_name: "wg-1-dev-team".to_string(),
            my_wg_dir: PathBuf::from("ignored-for-this-test"),
            ac_root: PathBuf::from("ignored-for-this-test"),
            my_project: "proj-a".to_string(),
        };
        let peer = build_root_agent_synthetic_peer(&wg, &paths)
            .expect("verified coordinator should see synthetic root peer");
        assert_eq!(peer.name, crate::config::root_agent::ROOT_AGENT_SENDER);
        assert!(peer.reachable);
        assert_eq!(peer.session_status, "none");
        assert!(peer.teams.is_empty());
    }

    #[test]
    fn build_root_agent_synthetic_peer_skips_non_coordinator() {
        let (_temp, paths) = make_root_discovery_fixture(false);
        let wg = WgReplicaInfo {
            my_agent_name: "dev-rust".to_string(),
            my_wg_name: "wg-1-dev-team".to_string(),
            my_wg_dir: PathBuf::from("ignored-for-this-test"),
            ac_root: PathBuf::from("ignored-for-this-test"),
            my_project: "proj-a".to_string(),
        };
        assert!(build_root_agent_synthetic_peer(&wg, &paths).is_none());
    }

    #[test]
    fn build_root_agent_synthetic_peer_skips_spoofed_coordinator_identity() {
        let (_temp, paths) = make_root_discovery_fixture(true);
        let wg = WgReplicaInfo {
            my_agent_name: "tech-lead".to_string(),
            my_wg_name: "wg-1-dev-team".to_string(),
            my_wg_dir: PathBuf::from("ignored-for-this-test"),
            ac_root: PathBuf::from("ignored-for-this-test"),
            my_project: "proj-a".to_string(),
        };
        assert!(build_root_agent_synthetic_peer(&wg, &paths).is_none());
    }

    #[test]
    fn build_root_agent_synthetic_peer_repairs_stale_same_basename_identity() {
        let temp = tempfile::TempDir::new().unwrap();
        let project = temp.path().join("AgentsCommander_ac");
        let ac_root = project.join(".ac");
        let team_dir = ac_root.join("_team_dev-team");
        let matrix = ac_root.join("_agent_tech-lead");
        let wg_dir = ac_root.join("wg-2-dev-team");
        let replica = wg_dir.join("__agent_tech-lead");
        for dir in [&team_dir, &matrix, &replica] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(
            team_dir.join("config.json"),
            r#"{"agents":["../_agent_tech-lead"],"coordinator":"../_agent_tech-lead"}"#,
        )
        .unwrap();
        std::fs::write(
            replica.join("config.json"),
            r#"{"identity":"../../../../agentscommander-old/.ac/_agent_tech-lead"}"#,
        )
        .unwrap();

        let wg = WgReplicaInfo {
            my_agent_name: "tech-lead".to_string(),
            my_wg_name: "wg-2-dev-team".to_string(),
            my_wg_dir: wg_dir,
            ac_root,
            my_project: "AgentsCommander_ac".to_string(),
        };
        let paths = vec![temp.path().to_string_lossy().to_string()];

        let peer = build_root_agent_synthetic_peer(&wg, &paths)
            .expect("stale same-basename identity should repair before root verification");
        assert_eq!(peer.name, crate::config::root_agent::ROOT_AGENT_SENDER);

        let repaired: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(replica.join("config.json")).expect("read repaired config"),
        )
        .expect("parse repaired config");
        assert_eq!(repaired["identity"], "../../_agent_tech-lead");
    }

    #[test]
    fn build_root_agent_synthetic_peer_emits_peer_with_project_absent_from_settings() {
        // grinch §12.2 regression: a verified coordinator running in a WG
        // replica whose project is NOT in `settings.project_paths` must still
        // see the synthetic root peer via the §5.2 augmentation pattern (parity
        // with cli/send.rs::effective_project_paths).
        let (temp, _seeded_paths) = make_root_discovery_fixture(false);
        let project_dir = temp.path().join("proj-a");
        let ac_root = project_dir.join(".ac");
        let wg = WgReplicaInfo {
            my_agent_name: "tech-lead".to_string(),
            my_wg_name: "wg-1-dev-team".to_string(),
            my_wg_dir: ac_root.join("wg-1-dev-team"),
            ac_root: ac_root.clone(),
            my_project: "proj-a".to_string(),
        };

        // Sanity check: with EMPTY settings, the helper returns None — proves
        // the augmentation matters and we're not just trivially passing.
        let empty: Vec<String> = vec![];
        assert!(
            build_root_agent_synthetic_peer(&wg, &empty).is_none(),
            "without augmentation, helper must reject (otherwise this test is trivial)"
        );

        // Replicate the call-site augmentation: walk up `wg.ac_root.parent()`.
        let mut augmented = empty.clone();
        let project_str = wg
            .ac_root
            .parent()
            .and_then(|d| d.to_str())
            .expect("fixture project dir is utf-8")
            .to_string();
        augmented.push(project_str);

        let peer = build_root_agent_synthetic_peer(&wg, &augmented)
            .expect("with project-dir augmentation, helper must emit synthetic root peer");
        assert_eq!(peer.name, crate::config::root_agent::ROOT_AGENT_SENDER);
        assert!(peer.reachable);
    }

    // ── norm_path / canon_or_norm ────────────────────────────────────

    #[test]
    fn norm_path_lowercases_and_normalizes_slashes() {
        assert_eq!(norm_path(r"C:\Users\Foo\Bar"), "c:/users/foo/bar");
        assert_eq!(norm_path("C:/Users/Foo/Bar/"), "c:/users/foo/bar");
        assert_eq!(norm_path(r"\\?\C:\Users\Foo"), "c:/users/foo");
        assert_eq!(norm_path("c:/x"), "c:/x");
    }

    // ── compute_peer_status (non-WG, expected_name=None) ─────────────

    #[test]
    fn no_session_yields_none() {
        let idx: HashMap<String, Vec<CandidateSession>> = HashMap::new();
        let ps = compute_peer_status(r"C:\does\not\exist", None, &idx);
        assert!(!ps.working);
        assert_eq!(ps.session_status, "none");
        assert_eq!(ps.status_legacy, "unknown");
        assert!(ps.session_id.is_none());
        assert!(!ps.waiting_for_input);
        assert!(ps.exit_code.is_none());
    }

    #[test]
    fn running_session_is_working() {
        let mut idx = HashMap::new();
        idx.insert(
            "c:/x".to_string(),
            vec![cand("any", SessionStatus::Running, false)],
        );
        let ps = compute_peer_status("C:/X", None, &idx);
        assert!(ps.working);
        assert_eq!(ps.session_status, "running");
        assert_eq!(ps.status_legacy, "active");
        assert!(ps.session_id.is_some());
    }

    #[test]
    fn active_session_is_working() {
        let mut idx = HashMap::new();
        idx.insert(
            "c:/x".to_string(),
            vec![cand("any", SessionStatus::Active, false)],
        );
        let ps = compute_peer_status(r"C:\X", None, &idx);
        assert!(ps.working);
        assert_eq!(ps.session_status, "active");
        assert_eq!(ps.status_legacy, "active");
    }

    #[test]
    fn idle_session_is_not_working() {
        let mut idx = HashMap::new();
        idx.insert(
            "c:/x".to_string(),
            vec![cand("any", SessionStatus::Idle, false)],
        );
        let ps = compute_peer_status("C:/X", None, &idx);
        assert!(!ps.working);
        assert_eq!(ps.session_status, "idle");
        assert_eq!(ps.status_legacy, "unknown");
    }

    #[test]
    fn waiting_overrides_running() {
        let mut idx = HashMap::new();
        idx.insert(
            "c:/x".to_string(),
            vec![cand("any", SessionStatus::Running, true)],
        );
        let ps = compute_peer_status("C:/X", None, &idx);
        assert!(!ps.working);
        assert_eq!(ps.session_status, "waiting");
        assert!(ps.waiting_for_input);
    }

    #[test]
    fn exited_session_carries_exit_code() {
        let mut idx = HashMap::new();
        idx.insert(
            "c:/x".to_string(),
            vec![cand("any", SessionStatus::Exited(42), false)],
        );
        let ps = compute_peer_status("C:/X", None, &idx);
        assert_eq!(ps.session_status, "exited");
        assert_eq!(ps.exit_code, Some(42));
        assert!(!ps.working);
    }

    #[test]
    fn priority_picks_active_over_idle_at_same_cwd() {
        let mut idx = HashMap::new();
        idx.insert(
            "c:/x".to_string(),
            vec![
                cand("any", SessionStatus::Idle, false),
                cand("any", SessionStatus::Active, false),
            ],
        );
        let ps = compute_peer_status("C:/X", None, &idx);
        assert_eq!(ps.session_status, "active");
        assert!(ps.working);
    }

    #[test]
    fn extended_length_prefix_normalizes() {
        let mut idx = HashMap::new();
        idx.insert(
            "c:/x".to_string(),
            vec![cand("any", SessionStatus::Running, false)],
        );
        let ps = compute_peer_status(r"\\?\C:\X", None, &idx);
        assert_eq!(ps.session_status, "running");
    }

    // ── compute_peer_status with WG name filter (expected_name=Some) ─

    #[test]
    fn wg_name_filter_matches_only_named_session() {
        let mut idx = HashMap::new();
        idx.insert(
            "c:/x".to_string(),
            vec![
                cand("wg-20/dev", SessionStatus::Active, false),
                cand("[temp]-foo", SessionStatus::Active, false),
                cand("other-name", SessionStatus::Running, true),
            ],
        );
        let ps = compute_peer_status("C:/X", Some("wg-20/dev"), &idx);
        assert_eq!(ps.session_status, "active");
        assert!(ps.working);
        assert!(!ps.waiting_for_input);
    }

    #[test]
    fn wg_name_filter_returns_none_when_no_name_match() {
        let mut idx = HashMap::new();
        idx.insert(
            "c:/x".to_string(),
            vec![cand("other-name", SessionStatus::Active, false)],
        );
        let ps = compute_peer_status("C:/X", Some("wg-20/dev"), &idx);
        assert_eq!(ps.session_status, "none");
        assert!(!ps.working);
    }

    // ── build_session_index_from filter tests ────────────────────────

    #[test]
    fn build_index_skips_temp_sessions() {
        let rows = vec![ps_row(
            "[temp]-dispatch",
            r"C:\X",
            Some(SessionStatus::Active),
            true,
        )];
        let idx = build_session_index_from(&rows);
        assert!(idx.is_empty(), "temp-prefixed sessions must be skipped");
    }

    #[test]
    fn build_index_skips_rows_without_id() {
        let rows = vec![ps_row(
            "wg-20/dev",
            r"C:\X",
            Some(SessionStatus::Active),
            false,
        )];
        let idx = build_session_index_from(&rows);
        assert!(idx.is_empty(), "rows without id must be skipped");
    }

    #[test]
    fn build_index_skips_rows_without_status() {
        let rows = vec![ps_row("wg-20/dev", r"C:\X", None, true)];
        let idx = build_session_index_from(&rows);
        assert!(idx.is_empty(), "rows without status must be skipped");
    }

    #[test]
    fn build_index_normalizes_cwd_with_extended_prefix() {
        let rows = vec![ps_row(
            "wg-20/dev",
            r"\\?\C:\X",
            Some(SessionStatus::Active),
            true,
        )];
        let idx = build_session_index_from(&rows);
        assert!(idx.contains_key("c:/x"));
    }

    #[test]
    fn build_index_groups_multiple_rows_at_same_cwd() {
        let rows = vec![
            ps_row("wg-20/dev", r"C:\X", Some(SessionStatus::Active), true),
            ps_row("other", r"C:/X", Some(SessionStatus::Idle), true),
        ];
        let idx = build_session_index_from(&rows);
        let bucket = idx.get("c:/x").expect("entries grouped under c:/x");
        assert_eq!(bucket.len(), 2);
    }

    // ── §6 / §12 list-peers-lean (issue #252) ──────────────────────────────

    fn sample_peer_info(name: &str) -> PeerInfo {
        PeerInfo {
            name: name.to_string(),
            path: r"C:\some\path".to_string(),
            status: "active".to_string(),
            role: "Senior architect with deep Rust expertise. Owns the PTY layer.".to_string(),
            teams: vec!["ac-devs".to_string()],
            reachable: true,
            last_coding_agent: Some("claude".to_string()),
            working: true,
            session_status: "running".to_string(),
            session_id: Some("11111111-1111-1111-1111-111111111111".to_string()),
            waiting_for_input: false,
            exit_code: None,
            context_percent: None,
            agent: None,
            profile: None,
            coding_agents: HashMap::new(),
        }
    }

    // §6.1 — command exists in clap

    #[test]
    fn list_peers_lean_subcommand_is_registered() {
        use clap::CommandFactory;
        let cmd = crate::cli::Cli::command();
        let names: Vec<&str> = cmd.get_subcommands().map(|c| c.get_name()).collect();
        assert!(
            names.contains(&"list-peers-lean"),
            "list-peers-lean should be a registered subcommand; got: {:?}",
            names
        );
    }

    #[test]
    fn list_peers_lean_help_documents_excluded_fields() {
        use clap::CommandFactory;
        let cmd = crate::cli::Cli::command();
        let lean = cmd
            .get_subcommands()
            .find(|c| c.get_name() == "list-peers-lean")
            .expect("list-peers-lean subcommand");
        let after = lean
            .get_after_help()
            .expect("after_help present")
            .to_string();
        assert!(after.contains("EXCLUDED VS list-peers"));
        assert!(after.contains("path"));
        assert!(after.contains("codingAgents"));
    }

    // §6.2 — JSON shape: kept and omitted fields

    #[test]
    fn lean_json_keeps_essential_fields() {
        let peer = sample_peer_info("project:wg-1-team/dev");
        let lean = LeanPeerInfo::from(&peer);
        let json = serde_json::to_string(&lean).unwrap();
        for kept in [
            "\"name\":",
            "\"working\":",
            "\"sessionStatus\":",
            "\"waitingForInput\":",
            "\"reachable\":",
            "\"teams\":",
            "\"roleSummary\":",
        ] {
            assert!(
                json.contains(kept),
                "lean JSON missing {} — got {}",
                kept,
                json
            );
        }
    }

    #[test]
    fn lean_json_omits_verbose_fields() {
        let peer = sample_peer_info("project:wg-1-team/dev");
        let lean = LeanPeerInfo::from(&peer);
        let json = serde_json::to_string(&lean).unwrap();
        for forbidden in [
            "\"path\":",
            "\"role\":",
            "\"codingAgents\":",
            "\"lastCodingAgent\":",
            "\"sessionId\":",
            "\"exitCode\":",
            "\"status\":",
        ] {
            assert!(
                !json.contains(forbidden),
                "lean JSON must not contain {} — got {}",
                forbidden,
                json
            );
        }
    }

    #[test]
    fn lean_output_is_valid_json_array() {
        let peers: Vec<PeerInfo> = vec![sample_peer_info("a"), sample_peer_info("b")];
        let lean: Vec<LeanPeerInfo> = peers.iter().map(LeanPeerInfo::from).collect();
        let json = serde_json::to_string_pretty(&lean).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert!(parsed.is_array(), "lean output must be a JSON array");
        assert_eq!(parsed.as_array().unwrap().len(), 2);
    }

    // ── #1088: contextPercent projection through the peer→session join ──

    #[test]
    fn build_session_index_copies_context_percent_from_persisted() {
        let mut row = ps_row("Session 1", r"C:\work", Some(SessionStatus::Running), true);
        row.context_percent = Some(42);
        let index = build_session_index_from(&[row]);
        let candidates = index
            .get(&canon_or_norm(r"C:\work"))
            .expect("row indexed by its cwd");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].context_percent, Some(42));
    }

    #[test]
    fn build_session_index_context_percent_absent_is_none() {
        let row = ps_row("Session 1", r"C:\work", Some(SessionStatus::Running), true);
        let index = build_session_index_from(&[row]);
        let candidates = index.get(&canon_or_norm(r"C:\work")).unwrap();
        assert_eq!(candidates[0].context_percent, None);
    }

    #[test]
    fn compute_peer_status_returns_chosen_context_percent() {
        let mut row = ps_row("Session 1", r"C:\work", Some(SessionStatus::Running), true);
        row.context_percent = Some(7);
        let index = build_session_index_from(&[row]);
        let status = compute_peer_status(r"C:\work", None, &index);
        assert_eq!(status.context_percent, Some(7));
    }

    #[test]
    fn compute_peer_status_context_percent_zero_is_preserved() {
        // `0` is a valid reading, never coerced to None (mod.rs:96-102 semantic).
        let mut row = ps_row("Session 1", r"C:\work", Some(SessionStatus::Running), true);
        row.context_percent = Some(0);
        let index = build_session_index_from(&[row]);
        let status = compute_peer_status(r"C:\work", None, &index);
        assert_eq!(status.context_percent, Some(0));
    }

    #[test]
    fn compute_peer_status_no_match_has_none_context_percent() {
        let index = build_session_index_from(&[]);
        let status = compute_peer_status(r"C:\nowhere", None, &index);
        assert_eq!(status.session_status, "none");
        assert_eq!(status.context_percent, None);
    }

    #[test]
    fn full_peer_json_context_percent_present_zero_and_omitted() {
        let mut peer = sample_peer_info("p");
        peer.context_percent = Some(42);
        let json = serde_json::to_string(&peer).unwrap();
        assert!(json.contains("\"contextPercent\":42"), "got {json}");

        peer.context_percent = Some(0);
        let json = serde_json::to_string(&peer).unwrap();
        assert!(json.contains("\"contextPercent\":0"), "got {json}");

        peer.context_percent = None;
        let json = serde_json::to_string(&peer).unwrap();
        assert!(
            !json.contains("contextPercent"),
            "None must omit the key: {json}"
        );
    }

    #[test]
    fn lean_peer_json_carries_context_percent_and_omits_when_none() {
        let mut peer = sample_peer_info("p");
        peer.context_percent = Some(88);
        let lean = LeanPeerInfo::from(&peer);
        assert_eq!(lean.context_percent, Some(88));
        let json = serde_json::to_string(&lean).unwrap();
        assert!(json.contains("\"contextPercent\":88"), "got {json}");

        peer.context_percent = None;
        let lean = LeanPeerInfo::from(&peer);
        assert_eq!(lean.context_percent, None);
        let json = serde_json::to_string(&lean).unwrap();
        assert!(
            !json.contains("contextPercent"),
            "None must omit the key: {json}"
        );
    }

    // ── #1641: agent/profile projection through the peer→session join ──

    /// Project a computed peer status onto the sample fixture so lean-JSON
    /// assertions run over the full projection path (CandidateSession →
    /// PeerStatus → PeerInfo → LeanPeerInfo).
    fn peer_with_status(name: &str, status: &PeerStatus) -> PeerInfo {
        let mut peer = sample_peer_info(name);
        peer.working = status.working;
        peer.session_status = status.session_status.to_string();
        peer.waiting_for_input = status.waiting_for_input;
        peer.agent = status.agent_id.clone();
        peer.profile = status.effective_profile.clone();
        peer
    }

    #[test]
    fn build_session_index_from_copies_agent_and_effective_profile() {
        let mut row = ps_row("Session 1", r"C:\work", Some(SessionStatus::Running), true);
        row.agent_id = Some("codex".into());
        row.effective_profile = Some("B".into());
        let index = build_session_index_from(&[row]);
        let candidates = index
            .get(&canon_or_norm(r"C:\work"))
            .expect("row indexed by its cwd");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].agent_id.as_deref(), Some("codex"));
        assert_eq!(candidates[0].effective_profile.as_deref(), Some("B"));
    }

    #[test]
    fn lean_peer_reports_agent_and_profile_for_live_agent_session() {
        let mut row = ps_row("Session 1", r"C:\work", Some(SessionStatus::Running), true);
        row.agent_id = Some("codex".into());
        row.effective_profile = Some("B".into());
        let index = build_session_index_from(&[row]);
        let status = compute_peer_status(r"C:\work", None, &index);
        let peer = peer_with_status("project:wg-1-team/dev", &status);
        let json = serde_json::to_string(&LeanPeerInfo::from(&peer)).unwrap();
        assert!(json.contains("\"agent\":\"codex\""), "got {json}");
        assert!(json.contains("\"profile\":\"B\""), "got {json}");
    }

    #[test]
    fn lean_peer_omits_agent_and_profile_for_cold_peer() {
        let index = build_session_index_from(&[]);
        let status = compute_peer_status(r"C:\nowhere", None, &index);
        assert_eq!(status.session_status, "none");
        let peer = peer_with_status("project:wg-1-team/dev", &status);
        let json = serde_json::to_string(&LeanPeerInfo::from(&peer)).unwrap();
        assert!(!json.contains("\"agent\""), "got {json}");
        assert!(!json.contains("\"profile\""), "got {json}");
    }

    #[test]
    fn lean_peer_omits_profile_for_plain_shell_session() {
        // Live session with an agent id but no effective profile (plain-shell
        // or pre-#1641 row): agent is reported, profile is omitted.
        let mut row = ps_row("Session 1", r"C:\work", Some(SessionStatus::Running), true);
        row.agent_id = Some("codex".into());
        let index = build_session_index_from(&[row]);
        let status = compute_peer_status(r"C:\work", None, &index);
        let peer = peer_with_status("project:wg-1-team/dev", &status);
        let json = serde_json::to_string(&LeanPeerInfo::from(&peer)).unwrap();
        assert!(json.contains("\"agent\":\"codex\""), "got {json}");
        assert!(!json.contains("\"profile\""), "got {json}");
    }

    #[test]
    fn lean_peer_reports_effective_profile_after_fallback() {
        // Requested "C" but the cell fell back to "B": the lean output must
        // surface what actually spawned, not what was requested.
        let mut row = ps_row("Session 1", r"C:\work", Some(SessionStatus::Running), true);
        row.agent_id = Some("codex".into());
        row.requested_profile = Some("C".into());
        row.effective_profile = Some("B".into());
        let index = build_session_index_from(&[row]);
        let status = compute_peer_status(r"C:\work", None, &index);
        let peer = peer_with_status("project:wg-1-team/dev", &status);
        let json = serde_json::to_string(&LeanPeerInfo::from(&peer)).unwrap();
        assert!(json.contains("\"profile\":\"B\""), "got {json}");
        assert!(
            !json.contains("\"C\""),
            "requested letter must not leak: {json}"
        );
    }

    #[test]
    fn old_sessions_json_without_effective_profile_loads_and_omits() {
        // Pre-#1641 row: no effectiveProfile key (and no agentId) — must
        // deserialize and project with both fields omitted; no migration.
        let json = r#"{
            "name": "legacy",
            "shell": "claude",
            "shellArgs": [],
            "workingDirectory": "C:\\legacy",
            "id": "11111111-1111-1111-1111-111111111111",
            "status": "running",
            "waitingForInput": false
        }"#;
        let row: PersistedSession = serde_json::from_str(json).expect("deserialize");
        assert_eq!(row.effective_profile, None);
        assert_eq!(row.agent_id, None);
        let index = build_session_index_from(&[row]);
        let status = compute_peer_status(r"C:\legacy", None, &index);
        let peer = peer_with_status("project:wg-1-team/dev", &status);
        let lean = LeanPeerInfo::from(&peer);
        assert_eq!(lean.agent, None);
        assert_eq!(lean.profile, None);
        let out = serde_json::to_string(&lean).unwrap();
        assert!(!out.contains("\"agent\""), "got {out}");
        assert!(!out.contains("\"profile\""), "got {out}");
    }

    #[test]
    fn peer_info_carries_agent_and_profile_into_lean_projection() {
        let mut peer = sample_peer_info("project:wg-1-team/dev");
        peer.agent = Some("codex".into());
        peer.profile = Some("B".into());
        let lean = LeanPeerInfo::from(&peer);
        assert_eq!(lean.agent.as_deref(), Some("codex"));
        assert_eq!(lean.profile.as_deref(), Some("B"));
    }

    // §6.3 — field preservation parity

    #[test]
    fn lean_preserves_name_working_session_status_reachable() {
        let peer = sample_peer_info("project:wg-1-team/dev");
        let lean = LeanPeerInfo::from(&peer);
        assert_eq!(lean.name, peer.name);
        assert_eq!(lean.working, peer.working);
        assert_eq!(lean.session_status, peer.session_status);
        assert_eq!(lean.reachable, peer.reachable);
        assert_eq!(lean.waiting_for_input, peer.waiting_for_input);
        assert_eq!(lean.teams, peer.teams);
    }

    #[test]
    fn lean_waiting_for_input_false_when_no_session() {
        let mut peer = sample_peer_info("p");
        peer.session_status = "none".to_string();
        peer.waiting_for_input = false;
        let lean = LeanPeerInfo::from(&peer);
        assert!(!lean.waiting_for_input);
        assert_eq!(lean.session_status, "none");
    }

    // §12.2 — From<&PeerInfo> binds to lean_role_summary helper

    #[test]
    fn lean_role_summary_is_produced_by_lean_role_summary_helper() {
        let mut peer = sample_peer_info("p");
        peer.role = "x".repeat(200); // long enough to force truncation
        let lean = LeanPeerInfo::from(&peer);
        assert_eq!(lean.role_summary, lean_role_summary(&peer.role));
        // Per §12.1 (Option A): ≤ MAX after truncation.
        assert!(lean.role_summary.chars().count() <= ROLE_SUMMARY_MAX);
        assert!(lean.role_summary.ends_with('…'));
    }

    // §6.4 — peer-set parity via projection

    #[test]
    fn lean_projection_preserves_peer_set_order_and_names() {
        let peers: Vec<PeerInfo> = vec![
            sample_peer_info("project/origin-agent"),
            sample_peer_info("project:wg-1-team/dev"),
            sample_peer_info("project:wg-1-team/architect"),
        ];

        let lean_names: Vec<String> = peers
            .iter()
            .map(LeanPeerInfo::from)
            .map(|l| l.name)
            .collect();
        let full_names: Vec<String> = peers.iter().map(|p| p.name.clone()).collect();

        assert_eq!(lean_names, full_names);
    }

    // §6.5 — roleSummary derivation (§12.1 contract: ≤ MAX total chars)

    #[test]
    fn role_summary_takes_first_non_empty_line() {
        assert_eq!(
            lean_role_summary("First line.\nSecond line."),
            "First line."
        );
        assert_eq!(
            lean_role_summary("\n\nFirst non-empty.\nSecond."),
            "First non-empty."
        );
    }

    #[test]
    fn role_summary_truncates_to_at_most_max_chars_with_ellipsis() {
        let long = "x".repeat(200);
        let out = lean_role_summary(&long);
        // ≤ MAX chars total, with `…` as the final char.
        assert_eq!(out.chars().count(), ROLE_SUMMARY_MAX);
        assert!(out.ends_with('…'));
        let prefix: String = out.chars().take(ROLE_SUMMARY_MAX - 1).collect();
        assert_eq!(prefix, "x".repeat(ROLE_SUMMARY_MAX - 1));
    }

    #[test]
    fn role_summary_returns_empty_for_no_role_sentinels() {
        assert_eq!(lean_role_summary(""), "");
        assert_eq!(lean_role_summary("   \n  \n"), "");
        assert_eq!(lean_role_summary("No role description available."), "");
        assert_eq!(lean_role_summary("Room replica agent."), "");
    }

    #[test]
    fn role_summary_is_omitted_from_json_when_empty() {
        let mut peer = sample_peer_info("p");
        peer.role = "No role description available.".to_string();
        let lean = LeanPeerInfo::from(&peer);
        let json = serde_json::to_string(&lean).unwrap();
        assert!(
            !json.contains("\"roleSummary\":"),
            "roleSummary must be omitted when empty — got {}",
            json
        );
    }

    // §12.7 — preamble sentinel filtering

    #[test]
    fn role_summary_filters_agentscommander_preamble() {
        let preamble = "You are running inside an AgentsCommander session — \
                        a terminal session manager that coordinates multiple AI agents.";
        assert_eq!(lean_role_summary(preamble), "");

        let context_header = "# AgentsCommander Context\n\nYou are running inside…";
        assert_eq!(lean_role_summary(context_header), "");
    }

    // §12.8 — fence-post truncation tests

    #[test]
    fn role_summary_no_ellipsis_at_exact_limit() {
        let exactly = "x".repeat(ROLE_SUMMARY_MAX);
        let out = lean_role_summary(&exactly);
        assert_eq!(out.chars().count(), ROLE_SUMMARY_MAX);
        assert!(!out.ends_with('…'));
    }

    #[test]
    fn role_summary_ellipsis_at_one_over_limit() {
        let one_over = "x".repeat(ROLE_SUMMARY_MAX + 1);
        let out = lean_role_summary(&one_over);
        // Per §12.1 (Option A): always ≤ MAX total, with ellipsis if truncated.
        assert_eq!(out.chars().count(), ROLE_SUMMARY_MAX);
        assert!(out.ends_with('…'));
        let prefix: String = out.chars().take(ROLE_SUMMARY_MAX - 1).collect();
        assert_eq!(prefix, "x".repeat(ROLE_SUMMARY_MAX - 1));
    }

    // §6.6 — validation guards (token/root) for both execute and execute_lean

    #[test]
    fn execute_returns_1_when_token_missing() {
        let args = ListPeersArgs {
            token: None,
            root: Some("anything".into()),
            peer: Vec::new(),
        };
        assert_eq!(execute(args), 1);
    }

    #[test]
    fn execute_returns_1_when_root_missing() {
        let args = ListPeersArgs {
            token: Some("11111111-1111-1111-1111-111111111111".into()),
            root: None,
            peer: Vec::new(),
        };
        assert_eq!(execute(args), 1);
    }

    #[test]
    fn execute_lean_returns_1_when_token_missing() {
        let args = ListPeersLeanArgs {
            token: None,
            root: Some("anything".into()),
            peer: Vec::new(),
            snapshot_targets: false,
        };
        assert_eq!(execute_lean(args), 1);
    }

    #[test]
    fn execute_lean_returns_1_when_root_missing() {
        let args = ListPeersLeanArgs {
            token: Some("11111111-1111-1111-1111-111111111111".into()),
            root: None,
            peer: Vec::new(),
            snapshot_targets: false,
        };
        assert_eq!(execute_lean(args), 1);
    }

    // ── §259 --peer filter ────────────────────────────────────────────

    #[test]
    fn peer_filter_empty_request_returns_input_unchanged() {
        // No --peer supplied → list-peers must behave byte-for-byte as
        // before (no filter, no reorder, no allocation surprises).
        let peers = vec![
            sample_peer_info("project/origin-a"),
            sample_peer_info("project:wg-1-team/dev"),
            sample_peer_info("project:wg-1-team/arch"),
        ];
        let result = apply_peer_filter(peers, &[]).expect("empty filter is Ok");
        let names: Vec<&str> = result.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "project/origin-a",
                "project:wg-1-team/dev",
                "project:wg-1-team/arch",
            ],
            "empty --peer must preserve discovery order verbatim"
        );
    }

    #[test]
    fn peer_filter_single_peer_returns_only_that_peer() {
        let peers = vec![
            sample_peer_info("project:wg-1-team/dev"),
            sample_peer_info("project:wg-1-team/arch"),
            sample_peer_info("project/origin-a"),
        ];
        let result = apply_peer_filter(peers, &["project:wg-1-team/arch".to_string()]).expect("Ok");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "project:wg-1-team/arch");
    }

    #[test]
    fn peer_filter_multiple_peers_preserves_user_order() {
        // Discovery order is [dev, arch, origin]; user asks for
        // [arch, origin, dev] → emit in that order, not discovery order.
        let peers = vec![
            sample_peer_info("project:wg-1-team/dev"),
            sample_peer_info("project:wg-1-team/arch"),
            sample_peer_info("project/origin-a"),
        ];
        let requested = vec![
            "project:wg-1-team/arch".to_string(),
            "project/origin-a".to_string(),
            "project:wg-1-team/dev".to_string(),
        ];
        let result = apply_peer_filter(peers, &requested).expect("Ok");
        let names: Vec<&str> = result.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "project:wg-1-team/arch",
                "project/origin-a",
                "project:wg-1-team/dev",
            ]
        );
    }

    #[test]
    fn peer_filter_duplicates_are_silently_deduplicated() {
        let peers = vec![
            sample_peer_info("project:wg-1-team/dev"),
            sample_peer_info("project:wg-1-team/arch"),
        ];
        let requested = vec![
            "project:wg-1-team/dev".to_string(),
            "project:wg-1-team/arch".to_string(),
            "project:wg-1-team/dev".to_string(),  // duplicate
            "project:wg-1-team/arch".to_string(), // duplicate
        ];
        let result = apply_peer_filter(peers, &requested).expect("Ok");
        let names: Vec<&str> = result.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["project:wg-1-team/dev", "project:wg-1-team/arch"],
            "duplicates must collapse to first-occurrence order with no\
             repeats and no error"
        );
    }

    #[test]
    fn peer_filter_unknown_peer_returns_err_with_unknown_names() {
        let peers = vec![
            sample_peer_info("project:wg-1-team/dev"),
            sample_peer_info("project:wg-1-team/arch"),
        ];
        let requested = vec![
            "project:wg-1-team/dev".to_string(),
            "project:wg-1-team/ghost".to_string(),
            "project:wg-1-team/missing".to_string(),
        ];
        let err = apply_peer_filter(peers, &requested).expect_err("must Err");
        assert_eq!(
            err,
            vec![
                "project:wg-1-team/ghost".to_string(),
                "project:wg-1-team/missing".to_string()
            ],
            "Err must list every unknown name in first-occurrence order"
        );
    }

    #[test]
    fn peer_filter_unknown_dedupes_before_reporting() {
        // Repeated unknown should appear once in the error list.
        let peers = vec![sample_peer_info("project:wg-1-team/dev")];
        let requested = vec![
            "project:wg-1-team/ghost".to_string(),
            "project:wg-1-team/ghost".to_string(),
        ];
        let err = apply_peer_filter(peers, &requested).expect_err("must Err");
        assert_eq!(err, vec!["project:wg-1-team/ghost".to_string()]);
    }

    #[test]
    fn peer_filter_match_is_exact_no_substring() {
        // "dev" must not match "project:wg-1-team/dev"; FQNs are matched
        // by exact equality only.
        let peers = vec![sample_peer_info("project:wg-1-team/dev")];
        let err = apply_peer_filter(peers, &["dev".to_string()]).expect_err("must Err");
        assert_eq!(err, vec!["dev".to_string()]);
    }

    #[test]
    fn peer_filter_match_is_case_sensitive() {
        let peers = vec![sample_peer_info("project:wg-1-team/Dev")];
        let err = apply_peer_filter(peers, &["project:wg-1-team/dev".to_string()])
            .expect_err("must Err — names differ in case");
        assert_eq!(err, vec!["project:wg-1-team/dev".to_string()]);
    }

    #[test]
    fn peer_filter_keeps_unreachable_peers_when_name_matches() {
        // Filtering is by name only; reachable=false must NOT cause the
        // peer to be excluded when its FQN is requested.
        let mut unreachable = sample_peer_info("project:wg-1-team/unreachable");
        unreachable.reachable = false;
        let peers = vec![unreachable];
        let result =
            apply_peer_filter(peers, &["project:wg-1-team/unreachable".to_string()]).expect("Ok");
        assert_eq!(result.len(), 1);
        assert!(
            !result[0].reachable,
            "unreachable peer must survive the filter"
        );
    }

    #[test]
    fn peer_filter_against_empty_discovery_errors_with_all_unknowns() {
        // No peers discovered, any --peer is unknown → fail fast.
        let err =
            apply_peer_filter(Vec::new(), &["project/anything".to_string()]).expect_err("must Err");
        assert_eq!(err, vec!["project/anything".to_string()]);
    }

    #[test]
    fn peer_filter_against_empty_discovery_and_empty_request_is_ok() {
        // No discovery + no filter → no error, empty result. The no-filter
        // fast path in execute would short-circuit before reaching the
        // helper; this test just nails down helper behavior.
        let result = apply_peer_filter(Vec::new(), &[]).expect("Ok");
        assert!(result.is_empty());
    }

    #[test]
    fn peer_filter_followed_by_lean_projection_preserves_filter_and_schema() {
        // The lean schema is a projection over PeerInfo (LeanPeerInfo::from).
        // Ensure that filter → lean projection composes: filtered names
        // appear in the lean output in user-requested order.
        let peers = vec![
            sample_peer_info("project:wg-1-team/dev"),
            sample_peer_info("project:wg-1-team/arch"),
            sample_peer_info("project/origin-a"),
        ];
        let requested = vec![
            "project/origin-a".to_string(),
            "project:wg-1-team/dev".to_string(),
        ];
        let filtered = apply_peer_filter(peers, &requested).expect("Ok");
        let lean: Vec<LeanPeerInfo> = filtered.iter().map(LeanPeerInfo::from).collect();
        let lean_names: Vec<&str> = lean.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            lean_names,
            vec!["project/origin-a", "project:wg-1-team/dev"]
        );

        // Verify the lean schema is still applied to filtered entries
        // (verbose fields absent, essential fields present).
        let json = serde_json::to_string(&lean).unwrap();
        assert!(json.contains("\"name\":"));
        assert!(json.contains("\"working\":"));
        assert!(!json.contains("\"path\":"));
        assert!(!json.contains("\"codingAgents\":"));
    }

    // ── §259 clap wiring: --peer is registered, repeatable, optional ──

    #[test]
    fn peer_flag_is_registered_on_list_peers() {
        use clap::CommandFactory;
        let cmd = crate::cli::Cli::command();
        let lp = cmd
            .get_subcommands()
            .find(|c| c.get_name() == "list-peers")
            .expect("list-peers subcommand");
        let has_peer = lp.get_arguments().any(|a| a.get_id() == "peer");
        assert!(has_peer, "list-peers must register a --peer flag");
    }

    #[test]
    fn peer_flag_is_registered_on_list_peers_lean() {
        use clap::CommandFactory;
        let cmd = crate::cli::Cli::command();
        let lp = cmd
            .get_subcommands()
            .find(|c| c.get_name() == "list-peers-lean")
            .expect("list-peers-lean subcommand");
        let has_peer = lp.get_arguments().any(|a| a.get_id() == "peer");
        assert!(has_peer, "list-peers-lean must register a --peer flag");
    }

    #[test]
    fn peer_flag_parses_repeatedly_for_list_peers() {
        use clap::Parser;
        let parsed = crate::cli::Cli::try_parse_from([
            "agentscommander",
            "list-peers",
            "--token",
            "11111111-1111-1111-1111-111111111111",
            "--root",
            "anything",
            "--peer",
            "project/a",
            "--peer",
            "project:wg-1-team/b",
        ])
        .expect("clap should accept repeated --peer");
        let cmd = parsed.command.expect("subcommand present");
        match cmd {
            crate::cli::Commands::ListPeers(args) => {
                assert_eq!(
                    args.peer,
                    vec!["project/a".to_string(), "project:wg-1-team/b".to_string()],
                    "--peer must accumulate across occurrences"
                );
            }
            _ => panic!("expected ListPeers subcommand"),
        }
    }

    #[test]
    fn peer_flag_parses_repeatedly_for_list_peers_lean() {
        use clap::Parser;
        let parsed = crate::cli::Cli::try_parse_from([
            "agentscommander",
            "list-peers-lean",
            "--token",
            "11111111-1111-1111-1111-111111111111",
            "--root",
            "anything",
            "--peer",
            "project/a",
            "--peer",
            "project:wg-1-team/b",
        ])
        .expect("clap should accept repeated --peer");
        let cmd = parsed.command.expect("subcommand present");
        match cmd {
            crate::cli::Commands::ListPeersLean(args) => {
                assert_eq!(
                    args.peer,
                    vec!["project/a".to_string(), "project:wg-1-team/b".to_string()]
                );
            }
            _ => panic!("expected ListPeersLean subcommand"),
        }
    }

    #[test]
    fn list_peers_help_documents_peer_filter() {
        use clap::CommandFactory;
        let cmd = crate::cli::Cli::command();
        let lp = cmd
            .get_subcommands()
            .find(|c| c.get_name() == "list-peers")
            .expect("list-peers subcommand");
        let after = lp.get_after_help().expect("after_help present").to_string();
        assert!(
            after.contains("PEER FILTER"),
            "list-peers after_help must document --peer"
        );
        assert!(after.contains("--peer"));
        assert!(after.contains("exact"));
    }

    #[test]
    fn list_peers_lean_help_documents_peer_filter() {
        use clap::CommandFactory;
        let cmd = crate::cli::Cli::command();
        let lp = cmd
            .get_subcommands()
            .find(|c| c.get_name() == "list-peers-lean")
            .expect("list-peers-lean subcommand");
        let after = lp.get_after_help().expect("after_help present").to_string();
        assert!(after.contains("PEER FILTER"));
        assert!(after.contains("--peer"));
    }
}
