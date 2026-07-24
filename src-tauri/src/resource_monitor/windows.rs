use super::registry::{require_backend_time, ProcessTreeBackend, ResourceError};
use super::types::{
    ObservedProcess, ObservedProcessTree, ProcessIdentity, ProcessMemory, TerminateOutcome,
};

fn run_platform_observation_until<T, F>(
    deadline: std::time::Instant,
    operation: &'static str,
    pid: u32,
    observe: F,
) -> Result<T, ResourceError>
where
    T: Send + 'static,
    F: FnOnce(std::sync::Arc<std::sync::atomic::AtomicBool>) -> Result<T, ResourceError>
        + Send
        + 'static,
{
    use std::sync::atomic::{AtomicBool, Ordering};

    require_backend_time(deadline, operation, pid)?;
    let cancelled = std::sync::Arc::new(AtomicBool::new(false));
    let cancelled_for_worker = std::sync::Arc::clone(&cancelled);
    let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name(format!("resource-{operation}-{pid}"))
        .spawn(move || {
            #[cfg(test)]
            pause_platform_observation_if_injected(operation, pid);
            if cancelled_for_worker.load(Ordering::Acquire) {
                return;
            }
            let result = observe(std::sync::Arc::clone(&cancelled_for_worker));
            if !cancelled_for_worker.load(Ordering::Acquire) {
                let _ = result_tx.send(result);
            }
        })
        .map_err(|error| {
            ResourceError::Message(format!(
                "failed to start bounded {operation} worker for pid {pid}: {error}"
            ))
        })?;

    let remaining = deadline.saturating_duration_since(std::time::Instant::now());
    match result_rx.recv_timeout(remaining) {
        Ok(result) => result,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            cancelled.store(true, Ordering::Release);
            Err(ResourceError::Message(format!(
                "syscall={operation} deadline expired for pid {pid}"
            )))
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(ResourceError::Message(
            format!("bounded {operation} worker disconnected for pid {pid}"),
        )),
    }
}

#[cfg(test)]
struct PlatformObservationPause {
    operation: &'static str,
    pid: u32,
    reached: std::sync::mpsc::SyncSender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
fn platform_observation_pause() -> &'static std::sync::Mutex<Option<PlatformObservationPause>> {
    static PAUSE: std::sync::OnceLock<std::sync::Mutex<Option<PlatformObservationPause>>> =
        std::sync::OnceLock::new();
    PAUSE.get_or_init(|| std::sync::Mutex::new(None))
}

#[cfg(test)]
fn inject_platform_observation_pause(
    operation: &'static str,
    pid: u32,
    reached: std::sync::mpsc::SyncSender<()>,
    release: std::sync::mpsc::Receiver<()>,
) {
    let mut slot = platform_observation_pause()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    assert!(
        slot.is_none(),
        "only one platform observation pause may be active"
    );
    *slot = Some(PlatformObservationPause {
        operation,
        pid,
        reached,
        release,
    });
}

#[cfg(test)]
fn pause_platform_observation_if_injected(operation: &'static str, pid: u32) {
    let pause = {
        let mut slot = platform_observation_pause()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if slot
            .as_ref()
            .is_some_and(|pause| pause.operation == operation && pause.pid == pid)
        {
            slot.take()
        } else {
            None
        }
    };
    if let Some(pause) = pause {
        pause
            .reached
            .send(())
            .expect("signal native platform observation gate");
        pause
            .release
            .recv()
            .expect("release native platform observation gate");
    }
}

#[cfg(windows)]
mod platform {
    use std::collections::{HashMap, VecDeque};

