//! #632 - per-agent Windows Job Object for reliable process-tree teardown.
//!
//! Each spawned agent's ConPTY child is assigned to its own Job Object created
//! with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. Terminating the job (explicitly via
//! `TerminateJobObject`, or implicitly when the last handle closes on a hard
//! process exit / panic) kills the entire descendant tree atomically. This is
//! immune to PID reuse and per-PID ACCESS_DENIED, and needs no process-snapshot
//! walking or per-process waits, so it closes the orphan failure mode in #632.
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

#[cfg(windows)]
pub use windows_impl::JobObject;

#[cfg(not(windows))]
pub use stub_impl::JobObject;

#[cfg(windows)]
mod windows_impl {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
    };

    /// Owns a Job Object handle. Dropping it closes the handle; because the job is
    /// created with KILL_ON_JOB_CLOSE and we hold the only handle, the OS kills the
    /// whole assigned tree on drop too (the hard-exit / panic safety net).
    pub struct JobObject {
        handle: HANDLE,
    }

    // A HANDLE is an opaque kernel handle, safe to use and close from any thread;
    // the value is only moved into PtyInstance and read back out under a Mutex.
    unsafe impl Send for JobObject {}
    unsafe impl Sync for JobObject {}

    impl JobObject {
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
            // SAFETY: every returned handle is null-checked before use; the limit
            // info struct is zero-initialized then fully written.
            unsafe {
                // NON-INHERITABLE handle: null SECURITY_ATTRIBUTES. See module invariant.
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    log::warn!("[pty] CreateJobObjectW failed for pid {pid}; reaper-only cleanup");
                    return None;
                }
                let job = JobObject { handle };

                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let set_ok = SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if set_ok == 0 {
                    log::warn!(
                        "[pty] SetInformationJobObject failed for pid {pid}; reaper-only cleanup"
                    );
                    return None; // `job` drops here -> handle closed
                }

                let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
                if process.is_null() {
                    log::warn!(
                        "[pty] OpenProcess(SET_QUOTA|TERMINATE) failed for pid {pid}; reaper-only cleanup"
                    );
                    return None;
                }
                let assigned = AssignProcessToJobObject(handle, process);
                let _ = CloseHandle(process);
                if assigned == 0 {
                    // Most likely the process is in a non-nestable job on a pre-Win8
                    // kernel; on Win8+ nested jobs make this succeed. Either way the
                    // reaper is the fallback (and the MED-2 residual applies).
                    log::warn!(
                        "[pty] AssignProcessToJobObject failed for pid {pid}; reaper-only cleanup"
                    );
                    return None;
                }
                log::info!("[pty] assigned pid {pid} to job object for tree-kill");
                Some(job)
            }
        }

        /// Terminate every process in the job. Idempotent; safe on an already-dead
        /// tree (TerminateJobObject just reports failure, which we log at debug).
        pub fn terminate(&self) {
            // SAFETY: `self.handle` is a valid job handle owned by `self`.
            let ok = unsafe { TerminateJobObject(self.handle, 1) };
            if ok == 0 {
                log::debug!("[pty] TerminateJobObject failed (tree likely already gone)");
            }
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
    use super::JobObject;
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
}
