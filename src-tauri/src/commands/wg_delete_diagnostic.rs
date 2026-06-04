//! Diagnostic for `delete_workgroup` failures caused by file-in-use (Windows os error 32).
//!
//! Three scans:
//!   1. AC-internal sessions whose `working_directory` lives inside the workgroup tree.
//!   2. External processes holding handles on files inside the workgroup tree, via the
//!      Windows Restart Manager API (RmStartSession / RmRegisterResources / RmGetList).
//!   3. External Windows processes whose current working directory is under the
//!      workgroup tree, via a direct PEB ProcessParameters read.
//!
//! Pure helpers — no Tauri commands. Invoked from `entity_creation::delete_workgroup`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;

use crate::session::manager::SessionManager;

/// Locks the producer to the four-variant promise so the wire shape can't drift.
/// Lowercase JSON matches the TS literal union in `src/shared/types.ts`.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Windows,
    Linux,
    Macos,
    Other,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BlockerReport {
    pub workgroup: String,
    pub platform: Platform,
    pub diagnostic_available: bool,
    pub restart_manager_available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restart_manager_error: Option<DiagnosticError>,
    pub raw_os_error: String,
    pub raw_delete_error: String,
    pub live_sessions: Vec<BlockerSession>,
    pub exited_session_records_ignored: Vec<IgnoredSessionRecord>,
    pub external_processes: Vec<BlockerProcess>,
    pub sessions: Vec<BlockerSession>,
    pub processes: Vec<BlockerProcess>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BlockerSession {
    pub session_id: String,
    /// FQN like `agentscommander:wg-7-dev-team/architect` derived via
    /// `crate::config::teams::agent_fqn_from_path`.
    pub agent_name: String,
    pub cwd: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IgnoredSessionRecord {
    pub session_id: String,
    pub agent_name: String,
    pub cwd: String,
    pub status: String,
    pub waiting_for_input: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticError {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meaning: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BlockerProcess {
    pub pid: u32,
    /// Executable file name (e.g. "git.exe", "node.exe"). Best-effort.
    pub name: String,
    /// Current working directory if this process was identified by the CWD fallback.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Sample of paths inside the workgroup that this process holds. Capped at MAX_FILES_PER_PROCESS.
    pub files: Vec<String>,
}

#[derive(Debug, Default)]
struct AcSessionScan {
    live_sessions: Vec<BlockerSession>,
    exited_session_records_ignored: Vec<IgnoredSessionRecord>,
}

#[derive(Debug, Default)]
struct ExternalProcessScan {
    processes: Vec<BlockerProcess>,
    restart_manager_available: bool,
    restart_manager_error: Option<DiagnosticError>,
    cwd_scan_error: Option<DiagnosticError>,
}

const MAX_FILES_PER_PROCESS: usize = 5;
#[cfg(windows)]
const MAX_FILES_TO_PROBE: usize = 200;

fn win32_meaning(code: u32) -> &'static str {
    match code {
        2 => "File not found",
        5 => "Access denied",
        32 => "Sharing violation",
        33 => "Lock violation",
        87 => "Invalid parameter",
        234 => "More data available",
        1224 => "User mapped file",
        _ => "Unknown Win32 error",
    }
}

fn win32_error(context: &str, code: u32) -> DiagnosticError {
    let meaning = win32_meaning(code);
    DiagnosticError {
        message: format!("{}: WIN32_ERROR={} ({})", context, code, meaning),
        code: Some(code),
        meaning: Some(meaning.to_string()),
    }
}

fn diagnostic_error(message: impl Into<String>) -> DiagnosticError {
    DiagnosticError {
        message: message.into(),
        code: None,
        meaning: None,
    }
}

/// Top-level diagnostic. Always returns a `BlockerReport`; on non-Windows the body is empty
/// and `diagnostic_available = false`.
pub async fn diagnose_blockers(
    wg_dir: &Path,
    workgroup_name: &str,
    raw_os_error: &str,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
) -> BlockerReport {
    log::info!(
        "[wg_delete_diagnostic] starting blocker scan for workgroup '{}'",
        workgroup_name
    );
    let canonical_wg = canonicalize_for_compare(wg_dir);

    let ac_scan = scan_ac_sessions(&canonical_wg, session_mgr).await;

    // The Windows scan is fully synchronous (FFI + file-tree walk). We expect ~1 s
    // wall-time post-failure; running it on the current Tokio worker would block
    // that worker for the duration. Hand it off via `spawn_blocking` so other
    // async work (PTY reads, IPC events) keeps ticking. JoinError is treated as a
    // scan failure and falls through to the `diagnostic_available = false` path —
    // matches the FFI-error branch.
    #[cfg(windows)]
    let external_scan = {
        let wg_for_scan = canonical_wg.clone();
        match tokio::task::spawn_blocking(move || scan_external_processes_windows(&wg_for_scan))
            .await
        {
            Ok(scan) => scan,
            Err(join_err) => {
                log::warn!(
                    "[wg_delete_diagnostic] external process scan join failed: {}",
                    join_err
                );
                ExternalProcessScan {
                    processes: Vec::new(),
                    restart_manager_available: false,
                    restart_manager_error: Some(diagnostic_error(format!(
                        "external process scan join failed: {}",
                        join_err
                    ))),
                    cwd_scan_error: None,
                }
            }
        }
    };

    #[cfg(not(windows))]
    let external_scan = {
        let _ = canonical_wg;
        ExternalProcessScan {
            processes: Vec::new(),
            restart_manager_available: false,
            restart_manager_error: None,
            cwd_scan_error: None,
        }
    };

    let report = build_blocker_report(
        workgroup_name,
        detect_platform(),
        raw_os_error,
        ac_scan,
        external_scan,
    );

    log::info!(
        "[wg_delete_diagnostic] diagnostic done: live_sessions={}, exited_session_records_ignored={}, external_processes={}, restart_manager_available={}, restart_manager_error={}, raw_delete_error={}",
        report.live_sessions.len(),
        report.exited_session_records_ignored.len(),
        report.external_processes.len(),
        report.restart_manager_available,
        report
            .restart_manager_error
            .as_ref()
            .map(|e| e.message.as_str())
            .unwrap_or("none"),
        report.raw_delete_error
    );

    report
}

fn build_blocker_report(
    workgroup_name: &str,
    platform: Platform,
    raw_os_error: &str,
    ac_scan: AcSessionScan,
    external_scan: ExternalProcessScan,
) -> BlockerReport {
    let live_sessions = ac_scan.live_sessions;
    let ignored = ac_scan.exited_session_records_ignored;
    let external_processes = external_scan.processes;
    let diagnostic_available = matches!(platform, Platform::Windows)
        && (external_scan.restart_manager_available || !external_processes.is_empty());
    let sessions = live_sessions.clone();
    let processes = external_processes.clone();

    if let Some(cwd_err) = external_scan.cwd_scan_error.as_ref() {
        log::warn!(
            "[wg_delete_diagnostic] CWD fallback scan failed: {}",
            cwd_err.message
        );
    }

    BlockerReport {
        workgroup: workgroup_name.to_string(),
        platform,
        diagnostic_available,
        restart_manager_available: external_scan.restart_manager_available,
        restart_manager_error: external_scan.restart_manager_error,
        raw_os_error: raw_os_error.to_string(),
        raw_delete_error: raw_os_error.to_string(),
        live_sessions,
        exited_session_records_ignored: ignored,
        external_processes,
        sessions,
        processes,
    }
}

fn detect_platform() -> Platform {
    if cfg!(windows) {
        Platform::Windows
    } else if cfg!(target_os = "linux") {
        Platform::Linux
    } else if cfg!(target_os = "macos") {
        Platform::Macos
    } else {
        Platform::Other
    }
}

/// Strip Windows extended-length prefixes from a canonical path string.
/// Factored out of `canonicalize_for_compare` so unit tests can hit the
/// prefix-strip logic without disk I/O (G.4.8).
///
/// Order matters: `\\?\UNC\` starts with `\\?\`, so the longer prefix must
/// be tried first. `\\?\UNC\server\share\…` becomes `\\server\share\…` and
/// `\\?\C:\Users\…` becomes `C:\Users\…`.
fn strip_long_prefix_str(s: &str) -> String {
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{}", rest)
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_string()
    } else if let Some(rest) = s.strip_prefix(r"\??\UNC\") {
        format!(r"\\{}", rest)
    } else if let Some(rest) = s.strip_prefix(r"\??\") {
        rest.to_string()
    } else {
        s.to_string()
    }
}

/// Return wg_dir canonicalised, with the Windows extended-length prefixes stripped
/// for shape parity with `entity_creation.rs:903`.
fn canonicalize_for_compare(p: &Path) -> PathBuf {
    let canon = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let s = canon.to_string_lossy();
    PathBuf::from(strip_long_prefix_str(&s))
}

#[cfg(windows)]
fn path_is_under_windows(candidate: &Path, root: &Path) -> bool {
    fn normalize(p: &Path) -> String {
        strip_long_prefix_str(&p.to_string_lossy())
            .replace('/', r"\")
            .trim_end_matches('\\')
            .to_ascii_lowercase()
    }

    let candidate = normalize(candidate);
    let root = normalize(root);
    candidate == root || candidate.starts_with(&format!(r"{}\", root))
}

/// Walk the workgroup tree breadth-first and collect up to MAX_FILES_TO_PROBE absolute
/// resource paths to feed RmRegisterResources.
///
/// Priority order in the output:
///   1. **Directories** — WG root, top-level `repo-*` and `__agent_*` subdirs,
///      and `messaging/` if present. Surfaces dir-handle holders (terminal
///      cwd, IDE workspace open, file watchers via `ReadDirectoryChangesW`)
///      that file-only registration misses (#113 follow-up).
///   2. **Hot lock files** — lock-prone git metadata (`.lock`, `index`,
///      `HEAD`, etc.) so the budget can't be exhausted on a single
///      `.git/objects/` subtree.
///   3. **Cold files** — everything else, until the cap is hit.
///
/// Skips dirs we can't read; never follows symlinks.
#[cfg(windows)]
fn collect_files_to_probe(wg_dir: &Path) -> Vec<PathBuf> {
    use std::collections::VecDeque;

    /// Files commonly held by long-running processes inside an AC workgroup:
    /// any `*.lock` (git index.lock, ORIG_HEAD.lock, packfile locks), plus the
    /// lock-prone metadata files inside any `.git/` subtree.
    fn is_hot_lock_candidate(p: &Path) -> bool {
        let name = match p.file_name().and_then(|n| n.to_str()) {
            Some(s) => s,
            None => return false,
        };
        if name.ends_with(".lock") {
            return true;
        }
        let in_git = p
            .ancestors()
            .any(|a| a.file_name().and_then(|n| n.to_str()) == Some(".git"));
        in_git
            && matches!(
                name,
                "index" | "HEAD" | "ORIG_HEAD" | "FETCH_HEAD" | "MERGE_HEAD" | "packed-refs"
            )
    }

    /// Top-level WG-child dirs whose handles commonly indicate a blocker:
    /// `repo-*` (clones — git operations, IDE workspaces),
    /// `__agent_*` (replicas — agent-spawned shells holding cwd),
    /// `messaging/` (mailbox — file watchers).
    fn is_relevant_top_level_dir(name: &str) -> bool {
        name.starts_with("repo-") || name.starts_with("__agent_") || name == "messaging"
    }

    /// Soft ceiling on total walk size — once we've inventoried 4× the probe
    /// budget, we have plenty to choose from. Avoids walking gigabytes of
    /// `.git/objects/` when the WG is unusually large.
    const WALK_SOFT_CEILING: usize = MAX_FILES_TO_PROBE * 4;

    // Always include the WG root itself. A terminal cwd or IDE workspace open
    // anywhere under the tree usually surfaces via a handle on this directory.
    let mut dirs: Vec<PathBuf> = vec![wg_dir.to_path_buf()];
    let mut hot: Vec<PathBuf> = Vec::new();
    let mut cold: Vec<PathBuf> = Vec::new();
    let mut queue: VecDeque<PathBuf> = VecDeque::new();
    queue.push_back(wg_dir.to_path_buf());

    while let Some(dir) = queue.pop_front() {
        if hot.len() + cold.len() + dirs.len() >= WALK_SOFT_CEILING {
            break;
        }
        let is_wg_root = dir == wg_dir;
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let ft = match entry.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                if is_wg_root {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        if is_relevant_top_level_dir(name) {
                            dirs.push(path.clone());
                        }
                    }
                }
                queue.push_back(path);
            } else if ft.is_file() {
                if is_hot_lock_candidate(&path) {
                    hot.push(path);
                } else {
                    cold.push(path);
                }
            }
        }
    }

    // Dirs first (the new signal — small set, ~5–10 entries; prefer over cold
    // files as the plan dictates), then hot files, then cold files. All
    // capped at MAX_FILES_TO_PROBE total.
    let mut out = dirs;
    out.truncate(MAX_FILES_TO_PROBE);
    let remaining = MAX_FILES_TO_PROBE.saturating_sub(out.len());
    out.extend(hot.into_iter().take(remaining));
    let remaining = MAX_FILES_TO_PROBE.saturating_sub(out.len());
    out.extend(cold.into_iter().take(remaining));
    out
}

/// Scan SessionManager for sessions whose working_directory lives under wg_dir.
async fn scan_ac_sessions(
    canonical_wg: &Path,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
) -> AcSessionScan {
    use crate::session::session::{is_live_session_record, SessionStatus};

    let mgr = {
        let guard = session_mgr.read().await;
        guard.clone()
    };
    let sessions = mgr.list_sessions().await;

    let mut scan = AcSessionScan::default();
    for s in sessions {
        let cwd_canon = canonicalize_for_compare(Path::new(&s.working_directory));
        if !cwd_canon.starts_with(canonical_wg) {
            continue;
        }
        let agent_name = crate::config::teams::agent_fqn_from_path(&s.working_directory);
        if is_live_session_record(!s.id.is_empty(), Some(&s.status)) {
            scan.live_sessions.push(BlockerSession {
                session_id: s.id,
                agent_name,
                cwd: s.working_directory,
            });
            continue;
        }
        if matches!(&s.status, SessionStatus::Exited(_)) {
            scan.exited_session_records_ignored
                .push(IgnoredSessionRecord {
                    session_id: s.id,
                    agent_name,
                    cwd: s.working_directory,
                    status: format!("{:?}", s.status),
                    waiting_for_input: s.waiting_for_input,
                });
        }
    }
    scan
}

#[cfg(windows)]
fn scan_external_processes_windows(wg_dir: &Path) -> ExternalProcessScan {
    let rm = scan_restart_manager_processes_windows(wg_dir);
    let cwd = scan_cwd_processes_windows(wg_dir).map_err(diagnostic_error);

    match (rm, cwd) {
        (Ok(rm_processes), Ok(cwd_processes)) => ExternalProcessScan {
            processes: merge_blocker_processes(rm_processes, cwd_processes),
            restart_manager_available: true,
            restart_manager_error: None,
            cwd_scan_error: None,
        },
        (Ok(rm_processes), Err(cwd_err)) => {
            log::warn!(
                "[wg_delete_diagnostic] CWD fallback scan failed; preserving Restart Manager result: {}",
                cwd_err.message
            );
            ExternalProcessScan {
                processes: rm_processes,
                restart_manager_available: true,
                restart_manager_error: None,
                cwd_scan_error: Some(cwd_err),
            }
        }
        (Err(rm_err), Ok(cwd_processes)) => {
            log::warn!(
                "[wg_delete_diagnostic] Restart Manager scan failed; preserving CWD fallback result: {}",
                rm_err.message
            );
            ExternalProcessScan {
                processes: cwd_processes,
                restart_manager_available: false,
                restart_manager_error: Some(rm_err),
                cwd_scan_error: None,
            }
        }
        (Err(rm_err), Err(cwd_err)) => {
            log::warn!(
                "[wg_delete_diagnostic] CWD fallback scan failed after Restart Manager failure: {}",
                cwd_err.message
            );
            ExternalProcessScan {
                processes: Vec::new(),
                restart_manager_available: false,
                restart_manager_error: Some(rm_err),
                cwd_scan_error: Some(cwd_err),
            }
        }
    }
}

#[cfg(windows)]
fn merge_blocker_processes(
    rm_processes: Vec<BlockerProcess>,
    cwd_processes: Vec<BlockerProcess>,
) -> Vec<BlockerProcess> {
    use std::collections::HashMap;

    let mut by_pid: HashMap<u32, BlockerProcess> = HashMap::new();
    for process in rm_processes.into_iter().chain(cwd_processes) {
        by_pid
            .entry(process.pid)
            .and_modify(|existing| {
                if existing.cwd.is_none() {
                    existing.cwd = process.cwd.clone();
                }
                for file in &process.files {
                    if existing.files.len() < MAX_FILES_PER_PROCESS
                        && !existing.files.contains(file)
                    {
                        existing.files.push(file.clone());
                    }
                }
                // PID-only cross-source merge is best effort. Preserve a
                // non-placeholder RM name instead of replacing it with a later
                // CWD snapshot name.
                if existing.name.starts_with("pid ") && !process.name.starts_with("pid ") {
                    existing.name = process.name.clone();
                }
            })
            .or_insert(process);
    }

    let mut out: Vec<BlockerProcess> = by_pid.into_values().collect();
    out.sort_by_key(|p| p.pid);
    out
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
fn scan_restart_manager_processes_windows(
    wg_dir: &Path,
) -> Result<Vec<BlockerProcess>, DiagnosticError> {
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
                    "[wg_delete_diagnostic] bulk-pass RmStartSession failed (giving up on this sub-batch of {}): {}",
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

    // Seed the result map with one entry per unique PID. Resolve the executable
    // name now (prefer `strAppName`, fall back to `QueryFullProcessImageNameW`).
    let mut by_pid: HashMap<u32, BlockerProcess> = HashMap::new();
    for info in &bulk_list {
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

#[cfg(windows)]
#[derive(Debug)]
struct ProcessSnapshotEntry {
    pid: u32,
    name: String,
}

#[cfg(windows)]
struct HandleGuard(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl HandleGuard {
    fn raw(&self) -> windows_sys::Win32::Foundation::HANDLE {
        self.0
    }
}

#[cfg(windows)]
impl Drop for HandleGuard {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};

        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy)]
struct ProcessBasicInformationRaw {
    exit_status: windows_sys::Win32::Foundation::NTSTATUS,
    peb_base_address: *mut core::ffi::c_void,
    affinity_mask: usize,
    base_priority: i32,
    unique_process_id: usize,
    inherited_from_unique_process_id: usize,
}

#[cfg(all(windows, target_pointer_width = "64"))]
#[repr(C)]
#[derive(Clone, Copy)]
struct RemotePebPrefix64 {
    reserved: [u8; 0x20],
    process_parameters: *mut core::ffi::c_void,
}

#[cfg(all(windows, target_pointer_width = "64"))]
#[repr(C)]
#[derive(Clone, Copy)]
struct RemoteUnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *mut u16,
}

#[cfg(all(windows, target_pointer_width = "64"))]
#[repr(C)]
#[derive(Clone, Copy)]
struct RemoteCurDir {
    dos_path: RemoteUnicodeString,
    handle: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(all(windows, target_pointer_width = "64"))]
#[repr(C)]
#[derive(Clone, Copy)]
struct RemoteProcessParametersPrefix {
    maximum_length: u32,
    length: u32,
    flags: u32,
    debug_flags: u32,
    console_handle: windows_sys::Win32::Foundation::HANDLE,
    console_flags: u32,
    standard_input: windows_sys::Win32::Foundation::HANDLE,
    standard_output: windows_sys::Win32::Foundation::HANDLE,
    standard_error: windows_sys::Win32::Foundation::HANDLE,
    current_directory: RemoteCurDir,
}

#[cfg(all(windows, target_pointer_width = "64"))]
#[repr(C)]
#[derive(Clone, Copy)]
struct RemotePebPrefix32 {
    reserved: [u8; 0x10],
    process_parameters: u32,
}

#[cfg(all(windows, target_pointer_width = "64"))]
#[repr(C)]
#[derive(Clone, Copy)]
struct RemoteUnicodeString32 {
    length: u16,
    maximum_length: u16,
    buffer: u32,
}

#[cfg(all(windows, target_pointer_width = "64"))]
#[repr(C)]
#[derive(Clone, Copy)]
struct RemoteCurDir32 {
    dos_path: RemoteUnicodeString32,
    handle: u32,
}

#[cfg(all(windows, target_pointer_width = "64"))]
#[repr(C)]
#[derive(Clone, Copy)]
struct RemoteProcessParametersPrefix32 {
    maximum_length: u32,
    length: u32,
    flags: u32,
    debug_flags: u32,
    console_handle: u32,
    console_flags: u32,
    standard_input: u32,
    standard_output: u32,
    standard_error: u32,
    current_directory: RemoteCurDir32,
}

#[cfg(windows)]
const MAX_CWD_BYTES: usize = 32 * 1024;

#[cfg(windows)]
fn scan_cwd_processes_windows(wg_dir: &Path) -> Result<Vec<BlockerProcess>, String> {
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;

    let canonical_wg = canonicalize_for_compare(wg_dir);
    let current_pid = unsafe { GetCurrentProcessId() };
    let mut out = Vec::new();

    if let Some(blocker) = current_process_cwd_blocker(&canonical_wg, current_pid) {
        out.push(blocker);
    }

    for process in enumerate_processes_windows()? {
        if process.pid == 0 || process.pid == current_pid {
            continue;
        }
        let Some(cwd) = read_process_cwd_windows(process.pid) else {
            continue;
        };
        let cwd_compare = canonicalize_for_compare(Path::new(&cwd));
        if path_is_under_windows(&cwd_compare, &canonical_wg) {
            out.push(BlockerProcess {
                pid: process.pid,
                name: process.name,
                cwd: Some(cwd),
                files: Vec::new(),
            });
        }
    }

    Ok(out)
}

#[cfg(windows)]
fn current_process_cwd_blocker(canonical_wg: &Path, current_pid: u32) -> Option<BlockerProcess> {
    let current_dir = std::env::current_dir().ok()?;
    let current_exe = std::env::current_exe().ok();
    current_process_cwd_blocker_from_parts(
        canonical_wg,
        current_pid,
        &current_dir,
        current_exe.as_deref(),
    )
}

#[cfg(windows)]
fn current_process_cwd_blocker_from_parts(
    canonical_wg: &Path,
    current_pid: u32,
    current_dir: &Path,
    current_exe: Option<&Path>,
) -> Option<BlockerProcess> {
    let cwd_compare = canonicalize_for_compare(current_dir);
    if !path_is_under_windows(&cwd_compare, canonical_wg) {
        return None;
    }

    let name = current_exe
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .unwrap_or("current process")
        .to_string();

    Some(BlockerProcess {
        pid: current_pid,
        name,
        cwd: Some(strip_long_prefix_str(&current_dir.to_string_lossy())),
        files: Vec::new(),
    })
}

#[cfg(windows)]
fn enumerate_processes_windows() -> Result<Vec<ProcessSnapshotEntry>, String> {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err("CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS) failed".into());
    }
    let snapshot = HandleGuard(snapshot);

    let mut entry = unsafe { std::mem::zeroed::<PROCESSENTRY32W>() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;

    let mut out = Vec::new();
    let mut ok = unsafe { Process32FirstW(snapshot.raw(), &mut entry) };
    while ok != 0 {
        out.push(ProcessSnapshotEntry {
            pid: entry.th32ProcessID,
            name: nul_terminated_utf16(&entry.szExeFile),
        });
        ok = unsafe { Process32NextW(snapshot.raw(), &mut entry) };
    }

    Ok(out)
}

#[cfg(windows)]
fn nul_terminated_utf16(buf: &[u16]) -> String {
    let nul = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..nul])
}

#[cfg(windows)]
fn read_process_cwd_windows(pid: u32) -> Option<String> {
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
    };

    let handle =
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, 0, pid) };
    if handle.is_null() {
        log::debug!(
            "[wg_delete_diagnostic] CWD scan: OpenProcess failed for pid {}",
            pid
        );
        return None;
    }
    let handle = HandleGuard(handle);

    read_process_cwd_from_handle(handle.raw()).or_else(|| {
        log::debug!(
            "[wg_delete_diagnostic] CWD scan: unable to read cwd for pid {}",
            pid
        );
        None
    })
}

