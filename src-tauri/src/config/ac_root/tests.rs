use super::*;

#[test]
fn wg_replica_layout_resolves_all_pieces() {
    let temp = tempfile::TempDir::new().unwrap();
    let project = temp.path().join("proj-x");
    let agent_dir = project.join(".ac").join("wg-1-devs").join("__agent_alice");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let canon = std::fs::canonicalize(&agent_dir).unwrap();

    let layout = wg_replica_layout_from_agent_dir(&canon)
        .unwrap()
        .expect("WG replica layout should resolve");

    assert_eq!(layout.agent_name, "alice");
    assert_eq!(layout.wg_name, "wg-1-devs");
    assert_eq!(layout.project_dir, std::fs::canonicalize(&project).unwrap());
    assert_eq!(
        layout.ac_root,
        std::fs::canonicalize(project.join(".ac")).unwrap()
    );
}

#[test]
fn wg_replica_layout_none_for_non_agent_dir() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().join("proj-x").join(".ac").join("wg-1-devs");
    std::fs::create_dir_all(&dir).unwrap();
    // Points at the wg dir, not an __agent_* dir.
    let canon = std::fs::canonicalize(&dir).unwrap();
    assert!(wg_replica_layout_from_agent_dir(&canon).unwrap().is_none());
}

#[test]
fn wg_replica_layout_none_for_non_wg_parent() {
    let temp = tempfile::TempDir::new().unwrap();
    // __agent_* dir whose parent is not a wg-* dir.
    let agent_dir = temp.path().join("proj-x").join(".ac").join("__agent_alice");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let canon = std::fs::canonicalize(&agent_dir).unwrap();
    assert!(wg_replica_layout_from_agent_dir(&canon).unwrap().is_none());
}
