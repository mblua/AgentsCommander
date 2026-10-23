use super::*;

// ── resolve_agent_target tests (AR2-tests 12-16) ──

/// Auto-cleaned temp dir for fixture roots. Matches the convention used in
/// `phone/messaging.rs` tests — no new crate dependencies.
pub(super) struct FixtureRoot(PathBuf);
impl Drop for FixtureRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
impl FixtureRoot {
    pub(super) fn new(prefix: &str) -> Self {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        std::process::id().hash(&mut h);
        std::thread::current().id().hash(&mut h);
        let path = std::env::temp_dir().join(format!(
            "{}-{}-{}",
            prefix,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
            h.finish()
        ));
        std::fs::create_dir_all(&path).expect("fixture root");
        Self(path)
    }
    pub(super) fn path(&self) -> &Path {
        &self.0
    }
}

/// Build a fake project layout on disk so `resolve_agent_target` can scan it.
/// `projects` is a slice of `(project_name, &[(wg_name, &[agent_short])])`.
// The nested-slice shape is the most direct way to express the fixture; a
// type alias would obscure the structure at the only call sites.
#[allow(clippy::type_complexity)]
pub(super) fn make_project_fixture(
    projects: &[(&str, &[(&str, &[&str])])],
) -> (FixtureRoot, Vec<String>) {
    let tmp = FixtureRoot::new("teams-fixture");
    for (proj_name, wgs) in projects {
        let proj_dir = tmp.path().join(proj_name);
        std::fs::create_dir_all(&proj_dir).unwrap();
        let ac_root = proj_dir.join(".ac");
        std::fs::create_dir_all(&ac_root).unwrap();
        for (wg_name, agents) in *wgs {
            let wg_dir = ac_root.join(wg_name);
            std::fs::create_dir_all(&wg_dir).unwrap();
            for agent in *agents {
                let replica = wg_dir.join(format!("__agent_{}", agent));
                std::fs::create_dir_all(&replica).unwrap();
            }
        }
    }
    let paths = vec![tmp.path().to_string_lossy().to_string()];
    (tmp, paths)
}

pub(super) fn make_coordinator_fixture(
    spoofed_coordinator_identity: bool,
) -> (FixtureRoot, Vec<String>) {
    let tmp = FixtureRoot::new("teams-coord-fixture");
    let project = tmp.path().join("proj-a");
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
        concat!(
            r#"{"agents":["../_agent_dev-rust","../_agent_tech-lead"],"#,
            r#""coordinator":"../_agent_tech-lead"}"#
        ),
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

    let paths = vec![tmp.path().to_string_lossy().to_string()];
    (tmp, paths)
}

