use super::registry::{ProcessTreeBackend, ResourceError};
use super::types::{
    ObservedProcess, ObservedProcessTree, ProcessIdentity, ProcessMemory, TerminateOutcome,
};

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
            let identities = observe_identities(&entries);
            Ok(build_observed_tree(&entries, &identities, root, |pid| {
                process_memory(pid).unwrap_or_default()
            }))
        }

        fn observe_identity(&self, pid: u32) -> Result<Option<ProcessIdentity>, ResourceError> {
            observe_identity(pid)
        }

        fn terminate_verified(
            &self,
            process: &ObservedProcess,
        ) -> Result<TerminateOutcome, ResourceError> {
            let Some(current) = observe_identity(process.identity.pid)? else {
                return Ok(TerminateOutcome::AlreadyGone);
            };
            if current != process.identity {
                return Ok(TerminateOutcome::AlreadyGone);
            }

            let handle = match open_process(
                process.identity.pid,
                PROCESS_TERMINATE | PROCESS_SYNCHRONIZE | PROCESS_QUERY_INFORMATION,
            ) {
                Ok(handle) => handle,
                Err(err) => return verify_identity_exited(process.identity, err.to_string()),
            };
            let ok = unsafe { TerminateProcess(handle.raw(), 1) };
            if ok == 0 {
                return verify_identity_exited(
                    process.identity,
                    last_error("TerminateProcess failed").to_string(),
                );
            }
            let wait_result = unsafe { WaitForSingleObject(handle.raw(), 2_000) };
            let failure = match wait_result {
                WAIT_OBJECT_0 => None,
                WAIT_TIMEOUT => Some(format!(
                    "timed out waiting for pid {} to exit",
                    process.identity.pid
                )),
                WAIT_FAILED => Some(last_error("WaitForSingleObject failed").to_string()),
                other => Some(format!(
                    "unexpected WaitForSingleObject result {other} for pid {}",
                    process.identity.pid
                )),
            };
            if let Some(failure) = failure {
                return verify_identity_exited(process.identity, failure);
            }
            verify_identity_exited(
                process.identity,
                format!(
                    "pid {} is still alive after termination",
                    process.identity.pid
                ),
            )
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

    /// Walk the process snapshot from `root`, returning the observed subtree.
    ///
    /// Root-PID-reuse guard: the registry stores the root's creation time, so a
    /// recycled PID (the original root exited and a foreign process took the same
    /// PID) shows up here as a creation-time mismatch. When that happens we drop
    /// the whole subtree and report it exactly like a missing root, so the registry
    /// never adopts, and later terminates, a process it does not own. Descendant
    /// identities are already verified at terminate time; this makes the root just
    /// as strict at observe time. Memory lookups are injected so the walk is a pure
    /// function over the snapshot and can be unit-tested without real processes.
    fn build_observed_tree(
        entries: &HashMap<u32, ProcessEntry>,
        identities: &HashMap<u32, ProcessIdentity>,
        root: ProcessIdentity,
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
            let identity = identities.get(&pid).copied().unwrap_or_else(|| {
                errors.push(format!("identity unavailable for pid {pid}"));
                ProcessIdentity {
                    pid,
                    creation_time_100ns: 0,
                }
            });
            if pid == root.pid
                && root.creation_time_100ns != 0
                && identity.creation_time_100ns != 0
                && identity.creation_time_100ns != root.creation_time_100ns
            {
                // Live process at the root pid is not the one we registered: the
                // pid was recycled. Drop the subtree and surface the same error the
                // missing-root path uses, so cleanup releases the slot instead of
                // adopting a foreign process.
                errors.push(format!("root pid {} was not in process snapshot", root.pid));
                continue;
            }
            let parent_identity = (entry.parent_pid != 0)
                .then_some(entry.parent_pid)
                .and_then(|parent| identities.get(&parent).copied());
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

    fn observe_identities(entries: &HashMap<u32, ProcessEntry>) -> HashMap<u32, ProcessIdentity> {
        entries
            .keys()
            .filter_map(|pid| {
                observe_identity(*pid)
                    .ok()
                    .flatten()
                    .map(|identity| (*pid, identity))
            })
            .collect()
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

    fn verify_identity_exited(
        identity: ProcessIdentity,
        failure: String,
    ) -> Result<TerminateOutcome, ResourceError> {
        match observe_identity(identity.pid)? {
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

            let tree = build_observed_tree(&entries, &identities, identity(1000, 111), no_memory);

            assert!(tree.errors.is_empty(), "unexpected errors: {:?}", tree.errors);
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

            let tree = build_observed_tree(&entries, &identities, identity(1000, 111), no_memory);

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
        fn missing_root_reports_not_in_snapshot() {
            let entries: HashMap<u32, ProcessEntry> = HashMap::new();
            let identities: HashMap<u32, ProcessIdentity> = HashMap::new();

            let tree = build_observed_tree(&entries, &identities, identity(1000, 111), no_memory);

            assert!(tree.processes.is_empty());
            assert_eq!(
                tree.errors,
                vec!["root pid 1000 was not in process snapshot".to_string()]
            );
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use super::*;

    pub struct PlatformProcessTreeBackend;

    impl PlatformProcessTreeBackend {
        pub fn new() -> Self {
            Self
        }
    }

    impl ProcessTreeBackend for PlatformProcessTreeBackend {
        fn observe_tree(
            &self,
            _root: ProcessIdentity,
        ) -> Result<ObservedProcessTree, ResourceError> {
            Ok(ObservedProcessTree {
                processes: Vec::new(),
                errors: vec!["process tree telemetry unavailable on this platform".to_string()],
            })
        }

        fn observe_identity(&self, pid: u32) -> Result<Option<ProcessIdentity>, ResourceError> {
            Ok(Some(ProcessIdentity {
                pid,
                creation_time_100ns: 0,
            }))
        }

        fn terminate_verified(
            &self,
            _process: &ObservedProcess,
        ) -> Result<TerminateOutcome, ResourceError> {
            Err(ResourceError::Message(
                "process termination unavailable on this platform".to_string(),
            ))
        }

        fn current_process_memory(&self) -> Result<ProcessMemory, ResourceError> {
            Ok(ProcessMemory::default())
        }
    }
}

pub use platform::PlatformProcessTreeBackend;
