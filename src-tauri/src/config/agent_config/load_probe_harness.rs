use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const CHILD_TIMEOUT: Duration = Duration::from_secs(60);
/// r29 item 5: the ceiling on the reap poll taken ONLY after a kill that
/// errored, so a failed termination cannot block the row forever.
const REAP_BUDGET: Duration = Duration::from_secs(5);
static N: AtomicU64 = AtomicU64::new(0);

/// The `#[ignore]`d inner row the harness ownership rows drive: it blocks
/// until the harness kills it, so those rows always act on a LIVE child.
pub(crate) const SLEEPER: &str =
    "config::agent_config::tests::harness_rows::inner_sleeps_until_killed";

/// How far cleanup actually got for one child. Deliberately not a boolean
/// and not an `Option`: the three outcomes are distinguishable, and
/// `Reaped` is produced ONLY where a `wait`/`try_wait` call returned an
/// exit status, so the harness can never claim a child it did not observe
/// exit.
#[derive(Debug)]
pub(crate) enum ReapOutcome {
    /// `wait()`, or the bounded `try_wait()` loop, returned this status.
    Reaped(String),
    /// The bounded wait after a FAILED kill expired with the child never
    /// observed to exit: cleanup is PENDING and the process may be alive.
    Pending(String),
    /// `wait()`/`try_wait()` itself errored. Nothing was observed.
    Failed(String),
}

impl ReapOutcome {
    /// One word for the state. Used in every diagnostic.
    pub(crate) fn label(&self) -> &'static str {
        match self {
            ReapOutcome::Reaped(_) => "reaped",
            ReapOutcome::Pending(_) => "CLEANUP-PENDING",
            ReapOutcome::Failed(_) => "REAP-FAILED",
        }
    }
    /// The payload, READ here rather than suppressed: it is the exit status
    /// or the error text a human needs, so `dead_code` has no complaint and
    /// no `allow` is added for it.
    pub(crate) fn detail(&self) -> &str {
        match self {
            ReapOutcome::Reaped(s) | ReapOutcome::Pending(s) | ReapOutcome::Failed(s) => s,
        }
    }
}

/// What the ownership guard achieved for one child, and what it did not.
#[derive(Debug)]
pub(crate) struct Cleanup {
    pub(crate) pid: u32,
    pub(crate) outcome: ReapOutcome,
    pub(crate) kill_error: Option<String>,
    /// r31 F4/F5: the STILL-OWNED `Child`, on the two paths where the
    /// guard could not reap it. r30 dropped the handle here, which on a
    /// Unix host leaves the process as this process's un-reaped zombie
    /// and leaves nobody able to `wait()` it. The handle is now HANDED
    /// OVER instead of dropped: either the caller adopts it (the two
    /// direct controls do, before their first fallible assertion) or
    /// `ChildGuard::drop` kills and reaps it. `None` on the reaped path
    /// and after whoever took it has done so.
    pub(crate) unreaped: Option<Child>,
}

