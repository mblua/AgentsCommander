use super::*;

#[test]
fn captured_registered_paths_define_team_authorization_without_own_root_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("registered");
    let team = project.join(".ac/_team_devs");
    std::fs::create_dir_all(&team).unwrap();
    std::fs::write(
        team.join("config.json"),
        r#"{"agents":["_agent_peer"],"coordinator":"_agent_coord"}"#,
    )
    .unwrap();
    let paths = vec![project.to_string_lossy().into_owned()];
    let discovered = discover_teams_from_project_paths(&paths);
    assert_eq!(discovered.len(), 1);
    assert!(is_any_coordinator(
        "registered:room-01-devs/coord",
        &discovered
    ));
    assert!(!is_any_coordinator("other:room-01-devs/coord", &discovered));
    assert!(discover_teams_from_project_paths(&[]).is_empty());
}

#[cfg(windows)]
#[test]
fn issue_2868_global_discovery_keeps_junction_team_and_filter_runs_before_read() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let ac = project.join(".ac");
    let target = temp.path().join("external-team");
    std::fs::create_dir_all(&ac).unwrap();
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("config.json"), "{\"agents\":[]}").unwrap();
    let link = ac.join("_team_linked");
    create_test_junction(&link, &target);
    let mut global = Vec::new();
    discover_teams_in_project(&project, &mut global);
    assert_eq!(global.len(), 1);
    assert_eq!(global[0].name, "linked");
    let mut filtered = Vec::new();
    discover_teams_in_project_filtered(&project, &mut filtered, |_| false);
    assert!(filtered.is_empty());
    assert_eq!(
        std::fs::read_to_string(target.join("config.json")).unwrap(),
        "{\"agents\":[]}"
    );
    std::fs::remove_dir(&link).unwrap();
}

// ── Helper-function tests (AR2-tests 1-7) ──

#[test]
fn agent_fqn_from_path_wg_replica() {
    let cwd = "C:/repos/proj-a/.ac/wg-1-devs/__agent_alice";
    assert_eq!(agent_fqn_from_path(cwd), "proj-a:wg-1-devs/alice");
}

#[test]
fn agent_fqn_from_path_wg_replica_windows_style() {
    let cwd = "C:/repos/proj-a/.ac/wg-1-devs/__agent_alice";
    assert_eq!(agent_fqn_from_path(cwd), "proj-a:wg-1-devs/alice");
}

#[test]
fn agent_fqn_from_path_origin() {
    // Non-WG (origin) path: falls back to agent_name_from_path shape.
    let cwd = "C:/repos/my-project/tech-lead";
    assert_eq!(agent_fqn_from_path(cwd), "my-project/tech-lead");
}

/// §G6 case 1: subdirectory inside a replica still resolves to the replica's FQN.
#[test]
fn agent_fqn_from_path_deeper_cwd_returns_replica_fqn() {
    let cwd = "C:/repos/proj-a/.ac/wg-1-devs/__agent_alice/some/deep/subdir";
    assert_eq!(agent_fqn_from_path(cwd), "proj-a:wg-1-devs/alice");
}

/// §G6 case 3: Windows UNC `\\?\` prefix must still resolve correctly.
#[test]
fn agent_fqn_from_path_handles_unc_prefix() {
    let cwd = r"\\?\C:\repos\proj-a\.ac\wg-1-devs\__agent_alice";
    assert_eq!(agent_fqn_from_path(cwd), "proj-a:wg-1-devs/alice");
}

/// §G6 case 2: parent path containing `.ac` anchors on the right-most one (rposition).
#[test]
fn agent_fqn_from_path_parent_ac_prefix() {
    let cwd = "C:/.ac/repos/proj-a/.ac/wg-1-devs/__agent_alice";
    assert_eq!(agent_fqn_from_path(cwd), "proj-a:wg-1-devs/alice");
}

// ── #516 workgroup_and_agent_from_path ──

#[test]
fn workgroup_and_agent_from_path_wg_replica() {
    let cwd = "C:/repos/proj-a/.ac/wg-5-dev-team/__agent_dev-rust";
    assert_eq!(
        workgroup_and_agent_from_path(cwd),
        (
            Some("wg-5-dev-team".to_string()),
            Some("dev-rust".to_string())
        )
    );
}

/// A deep subdir inside a replica (and Windows backslashes) still resolves
/// to the owning replica's WG/agent pair.
#[test]
fn workgroup_and_agent_from_path_deeper_cwd() {
    let cwd = r"C:\repos\proj-a\.ac\wg-1-devs\__agent_alice\repo-x\src";
    assert_eq!(
        workgroup_and_agent_from_path(cwd),
        (Some("wg-1-devs".to_string()), Some("alice".to_string()))
    );
}

#[test]
fn workgroup_and_agent_from_path_non_wg_returns_none() {
    assert_eq!(
        workgroup_and_agent_from_path("C:/repos/my-project/tech-lead"),
        (None, None)
    );
}