/// #1245: the team-config shape the application's writer actually produces,
/// with the coordinator listed inside `agents`. A third member exists so the
/// surviving member order can be pinned once the coordinator entry is
/// consumed; `make_coordinator_fixture` has only one ordinary member.
fn make_writer_shaped_team_fixture() -> (FixtureRoot, Vec<String>) {
    let tmp = FixtureRoot::new("teams-writer-shape-fixture");
    let ac_root = tmp.path().join("proj-a").join(".ac");
    let team_dir = ac_root.join("_team_dev-team");
    let wg_dir = ac_root.join("wg-1-dev-team");

    std::fs::create_dir_all(&team_dir).unwrap();
    for agent in ["tech-lead", "dev-rust", "dev-ts"] {
        std::fs::create_dir_all(ac_root.join(format!("_agent_{agent}"))).unwrap();
        let replica = wg_dir.join(format!("__agent_{agent}"));
        std::fs::create_dir_all(&replica).unwrap();
        std::fs::write(
            replica.join("config.json"),
            format!(r#"{{"identity":"../../_agent_{agent}"}}"#),
        )
        .unwrap();
    }
    std::fs::write(
        team_dir.join("config.json"),
        concat!(
            r#"{"agents":["../_agent_dev-rust","../_agent_tech-lead","../_agent_dev-ts"],"#,
            r#""coordinator":"../_agent_tech-lead"}"#
        ),
    )
    .unwrap();

    let paths = vec![tmp.path().to_string_lossy().to_string()];
    (tmp, paths)
}

#[test]
fn team_config_with_coordinator_in_agents_verifies_and_excludes_the_coordinator_from_members() {
    let (fixture, paths) = make_writer_shaped_team_fixture();
    let ac_root = fixture.path().join("proj-a").join(".ac");
    let wg_dir = ac_root.join("wg-1-dev-team");
    let tech_lead = wg_dir.join("__agent_tech-lead");
    let dev_rust = wg_dir.join("__agent_dev-rust");

    let route =
        verify_pty_input_route(&tech_lead, false, "proj-a:wg-1-dev-team/dev-rust", &paths).unwrap();
    assert_eq!(route.sender.canonical_fqn, "proj-a:wg-1-dev-team/tech-lead");
    assert!(route.sender.is_coordinator);

    assert!(
        verify_pty_input_route(&dev_rust, false, "proj-a:wg-1-dev-team/dev-ts", &paths).is_err(),
        "a worker sender must still be rejected"
    );

    let snapshot =
        verify_terminal_snapshot_route(&tech_lead, false, "proj-a:wg-1-dev-team/dev-ts", &paths)
            .unwrap();
    assert_eq!(snapshot.kind, TerminalSnapshotAuthorityKind::Coordinator);

    // The coordinator is still not a valid capture target even though it now
    // appears in `agents`. This guards the authorization matrix. It cannot
    // observe the member exclusion: `verify_replica` derives
    // `is_coordinator` from the `coordinator` key and short-circuits the
    // `target_not_member` gate, so it holds for any `members` content.
    assert!(verify_terminal_snapshot_route(
        &tech_lead,
        false,
        "proj-a:wg-1-dev-team/tech-lead",
        &paths,
    )
    .is_err());

    // Only a direct call can pin the exclusion. Asserting the whole vector
    // also pins that consuming the coordinator entry leaves the order and
    // the count of the remaining members untouched.
    let (coordinator, members, _) = team_members(&ac_root, "dev-team").unwrap();
    assert_eq!(coordinator, "tech-lead");
    assert_eq!(members, vec!["dev-rust".to_string(), "dev-ts".to_string()]);
}

#[test]
fn team_config_rejects_repeated_agent_and_repeated_coordinator_entries() {
    let (fixture, paths) = make_writer_shaped_team_fixture();
    let ac_root = fixture.path().join("proj-a").join(".ac");
    let team_config = ac_root.join("_team_dev-team").join("config.json");
    let tech_lead = ac_root.join("wg-1-dev-team").join("__agent_tech-lead");

    for agents in [
        r#"["../_agent_dev-rust","../_agent_dev-rust","../_agent_tech-lead"]"#,
        r#"["../_agent_tech-lead","../_agent_tech-lead","../_agent_dev-rust"]"#,
        // Equivalent spellings collapse in `agent_bare_name_from_ref`, so a
        // repeated coordinator is caught whichever way it is written.
        r#"["../_agent_tech-lead","_agent_tech-lead","../_agent_dev-rust"]"#,
    ] {
        std::fs::write(
            &team_config,
            format!(r#"{{"agents":{agents},"coordinator":"../_agent_tech-lead"}}"#),
        )
        .unwrap();
        assert!(
            verify_pty_input_route(&tech_lead, false, "proj-a:wg-1-dev-team/dev-rust", &paths)
                .is_err(),
            "duplicate detection must not weaken for agents {agents}"
        );
    }
}

#[test]
fn strict_pty_target_syntax_rejects_aliases_paths_and_wildcards() {
    assert!(validate_pty_input_target_syntax("proj-a:wg-1-dev-team/dev-rust").is_ok());
    for invalid in [
        "dev-rust",
        "wg-1-dev-team/dev-rust",
        "proj-a/dev-rust",
        "proj-a:wg-1-dev-team/__agent_dev-rust",
        "proj-a:wg-1-dev-team/dev_rust",
        "proj-a:wg-x-dev-team/dev-rust",
        "proj-a:wg-1-dev-team/dev-rust/extra",
        "proj*:wg-1-dev-team/dev-rust",
        "../proj:wg-1-dev-team/dev-rust",
        "proj-a:wg-1-dev-team/dev-rust\u{202e}",
    ] {
        assert!(
            validate_pty_input_target_syntax(invalid).is_err(),
            "invalid={invalid:?}"
        );
    }
}

#[test]
fn terminal_snapshot_syntax_accepts_policy_denied_origin_and_root_targets() {
    assert!(validate_terminal_snapshot_target_syntax("proj-a:wg-1-dev-team/dev-rust").is_ok());
    assert!(validate_terminal_snapshot_target_syntax("proj-a/dev-rust").is_ok());
    assert!(
        validate_terminal_snapshot_target_syntax(crate::config::root_agent::ROOT_AGENT_SENDER)
            .is_ok()
    );
    assert!(validate_terminal_snapshot_target_syntax("*").is_err());
}

#[test]
fn terminal_snapshot_coordinator_policy_is_distinct_from_pty_input() {
    let (fixture, paths) = make_coordinator_fixture(false);
    let ac_root = fixture.path().join("proj-a").join(".ac");
    let coordinator = ac_root.join("wg-1-dev-team").join("__agent_tech-lead");
    let route = verify_terminal_snapshot_route(
        &coordinator,
        false,
        "proj-a:wg-1-dev-team/dev-rust",
        &paths,
    )
    .unwrap();
    assert_eq!(route.kind, TerminalSnapshotAuthorityKind::Coordinator);
    assert_eq!(route.target.canonical_fqn, "proj-a:wg-1-dev-team/dev-rust");
    let diagnostic = format!("{route:?}");
    let sender_path = route.sender.replica_root.to_string_lossy().into_owned();
    let target_path = route.target.replica_root.to_string_lossy().into_owned();
    for forbidden in [
        route.sender.canonical_fqn.as_str(),
        route.target.canonical_fqn.as_str(),
        sender_path.as_str(),
        target_path.as_str(),
        route.sender.incarnation_fingerprint.as_str(),
        route.target.authority_fingerprint.as_str(),
    ] {
        assert!(!diagnostic.contains(forbidden));
    }
    assert!(diagnostic.contains("kind: Coordinator"));
    assert!(verify_terminal_snapshot_route(
        &coordinator,
        false,
        "proj-a:wg-1-dev-team/tech-lead",
        &paths,
    )
    .is_err());
    assert!(verify_terminal_snapshot_route(
        &coordinator,
        false,
        crate::config::root_agent::ROOT_AGENT_SENDER,
        &paths,
    )
    .is_err());
}

#[test]
fn terminal_snapshot_target_debug_omits_identity_and_path_text() {
    const AUTH_CANARY: &str = "AUTH_1173_T8F2";
    const PATH_CANARY: &str = r"C:\PATH_1173_T8F2\replica";
    let target = TerminalSnapshotTargetIdentity {
        canonical_fqn: AUTH_CANARY.to_string(),
        replica_root: PathBuf::from(PATH_CANARY),
        project: AUTH_CANARY.to_string(),
        workgroup: AUTH_CANARY.to_string(),
        team: AUTH_CANARY.to_string(),
        is_coordinator: true,
    };
    let diagnostic = format!("{target:?}");
    assert!(!diagnostic.contains(AUTH_CANARY));
    assert!(!diagnostic.contains(PATH_CANARY));
    assert!(diagnostic.contains("is_coordinator: true"));
}

#[test]
fn privileged_route_is_exact_and_uses_duplicate_free_identity_snapshots() {
    let (fixture, mut paths) = make_coordinator_fixture(false);
    let ac_root = fixture.path().join("proj-a").join(".ac");
    let coordinator = ac_root.join("wg-1-dev-team").join("__agent_tech-lead");
    let route =
        verify_pty_input_route(&coordinator, false, "proj-a:wg-1-dev-team/dev-rust", &paths)
            .unwrap();
    assert_eq!(route.sender.canonical_fqn, "proj-a:wg-1-dev-team/tech-lead");
    assert_eq!(route.target.canonical_fqn, "proj-a:wg-1-dev-team/dev-rust");
    assert_eq!(route.kind, PtyInputAuthorityKind::Coordinator);

    paths.push(paths[0].clone());
    assert!(
        verify_pty_input_route(&coordinator, false, "proj-a:wg-1-dev-team/dev-rust", &paths,)
            .is_ok()
    );
    assert!(
        verify_pty_input_route(&coordinator, false, "Proj-a:wg-1-dev-team/dev-rust", &paths,)
            .is_err()
    );
    assert!(verify_pty_input_route(
        &coordinator,
        false,
        "proj-a:wg-1-dev-team/tech-lead",
        &paths,
    )
    .is_err());

    std::fs::write(
        ac_root.join("_team_dev-team").join("config.json"),
        concat!(
            r#"{"agents":["../_agent_dev-rust","../_agent_tech-lead"],"agents":[],"#,
            r#""coordinator":"../_agent_tech-lead"}"#
        ),
    )
    .unwrap();
    assert!(
        verify_pty_input_route(&coordinator, false, "proj-a:wg-1-dev-team/dev-rust", &paths,)
            .is_err()
    );
}

#[test]
fn sender_incarnation_survives_benign_config_content_changes() {
    let (fixture, paths) = make_coordinator_fixture(false);
    let ac_root = fixture.path().join("proj-a").join(".ac");
    let coordinator = ac_root.join("wg-1-dev-team").join("__agent_tech-lead");
    let first =
        verify_pty_input_route(&coordinator, false, "proj-a:wg-1-dev-team/dev-rust", &paths)
            .unwrap();

    std::fs::write(
        coordinator.join("config.json"),
        r#"{"identity":"../../_agent_tech-lead","benign":"changed"}"#,
    )
    .unwrap();
    std::fs::write(
        ac_root.join("_team_dev-team").join("config.json"),
        concat!(
            r#"{"agents":["../_agent_dev-rust","../_agent_tech-lead"],"#,
            r#""coordinator":"../_agent_tech-lead","benign":"changed"}"#
        ),
    )
    .unwrap();
    let second =
        verify_pty_input_route(&coordinator, false, "proj-a:wg-1-dev-team/dev-rust", &paths)
            .unwrap();

    assert_eq!(
        first.sender.incarnation_fingerprint, second.sender.incarnation_fingerprint,
        "the permanent sender incarnation must not include mutable config bytes"
    );
    assert_ne!(
        first.sender.authority_fingerprint, second.sender.authority_fingerprint,
        "queued authority revalidation must still observe config content changes"
    );
}

#[test]
fn privileged_route_rejects_noncanonical_identity_and_broken_project_roots() {
    let (fixture, mut paths) = make_coordinator_fixture(false);
    let ac_root = fixture.path().join("proj-a").join(".ac");
    let coordinator = ac_root.join("wg-1-dev-team").join("__agent_tech-lead");
    let coordinator_config = coordinator.join("config.json");
    std::fs::write(
        &coordinator_config,
        r#"{"identity":"elsewhere/_agent_tech-lead"}"#,
    )
    .unwrap();
    assert!(
        verify_pty_input_route(&coordinator, false, "proj-a:wg-1-dev-team/dev-rust", &paths,)
            .is_err()
    );

    std::fs::write(
        &coordinator_config,
        r#"{"identity":"../../_agent_tech-lead"}"#,
    )
    .unwrap();
    paths.push(
        fixture
            .path()
            .join("missing-project-root")
            .to_string_lossy()
            .to_string(),
    );
    assert!(
        verify_pty_input_route(&coordinator, false, "proj-a:wg-1-dev-team/dev-rust", &paths,)
            .is_err()
    );
}

#[test]
fn privileged_route_rejects_duplicate_replica_identity_and_worker_sender() {
    let (fixture, paths) = make_coordinator_fixture(false);
    let ac_root = fixture.path().join("proj-a").join(".ac");
    let coordinator = ac_root.join("wg-1-dev-team").join("__agent_tech-lead");
    let worker = ac_root.join("wg-1-dev-team").join("__agent_dev-rust");
    assert!(
        verify_pty_input_route(&worker, false, "proj-a:wg-1-dev-team/tech-lead", &paths,).is_err()
    );
    std::fs::write(
        worker.join("config.json"),
        r#"{"identity":"../../_agent_dev-rust","identity":"../../_agent_dev-rust"}"#,
    )
    .unwrap();
    assert!(
        verify_pty_input_route(&coordinator, false, "proj-a:wg-1-dev-team/dev-rust", &paths,)
            .is_err()
    );
}

pub(super) fn make_portable_coordinator_fixture(
    spoofed_coordinator_identity: bool,
) -> (FixtureRoot, PathBuf, PathBuf, Vec<String>) {
    let tmp = FixtureRoot::new("teams-portable-coord-fixture");
    let project = tmp.path().join("proj-a");
    let ac_root = project.join(".ac");
    let team_dir = ac_root.join("_team_dev-team");
    let local_tech_lead = ac_root.join("_agent_tech-lead");
    let local_dev_rust = ac_root.join("_agent_dev-rust");
    let origin = tmp.path().join("origin-matrix").join(".ac");
    let origin_tech_lead = origin.join("_agent_tech-lead");
    let origin_dev_rust = origin.join("_agent_dev-rust");
    let wg_dir = ac_root.join("wg-1-dev-team");
    let tech_lead_replica = wg_dir.join("__agent_tech-lead");
    let dev_rust_replica = wg_dir.join("__agent_dev-rust");

    for dir in [
        &team_dir,
        &local_tech_lead,
        &local_dev_rust,
        &origin_tech_lead,
        &origin_dev_rust,
        &tech_lead_replica,
        &dev_rust_replica,
    ] {
        std::fs::create_dir_all(dir).unwrap();
    }

    std::fs::write(
        team_dir.join("config.json"),
        r#"{"agents":["_agent_dev-rust"],"coordinator":"_agent_tech-lead"}"#,
    )
    .unwrap();

    let tech_lead_identity = if spoofed_coordinator_identity {
        &origin_dev_rust
    } else {
        &origin_tech_lead
    }
    .to_string_lossy()
    .replace('\\', "/");
    std::fs::write(
        tech_lead_replica.join("config.json"),
        format!(r#"{{"identity":"{}"}}"#, tech_lead_identity),
    )
    .unwrap();

    let dev_rust_identity = origin_dev_rust.to_string_lossy().replace('\\', "/");
    std::fs::write(
        dev_rust_replica.join("config.json"),
        format!(r#"{{"identity":"{}"}}"#, dev_rust_identity),
    )
    .unwrap();

    let paths = vec![tmp.path().to_string_lossy().to_string()];
    (tmp, ac_root, wg_dir, paths)
}

#[test]
fn resolve_wg_coordinator_replica_uses_identity_not_dir_name() {
    let (tmp, _paths) = make_coordinator_fixture(false);
    let ac_root = tmp.path().join("proj-a").join(".ac");
    let wg_dir = ac_root.join("wg-1-dev-team");

    let resolved = resolve_wg_coordinator_replica(&ac_root, &wg_dir).expect("coordinator");

    assert_eq!(resolved.project, "proj-a");
    assert_eq!(resolved.team, "dev-team");
    assert_eq!(resolved.wg_name, "wg-1-dev-team");
    assert_eq!(resolved.agent_name, "tech-lead");
    assert_eq!(
        identity_compare_key(&resolved.replica_dir),
        identity_compare_key(&wg_dir.join(format!("__agent_{}", resolved.agent_name)))
    );
}

#[test]
fn resolve_wg_coordinator_replica_rejects_spoofed_name() {
    let (tmp, _paths) = make_coordinator_fixture(true);
    let ac_root = tmp.path().join("proj-a").join(".ac");
    let wg_dir = ac_root.join("wg-1-dev-team");

    assert!(resolve_wg_coordinator_replica(&ac_root, &wg_dir).is_none());
}

#[test]
fn resolve_wg_coordinator_replica_accepts_portable_ref_with_external_identity() {
    let (_tmp, ac_root, wg_dir, _paths) = make_portable_coordinator_fixture(false);

    let resolved = resolve_wg_coordinator_replica(&ac_root, &wg_dir)
        .expect("portable coordinator ref should match declared identity agent");

    assert_eq!(resolved.project, "proj-a");
    assert_eq!(resolved.team, "dev-team");
    assert_eq!(resolved.wg_name, "wg-1-dev-team");
    assert_eq!(resolved.agent_name, "tech-lead");
}

#[test]
fn resolve_wg_coordinator_replica_rejects_portable_ref_with_spoofed_identity() {
    let (_tmp, ac_root, wg_dir, _paths) = make_portable_coordinator_fixture(true);

    assert!(resolve_wg_coordinator_replica(&ac_root, &wg_dir).is_none());
}

/// Build a fixture where both team config and replica configs reference
/// a renamed folder (`renamed-workspace`) that no longer exists on disk.
/// Mirrors the post-workspace-rename state from #299: persisted refs are
/// stale, but the same-workspace local matrices exist and are the only valid
/// authority targets.
fn make_stale_coordinator_fixture(
    replica_spoofs_identity: bool,
) -> (FixtureRoot, PathBuf, PathBuf) {
    let tmp = FixtureRoot::new("teams-stale-coord-fixture");
    let project = tmp.path().join("proj-a");
    let ac_root = project.join(".ac");
    let team_dir = ac_root.join("_team_test-team");
    let wg_dir = ac_root.join("wg-1-test-team");
    let alpha_matrix = ac_root.join("_agent_test-alpha");
    let beta_matrix = ac_root.join("_agent_test-beta");
    let alpha_replica = wg_dir.join("__agent_test-alpha");
    let beta_replica = wg_dir.join("__agent_test-beta");

    for dir in [
        &team_dir,
        &alpha_matrix,
        &beta_matrix,
        &alpha_replica,
        &beta_replica,
    ] {
        std::fs::create_dir_all(dir).unwrap();
    }

    // Stale absolute path (the `renamed-workspace` folder is never created).
    let stale_coord = tmp
        .path()
        .join("renamed-workspace")
        .join(".ac")
        .join("_agent_test-alpha");
    let stale_coord_str = stale_coord.to_string_lossy().replace('\\', "/");

    // Team config: coordinator points to the stale absolute path.
    std::fs::write(
        team_dir.join("config.json"),
        format!(r#"{{"coordinator":"{}"}}"#, stale_coord_str),
    )
    .unwrap();

    // Test-alpha replica identity: also stale. If `replica_spoofs_identity`,
    // points to a DIFFERENT stale matrix (the test-beta slot) — must reject.
    let alpha_identity = if replica_spoofs_identity {
        tmp.path()
            .join("renamed-workspace")
            .join(".ac")
            .join("_agent_test-beta")
            .to_string_lossy()
            .replace('\\', "/")
    } else {
        stale_coord_str.clone()
    };
    std::fs::write(
        alpha_replica.join("config.json"),
        format!(r#"{{"identity":"{}"}}"#, alpha_identity),
    )
    .unwrap();

    // Test-beta replica identity: stale, points at the test-beta matrix.
    // Not the coordinator — must not match.
    let beta_identity = tmp
        .path()
        .join("renamed-workspace")
        .join(".ac")
        .join("_agent_test-beta")
        .to_string_lossy()
        .replace('\\', "/");
    std::fs::write(
        beta_replica.join("config.json"),
        format!(r#"{{"identity":"{}"}}"#, beta_identity),
    )
    .unwrap();

    (tmp, ac_root, wg_dir)
}

/// Stale absolute identity refs are accepted only by repairing them
/// to the same-workspace local matrix with the same agent basename.
#[test]
fn resolve_wg_coordinator_replica_repairs_stale_absolute_refs() {
    let (_tmp, ac_root, wg_dir) = make_stale_coordinator_fixture(false);

    let resolved = resolve_wg_coordinator_replica(&ac_root, &wg_dir)
        .expect("same-workspace repair should resolve coordinator with stale refs");

    assert_eq!(resolved.agent_name, "test-alpha");
    assert_eq!(resolved.team, "test-team");
    assert_eq!(resolved.wg_name, "wg-1-test-team");
    let repaired_config: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(wg_dir.join("__agent_test-alpha").join("config.json"))
            .expect("read repaired config"),
    )
    .expect("parse repaired config");
    assert_eq!(repaired_config["identity"], "../../_agent_test-alpha");
}

/// #299: even when refs are stale, spoofing must still be rejected — a
/// replica that declares a stale identity ref naming a DIFFERENT matrix
/// (e.g. `_agent_test-beta`) must not be accepted as coordinator.
#[test]
fn resolve_wg_coordinator_replica_rejects_spoofed_stale_identity() {
    let (_tmp, ac_root, wg_dir) = make_stale_coordinator_fixture(true);

    // The test-alpha replica has been spoofed to claim the test-beta matrix.
    // The test-beta replica claims the test-beta matrix too. Neither matches
    // the team's coordinator ref (`_agent_test-alpha`), so no coordinator
    // is resolved.
    assert!(resolve_wg_coordinator_replica(&ac_root, &wg_dir).is_none());
}

/// Relative identity traversing out of the workspace must be repaired
/// to the same-workspace local matrix, not compared against the stale target.
#[test]
fn resolve_wg_coordinator_replica_repairs_stale_relative_identity() {
    let tmp = FixtureRoot::new("teams-stale-rel-fixture");
    let project = tmp.path().join("proj-a");
    let ac_root = project.join(".ac");
    let team_dir = ac_root.join("_team_test-team");
    let wg_dir = ac_root.join("wg-1-test-team");
    let alpha_matrix = ac_root.join("_agent_test-alpha");
    let alpha_replica = wg_dir.join("__agent_test-alpha");

    for dir in [&team_dir, &alpha_matrix, &alpha_replica] {
        std::fs::create_dir_all(dir).unwrap();
    }

    // Team config: stale absolute coordinator path
    // -> tmp/renamed-workspace/.ac/_agent_test-alpha
    let stale_abs = tmp
        .path()
        .join("renamed-workspace")
        .join(".ac")
        .join("_agent_test-alpha");
    let stale_abs_str = stale_abs.to_string_lossy().replace('\\', "/");
    std::fs::write(
        team_dir.join("config.json"),
        format!(r#"{{"coordinator":"{}"}}"#, stale_abs_str),
    )
    .unwrap();

    // Replica identity: relative path that traverses out to the same
    // legacy folder. From `<tmp>/proj-a/.ac/wg-1-test-team/__agent_test-alpha`,
    // `../../../../renamed-workspace/.ac/_agent_test-alpha` resolves to
    // `<tmp>/renamed-workspace/.ac/_agent_test-alpha` — the same logical
    // target as the team coordinator ref.
    std::fs::write(
        alpha_replica.join("config.json"),
        r#"{"identity":"../../../../renamed-workspace/.ac/_agent_test-alpha"}"#,
    )
    .unwrap();

    let resolved = resolve_wg_coordinator_replica(&ac_root, &wg_dir)
        .expect("same-workspace repair should resolve stale relative identity");
    assert_eq!(resolved.agent_name, "test-alpha");
}
