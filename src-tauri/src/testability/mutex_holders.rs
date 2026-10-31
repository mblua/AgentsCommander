//! #1773: name the process(es) holding the single-instance mutex when
//! `test-reset --confirm-testeable` refuses with `testable_gui_active`.
//!
//! Windows only. The kernel does not answer "who holds named object X", so the
//! lookup reads the system handle table (`NtQuerySystemInformation`,
//! `SystemExtendedHandleInformation`), keeps the entries whose object type index
//! equals the type index of this process's own probe handle (the `Mutant` type,
//! learned at runtime instead of from an undocumented table), duplicates each such
//! handle into this process and asks `CompareObjectHandles` whether it refers to the
//! same kernel object as the probe. Object addresses are never used: unelevated
//! callers receive them as 0 (measured on Windows 11 26200, 222,169 of 222,169).
//! Diagnostic only: the caller's refusal never depends on this result. The summary
//! reports what was and was not compared and never guesses why.

use serde::Serialize;

#[cfg(any(test, windows))]
pub const STATUS_FOUND: &str = "found";
#[cfg(any(test, windows))]
pub const STATUS_NOT_FOUND: &str = "not_found";
#[cfg(any(test, windows))]
pub const STATUS_INCOMPLETE: &str = "incomplete";
#[cfg(windows)]
pub const STATUS_FAILED: &str = "failed";
#[cfg(not(windows))]
pub const STATUS_UNSUPPORTED: &str = "unsupported";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MutexHolder {
    pub pid: u32,
    pub image_name: Option<String>,
    pub image_path: Option<String>,
    /// The holder's own handle value as `0x` + uppercase hex, pasteable into
    /// Process Explorer. A holder can compute the same string from its `HANDLE`
    /// with `format!("0x{:X}", handle as usize)`.
    pub handle: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanSummary {
    pub status: &'static str,
    pub scanned_processes: u32,
    pub scanned_mutant_handles: u32,
    pub uninspectable_processes: u32,
    pub skipped_handles: u32,
    pub message: Option<String>,
}

#[derive(Debug)]
pub struct HolderLookup {
    pub mutex_name: String,
    pub holders: Vec<MutexHolder>,
    pub scan: ScanSummary,
}

/// Pure classification of a completed scan; unit-tested on every platform.
///
/// `holders`: identity matches. `scanned_mutant_handles`: mutant handles of other
/// processes in the snapshot, compared or not. `uninspectable_processes`: distinct
/// PIDs whose `OpenProcess` returned null. `skipped_handles`: handles not compared,
/// for either reason. The message states counts only; it never names a cause.
#[cfg(any(test, windows))]
pub fn status_for(
    holders: usize,
    scanned_mutant_handles: u32,
    uninspectable_processes: u32,
    skipped_handles: u32,
) -> (&'static str, Option<String>) {
    match (holders > 0, skipped_handles > 0) {
        (true, false) => (STATUS_FOUND, None),
        (true, true) => (
            STATUS_FOUND,
            Some(format!(
                "Found {holders} holder(s), but {skipped_handles} of {scanned_mutant_handles} \
                 mutant handle(s) were not compared ({uninspectable_processes} process(es) \
                 refused PROCESS_DUP_HANDLE); other holders may exist."
            )),
        ),
        (false, false) => (
            STATUS_NOT_FOUND,
            Some(format!(
                "All {scanned_mutant_handles} mutant handle(s) on the system were compared and \
                 none matched; this scan could not see the holder. Re-run test-reset."
            )),
        ),
        (false, true) => (
            STATUS_INCOMPLETE,
            Some(format!(
                "No compared handle matched, and {skipped_handles} of {scanned_mutant_handles} \
                 mutant handle(s) were not compared ({uninspectable_processes} process(es) \
                 refused PROCESS_DUP_HANDLE), so the holder may be one of them. Re-run \
                 test-reset; an elevated shell can inspect processes that refused \
                 PROCESS_DUP_HANDLE."
            )),
        ),
    }
}

fn display_mutex_name() -> String {
    crate::config::profile::mutex_name()
        .trim_end_matches('\0')
        .to_string()
}

fn without_scan(mutex_name: String, status: &'static str, message: String) -> HolderLookup {
    HolderLookup {
        mutex_name,
        holders: Vec::new(),
        scan: ScanSummary {
            status,
            scanned_processes: 0,
            scanned_mutant_handles: 0,
            uninspectable_processes: 0,
            skipped_handles: 0,
            message: Some(message),
        },
    }
}

#[cfg(target_os = "windows")]
fn handle_snapshot() -> Result<(Vec<u64>, u32), String> {
    use windows_sys::Wdk::System::SystemInformation::{
        NtQuerySystemInformation, SYSTEM_INFORMATION_CLASS,
    };
    use windows_sys::Win32::Foundation::{STATUS_INFO_LENGTH_MISMATCH, STATUS_SUCCESS};

    /// Not exported by windows-sys 0.59; stable since Windows Vista. Mirrors the
    /// inline constants in `main.rs` and `config/profile.rs`.
    const SYSTEM_EXTENDED_HANDLE_INFORMATION: SYSTEM_INFORMATION_CLASS = 64;
    const INITIAL_BUFFER_BYTES: usize = 1 << 20;
    const MAX_BUFFER_BYTES: usize = 1 << 29;

    // 1. Snapshot the system handle table, growing on STATUS_INFO_LENGTH_MISMATCH.
    let mut buffer: Vec<u64> = vec![0; INITIAL_BUFFER_BYTES / 8];
    let mut returned: u32 = 0;
    loop {
        let bytes = buffer.len() * 8;
        // SAFETY: `buffer` is writable for `bytes` bytes and outlives the call;
        // `returned` is a valid out-pointer.
        let status = unsafe {
            NtQuerySystemInformation(
                SYSTEM_EXTENDED_HANDLE_INFORMATION,
                buffer.as_mut_ptr().cast(),
                bytes as u32,
                &mut returned,
            )
        };
        if status == STATUS_SUCCESS {
            break;
        }
        if status == STATUS_INFO_LENGTH_MISMATCH {
            if bytes >= MAX_BUFFER_BYTES {
                return Err("handle table exceeds 512 MiB".to_string());
            }
            buffer = vec![0; bytes * 2 / 8];
            continue;
        }
        return Err(format!(
            "NtQuerySystemInformation(SystemExtendedHandleInformation) returned NTSTATUS \
                 0x{:08X}",
            status as u32
        ));
    }

    Ok((buffer, returned))
}

#[cfg(target_os = "windows")]
/// `SYSTEM_HANDLE_TABLE_ENTRY_INFO_EX`. Fields that are read have plain names;
/// the rest keep their layout under a leading underscore.
#[repr(C)]
#[derive(Clone, Copy)]
struct HandleEntry {
    _object: *mut core::ffi::c_void,
    unique_process_id: usize,
    handle_value: usize,
    _granted_access: u32,
    _creator_back_trace_index: u16,
    object_type_index: u16,
    _handle_attributes: u32,
    _reserved: u32,
}

#[cfg(target_os = "windows")]
/// `SYSTEM_HANDLE_INFORMATION_EX` header; entries follow immediately.
#[repr(C)]
#[derive(Clone, Copy)]
struct HandleTableHeader {
    number_of_handles: usize,
    _reserved: usize,
}

#[cfg(target_os = "windows")]
pub fn find_holders(probe: &super::ProfileMutexGuard) -> HolderLookup {
    use std::collections::HashMap;
    use windows_sys::Win32::Foundation::{
        CloseHandle, CompareObjectHandles, DuplicateHandle, DUPLICATE_SAME_ACCESS, HANDLE,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, PROCESS_DUP_HANDLE, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    let mutex_name = display_mutex_name();
    let own_pid = std::process::id() as usize;
    let own_handle = probe.0 as usize;

    // 1. Snapshot the system handle table, growing on STATUS_INFO_LENGTH_MISMATCH.
    let (buffer, returned) = match handle_snapshot() {
        Ok(snapshot) => snapshot,
        Err(message) => return without_scan(mutex_name, STATUS_FAILED, message),
    };

    // 2. Parse with unaligned reads; never trust the count beyond the returned length.
    let header_len = std::mem::size_of::<HandleTableHeader>();
    let entry_len = std::mem::size_of::<HandleEntry>();
    let returned = returned as usize;
    if returned < header_len {
        return without_scan(
            mutex_name,
            STATUS_FAILED,
            "handle table shorter than its header".to_string(),
        );
    }
    let base = buffer.as_ptr().cast::<u8>();
    // SAFETY: `returned >= header_len` bytes of `buffer` were written by the kernel.
    let header = unsafe { std::ptr::read_unaligned(base.cast::<HandleTableHeader>()) };
    let count = header
        .number_of_handles
        .min((returned - header_len) / entry_len);
    let entry = |index: usize| -> HandleEntry {
        // SAFETY: `index < count`, and `count` entries fit inside the returned bytes.
        unsafe { std::ptr::read_unaligned(base.add(header_len + index * entry_len).cast()) }
    };

    // 3. Learn the Mutant type index from this process's own probe handle.
    let Some(mutant_type) = (0..count)
        .map(entry)
        .find(|e| e.unique_process_id == own_pid && e.handle_value == own_handle)
        .map(|e| e.object_type_index)
    else {
        return without_scan(
            mutex_name,
            STATUS_FAILED,
            "the probe handle was not found in the system handle table".to_string(),
        );
    };

    // 4. Compare every other process's Mutant handle against the probe by identity.
    let mut processes: HashMap<usize, HANDLE> = HashMap::new();
    let mut holders = Vec::new();
    let mut scanned_mutant_handles = 0u32;
    let mut skipped_handles = 0u32;
    for index in 0..count {
        let e = entry(index);
        if e.object_type_index != mutant_type || e.unique_process_id == own_pid {
            continue;
        }
        scanned_mutant_handles += 1;
        let process = *processes.entry(e.unique_process_id).or_insert_with(|| {
            // SAFETY: plain Win32 call; a null result is handled below.
            unsafe {
                OpenProcess(
                    PROCESS_DUP_HANDLE | PROCESS_QUERY_LIMITED_INFORMATION,
                    0,
                    e.unique_process_id as u32,
                )
            }
        });
        if process.is_null() {
            skipped_handles += 1;
            continue;
        }
        let mut duplicate: HANDLE = std::ptr::null_mut();
        // SAFETY: `process` is an open handle with PROCESS_DUP_HANDLE; `duplicate` is
        // a valid out-pointer; the source handle value came from the kernel's table.
        let duplicated = unsafe {
            DuplicateHandle(
                process,
                e.handle_value as HANDLE,
                GetCurrentProcess(),
                &mut duplicate,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        };
        if duplicated == 0 || duplicate.is_null() {
            // Not compared. The summary counts this; it does not guess why.
            skipped_handles += 1;
            continue;
        }
        // SAFETY: both handles are open and owned by this process.
        let same = unsafe { CompareObjectHandles(probe.0, duplicate) } != 0;
        // SAFETY: `duplicate` was created by DuplicateHandle above and is closed once.
        unsafe {
            let _ = CloseHandle(duplicate);
        }
        if same {
            let image_path = image_path(process);
            let image_name = image_path
                .as_deref()
                .and_then(|p| std::path::Path::new(p).file_name())
                .map(|n| n.to_string_lossy().into_owned());
            holders.push(MutexHolder {
                pid: e.unique_process_id as u32,
                image_name,
                image_path,
                handle: format!("0x{:X}", e.handle_value),
            });
        }
    }

    let scanned_processes = processes.len() as u32;
    let uninspectable_processes = processes.values().filter(|h| h.is_null()).count() as u32;
    for handle in processes.into_values().filter(|h| !h.is_null()) {
        // SAFETY: every non-null value was returned by OpenProcess above and is closed once.
        unsafe {
            let _ = CloseHandle(handle);
        }
    }

    let (status, message) = status_for(
        holders.len(),
        scanned_mutant_handles,
        uninspectable_processes,
        skipped_handles,
    );
    HolderLookup {
        mutex_name,
        holders,
        scan: ScanSummary {
            status,
            scanned_processes,
            scanned_mutant_handles,
            uninspectable_processes,
            skipped_handles,
            message,
        },
    }
}

#[cfg(target_os = "windows")]
fn image_path(process: windows_sys::Win32::Foundation::HANDLE) -> Option<String> {
    use windows_sys::Win32::System::Threading::{QueryFullProcessImageNameW, PROCESS_NAME_WIN32};

    let mut buf = vec![0u16; 32768];
    let mut len = buf.len() as u32;
    // SAFETY: `process` is open with PROCESS_QUERY_LIMITED_INFORMATION; `buf` is
    // writable for `len` UTF-16 units and `len` is a valid in-out pointer.
    let ok = unsafe {
        QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len)
    };
    if ok == 0 || len == 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..len as usize]))
}

#[cfg(not(target_os = "windows"))]
pub fn find_holders(_probe: &()) -> HolderLookup {
    without_scan(
        display_mutex_name(),
        STATUS_UNSUPPORTED,
        "the single-instance mutex exists only on Windows".to_string(),
    )
}

#[cfg(test)]
mod tests;
