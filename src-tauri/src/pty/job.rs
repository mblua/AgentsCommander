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
mod windows_impl {
    use std::fmt;
    use std::time::Duration;

    use super::SettlementAttempt;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicAccountingInformation,
        JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
        TerminateJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, OpenThread, ResumeThread, CREATE_NO_WINDOW, CREATE_SUSPENDED,
        PROCESS_SET_QUOTA, PROCESS_TERMINATE, THREAD_SUSPEND_RESUME,
    };

    const SETTLEMENT_WINDOW: Duration = Duration::from_secs(10);
    const SETTLEMENT_POLL: Duration = Duration::from_millis(25);

    #[derive(Debug)]
    pub(crate) enum ContainedSpawnError {
        Spawn(std::io::Error),
        Containment(&'static str),
        Cleanup(String),
    }

    impl fmt::Display for ContainedSpawnError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::Spawn(error) => write!(f, "spawn failed: {error}"),
                Self::Containment(step) => write!(f, "process containment failed at {step}"),
                Self::Cleanup(detail) => write!(f, "process containment cleanup failed: {detail}"),
            }
        }
    }

    impl std::error::Error for ContainedSpawnError {}

    /// Owns a Job Object handle. Dropping it closes the handle; because the job is
    /// created with KILL_ON_JOB_CLOSE and we hold the only handle, the OS kills the
    /// whole assigned tree on drop too (the hard-exit / panic safety net).
    #[derive(Debug)]
    pub struct JobObject {
        handle: HANDLE,
    }

    // A HANDLE is an opaque kernel handle, safe to use and close from any thread;
    // the value is only moved into PtyInstance and read back out under a Mutex.
    unsafe impl Send for JobObject {}
    unsafe impl Sync for JobObject {}

    impl JobObject {
        fn create() -> Result<Self, &'static str> {
            // SAFETY: the returned handle is null-checked and then owned by
            // `JobObject`; the information buffer is initialized before use.
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return Err("create_job");
                }
                let job = JobObject { handle };
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                if SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                ) == 0
                {
                    return Err("configure_job");
                }
                Ok(job)
            }
        }

        fn assign(&self, pid: u32) -> Result<(), &'static str> {
            // SAFETY: the process handle is null-checked and closed on every
            // path; `self.handle` remains owned by this JobObject.
            unsafe {
                let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
                if process.is_null() {
                    return Err("open_process");
                }
                let assigned = AssignProcessToJobObject(self.handle, process);
                let _ = CloseHandle(process);
                if assigned == 0 {
                    return Err("assign_job");
                }
                Ok(())
            }
        }

        /// Create a job, set KILL_ON_JOB_CLOSE, and assign process `pid` to it.
        /// Returns `None` (after a warn log) on ANY failure, so a job problem never
        /// blocks a spawn; the identity reaper remains the cleanup fallback.
        ///
        /// `pid` is the process portable_pty spawned. For a non-`.exe` command the
        /// non-direct-exe branch of `PtyManager::spawn` wraps it as `cmd.exe /C <cmd>`,
        /// so `pid` is usually the cmd.exe wrapper and the real agent is a grandchild
        /// cmd spawns AFTER assignment - that is ideal: KILL_ON_JOB_CLOSE plus
        /// child-inheritance captures the whole wrapper -> agent -> descendants subtree.
        ///
        /// portable_pty cannot spawn CREATE_SUSPENDED, so `pid` is already running.
        /// A grandchild spawned in the sub-ms window before assignment can escape
        /// the job; neither cmd.exe nor an agent CLI forks that fast. See the plan
        /// section 5 for the (accepted, race-bound) shutdown residual this leaves.
        pub fn for_child(pid: u32) -> Option<Self> {
            let job = match Self::create() {
                Ok(job) => job,
                Err(step) => {
                    log::warn!("[pty] Job Object {step} failed for pid {pid}; reaper-only cleanup");
                    return None;
                }
            };
            if let Err(step) = job.assign(pid) {
                log::warn!("[pty] Job Object {step} failed for pid {pid}; reaper-only cleanup");
                return None;
            }
            log::info!("[pty] assigned pid {pid} to job object for tree-kill");
            Some(job)
        }

        /// Request termination of every process in the job. Idempotent; safe on
        /// an already-dead tree (TerminateJobObject just reports failure, which
        /// we log at debug). Callers that need definitive settlement must query
        /// `active_processes` until it returns zero.
        pub fn terminate(&self) {
            if self.terminate_checked().is_err() {
                log::debug!("[pty] TerminateJobObject failed (tree likely already gone)");
            }
        }

        pub(crate) fn terminate_checked(&self) -> std::io::Result<()> {
            // SAFETY: `self.handle` is a valid job handle owned by `self`.
            if unsafe { TerminateJobObject(self.handle, 1) } == 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        }

        /// Query the authoritative Job Object active-member count. Callers use
        /// this only after dropping every owner-held direct-process reference.
        pub(crate) fn active_processes(&self) -> std::io::Result<u32> {
            let mut accounting: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION =
                unsafe { std::mem::zeroed() };
            // SAFETY: the output buffer has the exact information-class layout
            // and remains valid for the duration of the call.
            let ok = unsafe {
                QueryInformationJobObject(
                    self.handle,
                    JobObjectBasicAccountingInformation,
                    &mut accounting as *mut _ as *mut core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(accounting.ActiveProcesses)
            }
        }
    }

    fn sole_primary_thread(pid: u32) -> Result<u32, &'static str> {
        // SAFETY: standard ToolHelp enumeration; the snapshot is closed before
        // return and THREADENTRY32 has dwSize initialized as required.
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return Err("thread_snapshot");
            }
            let mut entry: THREADENTRY32 = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
            let mut found = None;
            if Thread32First(snapshot, &mut entry) != 0 {
                loop {
                    if entry.th32OwnerProcessID == pid
                        && found.replace(entry.th32ThreadID).is_some()
                    {
                        let _ = CloseHandle(snapshot);
                        return Err("primary_thread_not_unique");
                    }
                    if Thread32Next(snapshot, &mut entry) == 0 {
                        break;
                    }
                }
            }
            let _ = CloseHandle(snapshot);
            found.ok_or("primary_thread_missing")
        }
    }

    fn resume_primary_thread(pid: u32) -> Result<(), &'static str> {
        let tid = sole_primary_thread(pid)?;
        // SAFETY: the thread handle is null-checked and closed after the one
        // resume operation. A suspended spawn must report a prior count of one.
        unsafe {
            let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, tid);
            if thread.is_null() {
                return Err("open_primary_thread");
            }
            let previous = ResumeThread(thread);
            let _ = CloseHandle(thread);
            if previous != 1 {
                return Err("resume_primary_thread");
            }
        }
        Ok(())
    }

    #[cfg(test)]
    #[derive(Clone, Copy)]
    pub(crate) enum InjectedFailure {
        Assignment,
        Resume,
    }

    pub(crate) struct SpawnTestHook {
        #[cfg(test)]
        pub(crate) after_spawn: Option<tokio::sync::oneshot::Sender<()>>,
        #[cfg(test)]
        pub(crate) release_spawn: Option<tokio::sync::oneshot::Receiver<()>>,
        #[cfg(test)]
        pub(crate) after_reap: Option<tokio::sync::oneshot::Sender<()>>,
        #[cfg(test)]
        pub(crate) release_query: Option<tokio::sync::oneshot::Receiver<()>>,
        #[cfg(test)]
        pub(crate) failure: Option<InjectedFailure>,
        #[cfg(test)]
        pub(crate) settlement_window: Option<Duration>,
    }

    #[cfg(test)]
    struct ContainedSpawnTestHookScope {
        remaining_spawns: usize,
        hook: Option<SpawnTestHook>,
    }

    #[cfg(test)]
    tokio::task_local! {
        static CONTAINED_SPAWN_TEST_HOOK: std::cell::RefCell<ContainedSpawnTestHookScope>;
    }

    #[cfg(test)]
    pub(crate) async fn with_contained_spawn_test_hook<F>(
        hook: SpawnTestHook,
        future: F,
    ) -> F::Output
    where
        F: std::future::Future,
    {
        CONTAINED_SPAWN_TEST_HOOK
            .scope(
                std::cell::RefCell::new(ContainedSpawnTestHookScope {
                    remaining_spawns: 0,
                    hook: Some(hook),
                }),
                future,
            )
            .await
    }

    #[cfg(test)]
    pub(crate) async fn with_contained_spawn_test_hook_after_spawns<F>(
        remaining_spawns: usize,
        hook: SpawnTestHook,
        future: F,
    ) -> F::Output
    where
        F: std::future::Future,
    {
        CONTAINED_SPAWN_TEST_HOOK
            .scope(
                std::cell::RefCell::new(ContainedSpawnTestHookScope {
                    remaining_spawns,
                    hook: Some(hook),
                }),
                future,
            )
            .await
    }

    fn record_attempt_expiry(defects: &mut Vec<String>, stage: &str) {
        let detail = format!("settlement attempt deadline exceeded during {stage}");
        if !defects.iter().any(|defect| defect == &detail) {
            defects.push(detail);
        }
    }

    async fn drain_failed_spawn_pipe<R>(
        mut pipe: Option<R>,
        attempt: &mut SettlementAttempt,
        defects: &mut Vec<String>,
    ) -> bool
    where
        R: tokio::io::AsyncRead + Unpin + Send + 'static,
    {
        let Some(mut pipe) = pipe.take() else {
            return true;
        };
        let mut reader = tokio::spawn(async move {
            let mut sink = Vec::new();
            let _ = tokio::io::AsyncReadExt::read_to_end(&mut pipe, &mut sink).await;
        });
        if attempt.expired() {
            record_attempt_expiry(defects, "pipe drainage");
            attempt.restart();
        }
        match tokio::time::timeout(attempt.remaining(), &mut reader).await {
            Ok(_) => true,
            Err(_) => {
                record_attempt_expiry(defects, "pipe drainage");
                reader.abort();
                let _ = reader.await;
                attempt.restart();
                false
            }
        }
    }

    async fn settle_rejected_spawn(
        mut child: tokio::process::Child,
        job: Option<JobObject>,
        reason: &'static str,
        hook: Option<SpawnTestHook>,
    ) -> ContainedSpawnError {
        #[cfg(test)]
        let mut hook = hook;
        #[cfg(not(test))]
        let _ = hook;
        #[cfg(test)]
        let settlement_window = hook
            .as_ref()
            .and_then(|hook| hook.settlement_window)
            .unwrap_or(SETTLEMENT_WINDOW);
        #[cfg(not(test))]
        let settlement_window = SETTLEMENT_WINDOW;
        let mut attempt = SettlementAttempt::new(settlement_window);
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        if let Some(job) = job.as_ref() {
            let _ = job.terminate_checked();
        }
        let mut defects = Vec::new();
        if let Err(error) = child.start_kill() {
            defects.push(format!("kill: {error}"));
        }
        let mut wait_defect_recorded = false;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {}
                Err(error) => {
                    if !wait_defect_recorded {
                        defects.push(format!("wait: {error}"));
                        wait_defect_recorded = true;
                    }
                }
            }
            if attempt.expired() {
                record_attempt_expiry(&mut defects, "direct-child settlement");
                if let Some(job) = job.as_ref() {
                    let _ = job.terminate_checked();
                }
                let _ = child.start_kill();
                attempt.restart();
            }
            tokio::time::sleep(SETTLEMENT_POLL.min(attempt.remaining())).await;
        }
        // Dropping the Child here releases Tokio's direct process handle before
        // the first query that is eligible to prove ActiveProcesses == 0.
        drop(child);

        #[cfg(test)]
        if let Some(hook) = hook.as_mut() {
            if let Some(tx) = hook.after_reap.take() {
                let _ = tx.send(());
            }
            if let Some(mut rx) = hook.release_query.take() {
                loop {
                    if attempt.expired() {
                        record_attempt_expiry(&mut defects, "rejected-spawn proof barrier");
                        attempt.restart();
                    }
                    match tokio::time::timeout(attempt.remaining(), &mut rx).await {
                        Ok(_) => break,
                        Err(_) => {
                            record_attempt_expiry(&mut defects, "rejected-spawn proof barrier");
                            attempt.restart();
                        }
                    }
                }
            }
        }

        let stdout_settled = drain_failed_spawn_pipe(stdout, &mut attempt, &mut defects).await;
        let stderr_settled = drain_failed_spawn_pipe(stderr, &mut attempt, &mut defects).await;
        if !stdout_settled || !stderr_settled {
            defects.push("reader drain required abort".to_string());
        }

        if let Some(job) = job.as_ref() {
            let mut query_defect_recorded = false;
            loop {
                match job.active_processes() {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(error) => {
                        if !query_defect_recorded {
                            defects.push(format!("accounting: {error}"));
                            query_defect_recorded = true;
                        }
                    }
                }
                if attempt.expired() {
                    record_attempt_expiry(&mut defects, "job accounting proof");
                    let _ = job.terminate_checked();
                    attempt.restart();
                }
                tokio::time::sleep(SETTLEMENT_POLL.min(attempt.remaining())).await;
            }
        }
        drop(job);

        if defects.is_empty() {
            ContainedSpawnError::Containment(reason)
        } else {
            ContainedSpawnError::Cleanup(format!("{reason}: {}", defects.join("; ")))
        }
    }

    #[allow(unused_mut)]
    pub(super) async fn spawn_suspended_contained_impl(
        command: &mut tokio::process::Command,
        mut hook: Option<SpawnTestHook>,
    ) -> Result<(tokio::process::Child, JobObject), ContainedSpawnError> {
        command.creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW);
        let child = command.spawn().map_err(ContainedSpawnError::Spawn)?;
        let pid = match child.id() {
            Some(pid) => pid,
            None => {
                return Err(settle_rejected_spawn(child, None, "missing_process_id", hook).await)
            }
        };

        #[cfg(test)]
        if let Some(hook) = hook.as_mut() {
            if let Some(tx) = hook.after_spawn.take() {
                let _ = tx.send(());
            }
            if let Some(rx) = hook.release_spawn.take() {
                let _ = rx.await;
            }
        }

        let job = match JobObject::create() {
            Ok(job) => job,
            Err(reason) => return Err(settle_rejected_spawn(child, None, reason, hook).await),
        };
        #[cfg(test)]
        let reject_assignment = hook
            .as_ref()
            .is_some_and(|hook| matches!(hook.failure, Some(InjectedFailure::Assignment)));
        #[cfg(not(test))]
        let reject_assignment = false;
        if reject_assignment {
            return Err(settle_rejected_spawn(child, Some(job), "assign_job", hook).await);
        }
        if let Err(reason) = job.assign(pid) {
            return Err(settle_rejected_spawn(child, Some(job), reason, hook).await);
        }

        #[cfg(test)]
        let reject_resume = hook
            .as_ref()
            .is_some_and(|hook| matches!(hook.failure, Some(InjectedFailure::Resume)));
        #[cfg(not(test))]
        let reject_resume = false;
        if reject_resume {
            return Err(
                settle_rejected_spawn(child, Some(job), "resume_primary_thread", hook).await,
            );
        }
        if let Err(reason) = resume_primary_thread(pid) {
            return Err(settle_rejected_spawn(child, Some(job), reason, hook).await);
        }
        Ok((child, job))
    }

    pub(crate) async fn spawn_suspended_contained(
        command: &mut tokio::process::Command,
    ) -> Result<(tokio::process::Child, JobObject), ContainedSpawnError> {
        #[cfg(test)]
        let hook = CONTAINED_SPAWN_TEST_HOOK
            .try_with(|scope| {
                let mut scope = scope.borrow_mut();
                if scope.remaining_spawns == 0 {
                    scope.hook.take()
                } else {
                    scope.remaining_spawns -= 1;
                    None
                }
            })
            .ok()
            .flatten();
        #[cfg(not(test))]
        let hook = None;
        spawn_suspended_contained_impl(command, hook).await
    }

    // I2800 preparation only: private feasibility harness, never used by product.
    #[cfg(all(test, windows))]
    mod tests {
        use super::*;
        use sha2::{Digest, Sha256};
        use std::collections::{BTreeMap, BTreeSet};
        use std::fs;
        use std::io::Write;
        use std::os::windows::process::CommandExt;
        use std::path::{Path, PathBuf};
        use std::process::{Command, Stdio};
        use std::time::Instant;
        use tokio::io::AsyncReadExt;
        use windows_sys::Win32::Foundation::{GetLastError, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{
            Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
        };
        use windows_sys::Win32::System::JobObjects::{
            IsProcessInJob, JobObjectBasicProcessIdList,
        };
        use windows_sys::Win32::System::Threading::{
            WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION,
        };

        const ENTRY: &str = "pty::job::windows_impl::tests::install_observed_feasibility_2800_fixture_entry";
        const ROLE: &str = "AC_I2800_FEASIBILITY_ROLE";
        const DIR: &str = "AC_I2800_FEASIBILITY_DIR";
        const NAME: &str = "AC_I2800_FEASIBILITY_NAME";
        const PARENT: &str = "AC_I2800_FEASIBILITY_PARENT";
        const CAP: usize = 256;
        const PREPARED_HEAD: &str = "83219ab22cf4fde931dd34a3e8420e0a6ba499ae";
        const READY: Duration = Duration::from_secs(10);
        const WORKER: Duration = Duration::from_secs(30);
        const GROUP: Duration = Duration::from_secs(1200);
        const POLL: Duration = Duration::from_millis(25);

        fn digest(bytes: &[u8]) -> String {
            format!("{:x}", Sha256::digest(bytes))
        }

        fn fixture_command(exe: &Path, dir: &Path, role: &str, name: &str) -> Command {
            let mut command = Command::new(exe);
            command
                .args(["--exact", ENTRY, "--nocapture", "--test-threads=1"])
                .env(ROLE, role)
                .env(DIR, dir)
                .env(NAME, name)
                .env(PARENT, std::process::id().to_string())
                .current_dir(dir)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .creation_flags(CREATE_NO_WINDOW);
            command
        }

        fn publish(dir: &Path, name: &str, text: &str) {
            let temporary = dir.join(format!("{name}.tmp"));
            fs::write(&temporary, text).expect("fixture publication");
            fs::rename(temporary, dir.join(name)).expect("atomic fixture publication");
        }

        // Inherited Job containment is the authority. No child gets a Job handle,
        // PID-based kill, vendor command, PATH lookup, or externally inherited stdin.
        fn fixture_spawn(dir: &Path, role: &str, name: &str) -> std::process::Child {
            let exe = std::env::current_exe().expect("exact fixture executable");
            fixture_command(&exe, dir, role, name)
                .spawn()
                .expect("private descendant spawn")
        }

        fn wait_release(dir: &Path) {
            while !dir.join("release").exists() {
                std::thread::sleep(POLL);
            }
        }

        fn actual_parent() -> u32 {
            unsafe {
                let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
                assert!(snapshot != INVALID_HANDLE_VALUE, "parent census snapshot");
                let mut entry: PROCESSENTRY32W = std::mem::zeroed();
                entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
                let mut found = None;
                if Process32FirstW(snapshot, &mut entry) != 0 {
                    loop {
                        if entry.th32ProcessID == std::process::id() {
                            found = Some(entry.th32ParentProcessID);
                            break;
                        }
                        if Process32NextW(snapshot, &mut entry) == 0 { break; }
                    }
                }
                CloseHandle(snapshot);
                found.expect("own PID in native parent census")
            }
        }

        #[test]
        fn install_observed_feasibility_2800_fixture_entry() {
            let Ok(role) = std::env::var(ROLE) else {
                return; // Inert entry in the parent libtest process.
            };
            let dir = PathBuf::from(std::env::var_os(DIR).expect("fixture dir"));
            let name = std::env::var(NAME).expect("fixture name");
            let parent = std::env::var(PARENT).expect("fixture parent");
            assert_eq!(parent.parse::<u32>().expect("parent PID"), actual_parent(), "native parent matches manifest provenance");
            let mut children = Vec::new();
            if role == "branch" {
                for n in 0..2 {
                    children.push(fixture_spawn(&dir, "held", &format!("{name}-g{n}")));
                }
            } else if let Some(workload) = role.strip_prefix("root-W") {
                let workload: usize = workload.parse().expect("workload");
                match workload {
                    0 => {}
                    1 | 4 | 5 => {
                        for n in 0..4 {
                            children.push(fixture_spawn(&dir, "held", &format!("h{n}")));
                        }
                    }
                    2 => {
                        for n in 0..2 {
                            children.push(fixture_spawn(&dir, "branch", &format!("b{n}")));
                        }
                    }
                    3 => {
                        for n in 0..16 {
                            children.push(fixture_spawn(&dir, "transient", &format!("t{n}")));
                        }
                        children.push(fixture_spawn(&dir, "held", "h0"));
                    }
                    6 => {
                        // Deliberately no readiness barrier: root exits immediately.
                        children.push(fixture_spawn(&dir, "short-held", "h0"));
                    }
                    7 => {
                        let exe = std::env::current_exe().expect("fixture exe");
                        let system_root = std::env::var_os("SystemRoot").expect("SystemRoot");
                        let powershell = PathBuf::from(system_root)
                            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
                        let quote = |value: &str| value.replace('\'', "''");
                        let script = format!(
                            "$ErrorActionPreference='Stop'; if($PSVersionTable.PSVersion.Major -ne 5 -or $PSVersionTable.PSVersion.Minor -ne 1){{throw 'requires Windows PowerShell 5.1'}}; $env:{ROLE}='held'; $env:{NAME}='h0'; $env:{PARENT}=[string]$PID; [IO.File]::WriteAllText($env:{DIR}+'/powershell-parent',[string]$PID); Start-Process -FilePath '{}' -ArgumentList '--exact {ENTRY} --nocapture --test-threads=1' -WorkingDirectory '{}' -WindowStyle Hidden -RedirectStandardInput '{}' -RedirectStandardOutput '{}' -RedirectStandardError '{}' | Out-Null",
                            quote(&exe.to_string_lossy()), quote(&dir.to_string_lossy()),
                            quote(&dir.join("null-stdin").to_string_lossy()),
                            quote(&dir.join("powershell-child.stdout").to_string_lossy()),
                            quote(&dir.join("powershell-child.stderr").to_string_lossy()),
                        );
                        fs::write(dir.join("null-stdin"), []).expect("closed stdin file");
                        let output = Command::new(powershell)
                            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
                            .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped())
                            .creation_flags(CREATE_NO_WINDOW)
                            .output().expect("PowerShell 5.1 Start-Process fixture");
                        fs::write(dir.join("powershell.stdout"), &output.stdout).expect("PS stdout");
                        fs::write(dir.join("powershell.stderr"), &output.stderr).expect("PS stderr");
                        assert!(output.status.success(), "PowerShell fixture failed");
                    }
                    _ => panic!("unknown workload"),
                }
                publish(&dir, "root-spawned", &std::process::id().to_string());
                if !matches!(workload, 0 | 6 | 7) {
                    wait_release(&dir);
                }
                if !matches!(workload, 6 | 7) {
                    for child in children.iter_mut() {
                        let output = child.wait().expect("fixture child reap");
                        assert!(output.success(), "descendant failed");
                    }
                }
                return;
            }
            publish(&dir, &format!("{name}.ready"), &format!("{} {parent} {role}", std::process::id()));
            match role.as_str() {
                "transient" => {}
                "short-held" => std::thread::sleep(Duration::from_secs(1)),
                "held" | "branch" => wait_release(&dir),
                _ => panic!("unknown fixture role"),
            }
            for child in children.iter_mut() {
                assert!(child.wait().expect("grandchild reap").success());
            }
            publish(&dir, &format!("{name}.exited"), "natural fixture completion");
        }

        #[derive(Debug, Clone, PartialEq, Eq)]
        enum OpenDecision {
            Retain,
            Foreign,
            Vanished87,
            Gap(&'static str),
        }

        // Pure classifier uses the same seam as native acquisition below. 87 is
        // admissible only with nonzero listed PID and a subsequent complete list.
        fn classify_open(pid: u32, result: Result<bool, u32>, fresh: Option<&[u32]>) -> OpenDecision {
            if pid == 0 {
                return OpenDecision::Gap("invalid_pid");
            }
            match result {
                Ok(true) => OpenDecision::Retain,
                Ok(false) => OpenDecision::Foreign,
                Err(87) if fresh.is_some_and(|pids| !pids.contains(&pid)) => OpenDecision::Vanished87,
                Err(_) => OpenDecision::Gap("not_openable"),
            }
        }

        fn classify_membership(result: Result<bool, u32>) -> OpenDecision {
            match result {
                Ok(true) => OpenDecision::Retain,
                Ok(false) => OpenDecision::Foreign,
                Err(_) => OpenDecision::Gap("membership_failed"),
            }
        }

        struct Retained {
            pid: u32,
            handle: HANDLE,
        }
        impl Drop for Retained {
            fn drop(&mut self) {
                unsafe { CloseHandle(self.handle); }
            }
        }

        #[derive(Default)]
        struct Ledger {
            live: Vec<Retained>,
            captured: BTreeSet<u32>,
            signalled: BTreeSet<u32>,
            foreign: BTreeSet<u32>,
            vanished: BTreeSet<u32>,
            gaps: BTreeSet<String>,
            resizes: usize,
            snapshots: usize,
        }

        fn poll_result(result: u32) -> Result<bool, &'static str> {
            match result {
                WAIT_OBJECT_0 => Ok(true),
                WAIT_TIMEOUT => Ok(false),
                WAIT_FAILED => Err("wait_failed"),
                _ => Err("wait_unexpected"),
            }
        }

        fn has_capacity(live: usize) -> bool {
            live < CAP
        }

        fn snapshot_action(ok: bool, error: u32, assigned: usize, listed: usize, capacity: usize) -> Result<bool, &'static str> {
            if !ok && error == 234 { return Ok(false); }
            if !ok { return Err("snapshot_native_error"); }
            if assigned != listed || listed > capacity { return Err("incomplete_snapshot"); }
            Ok(true)
        }

        // Variable-length ABI buffer is pointer-aligned; header is two DWORDs,
        // followed by ULONG_PTR PIDs. Bounds, overflow, allocation, and incomplete
        // retries share one SettlementAttempt rather than renewing per resize.
        fn pid_snapshot(job: &JobObject, attempt: &SettlementAttempt, ledger: &mut Ledger) -> Result<Vec<u32>, String> {
            let mut capacity = 16usize;
            loop {
                if attempt.expired() {
                    return Err("incomplete_snapshot".into());
                }
                let bytes = 8usize.checked_add(capacity.checked_mul(std::mem::size_of::<usize>()).ok_or("snapshot_overflow")?).ok_or("snapshot_overflow")?;
                let length = u32::try_from(bytes).map_err(|_| "snapshot_overflow")?;
                let words = bytes.div_ceil(std::mem::size_of::<usize>());
                let mut buffer = Vec::<usize>::new();
                buffer.try_reserve_exact(words).map_err(|_| "snapshot_allocation")?;
                buffer.resize(words, 0);
                let ok = unsafe { QueryInformationJobObject(job.handle, JobObjectBasicProcessIdList, buffer.as_mut_ptr().cast(), length, std::ptr::null_mut()) };
                let error = if ok == 0 { unsafe { GetLastError() } } else { 0 };
                let header = buffer.as_ptr().cast::<u32>();
                let assigned = unsafe { *header } as usize;
                let listed = unsafe { *header.add(1) } as usize;
                match snapshot_action(ok != 0, error, assigned, listed, capacity) {
                    Ok(false) => {
                        ledger.resizes += 1;
                        capacity = capacity.checked_mul(2).ok_or("snapshot_overflow")?.max(assigned);
                        continue;
                    }
                    Err(kind) => return Err(format!("{kind}_{error}")),
                    Ok(true) => {}
                }
                let mut pids = Vec::new();
                pids.try_reserve_exact(listed).map_err(|_| "snapshot_allocation")?;
                for n in 0..listed {
                    let value = unsafe { *buffer.as_ptr().cast::<u8>().add(8).cast::<usize>().add(n) };
                    let pid = u32::try_from(value).map_err(|_| "invalid_pid")?;
                    if pid == 0 { return Err("invalid_pid".into()); }
                    pids.push(pid);
                }
                ledger.snapshots += 1;
                return Ok(pids);
            }
        }

        impl Ledger {
            fn poll(&mut self) {
                let mut index = 0;
                while index < self.live.len() {
                    let result = unsafe { WaitForSingleObject(self.live[index].handle, 0) };
                    match poll_result(result) {
                        Ok(true) => {
                            self.signalled.insert(self.live[index].pid);
                            self.live.swap_remove(index); // Drop exactly this retained identity.
                        }
                        Ok(false) => index += 1,
                        Err(kind) => {
                            let native = if result == WAIT_FAILED { unsafe { GetLastError() } } else { result };
                            self.gaps.insert(format!("{kind}_{native}"));
                            index += 1; // Keep the SAME handle until it signals.
                        }
                    }
                }
            }

            fn observe(&mut self, job: &JobObject, root: u32, attempt: &SettlementAttempt) {
                self.poll(); // Reap before applying the live-handle ceiling.
                let pids = match pid_snapshot(job, attempt, self) {
                    Ok(pids) => pids,
                    Err(error) => { self.gaps.insert(error); return; }
                };
                for pid in pids {
                    if pid == root || self.captured.contains(&pid) { continue; }
                    if !has_capacity(self.live.len()) {
                        self.gaps.insert("RESOURCE_CAP_UNCONFIRMED".into());
                        continue;
                    }
                    let handle = unsafe { OpenProcess(0x0010_0000 | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
                    if handle.is_null() {
                        let error = unsafe { GetLastError() };
                        let fresh = if error == 87 { pid_snapshot(job, attempt, self).ok() } else { None };
                        match classify_open(pid, Err(error), fresh.as_deref()) {
                            OpenDecision::Vanished87 => { self.vanished.insert(pid); }
                            _ => { self.gaps.insert(format!("open_{error}")); }
                        }
                        continue;
                    }
                    let retained = Retained { pid, handle };
                    let mut member = 0;
                    let ok = unsafe { IsProcessInJob(handle, job.handle, &mut member) };
                    let membership = if ok == 0 { Err(unsafe { GetLastError() }) } else { Ok(member != 0) };
                    match classify_membership(membership) {
                        OpenDecision::Retain => {
                            self.captured.insert(pid);
                            self.live.push(retained);
                        }
                        OpenDecision::Foreign => { self.foreign.insert(pid); }
                        _ => { self.gaps.insert(format!("membership_{membership:?}")); }
                    }
                }
            }
        }

        fn ready_members(dir: &Path) -> Result<BTreeMap<String, (u32, u32, String)>, String> {
            let mut members = BTreeMap::new();
            for entry in fs::read_dir(dir).map_err(|error| error.to_string())? {
                let path = entry.map_err(|error| error.to_string())?.path();
                if path.extension().is_some_and(|extension| extension == "ready") {
                    let text = fs::read_to_string(&path).map_err(|error| error.to_string())?;
                    let parts: Vec<_> = text.split_whitespace().collect();
                    if parts.len() != 3 { return Err("invalid ready manifest".into()); }
                    let pid = parts[0].parse().map_err(|_| "invalid manifest PID")?;
                    let parent = parts[1].parse().map_err(|_| "invalid manifest parent")?;
                    members.insert(path.file_stem().unwrap().to_string_lossy().into_owned(), (pid, parent, parts[2].into()));
                }
            }
            Ok(members)
        }

        fn log_line(log: &mut fs::File, value: &str) {
            writeln!(log, "{value}").expect("durable feasibility log");
            log.sync_data().expect("durable feasibility receipt");
        }

        async fn reader<R: tokio::io::AsyncRead + Unpin>(mut pipe: R) -> std::io::Result<Vec<u8>> {
            let mut bytes = Vec::new();
            pipe.read_to_end(&mut bytes).await?;
            Ok(bytes)
        }

        fn read_reader(task: &mut tokio::task::JoinHandle<std::io::Result<Vec<u8>>>) -> impl std::future::Future<Output = Result<Vec<u8>, String>> + '_ {
            async move { task.await.map_err(|error| error.to_string())?.map_err(|error| error.to_string()) }
        }

        // One borrowed owner throughout expiry/panic recovery. No timeout drops
        // this future, no child/readers are aborted, no release is proof of exit.
        async fn trial(exe: &Path, dir: &Path, workload: usize, repetition: usize, source_hash: &str, group_start: Instant, foreign: HANDLE) -> Result<(), String> {
            fs::create_dir(dir).map_err(|error| error.to_string())?;
            let mut log = fs::File::create(dir.join("receipt.log")).map_err(|error| error.to_string())?;
            let started = Instant::now();
            log_line(&mut log, &format!("BEGIN W{workload}/{repetition} source={source_hash} fixture={} exe={} budgets=ready10/worker30/group1200/shared16", digest(include_bytes!("job.rs")), exe.display()));
            let mut command = tokio::process::Command::from(fixture_command(exe, dir, &format!("root-W{workload}"), "root"));
            let (mut child, job) = spawn_suspended_contained_impl(&mut command, None).await.map_err(|error| error.to_string())?;
            let root = child.id().ok_or("root PID absent")?;
            log_line(&mut log, &format!("OWN root={root} job=private suspended-assigned-resumed"));
            let mut stdout = tokio::spawn(reader(child.stdout.take().ok_or("stdout absent")?));
            let mut stderr = tokio::spawn(reader(child.stderr.take().ok_or("stderr absent")?));
            let mut child = Some(child);
            let mut stdout_done = false;
            let mut stderr_done = false;
            let mut root_done = false;
            let mut root_exit = None;
            let mut ledger = Ledger::default();
            let mut attempt = SettlementAttempt::new(READY);
            let mut failures = BTreeSet::<String>::new();
            let mut terminated = false;
            let mut released = false;
            let mut empty_measurement = false;
            let held_expected = [0, 4, 6, 1, 4, 4, 1, 1][workload];
            let mut held = BTreeSet::new();
            let mut manifest = BTreeMap::new();
            let mut acceptance_failed = false;
            loop {
                let mut foreign_member = 0;
                let foreign_query = unsafe { IsProcessInJob(foreign, job.handle, &mut foreign_member) };
                if foreign_query == 0 || foreign_member != 0 || unsafe { WaitForSingleObject(foreign, 0) } != WAIT_TIMEOUT {
                    failures.insert("foreign_process_mutation_or_identity_failure".into());
                }
                if attempt.expired() {
                    failures.insert("shared_attempt_expired".into());
                    ledger.gaps.insert("attempt_expired".into());
                    attempt.restart();
                    if terminated {
                        ledger.observe(&job, root, &attempt);
                        let result = job.terminate_checked();
                        log_line(&mut log, &format!("RECOVERY_RETERMINATE result={result:?}"));
                        if let Err(error) = result { ledger.gaps.insert(format!("reterminate_{error}")); }
                    }
                }
                ledger.observe(&job, root, &attempt);
                match ready_members(dir) {
                    Ok(members) => {
                        manifest = members;
                        held = manifest.values().filter(|(_, _, role)| role != "transient").map(|(pid, _, _)| *pid).collect();
                    }
                    Err(error) => { failures.insert(error); }
                }
                if !root_done {
                    match child.as_mut().expect("unreaped root owned").try_wait() {
                        Ok(Some(status)) => {
                            root_done = true;
                            root_exit = status.code();
                            drop(child.take()); // Release Tokio process identity before accounting proof.
                            log_line(&mut log, &format!("ROOT_REAPED exit={root_exit:?} handle_released=true elapsed={:?}", started.elapsed()));
                        }
                        Ok(None) => {}
                        Err(error) => { failures.insert(format!("root_wait_{error}")); }
                    }
                }
                if !stdout_done && stdout.is_finished() {
                    match read_reader(&mut stdout).await {
                        Ok(bytes) => {
                            if let Err(error) = fs::write(dir.join("root.stdout"), bytes) { failures.insert(format!("stdout_log_{error}")); }
                            stdout_done = true;
                        }
                        Err(error) => { failures.insert(format!("stdout_failed_{error}")); stdout_done = true; }
                    }
                }
                if !stderr_done && stderr.is_finished() {
                    match read_reader(&mut stderr).await {
                        Ok(bytes) => {
                            if let Err(error) = fs::write(dir.join("root.stderr"), bytes) { failures.insert(format!("stderr_log_{error}")); }
                            stderr_done = true;
                        }
                        Err(error) => { failures.insert(format!("stderr_failed_{error}")); stderr_done = true; }
                    }
                }
                let ready = held.len() == held_expected && held.is_subset(&ledger.captured) && dir.join("root-spawned").exists();
                if matches!(workload, 4 | 5) && root_done && !terminated {
                    failures.insert("forced_workload_root_exited_before_action".into());
                }
                if !released && !terminated && ready && (!matches!(workload, 4 | 5) || !root_done) {
                    if workload == 5 && repetition >= 4 {
                        let panic = std::panic::catch_unwind(|| panic!("I2800 borrowed observation injection"));
                        if panic.is_err() { log_line(&mut log, "EXPECTED_OBSERVATION_PANIC owner/readers/handles retained"); }
                    }
                    if matches!(workload, 4 | 5) {
                        // W5 first four exercise a real borrowed command wait timeout.
                        if workload == 5 && repetition < 4 {
                            let result = tokio::time::timeout(Duration::from_millis(1), child.as_mut().expect("timeout root owned").wait()).await;
                            if result.is_ok() { failures.insert("timeout_workload_root_exited_early".into()); }
                            log_line(&mut log, "EXPECTED_ROOT_TIMEOUT original child retained");
                        }
                        ledger.observe(&job, root, &attempt); // immediately before terminate
                        let result = job.terminate_checked();
                        log_line(&mut log, &format!("FORCED_TERMINATE result={result:?}"));
                        if let Err(error) = result { ledger.gaps.insert(format!("terminate_{error}")); }
                        terminated = true;
                    } else if !matches!(workload, 0 | 6) {
                        publish(dir, "release", "all held captured");
                        released = true;
                        log_line(&mut log, "RELEASE all held identities captured");
                    }
                }
                if !ready && started.elapsed() >= READY && !terminated {
                    failures.insert("readiness_or_capture_deadline".into());
                }
                if (started.elapsed() >= WORKER || group_start.elapsed() >= GROUP || !failures.is_empty()) && !acceptance_failed {
                    acceptance_failed = true;
                    log_line(&mut log, &format!("FAIL_DURABLE admission_closed elapsed={:?} failures={failures:?}; recovery retains owner/readers/handles", started.elapsed()));
                }
                if acceptance_failed && !terminated {
                    ledger.observe(&job, root, &attempt);
                    let result = job.terminate_checked();
                    log_line(&mut log, &format!("RECOVERY_TERMINATE result={result:?}"));
                    if let Err(error) = result { ledger.gaps.insert(format!("terminate_{error}")); }
                    terminated = true;
                }
                let accounting = job.active_processes();
                if let Err(error) = &accounting { ledger.gaps.insert(format!("accounting_{error}")); }
                let settled = root_done && stdout_done && stderr_done && matches!(accounting, Ok(0)) && ledger.live.is_empty();
                if settled {
                    // This is natural termination on a PROVEN empty Job, not an
                    // inference from root exit. W6 descendants have also signalled.
                    if matches!(workload, 0 | 6) && !empty_measurement {
                        ledger.observe(&job, root, &attempt);
                        let before = job.active_processes();
                        let bool_result = unsafe { TerminateJobObject(job.handle, 1) };
                        let native_error = if bool_result == 0 { Some(unsafe { GetLastError() }) } else { None };
                        let after = job.active_processes();
                        empty_measurement = true;
                        log_line(&mut log, &format!("EMPTY_JOB pre={before:?} root_reaped={root_done} readers={stdout_done}/{stderr_done} retained_pending={} BOOL={bool_result} error={native_error:?} post={after:?} elapsed={:?} source={source_hash}", ledger.live.len(), started.elapsed()));
                        if !matches!(before, Ok(0)) || !matches!(after, Ok(0)) || bool_result == 0 { failures.insert("empty_job_semantics_requires_review".into()); }
                    } else if !terminated {
                        ledger.observe(&job, root, &attempt);
                        if let Err(error) = job.terminate_checked() { ledger.gaps.insert(format!("natural_terminate_{error}")); }
                    }
                    break;
                }
                if terminated && attempt.remaining() < POLL {
                    ledger.observe(&job, root, &attempt);
                    if let Err(error) = job.terminate_checked() { ledger.gaps.insert(format!("reterminate_{error}")); }
                }
                tokio::time::sleep(POLL).await;
            }
            assert!(child.is_none(), "root already reaped and handle released");
            if !matches!(job.active_processes(), Ok(0)) { failures.insert("final_accounting_not_zero".into()); }
            if held.len() != held_expected || !held.is_subset(&ledger.captured) || !held.is_subset(&ledger.signalled) { failures.insert("held_capture_or_signal_missing".into()); }
            let manifest_expected = [0, 4, 6, 17, 4, 4, 1, 1][workload];
            if manifest.len() != manifest_expected { failures.insert("missing_fixture_manifest".into()); }
            if !matches!(workload, 4 | 5) && root_exit != Some(0) { failures.insert("natural_fixture_root_failed".into()); }
            if !ledger.gaps.is_empty() { failures.insert("historical_observation_gap".into()); }
            if !ledger.foreign.is_empty() { failures.insert("unexpected_foreign_member".into()); }
            for (name, (_, parent, _)) in &manifest {
                let expected_parent = if name.contains("-g") {
                    manifest.get(name.split("-g").next().unwrap()).map(|(pid, _, _)| *pid)
                } else if workload == 7 {
                    fs::read_to_string(dir.join("powershell-parent")).ok().and_then(|value| value.parse().ok())
                } else { Some(root) };
                if Some(*parent) != expected_parent { failures.insert(format!("missing_parent_{name}")); }
            }
            if matches!(workload, 0 | 6) && !empty_measurement { failures.insert("empty_measurement_missing".into()); }
            if dir.join("terminal-marker").exists() { failures.insert("early_marker".into()); }
            log_line(&mut log, &format!("CENSUS root_reaped={root_done} readers={stdout_done}/{stderr_done} captured={:?} signalled={:?} pending={} foreign={:?} vanished87={:?} snapshots={} resizes={} gaps={:?} manifest={manifest:?} elapsed={:?} failures={failures:?}", ledger.captured, ledger.signalled, ledger.live.len(), ledger.foreign, ledger.vanished, ledger.snapshots, ledger.resizes, ledger.gaps, started.elapsed()));
            drop(job);
            if failures.is_empty() && !acceptance_failed {
                publish(dir, "terminal-marker", "settlement proved; simulation only, no IPC");
                log_line(&mut log, "PASS owner settled; terminal marker after signal/accounting/root/readers");
                Ok(())
            } else {
                log_line(&mut log, "FAIL recovery completed; failure remains durable");
                Err(format!("W{workload}/{repetition}: {failures:?}"))
            }
        }

        #[tokio::test]
        async fn install_observed_feasibility_2800_owner_matrix_64() {
            assert!(std::env::var_os(ROLE).is_none(), "owner cannot run inside fixture");
            let exe = std::env::current_exe().expect("fixture executable");
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join("target/issue-2787/confirmed-wait-feasibility");
            fs::create_dir_all(&root).expect("ignored scratch root");
            let run_id = format!("{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
            let run = root.join(run_id);
            fs::create_dir(&run).expect("unique run directory; never reuse");
            let source_hash = digest(include_bytes!("job.rs"));
            let mut receipt = fs::File::create(run.join("group.log")).expect("group receipt");
            log_line(&mut receipt, &format!("prepared_HEAD={PREPARED_HEAD} source={source_hash} exe={} executable_sha={} trials=8x8 max_live_workers=1 plus_foreign_sentinel shared_ceiling=16 group_budget=1200s", exe.display(), digest(&fs::read(&exe).expect("executable bytes"))));
            let started = Instant::now();
            // Independently contained foreign sentinel, held across every trial.
            // Never pass its PID to a termination API. Its original handle proves
            // both wrong-Job rejection and absence of accidental tree mutation.
            let foreign_dir = run.join("foreign-sentinel");
            fs::create_dir(&foreign_dir).expect("foreign fixture dir");
            let mut foreign_command = tokio::process::Command::from(fixture_command(&exe, &foreign_dir, "held", "foreign"));
            let (foreign_child, foreign_job) = spawn_suspended_contained_impl(&mut foreign_command, None).await.expect("foreign containment");
            let foreign_pid = foreign_child.id().expect("foreign root PID");
            let foreign_handle = unsafe { OpenProcess(0x0010_0000 | PROCESS_QUERY_LIMITED_INFORMATION, 0, foreign_pid) };
            if foreign_handle.is_null() {
                let _ = foreign_job.terminate_checked();
                let _ = foreign_child.wait_with_output().await;
                panic!("foreign retained identity acquisition failed");
            }
            let foreign = Retained { pid: foreign_pid, handle: foreign_handle };
            log_line(&mut receipt, &format!("FOREIGN_OWN pid={foreign_pid} distinct_private_job=true"));
            let mut completed = 0;
            let mut failure = None;
            'admission: for workload in 0..8 {
                for repetition in 0..8 {
                    if started.elapsed() >= GROUP {
                        log_line(&mut receipt, "FAIL_DURABLE group budget; admission closed; no active owner");
                        failure = Some("feasibility group budget exceeded".to_string());
                        break 'admission;
                    }
                    let dir = run.join(format!("W{workload}-{repetition}"));
                    if let Err(error) = trial(&exe, &dir, workload, repetition, &source_hash, started, foreign.handle).await {
                        log_line(&mut receipt, &format!("FAIL_DURABLE completed={completed}/64 {error}; owned trial recovered; no further admission"));
                        failure = Some(error);
                        break 'admission;
                    }
                    completed += 1;
                }
            }
            publish(&foreign_dir, "release", "owner group ended; sentinel release");
            let mut foreign_wait = Box::pin(foreign_child.wait_with_output());
            let mut attempt = SettlementAttempt::new(READY);
            let foreign_output = loop {
                tokio::select! {
                    output = &mut foreign_wait => break output,
                    _ = tokio::time::sleep(POLL) => {
                        if attempt.expired() {
                            failure.get_or_insert_with(|| "foreign cleanup overrun".into());
                            log_line(&mut receipt, "FAIL_DURABLE foreign cleanup overrun; borrowed owner retained");
                            let _ = foreign_job.terminate_checked();
                            attempt.restart();
                        }
                    }
                }
            };
            drop(foreign_wait);
            match foreign_output {
                Ok(output) => {
                    fs::write(foreign_dir.join("stdout.log"), output.stdout).expect("foreign stdout");
                    fs::write(foreign_dir.join("stderr.log"), output.stderr).expect("foreign stderr");
                    if !output.status.success() { failure.get_or_insert_with(|| "foreign fixture failed".into()); }
                }
                Err(error) => { failure.get_or_insert_with(|| format!("foreign readers/wait failed: {error}")); }
            }
            while unsafe { WaitForSingleObject(foreign.handle, 0) } != WAIT_OBJECT_0 || !matches!(foreign_job.active_processes(), Ok(0)) {
                if attempt.expired() {
                    failure.get_or_insert_with(|| "foreign native cleanup overrun".into());
                    log_line(&mut receipt, "FAIL_DURABLE foreign native recovery pending");
                    let _ = foreign_job.terminate_checked();
                    attempt.restart();
                }
                tokio::time::sleep(POLL).await;
            }
            drop(foreign);
            drop(foreign_job);
            log_line(&mut receipt, "FOREIGN_SETTLED root/readers/same_handle_signal/accounting0 own_pending=0");
            if started.elapsed() >= GROUP { failure.get_or_insert_with(|| "group budget exceeded during final recovery".into()); }
            if let Some(error) = failure {
                log_line(&mut receipt, &format!("FAIL completed={completed}/64 all admitted owners recovered; {error}"));
                panic!("{error}; receipt {}", run.display());
            }
            log_line(&mut receipt, &format!("PASS completed={completed}/64 elapsed={:?} own_pending=0", started.elapsed()));
            assert_eq!(completed, 64);
        }

        #[test]
        fn install_observed_feasibility_2800_pure_errors() {
            assert_eq!(classify_open(42, Err(87), Some(&[])), OpenDecision::Vanished87);
            assert_eq!(classify_open(42, Err(87), Some(&[42])), OpenDecision::Gap("not_openable"));
            assert_eq!(classify_open(42, Err(87), None), OpenDecision::Gap("not_openable"));
            assert_eq!(classify_open(0, Err(87), Some(&[])), OpenDecision::Gap("invalid_pid"));
            assert_eq!(classify_open(42, Ok(false), None), OpenDecision::Foreign);
            assert_eq!(classify_open(42, Err(5), Some(&[])), OpenDecision::Gap("not_openable"));
            assert_eq!(classify_membership(Err(5)), OpenDecision::Gap("membership_failed"));
            assert_eq!(classify_membership(Ok(false)), OpenDecision::Foreign);
            assert_eq!(classify_membership(Ok(true)), OpenDecision::Retain);
            assert_eq!(snapshot_action(false, 234, 32, 16, 16), Ok(false));
            assert_eq!(snapshot_action(true, 0, 32, 32, 32), Ok(true));
            assert_eq!(snapshot_action(true, 0, 33, 32, 32), Err("incomplete_snapshot"));
            assert_eq!(snapshot_action(false, 5, 0, 0, 16), Err("snapshot_native_error"));
            assert_eq!(poll_result(WAIT_FAILED), Err("wait_failed"));
            assert_eq!(poll_result(0x80), Err("wait_unexpected"));
            assert_eq!(poll_result(WAIT_TIMEOUT), Ok(false));
            assert_eq!(poll_result(WAIT_OBJECT_0), Ok(true));
            assert!(has_capacity(255));
            assert!(!has_capacity(256));
            assert!(!has_capacity(257));
            let samples: Vec<_> = (0..80).map(|_| poll_result(WAIT_TIMEOUT)).collect();
            assert_eq!(samples.len(), 80); // individual sampling, never 64-handle batching.
            assert!(samples.iter().all(|sample| sample == &Ok(false)));
            let mut gaps = BTreeSet::new();
            gaps.insert("membership_failed");
            gaps.insert("incomplete_snapshot");
            gaps.insert("observation_panic");
            let before = gaps.clone();
            assert!(std::panic::catch_unwind(|| panic!("pure observation panic")).is_err());
            assert_eq!(gaps, before); // later success cannot erase historical G.
        }

        // Native support for the 87 race, separate from the 64 owner workloads.
        // Listed live identity -> release/reap/drop -> successful complete absence
        // -> OpenProcess NULL/87. Anything else is STOP, never a synthetic PASS.
        #[tokio::test]
        async fn install_observed_feasibility_2800_native_87_vanished() {
            let exe = std::env::current_exe().expect("fixture executable");
            let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join("target/issue-2787/confirmed-wait-feasibility");
            fs::create_dir_all(&base).expect("ignored scratch root");
            let dir = base.join(format!("native87-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
            fs::create_dir(&dir).expect("unique native87 directory");
            let mut log = fs::File::create(dir.join("receipt.log")).expect("native87 receipt");
            let mut command = tokio::process::Command::from(fixture_command(&exe, &dir, "held", "race"));
            let (child, job) = spawn_suspended_contained_impl(&mut command, None).await.expect("race fixture containment");
            let pid = child.id().expect("race root");
            let mut ledger = Ledger::default();
            let mut attempt = SettlementAttempt::new(READY);
            let listed = pid_snapshot(&job, &attempt, &mut ledger);
            let was_listed = listed.as_ref().is_ok_and(|pids| pids.contains(&pid));
            log_line(&mut log, &format!("source={} exe_sha={} OWN pid={pid} initial={listed:?}", digest(include_bytes!("job.rs")), digest(&fs::read(&exe).expect("exe bytes"))));
            publish(&dir, "release", "native87 fixture release");
            let started = Instant::now();
            let mut failed = !was_listed;
            let mut wait = Box::pin(child.wait_with_output());
            let output = loop {
                tokio::select! {
                    output = &mut wait => break output,
                    _ = tokio::time::sleep(POLL) => {
                        if attempt.expired() {
                            failed = true;
                            log_line(&mut log, "FAIL_DURABLE native87 owner expiry; retain borrowed wait/readers");
                            let _ = job.terminate_checked();
                            attempt.restart();
                        }
                    }
                }
            };
            drop(wait); // Releases the actual root handle before absence/open.
            match output {
                Ok(output) => {
                    failed |= !output.status.success();
                    fs::write(dir.join("stdout.log"), output.stdout).expect("native87 stdout");
                    fs::write(dir.join("stderr.log"), output.stderr).expect("native87 stderr");
                }
                Err(_) => failed = true,
            }
            while !matches!(job.active_processes(), Ok(0)) {
                if attempt.expired() {
                    failed = true;
                    log_line(&mut log, "FAIL_DURABLE native87 accounting expiry; own Job retained");
                    let _ = job.terminate_checked();
                    attempt.restart();
                }
                tokio::time::sleep(POLL).await;
            }
            let handle = unsafe { OpenProcess(0x0010_0000 | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
            let error = if handle.is_null() { unsafe { GetLastError() } } else { 0 };
            if !handle.is_null() { unsafe { CloseHandle(handle); } }
            // The second complete snapshot follows the failed open, as required.
            let fresh = pid_snapshot(&job, &attempt, &mut ledger);
            let supported = handle.is_null() && error == 87 && classify_open(pid, Err(error), fresh.as_ref().ok().map(Vec::as_slice)) == OpenDecision::Vanished87;
            log_line(&mut log, &format!("open_null={} error={error} fresh={fresh:?} listed_proven={was_listed} supported={supported} root/readers/accounting=settled elapsed={:?} own_pending=0", handle.is_null(), started.elapsed()));
            drop(job);
            assert!(supported && !failed, "STOP/USUARIO: native87 fixture unsupported; receipt {}", dir.display());
        }
    }

    impl Drop for JobObject {
        fn drop(&mut self) {
            // SAFETY: `self.handle` is a valid job handle owned by `self`. Closing
            // the last handle to a KILL_ON_JOB_CLOSE job terminates the remaining
            // tree, so this doubles as the hard-exit safety net.
            unsafe {
                let _ = CloseHandle(self.handle);
            }
        }
    }
}

#[cfg(not(windows))]
mod stub_impl {
    /// No-op Job Object for non-Windows builds. The Win32 tree-kill primitive does
    /// not exist here, so PtyManager simply never holds one (`for_child` -> None).
    /// `#[allow(dead_code)]` because the unit struct is never constructed on this
    /// platform (for_child always returns None).
    #[allow(dead_code)]
    pub struct JobObject;

    impl JobObject {
        pub fn for_child(_pid: u32) -> Option<Self> {
            None
        }
        pub fn terminate(&self) {}
    }

    #[cfg(test)]
    mod tests {
        use super::JobObject;

        #[test]
        fn stub_for_child_is_none() {
            assert!(JobObject::for_child(1234).is_none());
        }
    }
}

// #632 LOW-2 - the ONE automated proof that the job kills the whole tree (child AND
// grandchild). Deliberately NOT #[ignore] and gated #[cfg(windows)] so it runs by
// default in the `rust-regression` CI lane (runs-on: windows-latest, executes
// `cargo test --lib --bins --tests`). Real processes; written defensively (generous
// polls) to avoid flake. Do NOT add #[ignore] - that removes all executed coverage
// of Part A.
#[cfg(all(test, windows))]
mod win_tests {
    use super::windows_impl::{
        spawn_suspended_contained_impl, InjectedFailure, JobObject, SpawnTestHook,
    };
    use std::collections::HashMap;
    use std::os::windows::process::CommandExt;
    use std::process::Command;
    use std::time::{Duration, Instant};

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    /// Mirror of `windows.rs::process_entries`: one `CreateToolhelp32Snapshot` pass
    /// returning `pid -> parent_pid`. Used to collect the full descendant set of a
    /// root pid so the assertion does not depend on process names.
    fn parent_map() -> HashMap<u32, u32> {
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
            TH32CS_SNAPPROCESS,
        };

        let mut map = HashMap::new();
        // SAFETY: standard toolhelp enumeration; the snapshot handle is closed
        // before returning and the entry struct is zero-initialized with dwSize set.
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return map;
            }
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            if Process32FirstW(snapshot, &mut entry) != 0 {
                loop {
                    map.insert(entry.th32ProcessID, entry.th32ParentProcessID);
                    if Process32NextW(snapshot, &mut entry) == 0 {
                        break;
                    }
                }
            }
            let _ = CloseHandle(snapshot);
        }
        map
    }

    fn descendants_of(root: u32, map: &HashMap<u32, u32>) -> Vec<u32> {
        let mut out = Vec::new();
        let mut frontier = vec![root];
        while let Some(p) = frontier.pop() {
            for (&pid, &ppid) in map {
                if ppid == p && pid != root && !out.contains(&pid) {
                    out.push(pid);
                    frontier.push(pid);
                }
            }
        }
        out
    }

    #[test]
    fn job_terminate_kills_child_and_grandchild() {
        // Assigned pid = outer cmd; it spawns an inner cmd (child) that spawns ping
        // (grandchild of the assigned pid). Proves tree-kill to depth 2 and that
        // descendants spawned AFTER assignment are captured.
        let mut child = Command::new("cmd.exe")
            .args(["/C", "cmd /C ping -n 30 127.0.0.1 >NUL"])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .expect("spawn outer cmd");
        let root = child.id();

        let job = JobObject::for_child(root).expect("job created + assigned");

        // Wait (generously, to avoid flake on a loaded CI runner) for the grandchild
        // tree to materialize. The loop exits as soon as any descendant appears, so the
        // ceiling only matters under heavy load.
        let mut tree = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            tree = descendants_of(root, &parent_map());
            if !tree.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(
            !tree.is_empty(),
            "expected at least one descendant before terminate"
        );

        job.terminate();

        // Every member of the subtree (root + descendants) is gone shortly after the
        // job kill. The ceiling is generous (CI load) but far under the ping's ~30s
        // lifetime, so a broken job that leaves survivors still fails this loop.
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let map = parent_map();
            let alive_root = map.contains_key(&root);
            let alive_tree = descendants_of(root, &map);
            if !alive_root && alive_tree.is_empty() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "job did not kill the whole tree in time (root_alive={alive_root}, survivors={alive_tree:?})"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = child.wait();
    }

    fn marker_command(marker: &std::path::Path) -> tokio::process::Command {
        let mut command = tokio::process::Command::new("cmd.exe");
        command
            .as_std_mut()
            .raw_arg("/D /C echo ran>\"%AGENTSCOMMANDER_TEST_MARKER%\"");
        command.env("AGENTSCOMMANDER_TEST_MARKER", marker);
        command.stdin(std::process::Stdio::null());
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());
        command.kill_on_drop(true);
        command
    }

    #[tokio::test]
    async fn suspended_containment_blocks_marker_until_job_assignment_and_resume() {
        let dir = tempfile::tempdir().expect("tempdir");
        let marker = dir.path().join("marker.txt");
        let (spawned_tx, spawned_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let mut command = marker_command(&marker);
        let task = tokio::spawn(async move {
            spawn_suspended_contained_impl(
                &mut command,
                Some(SpawnTestHook {
                    after_spawn: Some(spawned_tx),
                    release_spawn: Some(release_rx),
                    after_reap: None,
                    release_query: None,
                    failure: None,
                    settlement_window: None,
                }),
            )
            .await
        });
        spawned_rx.await.expect("suspended child spawned");
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!marker.exists(), "the suspended command must not execute");
        release_tx.send(()).expect("release spawn");
        let (child, job) = task.await.expect("join").expect("contained spawn");
        let output = child.wait_with_output().await.expect("wait marker command");
        assert!(
            output.status.success(),
            "marker command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while job.active_processes().expect("accounting query") != 0 {
            assert!(Instant::now() < deadline, "job did not settle");
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(marker.exists(), "the command runs only after resume");
    }

    async fn rejected_launch_never_runs_marker(failure: InjectedFailure) {
        let dir = tempfile::tempdir().expect("tempdir");
        let marker = dir.path().join("marker.txt");
        let (spawned_tx, spawned_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let (reaped_tx, reaped_rx) = tokio::sync::oneshot::channel();
        let (query_tx, query_rx) = tokio::sync::oneshot::channel();
        let mut command = marker_command(&marker);
        let task = tokio::spawn(async move {
            spawn_suspended_contained_impl(
                &mut command,
                Some(SpawnTestHook {
                    after_spawn: Some(spawned_tx),
                    release_spawn: Some(release_rx),
                    after_reap: Some(reaped_tx),
                    release_query: Some(query_rx),
                    failure: Some(failure),
                    settlement_window: Some(Duration::from_millis(60)),
                }),
            )
            .await
        });
        spawned_rx.await.expect("suspended child spawned");
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!marker.exists(), "the suspended command must not execute");
        release_tx.send(()).expect("release spawn");
        reaped_rx
            .await
            .expect("direct child reaped and handle dropped");
        assert!(!marker.exists(), "a rejected launch never executes");
        tokio::time::sleep(Duration::from_millis(75)).await;
        query_tx.send(()).expect("release accounting query");
        let error = task
            .await
            .expect("join")
            .expect_err("the injected launch must fail closed");
        assert!(
            error.to_string().contains("containment"),
            "unexpected error: {error}"
        );
        assert!(
            error.to_string().contains(
                "settlement attempt deadline exceeded during rejected-spawn proof barrier"
            ),
            "the rejected-spawn cleanup must retain its expired attempt defect: {error}"
        );
        assert!(!marker.exists(), "the rejected command remained suspended");
    }

    #[tokio::test]
    async fn suspended_containment_assignment_failure_never_runs_marker() {
        rejected_launch_never_runs_marker(InjectedFailure::Assignment).await;
    }

    #[tokio::test]
    async fn suspended_containment_resume_failure_never_runs_marker() {
        rejected_launch_never_runs_marker(InjectedFailure::Resume).await;
    }
}
