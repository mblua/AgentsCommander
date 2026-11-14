use crate::cleanup_diagnostics::{operation::Operation, schema};
use crate::resource_monitor::registry::{
    termination::DiagnosticTerminationError, validate_diagnostic_row,
};

use std::collections::{HashMap, HashSet, VecDeque};

use crate::resource_monitor::registry::{ProcessTreeBackend, ResourceError};
use crate::resource_monitor::types::{
    ObservedProcess, ObservedProcessTree, ProcessIdentity, ProcessMemory, TerminateOutcome,
};
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_NO_MORE_FILES, FILETIME, HANDLE, INVALID_HANDLE_VALUE,
    STILL_ACTIVE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::ProcessStatus::{
    K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetExitCodeProcess, GetProcessTimes, OpenProcess,
    QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject, PROCESS_QUERY_INFORMATION,
    PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, PROCESS_VM_READ,
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
    fn observe_tree(&self, root: ProcessIdentity) -> Result<ObservedProcessTree, ResourceError> {
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
        match terminate_with_calls(process, None, &NativeTerminationCalls) {
            Ok(outcome) => Ok(outcome),
            Err(DiagnosticTerminationError::Ordinary(error)) => Err(error),
            Err(DiagnosticTerminationError::Evidence(error)) => {
                Err(ResourceError::Message(error.to_string()))
            }
        }
    }

    #[allow(private_interfaces)]
    fn observe_diagnostic_identity(&self, pid: u32) -> schema::CurrentIdentity {
        match query_identity(pid) {
            Ok(Some((handle, _))) => {
                let mut code = 0;
                // SAFETY: `query_identity` returned an OwnedHandle opened with query rights;
                // `code` is writable u32 storage and GetExitCodeProcess success is checked.
                if unsafe { GetExitCodeProcess(handle.raw(), &mut code) } == 0 {
                    return unreadable(&last_error("GetExitCodeProcess failed"));
                }
                if code != STILL_ACTIVE as u32 {
                    return schema::CurrentIdentity {
                        status: schema::IdentityStatus::Gone,
                        ..schema::CurrentIdentity::unknown()
                    };
                }
                current_handle(&handle, pid)
            }
            Ok(None) => schema::CurrentIdentity {
                status: schema::IdentityStatus::Gone,
                ..schema::CurrentIdentity::unknown()
            },
            Err(error) => unreadable(&error),
        }
    }

    #[allow(private_interfaces)]
    fn terminate_verified_diagnostic(
        &self,
        process: &ObservedProcess,
        row: &schema::SelectionRow,
        operation: &Operation,
    ) -> Result<TerminateOutcome, DiagnosticTerminationError> {
        terminate_with_calls(process, Some((row, operation)), &NativeTerminationCalls)
    }

    fn current_process_memory(&self) -> Result<ProcessMemory, ResourceError> {
        // SAFETY: PROCESS_MEMORY_COUNTERS_EX contains only integer fields, so zero is a
        // valid initial value; `counters.cb` is set before K32GetProcessMemoryInfo.
        let mut counters = unsafe { std::mem::zeroed::<PROCESS_MEMORY_COUNTERS_EX>() };
        counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        // SAFETY: GetCurrentProcess supplies a borrowed pseudo-handle. `counters` is writable
        // PROCESS_MEMORY_COUNTERS_EX storage with cb set to its full size; the cast to its
        // PROCESS_MEMORY_COUNTERS prefix is the layout K32GetProcessMemoryInfo accepts.
        let ok = unsafe {
            K32GetProcessMemoryInfo(
                GetCurrentProcess(),
                (&mut counters as *mut PROCESS_MEMORY_COUNTERS_EX)
                    .cast::<PROCESS_MEMORY_COUNTERS>(),
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
/// read at all (`creation_time` 0), so a foreign process occupying the recycled PID
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
/// Each pid is marked when enqueued, so a parent-PID cycle left by PID reuse
/// (#2443) cannot re-enqueue a pid: the walk visits each pid at most once and
/// always terminates.
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
    let mut enqueued: HashSet<u32> = HashSet::from([root.pid]);
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
            && resolved_identity.map(|id| id.creation_time_100ns) != Some(root.creation_time_100ns)
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
                if enqueued.insert(*child) {
                    queue.push_back((*child, depth.saturating_add(1)));
                }
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
        // SAFETY: OwnedHandle is constructed only from checked successful OpenProcess or
        // CreateToolhelp32Snapshot results; Drop closes the owned handle exactly once.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn process_entries() -> Result<HashMap<u32, ProcessEntry>, ResourceError> {
    // SAFETY: TH32CS_SNAPPROCESS makes CreateToolhelp32Snapshot ignore the PID argument;
    // the INVALID_HANDLE_VALUE failure sentinel is checked before OwnedHandle construction.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(last_error("CreateToolhelp32Snapshot failed"));
    }
    let snapshot = OwnedHandle(snapshot);
    // SAFETY: PROCESSENTRY32W has only integers and a UTF-16 array, all zero-valid;
    // dwSize is set immediately below before Process32FirstW writes the structure.
    let mut entry = unsafe { std::mem::zeroed::<PROCESSENTRY32W>() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    // #559 (G1) - a valid snapshot always contains at least System Idle / System /
    // the calling process, so a zero return here is an enumeration FAILURE, not a
    // legitimately empty list. Returning Err keeps a failed/partial walk off the
    // reap path: the H1 nomination treats a missing root as "gone", so a spuriously
    // empty or truncated map must never be mistaken for dead roots.
    // SAFETY: `snapshot` owns the successful process snapshot, and entry.dwSize names
    // its full size; Process32FirstW receives exclusively writable `entry` storage.
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
        // SAFETY: `snapshot` is still owned and open; `entry` retains its dwSize and provides
        // writable PROCESSENTRY32W storage for each Process32NextW iteration.
        if unsafe { Process32NextW(snapshot.raw(), &mut entry) } == 0 {
            // SAFETY: GetLastError takes no arguments and reads the current thread's error
            // immediately after Process32NextW returned failure.
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
    let identity = match handle_identity(&handle, pid) {
        Ok(identity) => identity,
        Err(error) => {
            return if pid_exists(pid)? {
                Err(error)
            } else {
                Ok(None)
            }
        }
    };
    Ok(Some((handle, identity)))
}

fn handle_identity(handle: &OwnedHandle, pid: u32) -> Result<ProcessIdentity, ResourceError> {
    // SAFETY: FILETIME is two u32 fields, so zero initialization is valid for `creation`
    // and the three copied output structures passed to GetProcessTimes below.
    let mut creation: FILETIME = unsafe { std::mem::zeroed() };
    let mut exit = creation;
    let mut kernel = creation;
    let mut user = creation;
    // SAFETY: `handle` borrows an OwnedHandle opened with query rights in query_identity
    // or NativeTerminationCalls::open; the four distinct FILETIME locals are writable
    // for GetProcessTimes, and failure returns before their values are used.
    if unsafe {
        GetProcessTimes(
            handle.raw(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    } == 0
    {
        return Err(last_error("GetProcessTimes failed"));
    }
    Ok(ProcessIdentity {
        pid,
        creation_time_100ns: filetime_to_u64(creation),
    })
}

fn unreadable(error: &ResourceError) -> schema::CurrentIdentity {
    schema::CurrentIdentity {
        status: schema::IdentityStatus::Unreadable,
        identity: None,
        executable: None,
        os_error: error
            .to_string()
            .rsplit_once("win32 error ")
            .and_then(|(_, code)| code.parse().ok()),
    }
}

fn current_handle(handle: &OwnedHandle, pid: u32) -> schema::CurrentIdentity {
    let identity = match handle_identity(handle, pid) {
        Ok(identity) => identity,
        Err(error) => return unreadable(&error),
    };
    let mut name = vec![0_u16; 32_768];
    let mut size = name.len() as u32;
    // SAFETY: `handle` stays owned and has query rights from query_identity or
    // NativeTerminationCalls::open; `name` has 32768 writable UTF-16 units and `size`
    // supplies that capacity to QueryFullProcessImageNameW.
    let ok = unsafe { QueryFullProcessImageNameW(handle.raw(), 0, name.as_mut_ptr(), &mut size) };
    // SAFETY: GetLastError takes no arguments; the `ok == 0` closure runs immediately
    // after QueryFullProcessImageNameW fails on this thread.
    let os_error = (ok == 0).then(|| unsafe { GetLastError() });
    schema::CurrentIdentity {
        status: schema::IdentityStatus::Same,
        identity: Some(schema::ProcessIdentity {
            pid,
            start_time: identity.creation_time_100ns,
        }),
        executable: (ok != 0).then(|| String::from_utf16_lossy(&name[..size as usize])),
        os_error,
    }
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
    // SAFETY: `query_identity` retains its query-capable OwnedHandle in `handle`;
    // `exit_code` is a writable u32, read only after GetExitCodeProcess succeeds.
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
    // SAFETY: PROCESS_MEMORY_COUNTERS_EX contains only integers, all zero-valid;
    // `counters.cb` is set to its full size before K32GetProcessMemoryInfo below.
    let mut counters = unsafe { std::mem::zeroed::<PROCESS_MEMORY_COUNTERS_EX>() };
    counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
    // SAFETY: `open_process` supplied an owned handle with query and VM_READ rights.
    // `counters` is writable EX storage with cb set; its PROCESS_MEMORY_COUNTERS prefix
    // cast and full size are the buffer form accepted by K32GetProcessMemoryInfo.
    let ok = unsafe {
        K32GetProcessMemoryInfo(
            handle.raw(),
            (&mut counters as *mut PROCESS_MEMORY_COUNTERS_EX).cast::<PROCESS_MEMORY_COUNTERS>(),
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
    // SAFETY: OpenProcess takes numeric access/PID and disables inheritance here;
    // its null failure is checked before the successful handle becomes an OwnedHandle.
    let handle = unsafe { OpenProcess(access, 0, pid) };
    if handle.is_null() {
        return Err(last_error(&format!("OpenProcess({pid}) failed")));
    }
    Ok(OwnedHandle(handle))
}

fn pid_exists(pid: u32) -> Result<bool, ResourceError> {
    Ok(process_entries()?.contains_key(&pid))
}

// The handle port makes the final-handle ordering testable without touching host processes.
trait TerminationCalls {
    type Handle;
    fn observe(&self, pid: u32) -> Result<Option<ProcessIdentity>, ResourceError>;
    fn open(&self, pid: u32) -> Result<Self::Handle, ResourceError>;
    fn current(&self, handle: &Self::Handle, pid: u32) -> schema::CurrentIdentity;
    fn terminate(&self, handle: &Self::Handle) -> schema::RawApiResult;
    fn wait(&self, handle: &Self::Handle, pid: u32) -> (schema::ExitObservation, Option<String>);
}

struct NativeTerminationCalls;

impl TerminationCalls for NativeTerminationCalls {
    type Handle = OwnedHandle;

    fn observe(&self, pid: u32) -> Result<Option<ProcessIdentity>, ResourceError> {
        observe_identity(pid)
    }

    fn open(&self, pid: u32) -> Result<OwnedHandle, ResourceError> {
        open_process(
            pid,
            PROCESS_TERMINATE | PROCESS_SYNCHRONIZE | PROCESS_QUERY_INFORMATION,
        )
    }

    fn current(&self, handle: &OwnedHandle, pid: u32) -> schema::CurrentIdentity {
        current_handle(handle, pid)
    }

    fn terminate(&self, handle: &OwnedHandle) -> schema::RawApiResult {
        // SAFETY: NativeTerminationCalls::open obtains PROCESS_TERMINATE on this OwnedHandle;
        // the borrow keeps it open for TerminateProcess, which takes no pointer arguments.
        let ok = unsafe { TerminateProcess(handle.raw(), 1) };
        // SAFETY: GetLastError has no caller preconditions and this closure runs immediately
        // after TerminateProcess returns failure on the same thread.
        let os_error = (ok == 0).then(|| unsafe { GetLastError() });
        schema::RawApiResult {
            returned: Some(ok != 0),
            os_error,
            detail: os_error.map_or_else(
                || "TerminateProcess returned nonzero".into(),
                |code| format!("TerminateProcess failed: win32 error {code}"),
            ),
        }
    }

    fn wait(&self, handle: &OwnedHandle, pid: u32) -> (schema::ExitObservation, Option<String>) {
        // SAFETY: NativeTerminationCalls::open obtains PROCESS_SYNCHRONIZE on this
        // OwnedHandle; the borrow keeps it open while WaitForSingleObject waits for 2000 ms.
        match unsafe { WaitForSingleObject(handle.raw(), 2_000) } {
            WAIT_OBJECT_0 => (schema::ExitObservation::Exited { code: None }, None),
            WAIT_TIMEOUT => (
                schema::ExitObservation::Timeout,
                Some(format!("timed out waiting for pid {pid} to exit")),
            ),
            WAIT_FAILED => {
                // SAFETY: GetLastError takes no arguments; WAIT_FAILED from WaitForSingleObject
                // selects this thread's error before any intervening Win32 call.
                let error = unsafe { GetLastError() };
                (
                    schema::ExitObservation::Error {
                        os_error: Some(error),
                    },
                    Some(format!("WaitForSingleObject failed: win32 error {error}")),
                )
            }
            other => (
                schema::ExitObservation::Unknown,
                Some(format!(
                    "unexpected WaitForSingleObject result {other} for pid {pid}"
                )),
            ),
        }
    }
}

fn terminate_with_calls(
    process: &ObservedProcess,
    diagnostic: Option<(&schema::SelectionRow, &Operation)>,
    calls: &impl TerminationCalls,
) -> Result<TerminateOutcome, DiagnosticTerminationError> {
    if let Some((row, _)) = diagnostic {
        validate_diagnostic_row(process, row)?;
    }
    let current = calls
        .observe(process.identity.pid)
        .map_err(DiagnosticTerminationError::Ordinary)?;
    if current != Some(process.identity) {
        return Ok(TerminateOutcome::AlreadyGone);
    }
    let verify = |failure: String| match calls.observe(process.identity.pid)? {
        Some(current) if current == process.identity => Err(ResourceError::Message(failure)),
        _ => Ok(TerminateOutcome::Terminated),
    };
    let handle = match calls.open(process.identity.pid) {
        Ok(handle) => handle,
        Err(error) => {
            return verify(error.to_string()).map_err(DiagnosticTerminationError::Ordinary)
        }
    };
    let ticket = if let Some((row, operation)) = diagnostic {
        let mut final_handle = calls.current(&handle, process.identity.pid);
        if let Some(identity) = final_handle.identity {
            final_handle.status = if identity == row.expected {
                schema::IdentityStatus::Same
            } else {
                schema::IdentityStatus::Reused
            };
        }
        let precheck = schema::CurrentIdentity {
            identity: Some(row.expected),
            status: schema::IdentityStatus::Same,
            executable: None,
            os_error: None,
        };
        Some(operation.prepare(row, precheck, final_handle)?)
    } else {
        None
    };
    let raw = calls.terminate(&handle);
    if let (Some((_, operation)), Some(ticket)) = (diagnostic, ticket.as_ref()) {
        operation.returned(ticket, raw.clone())?;
    }
    let (exit, wait_failure) = calls.wait(&handle, process.identity.pid);
    if let (Some((_, operation)), Some(ticket)) = (diagnostic, ticket.as_ref()) {
        operation.observed(ticket, exit)?;
    }
    let failure = if raw.returned == Some(false) {
        raw.detail
    } else {
        wait_failure.unwrap_or_else(|| {
            format!(
                "pid {} is still alive after termination",
                process.identity.pid
            )
        })
    };
    verify(failure).map_err(DiagnosticTerminationError::Ordinary)
}

fn filetime_to_u64(value: FILETIME) -> u64 {
    (u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime)
}

fn exe_name(raw: &[u16]) -> String {
    let end = raw.iter().position(|ch| *ch == 0).unwrap_or(raw.len());
    String::from_utf16_lossy(&raw[..end])
}

fn last_error(context: &str) -> ResourceError {
    // SAFETY: GetLastError takes no arguments and reads the calling thread's error;
    // `last_error` is called directly on the failed Win32 paths above.
    let code = unsafe { GetLastError() };
    ResourceError::Message(format!("{context}: win32 error {code}"))
}

#[cfg(test)]
mod tests;
