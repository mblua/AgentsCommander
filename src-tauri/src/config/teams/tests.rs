use super::*;

mod identity_paths;
mod resolve_agent_target;
mod walker_discovery;

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

#[test]
fn issue_3349_team_ports_preserve_native_observations() {
    use crate::filesystem::ac_discovery as port;
    use std::collections::BTreeSet;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let config = root.join("config.json");
    let error = |e: std::io::Error| (e.kind(), e.raw_os_error(), e.to_string());
    for text in [
        String::new(),
        " \u{2003}team\n".into(),
        "\u{feff}{}".into(),
        "x".repeat(16_384),
    ] {
        std::fs::write(&config, text.as_bytes()).unwrap();
        assert_eq!(port::read_team_discovery_config(&config).unwrap(), text);
        assert_eq!(
            port::read_team_discovery_config(&config).unwrap(),
            std::fs::read_to_string(&config).unwrap()
        );
    }
    std::fs::write(&config, [0xff]).unwrap();
    for path in [config.clone(), root.to_path_buf(), root.join("missing")] {
        assert_eq!(
            port::read_team_discovery_config(&path).map_err(error),
            std::fs::read_to_string(&path).map_err(error)
        );
    }
    for path in [
        root.to_path_buf(),
        config.clone(),
        root.join("missing"),
        config.join("child"),
    ] {
        assert_eq!(port::discovery_path_is_dir(&path), path.is_dir());
    }
    let entries = root.join("entries");
    std::fs::create_dir(&entries).unwrap();
    std::fs::write(entries.join("équipe-中"), b"fake").unwrap();
    std::fs::create_dir(entries.join("directory")).unwrap();
    let native: BTreeSet<_> = std::fs::read_dir(&entries)
        .unwrap()
        .map(|entry| entry.map(|entry| (entry.path(), entry.file_name())))
        .collect::<std::io::Result<_>>()
        .unwrap();
    let observed: BTreeSet<_> = port::team_discovery_entries(&entries)
        .unwrap()
        .map(|entry| {
            entry.map(|(path, name)| {
                assert_eq!(path.file_name(), Some(name.as_os_str()));
                (path, name)
            })
        })
        .collect::<std::io::Result<_>>()
        .unwrap();
    assert_eq!(observed, native);
    assert_eq!(
        observed.len(),
        2,
        "files and directories must both be returned"
    );
    let empty = root.join("empty");
    std::fs::create_dir(&empty).unwrap();
    assert!(port::team_discovery_entries(&empty)
        .unwrap()
        .next()
        .is_none());
    for path in [config, root.join("missing")] {
        assert_eq!(
            error(
                port::team_discovery_entries(&path)
                    .err()
                    .expect("open must fail")
            ),
            error(
                std::fs::read_dir(&path)
                    .err()
                    .expect("native open must fail")
            )
        );
    }
}

#[test]
fn issue_3349_team_discovery_preserves_filter_and_read_fallbacks() {
    use std::cell::RefCell;
    use std::collections::BTreeSet;
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let ac = project.join(".ac");
    let valid = ac.join("_team_zz_valid");
    let missing = ac.join("_team_a_missing");
    let malformed = ac.join("_team_b_malformed");
    let invalid = ac.join("_team_c_invalid");
    let plain = ac.join("_team_plain");
    for dir in [&valid, &missing, &malformed, &invalid] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(valid.join("config.json"), br#"{"agents":[]}"#).unwrap();
    std::fs::write(malformed.join("config.json"), b"{bad").unwrap();
    std::fs::write(invalid.join("config.json"), [0xff]).unwrap();
    std::fs::write(&plain, b"not a directory").unwrap();
    let visited = RefCell::new(BTreeSet::new());
    let mut discovered = Vec::new();
    discover_teams_in_project_filtered(&project, &mut discovered, |path| {
        visited.borrow_mut().insert(path.to_path_buf());
        true
    });
    assert_eq!(discovered.len(), 1);
    assert_eq!(discovered[0].name, "zz_valid");
    assert_eq!(discovered[0].project, "project");
    assert!(discovered[0].agent_names.is_empty());
    assert!(discovered[0].agent_paths.is_empty());
    assert!(discovered[0].coordinator_name.is_none());
    assert!(discovered[0].coordinator_path.is_none());
    assert_eq!(
        visited.into_inner(),
        BTreeSet::from([valid.clone(), missing, malformed, invalid, plain])
    );
    let mut excluded = Vec::new();
    discover_teams_in_project_filtered(&project, &mut excluded, |path| path != valid.as_path());
    assert!(excluded.is_empty());
}
