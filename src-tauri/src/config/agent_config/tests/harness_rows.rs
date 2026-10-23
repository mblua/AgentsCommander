use super::*;

// ---- #2786 C1 r28: harness child-ownership rows ----
// These are NOT mutant classes of E15. They prove the harness itself never
// leaves a child behind when supervision fails, and they drive a BLOCKING
// inner row, so the child is certainly alive when the injected fault fires.

/// Blocks until the harness terminates it. It is armed, but it loads
/// nothing, so it opens no observation and records nothing.
#[test]
#[ignore = "#2786 C1: driven only by the harness child-ownership rows"]
fn inner_sleeps_until_killed() {
    println!("SLEEPER-READY");
    std::thread::sleep(std::time::Duration::from_secs(120));
}

#[test]
fn an_injected_wait_error_still_terminates_and_reaps_the_child() {
    use load_probe_harness::{run_inner_with_fault, Fault, SLEEPER};
    let r = run_inner_with_fault(SLEEPER, Fault::WaitError);
    let reported = r
        .wait_error
        .as_deref()
        .expect("the injected wait error must be reported, not discarded");
    assert!(reported.contains("injected try_wait failure"), "{reported}");
    let c = r
        .cleanup
        .as_ref()
        .expect("an errored wait must hand back a cleanup report");
    assert!(
        c.reaped_ok(),
        "the child was not reaped after the wait error: {}",
        c.describe()
    );
    assert!(
        c.kill_error.is_none(),
        "terminating the child failed, and the harness says so: {}",
        c.describe()
    );
    assert!(!r.ok, "a supervision failure is never a green child");
}

#[test]
fn a_timed_out_child_is_terminated_and_reaped() {
    use load_probe_harness::{run_inner_with_fault, Fault, SLEEPER};
    let r = run_inner_with_fault(SLEEPER, Fault::Timeout);
    assert!(r.timed_out, "the past deadline must time the child out");
    assert!(r.wait_error.is_none(), "{:?}", r.wait_error);
    let c = r
        .cleanup
        .as_ref()
        .expect("a timeout must hand back a cleanup report");
    assert!(
        c.reaped_ok(),
        "the child was not reaped after the timeout: {}",
        c.describe()
    );
    // r29 item 3: this row checks the KILL error report too, on its own,
    // exactly as the wait-error row does. Without it, a harness that
    // reported a failed termination still passed this row in silence. The
    // check is deliberately NOT in the shared `assert_one_test_ran`: two
    // redundant guards hide each other from mutation.
    assert!(
        c.kill_error.is_none(),
        "terminating the timed-out child reported an error: {}",
        c.describe()
    );
    assert!(!r.ok, "a timed-out child is never a green child");
}

// ---- #2786 C1 r30 item 5: DIRECT controls on the failed-kill branch ----
// Each one drives the blocking sleeper, injects its fault, and asserts the
// OUTCOME, the BOUND, the row's RED verdict and the pid's state before and
// after cleanup. The report-injection control k1 is kept as it was: it
// proves the REPORTING, never this branch.

/// The shared part: assert that `assert_one_test_ran`, the very assertion
/// every observation row uses, rejects this run. That is what "the row is
/// RED" means; nothing weaker and no new assertion.
fn assert_the_shared_gate_rejects(r: &load_probe_harness::ChildRun, needle: &str) {
    use load_probe_harness::assert_one_test_ran;
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let caught = std::panic::catch_unwind(|| {
        assert_one_test_ran(r, load_probe_harness::SLEEPER);
    });
    std::panic::set_hook(hook);
    let err = caught.expect_err("the shared gate must reject this run");
    let msg = err.downcast_ref::<String>().cloned().unwrap_or_else(|| {
        err.downcast_ref::<&str>()
            .map(|s| s.to_string())
            .unwrap_or_default()
    });
    assert!(
        msg.contains(needle),
        "the shared gate rejected it for the wrong reason, no {needle:?} in: {msg}"
    );
    println!("ROW-RED shared gate rejected the run: {msg}");
}

/// r31 F4: the cleanup guard, adopted as the FIRST statement of a control
/// row, before any fallible assertion.
///
/// r30's Pending and Failed rows ran every assertion before they killed
/// anything, so the dev's 6 s mutant panicked with the 120 s sleeper still
/// running and nothing left to clean it up. This guard takes the still-
/// owned `Child` out of the report at the top of the row. From then on it
/// owns the process on EVERY exit path: the normal one, where the row
/// calls `prove_alive_then_reap`, and the panic one, where `Drop` kills
/// and reaps it inside a bounded poll.
///
/// It owns a real `Child`, not a pid. A pid can only be killed, and on a
/// Unix host killing this process's own child without waiting leaves a
/// zombie that `ps` still reports. `wait()` on the handle is the portable
/// reap, on both platforms, with no external command and no dependency.
struct AdoptedChild {
    pid: u32,
    child: Option<std::process::Child>,
}

impl AdoptedChild {
    fn adopt(r: &mut load_probe_harness::ChildRun) -> AdoptedChild {
        match r.cleanup.as_mut() {
            Some(c) => AdoptedChild {
                pid: c.pid,
                child: c.unreaped.take(),
            },
            None => AdoptedChild {
                pid: 0,
                child: None,
            },
        }
    }