    use super::*;
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_NO_MORE_FILES, FILETIME, HANDLE, INVALID_HANDLE_VALUE,
        WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetProcessTimes, OpenProcess, TerminateProcess, WaitForSingleObject,
        PROCESS_QUERY_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, PROCESS_VM_READ,
    };

    pub struct PlatformProcessTreeBackend;

    impl Default for PlatformProcessTreeBackend {
        fn default() -> Self {
            Self::new()
        }
    }

    impl PlatformProcessTreeBackend {
        pub fn new() -> Self {
            Self
        }
    }

    impl ProcessTreeBackend for PlatformProcessTreeBackend {
        fn observe_tree(
            &self,
            root: ProcessIdentity,
        ) -> Result<ObservedProcessTree, ResourceError> {
            let entries = process_entries()?;
            // #564 - resolve identities lazily for only the PIDs the walk visits
            // (root + descendants + their direct parents), not every system PID, and
            // memoize so a pid referenced both as a node and as a child's parent is
            // opened at most once and yields one consistent value across the walk
            // (matching the old single-snapshot identity map). The previous full-system
            // precompute opened ~498 processes (each a syscall, with a redundant full
            // snapshot on every permission-denied open) and discarded ~98% of the results.
            let mut identity_cache: HashMap<u32, Option<ProcessIdentity>> = HashMap::new();
            Ok(build_observed_tree(
                &entries,
                root,
                |pid| {
                    *identity_cache
                        .entry(pid)
                        .or_insert_with(|| observe_identity(pid).ok().flatten())
                },
                |pid| process_memory(pid).unwrap_or_default(),
            ))
        }

        fn observe_identity(&self, pid: u32) -> Result<Option<ProcessIdentity>, ResourceError> {
            observe_identity(pid)
        }

        fn observe_tree_until(
            &self,
            root: ProcessIdentity,
            deadline: std::time::Instant,
        ) -> Result<ObservedProcessTree, ResourceError> {
            run_platform_observation_until(deadline, "observe_tree", root.pid, move |_| {
                Self::new().observe_tree(root)
            })
        }

        fn observe_identity_until(
            &self,
            pid: u32,
            deadline: std::time::Instant,
        ) -> Result<Option<ProcessIdentity>, ResourceError> {
            run_platform_observation_until(deadline, "observe_identity", pid, move |_| {
                observe_identity(pid)
            })
        }

        fn terminate_verified(
            &self,
            process: &ObservedProcess,
        ) -> Result<TerminateOutcome, ResourceError> {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            terminate_verified_before(process, deadline)
        }

        fn terminate_verified_until(
            &self,
            process: &ObservedProcess,
            deadline: std::time::Instant,
        ) -> Result<TerminateOutcome, ResourceError> {
            terminate_verified_before(process, deadline)
        }

        fn current_process_memory(&self) -> Result<ProcessMemory, ResourceError> {
            let mut counters = unsafe { std::mem::zeroed::<PROCESS_MEMORY_COUNTERS_EX>() };
            counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
            let ok = unsafe {
                K32GetProcessMemoryInfo(
                    GetCurrentProcess(),
                    &mut counters as *mut _ as *mut PROCESS_MEMORY_COUNTERS,
                    counters.cb,
                )
            };
            if ok == 0 {
                return Err(last_error("K32GetProcessMemoryInfo failed"));
            }
            Ok(ProcessMemory {
                private_bytes: Some(counters.PrivateUsage as u64),
                working_set_bytes: Some(counters.WorkingSetSize as u64),
            })
        }
    }

    fn terminate_verified_before(
        process: &ObservedProcess,
        deadline: std::time::Instant,
    ) -> Result<TerminateOutcome, ResourceError> {
        let pid = process.identity.pid;
        let Some(current) =
            run_platform_observation_until(deadline, "terminate_identity", pid, move |_| {
                observe_identity(pid)
            })?
        else {
            return Ok(TerminateOutcome::AlreadyGone);
        };
        if current != process.identity {
            return Ok(TerminateOutcome::AlreadyGone);
        }
        if std::time::Instant::now() >= deadline {
            return Err(ResourceError::Message(format!(
                "terminate_verified deadline expired for pid {}",
                process.identity.pid
            )));
        }

        let access = PROCESS_TERMINATE | PROCESS_SYNCHRONIZE | PROCESS_QUERY_INFORMATION;
        let (handle, opened_identity) =
            match run_platform_observation_until(deadline, "open_process", pid, move |_| {
                let handle = open_process(pid, access)?;
                let identity = identity_from_handle(pid, &handle)?;
                Ok((handle, identity))
            }) {
                Ok(opened) => opened,
                Err(err) => {
                    return verify_identity_exited_until(
                        process.identity,
                        err.to_string(),
                        deadline,
                    );
                }
            };
        if opened_identity != process.identity {
            return Ok(TerminateOutcome::AlreadyGone);
        }
        let handle = match run_platform_observation_until(
            deadline,
            "TerminateProcess",
            pid,
            move |cancelled| {
                if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                    return Err(ResourceError::Message(format!(
                        "syscall=TerminateProcess deadline expired for pid {pid}"
                    )));
                }
                require_backend_time(deadline, "TerminateProcess", pid)?;
                let ok = unsafe { TerminateProcess(handle.raw(), 1) };
                if ok == 0 {
                    Err(last_error("TerminateProcess failed"))
                } else {
                    Ok(handle)
                }
            },
        ) {
            Ok(handle) => handle,
            Err(error) => {
                return verify_identity_exited_until(process.identity, error.to_string(), deadline);
            }
        };
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let wait_ms =
            u32::try_from(remaining.as_millis().min(u128::from(u32::MAX))).unwrap_or(u32::MAX);
        if wait_ms == 0 {
            return Err(ResourceError::Message(format!(
                "WaitForSingleObject deadline expired for pid {}",
                process.identity.pid
            )));
        }
        let wait_result = unsafe { WaitForSingleObject(handle.raw(), wait_ms) };
        let failure = match wait_result {
            WAIT_OBJECT_0 => None,
            WAIT_TIMEOUT => Some(format!(
                "timed out waiting for pid {} to exit before absolute deadline",
                process.identity.pid
            )),
            WAIT_FAILED => Some(last_error("WaitForSingleObject failed").to_string()),
            other => Some(format!(
                "unexpected WaitForSingleObject result {other} for pid {}",
                process.identity.pid
            )),
        };
        if let Some(failure) = failure {
            return verify_identity_exited_until(process.identity, failure, deadline);
        }
        verify_identity_exited_until(
            process.identity,
            format!(
                "pid {} is still alive after termination",
                process.identity.pid
            ),
            deadline,
        )
    }

    /// Walk the process snapshot from `root`, returning the observed subtree.
    ///
    /// Root-PID-reuse guard: the registry stores the root's creation time, so the
    /// live process at the root PID must prove it is that same process before we
    /// adopt its subtree. It can fail to prove it two ways. First (#516), its
    /// creation time is readable but differs, because the original root exited and a
    /// foreign process took the recycled PID. Second (#543), its identity cannot be
    /// read at all (creation_time 0), so a foreign process occupying the recycled PID
    /// could otherwise still expose a readable child that would inherit
    /// `kill_allowed = true`. In either case we drop the whole subtree and report it
    /// exactly like a missing root, so the registry never adopts, and later
    /// terminates, a process it does not own, and no descendant under an unverifiable
    /// root is ever kill-eligible. The guard only applies when the registry captured
    /// a real root creation time (`root.creation_time_100ns != 0`); the drop is
    /// re-evaluated on every observe cycle, so a root that is only briefly unreadable
    /// is re-adopted in full on the next cycle once it reads cleanly. Descendant
    /// identities are already verified at terminate time; this makes the root just as
    /// strict at observe time. Memory lookups are injected so the walk is a pure
    /// function over the snapshot and can be unit-tested without real processes.
    fn build_observed_tree(
        entries: &HashMap<u32, ProcessEntry>,
        root: ProcessIdentity,
        mut identity_for: impl FnMut(u32) -> Option<ProcessIdentity>,
        mut memory_for: impl FnMut(u32) -> ProcessMemory,
    ) -> ObservedProcessTree {
        let mut by_parent: HashMap<u32, Vec<u32>> = HashMap::new();
        for entry in entries.values() {
            by_parent
                .entry(entry.parent_pid)
                .or_default()
                .push(entry.pid);
        }

        let mut processes = Vec::new();
        let mut errors = Vec::new();
        let mut queue = VecDeque::from([(root.pid, 0_u32)]);
        while let Some((pid, depth)) = queue.pop_front() {
            let Some(entry) = entries.get(&pid) else {
                if pid == root.pid {
                    errors.push(format!("root pid {} was not in process snapshot", root.pid));
                }
                continue;
            };
            let resolved_identity = identity_for(pid);
            if pid == root.pid
                && root.creation_time_100ns != 0
                && resolved_identity.map(|id| id.creation_time_100ns)
                    != Some(root.creation_time_100ns)
            {
                // Live process at the root pid is not provably the one we
                // registered: its creation time is readable but differs (#516, the
                // pid was recycled by a foreign process) or its identity can't be
                // read at all (#543, creation_time 0 / unreadable). Either way, drop
                // the subtree and surface the same error the missing-root path uses,
                // so cleanup releases the slot instead of adopting a foreign process
                // and no readable descendant under an unverifiable root becomes
                // kill-eligible.
                errors.push(format!("root pid {} was not in process snapshot", root.pid));
                continue;
            }
            let identity = match resolved_identity {
                Some(identity) => identity,
                None => {
                    errors.push(format!("identity unavailable for pid {pid}"));
                    ProcessIdentity {
                        pid,
                        creation_time_100ns: 0,
                    }
                }
            };
            let parent_identity = (entry.parent_pid != 0)
                .then_some(entry.parent_pid)
                .and_then(&mut identity_for);
            let memory = memory_for(pid);
            processes.push(ObservedProcess {
                identity,
                parent_pid: (entry.parent_pid != 0).then_some(entry.parent_pid),
                parent_identity,
                exe_name: entry.exe_name.clone(),
                depth,
                private_bytes: memory.private_bytes,
                working_set_bytes: memory.working_set_bytes,
                cpu_percent: None,
                kill_allowed: identity.creation_time_100ns != 0,
            });

            if let Some(children) = by_parent.get(&pid) {
                for child in children {
                    queue.push_back((*child, depth.saturating_add(1)));
                }
            }
        }

        ObservedProcessTree { processes, errors }
    }

    struct ProcessEntry {
        pid: u32,
        parent_pid: u32,
        exe_name: String,
    }

    struct OwnedHandle(HANDLE);

    // SAFETY: Windows kernel handles are process-wide values. This wrapper owns
    // exactly one CloseHandle call and may be transferred to the deadline caller.
    unsafe impl Send for OwnedHandle {}

    impl OwnedHandle {
        fn raw(&self) -> HANDLE {
            self.0
        }
    }

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    fn process_entries() -> Result<HashMap<u32, ProcessEntry>, ResourceError> {
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(last_error("CreateToolhelp32Snapshot failed"));
        }
        let snapshot = OwnedHandle(snapshot);
        let mut entry = unsafe { std::mem::zeroed::<PROCESSENTRY32W>() };
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        // #559 (G1) - a valid snapshot always contains at least System Idle / System /
        // the calling process, so a zero return here is an enumeration FAILURE, not a
        // legitimately empty list. Returning Err keeps a failed/partial walk off the
        // reap path: the H1 nomination treats a missing root as "gone", so a spuriously
        // empty or truncated map must never be mistaken for dead roots.
        if unsafe { Process32FirstW(snapshot.raw(), &mut entry) } == 0 {
            return Err(last_error("Process32FirstW failed"));
        }
        let mut entries = HashMap::new();
        loop {
            let pid = entry.th32ProcessID;
            entries.insert(
                pid,
                ProcessEntry {
                    pid,
                    parent_pid: entry.th32ParentProcessID,
                    exe_name: exe_name(&entry.szExeFile),
                },
            );
            if unsafe { Process32NextW(snapshot.raw(), &mut entry) } == 0 {
                let code = unsafe { GetLastError() };
                if code == ERROR_NO_MORE_FILES {
                    break;
                }
                return Err(ResourceError::Message(format!(
                    "Process32NextW failed: win32 error {code}"
                )));
            }
        }
        Ok(entries)
    }

    fn observe_identity(pid: u32) -> Result<Option<ProcessIdentity>, ResourceError> {
        let handle = match open_process(pid, PROCESS_QUERY_INFORMATION) {
            Ok(handle) => handle,
            Err(err) => {
                return if pid_exists(pid)? { Err(err) } else { Ok(None) };
            }
        };
        let mut creation = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let mut exit = creation;
        let mut kernel = creation;
        let mut user = creation;
        let ok = unsafe {
            GetProcessTimes(
                handle.raw(),
                &mut creation,
                &mut exit,
                &mut kernel,
                &mut user,
            )
        };
        if ok == 0 {
            let err = last_error("GetProcessTimes failed");
            return if pid_exists(pid)? { Err(err) } else { Ok(None) };
        }
        Ok(Some(ProcessIdentity {
            pid,
            creation_time_100ns: filetime_to_u64(creation),
        }))
    }

    fn identity_from_handle(
        pid: u32,
        handle: &OwnedHandle,
    ) -> Result<ProcessIdentity, ResourceError> {
        let mut creation = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let mut exit = creation;
        let mut kernel = creation;
        let mut user = creation;
        let ok = unsafe {
            GetProcessTimes(
                handle.raw(),
                &mut creation,
                &mut exit,
                &mut kernel,
                &mut user,
            )
        };
        if ok == 0 {
            return Err(last_error("GetProcessTimes on termination handle failed"));
        }
        Ok(ProcessIdentity {
            pid,
            creation_time_100ns: filetime_to_u64(creation),
        })
    }

    fn process_memory(pid: u32) -> Result<ProcessMemory, ResourceError> {
        let handle = open_process(pid, PROCESS_QUERY_INFORMATION | PROCESS_VM_READ)?;
        let mut counters = unsafe { std::mem::zeroed::<PROCESS_MEMORY_COUNTERS_EX>() };
        counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        let ok = unsafe {
            K32GetProcessMemoryInfo(
                handle.raw(),
                &mut counters as *mut _ as *mut PROCESS_MEMORY_COUNTERS,
                counters.cb,
            )
        };
        if ok == 0 {
            return Err(last_error("K32GetProcessMemoryInfo failed"));
        }
        Ok(ProcessMemory {
            private_bytes: Some(counters.PrivateUsage as u64),
            working_set_bytes: Some(counters.WorkingSetSize as u64),
        })
    }

    fn open_process(pid: u32, access: u32) -> Result<OwnedHandle, ResourceError> {
        let handle = unsafe { OpenProcess(access, 0, pid) };
        if handle.is_null() {
            return Err(last_error(&format!("OpenProcess({pid}) failed")));
        }
        Ok(OwnedHandle(handle))
    }

    fn pid_exists(pid: u32) -> Result<bool, ResourceError> {
        Ok(process_entries()?.contains_key(&pid))
    }

    fn verify_identity_exited_until(
        identity: ProcessIdentity,
        failure: String,
        deadline: std::time::Instant,
    ) -> Result<TerminateOutcome, ResourceError> {
        let pid = identity.pid;
        match run_platform_observation_until(
            deadline,
            "verify_termination_identity",
            pid,
            move |_| observe_identity(pid),
        )? {
            Some(current) if current == identity => Err(ResourceError::Message(failure)),
            _ => Ok(TerminateOutcome::Terminated),
        }
    }

    fn filetime_to_u64(value: FILETIME) -> u64 {
        ((value.dwHighDateTime as u64) << 32) | value.dwLowDateTime as u64
    }

    fn exe_name(raw: &[u16]) -> String {
        let end = raw.iter().position(|ch| *ch == 0).unwrap_or(raw.len());
        String::from_utf16_lossy(&raw[..end])
    }

    fn last_error(context: &str) -> ResourceError {
        let code = unsafe { GetLastError() };
        ResourceError::Message(format!("{context}: win32 error {code}"))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn entry(pid: u32, parent_pid: u32, exe: &str) -> ProcessEntry {
            ProcessEntry {
                pid,
                parent_pid,
                exe_name: exe.to_string(),
            }
        }

        fn identity(pid: u32, creation_time_100ns: u64) -> ProcessIdentity {
            ProcessIdentity {
                pid,
                creation_time_100ns,
            }
        }

        fn no_memory(_pid: u32) -> ProcessMemory {
            ProcessMemory::default()
        }

        #[test]
        fn matching_root_creation_time_observes_full_subtree() {
            let entries = HashMap::from([
                (1000, entry(1000, 4, "agent.exe")),
                (1001, entry(1001, 1000, "child.exe")),
            ]);
            let identities =
                HashMap::from([(1000, identity(1000, 111)), (1001, identity(1001, 222))]);

            let tree = build_observed_tree(
                &entries,
                identity(1000, 111),
                |pid| identities.get(&pid).copied(),
                no_memory,
            );

            assert!(
                tree.errors.is_empty(),
                "unexpected errors: {:?}",
                tree.errors
            );
            assert_eq!(tree.processes.len(), 2);
            assert_eq!(tree.processes[0].identity, identity(1000, 111));
            assert_eq!(tree.processes[0].depth, 0);
            assert_eq!(tree.processes[1].identity, identity(1001, 222));
            assert_eq!(tree.processes[1].depth, 1);
        }

        #[test]
        fn mismatched_root_creation_time_drops_subtree() {
            // pid 1000 was registered with creation time 111, but a different
            // process (creation time 222) now occupies the pid and has spawned a
            // child. Observing the original identity must drop the whole subtree and
            // report it like a missing root, so the foreign process is never adopted
            // or later terminated.
            let entries = HashMap::from([
                (1000, entry(1000, 4, "foreign.exe")),
                (1001, entry(1001, 1000, "foreign-child.exe")),
            ]);
            let identities =
                HashMap::from([(1000, identity(1000, 222)), (1001, identity(1001, 333))]);

            let tree = build_observed_tree(
                &entries,
                identity(1000, 111),
                |pid| identities.get(&pid).copied(),
                no_memory,
            );

            assert!(
                tree.processes.is_empty(),
                "recycled-root subtree must be dropped, got {:?}",
                tree.processes
            );
            assert_eq!(
                tree.errors,
                vec!["root pid 1000 was not in process snapshot".to_string()]
            );
        }

        #[test]
        fn unreadable_root_drops_subtree_and_protects_readable_child() {
            // #543 (follow-up to #516) - pid 1000 was registered with creation time
            // 111, but the live process now occupying the pid is UNREADABLE: its
            // identity resolves to None (creation_time 0). The #516 mismatch check
            // alone skipped the guard for this case, so a foreign but READABLE child
            // hanging off the unreadable root was still walked and marked
            // kill_allowed = true. The widened guard must drop the whole subtree
            // exactly like a recycled root, so the readable child is never observed
            // and can never be terminated.
            let entries = HashMap::from([
                (1000, entry(1000, 4, "foreign-unreadable.exe")),
                (1001, entry(1001, 1000, "foreign-child.exe")),
            ]);
            // Root identity is unreadable (absent from the map -> None); only the
            // readable child resolves to a real identity.
            let identities = HashMap::from([(1001, identity(1001, 222))]);

            let tree = build_observed_tree(
                &entries,
                identity(1000, 111),
                |pid| identities.get(&pid).copied(),
                no_memory,
            );

            assert!(
                tree.processes.is_empty(),
                "unverifiable-root subtree must be dropped, got {:?}",
                tree.processes
            );
            assert!(
                !tree.processes.iter().any(|p| p.kill_allowed),
                "no descendant under an unverifiable root may be kill_allowed"
            );
            assert_eq!(
                tree.errors,
                vec!["root pid 1000 was not in process snapshot".to_string()]
            );
        }

        #[test]
        fn missing_root_reports_not_in_snapshot() {
            let entries: HashMap<u32, ProcessEntry> = HashMap::new();
            let identities: HashMap<u32, ProcessIdentity> = HashMap::new();

            let tree = build_observed_tree(
                &entries,
                identity(1000, 111),
                |pid| identities.get(&pid).copied(),
                no_memory,
            );

            assert!(tree.processes.is_empty());
            assert_eq!(
                tree.errors,
                vec!["root pid 1000 was not in process snapshot".to_string()]
            );
        }

        #[test]
        fn resolves_identity_only_for_subtree_pids() {
            // Two unrelated processes (2000/2001) live in the snapshot but are not in
            // the root's subtree. The identity resolver must never be invoked for them;
            // that is exactly the ~498-PID cost #564 removes.
            let entries = HashMap::from([
                (1000, entry(1000, 4, "agent.exe")),
                (1001, entry(1001, 1000, "child.exe")),
                (2000, entry(2000, 4, "unrelated.exe")),
                (2001, entry(2001, 2000, "unrelated-child.exe")),
            ]);
            let identities = HashMap::from([
                (1000, identity(1000, 111)),
                (1001, identity(1001, 222)),
                (2000, identity(2000, 333)),
                (2001, identity(2001, 444)),
            ]);
            let mut probed: Vec<u32> = Vec::new();
            let tree = build_observed_tree(
                &entries,
                identity(1000, 111),
                |pid| {
                    probed.push(pid);
                    identities.get(&pid).copied()
                },
                no_memory,
            );

            assert!(
                tree.errors.is_empty(),
                "unexpected errors: {:?}",
                tree.errors
            );
            assert_eq!(tree.processes.len(), 2);
            assert!(probed.contains(&1000) && probed.contains(&1001));
            assert!(
                !probed.contains(&2000) && !probed.contains(&2001),
                "identity resolver must not touch processes outside the subtree, probed={probed:?}"
            );
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;

    pub struct PlatformProcessTreeBackend;

    impl Default for PlatformProcessTreeBackend {
        fn default() -> Self {
            Self::new()
        }
    }

    impl PlatformProcessTreeBackend {
        pub fn new() -> Self {
            Self
        }
    }

    impl ProcessTreeBackend for PlatformProcessTreeBackend {
        fn observe_tree(
            &self,
            root: ProcessIdentity,
        ) -> Result<ObservedProcessTree, ResourceError> {
            let current = observe_identity(root.pid)?;
            if current != Some(root) {
                return Ok(ObservedProcessTree {
                    processes: Vec::new(),
                    errors: vec![format!("root pid {} was not in process snapshot", root.pid)],
                });
            }
            Ok(ObservedProcessTree {
                processes: Vec::new(),
                errors: vec!["process tree telemetry unavailable on this platform".to_string()],
            })
        }

        fn observe_identity(&self, pid: u32) -> Result<Option<ProcessIdentity>, ResourceError> {
            observe_identity(pid)
        }

        fn observe_tree_until(
            &self,
            root: ProcessIdentity,
            deadline: std::time::Instant,
        ) -> Result<ObservedProcessTree, ResourceError> {
            run_platform_observation_until(deadline, "observe_tree", root.pid, move |_| {
                Self::new().observe_tree(root)
            })
        }

        fn observe_identity_until(
            &self,
            pid: u32,
            deadline: std::time::Instant,
        ) -> Result<Option<ProcessIdentity>, ResourceError> {
            run_platform_observation_until(deadline, "observe_identity", pid, move |_| {
                observe_identity(pid)
            })
        }

        fn terminate_verified(
            &self,
            process: &ObservedProcess,
        ) -> Result<TerminateOutcome, ResourceError> {
            if observe_identity(process.identity.pid)? != Some(process.identity) {
                return Ok(TerminateOutcome::AlreadyGone);
            }
            Err(ResourceError::Message(format!(
                "process termination unavailable on this platform for verified pid {}",
                process.identity.pid
            )))
        }

        fn terminate_verified_until(
            &self,
            process: &ObservedProcess,
            deadline: std::time::Instant,
        ) -> Result<TerminateOutcome, ResourceError> {
            require_backend_time(deadline, "terminate_verified", process.identity.pid)?;
            let identity = self.observe_identity_until(process.identity.pid, deadline)?;
            if identity != Some(process.identity) {
                Ok(TerminateOutcome::AlreadyGone)
            } else {
                Err(ResourceError::Message(format!(
                    "process termination unavailable on this platform for verified pid {}",
                    process.identity.pid
                )))
            }
        }

        fn current_process_memory(&self) -> Result<ProcessMemory, ResourceError> {
            Ok(ProcessMemory::default())
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct ProcStat {
        pid: u32,
        start_time_ticks: u64,
    }

    fn observe_identity(pid: u32) -> Result<Option<ProcessIdentity>, ResourceError> {
        let Some(stat) = read_proc_stat(pid)? else {
            return Ok(None);
        };
        let ticks_per_second = clock_ticks_per_second()?;
        let creation_time_100ns =
            u64::try_from(u128::from(stat.start_time_ticks) * 10_000_000 / ticks_per_second)
                .map_err(|_| {
                    ResourceError::Message(format!(
                        "Linux start identity overflowed for pid {}",
                        stat.pid
                    ))
                })?;
        Ok(Some(ProcessIdentity {
            pid: stat.pid,
            creation_time_100ns,
        }))
    }

    fn read_proc_stat(pid: u32) -> Result<Option<ProcStat>, ResourceError> {
        if pid == 0 {
            return Ok(None);
        }
        let path = format!("/proc/{pid}/stat");
        let value = match std::fs::read_to_string(&path) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(ResourceError::Message(format!(
                    "failed to read Linux process identity at {path}: {error}"
                )));
            }
        };
        parse_proc_stat(&value).map(Some)
    }

    fn parse_proc_stat(value: &str) -> Result<ProcStat, ResourceError> {
        let open = value.find('(').ok_or_else(|| {
            ResourceError::Message("Linux process stat omitted command start".to_string())
        })?;
        let close = value.rfind(')').ok_or_else(|| {
            ResourceError::Message("Linux process stat omitted command end".to_string())
        })?;
        if close <= open {
            return Err(ResourceError::Message(
                "Linux process stat had an invalid command field".to_string(),
            ));
        }
        let pid = value[..open].trim().parse::<u32>().map_err(|error| {
            ResourceError::Message(format!("Linux process stat had an invalid pid: {error}"))
        })?;
        let fields = value[close + 1..].split_whitespace().collect::<Vec<_>>();
        let start_time_ticks = fields
            .get(19)
            .ok_or_else(|| {
                ResourceError::Message("Linux process stat omitted start time".to_string())
            })?
            .parse::<u64>()
            .map_err(|error| {
                ResourceError::Message(format!(
                    "Linux process stat had an invalid start time: {error}"
                ))
            })?;
        Ok(ProcStat {
            pid,
            start_time_ticks,
        })
    }

    fn clock_ticks_per_second() -> Result<u128, ResourceError> {
        // SAFETY: sysconf reads the fixed process clock-tick configuration.
        let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        if ticks <= 0 {
            return Err(ResourceError::Message(format!(
                "failed to read Linux clock ticks per second: {ticks}"
            )));
        }
        Ok(ticks as u128)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn proc_stat_parser_preserves_a_real_start_identity() {
            let parsed = parse_proc_stat(
                "17 (name with ) parenthesis) S 1 17 17 0 -1 0 0 0 0 0 0 0 0 0 0 0 0 0 4242 0",
            )
            .expect("parse injected Linux proc stat");
            assert_eq!(
                parsed,
                ProcStat {
                    pid: 17,
                    start_time_ticks: 4242
                }
            );
        }

        #[test]
        fn linux_identity_is_non_placeholder_and_pid_reuse_is_not_adopted() {
            let backend = PlatformProcessTreeBackend::new();
            let pid = std::process::id();
            let identity = backend
                .observe_identity(pid)
                .expect("observe current Linux identity")
                .expect("current process exists");
            assert_ne!(identity.creation_time_100ns, 0);
            assert_eq!(
                backend
                    .observe_identity(pid)
                    .expect("reobserve current Linux identity"),
                Some(identity)
            );

            let reused = ProcessIdentity {
                pid,
                creation_time_100ns: identity.creation_time_100ns.saturating_add(1),
            };
            let tree = backend
                .observe_tree(reused)
                .expect("identity mismatch is a terminal observation");
            assert!(tree.processes.is_empty());
            assert_eq!(
                tree.errors,
                vec![format!("root pid {pid} was not in process snapshot")]
            );
            let process = ObservedProcess {
                identity: reused,
                parent_pid: None,
                parent_identity: None,
                exe_name: "reused".to_string(),
                depth: 0,
                private_bytes: None,
                working_set_bytes: None,
                cpu_percent: None,
                kill_allowed: true,
            };
            assert_eq!(
                backend
                    .terminate_verified(&process)
                    .expect("reused identity is already gone"),
                TerminateOutcome::AlreadyGone
            );
        }

        #[test]
        fn linux_production_observation_returns_at_the_absolute_native_deadline() {
            let mut child = std::process::Command::new("sh")
                .args(["-c", "exec sleep 30"])
                .spawn()
                .expect("spawn native observation fixture");
            let pid = child.id();
            let (reached_tx, reached_rx) = std::sync::mpsc::sync_channel(1);
            let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
            inject_platform_observation_pause("observe_identity", pid, reached_tx, release_rx);

            let started = std::time::Instant::now();
            let caller = std::thread::spawn(move || {
                PlatformProcessTreeBackend::new()
                    .observe_identity_until(pid, started + std::time::Duration::from_millis(75))
            });
            reached_rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .expect("native observation reached the production gate");
            let error = caller
                .join()
                .expect("join bounded native observation")
                .expect_err("held native observation must hit its deadline");
            let elapsed = started.elapsed();
            assert!(
                error
                    .to_string()
                    .contains("syscall=observe_identity deadline expired"),
                "{error}"
            );
            assert!(
                elapsed < std::time::Duration::from_secs(1),
                "native observation escaped its absolute deadline: {elapsed:?}"
            );
            assert!(
                child
                    .try_wait()
                    .expect("probe observation fixture")
                    .is_none(),
                "read-only observation cancellation must not terminate the fixture"
            );

            release_tx
                .send(())
                .expect("release cancelled native observation");
            child.kill().expect("kill native observation fixture");
            let reap_deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            loop {
                match child.try_wait().expect("reap native observation fixture") {
                    Some(_) => break,
                    None if std::time::Instant::now() < reap_deadline => {
                        std::thread::sleep(std::time::Duration::from_millis(2));
                    }
                    None => panic!("native observation fixture did not exit after kill"),
                }
            }

            let expired = PlatformProcessTreeBackend::new()
                .observe_identity_until(pid, std::time::Instant::now())
                .expect_err("expired deadline must reject before native observation");
            assert!(
                expired
                    .to_string()
                    .contains("observe_identity deadline expired"),
                "{expired}"
            );
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;

    pub struct PlatformProcessTreeBackend;

    impl Default for PlatformProcessTreeBackend {
        fn default() -> Self {
            Self::new()
        }
    }

    impl PlatformProcessTreeBackend {
        pub fn new() -> Self {
            Self
        }
    }

    impl ProcessTreeBackend for PlatformProcessTreeBackend {
        fn observe_tree(
            &self,
            root: ProcessIdentity,
        ) -> Result<ObservedProcessTree, ResourceError> {
            if observe_identity(root.pid)? != Some(root) {
                return Ok(ObservedProcessTree {
                    processes: Vec::new(),
                    errors: vec![format!("root pid {} was not in process snapshot", root.pid)],
                });
            }
            Ok(ObservedProcessTree {
                processes: Vec::new(),
                errors: vec!["process tree telemetry unavailable on this platform".to_string()],
            })
        }

        fn observe_identity(&self, pid: u32) -> Result<Option<ProcessIdentity>, ResourceError> {
            observe_identity(pid)
        }

        fn observe_tree_until(
            &self,
            root: ProcessIdentity,
            deadline: std::time::Instant,
        ) -> Result<ObservedProcessTree, ResourceError> {
            run_platform_observation_until(deadline, "observe_tree", root.pid, move |_| {
                Self::new().observe_tree(root)
            })
        }

        fn observe_identity_until(
            &self,
            pid: u32,
            deadline: std::time::Instant,
        ) -> Result<Option<ProcessIdentity>, ResourceError> {
            run_platform_observation_until(deadline, "observe_identity", pid, move |_| {
                observe_identity(pid)
            })
        }

        fn terminate_verified(
            &self,
            process: &ObservedProcess,
        ) -> Result<TerminateOutcome, ResourceError> {
            if observe_identity(process.identity.pid)? != Some(process.identity) {
                return Ok(TerminateOutcome::AlreadyGone);
            }
            Err(ResourceError::Message(format!(
                "process termination unavailable on this platform for verified pid {}",
                process.identity.pid
            )))
        }

        fn terminate_verified_until(
            &self,
            process: &ObservedProcess,
            deadline: std::time::Instant,
        ) -> Result<TerminateOutcome, ResourceError> {
            require_backend_time(deadline, "terminate_verified", process.identity.pid)?;
            let identity = self.observe_identity_until(process.identity.pid, deadline)?;
            if identity != Some(process.identity) {
                Ok(TerminateOutcome::AlreadyGone)
            } else {
                Err(ResourceError::Message(format!(
                    "process termination unavailable on this platform for verified pid {}",
                    process.identity.pid
                )))
            }
        }

        fn current_process_memory(&self) -> Result<ProcessMemory, ResourceError> {
            Ok(ProcessMemory::default())
        }
    }

    fn observe_identity(pid: u32) -> Result<Option<ProcessIdentity>, ResourceError> {
        let pid = libc::pid_t::try_from(pid)
            .map_err(|_| ResourceError::Message("macOS pid exceeded pid_t range".to_string()))?;
        if pid <= 0 {
            return Ok(None);
        }
        let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
        let size =
            libc::c_int::try_from(std::mem::size_of::<libc::proc_bsdinfo>()).map_err(|_| {
                ResourceError::Message("macOS process identity buffer exceeded c_int".to_string())
            })?;
        // SAFETY: the buffer is valid for exactly `size` bytes and
        // PROC_PIDTBSDINFO initializes proc_bsdinfo on a full-size success.
        let read = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                info.as_mut_ptr().cast(),
                size,
            )
        };
        if read == 0 {
            let error = std::io::Error::last_os_error();
            return match error.raw_os_error() {
                Some(libc::ESRCH) | Some(libc::ENOENT) => Ok(None),
                _ => Err(ResourceError::Message(format!(
                    "failed to read macOS process identity for pid {pid}: {error}"
                ))),
            };
        }
        if read != size {
            return Err(ResourceError::Message(format!(
                "macOS process identity for pid {pid} returned {read} of {size} bytes"
            )));
        }
        // SAFETY: proc_pidinfo reported a complete proc_bsdinfo buffer.
        let info = unsafe { info.assume_init() };
        if info.pbi_pid != pid as u32 {
            return Err(ResourceError::Message(format!(
                "macOS process identity pid mismatch: requested {pid}, observed {}",
                info.pbi_pid
            )));
        }
        let creation_time_100ns = info
            .pbi_start_tvsec
            .checked_mul(10_000_000)
            .and_then(|value| value.checked_add(info.pbi_start_tvusec.saturating_mul(10)))
            .filter(|value| *value != 0)
            .ok_or_else(|| {
                ResourceError::Message(format!(
                    "macOS process identity timestamp was invalid for pid {pid}"
                ))
            })?;
        Ok(Some(ProcessIdentity {
            pid: info.pbi_pid,
            creation_time_100ns,
        }))
    }
}

#[cfg(all(not(windows), not(target_os = "linux"), not(target_os = "macos")))]
mod platform {
    use super::*;

    pub struct PlatformProcessTreeBackend;

    impl Default for PlatformProcessTreeBackend {
        fn default() -> Self {
            Self::new()
        }
    }

    impl PlatformProcessTreeBackend {
        pub fn new() -> Self {
            Self
        }
    }

    impl ProcessTreeBackend for PlatformProcessTreeBackend {
        fn observe_tree(
            &self,
            root: ProcessIdentity,
        ) -> Result<ObservedProcessTree, ResourceError> {
            Err(stable_identity_unavailable(root.pid))
        }

        fn observe_identity(&self, pid: u32) -> Result<Option<ProcessIdentity>, ResourceError> {
            Err(stable_identity_unavailable(pid))
        }

        fn observe_tree_until(
            &self,
            root: ProcessIdentity,
            deadline: std::time::Instant,
        ) -> Result<ObservedProcessTree, ResourceError> {
            run_platform_observation_until(deadline, "observe_tree", root.pid, move |_| {
                Err(stable_identity_unavailable(root.pid))
            })
        }

        fn observe_identity_until(
            &self,
            pid: u32,
            deadline: std::time::Instant,
        ) -> Result<Option<ProcessIdentity>, ResourceError> {
            run_platform_observation_until(deadline, "observe_identity", pid, move |_| {
                Err(stable_identity_unavailable(pid))
            })
        }

        fn terminate_verified(
            &self,
            process: &ObservedProcess,
        ) -> Result<TerminateOutcome, ResourceError> {
            Err(stable_identity_unavailable(process.identity.pid))
        }

        fn terminate_verified_until(
            &self,
            process: &ObservedProcess,
            deadline: std::time::Instant,
        ) -> Result<TerminateOutcome, ResourceError> {
            let pid = process.identity.pid;
            run_platform_observation_until(deadline, "terminate_verified", pid, move |_| {
                Err(stable_identity_unavailable(pid))
            })
        }

        fn current_process_memory(&self) -> Result<ProcessMemory, ResourceError> {
            Ok(ProcessMemory::default())
        }
    }

    fn stable_identity_unavailable(pid: u32) -> ResourceError {
        ResourceError::Message(format!(
            "stable process identity unavailable on this platform for pid {pid}"
        ))
    }
}

pub use platform::PlatformProcessTreeBackend;
