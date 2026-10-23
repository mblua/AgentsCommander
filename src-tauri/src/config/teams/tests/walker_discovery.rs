use super::resolve_agent_target::make_coordinator_fixture;
use super::*;

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
