#[cfg(unix)]
use super::*;

#[cfg(unix)]
use std::path::PathBuf;

#[cfg(unix)]
fn fake_shell(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    // A parallel test's forked child can hold this file's write fd until its exec
    // (ETXTBSY); wait until it is executable.
    for attempt in 1..=50 {
        let spawned = std::process::Command::new(&path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        match spawned {
            Ok(mut child) => {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
            Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) && attempt < 50 => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(e) => panic!("fake shell {} is not executable: {e}", path.display()),
        }
    }
    path
}

/// Fallback branch of plan 6.3.T10: the spawn succeeding through the real
/// builder proves `setsid` did not fail, which a re-introduced process group
/// setting would make it do (EPERM, propagated as a spawn error).
#[cfg(unix)]
#[tokio::test]
async fn agent_path_2589_login_shell_probe_child_is_a_session_leader() {
    let dir = tempfile::tempdir().unwrap();
    let shell = fake_shell(
        dir.path(),
        "fake-shell",
        "printf '__AC_PATH_BEGIN__/session/leader__AC_PATH_END__'",
    );
    let child = build_login_shell_probe_command(&shell)
        .spawn()
        .expect("setsid must succeed, so the spawn must succeed");
    let output = child.wait_with_output().await.expect("probe output");
    assert!(output.status.success());
    assert_eq!(
        parse_sentinel_path(&String::from_utf8_lossy(&output.stdout)),
        Some("/session/leader")
    );
}