#[cfg(all(windows, target_pointer_width = "64"))]
fn read_process_cwd_from_handle(handle: windows_sys::Win32::Foundation::HANDLE) -> Option<String> {
    if let Some(wow64_peb) = query_wow64_peb_address(handle) {
        return read_process_cwd_wow64_32(handle, wow64_peb);
    }

    let pbi = query_process_basic_info(handle)?;
    if pbi.peb_base_address.is_null() {
        return None;
    }
    let peb: RemotePebPrefix64 = read_remote_struct(handle, pbi.peb_base_address as usize)?;
    if peb.process_parameters.is_null() {
        return None;
    }
    let params: RemoteProcessParametersPrefix =
        read_remote_struct(handle, peb.process_parameters as usize)?;
    read_remote_unicode_string(handle, params.current_directory.dos_path)
}

#[cfg(all(windows, target_pointer_width = "32"))]
fn read_process_cwd_from_handle(_handle: windows_sys::Win32::Foundation::HANDLE) -> Option<String> {
    // 32-bit AC builds reading remote ProcessParameters are out of scope for
    // this release. The shipped Windows app is 64-bit, which covers the
    // reported terminal-blocker repro.
    None
}

#[cfg(windows)]
fn query_process_basic_info(
    handle: windows_sys::Win32::Foundation::HANDLE,
) -> Option<ProcessBasicInformationRaw> {
    use windows_sys::Wdk::System::Threading::{NtQueryInformationProcess, ProcessBasicInformation};

    let mut pbi = std::mem::MaybeUninit::<ProcessBasicInformationRaw>::uninit();
    let mut return_len: u32 = 0;
    let status = unsafe {
        NtQueryInformationProcess(
            handle,
            ProcessBasicInformation,
            pbi.as_mut_ptr() as *mut core::ffi::c_void,
            std::mem::size_of::<ProcessBasicInformationRaw>() as u32,
            &mut return_len,
        )
    };
    if status != 0 {
        return None;
    }
    Some(unsafe { pbi.assume_init() })
}

