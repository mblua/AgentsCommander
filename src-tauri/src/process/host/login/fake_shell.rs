//! Test-only helper shared by login and agent_path tests (#4608).

use std::path::{Path, PathBuf};

/// Writes an executable `#!/bin/sh` script and returns its path once it can run.
pub(crate) fn fake_shell(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
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
