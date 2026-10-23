use super::*;

// #2682 - the cross-process Loop lock.

const LOCK_CHILD_ACTION_ENV: &str = "AC_2682_LOOP_LOCK_CHILD_ACTION";

const LOCK_CHILD_AC_ROOT_ENV: &str = "AC_2682_LOOP_LOCK_CHILD_AC_ROOT";

const LOCK_CHILD_DIR_ENV: &str = "AC_2682_LOOP_LOCK_CHILD_DIR";

const LOCK_CHILD_TEST_FQN: &str =
    "config::loops::tests::issue_2682_lock::issue_2682_loop_lock_child";

const LOCK_CHILD_READY_FILE: &str = "lock-child-ready";

const LOCK_CHILD_RELEASE_FILE: &str = "lock-child-release";

/// Two yearly schedules whose due times can never coincide (T2a).
const EXPR_JANUARY: &str = "0 0 1 1 *";

const EXPR_JULY: &str = "0 0 1 7 *";

fn lock_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = fixture_project();
    let ac_root = tmp.path().join(".ac");
    let config = sample_config();
    let (dir, _) = create_loop_files(tmp.path(), &ac_root, &config).expect("create Loop");
    (tmp, ac_root, dir)
}

fn expr_patch(expr: &str) -> LoopUpdatePatch {
    LoopUpdatePatch {
        expr: Some(expr.to_string()),
        ..LoopUpdatePatch::default()
    }
}

