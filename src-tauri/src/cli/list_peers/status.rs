use std::collections::HashMap;

use crate::config::sessions_persistence::{load_sessions_raw, PersistedSession};
use crate::session::session::{SessionCommunicationKind, SessionStatus, TEMP_SESSION_PREFIX};

// ── Issue #206: working-state derivation from sessions.json ──────────

pub(super) struct CandidateSession {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) status: SessionStatus,
    pub(super) waiting_for_input: bool,
    pub(super) context_percent: Option<u8>,
    pub(super) agent_id: Option<String>,
    pub(super) effective_profile: Option<String>,
    /// #1669 - the row is parked on a blocking terminal menu
    /// (`communication.kind == BlockedMenu && visible`).
    pub(super) blocked_menu: bool,
    /// #1669 - the agent-specific notice for that menu, when the row carries one.
    pub(super) blocked_menu_message: Option<String>,
}

pub(super) struct PeerStatus {
    pub(super) working: bool,
    pub(super) session_status: &'static str,
    pub(super) status_legacy: &'static str,
    pub(super) session_id: Option<String>,
    pub(super) waiting_for_input: bool,
    pub(super) exit_code: Option<i32>,
    pub(super) context_percent: Option<u8>,
    pub(super) agent_id: Option<String>,
    pub(super) effective_profile: Option<String>,
    /// #1669 - the chosen session is parked on a blocking terminal menu.
    pub(super) blocked_menu: bool,
    /// #1669 - the agent-specific notice for that menu, when there is one.
    pub(super) blocked_menu_message: Option<String>,
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
            blocked_menu: false,
            blocked_menu_message: None,
        }
    }
}

pub(super) fn norm_path(path: &str) -> String {
    let stripped = path.strip_prefix(r"\\?\").unwrap_or(path);
    stripped
        .replace('\\', "/")
        .to_lowercase()
        .trim_end_matches('/')
        .to_string()
}

pub(super) fn canon_or_norm(path: &str) -> String {
    match std::fs::canonicalize(path) {
        Ok(canon) => norm_path(&canon.to_string_lossy()),
        Err(_) => norm_path(path),
    }
}

/// Pure inner: build the cwd → candidate index from a slice of persisted rows.
/// Exposed (private) so unit tests can drive it without touching the filesystem.
pub(super) fn build_session_index_from(
    rows: &[PersistedSession],
) -> HashMap<String, Vec<CandidateSession>> {
    let mut index: HashMap<String, Vec<CandidateSession>> = HashMap::new();
    for ps in rows {
        if ps.name.starts_with(TEMP_SESSION_PREFIX) {
            continue;
        }
        let (Some(id), Some(status)) = (ps.id.clone(), ps.status.clone()) else {
            continue;
        };
        let key = canon_or_norm(&ps.working_directory);
        // #1669: a blocked terminal is a visible BlockedMenu communication. RaiseHand
        // and cleared/invisible communications are not blocked terminals.
        let blocked_menu = ps
            .communication
            .as_ref()
            .is_some_and(|c| c.kind == SessionCommunicationKind::BlockedMenu && c.visible);
        let blocked_menu_message = if blocked_menu {
            ps.communication.as_ref().and_then(|c| c.message.clone())
        } else {
            None
        };
        index.entry(key).or_default().push(CandidateSession {
            id,
            name: ps.name.clone(),
            status,
            waiting_for_input: ps.waiting_for_input.unwrap_or(false),
            context_percent: ps.context_percent,
            agent_id: ps.agent_id.clone(),
            effective_profile: ps.effective_profile.clone(),
            blocked_menu,
            blocked_menu_message,
        });
    }
    index
}

/// Production entry point: read sessions.json and build the index.
pub(super) fn build_session_index() -> HashMap<String, Vec<CandidateSession>> {
    build_session_index_from(&load_sessions_raw())
}

/// Priority: waiting(4) > active(3) > running(2) > idle(1) > exited(0).
/// Uses `match &c.status` to avoid moving the non-Copy `SessionStatus`
/// (matches the proven pattern in `list_sessions.rs:status_tag`).
pub(super) fn priority(c: &CandidateSession) -> u8 {
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
pub(super) fn compute_peer_status(
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

    // #1669: `snapshot_sessions` deliberately preserves `communication` for exited
    // rows (#747), so a crash between the block and the exit can leave a stale
    // BlockedMenu on a session whose terminal is gone. A terminal that exited is
    // not waiting on a menu, so suppress the pair rather than report a false
    // positive alongside sessionStatus == "exited".
    let blocked_menu = chosen.blocked_menu && !matches!(chosen.status, SessionStatus::Exited(_));
    let blocked_menu_message = if blocked_menu {
        chosen.blocked_menu_message.clone()
    } else {
        None
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
        blocked_menu,
        blocked_menu_message,
    }
}
