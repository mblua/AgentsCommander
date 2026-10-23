use clap::Args;
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;

use crate::config::agent_config::CodingAgentEntry;
#[cfg(test)]
use crate::session::session::SessionCommunicationKind;

mod discovery;
mod snapshot;
mod status;

use discovery::{apply_peer_filter, discover_peers, report_unknown_peers};
#[cfg(test)]
use discovery::{
    build_root_agent_synthetic_peer, detect_wg_replica,
    discover_root_coordinator_peers_from_project_paths, discover_wg_peers, WgReplicaInfo,
};
use snapshot::{
    apply_snapshot_target_filter, discover_snapshot_targets, serialize_snapshot_targets,
    snapshot_targets_counts_note,
};
#[cfg(test)]
use snapshot::{snapshot_targets_reason_note, SnapshotTargetsSilentReason};
#[cfg(test)]
use status::{
    build_session_index_from, canon_or_norm, compute_peer_status, norm_path, CandidateSession,
    PeerStatus,
};

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
  blockedMenu       true when the matched session is parked on a blocking\n                    \
                  terminal menu (a folder-trust prompt). OMITTED entirely\n                    \
                  when it is not blocked. working, sessionStatus and\n                    \
                  waitingForInput keep their normal values in this state.\n  \
  blockedMenuMessage\n                    \
                  The agent-specific notice for that menu, e.g. \"codex is\n                    \
                  waiting for you to answer the folder-trust menu in this\n                    \
                  terminal\". Omitted when not blocked or absent.\n  \
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
    Origin orchestrators and non-orchestrator Room replicas are omitted for \
    Root Agent discovery in #277.\n  \
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

    /// Agent root directory (required). Your working directory — used to identify you
    /// and your teams
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
  blockedMenu       true when the matched session is parked on a blocking\n                    \
                  terminal menu (a folder-trust prompt). OMITTED entirely\n                    \
                  when it is not blocked. working, sessionStatus and\n                    \
                  waitingForInput keep their normal values in this state.\n  \
  blockedMenuMessage\n                    \
                  The agent-specific notice for that menu, e.g. \"codex is\n                    \
                  waiting for you to answer the folder-trust menu in this\n                    \
                  terminal\". Omitted when not blocked or absent.\n                    \
                  Same semantics as `list-peers`.\n  \
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
`terminal-snapshot --to`. This view never emits `blockedMenu` or\n\
`blockedMenuMessage`, because it reads no session index. Candidates that fail\n\
identity verification are skipped rather than aborting the command, and are\n\
reported as counts on stderr.\n\n\
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

    /// Agent root directory (required). Your working directory — used to identify you
    /// and your teams
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

/// serde predicate: omit a `false` flag from the JSON entirely (#1669).
fn is_false(value: &bool) -> bool {
    !*value
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
    /// #1669 - same value as `PeerInfo.blocked_menu`. Omitted entirely when
    /// the matched session is not parked on a blocking terminal menu.
    #[serde(skip_serializing_if = "is_false")]
    pub blocked_menu: bool,
    /// #1669 - same value as `PeerInfo.blocked_menu_message`. Omitted when the
    /// session is not blocked or the row carries no message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_menu_message: Option<String>,
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
            blocked_menu: p.blocked_menu,
            blocked_menu_message: p.blocked_menu_message.clone(),
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
    /// #1669 - true when the matched session is parked on a blocking terminal
    /// menu (a folder-trust prompt detected by the #1646/#1647 menu guard).
    /// OMITTED entirely when the session is not blocked, so existing consumers
    /// are unaffected. `working`, `sessionStatus` and `waitingForInput` keep
    /// their normal values in this state: the blocked condition is communicated
    /// ONLY through this field and `blockedMenuMessage`.
    #[serde(skip_serializing_if = "is_false")]
    blocked_menu: bool,
    /// #1669 - the agent-specific notice for that menu, e.g. "codex is waiting
    /// for you to answer the folder-trust menu in this terminal". Omitted when
    /// the session is not blocked or the row carries no message.
    #[serde(skip_serializing_if = "Option::is_none")]
    blocked_menu_message: Option<String>,
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
        let (peers, skips) = match discover_snapshot_targets(&root) {
            Ok(discovered) => discovered,
            Err(error) => {
                eprintln!("Error: {}", error);
                return 1;
            }
        };
        if skips.any() {
            eprintln!("{}", snapshot_targets_counts_note(skips));
        }
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
mod tests;
