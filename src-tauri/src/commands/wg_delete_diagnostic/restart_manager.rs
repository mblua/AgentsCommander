use super::collect_files_to_probe;
use super::diagnostic_error;
use super::win32_error;
use super::BlockerProcess;
use super::DiagnosticError;
use super::MAX_FILES_PER_PROCESS;
use std::path::Path;
use std::path::PathBuf;

use std::collections::{HashMap, HashSet};
use std::os::windows::ffi::OsStrExt;
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::RestartManager::{
    RmEndSession, RmGetList, RmRegisterResources, RmStartSession, CCH_RM_SESSION_KEY,
    RM_PROCESS_INFO,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};

const ERROR_SUCCESS: u32 = 0;
const ERROR_MORE_DATA: u32 = 234;
/// `RmGetList` can keep returning `ERROR_MORE_DATA` if the process list grows between
/// probe and read. Three retries is the standard Microsoft sample bound.
const MAX_GETLIST_RETRIES: usize = 3;

fn to_wide_nul(s: &str) -> Vec<u16> {
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// RAII guard for an RM session — guarantees `RmEndSession` runs even on early
/// return / panic. Leaking a session persists across the AC process lifetime
/// and burns Restart Manager's per-process session quota (default 64).
/// This Drop is the only path to RmEndSession; covers panic, ?, return.
struct RmSession(u32);
impl Drop for RmSession {
    fn drop(&mut self) {
        unsafe {
            let _ = RmEndSession(self.0);
        }
    }
}

fn rm_start() -> Result<RmSession, DiagnosticError> {
    let mut handle: u32 = 0;
    // `strSessionKey` is PWSTR (mutable). Must hold CCH_RM_SESSION_KEY+1 wide chars.
    let mut key: Vec<u16> = vec![0u16; (CCH_RM_SESSION_KEY as usize) + 1];
    let rc = unsafe { RmStartSession(&mut handle, 0, key.as_mut_ptr()) };
    if rc != ERROR_SUCCESS {
        return Err(win32_error("RmStartSession failed", rc));
    }
    Ok(RmSession(handle))
}

fn rm_register(handle: u32, wide_files: &[Vec<u16>]) -> Result<(), DiagnosticError> {
    if wide_files.is_empty() {
        return Ok(());
    }
    // The collected Vec must outlive the FFI call so the pointers stay valid.
    let ptrs: Vec<*const u16> = wide_files.iter().map(|w| w.as_ptr()).collect();
    let rc = unsafe {
        RmRegisterResources(
            handle,
            ptrs.len() as u32,
            ptrs.as_ptr(),
            0,
            std::ptr::null(),
            0,
            std::ptr::null(),
        )
    };
    if rc != ERROR_SUCCESS {
        return Err(win32_error("RmRegisterResources failed", rc));
    }
    Ok(())
}

fn rm_get_list(handle: u32) -> Result<Vec<RM_PROCESS_INFO>, DiagnosticError> {
    let mut needed: u32 = 0;
    let mut have: u32 = 0;
    let mut reasons: u32 = 0;

    // Probe: zero buffer, learn `needed`. RM returns SUCCESS with needed=0 when
    // there are no blockers — distinct from MORE_DATA.
    let rc = unsafe {
        RmGetList(
            handle,
            &mut needed,
            &mut have,
            std::ptr::null_mut(),
            &mut reasons,
        )
    };
    if rc == ERROR_SUCCESS {
        return Ok(Vec::new());
    }
    if rc != ERROR_MORE_DATA {
        return Err(win32_error("RmGetList probe failed", rc));
    }

    for _ in 0..MAX_GETLIST_RETRIES {
        let mut buf: Vec<RM_PROCESS_INFO> = Vec::with_capacity(needed as usize);
        have = needed;
        let rc = unsafe {
            RmGetList(
                handle,
                &mut needed,
                &mut have,
                buf.as_mut_ptr(),
                &mut reasons,
            )
        };
        if rc == ERROR_SUCCESS {
            // Defensive cap. If RM ever wrote `have > needed` (RM bug, hostile
            // race, or kernel quirk), `set_len` on a Vec with `capacity == needed`
            // and `len > capacity` is immediate UB. The Microsoft docs say this
            // can't happen; one `min` removes the soundness hole entirely.
            debug_assert!(
                (have as usize) <= (needed as usize),
                "RmGetList wrote {} entries into a buffer sized for {}",
                have,
                needed
            );
            let actual = (have as usize).min(needed as usize);
            // SAFETY: actual ≤ needed = capacity. RM wrote `actual` valid
            // `RM_PROCESS_INFO`s into the buffer.
            unsafe {
                buf.set_len(actual);
            }
            return Ok(buf);
        }
        if rc == ERROR_MORE_DATA {
            continue; // grow on next iteration
        }
        return Err(win32_error("RmGetList read failed", rc));
    }
    Err(diagnostic_error(
        "RmGetList: ERROR_MORE_DATA persisted past retry budget",
    ))
}

/// `RM_PROCESS_INFO::strAppName` is a fixed `[u16; 256]` array, NUL-terminated.
fn process_info_app_name(info: &RM_PROCESS_INFO) -> String {
    let arr = &info.strAppName;
    let nul = arr.iter().position(|&c| c == 0).unwrap_or(arr.len());
    String::from_utf16_lossy(&arr[..nul])
}

/// Fallback PID → exe basename via `QueryFullProcessImageNameW`. Empty on failure.
fn pid_to_exe_basename(pid: u32) -> String {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return String::new();
        }
        // 1024 wide chars (2 KB stack) covers all practical exe paths on
        // Windows 10/11 with long-path support.
        let mut buf: [u16; 1024] = [0; 1024];
        let mut size: u32 = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut size);
        let _ = CloseHandle(h);
        if ok == 0 || size == 0 {
            return String::new();
        }
        let path = String::from_utf16_lossy(&buf[..size as usize]);
        path.rsplit(['\\', '/']).next().unwrap_or("").to_string()
    }
}