/// Root-agent dir has no `wg-`/`__agent_` segments: the helper returns
/// `(None, None)` and the caller applies the "Root agent" label fallback.
#[test]
fn workgroup_and_agent_from_path_root_agent_dir_returns_none() {
    assert_eq!(
        workgroup_and_agent_from_path("C:/repos/proj-a/.ac/ac-root-agent"),
        (None, None)
    );
}

// ── #566 project_from_path ──

#[test]
fn project_from_path_wg_replica() {
    let cwd = "C:/repos/AgentsCommander_ac/.ac/wg-5-dev-team/__agent_dev-rust";
    assert_eq!(
        project_from_path(cwd),
        Some("AgentsCommander_ac".to_string())
    );
}

/// Deep subdir inside a replica (and Windows backslashes) still anchors on
/// the right-most `.ac` and returns the owning project.
#[test]
fn project_from_path_deeper_cwd_windows_style() {
    let cwd = r"C:\repos\proj-a\.ac\wg-1-devs\__agent_alice\repo-x\src";
    assert_eq!(project_from_path(cwd), Some("proj-a".to_string()));
}

/// Permissive by design: root agents carry a project even though their
/// wg/agent resolve to `(None, None)` / the "Root agent" label fallback.
#[test]
fn project_from_path_root_agent() {
    assert_eq!(
        project_from_path("C:/repos/proj-a/.ac/ac-root-agent"),
        Some("proj-a".to_string())
    );
}

/// Origin / matrix agents also carry a project (the parent of `.ac`).
#[test]
fn project_from_path_origin_agent() {
    assert_eq!(
        project_from_path("C:/repos/proj-a/.ac/_agent_architect"),
        Some("proj-a".to_string())
    );
}

/// No `.ac` segment at all (ad-hoc / non-AC shell) -> `None`.
#[test]
fn project_from_path_no_ac_root_segment_returns_none() {
    assert_eq!(project_from_path("C:/repos/my-project/tech-lead"), None);
}

/// `.ac` as the first segment has no parent project dir -> `None`.
#[test]
fn project_from_path_ac_first_segment_returns_none() {
    assert_eq!(project_from_path("/.ac/wg-1-devs/__agent_alice"), None);
}

#[test]
fn split_project_prefix_present() {
    assert_eq!(
        split_project_prefix("proj-a:wg-1-devs/alice"),
        (Some("proj-a"), "wg-1-devs/alice")
    );
}

#[test]
fn split_project_prefix_absent() {
    assert_eq!(
        split_project_prefix("wg-1-devs/alice"),
        (None, "wg-1-devs/alice")
    );
    // Empty-project edge: `:foo` is not treated as qualified.
    assert_eq!(split_project_prefix(":foo"), (None, ":foo"));
    // Empty-local edge: `foo:` is not treated as qualified.
    assert_eq!(split_project_prefix("foo:"), (None, "foo:"));
}

#[test]
fn extract_wg_team_peels_project_prefix() {
    assert_eq!(
        extract_wg_team("proj-a:wg-1-dev-team/alice"),
        Some("dev-team")
    );
    assert_eq!(extract_wg_team("wg-1-dev-team/alice"), Some("dev-team"));
    assert_eq!(extract_wg_team("origin-proj/alice"), None);
}

// ── resolve_agent_target tests (AR2-tests 12-16) ──

