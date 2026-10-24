//! #632 - per-agent Windows Job Object for reliable process-tree teardown.
//!
//! Each spawned agent's ConPTY child is assigned to its own Job Object created
//! with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. Terminating the job (explicitly via
//! `TerminateJobObject`, or implicitly when the last handle closes on a hard
//! process exit / panic) kills the entire descendant tree atomically. This is
//! immune to PID reuse and per-PID ACCESS_DENIED, and needs no process-snapshot
//! walking to request a tree kill. Cancellable updater/probe owners additionally
//! query job accounting until `ActiveProcesses == 0` before publishing a terminal
//! outcome; `TerminateJobObject` alone is not a synchronous settlement proof.
//!
//! The resource-monitor identity reaper stays as the ACCOUNTING / slot-cap
//! mechanism (and the fallback when a job cannot be created or assigned). The job
//! is purely the KILL mechanism layered under it.
//!
//! INVARIANT (do not break): the job handle is created NON-INHERITABLE
//! (`CreateJobObjectW(null, null)` - no inheritable SECURITY_ATTRIBUTES) and is
//! owned solely by one `PtyInstance`. portable_pty spawns ConPTY children with
//! handle inheritance ON, so an inheritable job handle would leak into the child
//! and defeat KILL_ON_JOB_CLOSE (the job would not be the last handle holder).
//! A non-inheritable, singly-owned handle makes KILL_ON_JOB_CLOSE fire exactly
//! once, when AC's only handle closes (terminate / drop / process-exit).
//!
//! Non-Windows builds use a zero-sized stub so PtyManager stays platform-agnostic.

/// One monotonic native-settlement attempt budget. Updater, probe, and rejected
/// suspended-spawn cleanup pass this same deadline through direct-child, reader,
/// and native tree proof work; a new deadline is created only after the current
/// attempt has actually expired.
pub(crate) struct SettlementAttempt {
    window: std::time::Duration,
    deadline: std::time::Instant,
}

impl SettlementAttempt {
    pub(crate) fn new(window: std::time::Duration) -> Self {
        Self {
            window,
            deadline: std::time::Instant::now() + window,
        }
    }

    pub(crate) fn remaining(&self) -> std::time::Duration {
        self.deadline
            .saturating_duration_since(std::time::Instant::now())
    }

    pub(crate) fn expired(&self) -> bool {
        self.remaining().is_zero()
    }

    pub(crate) fn restart(&mut self) {
        self.deadline = std::time::Instant::now() + self.window;
    }
}

#[cfg(windows)]
pub use windows_impl::JobObject;

#[cfg(windows)]
pub(crate) use windows_impl::{spawn_suspended_contained, ContainedSpawnError};

#[cfg(all(test, windows))]
pub(crate) use windows_impl::{
    with_contained_spawn_test_hook, with_contained_spawn_test_hook_after_spawns, InjectedFailure,
    SpawnTestHook,
};

#[cfg(not(windows))]
pub use stub_impl::JobObject;

#[cfg(windows)]
mod windows_impl;

#[cfg(not(windows))]
mod stub_impl;

// #632 LOW-2 - the ONE automated proof that the job kills the whole tree (child AND
// grandchild). Deliberately NOT #[ignore] and gated #[cfg(windows)] so it runs by
// default in the `rust-regression-windows` CI lane (runs-on: windows-latest, executes
// `cargo test --lib --bins --tests`). Real processes; written defensively (generous
// polls) to avoid flake. Do NOT add #[ignore] - that removes all executed coverage
// of Part A.
#[cfg(all(test, windows))]
mod tests;
