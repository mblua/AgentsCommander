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

#[cfg(windows)]
mod cwd;
#[cfg(windows)]
mod restart_manager;

#[cfg(all(windows, test))]
use cwd::current_process_cwd_blocker_from_parts;
#[cfg(windows)]
use cwd::scan_cwd_processes_windows;
#[cfg(windows)]
use restart_manager::scan_restart_manager_processes_windows;

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
    /// Sample of paths inside the workgroup that this process holds.
    /// Capped at MAX_FILES_PER_PROCESS.
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

#[cfg(windows)]
const MAX_FILES_PER_PROCESS: usize = 5;
#[cfg(windows)]
const MAX_FILES_TO_PROBE: usize = 200;

#[cfg(any(windows, test))]
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

#[cfg(any(windows, test))]
fn win32_error(context: &str, code: u32) -> DiagnosticError {
    let meaning = win32_meaning(code);
    DiagnosticError {
        message: format!("{}: WIN32_ERROR={} ({})", context, code, meaning),
        code: Some(code),
        meaning: Some(meaning.to_string()),
    }
}

#[cfg(windows)]
fn diagnostic_error(message: impl Into<String>) -> DiagnosticError {
    DiagnosticError {
        message: message.into(),
        code: None,
        meaning: None,
    }
}

/// Top-level diagnostic. Always returns a `BlockerReport`; on non-Windows the body is empty
/// and `diagnostic_available = false`.
pub async fn diagnose_delete_root_blockers(
    root_dir: &Path,
    display_name: &str,
    raw_os_error: &str,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
) -> BlockerReport {
    log::info!(
        "[wg_delete_diagnostic] starting blocker scan for delete root '{}'",
        display_name
    );
    let canonical_root = canonicalize_for_compare(root_dir);

    let ac_scan = scan_ac_sessions(&canonical_root, session_mgr).await;

    // The Windows scan is fully synchronous (FFI + file-tree walk). We expect ~1 s
    // wall-time post-failure; running it on the current Tokio worker would block
    // that worker for the duration. Hand it off via `spawn_blocking` so other
    // async work (PTY reads, IPC events) keeps ticking. JoinError is treated as a
    // scan failure and falls through to the `diagnostic_available = false` path —
    // matches the FFI-error branch.
    #[cfg(windows)]
    let external_scan = {
        let root_for_scan = canonical_root.clone();
        match tokio::task::spawn_blocking(move || scan_external_processes_windows(&root_for_scan))
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
        let _ = canonical_root;
        ExternalProcessScan {
            processes: Vec::new(),
            restart_manager_available: false,
            restart_manager_error: None,
            cwd_scan_error: None,
        }
    };

    let report = build_blocker_report(
        display_name,
        detect_platform(),
        raw_os_error,
        ac_scan,
        external_scan,
    );

    log::info!(
        "[wg_delete_diagnostic] diagnostic done: live_sessions={}, \
        exited_session_records_ignored={}, external_processes={}, restart_manager_available={}, \
        restart_manager_error={}, raw_delete_error={}",
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

pub async fn diagnose_blockers(
    wg_dir: &Path,
    workgroup_name: &str,
    raw_os_error: &str,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
) -> BlockerReport {
    diagnose_delete_root_blockers(wg_dir, workgroup_name, raw_os_error, session_mgr).await
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
    let canon =
        crate::filesystem::projects::canonical_project_path(p).unwrap_or_else(|_| p.to_path_buf());
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
    crate::filesystem::room_diagnostics::collect_room_lock_resources(wg_dir, MAX_FILES_TO_PROBE)
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
                "[wg_delete_diagnostic] CWD fallback scan failed; preserving Restart Manager \
                result: {}",
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
                "[wg_delete_diagnostic] Restart Manager scan failed; preserving CWD fallback \
                result: {}",
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

#[cfg(test)]
mod tests;