    /// Prove the pid is ALIVE, prove the detector can also report a query
    /// it could not answer, then reap through the owned handle and prove
    /// the pid is gone. All four legs, every time, bounded.
    ///
    /// Why this is not the shell teardown artefact the r29 evidence hit:
    /// the `Alive` reading is taken here, inside the test process that
    /// spawned the child and which is still running, before anything is
    /// torn down. No command substitution, no wrapper shell, no job object.
    fn prove_alive_then_reap(&mut self) {
        use load_probe_harness::{bounded_kill_and_reap, pid_state, pid_state_by_filter};
        use load_probe_harness::{PidState, BAD_PID_SELECTOR};
        let pid = self.pid;
        let mut child = self
            .child
            .take()
            .expect("#2786 C1 r31 F4: the guard must own the un-reaped child");
        let before = pid_state(pid);
        assert_eq!(
            before,
            PidState::Alive,
            "the injected failed kill must leave pid {pid} running; \
             the detector said                  \
             {before:?}"
        );
        println!("PID-BEFORE-CLEANUP pid={pid} state={before:?} (the harness did NOT reap it)");
        let bad = pid_state_by_filter(BAD_PID_SELECTOR);
        assert!(
            matches!(bad, PidState::QueryError(_)),
            "a malformed query must be a query error, not a verdict: {bad:?}"
        );
        println!("PID-QUERY-ERROR-CONTROL {bad:?}");
        let report = bounded_kill_and_reap(&mut child, pid, std::time::Duration::from_secs(10));
        println!("OWNED-REAP {report}");
        assert!(
            report.contains("REAPED status"),
            "the owned handle must really reap the child: {report}"
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut after = pid_state(pid);
        while after == PidState::Alive && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
            after = pid_state(pid);
        }
        assert_eq!(
            after,
            PidState::Dead,
            "pid {pid} survived the owned reap: {after:?}"
        );
        println!("PID-AFTER-CLEANUP pid={pid} state={after:?}");
    }
}

impl Drop for AdoptedChild {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let r = load_probe_harness::bounded_kill_and_reap(
                &mut c,
                self.pid,
                std::time::Duration::from_secs(10),
            );
            eprintln!("#2786 C1 r31 F4: ADOPTED-CLEANUP on the unwind path: {r}");
        }
    }
}

#[test]
fn a_failed_kill_with_a_live_child_is_reported_as_pending_within_the_budget() {
    use load_probe_harness::{run_inner_with_fault, Fault, SLEEPER};
    let t0 = std::time::Instant::now();
    let mut r = run_inner_with_fault(SLEEPER, Fault::KillFails);
    let elapsed = t0.elapsed();
    // r31 F4: FIRST, before a single fallible assertion. From here the
    // guard owns the surviving child on every exit path, panic included.
    let mut adopted = AdoptedChild::adopt(&mut r);
    assert!(
        adopted.child.is_some(),
        "the report must hand over the un-reaped child, not just its pid"
    );
    let c = r
        .cleanup
        .as_ref()
        .expect("a failed kill must hand back a cleanup report");
    assert_eq!(
        c.outcome.label(),
        "CLEANUP-PENDING",
        "a failed kill on a live child must be PENDING: {}",
        c.describe()
    );
    assert!(
        !c.reaped_ok(),
        "PENDING is never a reaped child: {}",
        c.describe()
    );
    assert!(
        c.kill_error
            .as_deref()
            .is_some_and(|e| e.contains("injected kill failure")),
        "the failed kill must be reported: {}",
        c.describe()
    );
    // The bound, from both sides: the poll ran its full 5 s ceiling and
    // the row did not hang. 30 s is the ceiling of the assertion, not an
    // expected duration.
    assert!(
        elapsed >= std::time::Duration::from_secs(5)
            && elapsed <= std::time::Duration::from_secs(30),
        "the bounded poll took {elapsed:?}"
    );
    println!("PENDING-CONTROL {} in {elapsed:?}", c.describe());
    assert_the_shared_gate_rejects(&r, "was not reaped");
    adopted.prove_alive_then_reap();
}

#[test]
fn a_failed_kill_with_an_errored_poll_is_reported_as_failed_at_once() {
    use load_probe_harness::{run_inner_with_fault, Fault, SLEEPER};
    let t0 = std::time::Instant::now();
    let mut r = run_inner_with_fault(SLEEPER, Fault::KillFailsAndReapErrors);
    let elapsed = t0.elapsed();
    // r31 F4: FIRST, before a single fallible assertion. From here the
    // guard owns the surviving child on every exit path, panic included.
    let mut adopted = AdoptedChild::adopt(&mut r);
    assert!(
        adopted.child.is_some(),
        "the report must hand over the un-reaped child, not just its pid"
    );
    let c = r
        .cleanup
        .as_ref()
        .expect("an errored reap must hand back a cleanup report");
    assert_eq!(
        c.outcome.label(),
        "REAP-FAILED",
        "an errored poll must be REAP-FAILED: {}",
        c.describe()
    );
    assert!(
        !c.reaped_ok(),
        "FAILED is never a reaped child: {}",
        c.describe()
    );
    assert!(
        c.outcome.detail().contains("injected try_wait failure"),
        "the poll error must be carried, not discarded: {}",
        c.describe()
    );
    // It must NOT spend the 5 s budget: the error breaks out at once.
    assert!(
        elapsed <= std::time::Duration::from_secs(20),
        "the errored poll took {elapsed:?}"
    );
    println!("FAILED-CONTROL {} in {elapsed:?}", c.describe());
    assert_the_shared_gate_rejects(&r, "was not reaped");
    adopted.prove_alive_then_reap();
}