fn wait_until(what: &str, bound: Duration, mut done: impl FnMut() -> bool) {
    let started = std::time::Instant::now();
    while !done() {
        assert!(
            started.elapsed() < bound,
            "{what}: not reached in {bound:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Gives an unlocked writer every chance to run its whole section; a
/// locked one stays blocked on acquire, so this returns after `bound`.
fn let_it_finish_if_unblocked(handle: &std::thread::JoinHandle<impl Send>, bound: Duration) {
    let started = std::time::Instant::now();
    while !handle.is_finished() && started.elapsed() < bound {
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The on-disk pair agrees: `state.next_due_at` is what the on-disk cron
/// yields from that state's own `last_checked_at`.
fn assert_pair_agrees(dir: &Path) -> (LoopConfigToml, LoopState) {
    let config = read_loop_config(dir).expect("config on disk");
    let state = read_loop_state(dir).expect("state on disk");
    let checked = state.last_checked_at.expect("state has last_checked_at");
    assert_eq!(
        state.next_due_at,
        next_due_after(&config.trigger.expr, checked).expect("next due"),
        "config.toml ({}) and state.json disagree",
        config.trigger.expr
    );
    (config, state)
}

fn spawn_lock_child(action: &str, ac_root: &Path, rendezvous: &Path) -> std::process::Child {
    std::process::Command::new(std::env::current_exe().expect("current test exe"))
        .args([
            "--exact",
            LOCK_CHILD_TEST_FQN,
            "--nocapture",
            "--test-threads=1",
        ])
        .env(LOCK_CHILD_ACTION_ENV, action)
        .env(LOCK_CHILD_AC_ROOT_ENV, ac_root)
        .env(LOCK_CHILD_DIR_ENV, rendezvous)
        .env_remove(pause::PAUSE_DIR_ENV)
        .env_remove(pause::PAUSE_STAGE_ENV)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn Loop lock child")
}

fn wait_child(mut child: std::process::Child, bound: Duration) -> String {
    let started = std::time::Instant::now();
    while child.try_wait().expect("poll child").is_none() {
        if started.elapsed() >= bound {
            let _ = child.kill();
            let _ = child.wait();
            panic!("Loop lock child exceeded its {bound:?} bound and was killed");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = child.wait_with_output().expect("child output");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success()
            && stdout.contains(&format!("test {LOCK_CHILD_TEST_FQN} ..."))
            && stdout.contains("test result: ok. 1 passed; 0 failed"),
        "Loop lock child failed: {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status
    );
    stdout
}

/// #2682 - the child half of T1. A no-op without the child-only action
/// environment, so an ordinary suite run passes it untouched.
#[test]
fn issue_2682_loop_lock_child() {
    let Some(action) = std::env::var_os(LOCK_CHILD_ACTION_ENV) else {
        return;
    };
    let ac_root = PathBuf::from(std::env::var_os(LOCK_CHILD_AC_ROOT_ENV).expect("ac root"));
    let rendezvous = PathBuf::from(std::env::var_os(LOCK_CHILD_DIR_ENV).expect("dir"));
    match action.to_string_lossy().as_ref() {
        "hold" => {
            let _lock = acquire_loop_lock(&ac_root, "weekday-standup", LOOP_LOCK_TIMEOUT)
                .expect("child acquires the Loop lock");
            std::fs::write(rendezvous.join(LOCK_CHILD_READY_FILE), b"ready").expect("ready");
            wait_until("child release", Duration::from_secs(60), || {
                rendezvous.join(LOCK_CHILD_RELEASE_FILE).exists()
            });
        }
        other => panic!("unknown child action {other}"),
    }
    println!("AC_2682_LOOP_LOCK_CHILD_DONE");
}

/// #2682 T0 - two handles on one sidecar in one process exclude each
/// other. The in-process legs of T2a/T2b/T7 and the no-self-lock rule for
/// the leaf writers rest on this.
#[test]
fn issue_2682_t0_same_process_handles_exclude() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let sidecar = tmp.path().join("._loop_x.lock");
    let open = || {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&sidecar)
            .expect("open sidecar")
    };
    let first = open();
    let second = open();
    first.try_lock().expect("first handle locks");
    assert!(
        matches!(second.try_lock(), Err(std::fs::TryLockError::WouldBlock)),
        "a second handle in the same process must see WouldBlock"
    );
    drop(first);
    second
        .try_lock()
        .expect("the lock frees when the first handle closes");
}

/// #2682 T1 - a child process holds the Loop lock; the parent times out
/// with `loopLockTimeout` naming the sidecar, then acquires once released
/// (the positive control: a sidecar that never opens also times out).
#[test]
fn issue_2682_t1_lock_excludes_another_process() {
    let (_tmp, ac_root, _dir) = lock_fixture();
    let rendezvous = tempfile::tempdir().expect("rendezvous");
    let child = spawn_lock_child("hold", &ac_root, rendezvous.path());
    let ready = rendezvous.path().join(LOCK_CHILD_READY_FILE);
    wait_until("child ready", Duration::from_secs(60), || ready.exists());

    let err = acquire_loop_lock(&ac_root, "weekday-standup", Duration::from_millis(200))
        .expect_err("a held Loop lock must time out");
    assert!(err.contains("loopLockTimeout"), "{err}");
    assert!(err.contains("._loop_weekday-standup.lock"), "{err}");
    assert!(err.contains("waiting for Loop write lock"), "{err}");
    assert!(!err.contains("local config"), "{err}");

    std::fs::write(rendezvous.path().join(LOCK_CHILD_RELEASE_FILE), b"go").expect("release");
    let stdout = wait_child(child, Duration::from_secs(60));
    assert!(stdout.contains("AC_2682_LOOP_LOCK_CHILD_DONE"), "{stdout}");
    acquire_loop_lock(&ac_root, "weekday-standup", Duration::from_secs(5))
        .expect("positive control: acquires after the child releases");
}

/// #2682 T2a - two update sections with different crons. Writer A stops
/// between its config and state writes while writer B runs. With the lock
/// B waits, so the on-disk pair agrees; without it B's config sits next to
/// A's state and the pair disagrees.
#[test]
fn issue_2682_t2a_update_sections_keep_config_and_state_agreeing() {
    let (tmp, ac_root, dir) = lock_fixture();
    let armed = pause::arm("between_config_and_state", &dir);
    let (project, root) = (tmp.path().to_path_buf(), ac_root.clone());
    let writer_a = std::thread::spawn(move || {
        update_loop_files(&project, &root, "weekday-standup", expr_patch(EXPR_JANUARY))
    });
    armed
        .reached
        .recv_timeout(Duration::from_secs(10))
        .expect("writer A reaches the pause");
    let (project, root) = (tmp.path().to_path_buf(), ac_root.clone());
    let writer_b = std::thread::spawn(move || {
        update_loop_files(&project, &root, "weekday-standup", expr_patch(EXPR_JULY))
    });
    let_it_finish_if_unblocked(&writer_b, Duration::from_millis(500));
    armed.release.send(()).expect("release writer A");
    writer_a
        .join()
        .expect("writer A")
        .expect("writer A section");
    writer_b
        .join()
        .expect("writer B")
        .expect("writer B section");

    let (config, _) = assert_pair_agrees(&dir);
    assert_eq!(config.trigger.expr, EXPR_JULY, "B's section runs after A's");
}

/// #2682 T3 - the sidecar lives outside the Loop directory, so removing the
/// Loop leaves it in place, and a re-created Loop reuses it.
#[test]
fn issue_2682_t3_sidecar_survives_loop_removal() {
    let (tmp, ac_root, dir) = lock_fixture();
    let sidecar = std::fs::canonicalize(&ac_root)
        .expect("canonical root")
        .join("._loop_weekday-standup.lock");
    drop(acquire_loop_lock(&ac_root, "weekday-standup", LOOP_LOCK_TIMEOUT).expect("lock"));
    remove_loop_files(&ac_root, "weekday-standup").expect("remove Loop");
    assert!(!dir.exists(), "Loop directory is gone");
    assert!(sidecar.is_file(), "sidecar survives the removal");
    let held = acquire_loop_lock(&ac_root, "weekday-standup", LOOP_LOCK_TIMEOUT)
        .expect("sidecar still acquirable");
    drop(held);

    create_loop_files(tmp.path(), &ac_root, &sample_config()).expect("re-create Loop");
    let _held = acquire_loop_lock(&ac_root, "weekday-standup", LOOP_LOCK_TIMEOUT)
        .expect("re-created Loop lock");
    let lock_files: Vec<_> = std::fs::read_dir(&ac_root)
        .expect("read root")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".lock"))
        .collect();
    assert_eq!(lock_files, vec!["._loop_weekday-standup.lock".to_string()]);
}

/// #2682 T4 - a section holding the Loop lock calls every leaf writer and
/// completes: the leaves never take the lock themselves. Bounded, so a
/// regression fails instead of hanging the job.
#[test]
fn issue_2682_t4_leaf_writers_do_not_self_deadlock() {
    let (tmp, ac_root, dir) = lock_fixture();
    let project = tmp.path().to_string_lossy().to_string();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| -> Result<(), String> {
            let _lock = acquire_loop_lock(&ac_root, "weekday-standup", LOOP_LOCK_TIMEOUT)?;
            write_loop_config(&ac_root, &sample_config())?;
            write_loop_state_atomic(&dir, &LoopState::default())?;
            let now = Utc::now();
            append_loop_audit_once(
                &dir,
                &LoopAuditEntry {
                    run_id: Uuid::new_v4(),
                    loop_id: "weekday-standup".to_string(),
                    project_path: project,
                    kind: LoopAuditKind::Delivered,
                    due_at: now,
                    started_at: now,
                    completed_at: Some(now),
                    target: None,
                    session_id: None,
                    busy_coordinator_policy: BusyCoordinatorPolicy::default(),
                    session_start: None,
                    error: None,
                    prompt_snapshot: None,
                },
            )?;
            Ok(())
        })();
        let _ = done_tx.send(result);
    });
    done_rx
        .recv_timeout(Duration::from_secs(20))
        .expect("the locked section finishes within 20 s")
        .expect("every leaf writer succeeds under the held lock");
}

