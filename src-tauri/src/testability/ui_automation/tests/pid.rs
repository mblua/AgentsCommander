use super::*;

#[test]
fn pid_is_alive_reports_the_current_process() {
    assert!(pid_is_alive(std::process::id()));
}

#[test]
fn pid_is_alive_rejects_zero_and_unrepresentable_pids() {
    // PID 0 names the scheduler, not a targetable process, and `u32::MAX`
    // does not fit a `pid_t`; both must be dead without calling `kill`.
    assert!(!pid_is_alive(0));
    assert!(!pid_is_alive(u32::MAX));
}

#[cfg(unix)]
#[test]
fn reaped_pid_is_not_alive() {
    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg("exit 0")
        .spawn()
        .expect("spawn sh");
    let pid = child.id();
    child.wait().expect("reap child");
    assert!(!pid_is_alive(pid));
}

#[cfg(unix)]
#[test]
fn init_pid_is_alive() {
    // PID 1 always exists; when the test runs unprivileged this also pins
    // the EPERM branch (exists, no permission => alive).
    assert!(pid_is_alive(1));
}

#[cfg(target_os = "linux")]
#[test]
fn zombie_pid_is_not_alive() {
    use std::time::{Duration, Instant};
    if !std::path::Path::new("/proc/self/stat").exists() {
        eprintln!("skip: /proc is not mounted");
        return;
    }
    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg("exit 0")
        .spawn()
        .expect("spawn sh");
    let pid = child.id();
    // Wait until the kernel reports the unreaped child as a zombie. Calling
    // `wait()` here would reap it and make the PID disappear, so poll
    // /proc directly.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let state = std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|stat| {
                stat.rsplit_once(')')
                    .and_then(|(_, rest)| rest.trim_start().chars().next())
            });
        if state == Some('Z') {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "child {pid} never became a zombie"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let zombie_is_alive = pid_is_alive(pid);
    child.wait().expect("reap zombie");
    assert!(!zombie_is_alive, "a zombie must not be reported alive");
}
