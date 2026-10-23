use super::*;

// #2682 T2b - the CAS window in `commit_scan_section`, under the Loop lock.

const SCAN_CHILD_DIR_ENV: &str = "AC_2682_SCAN_CHILD_DIR";

const SCAN_CHILD_TEST_FQN: &str =
    "loops::scheduler::tests::issue_2682::issue_2682_scan_commit_child";

const T2B_EXPR: &str = "0 0 1 1 *";

/// The scheduler-shaped writer: one scan commit from a fresh snapshot.
fn t2b_scan_commit(dir: &Path) -> Result<LoopStateWrite, String> {
    let scheduler = LoopScheduler::new();
    let s0_raw = read_raw_loop_state(dir)?;
    let s0_generation = scheduler.loop_generation(dir);
    let state = LoopState {
        last_checked_at: Some(Utc::now()),
        ..LoopState::default()
    };
    scheduler.commit_scan_section(dir, s0_generation, s0_raw.as_deref(), None, &state)
}

fn t2b_update(
    tmp: &tempfile::TempDir,
    ac_root: &Path,
) -> std::thread::JoinHandle<Result<LoopState, String>> {
    let (project, root) = (tmp.path().to_path_buf(), ac_root.to_path_buf());
    std::thread::spawn(move || {
        crate::config::loops::update_loop_files(
            &project,
            &root,
            "daily-sync",
            crate::config::loops::LoopUpdatePatch {
                expr: Some(T2B_EXPR.to_string()),
                ..Default::default()
            },
        )
        .map(|(_, _, state)| state)
    })
}

fn let_it_finish_if_unblocked<T>(handle: &std::thread::JoinHandle<T>) {
    let started = std::time::Instant::now();
    while !handle.is_finished() && started.elapsed() < Duration::from_millis(500) {
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The update's reset state is what is on disk: no lost update.
fn assert_update_survived(dir: &Path, update: LoopState) {
    let on_disk = crate::config::loops::read_loop_state(dir).expect("state on disk");
    assert_eq!(
        (on_disk.last_checked_at, on_disk.next_due_at),
        (update.last_checked_at, update.next_due_at),
        "the scan commit clobbered the update's reset state"
    );
    assert_eq!(
        read_loop_config(dir).expect("config").trigger.expr,
        T2B_EXPR
    );
}

/// #2682 T2b, in-process - the scan commit stops between its CAS compare
/// and its replace while an update runs. With the Loop lock the update
/// waits and lands after; without it the commit clobbers the update.
#[test]
fn issue_2682_t2b_scan_commit_does_not_clobber_an_update() {
    let (tmp, ac_root, dir) = t2b_fixture();
    let armed = crate::config::loops::pause::arm("after_cas_compare", &dir);
    let commit_dir = dir.clone();
    let writer_a = std::thread::spawn(move || t2b_scan_commit(&commit_dir));
    armed
        .reached
        .recv_timeout(Duration::from_secs(10))
        .expect("the scan commit reaches the pause");
    let writer_b = t2b_update(&tmp, &ac_root);
    let_it_finish_if_unblocked(&writer_b);
    armed.release.send(()).expect("release the scan commit");
    assert_eq!(
        writer_a.join().expect("writer A"),
        Ok(LoopStateWrite::Written)
    );
    let update = writer_b.join().expect("writer B").expect("update section");
    assert_update_survived(&dir, update);
}

/// #2682 - the child half of T2b's cross-process leg: a scan commit that
/// pauses (armed by environment) after its CAS compare. A no-op without
/// the child-only environment.
#[test]
fn issue_2682_scan_commit_child() {
    let Some(dir) = std::env::var_os(SCAN_CHILD_DIR_ENV) else {
        return;
    };
    let written = t2b_scan_commit(Path::new(&dir)).expect("child scan commit");
    assert_eq!(written, LoopStateWrite::Written);
    println!("AC_2682_SCAN_CHILD_WRITTEN");
}

/// #2682 T2b, cross-process - the lost update #2682 names: the scan commit
/// runs in a child process, the update in this one.
#[test]
fn issue_2682_t2b_cross_process_scan_commit_does_not_clobber_an_update() {
    use crate::config::loops::pause;
    let (tmp, ac_root, dir) = t2b_fixture();
    let rendezvous = tempfile::tempdir().expect("rendezvous");
    let mut child = std::process::Command::new(std::env::current_exe().expect("test exe"))
        .args([
            "--exact",
            SCAN_CHILD_TEST_FQN,
            "--nocapture",
            "--test-threads=1",
        ])
        .env(SCAN_CHILD_DIR_ENV, &dir)
        .env(pause::PAUSE_DIR_ENV, rendezvous.path())
        .env(pause::PAUSE_STAGE_ENV, "after_cas_compare")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn scan child");
    let ready = rendezvous.path().join(pause::READY_FILE);
    let started = std::time::Instant::now();
    while !ready.exists() {
        if started.elapsed() > Duration::from_secs(60) || child.try_wait().ok().flatten().is_some()
        {
            let _ = child.kill();
            let output = child.wait_with_output().expect("child output");
            panic!(
                "scan child never paused:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let writer_b = t2b_update(&tmp, &ac_root);
    let_it_finish_if_unblocked(&writer_b);
    std::fs::write(rendezvous.path().join(pause::RELEASE_FILE), b"go").expect("release");
    let started = std::time::Instant::now();
    while child.try_wait().expect("poll child").is_none() {
        if started.elapsed() > Duration::from_secs(60) {
            let _ = child.kill();
            panic!("scan child exceeded its 60 s bound");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = child.wait_with_output().expect("child output");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success()
            && stdout.contains(&format!("test {SCAN_CHILD_TEST_FQN} ..."))
            && stdout.contains("test result: ok. 1 passed; 0 failed")
            && stdout.contains("AC_2682_SCAN_CHILD_WRITTEN"),
        "scan child failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let update = writer_b.join().expect("writer B").expect("update section");
    assert_update_survived(&dir, update);
}

// #2698 - a failed state write after delivery must not cause re-delivery.
