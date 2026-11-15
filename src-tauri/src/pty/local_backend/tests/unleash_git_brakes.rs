use super::*;

// A temp replica tree `<tmp>/.ac/room-1-t/__agent_a` (no PTY, no Tauri app).
fn replica_tree() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let replica = dir.path().join(".ac").join("room-1-t").join("__agent_a");
    std::fs::create_dir_all(&replica).unwrap();
    let replica = replica.to_string_lossy().into_owned();
    (dir, replica)
}

#[test]
fn leashed_replica_gets_git_ceiling() {
    let (_dir, replica) = replica_tree();
    let mut command = portable_pty::CommandBuilder::new("x");
    #[cfg(windows)]
    let git_guard = GitGuardTempSequence::new();
    apply_git_brakes(
        &mut command,
        &replica,
        false,
        #[cfg(windows)]
        &git_guard,
    )
    .unwrap();
    assert!(command.get_env("GIT_CEILING_DIRECTORIES").is_some());
}

#[test]
fn unleashed_replica_gets_no_git_brakes() {
    let (_dir, replica) = replica_tree();
    let mut command = portable_pty::CommandBuilder::new("x");
    // Also covers the values inherited inside an AC PTY.
    command.env("GIT_CEILING_DIRECTORIES", "/inherited");
    command.env("AC_REAL_GIT", "/inherited/git");
    #[cfg(windows)]
    let git_guard = GitGuardTempSequence::new();
    apply_git_brakes(
        &mut command,
        &replica,
        true,
        #[cfg(windows)]
        &git_guard,
    )
    .unwrap();
    assert!(command.get_env("GIT_CEILING_DIRECTORIES").is_none());
    assert!(command.get_env("AC_REAL_GIT").is_none());
}

#[cfg(windows)]
#[test]
fn leashed_replica_gets_git_guard_when_git_exists() {
    if resolve_real_git_path().is_none() {
        return;
    }
    let (_dir, replica) = replica_tree();
    let mut command = portable_pty::CommandBuilder::new("x");
    let git_guard = GitGuardTempSequence::new();
    apply_git_brakes(&mut command, &replica, false, &git_guard).unwrap();
    assert!(command.get_env("AC_REAL_GIT").is_some());
    let guard_dir = ensure_git_guard_wrapper(&git_guard).unwrap();
    let path = command
        .get_env("PATH")
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(path.starts_with(&guard_dir.to_string_lossy().into_owned()));
}
