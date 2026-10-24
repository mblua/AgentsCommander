use super::*;

#[test]
fn current_process_is_alive() {
    // On Windows, OpenProcess against our own PID succeeds and
    // GetExitCodeProcess returns STILL_ACTIVE. On Unix, `kill(pid, 0)`
    // against our own PID succeeds.
    assert!(is_pid_alive(std::process::id()));
}

#[test]
fn extremely_high_pid_is_dead() {
    // A 32-bit max-value pid is essentially guaranteed not to be a live
    // process on Windows. On Unix, `u32::MAX` fails `pid_t::try_from`, so
    // it is dead without ever calling `kill(-1, 0)`.
    assert!(!is_pid_alive(u32::MAX));
}

#[test]
fn missing_pid_file_yields_no_pid_file() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("daemon.pid");
    // Do NOT create the file.
    assert_eq!(detect_daemon_state_at(&path), DaemonState::NoPidFile);
}

#[test]
fn malformed_pid_file_detected_against_tempdir() {
    // This test MUST NOT write to the live binary's
    // <config_dir>/daemon.pid (`pid_path()`). Use a TempDir and the
    // path-parameterized `detect_daemon_state_at` instead.
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("daemon.pid");
    std::fs::write(&path, "not-a-pid").unwrap();
    assert_eq!(detect_daemon_state_at(&path), DaemonState::MalformedPidFile);
}

#[test]
fn living_pid_file_yields_running_against_tempdir() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("daemon.pid");
    std::fs::write(&path, std::process::id().to_string()).unwrap();
    match detect_daemon_state_at(&path) {
        DaemonState::Running { pid } => assert_eq!(pid, std::process::id()),
        other => panic!("expected Running, got {:?}", other),
    }
}

#[test]
fn dead_pid_file_yields_stale_against_tempdir() {
    // `u32::MAX` is dead on every platform; see `extremely_high_pid_is_dead`.
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("daemon.pid");
    std::fs::write(&path, u32::MAX.to_string()).unwrap();
    match detect_daemon_state_at(&path) {
        DaemonState::StalePidFile { pid } => assert_eq!(pid, u32::MAX),
        other => panic!("expected StalePidFile, got {:?}", other),
    }
}

#[cfg(unix)]
#[test]
fn reaped_child_pid_file_yields_stale_against_tempdir() {
    // Positive control with a real, once-valid pid: spawn a child, reap it,
    // then its pid must read as stale.
    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg("exit 0")
        .spawn()
        .expect("spawn sh");
    let pid = child.id();
    child.wait().expect("reap child");
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("daemon.pid");
    std::fs::write(&path, pid.to_string()).unwrap();
    match detect_daemon_state_at(&path) {
        DaemonState::StalePidFile { pid: p } => assert_eq!(p, pid),
        other => panic!("expected StalePidFile, got {:?}", other),
    }
}

#[cfg(unix)]
#[test]
fn zero_pid_file_yields_stale_against_tempdir() {
    // pid 0 would signal our own process group; the probe treats it as dead.
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("daemon.pid");
    std::fs::write(&path, "0").unwrap();
    assert_eq!(
        detect_daemon_state_at(&path),
        DaemonState::StalePidFile { pid: 0 }
    );
}
