use super::*;
use std::path::Path;

#[cfg(unix)]
fn try_symlink_dir(target: &Path, link: &Path) -> bool {
    std::os::unix::fs::symlink(target, link).is_ok()
}

#[cfg(windows)]
fn try_symlink_dir(target: &Path, link: &Path) -> bool {
    std::os::windows::fs::symlink_dir(target, link).is_ok()
}

/// Build `<tmp>/.ac/wg-test-team/` with the given replica name, and return
/// (tmp, wg_root, replica_root). `find_ac_root_ancestor` keys on a `.ac`
/// ancestor and the wg-root name must start with `wg-`, so both are created.
fn make_workgroup(replica: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().expect("tmp");
    let wg_root = tmp.path().join(".ac").join("wg-test-team");
    let replica_root = wg_root.join(replica);
    std::fs::create_dir_all(&replica_root).expect("replica dir");
    let wg_root = std::fs::canonicalize(&wg_root).expect("canonical wg root");
    let replica_root = std::fs::canonicalize(&replica_root).expect("canonical replica");
    (tmp, wg_root, replica_root)
}

fn write_repos(replica_root: &Path, repos: &[&str]) {
    let json = serde_json::json!({ "repos": repos });
    std::fs::write(
        replica_root.join("config.json"),
        serde_json::to_string(&json).unwrap(),
    )
    .expect("write config.json");
}

fn mkdir(base: &Path, name: &str) -> PathBuf {
    let path = base.join(name);
    std::fs::create_dir_all(&path).expect("mkdir");
    path
}

#[test]
fn normal_sibling_repo_resolves_to_repos_container_path() {
    let (_tmp, wg_root, replica) = make_workgroup("__agent_self");
    mkdir(&wg_root, "repo-AgentsCommander");
    write_repos(&replica, &["../repo-AgentsCommander"]);

    let resolution = resolve_repo_mounts(&replica).expect("ok");
    assert_eq!(resolution.entries.len(), 1);
    match &resolution.entries[0].outcome {
        RepoOutcome::Mounted {
            name,
            container_path,
            ..
        } => {
            assert_eq!(name, "repo-AgentsCommander");
            assert_eq!(container_path, "/repos/repo-AgentsCommander");
        }
        other => panic!("expected Mounted, got {:?}", other),
    }
    let mounts: Vec<_> = resolution.mounts().collect();
    assert_eq!(mounts.len(), 1);
    assert_eq!(mounts[0].1, "/repos/repo-AgentsCommander");
}