#[cfg(all(windows, target_pointer_width = "64"))]
fn query_wow64_peb_address(handle: windows_sys::Win32::Foundation::HANDLE) -> Option<usize> {
    use windows_sys::Wdk::System::Threading::{NtQueryInformationProcess, ProcessWow64Information};

    let mut peb_address: usize = 0;
    let mut return_len: u32 = 0;
    let status = unsafe {
        NtQueryInformationProcess(
            handle,
            ProcessWow64Information,
            &mut peb_address as *mut usize as *mut core::ffi::c_void,
            std::mem::size_of::<usize>() as u32,
            &mut return_len,
        )
    };
    if status == 0 && peb_address != 0 {
        Some(peb_address)
    } else {
        None
    }
}

#[cfg(windows)]
fn read_remote_struct<T: Copy>(
    handle: windows_sys::Win32::Foundation::HANDLE,
    address: usize,
) -> Option<T> {
    use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;

    if address == 0 {
        return None;
    }
    let mut out = std::mem::MaybeUninit::<T>::uninit();
    let requested = std::mem::size_of::<T>();
    let mut bytes_read: usize = 0;
    let ok = unsafe {
        ReadProcessMemory(
            handle,
            address as *const core::ffi::c_void,
            out.as_mut_ptr() as *mut core::ffi::c_void,
            requested,
            &mut bytes_read,
        )
    };
    if ok == 0 || bytes_read != requested {
        return None;
    }
    Some(unsafe { out.assume_init() })
}

