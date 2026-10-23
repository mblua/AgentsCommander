use super::*;

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