#[test]
fn mounted_host_path_has_no_windows_verbatim_prefix() {
    // #993 - std::fs::canonicalize yields \\?\C:\... on Windows and dockerd
    // rejects that as a --mount source. make_workgroup canonicalizes the
    // tempdir, so on Windows this fails without the normalize at the store.
    let (_tmp, wg_root, replica) = make_workgroup("__agent_self");
    mkdir(&wg_root, "repo-AgentsCommander");
    write_repos(&replica, &["../repo-AgentsCommander"]);

    let resolution = resolve_repo_mounts(&replica).expect("ok");
    let mounts: Vec<_> = resolution.mounts().collect();
    assert_eq!(mounts.len(), 1);
    let host = mounts[0].0.to_string_lossy();
    assert!(
        !host.starts_with(r"\\?\"),
        "mount source must not carry the verbatim prefix: {host}"
    );
}

#[test]
fn symlink_named_repo_but_targeting_sibling_replica_is_rejected() {
    // grinch Finding 2, THE critical test. A symlink whose APPARENT name
    // starts with `repo-` but whose CANONICAL target is a sibling replica.
    // The parent check PASSES (the target's parent IS the wg root), so only
    // the canonical-NAME check can reject it. Nothing else does:
    // container_mount_source_rejection does not catch a sibling replica.
    let (_tmp, wg_root, replica) = make_workgroup("__agent_self");
    let other = mkdir(&wg_root, "__agent_other");
    let trap = replica.join("repo-trap");
    if !try_symlink_dir(&other, &trap) {
        eprintln!("skipping: no symlink privilege on this platform");
        return;
    }
    write_repos(&replica, &["./repo-trap"]);

    let result = resolve_repo_mounts(&replica);
    assert!(
        result.is_err(),
        "a repo-*-named symlink to a sibling replica must hard-fail, got {:?}",
        result
    );
}

#[test]
fn symlink_named_repo_but_targeting_messaging_is_rejected() {
    // The twin: same shape, canonical target is an AC-owned NON-replica.
    // Parent check passes again; canonical name `messaging` has no `repo-`
    // prefix -> Err. Covers the other class of AC-owned target.
    let (_tmp, wg_root, replica) = make_workgroup("__agent_self");
    let messaging = mkdir(&wg_root, "messaging");
    let trap = replica.join("repo-trap");
    if !try_symlink_dir(&messaging, &trap) {
        eprintln!("skipping: no symlink privilege on this platform");
        return;
    }
    write_repos(&replica, &["./repo-trap"]);

    assert!(
        resolve_repo_mounts(&replica).is_err(),
        "a repo-*-named symlink to messaging/ must hard-fail"
    );
}

#[test]
fn plain_symlink_to_messaging_is_rejected_before_the_name_check() {
    // Apparent name NOT `repo-`-prefixed: proves canonicalize runs before
    // validation. Note this does NOT cover the two tests above: `messaging`
    // fails the parent check on its own and never exercises the name rule.
    let (_tmp, wg_root, replica) = make_workgroup("__agent_self");
    let messaging = mkdir(&wg_root, "messaging");
    let link = replica.join("plain-link");
    if !try_symlink_dir(&messaging, &link) {
        eprintln!("skipping: no symlink privilege on this platform");
        return;
    }
    write_repos(&replica, &["./plain-link"]);

    assert!(resolve_repo_mounts(&replica).is_err());
}

#[test]
fn messaging_sibling_is_rejected() {
    let (_tmp, wg_root, replica) = make_workgroup("__agent_self");
    mkdir(&wg_root, "messaging");
    write_repos(&replica, &["../messaging"]);
    assert!(resolve_repo_mounts(&replica).is_err());
}

#[test]
fn other_agent_replica_is_rejected() {
    let (_tmp, wg_root, replica) = make_workgroup("__agent_self");
    mkdir(&wg_root, "__agent_other");
    write_repos(&replica, &["../__agent_other"]);
    assert!(resolve_repo_mounts(&replica).is_err());
}

#[test]
fn own_replica_is_rejected() {
    let (_tmp, _wg_root, replica) = make_workgroup("__agent_self");
    write_repos(&replica, &["../__agent_self"]);
    assert!(resolve_repo_mounts(&replica).is_err());
}

#[test]
fn workgroup_root_itself_is_rejected() {
    let (_tmp, _wg_root, replica) = make_workgroup("__agent_self");
    write_repos(&replica, &[".."]);
    assert!(resolve_repo_mounts(&replica).is_err());
}

#[test]
fn path_at_or_above_ac_root_is_rejected() {
    let (_tmp, _wg_root, replica) = make_workgroup("__agent_self");
    write_repos(&replica, &["../../.."]);
    assert!(resolve_repo_mounts(&replica).is_err());
}

#[test]
fn absolute_path_to_real_dir_elsewhere_is_rejected() {
    let (_tmp, _wg_root, replica) = make_workgroup("__agent_self");
    let elsewhere = tempfile::tempdir().expect("elsewhere");
    let elsewhere_repo = mkdir(elsewhere.path(), "repo-elsewhere");
    write_repos(&replica, &[elsewhere_repo.to_str().unwrap()]);
    assert!(
        resolve_repo_mounts(&replica).is_err(),
        "Q4: a repo outside the workgroup root is prohibited (post-collapse: hard-fail)"
    );
}

#[test]
fn direct_wg_child_without_repo_prefix_is_rejected() {
    let (_tmp, wg_root, replica) = make_workgroup("__agent_self");
    mkdir(&wg_root, "not-repo-prefixed");
    write_repos(&replica, &["../not-repo-prefixed"]);
    assert!(resolve_repo_mounts(&replica).is_err());
}

#[test]
fn missing_dir_is_not_found_and_still_launches() {
    let (_tmp, _wg_root, replica) = make_workgroup("__agent_self");
    write_repos(&replica, &["../repo-missing"]);
    let resolution = resolve_repo_mounts(&replica).expect("NOT FOUND must not hard-fail");
    assert_eq!(resolution.entries.len(), 1);
    assert_eq!(resolution.entries[0].outcome, RepoOutcome::NotFound);
    assert_eq!(resolution.mounts().count(), 0);
}

#[test]
fn a_plain_file_named_repo_is_rejected() {
    let (_tmp, wg_root, replica) = make_workgroup("__agent_self");
    std::fs::write(wg_root.join("repo-file"), b"not a dir").expect("write file");
    write_repos(&replica, &["../repo-file"]);
    assert!(
        resolve_repo_mounts(&replica).is_err(),
        "an existing non-directory is inadmissible"
    );
}

#[test]
fn two_agents_with_different_repos_resolve_to_different_mount_sets() {
    // Per-agent enablement (plan Sec 2): each replica's own repos[] drives its
    // own mount set.
    let (_tmp, wg_root, replica_one) = make_workgroup("__agent_one");
    let replica_two = wg_root.join("__agent_two");
    std::fs::create_dir_all(&replica_two).expect("agent two");
    let replica_two = std::fs::canonicalize(&replica_two).expect("canonical two");
    mkdir(&wg_root, "repo-AgentsCommander");
    mkdir(&wg_root, "repo-webpage");
    write_repos(&replica_one, &["../repo-AgentsCommander"]);
    write_repos(&replica_two, &["../repo-webpage"]);

    let one: Vec<_> = resolve_repo_mounts(&replica_one)
        .expect("ok")
        .mounts()
        .map(|(_, container)| container.to_string())
        .collect();
    let two: Vec<_> = resolve_repo_mounts(&replica_two)
        .expect("ok")
        .mounts()
        .map(|(_, container)| container.to_string())
        .collect();
    assert_eq!(one, vec!["/repos/repo-AgentsCommander".to_string()]);
    assert_eq!(two, vec!["/repos/repo-webpage".to_string()]);
    assert_ne!(one, two);
}

#[test]
fn no_config_json_yields_no_mounts() {
    let (_tmp, _wg_root, replica) = make_workgroup("__agent_self");
    // No config.json written.
    let resolution = resolve_repo_mounts(&replica).expect("ok");
    assert!(resolution.entries.is_empty());
}

#[test]
fn not_a_workgroup_replica_yields_no_mounts() {
    // A replica whose parent is not a `wg-` workspace child: no repo mounts,
    // full stop (no error).
    let tmp = tempfile::tempdir().expect("tmp");
    let replica = tmp.path().join("some-dir");
    std::fs::create_dir_all(&replica).expect("dir");
    let replica = std::fs::canonicalize(&replica).expect("canonical");
    write_repos(&replica, &["../repo-x"]);
    let resolution = resolve_repo_mounts(&replica).expect("ok");
    assert!(resolution.entries.is_empty());
}

#[test]
fn issue_3334_repo_config_preserves_raw_reads_and_empty_fallbacks() {
    let (_root, _room, replica) = make_workgroup("__agent_self");
    let path = replica.join("config.json");
    for bytes in [b"{bad".as_slice(), &[0xff], b"{\"repos\":false}", b""] {
        std::fs::write(&path, bytes).unwrap();
        let native = std::fs::read_to_string(&path);
        let port = documents::read_container_repo_config(&path);
        match (native, port) {
            (Ok(native), Ok(port)) => assert_eq!(native, port),
            (Err(native), Err(port)) => assert_eq!(
                (native.kind(), native.raw_os_error(), native.to_string()),
                (port.kind(), port.raw_os_error(), port.to_string())
            ),
            results => panic!("native/port mismatch: {results:?}"),
        }
        assert!(read_repos_array(&replica).is_empty());
        assert!(resolve_repo_mounts(&replica).unwrap().entries.is_empty());
    }
    std::fs::remove_file(&path).unwrap();
    assert!(read_repos_array(&replica).is_empty());
    let native = std::fs::read_to_string(&path).unwrap_err();
    let port = documents::read_container_repo_config(&path).unwrap_err();
    assert_eq!(native.kind(), port.kind());
    assert_eq!(native.raw_os_error(), port.raw_os_error());
    assert_eq!(native.to_string(), port.to_string());
}

#[test]
fn issue_3334_repo_parent_canonicalization_preserves_results() {
    let (root, room, replica) = make_workgroup("__agent_self");
    assert_eq!(
        directories::canonical_container_mount(replica.parent().unwrap()).unwrap(),
        std::fs::canonicalize(&room).unwrap()
    );
    let missing_parent = root.path().join("absent");
    let native = std::fs::canonicalize(&missing_parent).unwrap_err();
    let port = directories::canonical_container_mount(&missing_parent).unwrap_err();
    assert_eq!(native.kind(), port.kind());
    assert_eq!(native.raw_os_error(), port.raw_os_error());
    assert_eq!(native.to_string(), port.to_string());
    assert!(resolve_repo_mounts(&missing_parent.join("replica"))
        .unwrap()
        .entries
        .is_empty());
}