/// #2682 T5 - raw, dot-segment and (on Windows) case-variant spellings of
/// one AC root resolve to one sidecar and exclude each other.
#[test]
fn issue_2682_t5_root_aliases_share_one_lock() {
    let (tmp, ac_root, _dir) = lock_fixture();
    let mut aliases = vec![ac_root.join("..").join(".ac")];
    if cfg!(windows) {
        aliases.push(PathBuf::from(
            tmp.path().to_string_lossy().to_uppercase() + "\\.AC",
        ));
    }
    let _held =
        acquire_loop_lock(&ac_root, "weekday-standup", LOOP_LOCK_TIMEOUT).expect("raw spelling");
    for alias in aliases {
        let err = acquire_loop_lock(&alias, "weekday-standup", Duration::from_millis(100))
            .expect_err("an alias must hit the held sidecar");
        assert!(
            err.contains("loopLockTimeout"),
            "{}: {err}",
            alias.display()
        );
    }
}

/// #2682 T7 - writer A (an update) stops between config and state while
/// writer B removes the Loop. The only clean outcomes: A's section then B's
/// removal, or B first and A fails "not found" from a check under the lock.
/// Never a failed half-write, never `state.json` without `config.toml`.
#[test]
fn issue_2682_t7_removal_waits_for_an_update_section() {
    let (tmp, ac_root, dir) = lock_fixture();
    let armed = pause::arm("between_config_and_state", &dir);
    let (project, root) = (tmp.path().to_path_buf(), ac_root.clone());
    let writer_a = std::thread::spawn(move || {
        update_loop_files(&project, &root, "weekday-standup", expr_patch(EXPR_JANUARY))
    });
    armed
        .reached
        .recv_timeout(Duration::from_secs(10))
        .expect("writer A reaches the pause");
    let root = ac_root.clone();
    let writer_b = std::thread::spawn(move || remove_loop_files(&root, "weekday-standup"));
    let_it_finish_if_unblocked(&writer_b, Duration::from_millis(500));
    armed.release.send(()).expect("release writer A");
    let a = writer_a.join().expect("writer A");
    let b = writer_b.join().expect("writer B");

    assert!(
        !(dir.join(LOOP_STATE_FILE).exists() && !dir.join(LOOP_CONFIG_FILE).exists()),
        "a Loop directory holds state.json without config.toml"
    );
    match a {
        Ok(_) => {
            b.expect("B removes after A's section");
            assert!(!dir.exists(), "B's removal ran last");
        }
        Err(e) => assert!(e.contains("not found"), "A failed mid-section: {e}"),
    }
}

/// #2682 gate 4 - the pause hook is inert unless armed: no environment
/// arming leaks into an ordinary run, and an unarmed stage returns at once.
#[test]
fn issue_2682_pause_hook_is_disarmed_by_default() {
    assert!(std::env::var_os(pause::PAUSE_DIR_ENV).is_none());
    assert!(std::env::var_os(pause::PAUSE_STAGE_ENV).is_none());
    let tmp = tempfile::tempdir().expect("tempdir");
    let started = std::time::Instant::now();
    loop_write_pause_hook("between_config_and_state", tmp.path());
    loop_write_pause_hook("after_cas_compare", tmp.path());
    assert!(started.elapsed() < Duration::from_secs(1));
}