/// Auto-cleaned temp dir for fixture roots. Matches the convention used in
/// `phone/messaging.rs` tests — no new crate dependencies.
struct FixtureRoot(PathBuf);
impl Drop for FixtureRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
impl FixtureRoot {
    fn new(prefix: &str) -> Self {
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
    fn path(&self) -> &Path {
        &self.0
    }
}

/// Build a fake project layout on disk so `resolve_agent_target` can scan it.
/// `projects` is a slice of `(project_name, &[(wg_name, &[agent_short])])`.
// The nested-slice shape is the most direct way to express the fixture; a
// type alias would obscure the structure at the only call sites.
#[allow(clippy::type_complexity)]
fn make_project_fixture(projects: &[(&str, &[(&str, &[&str])])]) -> (FixtureRoot, Vec<String>) {
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

fn make_coordinator_fixture(spoofed_coordinator_identity: bool) -> (FixtureRoot, Vec<String>) {
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
        r#"{"agents":["../_agent_dev-rust","../_agent_tech-lead"],"coordinator":"../_agent_tech-lead"}"#,
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
        r#"{"agents":["../_agent_dev-rust","../_agent_tech-lead","../_agent_dev-ts"],"coordinator":"../_agent_tech-lead"}"#,
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
        r#"{"agents":["../_agent_dev-rust","../_agent_tech-lead"],"agents":[],"coordinator":"../_agent_tech-lead"}"#,
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
        r#"{"agents":["../_agent_dev-rust","../_agent_tech-lead"],"coordinator":"../_agent_tech-lead","benign":"changed"}"#,
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

fn make_portable_coordinator_fixture(
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

// ── #299 logical_path_resolve unit tests ──────────────────────────

#[test]
fn logical_path_resolve_resolves_parent_dirs() {
    let p = Path::new("/a/b/c/../d");
    assert_eq!(logical_path_resolve(p), "/a/b/d");
}

#[test]
fn logical_path_resolve_drops_parent_above_root() {
    let p = Path::new("/a/../../b");
    // `/a/..` → `/`, then `/..` is dropped (can't go above root).
    assert_eq!(logical_path_resolve(p), "/b");
}

#[test]
fn logical_path_resolve_preserves_relative_parent_prefix() {
    let p = Path::new("../../foo");
    assert_eq!(logical_path_resolve(p), "../../foo");
}

#[test]
fn logical_path_resolve_strips_current_dir() {
    let p = Path::new("/a/./b/./c");
    assert_eq!(logical_path_resolve(p), "/a/b/c");
}

// ── #299 identity_compare_key unit tests ──────────────────────────

#[test]
fn identity_compare_key_equal_for_stale_paths_pointing_to_same_target() {
    // Two refs expressed differently but logically pointing at the same
    // non-existent legacy location should produce equal keys.
    let a = Path::new("/some/where/legacy/.ac/_agent_test-alpha");
    let b = Path::new("/some/where/proj/.ac/wg-1-test-team/__agent_test-alpha/../../../../legacy/.ac/_agent_test-alpha");
    assert_eq!(identity_compare_key(a), identity_compare_key(b));
}

#[test]
fn identity_compare_key_differs_for_different_matrix_names() {
    // Stale refs to different matrix dirs must produce different keys —
    // protects spoofing when both refs are stale.
    let a = Path::new("/some/where/legacy/.ac/_agent_test-alpha");
    let b = Path::new("/some/where/legacy/.ac/_agent_test-beta");
    assert_ne!(identity_compare_key(a), identity_compare_key(b));
}

#[test]
fn identity_compare_key_case_insensitive() {
    // Comparison is case-insensitive (Windows convention; matches
    // cli/list_peers::norm_path).
    let a = Path::new("/Some/Where/.ac/_Agent_Test-Alpha");
    let b = Path::new("/some/where/.ac/_agent_test-alpha");
    assert_eq!(identity_compare_key(a), identity_compare_key(b));
}

#[test]
fn verified_wg_coordinator_target_rejects_origin_coordinator() {
    let (_tmp, paths) = make_coordinator_fixture(false);

    assert!(verified_wg_coordinator_target("proj-a/tech-lead", &paths).is_none());
}

#[test]
fn verified_wg_coordinator_target_rejects_wrong_wg_member() {
    let (_tmp, paths) = make_coordinator_fixture(false);

    assert!(verified_wg_coordinator_target("proj-a:wg-1-dev-team/dev-rust", &paths).is_none());
}

#[test]
fn verified_wg_coordinator_target_accepts_identity_verified_coordinator() {
    let (_tmp, paths) = make_coordinator_fixture(false);

    let resolved = verified_wg_coordinator_target("proj-a:wg-1-dev-team/tech-lead", &paths)
        .expect("verified coordinator");

    assert_eq!(resolved.agent_name, "tech-lead");
    assert_eq!(resolved.team, "dev-team");
}

#[test]
fn verified_wg_coordinator_target_accepts_portable_ref_with_external_identity() {
    let (_tmp, _ac_root, _wg_dir, paths) = make_portable_coordinator_fixture(false);

    let resolved = verified_wg_coordinator_target("proj-a:wg-1-dev-team/tech-lead", &paths)
        .expect("portable verified coordinator");

    assert_eq!(resolved.agent_name, "tech-lead");
    assert_eq!(resolved.team, "dev-team");
}

#[test]
fn resolve_agent_target_passes_through_qualified() {
    let (_tmp, paths) = make_project_fixture(&[("proj-a", &[("wg-1-devs", &["alice"])])]);
    let fqn = "proj-a:wg-1-devs/alice";
    assert_eq!(resolve_agent_target(fqn, &paths).unwrap(), fqn);
}

#[test]
fn resolve_agent_target_qualifies_unambiguous_unqualified() {
    let (_tmp, paths) = make_project_fixture(&[("proj-a", &[("wg-1-devs", &["alice"])])]);
    let unqualified = "wg-1-devs/alice";
    assert_eq!(
        resolve_agent_target(unqualified, &paths).unwrap(),
        "proj-a:wg-1-devs/alice"
    );
}

#[test]
fn resolve_agent_target_rejects_ambiguous() {
    let (_tmp, paths) = make_project_fixture(&[
        ("proj-a", &[("wg-1-devs", &["alice"])]),
        ("proj-b", &[("wg-1-devs", &["alice"])]),
    ]);
    let err = resolve_agent_target("wg-1-devs/alice", &paths).unwrap_err();
    match err {
        ResolutionError::Ambiguous { target, candidates } => {
            assert_eq!(target, "wg-1-devs/alice");
            assert!(candidates.contains(&"proj-a:wg-1-devs/alice".to_string()));
            assert!(candidates.contains(&"proj-b:wg-1-devs/alice".to_string()));
            assert_eq!(candidates.len(), 2);
        }
        other => panic!("expected Ambiguous, got {:?}", other),
    }
}

#[test]
fn resolve_agent_target_rejects_unknown() {
    let (_tmp, paths) = make_project_fixture(&[("proj-a", &[("wg-1-devs", &["alice"])])]);
    // Qualified-but-missing.
    assert!(matches!(
        resolve_agent_target("proj-c:wg-1-devs/alice", &paths).unwrap_err(),
        ResolutionError::UnknownQualified(_)
    ));
    // Unqualified, zero candidates.
    assert!(matches!(
        resolve_agent_target("wg-9-none/nobody", &paths).unwrap_err(),
        ResolutionError::NoMatch(_)
    ));
    // Invalid shape: empty.
    assert!(matches!(
        resolve_agent_target("", &paths).unwrap_err(),
        ResolutionError::InvalidShape(_)
    ));
    // Invalid shape: double colon.
    assert!(matches!(
        resolve_agent_target("a:b:wg-1/x", &paths).unwrap_err(),
        ResolutionError::InvalidShape(_)
    ));
    // Invalid shape: qualified with non-WG local.
    assert!(matches!(
        resolve_agent_target("proj-a:not-wg/alice", &paths).unwrap_err(),
        ResolutionError::InvalidShape(_)
    ));
}

/// §DR4: `project_paths` entry is a parent dir containing sibling projects.
#[test]
fn resolve_agent_target_two_level_scan() {
    let tmp = FixtureRoot::new("teams-two-level");
    // Lay out: tmp/ contains proj-a/ and proj-b/, each with a .ac/ + colliding replica.
    for proj in ["proj-a", "proj-b"] {
        let replica = tmp
            .path()
            .join(proj)
            .join(".ac")
            .join("wg-1-devs")
            .join("__agent_alice");
        std::fs::create_dir_all(&replica).unwrap();
    }
    // project_paths = [tmp] (parent only — must descend one level).
    let paths = vec![tmp.path().to_string_lossy().to_string()];
    let err = resolve_agent_target("wg-1-devs/alice", &paths).unwrap_err();
    match err {
        ResolutionError::Ambiguous { candidates, .. } => {
            assert_eq!(candidates.len(), 2);
        }
        other => panic!("expected Ambiguous (two-level scan), got {:?}", other),
    }
}

// Origin-form and bare inputs pass through.
#[test]
fn resolve_agent_target_origin_and_bare_passthrough() {
    let (_tmp, paths) = make_project_fixture(&[]);
    assert_eq!(
        resolve_agent_target("some-project/agent", &paths).unwrap(),
        "some-project/agent"
    );
    assert_eq!(
        resolve_agent_target("bare-agent", &paths).unwrap(),
        "bare-agent"
    );
}

/// Issue #134: filesystem directory names like `__agent_shipper` are never
/// valid peer FQNs. They must reject early with `LooksLikeFilesystemDir`
/// regardless of where they appear (bare, after a project prefix, after a
/// WG-local prefix, or fully qualified).
#[test]
fn resolve_agent_target_rejects_filesystem_dir_names() {
    let (_tmp, paths) = make_project_fixture(&[]);
    for bad in [
        "__agent_shipper",
        "__agent_dev-rust",
        "_agent_architect",
        "_agent_tech-lead",
        "some-project/__agent_shipper",
        "wg-1-devs/__agent_alice",
        "proj-a:wg-1-devs/__agent_alice",
    ] {
        match resolve_agent_target(bad, &paths) {
            Err(ResolutionError::LooksLikeFilesystemDir(s)) => assert_eq!(s, bad),
            other => panic!(
                "expected LooksLikeFilesystemDir for {:?}, got {:?}",
                bad, other
            ),
        }
    }
}

#[test]
fn resolve_agent_target_accepts_root_agent_uri_verbatim() {
    let paths: Vec<String> = vec![];
    assert_eq!(
        resolve_agent_target(crate::config::root_agent::ROOT_AGENT_SENDER, &paths).unwrap(),
        crate::config::root_agent::ROOT_AGENT_SENDER
    );
}

#[test]
fn resolve_agent_target_still_rejects_other_multi_colon_strings() {
    let paths: Vec<String> = vec![];
    assert!(matches!(
        resolve_agent_target("a:b:wg-1/x", &paths),
        Err(ResolutionError::InvalidShape(_))
    ));
}

/// Validation #16: `is_coordinator_for_cwd` correctness guard.
/// Live sessions always run inside WG replica dirs (`wg-*/__agent_*`); the function
/// consumes those via `agent_name_from_path` + WG-aware `is_coordinator`.
#[test]
fn is_coordinator_for_cwd_matches_wg_replica() {
    let teams = vec![DiscoveredTeam {
        name: "dev-team".into(),
        project: "foo".into(),
        agent_names: vec!["foo/dev-rust".into()],
        agent_paths: vec![None],
        coordinator_name: Some("foo/tech-lead".into()),
        coordinator_path: None,
    }];

    // Coordinator replica (any WG of the same team) resolves true.
    let coord_cwd = "C:/repos/foo/.ac/wg-4-dev-team/__agent_tech-lead";
    assert!(is_coordinator_for_cwd(coord_cwd, &teams));

    // Non-coordinator member of the team → false.
    let member_cwd = "C:/repos/foo/.ac/wg-4-dev-team/__agent_dev-rust";
    assert!(!is_coordinator_for_cwd(member_cwd, &teams));

    // Unrelated agent outside any team → false.
    let other_cwd = "C:/repos/foo/.ac/wg-9-other-team/__agent_dev-rust";
    assert!(!is_coordinator_for_cwd(other_cwd, &teams));
}

/// Empty teams list → nothing is a coordinator.
#[test]
fn is_coordinator_for_cwd_empty_teams() {
    let teams: Vec<DiscoveredTeam> = vec![];
    let cwd = "C:/repos/foo/.ac/wg-1-dev-team/__agent_tech-lead";
    assert!(!is_coordinator_for_cwd(cwd, &teams));
}

// ── Team-membership tests (AR2-tests 8-11) ──

fn dev_team(project: &str) -> DiscoveredTeam {
    DiscoveredTeam {
        name: "dev-team".into(),
        project: project.into(),
        agent_names: vec![format!("{}/dev-rust", project)],
        agent_paths: vec![None],
        coordinator_name: Some(format!("{}/tech-lead", project)),
        coordinator_path: None,
    }
}

/// §DR7: WG-aware `is_in_team` must not cross project boundaries when
/// both sides are qualified.
#[test]
fn is_in_team_rejects_cross_project_wg_match() {
    let team_a = dev_team("proj-a");
    let team_b = dev_team("proj-b");
    let agent_in_a = "proj-a:wg-1-dev-team/dev-rust";
    assert!(is_in_team(agent_in_a, &team_a));
    assert!(!is_in_team(agent_in_a, &team_b));
}

/// §DR7: agents in colliding same-named WG teams across projects MUST NOT
/// communicate via the same-WG rule.
#[test]
fn can_communicate_rejects_cross_project_same_wg() {
    let team_a = dev_team("proj-a");
    let team_b = dev_team("proj-b");
    let teams = vec![team_a, team_b];
    let from = "proj-a:wg-1-dev-team/alice";
    let to = "proj-b:wg-1-dev-team/bob";
    assert!(!can_communicate(from, to, &teams));
}

/// §DR7: lenient tolerance for legacy-unqualified names — unqualified
/// pairs on the same WG can still communicate during the migration window.
#[test]
fn can_communicate_allows_legacy_unqualified() {
    let teams = vec![dev_team("proj-a")];
    let from = "wg-1-dev-team/alice";
    let to = "wg-1-dev-team/bob";
    assert!(can_communicate(from, to, &teams));
}

/// §DR7: `is_coordinator_for_cwd` resolves project from the CWD so
/// coordinators in different projects with same-named teams are isolated.
#[test]
fn is_coordinator_for_cwd_project_qualified() {
    let teams = vec![dev_team("proj-a"), dev_team("proj-b")];
    // tech-lead of proj-a's dev-team.
    let coord_a_cwd = "C:/repos/proj-a/.ac/wg-1-dev-team/__agent_tech-lead";
    // tech-lead of proj-b's dev-team.
    let coord_b_cwd = "C:/repos/proj-b/.ac/wg-1-dev-team/__agent_tech-lead";
    assert!(is_coordinator_for_cwd(coord_a_cwd, &teams));
    assert!(is_coordinator_for_cwd(coord_b_cwd, &teams));
}

/// Issue #77 regression guard: `is_any_coordinator` is the hot path used by
/// `commands::ac_discovery` to populate `AcAgentReplica.isCoordinator`. The
/// §AR2-strict gate in `is_coordinator` requires a project-qualified FQN —
/// callers that pass an unqualified WG-local name will silently get `false`
/// (which is exactly the bug fixed in #77). This test pins the contract so
/// no future refactor can re-introduce the regression.
#[test]
fn is_any_coordinator_requires_qualified_fqn() {
    let teams = vec![dev_team("foo")];

    // 1. Project-qualified WG replica matching the team's project → true.
    assert!(is_any_coordinator("foo:wg-1-dev-team/tech-lead", &teams));

    // 2. Unqualified WG replica (legacy shape) → false. §AR2-strict guard.
    assert!(!is_any_coordinator("wg-1-dev-team/tech-lead", &teams));

    // 3. Cross-project qualified → false (project mismatch).
    assert!(!is_any_coordinator("bar:wg-1-dev-team/tech-lead", &teams));
}

/// §AR2-strict: unqualified `from` (legacy) MUST NOT grant coordinator
/// authority even if the local part matches. Locks in the §DR8/§G13 call.
#[test]
fn is_coordinator_rejects_legacy_unqualified_from() {
    let teams = [dev_team("proj-a")];
    // Legacy-unqualified name — local part matches the team coordinator, but
    // with no project prefix the strict rule rejects.
    assert!(!is_coordinator("wg-1-dev-team/tech-lead", &teams[0]));
    // For completeness, the fully-qualified form DOES grant authority.
    assert!(is_coordinator("proj-a:wg-1-dev-team/tech-lead", &teams[0]));
}

/// #280 §3.4 — `note_missing_team_config` is a process-local one-shot
/// dedup keyed on `(project, team_dir)`. First sighting returns true so
/// the caller emits the INFO; later sightings return false so the WARN
/// storm collapses to a single line per unique pair per process.
#[test]
fn note_missing_team_config_returns_true_first_time_only() {
    // Use unique pair to avoid collisions with other tests' state — the
    // helper's HashSet is process-global.
    let project = "proj-test-280-3-4";
    let dir = "_team_foo_test_280_3_4";
    assert!(note_missing_team_config(project, dir));
    assert!(!note_missing_team_config(project, dir));
    // A different pair is independent.
    let dir2 = "_team_bar_test_280_3_4";
    assert!(note_missing_team_config(project, dir2));
    assert!(!note_missing_team_config(project, dir2));
}

// ──────────────────────────────────────────────────────────────────────
// #1535 walker confinement (plan §6, T1-T7)
// ──────────────────────────────────────────────────────────────────────

/// #1535 T1: the exact bug shape — a nested project's origin matrix dir
/// under an ancestor workgroup replica. The only `__agent_*` ancestor
/// (`__agent_alice`) belongs to the ANCESTOR project's `.ac`, so it must
/// not bind: the walker returns `Ok(None)` and the sessionRace gate is
/// skipped (pre-fix: `Ok(Some(alice))` → false-positive duplicate).
#[test]
fn strict_wg_replica_anchor_from_cwd_nested_project_origin_cwd_binds_nothing() {
    let temp = tempfile::TempDir::new().unwrap();
    let nested_origin = temp
        .path()
        .join("proj-outer")
        .join(".ac")
        .join("wg-1-devs")
        .join("__agent_alice")
        .join("nested-proj")
        .join(".ac")
        .join("_agent_phase0");
    std::fs::create_dir_all(&nested_origin).unwrap();

    let anchor = strict_wg_replica_anchor_from_cwd(&nested_origin).expect("walk must not error");
    assert_eq!(
        anchor, None,
        "the ancestor project's replica must never anchor a nested-project cwd"
    );
}

/// #1535 T2: a nested project with its OWN replica anchors that replica,
/// not the ancestor's.
#[test]
fn strict_wg_replica_anchor_from_cwd_nested_project_replica_binds_own_anchor() {
    let temp = tempfile::TempDir::new().unwrap();
    let alice = temp
        .path()
        .join("proj-outer")
        .join(".ac")
        .join("wg-1-devs")
        .join("__agent_alice");
    let bob = alice
        .join("nested-proj")
        .join(".ac")
        .join("wg-2-devs")
        .join("__agent_bob");
    for dir in [&alice, &bob] {
        std::fs::create_dir_all(dir).unwrap();
    }

    let anchor = strict_wg_replica_anchor_from_cwd(&bob).expect("walk must not error");
    let anchor = anchor.expect("the nested project's own replica must bind");
    assert_eq!(
        std::fs::canonicalize(&anchor).unwrap(),
        std::fs::canonicalize(&bob).unwrap(),
        "the nested replica must anchor itself, not the ancestor replica"
    );
    assert!(
        !anchor.ends_with("__agent_alice"),
        "the ancestor replica must not anchor a nested-project cwd"
    );
}

/// #1535 T3 regression: an ordinary replica cwd still anchors itself.
#[test]
fn strict_wg_replica_anchor_from_cwd_own_replica_regression() {
    let temp = tempfile::TempDir::new().unwrap();
    let alice = temp
        .path()
        .join("proj")
        .join(".ac")
        .join("wg-1-devs")
        .join("__agent_alice");
    std::fs::create_dir_all(&alice).unwrap();

    let anchor = strict_wg_replica_anchor_from_cwd(&alice).expect("walk must not error");
    assert_eq!(
        std::fs::canonicalize(anchor.expect("own replica must bind")).unwrap(),
        std::fs::canonicalize(&alice).unwrap()
    );
}

/// #1535 T4 regression: a deeper cwd inside an ordinary replica still
/// resolves to the same own anchor.
#[test]
fn strict_wg_replica_anchor_from_cwd_deeper_cwd_inside_own_replica_regression() {
    let temp = tempfile::TempDir::new().unwrap();
    let deep = temp
        .path()
        .join("proj")
        .join(".ac")
        .join("wg-1-devs")
        .join("__agent_alice")
        .join("some")
        .join("deep")
        .join("subdir");
    std::fs::create_dir_all(&deep).unwrap();

    let anchor = strict_wg_replica_anchor_from_cwd(&deep).expect("walk must not error");
    assert_eq!(
        std::fs::canonicalize(anchor.expect("deeper cwd must bind the own replica")).unwrap(),
        std::fs::canonicalize(
            temp.path()
                .join("proj")
                .join(".ac")
                .join("wg-1-devs")
                .join("__agent_alice")
        )
        .unwrap()
    );
}

/// #1535 T5/T6/T7 fixture: `strict_target_fixture`-shaped outer project
/// (`proj/.ac/{_team_team, _agent_lead, _agent_dev-one, wg-1-team/...}`)
/// plus a nested project with its own full workgroup inside
/// `__agent_dev-one`: `nested-proj/.ac/{_team_team, _agent_lead2,
/// _agent_inner2, wg-2-team/{__agent_lead2, __agent_inner2}}` and an
/// origin-only dir `_agent_phase0` (the #1535 bug shape). The nested
/// replica `config.json` identity is `../../_agent_inner2`; the nested
/// team config names the nested origin matrices.
fn nested_strict_target_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temp = tempfile::TempDir::new().unwrap();
    let project = temp.path().join("proj");
    let ac_root = project.join(".ac");
    let team = ac_root.join("_team_team");
    let wg = ac_root.join("wg-1-team");
    let outer_dev_one = wg.join("__agent_dev-one");
    for directory in [
        &team,
        &ac_root.join("_agent_lead"),
        &ac_root.join("_agent_dev-one"),
        &wg.join("__agent_lead"),
        &outer_dev_one,
    ] {
        std::fs::create_dir_all(directory).unwrap();
    }
    std::fs::write(
        team.join("config.json"),
        r#"{"agents":["../_agent_dev-one","../_agent_lead"],"coordinator":"../_agent_lead"}"#,
    )
    .unwrap();
    for (replica, identity) in [
        (&wg.join("__agent_lead"), "../../_agent_lead"),
        (&outer_dev_one, "../../_agent_dev-one"),
    ] {
        std::fs::write(
            replica.join("config.json"),
            format!(r#"{{"identity":"{identity}"}}"#),
        )
        .unwrap();
    }

    let nested_ac_root = outer_dev_one.join("nested-proj").join(".ac");
    let nested_team = nested_ac_root.join("_team_team");
    let nested_wg = nested_ac_root.join("wg-2-team");
    let nested_inner2 = nested_wg.join("__agent_inner2");
    let nested_origin = nested_ac_root.join("_agent_phase0");
    for directory in [
        &nested_team,
        &nested_ac_root.join("_agent_lead2"),
        &nested_ac_root.join("_agent_inner2"),
        &nested_wg.join("__agent_lead2"),
        &nested_inner2,
        &nested_origin,
    ] {
        std::fs::create_dir_all(directory).unwrap();
    }
    std::fs::write(
        nested_team.join("config.json"),
        r#"{"agents":["../_agent_lead2","../_agent_inner2"],"coordinator":"../_agent_lead2"}"#,
    )
    .unwrap();
    std::fs::write(
        nested_inner2.join("config.json"),
        r#"{"identity":"../../_agent_inner2"}"#,
    )
    .unwrap();

    (temp, nested_inner2, nested_origin)
}

/// #1535 T5: a nested project's replica resolves to its own verified
/// sender identity (project/workgroup/agent of the NESTED project), not
/// the ancestor workgroup's.
#[test]
fn verify_pty_input_replica_cwd_nested_replica_resolves_own_fqn() {
    let (_temp, nested_inner2, _nested_origin) = nested_strict_target_fixture();
    let identity = verify_pty_input_replica_cwd(&nested_inner2)
        .expect("nested replica must resolve its own identity");
    assert_eq!(identity.canonical_fqn, "nested-proj:wg-2-team/inner2");
    assert_eq!(identity.project, "nested-proj");
    assert_eq!(identity.workgroup, "wg-2-team");
    assert_eq!(identity.agent, "inner2");
}

/// #1535 T6: the #1535 bug-shaped cwd (an origin matrix dir, not a
/// replica) resolves to `Err("sender_identity_invalid")` — and never to
/// the outer workgroup.
#[test]
fn verify_pty_input_replica_cwd_nested_origin_cwd_is_invalid() {
    let (_temp, _nested_inner2, nested_origin) = nested_strict_target_fixture();
    let err =
        verify_pty_input_replica_cwd(&nested_origin).expect_err("origin dir must not resolve");
    assert_eq!(err, "sender_identity_invalid");
}

/// #1535 T7: the create-gate key derives from the NESTED project for a
/// nested replica cwd, and is `None` (gate skipped) for the nested origin
/// cwd.
#[test]
fn pty_input_create_gate_key_from_cwd_nested_replica_uses_own_fqn_and_nested_origin_is_none() {
    let (_temp, nested_inner2, nested_origin) = nested_strict_target_fixture();
    assert_eq!(
        pty_input_create_gate_key_from_cwd(&nested_inner2).expect("key must not error"),
        Some("nested-proj:wg-2-team/inner2".to_string())
    );
    assert_eq!(
        pty_input_create_gate_key_from_cwd(&nested_origin).expect("key must not error"),
        None
    );
}

// ── #2228 phase 2: fail-soft snapshot-target discovery (RED-FIRST FORM) ──

/// Unix-only: a child directory that passes `DirEntry::metadata().is_dir()`
/// (so it reaches `teams.rs:322`) but cannot be opened, so `verify_directory`
/// fails with `OpenFailed`. Returns the child path; the caller must restore
/// the mode so the fixture can be removed.
#[cfg(unix)]
fn make_unopenable_child(parent: &Path, name: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let child = parent.join(name);
    std::fs::create_dir_all(&child).unwrap();
    std::fs::set_permissions(&child, std::fs::Permissions::from_mode(0o000)).unwrap();
    child
}

#[cfg(unix)]
fn restore_child_mode(child: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(child, std::fs::Permissions::from_mode(0o700));
}

#[cfg(unix)]
fn running_as_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

#[cfg(unix)]
#[test]
fn enumerate_project_dirs_strict_skips_unopenable_child() {
    if running_as_root() {
        return;
    }
    let (tmp, paths) = make_coordinator_fixture(false);
    let child = make_unopenable_child(tmp.path(), "unopenable");
    let mut skipped = 0usize;
    let result = enumerate_project_dirs_strict(&paths, &mut skipped);
    restore_child_mode(&child);
    let projects = result.expect("scan must not abort on an unverifiable child");
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].2, ProjectOrigin::ScannedChild);
    assert_eq!(skipped, 1);
}

#[test]
fn discover_snapshot_targets_skips_unverifiable_room() {
    let (tmp, paths) = make_coordinator_fixture(false);
    let ac_root = tmp.path().join("proj-a").join(".ac");
    let real = ac_root.join("wg-1-dev-team");
    let linked = ac_root.join("room-9-linked");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&real, &linked).unwrap();
    #[cfg(windows)]
    create_test_junction(&linked, &real);
    let discovered = discover_verified_terminal_snapshot_targets_counted(&paths)
        .expect("scan must not abort on a linked room");
    assert_eq!(discovered.targets.len(), 2);
    assert_eq!(discovered.skips.rooms, 1);
    assert_eq!(discovered.skips.replicas, 0);
    assert_eq!(discovered.skips.project_children, 0);
}

#[test]
fn discover_snapshot_targets_skips_unverifiable_replica() {
    let (tmp, paths) = make_coordinator_fixture(false);
    let ghost = tmp
        .path()
        .join("proj-a")
        .join(".ac")
        .join("wg-1-dev-team")
        .join("__agent_ghost");
    std::fs::create_dir_all(&ghost).unwrap();
    let discovered = discover_verified_terminal_snapshot_targets_counted(&paths)
        .expect("scan must not abort on a broken replica");
    assert_eq!(discovered.targets.len(), 2);
    assert_eq!(discovered.skips.replicas, 1);
    assert_eq!(discovered.skips.rooms, 0);
}

/// Negative control: the fail-soft change must not swallow a collision
/// between two candidates that both verify.
#[test]
fn discover_snapshot_targets_still_fails_hard_on_ambiguity() {
    let (tmp, paths) = make_coordinator_fixture(false);
    let clone = tmp.path().join("copy").join("proj-a");
    let source = tmp.path().join("proj-a");
    copy_tree(&source, &clone);
    let mut both = paths;
    both.push(tmp.path().join("copy").to_string_lossy().to_string());
    match discover_verified_terminal_snapshot_targets_counted(&both) {
        Err(error) => assert_eq!(error, "ambiguous_project"),
        Ok(_) => panic!("two verifiable copies must still collide"),
    }
}

/// Pins the signature that keeps `phone/mailbox.rs` out of this phase's
/// file set: the wrapper returns exactly `counted().targets`.
#[test]
fn discover_verified_terminal_snapshot_targets_wrapper_matches_counted() {
    let (tmp, paths) = make_coordinator_fixture(false);
    let ghost = tmp
        .path()
        .join("proj-a")
        .join(".ac")
        .join("wg-1-dev-team")
        .join("__agent_ghost");
    std::fs::create_dir_all(&ghost).unwrap();
    let counted = discover_verified_terminal_snapshot_targets_counted(&paths).unwrap();
    assert!(counted.skips.any(), "fixture must contain a skip");
    let wrapped = discover_verified_terminal_snapshot_targets(&paths).unwrap();
    assert_eq!(
        wrapped.iter().map(|t| &t.canonical_fqn).collect::<Vec<_>>(),
        counted
            .targets
            .iter()
            .map(|t| &t.canonical_fqn)
            .collect::<Vec<_>>()
    );
}

fn copy_tree(source: &Path, destination: &Path) {
    std::fs::create_dir_all(destination).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[cfg(unix)]
#[test]
fn find_target_identity_resolves_when_the_colliding_copy_is_skipped() {
    if running_as_root() {
        return;
    }
    let (tmp, _paths) = make_coordinator_fixture(false);
    let project = tmp.path().join("proj-a");
    let shadow = make_unopenable_child(&project, "proj-a");
    let configured = vec![project.to_string_lossy().to_string()];
    let result = resolve_pty_input_target("proj-a:wg-1-dev-team/dev-rust", &configured);
    restore_child_mode(&shadow);
    let identity = result.expect("resolution must survive a skipped colliding copy");
    assert_eq!(identity.canonical_fqn, "proj-a:wg-1-dev-team/dev-rust");
    // The surviving candidate must be the good copy, not the shadow.
    assert_eq!(
        identity.project_identity.canonical_path,
        std::fs::canonicalize(&project).unwrap()
    );
}
