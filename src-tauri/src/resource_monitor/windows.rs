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
        STILL_ACTIVE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetExitCodeProcess, GetProcessTimes, OpenProcess, TerminateProcess,
        WaitForSingleObject, PROCESS_QUERY_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
        PROCESS_VM_READ,
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
                        .or_insert_with(|| observe_identity_queryable(pid).ok().flatten())
                },
                |pid| process_memory(pid).unwrap_or_default(),
            ))
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
                let failure = last_error("TerminateProcess failed").to_string();
                // #1438 - ERROR_ACCESS_DENIED here is usually STATUS_PROCESS_IS_TERMINATING:
                // the process is already tearing itself down. Give it the same grace the
                // success path gets so verification sees the settled state. The wait result
                // is deliberately ignored: a target that is still alive after it produces
                // the same Err, carrying the original message, so blocked_by_security still
                // fires on a genuine denial.
                let _ = unsafe { WaitForSingleObject(handle.raw(), 2_000) };
                return verify_identity_exited(process.identity, failure);
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

    /// #1438 - shared probe body: open the pid and read its creation time, returning
    /// the OPENED HANDLE alongside the identity so the caller can keep querying the
    /// SAME handle. The handle must be shared rather than re-opened per query: a
    /// second `OpenProcess` between two reads could land on a recycled pid and report
    /// the new occupant's state as the old identity's.
    fn query_identity(pid: u32) -> Result<Option<(OwnedHandle, ProcessIdentity)>, ResourceError> {
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
        Ok(Some((
            handle,
            ProcessIdentity {
                pid,
                creation_time_100ns: filetime_to_u64(creation),
            },
        )))
    }

    /// #1438 - corpse-AWARE probe: `Ok(Some(identity))` means a LIVE process with that
    /// identity exists right now. A process that has exited yields `Ok(None)` even while
    /// open handles elsewhere keep the corpse queryable (Windows keeps a terminated
    /// process openable, with its creation time intact, for as long as ANY handle to it
    /// exists, which is what made `Ok(Terminated)` unreachable). Every kill and verify
    /// path uses this: the terminate pre-check, `verify_identity_exited`, the trait
    /// method, the registry's `!kill_allowed` checks and second-chance drain, and every
    /// watchdog retry.
    fn observe_identity(pid: u32) -> Result<Option<ProcessIdentity>, ResourceError> {
        let Some((handle, identity)) = query_identity(pid)? else {
            return Ok(None);
        };
        let mut exit_code: u32 = 0;
        let ok = unsafe { GetExitCodeProcess(handle.raw(), &mut exit_code) };
        if ok == 0 {
            let err = last_error("GetExitCodeProcess failed");
            return if pid_exists(pid)? { Err(err) } else { Ok(None) };
        }
        if exit_code != STILL_ACTIVE as u32 {
            // Exited; open handles elsewhere merely keep the corpse queryable.
            return Ok(None);
        }
        Ok(Some(identity))
    }

    /// #1438 G1 - corpse-TOLERANT resolver for the tree walk: returns the identity of
    /// any process that is still queryable, INCLUDING one that has exited but is kept
    /// queryable by open handles elsewhere. The tree walk's question is "what did the OS
    /// snapshot contain", never "is this alive right now", and answering it with the
    /// corpse-aware probe would let a root that exits mid-walk fail the root guard and
    /// drop its whole subtree from the target set. Kill and verify paths must use
    /// `observe_identity` instead.
    fn observe_identity_queryable(pid: u32) -> Result<Option<ProcessIdentity>, ResourceError> {
        Ok(query_identity(pid)?.map(|(_handle, identity)| identity))
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
        use std::process::{Command, Stdio};

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

        /// #1438 - real-process guard for the corpse-aware probe, following the
        /// `pty/job.rs` real-process precedent. The `Child` is deliberately kept in
        /// scope and un-waited: its open process handle keeps the terminated process
        /// queryable, which is the exact production condition (the PTY layer holds the
        /// same kind of handle) that made `Ok(Terminated)` unreachable.
        #[test]
        fn terminate_verified_reports_terminated_then_already_gone_for_real_process() {
            let mut child = Command::new("ping")
                .args(["-n", "30", "127.0.0.1"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn ping child");
            let pid = child.id();

            let backend = PlatformProcessTreeBackend::new();
            let live = backend
                .observe_identity(pid)
                .expect("observe live child")
                .expect("a live child must resolve to an identity");
            assert!(
                observe_identity(pid).expect("probe live child").is_some(),
                "negative control: a live process must be observable to the kill path"
            );

            let observed = ObservedProcess {
                identity: live,
                parent_pid: None,
                parent_identity: None,
                exe_name: "ping.exe".to_string(),
                depth: 0,
                private_bytes: None,
                working_set_bytes: None,
                cpu_percent: None,
                kill_allowed: true,
            };

            assert_eq!(
                backend
                    .terminate_verified(&observed)
                    .expect("first terminate must verify the exit"),
                TerminateOutcome::Terminated
            );

            // The `Child` handle is still held here: the exact production condition.
            assert!(
                observe_identity(pid).expect("probe corpse").is_none(),
                "a corpse must not be observable to the kill path"
            );
            assert!(
                observe_identity_queryable(pid)
                    .expect("resolve corpse")
                    .is_some(),
                "a corpse must stay queryable to the tree resolver (#1438 G1)"
            );

            assert_eq!(
                backend
                    .terminate_verified(&observed)
                    .expect("retry must not error"),
                TerminateOutcome::AlreadyGone,
                "a watchdog retry over a dead target must converge without TerminateProcess"
            );

            child.wait().expect("reap the child");
        }
    }
}

#[cfg(not(windows))]
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
        fn supports_process_tree_enforcement(&self) -> bool {
            false
        }

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

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn non_windows_production_backend_disables_process_tree_enforcement() {
            let backend = PlatformProcessTreeBackend::new();

            assert!(!backend.supports_process_tree_enforcement());
        }
    }
}

pub use platform::PlatformProcessTreeBackend;
