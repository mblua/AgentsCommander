use super::*;

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

// ── #2228 phase 2: stderr notes disclose counts and closed literals only ──

/// Tier-B boundary test: the counts note may carry integers and nothing
/// else, whatever the candidate paths were.
#[test]
fn snapshot_targets_note_discloses_counts_only() {
    let candidate_paths = [
        "/home/someone/secret-project",
        r"C:\Users\someone\room-9-private",
        "proj-a:wg-1-dev-team/dev-rust",
    ];
    let skips = crate::config::teams::SnapshotDiscoverySkips {
        project_children: 3,
        rooms: 1,
        replicas: 7,
    };
    let note = snapshot_targets_counts_note(skips);
    for forbidden in ['/', '\\', ':'] {
        assert!(
            !note.contains(forbidden),
            "note must not contain {forbidden:?}: {note}"
        );
    }
    for path in candidate_paths {
        for segment in path.split(['/', '\\', ':']).filter(|s| s.len() > 2) {
            assert!(!note.contains(segment), "note leaked {segment:?}: {note}");
        }
    }
    let (head, rest) = note.split_once(' ').expect("note must have fields");
    assert_eq!(head, "snapshot_targets_note");
    for field in rest.split(' ') {
        let (key, value) = field.split_once('=').expect("field must be key=value");
        assert!(
            !key.is_empty() && key.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
            "key {key:?} must match [a-z_]+"
        );
        assert!(
            !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()),
            "value {value:?} must be an integer"
        );
    }
    assert_eq!(
        note,
        "snapshot_targets_note skipped_project_children=3 skipped_rooms=1 skipped_replicas=7"
    );
}

/// The second note shape, which test 10's integer regex deliberately does
/// not cover.
#[test]
fn snapshot_targets_note_non_orchestrator_is_a_closed_literal() {
    for reason in [
        SnapshotTargetsSilentReason::NotRoomOrchestrator,
        SnapshotTargetsSilentReason::NotRoomReplica,
    ] {
        let note = snapshot_targets_reason_note(reason);
        let rendered = note
            .strip_prefix("snapshot_targets_note reason=")
            .unwrap_or_else(|| panic!("unexpected note shape: {note}"));
        let mut characters = rendered.chars();
        assert!(
            characters.next().is_some_and(|c| c.is_ascii_lowercase()),
            "{rendered:?} must start with [a-z]"
        );
        assert!(
            rendered
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "{rendered:?} must match ^[a-z][a-z0-9_]*$"
        );
    }
    assert_eq!(
        snapshot_targets_reason_note(SnapshotTargetsSilentReason::NotRoomOrchestrator),
        "snapshot_targets_note reason=requester_not_room_orchestrator"
    );
    assert_eq!(
        snapshot_targets_reason_note(SnapshotTargetsSilentReason::NotRoomReplica),
        "snapshot_targets_note reason=requester_not_room_replica"
    );
}
