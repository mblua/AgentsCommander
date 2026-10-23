use super::resolve_agent_target::make_coordinator_fixture;
use super::resolve_agent_target::make_portable_coordinator_fixture;
use super::resolve_agent_target::make_project_fixture;
use super::resolve_agent_target::FixtureRoot;
use super::*;

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
    let b = Path::new(
        "/some/where/proj/.ac/wg-1-test-team/__agent_test-alpha/\
         ../../../../legacy/.ac/_agent_test-alpha",
    );
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
