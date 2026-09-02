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