/// FILETIME equality — tolerates PID recycling between bulk and per-file passes.
fn same_start(
    a: &windows_sys::Win32::Foundation::FILETIME,
    b: &windows_sys::Win32::Foundation::FILETIME,
) -> bool {
    a.dwLowDateTime == b.dwLowDateTime && a.dwHighDateTime == b.dwHighDateTime
}

struct RmBulkResult {
    blockers: Vec<RM_PROCESS_INFO>,
    systemic_error: Option<DiagnosticError>,
    skipped_register_failures: usize,
    skipped_register_error_codes: Vec<Option<u32>>,
}

impl RmBulkResult {
    fn empty() -> Self {
        Self {
            blockers: Vec::new(),
            systemic_error: None,
            skipped_register_failures: 0,
            skipped_register_error_codes: Vec::new(),
        }
    }

    fn skipped(error: &DiagnosticError) -> Self {
        Self {
            blockers: Vec::new(),
            systemic_error: None,
            skipped_register_failures: 1,
            skipped_register_error_codes: vec![error.code],
        }
    }

    fn systemic(error: DiagnosticError) -> Self {
        Self {
            blockers: Vec::new(),
            systemic_error: Some(error),
            skipped_register_failures: 0,
            skipped_register_error_codes: Vec::new(),
        }
    }

    fn merge(&mut self, other: Self) {
        self.blockers.extend(other.blockers);
        if self.systemic_error.is_none() {
            self.systemic_error = other.systemic_error;
        }
        self.skipped_register_failures += other.skipped_register_failures;
        self.skipped_register_error_codes
            .extend(other.skipped_register_error_codes);
    }
}

/// Binary-search-fallback bulk register: try the whole batch in one session;
/// on RmRegisterResources error, drop the session, split the batch in half,
/// and recurse. Single-bad-file isolation at `wide.len() == 1`.
/// Bound: O(log N + bad_files) RM sessions.
fn collect_blockers_tolerant(wide: &[Vec<u16>], original_paths: &[&PathBuf]) -> RmBulkResult {
    if wide.is_empty() {
        return RmBulkResult::empty();
    }
    let session = match rm_start() {
        Ok(s) => s,
        Err(e) => {
            log::warn!(
                "[wg_delete_diagnostic] bulk-pass RmStartSession failed (giving up on this \
                sub-batch of {}): {}",
                wide.len(),
                e.message
            );
            return RmBulkResult::systemic(e);
        }
    };
    match rm_register(session.0, wide) {
        Ok(()) => match rm_get_list(session.0) {
            Ok(blockers) => RmBulkResult {
                blockers,
                systemic_error: None,
                skipped_register_failures: 0,
                skipped_register_error_codes: Vec::new(),
            },
            Err(e) => {
                log::warn!(
                    "[wg_delete_diagnostic] bulk-pass RmGetList failed for sub-batch of {}: {}",
                    wide.len(),
                    e.message
                );
                RmBulkResult::systemic(e)
            }
        },
        Err(e) if wide.len() == 1 => {
            log::warn!(
                "[wg_delete_diagnostic] skipping unregisterable file '{}': {}",
                original_paths[0].display(),
                e.message
            );
            RmBulkResult::skipped(&e)
        }
        Err(_) => {
            drop(session); // release before recursing
            let mid = wide.len() / 2;
            let mut left = collect_blockers_tolerant(&wide[..mid], &original_paths[..mid]);
            let right = collect_blockers_tolerant(&wide[mid..], &original_paths[mid..]);
            left.merge(right);
            left
        }
    }
}

fn blocker_processes_by_pid(bulk_list: &[RM_PROCESS_INFO]) -> HashMap<u32, BlockerProcess> {
    // Seed the result map with one entry per unique PID. Resolve the executable
    // name now (prefer `strAppName`, fall back to `QueryFullProcessImageNameW`).
    let mut by_pid: HashMap<u32, BlockerProcess> = HashMap::new();
    for info in bulk_list {
        let pid = info.Process.dwProcessId;
        if pid == 0 {
            // System Idle / kernel — never actionable, never a real blocker.
            continue;
        }
        let name = {
            let app = process_info_app_name(info);
            if app.is_empty() {
                let exe = pid_to_exe_basename(pid);
                if exe.is_empty() {
                    format!("pid {}", pid)
                } else {
                    exe
                }
            } else {
                app
            }
        };
        by_pid.entry(pid).or_insert(BlockerProcess {
            pid,
            name,
            cwd: None,
            files: Vec::new(),
        });
    }

    by_pid
}

