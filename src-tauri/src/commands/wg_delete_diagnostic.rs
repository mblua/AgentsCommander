//! Diagnostic for `delete_workgroup` failures caused by file-in-use (Windows os error 32).
//!
//! Three scans:
//!   1. AC-internal sessions whose `working_directory` lives inside the workgroup tree.
//!   2. External processes holding handles on files inside the workgroup tree, via the
//!      Windows Restart Manager API (RmStartSession / RmRegisterResources / RmGetList).
//!   3. External Windows processes whose current working directory is under the
//!      workgroup tree, via a direct PEB ProcessParameters read.
//!
//! Pure helpers - no Tauri commands. Invoked from `entity_creation::delete_workgroup`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;

use crate::session::manager::SessionManager;

/// Locks the producer to the four-variant promise so the wire shape can't drift.
/// Lowercase JSON matches the TS literal union in `src/shared/types.ts`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Windows,
    Linux,
    Macos,
    Other,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockerReport {
    pub workgroup: String,
    pub platform: Platform,
    pub diagnostic_available: bool,
    pub raw_os_error: String,
    pub sessions: Vec<BlockerSession>,
    pub processes: Vec<BlockerProcess>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockerSession {
    pub session_id: String,
    /// FQN like `agentscommander:wg-7-dev-team/architect` derived via
    /// `crate::config::teams::agent_fqn_from_path`.
    pub agent_name: String,
    pub cwd: String,
}

#[derive(Debug, Clone, Serialize)]
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

const MAX_FILES_PER_PROCESS: usize = 5;
#[cfg(windows)]
const MAX_FILES_TO_PROBE: usize = 200;

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

    let sessions = scan_ac_sessions(&canonical_wg, session_mgr).await;

    // The Windows scan is fully synchronous (FFI + file-tree walk). We expect ~1 s
    // wall-time post-failure; running it on the current Tokio worker would block
    // that worker for the duration. Hand it off via `spawn_blocking` so other
    // async work (PTY reads, IPC events) keeps ticking. JoinError is treated as a
    // scan failure and falls through to the `diagnostic_available = false` path -
    // matches the FFI-error branch.
    #[cfg(windows)]
    let (processes, diagnostic_available) = {
        let wg_for_scan = canonical_wg.clone();
        match tokio::task::spawn_blocking(move || scan_external_processes_windows(&wg_for_scan))
            .await
        {
            Ok(Ok(p)) => (p, true),
            Ok(Err(e)) => {
                log::warn!("[wg_delete_diagnostic] external process scan failed: {}", e);
                (Vec::new(), false)
            }
            Err(join_err) => {
                log::warn!(
                    "[wg_delete_diagnostic] external process scan join failed: {}",
                    join_err
                );
                (Vec::new(), false)
            }
        }
    };

    #[cfg(not(windows))]
    let (processes, diagnostic_available) = {
        let _ = canonical_wg;
        (Vec::<BlockerProcess>::new(), false)
    };

    let report = BlockerReport {
        workgroup: workgroup_name.to_string(),
        platform: detect_platform(),
        diagnostic_available,
        raw_os_error: raw_os_error.to_string(),
        sessions,
        processes,
    };

    log::info!(
        "[wg_delete_diagnostic] diagnostic done: {} sessions, {} processes, available={}",
        report.sessions.len(),
        report.processes.len(),
        report.diagnostic_available
    );

    report
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
///   1. **Directories** - WG root, top-level `repo-*` and `__agent_*` subdirs,
///      and `messaging/` if present. Surfaces dir-handle holders (terminal
///      cwd, IDE workspace open, file watchers via `ReadDirectoryChangesW`)
///      that file-only registration misses (#113 follow-up).
///   2. **Hot lock files** - lock-prone git metadata (`.lock`, `index`,
///      `HEAD`, etc.) so the budget can't be exhausted on a single
///      `.git/objects/` subtree.
///   3. **Cold files** - everything else, until the cap is hit.
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
    /// `repo-*` (clones - git operations, IDE workspaces),
    /// `__agent_*` (replicas - agent-spawned shells holding cwd),
    /// `messaging/` (mailbox - file watchers).
    fn is_relevant_top_level_dir(name: &str) -> bool {
        name.starts_with("repo-") || name.starts_with("__agent_") || name == "messaging"
    }

    /// Soft ceiling on total walk size - once we've inventoried 4× the probe
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

    // Dirs first (the new signal - small set, ~5–10 entries; prefer over cold
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
) -> Vec<BlockerSession> {
    let mgr = session_mgr.read().await;
    let sessions = mgr.list_sessions().await;
    drop(mgr);

    sessions
        .into_iter()
        .filter_map(|s| {
            let cwd_canon = canonicalize_for_compare(Path::new(&s.working_directory));
            if !cwd_canon.starts_with(canonical_wg) {
                return None;
            }
            let agent_name = crate::config::teams::agent_fqn_from_path(&s.working_directory);
            Some(BlockerSession {
                session_id: s.id,
                agent_name,
                cwd: s.working_directory,
            })
        })
        .collect()
}

