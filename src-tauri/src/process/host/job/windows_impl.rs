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
    OpenProcess, OpenThread, ResumeThread, CREATE_NO_WINDOW, CREATE_SUSPENDED, PROCESS_SET_QUOTA,
    PROCESS_TERMINATE, THREAD_SUSPEND_RESUME,
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
                (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast::<core::ffi::c_void>(),
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
        let mut accounting: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: the output buffer has the exact information-class layout
        // and remains valid for the duration of the call.
        let ok = unsafe {
            QueryInformationJobObject(
                self.handle,
                JobObjectBasicAccountingInformation,
                (&mut accounting as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION)
                    .cast::<core::ffi::c_void>(),
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
                if entry.th32OwnerProcessID == pid && found.replace(entry.th32ThreadID).is_some() {
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
pub(crate) async fn with_contained_spawn_test_hook<F>(hook: SpawnTestHook, future: F) -> F::Output
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
        None => return Err(settle_rejected_spawn(child, None, "missing_process_id", hook).await),
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
        return Err(settle_rejected_spawn(child, Some(job), "resume_primary_thread", hook).await);
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
