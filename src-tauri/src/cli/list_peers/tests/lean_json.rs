use super::*;

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

// ── #1669: blockedMenu projection through the peer→session join ──