#[test]
fn a_successful_kill_is_reported_as_reaped_and_leaves_no_live_pid() {
    use load_probe_harness::{pid_state, run_inner_with_fault, Fault, PidState, SLEEPER};
    let t0 = std::time::Instant::now();
    let r = run_inner_with_fault(SLEEPER, Fault::Timeout);
    let elapsed = t0.elapsed();
    let c = r
        .cleanup
        .as_ref()
        .expect("a timeout must hand back a cleanup report");
    assert_eq!(c.outcome.label(), "reaped", "{}", c.describe());
    assert!(c.reaped_ok(), "{}", c.describe());
    assert!(c.kill_error.is_none(), "{}", c.describe());
    assert!(
        elapsed <= std::time::Duration::from_secs(30),
        "the reaping path took {elapsed:?}"
    );
    let after = pid_state(c.pid);
    assert_eq!(
        after,
        PidState::Dead,
        "a reaped child must leave no live pid: {after:?}"
    );
    println!(
        "REAPED-CONTROL {} in {elapsed:?} pid state after={after:?}",
        c.describe()
    );
    // Same shared gate, same rejection: a timed-out run is never green,
    // for a different reason, so the two verdicts stay distinguishable.
    assert_the_shared_gate_rejects(&r, "timed out");
}

#[test]
fn an_unwind_between_spawn_and_reap_still_reaps_the_child() {
    use load_probe_harness::{run_inner_with_fault, take_unwind_cleanups, Fault, SLEEPER};
    let _ = take_unwind_cleanups();
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let caught = std::panic::catch_unwind(|| run_inner_with_fault(SLEEPER, Fault::PanicWhileOwned));
    std::panic::set_hook(hook);
    assert!(caught.is_err(), "the injected unwind must reach the row");
    let done = take_unwind_cleanups();
    assert_eq!(
        done.len(),
        1,
        "the guard must reap exactly one child on the unwind path: {done:?}"
    );
    assert!(
        done[0].reaped_ok(),
        "the child was not reaped on the unwind path: {}",
        done[0].describe()
    );
    // r29 item 3: the unwind path checks the KILL error report on its own
    // too. See the comment in the timeout row for why it is not shared.
    assert!(
        done[0].kill_error.is_none(),
        "terminating the child on the unwind path reported an error: {}",
        done[0].describe()
    );
}

// #2786 C1 E15, third selector: an off-thread load IS counted. One load on
// a spawned, joined thread and one on the test thread, two directories.
#[test]
fn the_load_probe_observes_every_thread() {
    super::load_probe_harness::expect_child_pass(
        "config::agent_config::tests::harness_rows::inner_the_load_probe_observes_every_thread",
    );
}

#[test]
#[ignore = "#2786 C1: runs only in the dedicated armed process"]
fn inner_the_load_probe_observes_every_thread() {
    let off = tempfile::tempdir().expect("tempdir");
    let on = tempfile::tempdir().expect("tempdir");
    let obs = super::load_probe::Observation::open();
    let off_dir = off.path().to_path_buf();
    std::thread::spawn(move || super::read_agent_local_config(&off_dir))
        .join()
        .expect("the off-thread load");
    super::read_agent_local_config_json(on.path()).expect("the on-thread load");
    println!("REACHED-CALLER");
    let loads = obs.loads();
    let want = vec![off.path().to_path_buf(), on.path().to_path_buf()];
    assert_eq!(loads, want, "recorded {loads:?}, expected exactly {want:?}");
}

// #2786 C1 E15, fourth selector: inside the armed process a wrapper load
// with no open observation panics, naming the directory.
#[test]
fn a_load_with_no_open_observation_fails_closed() {
    super::load_probe_harness::expect_child_pass(concat!(
        "config::agent_config::tests::harness_rows::",
        "inner_a_load_with_no_open_observation_fails_closed"
    ));
}

#[test]
#[ignore = "#2786 C1: runs only in the dedicated armed process"]
fn inner_a_load_with_no_open_observation_fails_closed() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _slot = super::load_probe::NoObservation::open();
    println!("REACHED-CALLER");
    let err = std::panic::catch_unwind(|| super::read_agent_local_config(tmp.path()))
        .expect_err("a load with no open observation must panic");
    let text = err.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(
        text.contains("wrapper load with no open Observation")
            && text.contains(&format!("{:?}", tmp.path())),
        "the panic must name the directory: {text:?}"
    );
}