/// Two-pass Restart Manager scan:
///
/// 1. **Bulk pass** — open RM sessions for batches of probed files (binary-search
///    fallback when RmRegisterResources rejects a batch), call `RmGetList` to learn
///    which PIDs hold any handles. Tolerant of single bad files; ~O(log N + bad_files)
///    sessions.
/// 2. **Per-file attribution pass** — RM doesn't tell us *which* file each PID
///    held. Iterate files; for each, open a fresh session, register only that
///    file, and accumulate (file → matching PID) entries. Cap each PID at
///    `MAX_FILES_PER_PROCESS` (5) and short-circuit once every blocker PID is
///    saturated.
#[cfg(windows)]
pub(super) fn scan_restart_manager_processes_windows(
    wg_dir: &Path,
) -> Result<Vec<BlockerProcess>, DiagnosticError> {
    // ── Phase 0: collect files to probe ──────────────────────────────────────
    let files = collect_files_to_probe(wg_dir);
    if files.is_empty() {
        return Ok(Vec::new());
    }
    // Defensive: drop files that vanished between collection and the FFI call.
    // `RmRegisterResources` aborts the batch on `ERROR_FILE_NOT_FOUND`.
    let alive: Vec<&PathBuf> = files.iter().filter(|p| p.exists()).collect();
    if alive.is_empty() {
        return Ok(Vec::new());
    }
    let wide_files: Vec<Vec<u16>> = alive
        .iter()
        .map(|p| to_wide_nul(&p.to_string_lossy()))
        .collect();

    // ── Phase 1: bulk pass — get the set of blocker PIDs ─────────────────────
    let bulk_result = collect_blockers_tolerant(&wide_files, &alive);
    let bulk_list: Vec<RM_PROCESS_INFO> = bulk_result.blockers;

    if bulk_list.is_empty() {
        if let Some(error) = bulk_result.systemic_error {
            return Err(error);
        }
        if bulk_result.skipped_register_failures > 0 {
            log::warn!(
                "[wg_delete_diagnostic] Restart Manager skipped {} unregisterable resources",
                bulk_result.skipped_register_failures
            );
        }
        if bulk_result.skipped_register_failures == wide_files.len()
            && bulk_result
                .skipped_register_error_codes
                .iter()
                .any(|code| *code != Some(2))
        {
            return Err(diagnostic_error(
                "RmRegisterResources failed for all probed resources",
            ));
        }
        return Ok(Vec::new());
    }

    let mut by_pid = blocker_processes_by_pid(&bulk_list);

    // ── Phase 2: per-file attribution ────────────────────────────────────────
    let target_pids: HashSet<u32> = by_pid.keys().copied().collect();
    for (path, wide) in alive.iter().zip(wide_files.iter()) {
        if by_pid
            .values()
            .all(|p| p.files.len() >= MAX_FILES_PER_PROCESS)
        {
            break; // every blocker has its quota — further probing wastes RM sessions
        }
        if !path.exists() {
            continue; // raced again; skip
        }
        let single = std::slice::from_ref(wide);

        let session = match rm_start() {
            Ok(s) => s,
            Err(e) => {
                log::warn!(
                    "[wg_delete_diagnostic] per-file RmStartSession failed for {}: {}",
                    path.display(),
                    e.message
                );
                continue;
            }
        };
        if let Err(e) = rm_register(session.0, single) {
            log::warn!(
                "[wg_delete_diagnostic] per-file RmRegisterResources failed for {}: {}",
                path.display(),
                e.message
            );
            continue; // file vanished mid-scan or RM rejected it; non-fatal
        }
        let list = match rm_get_list(session.0) {
            Ok(l) => l,
            Err(e) => {
                log::warn!(
                    "[wg_delete_diagnostic] per-file RmGetList failed for {}: {}",
                    path.display(),
                    e.message
                );
                continue;
            }
        };
        drop(session); // explicit RmEndSession before next iteration

        for info in &list {
            let pid = info.Process.dwProcessId;
            if !target_pids.contains(&pid) {
                continue;
            }
            // PID-recycle defence: the bulk pass saw a different process if the
            // ProcessStartTime FILETIME differs.
            if let Some(b) = bulk_list.iter().find(|b| b.Process.dwProcessId == pid) {
                if !same_start(&b.Process.ProcessStartTime, &info.Process.ProcessStartTime) {
                    continue;
                }
            }
            if let Some(entry) = by_pid.get_mut(&pid) {
                if entry.files.len() < MAX_FILES_PER_PROCESS {
                    entry.files.push(path.to_string_lossy().to_string());
                }
            }
        }
    }

    Ok(by_pid.into_values().collect())
}