#[cfg(windows)]
fn read_remote_bytes(
    handle: windows_sys::Win32::Foundation::HANDLE,
    address: usize,
    len: usize,
) -> Option<Vec<u8>> {
    use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;

    if address == 0 || len == 0 || len > MAX_CWD_BYTES {
        return None;
    }
    let mut buf = vec![0u8; len];
    let mut bytes_read: usize = 0;
    let ok = unsafe {
        ReadProcessMemory(
            handle,
            address as *const core::ffi::c_void,
            buf.as_mut_ptr() as *mut core::ffi::c_void,
            len,
            &mut bytes_read,
        )
    };
    if ok == 0 || bytes_read != len {
        return None;
    }
    Some(buf)
}

#[cfg(all(windows, target_pointer_width = "64"))]
fn read_process_cwd_wow64_32(
    handle: windows_sys::Win32::Foundation::HANDLE,
    peb_address: usize,
) -> Option<String> {
    let peb: RemotePebPrefix32 = read_remote_struct(handle, peb_address)?;
    if peb.process_parameters == 0 {
        return None;
    }
    let params: RemoteProcessParametersPrefix32 =
        read_remote_struct(handle, peb.process_parameters as usize)?;
    read_remote_unicode_string32(handle, params.current_directory.dos_path)
}

