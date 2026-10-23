// ---------------------------------------------------------------------------
// Stage E (#1064) cross-process lock-order inversion PARENTS.
//
// Stage E owns the parent side of the deterministic cross-process lock-order
// proof (plan section 10.4 item 22, acceptance items 43/44): it spawns Stage D's
// landed child helpers UNEDITED by their frozen fully-qualified names via
// `current_exe --exact <fqn> --ignored`, drains both output streams
// concurrently, and enforces a finite kill-and-collect watchdog. The matrix
// excludes `cli/team.rs` and `cli/workgroup.rs`, so the child helpers are reused
// as-is and the parents live in this E-owned file. These parents are `#[ignore]`
// (they spawn a real child process and must run under `--test-threads=1`) and
// are registered in `test-debt.allowlist.json`.
//
// The proof: while the child is paused at its project-gate barrier
// (`after_project_before_team` for member, `after_project_acquired` for
// workgroup), an independent process (this parent) contends on the SAME project
// kernel lock and times out, proving the deletion holds the project gate
// cross-process; the dual-lock member path additionally proves the team guard is
// still free (the deletion took project BEFORE team). Releasing the child lets
// its deletion complete through its own team acquisition.

use super::*;
use crate::cli::team::stage_d_lock_order_child::{
    ACTION_VAR, CONTROL_DIR_VAR, MEMBER_ACTION, NONCE_VAR, WORKGROUP_ACTION,
};
use crate::config::seed_manifest::{ProjectSeedManifestGuard, SeedManifestError};
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

static NONCE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_nonce(tag: &str) -> String {
    let n = NONCE_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("e-{tag}-{}-{n}", std::process::id())
}

/// Concurrently read a child output stream to end into a shared buffer so a
/// blocked pipe can never deadlock the parent (plan section 10.4 item 22).
fn drain(
    mut source: impl Read + Send + 'static,
) -> (Arc<Mutex<String>>, std::thread::JoinHandle<()>) {
    let buffer = Arc::new(Mutex::new(String::new()));
    let sink = Arc::clone(&buffer);
    let handle = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = source.read_to_string(&mut text);
        if let Ok(mut guard) = sink.lock() {
            *guard = text;
        }
    });
    (buffer, handle)
}

fn wait_for_file(path: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    path.exists()
}

fn run_cross_process_parent(action: &str, child_fqn: &str, member_probe_team_free: bool) {
    let control = tempfile::tempdir().expect("control tempdir");
    let nonce = unique_nonce(action);
    let exe = std::env::current_exe().expect("current test exe");

    let mut command = std::process::Command::new(&exe);
    command
        .args([
            "--exact",
            child_fqn,
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(ACTION_VAR, action)
        .env(NONCE_VAR, &nonce)
        .env(CONTROL_DIR_VAR, control.path())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn().expect("spawn lock-order child");
    let (stdout_buf, stdout_join) = drain(child.stdout.take().expect("child stdout"));
    let (stderr_buf, stderr_join) = drain(child.stderr.take().expect("child stderr"));

    let reached = control.path().join(format!("reached-{nonce}"));
    let release = control.path().join(format!("release-{nonce}"));
    let fixture_project = control
        .path()
        .join(format!("fixture-{nonce}"))
        .join("Project");
    let fixture_ac = fixture_project.join(".ac");

    let collect = |child: &mut std::process::Child| {
        let _ = child.kill();
        let _ = child.wait();
    };

    if !wait_for_file(&reached, Duration::from_secs(90)) {
        collect(&mut child);
        let _ = stdout_join.join();
        let _ = stderr_join.join();
        panic!(
            "child never reported reaching the project gate; stdout={:?} stderr={:?}",
            stdout_buf.lock().map(|g| g.clone()),
            stderr_buf.lock().map(|g| g.clone())
        );
    }

    // Cross-process proof: an independent process contends on the same project
    // kernel lock and times out (never a lost update).
    let contended = ProjectSeedManifestGuard::acquire_with_timeout(
        &fixture_project,
        Duration::from_millis(400),
    );
    assert!(
        matches!(contended, Err(SeedManifestError::BusyTimeout { .. })),
        "project gate must be held cross-process while the child is paused, got {contended:?}"
    );

    // Dual-lock (member) path: the child paused BEFORE the team guard, so
    // team-config is still free. Acquire and release it, proving project was
    // taken first, before releasing the child to its own team acquisition.
    if member_probe_team_free {
        let team = TeamConfigMutationGuard::acquire_with_timing(
            &fixture_ac,
            TeamConfigLockTiming {
                poll_interval: Duration::from_millis(20),
                timeout: Duration::from_millis(750),
            },
        );
        assert!(
            team.is_ok(),
            "team guard must be free while the child pauses before it, got {team:?}"
        );
        drop(team);
    }

    std::fs::write(&release, b"go").expect("write release marker");

    // Finite kill-and-collect watchdog.
    let deadline = Instant::now() + Duration::from_secs(60);
    let status = loop {
        match child.try_wait().expect("try_wait") {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                break child.wait().expect("wait after kill");
            }
            None => std::thread::sleep(Duration::from_millis(25)),
        }
    };
    let _ = stdout_join.join();
    let _ = stderr_join.join();
    let stdout = stdout_buf.lock().map(|g| g.clone()).unwrap_or_default();
    let stderr = stderr_buf.lock().map(|g| g.clone()).unwrap_or_default();

    assert!(
        status.success(),
        "child test must pass; stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        stdout.contains(&format!("STAGE_D_LOCK_ORDER_REACHED {nonce}")),
        "child must report reaching the gate on stdout; stdout={stdout}"
    );
    assert!(
        stdout.contains(&format!("STAGE_D_LOCK_ORDER_DONE {nonce}")) && stdout.contains("ok=true"),
        "child deletion must complete after release; stdout={stdout}"
    );
}

#[test]
#[ignore = "spawns a child test process; Stage F acceptance / manual (plan 10.4 item 22)"]
fn cli_member_deletion_takes_project_gate_before_team_cross_process() {
    run_cross_process_parent(
        MEMBER_ACTION,
        "cli::team::tests::cli_member_lock_order_inversion_child",
        true,
    );
}

#[test]
#[ignore = "spawns a child test process; Stage F acceptance / manual (plan 10.4 item 22)"]
fn cli_workgroup_deletion_takes_project_gate_only_cross_process() {
    run_cross_process_parent(
        WORKGROUP_ACTION,
        "cli::workgroup::tests::cli_workgroup_lock_order_inversion_child",
        false,
    );
}
