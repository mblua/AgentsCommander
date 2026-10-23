use super::*;
use crate::config::sessions_persistence::PersistedSession;
use crate::session::session::SessionCommunication;
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
        blocked_menu: false,
        blocked_menu_message: None,
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
        blocked_menu: false,
        blocked_menu_message: None,
        exit_code: None,
        context_percent: None,
        agent: None,
        profile: None,
        coding_agents: HashMap::new(),
    }
}

// §6.1 — command exists in clap

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
    peer.blocked_menu = status.blocked_menu;
    peer.blocked_menu_message = status.blocked_menu_message.clone();
    peer
}

/// The agent-specific notice a #1646/#1647 menu-guard row carries.
const BLOCKED_MENU_MESSAGE: &str =
    "codex is waiting for you to answer the folder-trust menu in this terminal";

/// #1774 - `ps_row` hardcodes one UUID for every row it builds, so a
/// multi-row fixture assigns its own ids to keep the two rows distinct.
const CHOSEN_SESSION_ID: &str = "22222222-2222-2222-2222-222222222222";

const OTHER_SESSION_ID: &str = "33333333-3333-3333-3333-333333333333";

/// #1774 - two more ids, so the three-row fixtures can bracket the chosen
/// row: `BELOW_CHOSEN_SESSION_ID` sorts under `CHOSEN_SESSION_ID` and
/// `TRAILING_SESSION_ID` over it, so a lexical minimum-by-id or
/// maximum-by-id read cannot coincide with `chosen`.
const BELOW_CHOSEN_SESSION_ID: &str = "00000000-0000-0000-0000-000000000000";

const TRAILING_SESSION_ID: &str = "44444444-4444-4444-4444-444444444444";

/// #1774 - two more notices, distinct from `BLOCKED_MENU_MESSAGE` and from
/// each other, so a fixture can give three blocked rows three different
/// messages and see which one is reported. They sort
/// `OTHER_BLOCKED_MENU_MESSAGE` < `BLOCKED_MENU_MESSAGE` <
/// `TRAILING_BLOCKED_MENU_MESSAGE`, so the chosen row's message is the
/// lexical middle one as well as the middle row.
const OTHER_BLOCKED_MENU_MESSAGE: &str =
    "claude is waiting for you to answer the tool-permission menu in this terminal";

const TRAILING_BLOCKED_MENU_MESSAGE: &str =
    "gemini is waiting for you to answer the model-selection menu in this terminal";

mod blocked_menu;
mod discovery;
mod lean_json;
mod peer_filter;
mod role_execute;
mod status_index;