#[cfg(windows)]
fn scan_external_processes_windows(wg_dir: &Path) -> Result<Vec<BlockerProcess>, String> {
    let rm = scan_restart_manager_processes_windows(wg_dir);
    let cwd = scan_cwd_processes_windows(wg_dir);

    match (rm, cwd) {
        (Ok(rm_processes), Ok(cwd_processes)) => {
            Ok(merge_blocker_processes(rm_processes, cwd_processes))
        }
        (Ok(rm_processes), Err(cwd_err)) => {
            log::warn!(
                "[wg_delete_diagnostic] CWD fallback scan failed; preserving Restart Manager result: {}",
                cwd_err
            );
            Ok(rm_processes)
        }
        (Err(rm_err), Ok(cwd_processes)) => {
            log::warn!(
                "[wg_delete_diagnostic] Restart Manager scan failed; preserving CWD fallback result: {}",
                rm_err
            );
            Ok(cwd_processes)
        }
        (Err(rm_err), Err(cwd_err)) => Err(format!(
            "Restart Manager scan failed: {}; CWD fallback scan failed: {}",
            rm_err, cwd_err
        )),
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
/// 1. **Bulk pass** - open RM sessions for batches of probed files (binary-search
///    fallback when RmRegisterResources rejects a batch), call `RmGetList` to learn
///    which PIDs hold any handles. Tolerant of single bad files; ~O(log N + bad_files)
///    sessions.
/// 2. **Per-file attribution pass** - RM doesn't tell us *which* file each PID
///    held. Iterate files; for each, open a fresh session, register only that
///    file, and accumulate (file → matching PID) entries. Cap each PID at
///    `MAX_FILES_PER_PROCESS` (5) and short-circuit once every blocker PID is
///    saturated.
#[cfg(windows)]
fn scan_restart_manager_processes_windows(wg_dir: &Path) -> Result<Vec<BlockerProcess>, String> {
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

    /// RAII guard for an RM session - guarantees `RmEndSession` runs even on early
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

    fn rm_start() -> Result<RmSession, String> {
        let mut handle: u32 = 0;
        // `strSessionKey` is PWSTR (mutable). Must hold CCH_RM_SESSION_KEY+1 wide chars.
        let mut key: Vec<u16> = vec![0u16; (CCH_RM_SESSION_KEY as usize) + 1];
        let rc = unsafe { RmStartSession(&mut handle, 0, key.as_mut_ptr()) };
        if rc != ERROR_SUCCESS {
            return Err(format!("RmStartSession failed: WIN32_ERROR={}", rc));
        }
        Ok(RmSession(handle))
    }

    fn rm_register(handle: u32, wide_files: &[Vec<u16>]) -> Result<(), String> {
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
            return Err(format!("RmRegisterResources failed: WIN32_ERROR={}", rc));
        }
        Ok(())
    }

    fn rm_get_list(handle: u32) -> Result<Vec<RM_PROCESS_INFO>, String> {
        let mut needed: u32 = 0;
        let mut have: u32 = 0;
        let mut reasons: u32 = 0;

        // Probe: zero buffer, learn `needed`. RM returns SUCCESS with needed=0 when
        // there are no blockers - distinct from MORE_DATA.
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
            return Err(format!("RmGetList probe failed: WIN32_ERROR={}", rc));
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
            return Err(format!("RmGetList read failed: WIN32_ERROR={}", rc));
        }
        Err("RmGetList: ERROR_MORE_DATA persisted past retry budget".into())
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

    /// FILETIME equality - tolerates PID recycling between bulk and per-file passes.
    fn same_start(
        a: &windows_sys::Win32::Foundation::FILETIME,
        b: &windows_sys::Win32::Foundation::FILETIME,
    ) -> bool {
        a.dwLowDateTime == b.dwLowDateTime && a.dwHighDateTime == b.dwHighDateTime
    }

    /// Binary-search-fallback bulk register: try the whole batch in one session;
    /// on RmRegisterResources error, drop the session, split the batch in half,
    /// and recurse. Single-bad-file isolation at `wide.len() == 1`.
    /// Bound: O(log N + bad_files) RM sessions.
    fn collect_blockers_tolerant(
        wide: &[Vec<u16>],
        original_paths: &[&PathBuf],
    ) -> Vec<RM_PROCESS_INFO> {
        if wide.is_empty() {
            return Vec::new();
        }
        let session = match rm_start() {
            Ok(s) => s,
            Err(e) => {
                log::warn!(
                    "[wg_delete_diagnostic] bulk-pass RmStartSession failed (giving up on this sub-batch of {}): {}",
                    wide.len(),
                    e
                );
                return Vec::new();
            }
        };
        match rm_register(session.0, wide) {
            Ok(()) => rm_get_list(session.0).unwrap_or_else(|e| {
                log::warn!(
                    "[wg_delete_diagnostic] bulk-pass RmGetList failed for sub-batch of {}: {}",
                    wide.len(),
                    e
                );
                Vec::new()
            }),
            Err(e) if wide.len() == 1 => {
                log::warn!(
                    "[wg_delete_diagnostic] skipping unregisterable file '{}': {}",
                    original_paths[0].display(),
                    e
                );
                Vec::new()
            }
            Err(_) => {
                drop(session); // release before recursing
                let mid = wide.len() / 2;
                let mut left = collect_blockers_tolerant(&wide[..mid], &original_paths[..mid]);
                let right = collect_blockers_tolerant(&wide[mid..], &original_paths[mid..]);
                left.extend(right);
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

    // ── Phase 1: bulk pass - get the set of blocker PIDs ─────────────────────
    let bulk_list: Vec<RM_PROCESS_INFO> = collect_blockers_tolerant(&wide_files, &alive);

    if bulk_list.is_empty() {
        return Ok(Vec::new());
    }

    // Seed the result map with one entry per unique PID. Resolve the executable
    // name now (prefer `strAppName`, fall back to `QueryFullProcessImageNameW`).
    let mut by_pid: HashMap<u32, BlockerProcess> = HashMap::new();
    for info in &bulk_list {
        let pid = info.Process.dwProcessId;
        if pid == 0 {
            // System Idle / kernel - never actionable, never a real blocker.
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
            break; // every blocker has its quota - further probing wastes RM sessions
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
                    e
                );
                continue;
            }
        };
        if rm_register(session.0, single).is_err() {
            continue; // file vanished mid-scan or RM rejected it; non-fatal
        }
        let list = match rm_get_list(session.0) {
            Ok(l) => l,
            Err(_) => continue,
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

        assert_eq!(blockers.len(), 1, "exactly one session inside wg_dir");
        assert!(
            blockers[0].cwd.contains("__agent_inside"),
            "filtered session must be the inside one, got cwd={}",
            blockers[0].cwd
        );
    }

    /// §7.4 (with R.1.a fix): `BlockerReport` JSON shape preserves camelCase
    /// at the wire boundary AND the `Platform` enum serializes lowercase.
    #[test]
    fn blocker_report_serializes_with_camelcase_fields() {
        let report = BlockerReport {
            workgroup: "wg-7-dev-team".into(),
            platform: Platform::Windows,
            diagnostic_available: true,
            raw_os_error: "...".into(),
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
            "rawOsError",
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
        let s = &json["sessions"][0];
        for k in &["sessionId", "agentName", "cwd"] {
            assert!(s.get(*k).is_some(), "missing session field: {}", k);
        }
        // BlockerProcess fields
        let p = &json["processes"][0];
        for k in &["pid", "name", "cwd", "files"] {
            assert!(p.get(*k).is_some(), "missing process field: {}", k);
        }
        // Snake-case must NOT leak at the wire boundary.
        for k in &[
            "diagnostic_available",
            "raw_os_error",
            "session_id",
            "agent_name",
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
    /// This is the VSCode / IDE memory-mapped-I/O case - the motivating scenario for the
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
        // ERROR_ACCESS_DENIED - separate failure mode, not file-in-use.
        let access_denied = std::io::Error::from_raw_os_error(5);
        assert!(
            !is_file_in_use_error(&access_denied),
            "os error 5 (ERROR_ACCESS_DENIED) must NOT match"
        );
        // ERROR_FILE_NOT_FOUND - not a file-in-use case.
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
    /// `BLOCKERS:` or `DIRTY_REPOS:` sentinel prefixes - `validate_existing_name`
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
        // A regular file at WG root - should still appear in the result, just
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
            // the test is moot - but it shouldn't happen with the setup above.
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