/// r31 F4/F5: kill a still-owned child and REALLY reap it, bounded.
///
/// `kill()` alone is not cleanup: on a Unix host the child stays in the
/// process table as a zombie until its parent waits, and `ps` reports it.
/// `wait()` on the owned handle is the portable reap, needs no external
/// command and no new dependency, and is what makes the pid disappear on
/// both platforms. The poll is bounded so no path can wait for ever.
pub(crate) fn bounded_kill_and_reap(child: &mut Child, pid: u32, budget: Duration) -> String {
    let killed = match child.kill() {
        Ok(()) => "kill ok".to_string(),
        Err(e) => format!("kill error: {e}"),
    };
    let deadline = Instant::now() + budget;
    loop {
        match child.try_wait() {
            Ok(Some(st)) => return format!("pid {pid}: {killed}; REAPED status {st}"),
            Ok(None) => {
                if Instant::now() >= deadline {
                    return format!("pid {pid}: {killed}; NOT reaped within {budget:?}");
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return format!("pid {pid}: {killed}; reap error: {e}"),
        }
    }
}

impl Cleanup {
    /// True ONLY for an observed exit status. `Pending` and `Failed` are
    /// both false, so a row cannot pass on an unproven cleanup.
    pub(crate) fn reaped_ok(&self) -> bool {
        matches!(self.outcome, ReapOutcome::Reaped(_))
    }
    pub(crate) fn describe(&self) -> String {
        format!(
            "pid {} outcome={} ({}) kill_error={:?}",
            self.pid,
            self.outcome.label(),
            self.outcome.detail(),
            self.kill_error
        )
    }
}

/// Cleanups performed by `Drop`, i.e. on an unwind path, where the caller
/// is gone and cannot be handed the report.
static UNWIND_CLEANUPS: Mutex<Vec<Cleanup>> = Mutex::new(Vec::new());

pub(crate) fn take_unwind_cleanups() -> Vec<Cleanup> {
    std::mem::take(&mut *UNWIND_CLEANUPS.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Owns the child from spawn to reap. While `child` is `Some`, terminating
/// and reaping it is this guard's duty on EVERY exit path, return or
/// unwind. Ownership is released only by `reaped_by_caller`, which the
/// caller may call only after `try_wait` handed it an exit status.
struct ChildGuard {
    pid: u32,
    child: Option<Child>,
    /// The injected fault of the row that created this guard, so the two
    /// r30 failure branches can be reached deterministically. `Fault::None`
    /// for every observation row, which is the whole ordinary suite.
    fault: Fault,
}

impl ChildGuard {
    fn new(child: Child, fault: Fault) -> ChildGuard {
        let pid = child.id();
        ChildGuard {
            pid,
            child: Some(child),
            fault,
        }
    }
    fn owned(&mut self) -> &mut Child {
        self.child
            .as_mut()
            .expect("#2786 C1: the guard no longer owns the child")
    }
    fn reaped_by_caller(&mut self) {
        self.child = None;
    }
    /// r29: bounded on the failure path. After a kill that SUCCEEDED,
    /// `wait()` is the portable reap and returns promptly. After a kill
    /// that ERRORED the child may still be running, and `wait()` would then
    /// block for as long as it lives, which on the 120 s sleeper hangs the
    /// row and the suite with no diagnostic. That path polls `try_wait`
    /// for at most `REAP_BUDGET` and, if the child is never observed to
    /// exit, reports `Pending` with the pid, so the failure is visible and
    /// owned instead of silent.
    fn terminate_and_reap(&mut self) -> Option<Cleanup> {
        // NOTE, and this is the incompleteness the plan states: `take`
        // moves the `Child` out of the guard. From here on the guard owns
        // nothing, and when `child` is dropped at the end of this function
        // the handle is gone. On the `Pending` and `Failed` paths the
        // process may still be running with nobody holding a handle to it.
        let mut child = self.child.take()?;
        let injected_kill = matches!(self.fault, Fault::KillFails | Fault::KillFailsAndReapErrors);
        let kill_error = if injected_kill {
            Some("#2786 C1: injected kill failure; the child was NOT terminated".to_string())
        } else {
            child.kill().err().map(|e| e.to_string())
        };
        let mut inject_reap_error = self.fault == Fault::KillFailsAndReapErrors;
        let outcome = if kill_error.is_none() {
            match child.wait() {
                Ok(s) => ReapOutcome::Reaped(format!("{s}")),
                Err(e) => ReapOutcome::Failed(e.to_string()),
            }
        } else {
            let deadline = Instant::now() + REAP_BUDGET;
            loop {
                let probed = if inject_reap_error {
                    inject_reap_error = false;
                    Err(std::io::Error::other(
                        "#2786 C1: injected try_wait failure inside the reap poll",
                    ))
                } else {
                    child.try_wait()
                };
                match probed {
                    Ok(Some(s)) => break ReapOutcome::Reaped(format!("{s}")),
                    Ok(None) => {
                        if Instant::now() >= deadline {
                            break ReapOutcome::Pending(format!(
                                "terminating pid {} failed and it was not observed to exit \
                                 within {REAP_BUDGET:?}: cleanup is PENDING and the process \
                                 may still be running",
                                self.pid
                            ));
                        }
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    Err(e) => break ReapOutcome::Failed(e.to_string()),
                }
            }
        };
        // r31 F4/F5: the handle is dropped ONLY when the child was really
        // reaped. On `Pending` and `Failed` it travels with the report, so
        // no code path drops a handle to a process that is still running.
        let unreaped = match outcome {
            ReapOutcome::Reaped(_) => None,
            ReapOutcome::Pending(_) | ReapOutcome::Failed(_) => Some(child),
        };
        Some(Cleanup {
            pid: self.pid,
            outcome,
            kill_error,
            unreaped,
        })
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut c) = self.terminate_and_reap() {
            // r31 F4: the caller is gone, so nobody can adopt the handle.
            // Kill and REAP it here, bounded, on this unwind or return
            // path, before the report is stored.
            if let Some(mut child) = c.unreaped.take() {
                let r = bounded_kill_and_reap(&mut child, c.pid, REAP_BUDGET);
                eprintln!("#2786 C1 r31 F4: DROP-CLEANUP {r}");
            }
            if !c.reaped_ok() {
                // The caller is gone; this is the only place the failure
                // can still be seen by a human reading the child log.
                eprintln!(
                    "#2786 C1: CLEANUP NOT ACHIEVED on the unwind path: {}",
                    c.describe()
                );
            }
            UNWIND_CLEANUPS
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(c);
        }
    }
}

/// The child log, removed on every exit path so an unwind does not leave a
/// stray file in the temp directory.
struct LogFile(std::path::PathBuf);

impl Drop for LogFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Deterministic faults for the harness ownership rows. Never used by an
/// observation row: `run_inner` passes `Fault::None`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Fault {
    None,
    /// The first `try_wait` is replaced by an injected I/O error.
    WaitError,
    /// The deadline is already past when the supervision loop starts.
    Timeout,
    /// Unwind while the guard still owns a live child.
    PanicWhileOwned,
    /// r30 item 5, DIRECT control: `Child::kill` is replaced by an
    /// injected error and NOT performed, so the child stays ALIVE and the
    /// bounded `try_wait` poll runs to its ceiling and reports `Pending`.
    /// This is the only way to reach that branch: a real `kill` on a live
    /// child of this process does not fail on either platform.
    KillFails,
    /// r30 item 5, DIRECT control: the kill is injected as above AND the
    /// first `try_wait` inside `terminate_and_reap` is replaced by an
    /// injected I/O error, so the branch reports `Failed` at once.
    KillFailsAndReapErrors,
}

/// r30 item 5: what a pid query concluded. Three outcomes, never two: an
/// absent process and a query that could not answer are different facts,
/// and conflating them once made a liveness check read `dead` for both.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PidState {
    Alive,
    Dead,
    QueryError(String),
}

/// Query one pid by an explicit filter string. `pid_state` renders the
/// filter; a test may pass a malformed one to exercise `QueryError`.
///
/// It shells out to the platform's process lister through `Command`, whose
/// output this process reads directly. It does NOT use a shell, command
/// substitution, a job object, or the teardown of any wrapper process, so
/// what it reports is the state of the pid at the moment of the call and
/// not an artefact of some parent exiting.
#[cfg(windows)]
pub(crate) fn pid_state_by_filter(filter: &str) -> PidState {
    let out = match Command::new("tasklist")
        .args(["/FI", filter, "/NH", "/FO", "CSV"])
        .output()
    {
        Ok(o) => o,
        Err(e) => return PidState::QueryError(format!("spawning tasklist failed: {e}")),
    };
    let text =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        return PidState::QueryError(format!(
            "tasklist exit {:?}: {}",
            out.status.code(),
            text.trim()
        ));
    }
    // An INFO line is the documented way tasklist says "no match". It is
    // NOT an error, and it is NOT a live process.
    if text.contains("INFO:") || text.contains("No tasks") {
        return PidState::Dead;
    }
    if text.trim_start().starts_with('"') {
        return PidState::Alive;
    }
    PidState::QueryError(format!("tasklist output not understood: {}", text.trim()))
}

/// r31 F5. `ps -p` cannot tell these two apart by exit code and stdout:
/// a legitimately absent pid and a malformed selector BOTH exit 1 with
/// empty stdout (measured on Debian, `evidence/f5-unix-ps-probe.txt`).
/// stderr does not separate them reliably either: under WSL every `ps`
/// call writes a screen-size warning, so "stderr is non-empty" reads the
/// absent-pid case as an error too, and the only real discriminator,
/// `ps`'s own `error:` line, is locale text.
///
/// So the selector is validated HERE, before `ps` is ever spawned:
/// `pid_state` renders a decimal pid, and any other selector is a caller
/// mistake, reported as `QueryError` without a subprocess. A numeric
/// selector then keeps the exit-code mapping, and anything `ps` answers
/// that does not fit it is still `QueryError`. No new dependency.
#[cfg(not(windows))]
pub(crate) fn pid_state_by_filter(filter: &str) -> PidState {
    if filter.is_empty() || !filter.bytes().all(|b| b.is_ascii_digit()) {
        return PidState::QueryError(format!(
            "not a pid selector for ps: {filter:?};                  \
             ps answers a malformed selector exactly as it answers an absent pid"
        ));
    }
    let out = match Command::new("ps")
        .args(["-p", filter, "-o", "pid="])
        .output()
    {
        Ok(o) => o,
        Err(e) => return PidState::QueryError(format!("spawning ps failed: {e}")),
    };
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    match (out.status.code(), text.is_empty()) {
        (Some(0), false) => PidState::Alive,
        (Some(1), true) => PidState::Dead,
        (c, _) => PidState::QueryError(format!("ps exit {c:?}: {text}")),
    }
}

/// r31 F5: a selector that is malformed ON THIS PLATFORM, so the
/// query-error leg is exercised by the same control everywhere. r30 used
/// the Windows filter string on both, and on a Unix host `ps` answers it
/// exactly as it answers an absent pid, which made that leg RED there.
#[cfg(windows)]
pub(crate) const BAD_PID_SELECTOR: &str = "PID eq not-a-pid";
#[cfg(not(windows))]
pub(crate) const BAD_PID_SELECTOR: &str = "not-a-pid";

pub(crate) fn pid_state(pid: u32) -> PidState {
    #[cfg(windows)]
    let f = format!("PID eq {pid}");
    #[cfg(not(windows))]
    let f = format!("{pid}");
    pid_state_by_filter(&f)
}

// r31 F5: r30's `external_kill_by_pid` is REMOVED, not kept unused.
//
// It shelled out to `taskkill`/`kill` on a bare pid, which is the very
// thing the dev's F5 shows is not cleanup: on a Unix host a signal to
// this process's own child without a `wait()` leaves a zombie that `ps`
// still reports. The two paths that used it now hand the still-owned
// `Child` to the caller and reap it through `bounded_kill_and_reap`,
// which is portable and needs no external command.

pub(crate) struct ChildRun {
    pub(crate) ok: bool,
    pub(crate) out: String,
    pub(crate) timed_out: bool,
    /// `Some` when supervision itself failed, e.g. an errored `try_wait`.
    pub(crate) wait_error: Option<String>,
    /// `Some` whenever the guard had to terminate the child, carrying what
    /// it achieved. Absent on the normal path, where the child exited by
    /// itself and `try_wait` reaped it.
    pub(crate) cleanup: Option<Cleanup>,
}

/// Re-execute THIS test binary for one `#[ignore]`d inner row, with the
/// probe armed through `Command::env` only.
pub(crate) fn run_inner(name: &str) -> ChildRun {
    run_inner_with_fault(name, Fault::None)
}

pub(crate) fn run_inner_with_fault(name: &str, fault: Fault) -> ChildRun {
    let exe = std::env::current_exe().expect("test binary path");
    let log = LogFile(std::env::temp_dir().join(format!(
        "ac2786-child-{}-{}.log",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    )));
    let sink = std::fs::File::create(&log.0).expect("child log");
    let sink2 = sink.try_clone().expect("child log clone");
    let child = Command::new(exe)
        .args([
            name,
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(super::load_probe::ARM_VAR, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::from(sink))
        .stderr(Stdio::from(sink2))
        .spawn()
        .expect("spawn the dedicated observation process");
    let mut guard = ChildGuard::new(child, fault);

    // The two r30 failure controls need `terminate_and_reap` to run on a
    // LIVE child, which is exactly what the past deadline produces.
    let deadline = if matches!(
        fault,
        Fault::Timeout | Fault::KillFails | Fault::KillFailsAndReapErrors
    ) {
        Instant::now()
    } else {
        Instant::now() + CHILD_TIMEOUT
    };
    let mut status = None;
    let mut timed_out = false;
    let mut wait_error = None;
    let mut cleanup = None;
    let mut inject_wait_error = fault == Fault::WaitError;
    loop {
        if fault == Fault::PanicWhileOwned {
            panic!(
                "#2786 C1: injected unwind while the guard owns child pid {}",
                guard.pid
            );
        }
        let probed = if inject_wait_error {
            inject_wait_error = false;
            Err(std::io::Error::other("#2786 C1: injected try_wait failure"))
        } else {
            guard.owned().try_wait()
        };
        match probed {
            Ok(Some(s)) => {
                // `try_wait` reaped it; nothing is left for the guard.
                status = Some(s);
                guard.reaped_by_caller();
                break;
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    timed_out = true;
                    cleanup = guard.terminate_and_reap();
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                wait_error = Some(e.to_string());
                cleanup = guard.terminate_and_reap();
                break;
            }
        }
    }

    let mut out = String::new();
    if let Ok(mut f) = std::fs::File::open(&log.0) {
        let _ = f.read_to_string(&mut out);
    }
    ChildRun {
        ok: status.map(|s| s.success()).unwrap_or(false),
        out,
        timed_out,
        wait_error,
        cleanup,
    }
}

/// Shared assertions: supervision itself succeeded, any termination the
/// guard had to perform really reaped the child, the child really ran ONE
/// test, so a filter that matched nothing cannot pass as a green row, and
/// it really reached the caller.
pub(crate) fn assert_one_test_ran(r: &ChildRun, name: &str) {
    assert!(
        r.wait_error.is_none(),
        "supervising the child for {name} failed: {:?}",
        r.wait_error
    );
    if let Some(c) = &r.cleanup {
        assert!(
            c.reaped_ok(),
            "the child for {name} was not reaped: {}",
            c.describe()
        );
    }
    assert!(!r.timed_out, "child for {name} timed out: {}", r.out);
    assert!(
        r.out.contains("running 1 test"),
        "child for {name} did not run exactly one test: {}",
        r.out
    );
    assert!(
        r.out.contains("test result:"),
        "child for {name} printed no result line: {}",
        r.out
    );
    assert!(
        r.out.contains("REACHED-CALLER"),
        "child for {name} never reached the real caller: {}",
        r.out
    );
}

pub(crate) fn expect_child_pass(name: &str) -> String {
    let r = run_inner(name);
    assert_one_test_ran(&r, name);
    assert!(r.ok, "child for {name} failed: {}", r.out);
    r.out
}

/// The foreign id both launch-path fixtures store; no local agent has it,
/// so it can only resolve through its descriptor.
pub(crate) const SNAPSHOT_ID: &str = "foreign-claude";

/// #2786 C1 E14/E15: the one fixture both callers' rows build. `config.json`
/// holds the id under `id_key` plus a descriptor with `command: "old"`; the
/// state file holds the descriptor for that id with `state_command` and no
/// marker. Rewriting it on the same directory changes only the state side.
/// Written through `write_file_atomic`, the shared writer the #1938 scan
/// requires outside a `mod tests` block.
pub(crate) fn write_snapshot_fixture(dir: &std::path::Path, id_key: &str, state_command: &str) {
    let tracked = serde_json::json!({"tooling": {
        id_key: SNAPSHOT_ID,
        "codingAgents": {SNAPSHOT_ID: {"command": "old"}}}});
    let state = serde_json::json!({"tooling": {
        "codingAgents": {SNAPSHOT_ID: {"command": state_command}}}});
    for (name, value) in [
        ("config.json", tracked),
        (
            crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME,
            state,
        ),
    ] {
        crate::config::local_config_io::write_file_atomic(
            &dir.join(name),
            value.to_string().as_bytes(),
        )
        .unwrap();
    }
}

/// One local agent per descriptor command the fixtures use, named
/// `via-<command>`, so the resolved agent id names the descriptor read.
pub(crate) fn snapshot_settings() -> crate::config::settings::AppSettings {
    let agent = |command: &str| crate::config::settings::AgentConfig {
        preflight: None,
        preflight_timeout_seconds: None,
        id: format!("via-{command}"),
        id_hash: None,
        id_hash_debug: None,
        label: format!("label-{command}"),
        command: command.to_string(),
        color: "#000000".to_string(),
        order: None,
        envs: Vec::new(),
        isolated_home: false,
        instructions_filename: None,
        config_seed: None,
        context_regex: None,
        blocking_menus: None,
        backend: Default::default(),
    };
    crate::config::settings::AppSettings {
        agents: ["old", "new", "new-a", "new-b"].map(agent).to_vec(),
        ..Default::default()
    }
}