#[cfg(all(windows, target_pointer_width = "64"))]
fn read_remote_unicode_string(
    handle: windows_sys::Win32::Foundation::HANDLE,
    remote: RemoteUnicodeString,
) -> Option<String> {
    read_remote_utf16_path(
        handle,
        remote.buffer as usize,
        remote.length,
        remote.maximum_length,
    )
}

#[cfg(all(windows, target_pointer_width = "64"))]
fn read_remote_unicode_string32(
    handle: windows_sys::Win32::Foundation::HANDLE,
    remote: RemoteUnicodeString32,
) -> Option<String> {
    read_remote_utf16_path(
        handle,
        remote.buffer as usize,
        remote.length,
        remote.maximum_length,
    )
}

#[cfg(windows)]
fn read_remote_utf16_path(
    handle: windows_sys::Win32::Foundation::HANDLE,
    buffer: usize,
    length: u16,
    maximum_length: u16,
) -> Option<String> {
    let length = usize::from(length);
    let maximum_length = usize::from(maximum_length);
    if length == 0
        || length % 2 != 0
        || maximum_length % 2 != 0
        || length > maximum_length
        || length > MAX_CWD_BYTES
        || maximum_length > MAX_CWD_BYTES + 2
        || buffer == 0
    {
        return None;
    }

    let bytes = read_remote_bytes(handle, buffer, length)?;
    let wide: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .collect();
    let cwd = strip_long_prefix_str(String::from_utf16_lossy(&wide).trim_end_matches('\0'));
    if cwd.is_empty() {
        None
    } else {
        Some(cwd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §7.23 (covers G.4.8): `strip_long_prefix_str` handles drive paths,
    /// UNC paths, and unprefixed paths uniformly.
    #[test]
    fn strip_long_prefix_str_handles_drive_unc_and_no_prefix() {
        assert_eq!(
            strip_long_prefix_str(r"\\?\UNC\server\share\proj"),
            r"\\server\share\proj"
        );
        assert_eq!(
            strip_long_prefix_str(r"\??\UNC\server\share\proj"),
            r"\\server\share\proj"
        );
        assert_eq!(
            strip_long_prefix_str(r"\\?\C:\Users\me\proj"),
            r"C:\Users\me\proj"
        );
        assert_eq!(
            strip_long_prefix_str(r"\??\C:\Users\me\proj"),
            r"C:\Users\me\proj"
        );
        assert_eq!(
            strip_long_prefix_str(r"C:\Users\me\proj"),
            r"C:\Users\me\proj"
        );
        // Edge: empty string and prefix-only.
        assert_eq!(strip_long_prefix_str(""), "");
        assert_eq!(strip_long_prefix_str(r"\\?\"), "");
    }

    /// §7.2: `canonicalize_for_compare` round-trips against
    /// `std::fs::canonicalize` + manual prefix strip.
    #[test]
    fn canonicalize_for_compare_strips_prefix_for_existing_path() {
        let tmp = std::env::temp_dir();
        let canon_via_helper = canonicalize_for_compare(&tmp);
        let canon_raw = std::fs::canonicalize(&tmp).expect("canonicalize tempdir");
        let raw_str = canon_raw.to_string_lossy();
        let expected = strip_long_prefix_str(&raw_str);
        assert_eq!(canon_via_helper.to_string_lossy(), expected);
    }

    /// §7.3: `scan_ac_sessions` filters sessions by canonical-path prefix.
    /// Exercises the path canonicalisation + `PathBuf::starts_with` filter
    /// on real disk paths.
    #[tokio::test]
    async fn scan_ac_sessions_filters_by_canonical_prefix() {
        use crate::session::manager::SessionManager;

        let tmp = tempfile::tempdir().expect("tempdir");
        let wg_dir = tmp.path().join("wg-test");
        let inside_session_dir = wg_dir.join("__agent_inside");
        let outside_session_dir = tmp.path().join("outside");
        std::fs::create_dir_all(&inside_session_dir).expect("create inside dir");
        std::fs::create_dir_all(&outside_session_dir).expect("create outside dir");

        let mgr = SessionManager::new();
        let _ = mgr
            .create_session(
                "powershell.exe".into(),
                vec![],
                inside_session_dir.to_string_lossy().to_string(),
                None,
                None,
                vec![],
                false,
            )
            .await
            .expect("create inside session");
        let _ = mgr
            .create_session(
                "powershell.exe".into(),
                vec![],
                outside_session_dir.to_string_lossy().to_string(),
                None,
                None,
                vec![],
                false,
            )
            .await
            .expect("create outside session");
        let mgr = Arc::new(tokio::sync::RwLock::new(mgr));

        let canonical_wg = canonicalize_for_compare(&wg_dir);
        let blockers = scan_ac_sessions(&canonical_wg, &mgr).await;

        assert_eq!(
            blockers.live_sessions.len(),
            1,
            "exactly one session inside wg_dir"
        );
        assert!(
            blockers.live_sessions[0].cwd.contains("__agent_inside"),
            "filtered session must be the inside one, got cwd={}",
            blockers.live_sessions[0].cwd
        );
        assert!(blockers.exited_session_records_ignored.is_empty());
    }

    #[tokio::test]
    async fn scan_ac_sessions_ignores_exited_records_under_workgroup() {
        use crate::session::manager::SessionManager;

        let tmp = tempfile::tempdir().expect("tempdir");
        let wg_dir = tmp.path().join("wg-test");
        let inside_session_dir = wg_dir.join("__agent_inside");
        std::fs::create_dir_all(&inside_session_dir).expect("create inside dir");

        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "powershell.exe".into(),
                vec![],
                inside_session_dir.to_string_lossy().to_string(),
                None,
                None,
                vec![],
                false,
            )
            .await
            .expect("create inside session");
        mgr.mark_exited(session.id, 0).await;
        let mgr = Arc::new(tokio::sync::RwLock::new(mgr));

        let canonical_wg = canonicalize_for_compare(&wg_dir);
        let blockers = scan_ac_sessions(&canonical_wg, &mgr).await;

        assert!(blockers.live_sessions.is_empty());
        assert_eq!(blockers.exited_session_records_ignored.len(), 1);
        let ignored = &blockers.exited_session_records_ignored[0];
        assert_eq!(ignored.status, "Exited(0)");
        assert!(!ignored.waiting_for_input);
    }

    #[tokio::test]
    async fn scan_ac_sessions_splits_live_and_exited_records() {
        use crate::session::manager::SessionManager;

        let tmp = tempfile::tempdir().expect("tempdir");
        let wg_dir = tmp.path().join("wg-test");
        let live_dir = wg_dir.join("__agent_live");
        let exited_dir = wg_dir.join("__agent_exited");
        std::fs::create_dir_all(&live_dir).expect("create live dir");
        std::fs::create_dir_all(&exited_dir).expect("create exited dir");

        let mgr = SessionManager::new();
        let _live = mgr
            .create_session(
                "powershell.exe".into(),
                vec![],
                live_dir.to_string_lossy().to_string(),
                None,
                None,
                vec![],
                false,
            )
            .await
            .expect("create live session");
        let exited = mgr
            .create_session(
                "powershell.exe".into(),
                vec![],
                exited_dir.to_string_lossy().to_string(),
                None,
                None,
                vec![],
                false,
            )
            .await
            .expect("create exited session");
        mgr.mark_exited(exited.id, 0).await;
        let mgr = Arc::new(tokio::sync::RwLock::new(mgr));

        let canonical_wg = canonicalize_for_compare(&wg_dir);
        let blockers = scan_ac_sessions(&canonical_wg, &mgr).await;

        assert_eq!(blockers.live_sessions.len(), 1);
        assert_eq!(blockers.exited_session_records_ignored.len(), 1);
        assert!(blockers.live_sessions[0].cwd.contains("__agent_live"));
        assert!(blockers.exited_session_records_ignored[0]
            .cwd
            .contains("__agent_exited"));
    }

    /// §7.4 (with R.1.a fix): `BlockerReport` JSON shape preserves camelCase
    /// at the wire boundary AND the `Platform` enum serializes lowercase.
    #[test]
    fn blocker_report_serializes_with_camelcase_fields() {
        let report = BlockerReport {
            workgroup: "wg-7-dev-team".into(),
            platform: Platform::Windows,
            diagnostic_available: true,
            restart_manager_available: false,
            restart_manager_error: Some(DiagnosticError {
                message: "RmStartSession failed: WIN32_ERROR=5 (Access denied)".into(),
                code: Some(5),
                meaning: Some("Access denied".into()),
            }),
            raw_os_error: "...".into(),
            raw_delete_error: "... (os error 32)".into(),
            live_sessions: vec![BlockerSession {
                session_id: "abc".into(),
                agent_name: "agentscommander:wg-7-dev-team/architect".into(),
                cwd: r"C:\foo".into(),
            }],
            exited_session_records_ignored: vec![IgnoredSessionRecord {
                session_id: "def".into(),
                agent_name: "agentscommander:wg-7-dev-team/dev-rust".into(),
                cwd: r"C:\foo\__agent_dev-rust".into(),
                status: "Exited(0)".into(),
                waiting_for_input: false,
            }],
            external_processes: vec![BlockerProcess {
                pid: 42,
                name: "git.exe".into(),
                cwd: Some(r"C:\foo".into()),
                files: vec![r"C:\foo\bar".into()],
            }],
            sessions: vec![BlockerSession {
                session_id: "abc".into(),
                agent_name: "agentscommander:wg-7-dev-team/architect".into(),
                cwd: r"C:\foo".into(),
            }],
            processes: vec![BlockerProcess {
                pid: 42,
                name: "git.exe".into(),
                cwd: Some(r"C:\foo".into()),
                files: vec![r"C:\foo\bar".into()],
            }],
        };
        let json: serde_json::Value = serde_json::to_value(&report).expect("serialize");
        // Top-level fields
        for k in &[
            "workgroup",
            "platform",
            "diagnosticAvailable",
            "restartManagerAvailable",
            "restartManagerError",
            "rawOsError",
            "rawDeleteError",
            "liveSessions",
            "exitedSessionRecordsIgnored",
            "externalProcesses",
            "sessions",
            "processes",
        ] {
            assert!(json.get(*k).is_some(), "missing field: {}", k);
        }
        // R.1.a: Platform enum must serialize to lowercase string.
        assert_eq!(
            json.get("platform").and_then(|v| v.as_str()),
            Some("windows"),
            "Platform enum must serialize to lowercase string"
        );
        // BlockerSession fields
        let s = &json["liveSessions"][0];
        for k in &["sessionId", "agentName", "cwd"] {
            assert!(s.get(*k).is_some(), "missing session field: {}", k);
        }
        // BlockerProcess fields
        let p = &json["externalProcesses"][0];
        for k in &["pid", "name", "cwd", "files"] {
            assert!(p.get(*k).is_some(), "missing process field: {}", k);
        }
        let ignored = &json["exitedSessionRecordsIgnored"][0];
        for k in &["sessionId", "agentName", "cwd", "status", "waitingForInput"] {
            assert!(ignored.get(*k).is_some(), "missing ignored field: {}", k);
        }
        // Snake-case must NOT leak at the wire boundary.
        for k in &[
            "diagnostic_available",
            "restart_manager_available",
            "restart_manager_error",
            "raw_os_error",
            "raw_delete_error",
            "live_sessions",
            "exited_session_records_ignored",
            "external_processes",
            "session_id",
            "agent_name",
            "waiting_for_input",
        ] {
            assert!(
                json.get(*k).is_none(),
                "leaked snake_case field at top level: {}",
                k
            );
        }
        for k in &["session_id", "agent_name"] {
            assert!(
                s.get(*k).is_none(),
                "leaked snake_case session field: {}",
                k
            );
        }
        assert!(ignored.get("waiting_for_input").is_none());
    }

    #[test]
    fn win32_error_formats_access_denied() {
        let err = win32_error("RmStartSession failed", 5);
        assert_eq!(err.code, Some(5));
        assert_eq!(err.meaning.as_deref(), Some("Access denied"));
        assert!(err.message.contains("WIN32_ERROR=5 (Access denied)"));
    }

    #[test]
    fn win32_error_formats_unknown_code() {
        let err = win32_error("RmGetList failed", 99999);
        assert_eq!(err.code, Some(99999));
        assert_eq!(err.meaning.as_deref(), Some("Unknown Win32 error"));
        assert!(err
            .message
            .contains("WIN32_ERROR=99999 (Unknown Win32 error)"));
    }

    fn sample_live_session(name: &str) -> BlockerSession {
        BlockerSession {
            session_id: format!("session-{name}"),
            agent_name: format!("agentscommander:wg-7-dev-team/{name}"),
            cwd: format!(r"C:\work\wg-7-dev-team\__agent_{name}"),
        }
    }

    fn sample_ignored_session(name: &str) -> IgnoredSessionRecord {
        IgnoredSessionRecord {
            session_id: format!("session-{name}"),
            agent_name: format!("agentscommander:wg-7-dev-team/{name}"),
            cwd: format!(r"C:\work\wg-7-dev-team\__agent_{name}"),
            status: "Exited(0)".into(),
            waiting_for_input: false,
        }
    }

    fn sample_process(pid: u32) -> BlockerProcess {
        BlockerProcess {
            pid,
            name: "powershell.exe".into(),
            cwd: Some(r"C:\work\wg-7-dev-team".into()),
            files: Vec::new(),
        }
    }

    #[test]
    fn build_blocker_report_live_and_ignored_sessions() {
        let ac_scan = AcSessionScan {
            live_sessions: vec![sample_live_session("dev-rust")],
            exited_session_records_ignored: vec![sample_ignored_session("architect")],
        };
        let report = build_blocker_report(
            "wg-7-dev-team",
            Platform::Windows,
            "sharing violation",
            ac_scan,
            ExternalProcessScan::default(),
        );

        assert_eq!(report.live_sessions.len(), 1);
        assert_eq!(report.exited_session_records_ignored.len(), 1);
        assert_eq!(report.sessions, report.live_sessions);
        assert_eq!(report.processes, report.external_processes);
        assert_eq!(report.raw_os_error, "sharing violation");
        assert_eq!(report.raw_delete_error, "sharing violation");
    }

    #[test]
    fn build_blocker_report_rm_failure_with_cwd_processes() {
        let rm_error = win32_error("RmStartSession failed", 5);
        let report = build_blocker_report(
            "wg-7-dev-team",
            Platform::Windows,
            "sharing violation",
            AcSessionScan::default(),
            ExternalProcessScan {
                processes: vec![sample_process(42)],
                restart_manager_available: false,
                restart_manager_error: Some(rm_error.clone()),
                cwd_scan_error: None,
            },
        );

        assert_eq!(report.restart_manager_error, Some(rm_error));
        assert!(!report.restart_manager_available);
        assert!(report.diagnostic_available);
        assert_eq!(report.processes, report.external_processes);
        assert_eq!(report.sessions, report.live_sessions);
    }

    #[test]
    fn build_blocker_report_rm_failure_no_processes() {
        let rm_error = win32_error("RmGetList probe failed", 87);
        let report = build_blocker_report(
            "wg-7-dev-team",
            Platform::Windows,
            "lock violation",
            AcSessionScan::default(),
            ExternalProcessScan {
                processes: Vec::new(),
                restart_manager_available: false,
                restart_manager_error: Some(rm_error.clone()),
                cwd_scan_error: None,
            },
        );

        assert_eq!(report.restart_manager_error, Some(rm_error));
        assert!(!report.restart_manager_available);
        assert!(!report.diagnostic_available);
        assert_eq!(report.raw_os_error, "lock violation");
        assert_eq!(report.raw_delete_error, "lock violation");
        assert_eq!(report.processes, report.external_processes);
        assert_eq!(report.sessions, report.live_sessions);
    }

    #[test]
    fn blocker_report_serializes_without_windows_scanner_fields_present() {
        let report = build_blocker_report(
            "wg-7-dev-team",
            Platform::Linux,
            "permission denied",
            AcSessionScan::default(),
            ExternalProcessScan::default(),
        );

        let json: serde_json::Value = serde_json::to_value(&report).expect("serialize");
        assert_eq!(
            json.get("restartManagerAvailable")
                .and_then(|v| v.as_bool()),
            Some(false)
        );
        assert!(json.get("restartManagerError").is_none());
        assert!(json.get("externalProcesses").is_some());
    }

    /// §7.1 (Windows variant): `is_file_in_use_error` matches `ERROR_SHARING_VIOLATION` (32).
    #[cfg(windows)]
    #[test]
    fn is_file_in_use_error_matches_sharing_violation_on_windows() {
        use crate::commands::entity_creation::is_file_in_use_error;
        let e = std::io::Error::from_raw_os_error(32);
        assert!(is_file_in_use_error(&e), "os error 32 must match");
    }

    /// §7.1 (Windows variant): `is_file_in_use_error` matches `ERROR_LOCK_VIOLATION` (33).
    #[cfg(windows)]
    #[test]
    fn is_file_in_use_error_matches_lock_violation_on_windows() {
        use crate::commands::entity_creation::is_file_in_use_error;
        let e = std::io::Error::from_raw_os_error(33);
        assert!(is_file_in_use_error(&e), "os error 33 must match");
    }

    /// §7.1 (Windows variant): `is_file_in_use_error` matches `ERROR_USER_MAPPED_FILE` (1224).
    /// This is the VSCode / IDE memory-mapped-I/O case — the motivating scenario for the
    /// diagnostic. See plan §6.1.
    #[cfg(windows)]
    #[test]
    fn is_file_in_use_error_matches_user_mapped_file_on_windows() {
        use crate::commands::entity_creation::is_file_in_use_error;
        let e = std::io::Error::from_raw_os_error(1224);
        assert!(is_file_in_use_error(&e), "os error 1224 must match");
    }

    /// §7.1 (Windows variant, negative): `is_file_in_use_error` does NOT match unrelated
    /// OS errors. Guards against accidental over-widening of the gate.
    #[cfg(windows)]
    #[test]
    fn is_file_in_use_error_rejects_unrelated_errors_on_windows() {
        use crate::commands::entity_creation::is_file_in_use_error;
        // ERROR_ACCESS_DENIED — separate failure mode, not file-in-use.
        let access_denied = std::io::Error::from_raw_os_error(5);
        assert!(
            !is_file_in_use_error(&access_denied),
            "os error 5 (ERROR_ACCESS_DENIED) must NOT match"
        );
        // ERROR_FILE_NOT_FOUND — not a file-in-use case.
        let not_found = std::io::Error::from_raw_os_error(2);
        assert!(
            !is_file_in_use_error(&not_found),
            "os error 2 (ERROR_FILE_NOT_FOUND) must NOT match"
        );
    }

    /// §7.1 (non-Windows variant): `is_file_in_use_error` always returns false off Windows.
    #[cfg(not(windows))]
    #[test]
    fn is_file_in_use_error_no_op_on_non_windows() {
        use crate::commands::entity_creation::is_file_in_use_error;
        let e = std::io::Error::from_raw_os_error(32);
        assert!(
            !is_file_in_use_error(&e),
            "non-Windows must always return false"
        );
    }

    /// §7.22 (covers G.4.7): no valid workgroup name can collide with the
    /// `BLOCKERS:` or `DIRTY_REPOS:` sentinel prefixes — `validate_existing_name`
    /// rejects both `:` and `_`. Locks the wire-protocol invariant.
    #[test]
    fn workgroup_names_cannot_collide_with_sentinels() {
        use crate::commands::entity_creation::validate_existing_name;
        // Both prefixes contain ':' (BLOCKERS:) or '_' and ':' (DIRTY_REPOS:),
        // neither of which is in the validator's alphanumeric+'-' whitelist.
        assert!(validate_existing_name("BLOCKERS:foo", "Workgroup").is_err());
        assert!(validate_existing_name("DIRTY_REPOS:foo", "Workgroup").is_err());
        // Bare-prefix-without-colon would be alphanumeric and pass the validator,
        // but bare prefixes aren't sentinel hits (frontend uses startsWith with the colon).
        assert!(validate_existing_name("BLOCKERS", "Workgroup").is_ok());
        assert!(validate_existing_name("DIRTY-REPOS", "Workgroup").is_ok());
    }

    /// #113 follow-up: `collect_files_to_probe` must include the WG root
    /// directory plus every `repo-*`, `__agent_*`, and `messaging/` top-level
    /// subdir. Surfacing dir handles is what lets RM detect terminal-cwd /
    /// IDE-workspace-open / file-watcher blockers, which file-only registration
    /// missed.
    #[cfg(windows)]
    #[test]
    fn collect_files_to_probe_includes_wg_root_and_top_level_dirs() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let wg_dir = tmp.path().join("wg-1-test");
        std::fs::create_dir(&wg_dir).expect("create wg_dir");
        // Top-level dirs the diagnostic should now register.
        let repo_dir = wg_dir.join("repo-foo");
        let agent_dir = wg_dir.join("__agent_dev-rust");
        let messaging_dir = wg_dir.join("messaging");
        std::fs::create_dir(&repo_dir).expect("create repo-foo");
        std::fs::create_dir(&agent_dir).expect("create __agent_dev-rust");
        std::fs::create_dir(&messaging_dir).expect("create messaging");
        // A non-relevant top-level dir must NOT be registered (filter discipline).
        let unrelated = wg_dir.join("docs");
        std::fs::create_dir(&unrelated).expect("create docs");
        // A regular file at WG root — should still appear in the result, just
        // after the dirs.
        std::fs::write(wg_dir.join("TASK.md"), "# t\n").expect("write TASK.md");

        let result = collect_files_to_probe(&wg_dir);

        assert!(
            result.iter().any(|p| p == &wg_dir),
            "result must include WG root, got {:?}",
            result
        );
        assert!(
            result.iter().any(|p| p == &repo_dir),
            "result must include repo-* subdir, got {:?}",
            result
        );
        assert!(
            result.iter().any(|p| p == &agent_dir),
            "result must include __agent_* subdir, got {:?}",
            result
        );
        assert!(
            result.iter().any(|p| p == &messaging_dir),
            "result must include messaging/ subdir, got {:?}",
            result
        );
        assert!(
            !result.iter().any(|p| p == &unrelated),
            "result must NOT include unrelated top-level dirs (got 'docs'); result={:?}",
            result
        );
    }

    /// #113 follow-up: the dir entries must come BEFORE files in the output
    /// so that, under the `MAX_FILES_TO_PROBE` cap, dirs are preferred. The
    /// plan calls this out explicitly: dir handles are the new signal, file
    /// handles were already covered.
    #[cfg(windows)]
    #[test]
    fn collect_files_to_probe_orders_dirs_before_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let wg_dir = tmp.path().join("wg-1-test");
        std::fs::create_dir(&wg_dir).expect("create wg_dir");
        let repo_dir = wg_dir.join("repo-foo");
        std::fs::create_dir(&repo_dir).expect("create repo-foo");
        std::fs::write(wg_dir.join("TASK.md"), "# t\n").expect("write TASK.md");
        std::fs::write(repo_dir.join("README.md"), "x").expect("write README.md");

        let result = collect_files_to_probe(&wg_dir);

        let first_file_idx = result.iter().position(|p| p.is_file());
        let last_dir_idx = result.iter().rposition(|p| p.is_dir());
        match (first_file_idx, last_dir_idx) {
            (Some(file_i), Some(dir_i)) => assert!(
                dir_i < file_i,
                "all dirs must precede all files in output; got dirs ending at {} but a file at {}; result={:?}",
                dir_i,
                file_i,
                result
            ),
            // If there are no files (empty WG) or no dirs (would be a bug),
            // the test is moot — but it shouldn't happen with the setup above.
            other => panic!(
                "expected at least one dir and one file in result, got {:?}; result={:?}",
                other, result
            ),
        }
    }

    #[cfg(windows)]
    #[test]
    fn merge_blocker_processes_combines_rm_files_and_cwd_by_pid() {
        let merged = merge_blocker_processes(
            vec![BlockerProcess {
                pid: 123,
                name: "git.exe".into(),
                cwd: None,
                files: vec![r"C:\wg\repo\.git\index".into()],
            }],
            vec![BlockerProcess {
                pid: 123,
                name: "powershell.exe".into(),
                cwd: Some(r"C:\wg\repo".into()),
                files: Vec::new(),
            }],
        );

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].pid, 123);
        assert_eq!(merged[0].name, "git.exe");
        assert_eq!(merged[0].cwd.as_deref(), Some(r"C:\wg\repo"));
        assert_eq!(merged[0].files, vec![r"C:\wg\repo\.git\index"]);
    }

    #[cfg(windows)]
    #[test]
    fn path_is_under_windows_is_case_insensitive_prefix_aware_and_boundary_safe() {
        assert!(path_is_under_windows(
            Path::new(r"C:\Users\Maria\WG\repo-foo"),
            Path::new(r"c:\users\maria\wg")
        ));
        assert!(path_is_under_windows(
            Path::new(r"\??\C:\Users\Maria\WG\repo-foo"),
            Path::new(r"C:\Users\Maria\WG")
        ));
        assert!(path_is_under_windows(
            Path::new(r"\??\UNC\server\share\WG\repo-foo"),
            Path::new(r"\\server\share\WG")
        ));
        assert!(!path_is_under_windows(
            Path::new(r"C:\Users\Maria\WG2"),
            Path::new(r"C:\Users\Maria\WG")
        ));
    }

    #[cfg(windows)]
    #[test]
    fn current_process_cwd_blocker_detects_self_cwd_under_wg() {
        let blocker = current_process_cwd_blocker_from_parts(
            Path::new(r"C:\Users\Maria\WG"),
            4242,
            Path::new(r"C:\Users\Maria\WG\repo-foo"),
            Some(Path::new(
                r"C:\Program Files\AgentsCommander\AgentsCommander.exe",
            )),
        )
        .expect("current process cwd should be reported when it is under the workgroup");

        assert_eq!(blocker.pid, 4242);
        assert_eq!(blocker.name, "AgentsCommander.exe");
        assert_eq!(blocker.cwd.as_deref(), Some(r"C:\Users\Maria\WG\repo-foo"));
        assert!(blocker.files.is_empty());

        assert!(
            current_process_cwd_blocker_from_parts(
                Path::new(r"C:\Users\Maria\WG"),
                4242,
                Path::new(r"C:\Users\Maria\outside"),
                None,
            )
            .is_none(),
            "current process outside the workgroup must not be reported"
        );
    }

    #[cfg(windows)]
    #[test]
    fn scan_cwd_processes_windows_detects_child_process_current_dir() {
        use std::process::{Child, Command, Stdio};
        use std::time::{Duration, Instant};

        struct ChildGuard(Child);
        impl Drop for ChildGuard {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        let tmp = tempfile::tempdir().expect("tempdir");
        let wg_dir = tmp.path().join("wg-1-test");
        let repo_dir = wg_dir.join("repo-foo");
        std::fs::create_dir_all(&repo_dir).expect("create repo");

        let mut cmd = Command::new("powershell.exe");
        crate::pty::credentials::scrub_credentials_from_std_command(&mut cmd);
        let child = cmd
            .args(["-NoProfile", "-Command", "Start-Sleep -Seconds 30"])
            .current_dir(&repo_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn powershell blocker");
        let child_pid = child.id();
        let _child = ChildGuard(child);

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut found = false;
        let mut last_error: Option<String> = None;
        while Instant::now() < deadline {
            match scan_cwd_processes_windows(&wg_dir) {
                Ok(blockers) => {
                    found = blockers.iter().any(|p| {
                        p.pid == child_pid
                            && p.cwd
                                .as_deref()
                                .is_some_and(|cwd| path_is_under_windows(Path::new(cwd), &repo_dir))
                    });
                    if found {
                        break;
                    }
                }
                Err(e) => {
                    last_error = Some(e);
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        assert!(
            found,
            "CWD fallback must detect child powershell.exe under WG; last_error={:?}",
            last_error
        );
    }
}
