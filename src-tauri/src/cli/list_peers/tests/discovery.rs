use super::*;

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
    let peers = discover_wg_peers(wg, &[]);
    let names: Vec<&str> = peers.iter().map(|peer| peer.name.as_str()).collect();

    assert!(names.contains(&"proj-a:wg-1-devs/bob"));
}

#[test]
fn discover_wg_peers_for_portable_verified_coordinator_includes_cross_wg_and_root() {
    let (_temp, wg1_tech_lead, _wg1_dev_rust) = make_portable_cross_wg_fixture();
    let wg = detect_wg_replica(wg1_tech_lead.to_str().unwrap())
        .unwrap()
        .expect("portable coordinator replica");

    let peers = discover_wg_peers(wg, &[]);
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

    let peers = discover_wg_peers(wg, &[]);
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
