use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use uuid::Uuid;

use crate::config::settings::WindowGeometry;
use crate::session::manager::SessionManager;
use crate::session::profile::CodingAgentKind;
use crate::session::session::{
    SessionCommunication, SessionCommunicationKind, SessionInfo, SessionStatus, TEMP_SESSION_PREFIX,
};

/// #291 — in-process mutex serializing all `save_sessions` calls.
///
/// The historical race: two concurrent callers both wrote to the shared
/// `sessions.json.tmp`, then both tried to `rename` it. The first rename
/// consumed the temp file, the second rename returned Windows
/// `ERROR_FILE_NOT_FOUND` (os error 2), and the second caller's snapshot
/// silently failed to persist.
///
/// The fix is twofold:
///   1. This mutex serializes the write+rename window so the order of saves
///      matches the order of `lock()` acquisition. "Last writer wins."
///   2. Per-call unique temp filenames (`sessions.json.<pid>.<op_id>.tmp`)
///      provide defense-in-depth: even if a future caller forgets the lock,
///      two callers can never collide on the same temp filename.
///
/// Lock window: held only across the synchronous serialize + write + rename
/// inside `save_sessions_to_dir` (no await points). Worst-case contention
/// per waiter is bounded by `RENAME_ATTEMPTS * max(RENAME_BACKOFFS_MS)`
/// (~260 ms), same as the existing rename window.
///
/// Poisoning is tolerated via `into_inner()`: a poisoned lock means a prior
/// holder panicked mid-write, but the persisted file is atomic (tmp+rename),
/// so picking up the lock cleanly is safe.
static SAVE_SESSIONS_LOCK: Mutex<()> = Mutex::new(());

/// #291 — counter feeding the per-call unique temp filename for
/// `save_sessions`. Combined with the PID it makes the temp filename
/// `sessions.json.<pid>.<op_id>.tmp` distinct from any concurrent in-process
/// or cross-process save, and from any leftover temp file written by a
/// prior crashed run. Kept separate from `rename_with_retry`'s `OP_ID` so
/// the two counters can be reasoned about independently in diagnostics.
static SAVE_OP_ID: AtomicU64 = AtomicU64::new(0);

/// #280 §3.1 — diagnostic context captured when an atomic rename exhausts
/// its retry budget. Surfaced in the caller's error string so a single
/// ERROR line carries enough state to investigate the AV / Indexer / second
/// instance contention pattern.
#[derive(Debug)]
pub(crate) struct RenameDiagnostics {
    op_id: u64,
    pid: u32,
    instance_id: String,
    tmp_exists_before: bool,
    final_exists_before: bool,
    attempts: u32,
    last_os_error: Option<i32>,
    duration: std::time::Duration,
}

/// #280 §3.1 — number of `std::fs::rename` attempts. With the
/// `BACKOFFS_MS = [10, 50, 200]` schedule below, this gives a worst-case
/// 260 ms blocking window before surfacing the error. G-MED-2 fix:
/// 4 attempts so all three backoff entries are actually used (with
/// `ATTEMPTS = 3` the 200 ms entry would be dead code).
const RENAME_ATTEMPTS: u32 = 4;

/// #280 §3.1 — backoff schedule between rename attempts in milliseconds.
/// Tuned for Windows AV / Indexer holds which typically clear within
/// ~50 ms but occasionally take >100 ms on cold caches. The terminal
/// attempt has no backoff (we already failed `RENAME_ATTEMPTS - 1` times).
const RENAME_BACKOFFS_MS: [u64; 3] = [10, 50, 200];

/// Atomic rename with bounded retries. Returns the diagnostic context so
/// the caller can fold it into a single ERROR line. Successful retries are
/// logged at INFO (low-frequency, useful signal that the race is
/// happening). Intermediate failures log at DEBUG.
///
/// This retry loop targets *cross-process* contention on the destination
/// (`sessions.json`): AV scanners, the Windows Indexer, or a second AC
/// instance briefly holding the file. It is NOT the mitigation for #291,
/// which was an in-process race on a shared *temp* filename — that one is
/// solved upstream in `save_sessions_to_dir` by a per-call unique temp
/// name plus `SAVE_SESSIONS_LOCK`, so by the time we get here the source
/// path is guaranteed unique to this save.
///
/// Risk note (deliberately accepted for #280 scope): this uses
/// `std::thread::sleep`, which blocks the calling tokio worker thread for
/// up to `sum(RENAME_BACKOFFS_MS)` = 260 ms during a contended rename.
/// `save_sessions` is invoked from async Tauri command handlers, but the
/// surrounding code (`std::fs::write`, `std::fs::rename`) is already sync,
/// so this does not introduce a new class of blocking — only enlarges an
/// existing one. The clean fix (`tokio::task::spawn_blocking` for the
/// whole persistence block) is a wider signature/caller refactor and is
/// out of scope for #280 (observability hardening, not concurrency
/// rework). File a follow-up if the 260 ms tail latency becomes
/// user-perceptible.
pub(crate) fn rename_with_retry(tmp: &Path, dst: &Path) -> Result<(), (String, RenameDiagnostics)> {
    static OP_ID: AtomicU64 = AtomicU64::new(0);
    let op_id = OP_ID.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let instance_id = crate::config::agent_local_dir_name();
    let start = std::time::Instant::now();
    let tmp_exists_before = tmp.exists();
    let final_exists_before = dst.exists();

    let mut last_err: Option<std::io::Error> = None;
    for attempt in 0..RENAME_ATTEMPTS {
        match std::fs::rename(tmp, dst) {
            Ok(()) => {
                if attempt > 0 {
                    log::info!(
                        "[sessions] rename succeeded after retry — op_id={} pid={} instance={} attempt={}/{} duration={:?}",
                        op_id,
                        pid,
                        instance_id,
                        attempt + 1,
                        RENAME_ATTEMPTS,
                        start.elapsed()
                    );
                }
                return Ok(());
            }
            Err(e) => {
                log::debug!(
                    "[sessions] rename attempt {}/{} failed — op_id={} pid={} os_error={:?} kind={:?}",
                    attempt + 1,
                    RENAME_ATTEMPTS,
                    op_id,
                    pid,
                    e.raw_os_error(),
                    e.kind()
                );
                last_err = Some(e);
                let backoff_idx = attempt as usize;
                if backoff_idx < RENAME_BACKOFFS_MS.len() {
                    std::thread::sleep(std::time::Duration::from_millis(
                        RENAME_BACKOFFS_MS[backoff_idx],
                    ));
                }
            }
        }
    }

    let e = last_err.expect("RENAME_ATTEMPTS >= 1, so the loop runs at least once");
    let diag = RenameDiagnostics {
        op_id,
        pid,
        instance_id,
        tmp_exists_before,
        final_exists_before,
        attempts: RENAME_ATTEMPTS,
        last_os_error: e.raw_os_error(),
        duration: start.elapsed(),
    };
    Err((e.to_string(), diag))
}

/// Minimal session data needed to restore a session on next app start.
/// No UUID, just the "recipe" to re-create it.
///
/// The optional runtime fields (id, waiting_for_input, communication,
/// created_at) are populated during live snapshots so the CLI can read session
/// state from the file without requiring an HTTP request. They are ignored on
/// restore.
///
/// `status` is also populated during live snapshots for CLI consumption AND is
/// now **consumed on restore** by the issue #248 startup wake policy: the
/// restore-task closure in `lib.rs` reads `status` to decide whether a
/// coordinator should be auto-woken (was awake at shutdown) or left dormant
/// (was asleep at shutdown). See `should_wake_on_restore` in `lib.rs`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedSession {
    pub name: String,
    pub shell: String,
    pub shell_args: Vec<String>,
    pub working_directory: String,
    /// True for the session that was active when the app closed
    #[serde(default)]
    pub was_active: bool,
    /// Authoritative repo list. Empty = no repo badge rendered.
    #[serde(default)]
    pub git_repos: Vec<crate::session::session::SessionRepo>,
    /// Recomputed on restore; persisted for forward-compat only.
    #[serde(default)]
    pub is_coordinator: bool,
    /// True for the global Root Agent session. Defaults false for old sessions.json.
    #[serde(default)]
    pub is_root_agent: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_profile: Option<String>,
    /// Telegram bot id that was ON for this session at the last successful
    /// bridge attach. None means Telegram was OFF.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telegram_bot_id: Option<String>,

    /// True if the session was detached into its own window at snapshot time.
    /// Phase 3 restore re-spawns a detached window for every persisted row with
    /// `was_detached=true` (except deferred sessions — see plan §R.9). Sourced
    /// from `Session::was_detached` under Fix A — NOT from `DetachedSessionsState`.
    #[serde(default)]
    pub was_detached: bool,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_prompt: Option<String>,

    /// Last-known geometry of this session's detached window. `None` for sessions
    /// that were never detached, or detached without any drag/resize yet. Auto-GC'd
    /// when the session is destroyed (field travels with the PersistedSession row).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detached_geometry: Option<WindowGeometry>,

    /// (#630/#631) Durable resume intent. `true` => start fresh on restore
    /// ("Restart Session"); `false` => resume. `#[serde(default)]` so pre-existing
    /// records deserialize to `false` = resume (polarity is deliberate: the safe
    /// serde/Default value must mean "resume"). No `skip_serializing_if`: always
    /// written so the on-disk record is explicit after the first save.
    #[serde(default)]
    pub start_fresh_on_restore: bool,

    // ── Legacy fields — read-only, consumed by the upgrade pass in load_sessions. ──
    // `skip_serializing_if = "Option::is_none"` means snapshot_sessions never writes them
    // back, and the first save after upgrade retires them from disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_branch_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_branch_prefix: Option<String>,

    // ── Runtime fields (populated during live snapshots; `status` consumed on
    //    restore per issue #248 and `communication` consumed on restore per
    //    issue #747, the others ignored on restore) ──
    /// Session UUID (only present in live snapshots)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Current session status. Populated during live snapshots; **consumed on
    /// restore** by the issue #248 startup wake policy (see
    /// `should_wake_on_restore` in `lib.rs`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<SessionStatus>,
    /// Whether the session is waiting for user input (only present in live snapshots)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_for_input: Option<bool>,
    /// Current visible session communication state. Populated during live
    /// snapshots; **consumed on restore** since issue #747 (a persisted raised
    /// hand re-applies to the restored record), and preserved on
    /// failed-recoverable rows by `sanitize_failed_recoverable` so the
    /// next-startup retry can restore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub communication: Option<SessionCommunication>,
    /// ISO 8601 creation timestamp (only present in live snapshots)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
}

fn sessions_path() -> Option<PathBuf> {
    super::config_dir().map(|d| d.join("sessions.json"))
}

fn strip_long_prefix_str(s: &str) -> String {
    crate::path_utils::normalize_windows_verbatim_path(s)
}

#[cfg(test)]
static FILTER_PROJECT_PATHS_THREAD_IDS: std::sync::OnceLock<
    std::sync::Mutex<Vec<std::thread::ThreadId>>,
> = std::sync::OnceLock::new();

#[cfg(test)]
thread_local! {
    static NORMALIZE_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn reset_normalize_call_count() {
    NORMALIZE_CALLS.with(|calls| calls.set(0));
}

#[cfg(test)]
pub(crate) fn normalize_call_count() -> usize {
    NORMALIZE_CALLS.with(|calls| calls.get())
}

fn normalize_for_project_compare(path: &Path) -> String {
    #[cfg(test)]
    NORMALIZE_CALLS.with(|calls| calls.set(calls.get() + 1));
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut s = strip_long_prefix_str(&path.to_string_lossy()).replace('\\', "/");
    while s.ends_with('/') && s.len() > 1 {
        s.pop();
    }
    if cfg!(windows) {
        s.make_ascii_lowercase();
    }
    s
}

fn path_is_under_or_equal(candidate: &str, root: &str) -> bool {
    if root.is_empty() {
        return false;
    }
    if candidate == root {
        return true;
    }
    if root == "/" {
        return candidate.starts_with('/');
    }
    candidate.starts_with(&format!("{}/", root))
}

/// Raw-root convenience for active project lists where each candidate session
/// is checked against a short per-command scope, such as `archive_blockers`.
///
/// Do not call this with `archived_project_paths`,
/// `session_retention_project_paths`, or inside a per-dir loop. In those paths,
/// normalize roots once with `normalize_project_roots` and use the normalized
/// helpers.
pub(crate) fn working_directory_under_any_project_path(
    working_directory: &str,
    project_paths: &[String],
) -> bool {
    let cwd = normalize_for_project_compare(Path::new(working_directory));
    let roots = normalize_project_roots(project_paths);
    working_directory_under_any_normalized_root(&cwd, &roots)
}

/// #881: canonicalize a project-root list once. Blocking
/// (`std::fs::canonicalize` per root). Hoist the call out of per-session or
/// per-dir loops, and off the async runtime when the caller is on a poll
/// interval. Empty input yields empty output with zero syscalls.
pub(crate) fn normalize_project_roots(project_paths: &[String]) -> Vec<String> {
    project_paths
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .map(|p| normalize_for_project_compare(Path::new(p)))
        .collect()
}

fn working_directory_under_any_normalized_root(cwd: &str, roots: &[String]) -> bool {
    roots
        .iter()
        .any(|project| path_is_under_or_equal(cwd, project))
}

/// #881: paths that may retain persisted sessions. Active projects plus archived
/// projects are both registered from the session store's point of view; only
/// active projects should be used for discovery and background project work.
pub(crate) fn session_retention_project_paths(
    settings: &crate::config::settings::AppSettings,
) -> Vec<String> {
    let mut paths = settings.project_paths.clone();
    paths.extend(settings.archived_project_paths.iter().cloned());
    paths
}

/// #881: true when `path` lives under one of `normalized_archived_roots`,
/// which the caller produced with `normalize_project_roots` once per poll tick,
/// restore loop, or resolution.
///
/// The empty-list fast path lives here, before `path` itself is canonicalized,
/// so the common case of no archived projects pays zero syscalls.
///
/// Do not add a raw-root convenience wrapper. The per-root canonicalize must
/// not hide inside a per-dir loop.
pub(crate) fn is_under_normalized_archived_roots(
    path: &str,
    normalized_archived_roots: &[String],
) -> bool {
    if normalized_archived_roots.is_empty() {
        return false;
    }
    working_directory_under_any_normalized_root(
        &normalize_for_project_compare(Path::new(path)),
        normalized_archived_roots,
    )
}

/// #881: return the stored raw root containing `path`, while matching on the
/// same normalized form used by `normalize_project_roots`.
pub(crate) fn raw_project_path_containing(path: &str, project_paths: &[String]) -> Option<String> {
    if project_paths.is_empty() {
        return None;
    }
    let cwd = normalize_for_project_compare(Path::new(path));
    project_paths.iter().find_map(|raw| {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        let root = normalize_for_project_compare(Path::new(trimmed));
        if path_is_under_or_equal(&cwd, &root) {
            Some(raw.clone())
        } else {
            None
        }
    })
}

fn is_root_persisted_session(session: &PersistedSession) -> bool {
    session.is_root_agent
        || crate::config::root_agent::is_root_agent_dir_name(&session.working_directory)
}

fn filter_sessions_for_project_paths(
    sessions: Vec<PersistedSession>,
    project_paths: &[String],
) -> Vec<PersistedSession> {
    let roots = normalize_project_roots(project_paths);
    filter_sessions_for_normalized_roots(sessions, &roots)
}

fn filter_sessions_for_normalized_roots(
    sessions: Vec<PersistedSession>,
    roots: &[String],
) -> Vec<PersistedSession> {
    #[cfg(test)]
    if sessions
        .iter()
        .any(|session| session.name == "__panic_filter_for_test__")
    {
        panic!("test-only session filter panic");
    }

    let total = sessions.len();
    let filtered: Vec<PersistedSession> = sessions
        .into_iter()
        .filter(|session| {
            if is_root_persisted_session(session) {
                return true;
            }
            let cwd = normalize_for_project_compare(Path::new(&session.working_directory));
            let keep = working_directory_under_any_normalized_root(&cwd, roots);
            if !keep {
                log::warn!(
                    "[sessions] Dropping orphan persisted session '{}' at '{}' (outside current projectPaths)",
                    session.name,
                    session.working_directory
                );
            }
            keep
        })
        .collect();

    if filtered.len() < total {
        log::info!(
            "[sessions] Purged {} orphan persisted session(s) outside current projectPaths",
            total - filtered.len()
        );
    }

    filtered
}

#[cfg(test)]
fn record_filter_project_paths_thread_id() {
    let slot = FILTER_PROJECT_PATHS_THREAD_IDS.get_or_init(|| std::sync::Mutex::new(Vec::new()));
    slot.lock()
        .expect("thread id mutex poisoned")
        .push(std::thread::current().id());
}

async fn filter_sessions_for_project_paths_blocking(
    sessions: Vec<PersistedSession>,
    project_paths: &[String],
) -> Result<Vec<PersistedSession>, String> {
    let project_paths = project_paths.to_vec();
    tokio::task::spawn_blocking(move || {
        #[cfg(test)]
        record_filter_project_paths_thread_id();
        filter_sessions_for_project_paths(sessions, &project_paths)
    })
    .await
    .map_err(|e| format!("session filter task failed: {}", e))
}

/// Remove duplicate sessions by name AND working_directory.
/// When duplicates share the same key (name or CWD), keep the one with
/// `was_active=true`; if none (or both) are active, keep the last occurrence.
/// Note: callers are expected to filter out temp sessions before calling this.
fn deduplicate(sessions: Vec<PersistedSession>) -> Vec<PersistedSession> {
    let total = sessions.len();
    let mut name_index: HashMap<String, usize> = HashMap::new();
    let mut cwd_index: HashMap<String, usize> = HashMap::new();
    let mut root_index: Option<usize> = None;
    let mut result: Vec<PersistedSession> = Vec::with_capacity(total);

    for session in sessions {
        let norm_cwd = normalized_cwd_key(&session.working_directory);
        let is_root_agent = session.is_root_agent
            || crate::config::root_agent::is_root_agent_path(&session.working_directory);

        if is_root_agent {
            if let Some(idx) = root_index {
                log::warn!(
                    "[sessions] Dropping duplicate root agent session '{}' at '{}'",
                    session.name,
                    session.working_directory
                );
                if !result[idx].was_active || session.was_active {
                    let old_cwd = normalized_cwd_key(&result[idx].working_directory);
                    name_index.remove(&result[idx].name);
                    cwd_index.remove(&old_cwd);
                    name_index.insert(session.name.clone(), idx);
                    cwd_index.insert(norm_cwd, idx);
                    result[idx] = session;
                    result[idx].is_root_agent = true;
                } else {
                    result[idx].is_root_agent = true;
                }
                continue;
            }
        }

        // Check name-based duplicate
        if let Some(&idx) = name_index.get(&session.name) {
            log::warn!(
                "[sessions] Dropping duplicate session by name '{}'",
                session.name
            );
            if !result[idx].was_active || session.was_active {
                // Patch cwd_index if the CWD changed
                let old_cwd = normalized_cwd_key(&result[idx].working_directory);
                if old_cwd != norm_cwd {
                    cwd_index.remove(&old_cwd);
                    cwd_index.insert(norm_cwd, idx);
                }
                result[idx] = session;
            }
            continue;
        }

        // Check CWD-based duplicate
        if let Some(&idx) = cwd_index.get(&norm_cwd) {
            log::warn!(
                "[sessions] Dropping duplicate session by CWD '{}' (existing='{}', incoming='{}')",
                session.working_directory,
                result[idx].name,
                session.name
            );
            if !result[idx].was_active || session.was_active {
                name_index.remove(&result[idx].name);
                name_index.insert(session.name.clone(), idx);
                result[idx] = session;
            }
            continue;
        }

        // New unique session
        name_index.insert(session.name.clone(), result.len());
        cwd_index.insert(norm_cwd, result.len());
        if is_root_agent {
            root_index = Some(result.len());
        }
        result.push(session);
        if is_root_agent {
            if let Some(last) = result.last_mut() {
                last.is_root_agent = true;
            }
        }
    }

    if result.len() < total {
        log::info!(
            "[sessions] Deduplicated: {} → {} sessions",
            total,
            result.len()
        );
    }

    result
}

fn normalized_cwd_key(cwd: &str) -> String {
    crate::path_utils::normalize_windows_verbatim_path(cwd)
        .replace('\\', "/")
        .to_lowercase()
}

fn normalize_persisted_session_paths(session: &mut PersistedSession) {
    session.working_directory =
        crate::path_utils::normalize_windows_verbatim_path(&session.working_directory);
    for repo in &mut session.git_repos {
        repo.source_path = crate::path_utils::normalize_windows_verbatim_path(&repo.source_path);
    }
}

/// Load sessions from disk without deduplication or temp-session filtering.
/// Used by the CLI to read the live snapshot as-is.
pub fn load_sessions_raw() -> Vec<PersistedSession> {
    let path = match sessions_path() {
        Some(p) => p,
        None => return vec![],
    };
    load_sessions_raw_from_path(&path)
}

#[cfg(debug_assertions)]
pub fn load_sessions_raw_from_dir_for_test(dir: &Path) -> Vec<PersistedSession> {
    load_sessions_raw_from_path(&dir.join("sessions.json"))
}

fn load_sessions_raw_from_path(path: &Path) -> Vec<PersistedSession> {
    if !path.exists() {
        return vec![];
    }
    match std::fs::read_to_string(path) {
        Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
        Err(_) => vec![],
    }
}

/// Load persisted sessions from the app config directory (see config_dir()).
/// Returns empty vec on any error (missing file, corrupt JSON, etc.)
pub fn load_sessions() -> Vec<PersistedSession> {
    let path = match sessions_path() {
        Some(p) => p,
        None => {
            log::warn!("Could not determine home directory for session restore");
            return vec![];
        }
    };

    load_sessions_from_path(&path)
}

fn load_sessions_from_dir(dir: &Path) -> Vec<PersistedSession> {
    load_sessions_from_path(&dir.join("sessions.json"))
}

fn load_sessions_from_path(path: &Path) -> Vec<PersistedSession> {
    if !path.exists() {
        return vec![];
    }

    match std::fs::read_to_string(path) {
        Ok(contents) => match serde_json::from_str::<Vec<PersistedSession>>(&contents) {
            Ok(sessions) => {
                // Safety net: filter out [temp] sessions that should never survive a restart
                let temp_count = sessions
                    .iter()
                    .filter(|s| s.name.starts_with(TEMP_SESSION_PREFIX))
                    .count();
                let filtered: Vec<PersistedSession> = sessions
                    .into_iter()
                    .filter(|s| {
                        if s.name.starts_with(TEMP_SESSION_PREFIX) {
                            log::warn!(
                                "[sessions] Filtering out temp session '{}' from persistence",
                                s.name
                            );
                            false
                        } else {
                            true
                        }
                    })
                    .collect();
                if temp_count > 0 {
                    log::info!(
                        "[sessions] Removed {} temp sessions from persistence file",
                        temp_count
                    );
                }
                let mut deduped = deduplicate(filtered);

                // Legacy-schema upgrade: run AFTER deduplicate() so each row's legacy
                // payload travels with its own entry. `.take()` clears the Options and
                // `skip_serializing_if` in PersistedSession elides them on next save.
                for ps in deduped.iter_mut() {
                    if !ps.git_repos.is_empty() {
                        // Already new-schema; drop any ghost legacy values.
                        ps.git_branch_source = None;
                        ps.git_branch_prefix = None;
                        continue;
                    }
                    match (ps.git_branch_source.take(), ps.git_branch_prefix.take()) {
                        (Some(source), Some(prefix)) if prefix != "multi-repo" => {
                            log::info!(
                                "[sessions] Upgrading legacy single-repo session '{}' → git_repos[1]={{label:{}, source:{}}}",
                                ps.name, prefix, source
                            );
                            ps.git_repos.push(crate::session::session::SessionRepo {
                                label: prefix,
                                source_path: source,
                                branch: None,
                                dirty: None,
                            });
                        }
                        (Some(source), None) => {
                            // Shouldn't happen in data this codebase produces, but serde(default)
                            // + hand-edited files can land here. Synthesize label from dir name.
                            let dir = source
                                .replace('\\', "/")
                                .split('/')
                                .next_back()
                                .unwrap_or("")
                                .to_string();
                            let label =
                                dir.strip_prefix("repo-").map(str::to_string).unwrap_or(dir);
                            log::warn!(
                                "[sessions] Upgrading legacy session '{}' with source but no prefix; synthesized label '{}'",
                                ps.name, label
                            );
                            ps.git_repos.push(crate::session::session::SessionRepo {
                                label,
                                source_path: source,
                                branch: None,
                                dirty: None,
                            });
                        }
                        (None, Some(prefix)) if prefix == "multi-repo" => {
                            log::info!(
                                "[sessions] Legacy multi-repo session '{}' → git_repos left empty; DiscoveryBranchWatcher will backfill",
                                ps.name
                            );
                        }
                        (None, Some(other)) => {
                            log::warn!(
                                "[sessions] Legacy session '{}' has unknown prefix '{}' without source; dropping",
                                ps.name, other
                            );
                        }
                        (None, None) => {}
                        (Some(_), Some(_)) => {
                            // prefix == "multi-repo" with a source — ambiguous legacy shape.
                            log::warn!(
                                "[sessions] Legacy session '{}' had source + multi-repo prefix; leaving git_repos empty for discovery backfill",
                                ps.name
                            );
                        }
                    }
                }

                for ps in deduped.iter_mut() {
                    normalize_persisted_session_paths(ps);
                }

                log::info!(
                    "Loaded {} persisted sessions from {:?}",
                    deduped.len(),
                    path
                );
                deduped
            }
            Err(e) => {
                log::error!("Failed to parse sessions file: {}", e);
                vec![]
            }
        },
        Err(e) => {
            log::error!("Failed to read sessions file: {}", e);
            vec![]
        }
    }
}

pub async fn load_sessions_purging_outside_project_paths(
    project_paths: &[String],
) -> Vec<PersistedSession> {
    let dir = match super::config_dir() {
        Some(d) => d,
        None => {
            log::warn!("Could not determine home directory for session restore");
            return vec![];
        }
    };

    load_sessions_purging_outside_project_paths_in_dir(&dir, project_paths).await
}

async fn load_sessions_purging_outside_project_paths_in_dir(
    dir: &Path,
    project_paths: &[String],
) -> Vec<PersistedSession> {
    match purge_sessions_outside_project_paths_in_dir(dir, project_paths).await {
        Ok(filtered) => filtered,
        Err(e) => {
            log::error!(
                "Failed to rewrite sessions.json after orphan-session purge: {}",
                e
            );
            // Read-only fallback (no save): a stale read here cannot violate the
            // atomicity contract because nothing is written back. Restore
            // reconciles on the next persist.
            match filter_sessions_for_project_paths_blocking(
                load_sessions_from_dir(dir),
                project_paths,
            )
            .await
            {
                Ok(filtered) => filtered,
                Err(e) => {
                    log::error!("Failed to filter sessions after purge failure: {}", e);
                    load_sessions_from_dir(dir)
                }
            }
        }
    }
}

pub(crate) async fn purge_sessions_outside_project_paths_in_dir(
    dir: &Path,
    project_paths: &[String],
) -> Result<Vec<PersistedSession>, String> {
    let (_before_len, filtered) =
        purge_sessions_outside_project_paths_in_dir_locked(dir, project_paths).await?;
    Ok(filtered)
}

/// #698 HIGH fix: the lock-holding core shared by both live purge callers:
/// `commands::config::purge_sessions_after_settings_update_in_dir` (settings
/// update path) and `load_sessions_purging_outside_project_paths` (startup
/// restore path).
///
/// The orphan purge is a load -> filter -> save sequence. Before this fix the
/// load and save ran OUTSIDE `sessions_save_lock()`, so another locked
/// persistence writer could land its durable write between them: the purge read
/// a stale `sessions.json`, a raise-hand then persisted `communication:
/// raiseHand`, and the purge's already-computed stale copy overwrote it,
/// silently dropping a successfully-emitted raise from disk and from CLI
/// `list-sessions`.
///
/// Holding `sessions_save_lock()` across the whole load+filter+save makes the
/// purge a full participant in the same mutual exclusion the raise-hand and
/// `persist_current_state` helpers use, so every sessions persistence writer
/// now serializes on one lock. Whichever writer acquires the lock second
/// re-reads the fresh on-disk state, so neither can clobber the other with stale
/// data. The save is the sync, leaf-level `save_sessions_to_dir` (its own
/// `SAVE_SESSIONS_LOCK` never re-enters this one), so there is no deadlock and
/// no await is held across a second acquisition of this lock.
///
/// Returns `(before_len, filtered)` so callers can retain the filtered snapshot
/// and keep the removed count available for diagnostics.
async fn purge_sessions_outside_project_paths_in_dir_locked(
    dir: &Path,
    project_paths: &[String],
) -> Result<(usize, Vec<PersistedSession>), String> {
    let _guard = sessions_save_lock().lock().await;
    let before = load_sessions_from_dir(dir);
    let before_len = before.len();
    let filtered = filter_sessions_for_project_paths_blocking(before, project_paths).await?;
    if filtered.len() < before_len {
        save_sessions_to_dir(dir, &filtered)?;
    }
    Ok((before_len, filtered))
}

/// Save current sessions to the app config directory (see config_dir()).
///
/// Concurrency model (#291):
/// - **In-process callers are serialized** by `SAVE_SESSIONS_LOCK`; the
///   rename order matches the order of `lock()` acquisition, so "last
///   writer wins" deterministically.
/// - **Per-call unique temp filenames** (`sessions.json.<pid>.<op_id>.tmp`)
///   prevent two callers from ever colliding on a shared `.tmp`, which was
///   the historical Windows `ERROR_FILE_NOT_FOUND` (os error 2) race.
/// - **Cross-process contention** on the destination (a second AC instance,
///   AV, or the Indexer) is absorbed by `rename_with_retry`. Per-call temp
///   names also keep cross-process callers from colliding on `.tmp`.
///
/// What this does NOT solve (out of scope for #291):
/// - **Snapshot freshness races.** Callers usually call
///   `snapshot_sessions(&mgr).await` and then `save_sessions(&snapshot)`
///   non-atomically. A concurrent `SessionManager` mutation between the
///   two can leave the snapshot stale by the time it lands on disk. The
///   next session-lifecycle event re-persists. See §224 G-IMPL-4 in
///   `phone/mailbox.rs` for the documented behavior.
pub fn save_sessions(sessions: &[PersistedSession]) -> Result<(), String> {
    save_sessions_to_config_dir(sessions)
}

fn save_sessions_to_config_dir(sessions: &[PersistedSession]) -> Result<(), String> {
    let dir = super::config_dir().ok_or("Could not determine home directory")?;
    save_sessions_to_dir(&dir, sessions)
}

/// Path-injected core of `save_sessions`. Same concurrency guarantees as
/// `save_sessions`; the explicit `dir` parameter lets tests drive the
/// persistence path through a `tempfile::tempdir()` without touching the
/// process-wide `config_dir()` once-cell.
fn save_sessions_to_dir(dir: &Path, sessions: &[PersistedSession]) -> Result<(), String> {
    // #291 — serialize in-process saves. Recover from poison: a prior
    // panic inside the critical section is rare (the body is sync std::fs
    // + serde), and the on-disk file is atomic (tmp+rename), so the next
    // caller can safely proceed.
    let _guard = SAVE_SESSIONS_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let path = dir.join("sessions.json");

    std::fs::create_dir_all(dir)
        .map_err(|e| format!("Failed to create config directory: {}", e))?;

    let json = serde_json::to_string_pretty(sessions)
        .map_err(|e| format!("Failed to serialize sessions: {}", e))?;

    // #291 — unique temp filename per save. Combined with the mutex above,
    // this kills the shared-`sessions.json.tmp` race: even cross-process
    // concurrent saves cannot race on the temp file, and any leftover
    // `.tmp` from a prior crashed run cannot be mistaken for ours.
    let op_id = SAVE_OP_ID.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let tmp_path = dir.join(format!("sessions.json.{}.{}.tmp", pid, op_id));

    if let Err(e) = std::fs::write(&tmp_path, &json) {
        // Best-effort cleanup: the partial write may have created the file
        // even though the write returned Err (e.g. ENOSPC mid-stream).
        let _ = std::fs::remove_file(&tmp_path);
        return Err(format!("Failed to write temp sessions file: {}", e));
    }

    // #280 §3.1 — atomic rename with bounded retries to absorb transient
    // AV / Indexer / second-instance contention. On exhaustion, fold the
    // diagnostic context into the error so the upstream `log::error!` in
    // `persist_*` is self-contained for forensics.
    if let Err((err_msg, d)) = rename_with_retry(&tmp_path, &path) {
        // #291 — best-effort cleanup of our unique temp file so we don't
        // accumulate `.tmp` litter across failed saves. The mutex + unique
        // name guarantee this remove only touches OUR own temp file.
        let _ = std::fs::remove_file(&tmp_path);
        return Err(format!(
            "Failed to rename sessions file: {} [op_id={} pid={} instance={} attempts={} \
             tmp_existed_before={} final_existed_before={} os_error={:?} duration={:?}]",
            err_msg,
            d.op_id,
            d.pid,
            d.instance_id,
            d.attempts,
            d.tmp_exists_before,
            d.final_exists_before,
            d.last_os_error,
            d.duration
        ));
    }

    log::info!("Saved {} sessions to {:?}", sessions.len(), path);
    Ok(())
}

fn sessions_save_lock() -> &'static tokio::sync::Mutex<()> {
    static SAVE_LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    SAVE_LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Snapshot current live sessions into the persisted format.
/// Strips auto-injected resume flags so they are re-evaluated on next restore.
pub async fn snapshot_sessions(mgr: &SessionManager) -> Vec<PersistedSession> {
    let aggregate = mgr.aggregate_snapshot().await;
    let sessions = aggregate
        .sessions
        .iter()
        .map(SessionInfo::from)
        .collect::<Vec<_>>();
    let active_id = aggregate.selection.id().map(|id| id.to_string());

    let all: Vec<PersistedSession> = sessions
        .iter()
        .filter(|s| {
            if s.name.starts_with(TEMP_SESSION_PREFIX) {
                log::debug!(
                    "[sessions] Excluding temp session '{}' from snapshot",
                    s.name
                );
                false
            } else {
                true
            }
        })
        .map(|s| PersistedSession {
            name: s.name.clone(),
            shell: s.shell.clone(),
            shell_args: strip_auto_injected_args(&s.shell, &s.shell_args),
            working_directory: s.working_directory.clone(),
            was_active: active_id.as_deref() == Some(&s.id),
            git_repos: s.git_repos.clone(),
            is_coordinator: s.is_coordinator,
            is_root_agent: s.is_root_agent,
            agent_id: s.agent_id.clone(),
            agent_label: s.agent_label.clone(),
            requested_profile: s.requested_profile.clone(),
            telegram_bot_id: s.telegram_bot_id.clone(),
            // Fix A: read detach state directly from the Session (via SessionInfo). The
            // `DetachedSessionsState` set is NOT consulted at persist time — the Destroyed
            // handler clears the set before `RunEvent::Exit` runs the final persist.
            was_detached: s.was_detached,
            last_prompt: s.last_prompt.clone(),
            detached_geometry: s.detached_geometry.clone(),
            start_fresh_on_restore: s.start_fresh_on_restore,
            // Legacy fields are always None on new saves; skip_serializing_if elides them.
            git_branch_source: None,
            git_branch_prefix: None,
            // Runtime fields for CLI consumption
            id: Some(s.id.clone()),
            status: Some(s.status.clone()),
            waiting_for_input: Some(s.waiting_for_input),
            communication: s.communication.clone(),
            created_at: Some(s.created_at.clone()),
        })
        .collect();

    deduplicate(all)
}

/// Strip AC-managed provider args from saved shell arguments.
/// Current launch-time injections are Claude's `--continue`, Codex's
/// `resume --last`, and Gemini's `--resume latest`.
/// These must not be baked into the saved "recipe" because they self-perpetuate
/// across app restarts (or session restarts) even when the conditions change.
///
/// Handles two injection modes:
/// - **Direct-exec**: args are separate tokens like `["--continue", ...]` or `["resume", "--last", ...]`
/// - **cmd.exe wrapper**: tokens may be separate args (`["/C", "codex", "resume", "--last"]`)
///   or embedded in a single arg string (`["/K", "git pull && codex resume --last"]`)
pub(crate) fn strip_auto_injected_args(shell: &str, args: &[String]) -> Vec<String> {
    fn strip_claude_tokens(tokens: &mut Vec<String>, start: usize) {
        // #260: Claude's resume flag from the CodingAgentProfile. resume_tokens
        // is a 1-element const for Claude, so [0] is provably in bounds.
        let continue_flag = CodingAgentKind::Claude.profile().resume_tokens[0];
        let mut idx = start;
        while idx < tokens.len() {
            if tokens[idx].eq_ignore_ascii_case(continue_flag) {
                tokens.remove(idx);
                continue;
            }
            // (#756) Rider belt: strip the launcher-minted identity so it can
            // never self-perpetuate; a replayed stale --session-id hard-fails
            // the spawn (transcript collision), unlike a stale --continue.
            // UUID-gated: only the AC-injected `--session-id <v4>` shape is
            // removed; user-authored non-UUID values win.
            let lower = tokens[idx].to_lowercase();
            if lower == "--session-id"
                && tokens
                    .get(idx + 1)
                    .is_some_and(|v| uuid::Uuid::parse_str(v).is_ok())
            {
                tokens.remove(idx);
                tokens.remove(idx); // paired UUID value
                continue;
            }
            if lower
                .strip_prefix("--session-id=")
                .is_some_and(|v| uuid::Uuid::parse_str(v).is_ok())
            {
                tokens.remove(idx);
                continue;
            }
            idx += 1;
        }
    }

    fn strip_codex_tokens(tokens: &mut Vec<String>, start: usize) {
        // #260 — resume tokens from the CodingAgentProfile. G6: slice-pattern
        // destructure, never index; a wrong-arity slice no-ops gracefully.
        let &[resume_subcmd, resume_flag] = CodingAgentKind::Codex.profile().resume_tokens else {
            debug_assert!(false, "Codex resume_tokens must have exactly 2 elements");
            return;
        };
        if tokens
            .get(start)
            .is_some_and(|token| token.eq_ignore_ascii_case(resume_subcmd))
            && tokens
                .get(start + 1)
                .is_some_and(|token| token.eq_ignore_ascii_case(resume_flag))
        {
            tokens.remove(start);
            tokens.remove(start);
        }
    }

    fn strip_gemini_tokens(tokens: &mut Vec<String>, start: usize) {
        // #260 — resume tokens from the CodingAgentProfile. G6: slice-pattern
        // destructure, never index. The joined `--resume=latest` variant is
        // derived from the same two tokens.
        let &[resume_flag, resume_value] = CodingAgentKind::Gemini.profile().resume_tokens else {
            debug_assert!(false, "Gemini resume_tokens must have exactly 2 elements");
            return;
        };
        let joined = format!("{}={}", resume_flag, resume_value);
        if tokens
            .get(start)
            .is_some_and(|token| token.eq_ignore_ascii_case(resume_flag))
            && tokens
                .get(start + 1)
                .is_some_and(|token| token.eq_ignore_ascii_case(resume_value))
        {
            tokens.remove(start);
            tokens.remove(start);
        } else if tokens
            .get(start)
            .is_some_and(|token| token.to_lowercase() == joined)
        {
            tokens.remove(start);
        }
    }

    // #260 — consult the single detector (session/profile.rs) instead of
    // re-deriving agent identity here. Guarantees this stripper agrees with
    // the `agent_kind` that `create_session_inner` stamped on the session.
    let (is_claude, is_codex, is_gemini) = match CodingAgentKind::detect(shell, args) {
        Some(CodingAgentKind::Claude) => (true, false, false),
        Some(CodingAgentKind::Codex) => (false, true, false),
        Some(CodingAgentKind::Gemini) => (false, false, true),
        None => return args.to_vec(),
    };

    let is_cmd = crate::commands::session::executable_basename(shell) == "cmd";

    if is_cmd {
        let mut result = args.to_vec();

        if is_claude {
            if let Some(idx) = result.iter().position(|arg| {
                crate::commands::session::executable_basename(arg).starts_with("claude")
            }) {
                strip_claude_tokens(&mut result, idx + 1);
            }
        }
        if is_codex {
            if let Some(idx) = result
                .iter()
                .position(|arg| crate::commands::session::executable_basename(arg) == "codex")
            {
                strip_codex_tokens(&mut result, idx + 1);
            }
        }
        if is_gemini {
            if let Some(idx) = result
                .iter()
                .position(|arg| crate::commands::session::executable_basename(arg) == "gemini")
            {
                strip_gemini_tokens(&mut result, idx + 1);
            }
        }

        for arg in &mut result {
            let mut tokens: Vec<String> = arg
                .split_whitespace()
                .map(|token| token.to_string())
                .collect();
            let mut changed = false;

            if is_claude {
                if let Some(idx) = tokens.iter().position(|token| {
                    crate::commands::session::executable_basename(token).starts_with("claude")
                }) {
                    let before = tokens.len();
                    strip_claude_tokens(&mut tokens, idx + 1);
                    changed |= tokens.len() != before;
                }
            }

            if is_codex {
                if let Some(idx) = tokens.iter().position(|token| {
                    crate::commands::session::executable_basename(token) == "codex"
                }) {
                    let before = tokens.len();
                    strip_codex_tokens(&mut tokens, idx + 1);
                    changed |= tokens.len() != before;
                }
            }

            if is_gemini {
                if let Some(idx) = tokens.iter().position(|token| {
                    crate::commands::session::executable_basename(token) == "gemini"
                }) {
                    let before = tokens.len();
                    strip_gemini_tokens(&mut tokens, idx + 1);
                    changed |= tokens.len() != before;
                }
            }

            if changed {
                *arg = tokens.join(" ");
            }
        }
        if is_gemini {
            if let Some(idx) = result
                .iter()
                .position(|arg| crate::commands::session::executable_basename(arg) == "gemini")
            {
                strip_gemini_tokens(&mut result, idx + 1);
            }
        }

        result
    } else {
        let mut result = Vec::with_capacity(args.len());
        let mut skip_next = false;
        for (idx, a) in args.iter().enumerate() {
            if skip_next {
                skip_next = false;
                continue;
            }
            if is_codex
                && idx == 0
                && a.eq_ignore_ascii_case("resume")
                && args
                    .get(1)
                    .is_some_and(|next| next.eq_ignore_ascii_case("--last"))
            {
                continue;
            }
            if is_codex
                && idx == 1
                && args
                    .first()
                    .is_some_and(|first| first.eq_ignore_ascii_case("resume"))
                && a.eq_ignore_ascii_case("--last")
            {
                continue;
            }

            if is_gemini && idx == 0 {
                if a.eq_ignore_ascii_case("--resume") {
                    if args
                        .get(1)
                        .is_some_and(|next| next.eq_ignore_ascii_case("latest"))
                    {
                        continue;
                    }
                } else if a.to_lowercase() == "--resume=latest" {
                    continue;
                }
            }
            if is_gemini
                && idx == 1
                && args
                    .first()
                    .is_some_and(|first| first.eq_ignore_ascii_case("--resume"))
                && a.eq_ignore_ascii_case("latest")
            {
                continue;
            }

            if is_claude && a.eq_ignore_ascii_case("--continue") {
                continue;
            }
            // (#756) Rider belt, direct-exec form: strip the launcher-minted
            // `--session-id <v4>` pair / `--session-id=<v4>` joined token.
            // UUID-gated so user-authored non-UUID values are preserved.
            if is_claude {
                let lower = a.to_lowercase();
                if lower == "--session-id"
                    && args
                        .get(idx + 1)
                        .is_some_and(|v| uuid::Uuid::parse_str(v).is_ok())
                {
                    skip_next = true; // consume the paired UUID value too
                    continue;
                }
                if lower
                    .strip_prefix("--session-id=")
                    .is_some_and(|v| uuid::Uuid::parse_str(v).is_ok())
                {
                    continue;
                }
            }
            result.push(a.clone());
        }
        result
    }
}

/// Pure: produce a sanitized copy of a failed-recoverable PersistedSession
/// suitable for merging into a fresh snapshot. Drops runtime fields (`id`,
/// `status`, `waiting_for_input`, `created_at`) since those describe the
/// PRIOR run's state; the session is no longer live, and persisting them
/// would make `list-sessions` (which filters on `id.is_some()`) report the
/// session as alive when it is not. See §224.
///
/// `communication` is deliberately PRESERVED (#747): a raised hand is durable
/// intent, not prior-run runtime state, so a transiently failed restore keeps
/// it for the next startup attempt. `list-sessions` still reports
/// `raisedHand: false` for these rows because `id` and `status` are stripped
/// (both the `id.is_some()` row filter and the `status.is_none()` gate exclude
/// them). Best-effort with the SAME LIFETIME AS THE RECIPE ROW: the row (hand
/// included) survives only until the next `persist_current_state` (§224 G5).
pub(crate) fn sanitize_failed_recoverable(ps: &PersistedSession) -> PersistedSession {
    let mut clean = ps.clone();
    clean.id = None;
    clean.status = None;
    clean.waiting_for_input = None;
    clean.created_at = None;
    clean
}

/// Persist live sessions plus stripped recipes for entries that failed to
/// restore. Stripped recipes survive on disk only until the next
/// `persist_current_state` call (any session-lifecycle event) overwrites the
/// snapshot — so retry-on-next-startup is best-effort. §224 G5/G8.
pub async fn persist_merging_failed_result(
    mgr: &SessionManager,
    failed: &[PersistedSession],
) -> Result<(), String> {
    let dir = super::config_dir().ok_or("Could not determine home directory")?;
    let settings = crate::config::settings::load_settings_for_cli();
    let project_paths = session_retention_project_paths(&settings);
    persist_merging_failed_to_dir_for_project_paths_result(mgr, failed, &dir, Some(&project_paths))
        .await
}

async fn persist_merging_failed_to_dir_for_project_paths_result(
    mgr: &SessionManager,
    failed: &[PersistedSession],
    dir: &Path,
    project_paths: Option<&[String]>,
) -> Result<(), String> {
    let _guard = sessions_save_lock().lock().await;
    let mut snapshot = snapshot_sessions(mgr).await;
    // §224 — strip stale runtime fields (`id`, `status`, `waiting_for_input`,
    // `created_at`) from failed-recoverable entries (`communication` is kept,
    // #747, see `sanitize_failed_recoverable`). Without this, the prior
    // run's runtime fields travel into the new snapshot, and `list-sessions`
    // reports a session as alive (its `s.id.is_some()` filter passes) while
    // the in-memory `SessionManager` does not contain it. `close-session`
    // then can't find the session and rejects with "No active session found".
    // Stripping enforces the invariant: any persisted row with `id.is_some()`
    // is guaranteed to be live in SessionManager.
    //
    // NOTE: this strip preserves the recipe (working_directory, shell,
    // agent_id, was_active, was_detached, etc.) so the next-startup restore
    // can retry. However, "retry-on-next-startup" persistence is best-effort:
    // any subsequent idle/busy event after this call invokes
    // `persist_current_state`, which rebuilds the snapshot from the live
    // SessionManager ONLY and silently drops failed-recoverable entries (§224
    // G5). Proper plumb of `failed_recoverable` into SessionManager is filed
    // as a follow-up.
    snapshot.extend(failed.iter().map(sanitize_failed_recoverable));
    let snapshot = deduplicate(snapshot);
    let snapshot = match project_paths {
        Some(project_paths) => {
            filter_sessions_for_project_paths_blocking(snapshot, project_paths).await?
        }
        None => snapshot,
    };
    save_sessions_to_dir(dir, &snapshot)
}

pub async fn persist_merging_failed(mgr: &SessionManager, failed: &[PersistedSession]) {
    if let Err(e) = persist_merging_failed_result(mgr, failed).await {
        log::error!("Failed to persist sessions (with merge): {}", e);
    }
}

/// Convenience: snapshot and save in one call. Logs errors but never fails.
pub async fn persist_current_state_result(mgr: &SessionManager) -> Result<(), String> {
    let dir = super::config_dir().ok_or("Could not determine home directory")?;
    let settings = crate::config::settings::load_settings_for_cli();
    let project_paths = session_retention_project_paths(&settings);
    persist_current_state_to_dir_for_project_paths_result(mgr, &dir, Some(&project_paths)).await
}

async fn persist_current_state_to_dir_for_project_paths_result(
    mgr: &SessionManager,
    dir: &Path,
    project_paths: Option<&[String]>,
) -> Result<(), String> {
    let _guard = sessions_save_lock().lock().await;
    snapshot_and_save_locked(mgr, dir, project_paths).await
}

/// Snapshot the live sessions, apply the project-path filter, and save.
///
/// CONTRACT: the caller MUST already hold `sessions_save_lock()`. This is the
/// lock-free body shared by `persist_current_state_to_dir_for_project_paths_result`
/// and the #698 atomic mutate-then-persist helpers
/// (`raise_hand_and_persist_*`, `clear_user_input_transitions_and_persist_*`),
/// which take that lock once and run a `SessionManager` mutation plus this save
/// under it. Splitting it out keeps those helpers from re-acquiring the tokio
/// mutex (which is not reentrant and would deadlock).
async fn snapshot_and_save_locked(
    mgr: &SessionManager,
    dir: &Path,
    project_paths: Option<&[String]>,
) -> Result<(), String> {
    let snapshot = snapshot_sessions(mgr).await;
    let snapshot = match project_paths {
        Some(project_paths) => {
            filter_sessions_for_project_paths_blocking(snapshot, project_paths).await?
        }
        None => snapshot,
    };
    save_sessions_to_dir(dir, &snapshot)
}

#[cfg(test)]
async fn persist_current_state_to_dir_result(
    mgr: &SessionManager,
    dir: &Path,
) -> Result<(), String> {
    persist_current_state_to_dir_for_project_paths_result(mgr, dir, None).await
}

pub async fn persist_current_state(mgr: &SessionManager) {
    if let Err(e) = persist_current_state_result(mgr).await {
        log::error!("Failed to persist sessions: {}", e);
    }
}

/// #698 — outcome of an atomic raise-hand-and-persist attempt.
#[derive(Debug)]
pub enum RaiseHandPersistOutcome {
    /// The hand was newly raised and the snapshot was saved durably. The caller
    /// should emit `session_communication_changed` with this communication.
    Raised(SessionCommunication),
    /// A visible raise-hand was already present; nothing changed and nothing was
    /// persisted. The raise is still active (`raised: true`, status `already_visible`).
    AlreadyVisible,
    /// The session cannot raise its hand (missing, non-coordinator, or exited);
    /// `raised: false`, status `not_visible`.
    NotRaisable,
}

/// #698 — apply the raise-hand transition and persist the resulting snapshot
/// atomically with respect to ALL session persistence.
///
/// Fix for the HIGH grinch finding: the live mutation and its durable snapshot
/// must not be observable independently. We take `sessions_save_lock()` FIRST,
/// so no concurrent `persist_current_state` caller can snapshot or write the
/// raised state between our mutation and our save. We then mutate, snapshot, and
/// save under that single lock. On save failure we roll back the live
/// communication BEFORE releasing the lock, so neither memory nor disk retains a
/// raise that did not survive: any later persist can only ever snapshot the
/// cleared state.
///
/// We deliberately do NOT call `persist_current_state_result` here; it would
/// re-acquire the same (non-reentrant) tokio mutex and deadlock. We call the
/// lock-free `snapshot_and_save_locked` instead.
pub async fn raise_hand_and_persist_result(
    mgr: &SessionManager,
    session_id: Uuid,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<RaiseHandPersistOutcome, String> {
    let dir = super::config_dir().ok_or("Could not determine home directory")?;
    let settings = crate::config::settings::load_settings_for_cli();
    let project_paths = session_retention_project_paths(&settings);
    raise_hand_and_persist_to_dir_result(mgr, session_id, now, &dir, Some(&project_paths)).await
}

async fn raise_hand_and_persist_to_dir_result(
    mgr: &SessionManager,
    session_id: Uuid,
    now: chrono::DateTime<chrono::Utc>,
    dir: &Path,
    project_paths: Option<&[String]>,
) -> Result<RaiseHandPersistOutcome, String> {
    let _guard = sessions_save_lock().lock().await;

    let communication = match mgr.raise_hand(session_id, now).await {
        Some((true, communication)) => communication,
        Some((false, _)) => return Ok(RaiseHandPersistOutcome::AlreadyVisible),
        None => return Ok(RaiseHandPersistOutcome::NotRaisable),
    };

    if let Err(e) = snapshot_and_save_locked(mgr, dir, project_paths).await {
        // Roll back the live raise under the still-held lock so a raise that
        // failed to persist is visible nowhere (memory or disk).
        let _ = mgr
            .clear_communication_if_kind(session_id, SessionCommunicationKind::RaiseHand)
            .await;
        return Err(e);
    }

    Ok(RaiseHandPersistOutcome::Raised(communication))
}

/// #698: the user-input session-state transitions cleared by
/// `clear_user_input_transitions_and_persist_result`, reported so the caller can
/// decide whether to emit the raise-hand clear event.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClearedUserInputTransitions {
    /// `start_fresh_on_restore` flipped `true -> false` (#630/#631 re-arm).
    pub cleared_start_fresh: bool,
    /// A visible raise-hand was lowered (#698).
    pub cleared_raise_hand: bool,
}

/// #698: clear the user-input session-state transitions and persist the result
/// atomically with respect to all session persistence. `clear_fresh` gates the
/// `start_fresh_on_restore` re-arm to substantive post-boundary submissions
/// (#871); lowering any visible raise-hand remains unconditional.
///
/// Fix for the MEDIUM grinch finding: the two field clears previously ran in two
/// separate critical sections with an await between them, so a concurrent persist
/// could snapshot a half-applied state (`startFreshOnRestore: false` with a still
/// visible `raiseHand`). Here both fields flip in ONE `SessionManager` critical
/// section (`clear_user_input_transitions`), and the mutation plus its snapshot
/// save run under a single `sessions_save_lock()` acquisition, so no other
/// persistence caller can write an intermediate state.
///
/// Unlike the raise-hand path there is no rollback: a real user message means the
/// hand is lowered and the fresh intent re-armed regardless of whether this
/// snapshot reaches disk. The in-memory clear is therefore applied unconditionally
/// (even when the home directory cannot be resolved); on save failure it stands
/// and the next persist reconciles the file. The caller learns of any failure
/// through `Err` and, per existing behavior, suppresses the clear event.
pub async fn clear_user_input_transitions_and_persist_result(
    mgr: &SessionManager,
    session_id: Uuid,
    clear_fresh: bool,
) -> Result<ClearedUserInputTransitions, String> {
    let dir = super::config_dir();
    let settings = crate::config::settings::load_settings_for_cli();
    let project_paths = session_retention_project_paths(&settings);
    clear_user_input_transitions_and_persist_to_dir_result(
        mgr,
        session_id,
        clear_fresh,
        dir.as_deref(),
        Some(&project_paths),
    )
    .await
}

async fn clear_user_input_transitions_and_persist_to_dir_result(
    mgr: &SessionManager,
    session_id: Uuid,
    clear_fresh: bool,
    dir: Option<&Path>,
    project_paths: Option<&[String]>,
) -> Result<ClearedUserInputTransitions, String> {
    let _guard = sessions_save_lock().lock().await;

    // Mutate FIRST and unconditionally: the user typed, so the hand is lowered
    // and, when gated, the fresh intent is re-armed even if we cannot persist.
    // Both fields flip in one critical section, so no snapshot can capture a
    // half-applied state.
    let (cleared_start_fresh, cleared_raise_hand) = mgr
        .clear_user_input_transitions(session_id, clear_fresh)
        .await;
    let cleared = ClearedUserInputTransitions {
        cleared_start_fresh,
        cleared_raise_hand,
    };

    if cleared_start_fresh || cleared_raise_hand {
        let dir = dir.ok_or("Could not determine home directory")?;
        snapshot_and_save_locked(mgr, dir, project_paths).await?;
    }

    Ok(cleared)
}

/// (#756) Stamp the durable fresh intent (AC-driven clear boundary) and persist
/// the result atomically with respect to all session persistence. Returns
/// Ok(true) iff the field transitioned false -> true (a snapshot was saved).
/// Same lock discipline as `clear_user_input_transitions_and_persist_result`:
/// mutation + snapshot + save under a single `sessions_save_lock()` acquisition,
/// so no concurrent persist can write an intermediate state.
pub async fn set_start_fresh_and_persist_result(
    mgr: &SessionManager,
    session_id: Uuid,
) -> Result<bool, String> {
    let dir = super::config_dir();
    let settings = crate::config::settings::load_settings_for_cli();
    let project_paths = session_retention_project_paths(&settings);
    write_start_fresh_and_persist_to_dir_result(
        mgr,
        session_id,
        true,
        dir.as_deref(),
        Some(&project_paths),
    )
    .await
}

/// (#756) Drop the durable fresh intent (AC injected post-boundary CONTENT) and
/// persist. Returns Ok(true) iff the field transitioned true -> false.
/// DELIBERATELY NARROW: touches only `start_fresh_on_restore`; never raise-hand,
/// silence, or badge state (this is NOT the user-input choke point).
pub async fn clear_start_fresh_and_persist_result(
    mgr: &SessionManager,
    session_id: Uuid,
) -> Result<bool, String> {
    let dir = super::config_dir();
    let settings = crate::config::settings::load_settings_for_cli();
    let project_paths = session_retention_project_paths(&settings);
    write_start_fresh_and_persist_to_dir_result(
        mgr,
        session_id,
        false,
        dir.as_deref(),
        Some(&project_paths),
    )
    .await
}

/// (#756) Shared core for the fresh-intent stamp/drop persist wrappers. Like the
/// #698 helper, the in-memory mutation stands even if the save fails (Err
/// reports the save failure; callers log a warn, no rollback).
async fn write_start_fresh_and_persist_to_dir_result(
    mgr: &SessionManager,
    session_id: Uuid,
    value: bool,
    dir: Option<&Path>,
    project_paths: Option<&[String]>,
) -> Result<bool, String> {
    let _guard = sessions_save_lock().lock().await;
    let changed = if value {
        mgr.set_start_fresh_on_restore_if_unset(session_id).await
    } else {
        mgr.clear_start_fresh_on_restore_if_set(session_id).await
    };
    if changed {
        let dir = dir.ok_or("Could not determine home directory")?;
        snapshot_and_save_locked(mgr, dir, project_paths).await?;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::{
        clear_user_input_transitions_and_persist_to_dir_result,
        filter_sessions_for_normalized_roots, filter_sessions_for_project_paths,
        filter_sessions_for_project_paths_blocking, is_under_normalized_archived_roots,
        load_sessions_purging_outside_project_paths_in_dir, load_sessions_raw_from_dir_for_test,
        normalize_project_roots, persist_current_state_result, persist_current_state_to_dir_result,
        purge_sessions_outside_project_paths_in_dir, raise_hand_and_persist_to_dir_result,
        rename_with_retry, sanitize_failed_recoverable, save_sessions_to_dir,
        session_retention_project_paths, sessions_save_lock, snapshot_sessions,
        strip_auto_injected_args, working_directory_under_any_project_path,
        write_start_fresh_and_persist_to_dir_result, PersistedSession, RaiseHandPersistOutcome,
        FILTER_PROJECT_PATHS_THREAD_IDS, NORMALIZE_CALLS, RENAME_ATTEMPTS,
    };
    #[cfg(windows)]
    use super::{deduplicate, load_sessions_from_path};
    use crate::config::settings::AppSettings;
    use crate::session::manager::SessionManager;
    use crate::session::session::{SessionCommunication, SessionCommunicationKind, SessionStatus};
    use std::sync::Arc;
    use std::time::Duration;

    /// §224 D.2 — the strip drops every runtime field but preserves the recipe
    /// fields needed for the next-startup restore attempt. Since #747 a raised
    /// hand counts as durable intent, not runtime state: `communication`
    /// survives the strip so the retry can restore it.
    #[test]
    fn sanitize_failed_recoverable_drops_runtime_fields_keeps_raise_hand() {
        let ps = PersistedSession {
            last_prompt: None,
            name: "alice".into(),
            shell: "claude".into(),
            shell_args: vec!["--continue".into()],
            working_directory: r"C:\proj\.ac\wg-1-devs\__agent_alice".into(),
            was_active: false,
            git_repos: vec![],
            is_coordinator: false,
            is_root_agent: false,
            agent_id: Some("aid-1".into()),
            agent_label: Some("Claude Code".into()),
            requested_profile: None,
            telegram_bot_id: Some("bot-1".into()),
            was_detached: false,
            detached_geometry: None,
            git_branch_source: None,
            git_branch_prefix: None,
            // Stale runtime fields from a prior run:
            id: Some("uuid-prior-run".into()),
            status: Some(SessionStatus::Idle),
            waiting_for_input: Some(true),
            communication: Some(SessionCommunication {
                kind: SessionCommunicationKind::RaiseHand,
                visible: true,
                updated_at: "2026-06-30T11:00:00+00:00".into(),
            }),
            created_at: Some("2026-05-15T00:00:00Z".into()),
            ..Default::default()
        };

        let clean = sanitize_failed_recoverable(&ps);

        // Runtime fields cleared:
        assert!(clean.id.is_none(), "id must be cleared");
        assert!(clean.status.is_none(), "status must be cleared");
        assert!(
            clean.waiting_for_input.is_none(),
            "waiting_for_input must be cleared"
        );
        assert!(clean.created_at.is_none(), "created_at must be cleared");

        // Recipe fields preserved (so next-run restore can retry):
        let kept = clean
            .communication
            .as_ref()
            .expect("#747: the raised hand must survive the strip");
        assert_eq!(kept.kind, SessionCommunicationKind::RaiseHand);
        assert!(kept.visible, "visibility must survive the strip");
        assert_eq!(clean.name, "alice");
        assert_eq!(clean.shell, "claude");
        assert_eq!(clean.shell_args, vec!["--continue".to_string()]);
        assert_eq!(clean.working_directory, ps.working_directory);
        assert_eq!(clean.agent_id.as_deref(), Some("aid-1"));
        assert_eq!(clean.agent_label.as_deref(), Some("Claude Code"));
        assert_eq!(clean.telegram_bot_id.as_deref(), Some("bot-1"));
        assert!(!clean.was_active);
        assert!(!clean.was_detached);
    }

    /// §224 D.2 — idempotence: stripping an entry that already has None
    /// runtime fields is a no-op (does not flip recipe fields).
    #[test]
    fn sanitize_failed_recoverable_is_idempotent() {
        let ps = PersistedSession {
            last_prompt: None,
            name: "bob".into(),
            shell: "cmd".into(),
            shell_args: vec![],
            working_directory: "C:/x".into(),
            was_active: false,
            git_repos: vec![],
            is_coordinator: false,
            is_root_agent: false,
            agent_id: None,
            agent_label: None,
            requested_profile: None,
            telegram_bot_id: None,
            was_detached: false,
            detached_geometry: None,
            git_branch_source: None,
            git_branch_prefix: None,
            id: None,
            status: None,
            waiting_for_input: None,
            created_at: None,
            ..Default::default()
        };
        let once = sanitize_failed_recoverable(&ps);
        let twice = sanitize_failed_recoverable(&once);
        assert!(twice.id.is_none());
        assert!(twice.status.is_none());
        assert!(twice.waiting_for_input.is_none());
        assert!(twice.created_at.is_none());
        assert_eq!(twice.name, "bob");
    }

    #[test]
    fn telegram_bot_id_defaults_none_for_legacy_json() {
        let json = r#"{
            "name": "legacy",
            "shell": "cmd",
            "shellArgs": [],
            "workingDirectory": "C:/x"
        }"#;

        let back: PersistedSession = serde_json::from_str(json).expect("deserialize");
        assert!(back.telegram_bot_id.is_none());
    }

    #[test]
    fn communication_defaults_none_for_legacy_json() {
        let json = r#"{
            "name": "legacy",
            "shell": "cmd",
            "shellArgs": [],
            "workingDirectory": "C:/x"
        }"#;

        let back: PersistedSession = serde_json::from_str(json).expect("deserialize");
        assert!(back.communication.is_none());
    }

    /// Issue 698: a malformed/future typed `communication` field invalidates the
    /// entire raw load (serde fails the whole array, `unwrap_or_default` yields
    /// `[]`), exactly like other malformed typed persisted fields. This is the
    /// accepted behavior for #698; documented here so it is intentional, not a
    /// silent regression. Revisit in a separate persistence-resilience issue.
    #[test]
    fn load_sessions_raw_returns_empty_for_malformed_communication() {
        let temp = tempfile::tempdir().expect("tempdir");
        let sessions = serde_json::json!([
            {
                "name": "coord-x",
                "shell": "codex",
                "shellArgs": [],
                "workingDirectory": "C:/proj/.ac/wg-1-dev-team/__agent_tech-lead",
                "id": "11111111-1111-1111-1111-111111111111",
                "status": "running",
                "waitingForInput": false,
                "isCoordinator": true,
                "communication": {
                    "kind": "futureKind",
                    "visible": true,
                    "updatedAt": "2026-06-30T11:00:00+00:00"
                },
                "createdAt": "2026-06-30T10:00:00+00:00"
            }
        ]);
        std::fs::write(
            temp.path().join("sessions.json"),
            serde_json::to_string_pretty(&sessions).expect("sessions json"),
        )
        .expect("write sessions");

        let rows = load_sessions_raw_from_dir_for_test(temp.path());

        assert!(
            rows.is_empty(),
            "malformed typed communication intentionally invalidates the raw load in issue 698"
        );
    }

    #[test]
    fn telegram_bot_id_round_trips_when_present() {
        let ps = PersistedSession {
            last_prompt: None,
            name: "telegram-on".into(),
            shell: "codex".into(),
            shell_args: vec![],
            working_directory: "C:/x".into(),
            telegram_bot_id: Some("bot-1".into()),
            ..Default::default()
        };

        let json = serde_json::to_value(&ps).expect("serialize");
        assert_eq!(json["telegramBotId"], "bot-1");
        let back: PersistedSession = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back.telegram_bot_id.as_deref(), Some("bot-1"));
    }

    // (#630 fleet-safety) A pre-existing sessions.json record that predates this
    // field must deserialize to `false` = resume, so no existing user is silently
    // flipped to "start fresh" on upgrade. Polarity guard for §3.1.
    #[test]
    fn start_fresh_on_restore_defaults_false_for_legacy_json() {
        let json = r#"{
            "name": "legacy",
            "shell": "cmd",
            "shellArgs": [],
            "workingDirectory": "C:/x"
        }"#;

        let back: PersistedSession = serde_json::from_str(json).expect("deserialize");
        assert!(!back.start_fresh_on_restore);
    }

    // (#631) The durable fresh intent survives a serialize -> deserialize round-trip
    // in both polarities, so "Restart Session" persists across an app restart. The
    // field has no `skip_serializing_if`, so it is always written explicitly.
    #[test]
    fn start_fresh_on_restore_round_trips() {
        for fresh in [true, false] {
            let ps = PersistedSession {
                name: "round-trip".into(),
                shell: "claude".into(),
                shell_args: vec![],
                working_directory: "C:/x".into(),
                start_fresh_on_restore: fresh,
                ..Default::default()
            };
            let json = serde_json::to_value(&ps).expect("serialize");
            assert_eq!(json["startFreshOnRestore"], fresh);
            let back: PersistedSession = serde_json::from_value(json).expect("deserialize");
            assert_eq!(back.start_fresh_on_restore, fresh);
        }
    }

    #[tokio::test]
    async fn snapshot_sessions_preserves_telegram_bot_id() {
        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "powershell.exe".to_string(),
                Vec::new(),
                "C:\\tmp".to_string(),
                None,
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create_session should succeed");

        mgr.set_telegram_bot_id(session.id, Some("bot-1".into()))
            .await;

        let snapshot = snapshot_sessions(&mgr).await;

        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].telegram_bot_id.as_deref(), Some("bot-1"));
    }

    #[tokio::test]
    async fn snapshot_sessions_preserves_raise_hand_communication() {
        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "powershell.exe".to_string(),
                Vec::new(),
                "C:\\tmp".to_string(),
                None,
                None,
                Vec::new(),
                true,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create_session should succeed");
        let now = chrono::DateTime::parse_from_rfc3339("2026-06-30T11:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let (_, expected) = mgr.raise_hand(session.id, now).await.unwrap();

        let snapshot = snapshot_sessions(&mgr).await;

        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].communication, Some(expected));
    }

    // (#630/#631) The durable fresh intent flows Session -> SessionInfo carrier ->
    // PersistedSession through the real snapshot path, so a restart-fresh session's
    // intent reaches disk (and is not lost at the SessionInfo boundary).
    #[tokio::test]
    async fn snapshot_sessions_preserves_start_fresh_on_restore() {
        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "claude-mb".to_string(),
                Vec::new(),
                "C:\\tmp".to_string(),
                None,
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create_session should succeed");

        // Default snapshots as resume (false).
        let before = snapshot_sessions(&mgr).await;
        assert_eq!(before.len(), 1);
        assert!(!before[0].start_fresh_on_restore);

        mgr.set_start_fresh_on_restore(session.id, true).await;

        let after = snapshot_sessions(&mgr).await;
        assert_eq!(after.len(), 1);
        assert!(after[0].start_fresh_on_restore);
    }

    #[tokio::test]
    async fn persist_current_state_captures_snapshot_inside_save_lock() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "powershell.exe".to_string(),
                Vec::new(),
                "C:\\tmp".to_string(),
                None,
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create_session should succeed");

        let stale_snapshot = snapshot_sessions(&mgr).await;
        assert!(stale_snapshot[0].telegram_bot_id.is_none());

        let guard = sessions_save_lock().lock().await;
        let mut persist = Box::pin(persist_current_state_to_dir_result(&mgr, temp.path()));
        let timed_out = tokio::time::timeout(Duration::from_millis(25), &mut persist)
            .await
            .is_err();
        assert!(timed_out, "persistence should wait for the save lock");

        mgr.set_telegram_bot_id(session.id, Some("bot-1".into()))
            .await;
        drop(guard);
        persist.await.expect("persist current state");

        let saved =
            std::fs::read_to_string(temp.path().join("sessions.json")).expect("read sessions.json");
        let rows: Vec<PersistedSession> = serde_json::from_str(&saved).expect("deserialize");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].telegram_bot_id.as_deref(), Some("bot-1"));
    }

    #[test]
    fn save_sessions_to_dir_returns_create_dir_error_for_file_target() {
        let temp = tempfile::tempdir().expect("tempdir");
        let file_path = temp.path().join("not-a-dir");
        std::fs::write(&file_path, "already a file").expect("write file target");

        let err = save_sessions_to_dir(&file_path, &[]).expect_err("file target should fail");

        assert!(
            err.contains("Failed to create config directory"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn persist_current_state_result_succeeds_for_simple_manager() {
        let mgr = SessionManager::new();
        mgr.create_session(
            "powershell.exe".to_string(),
            Vec::new(),
            "C:\\tmp".to_string(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create_session should succeed");

        persist_current_state_result(&mgr)
            .await
            .expect("persist_current_state_result should succeed");
    }

    // ---- #698 grinch HIGH: atomic raise-hand + persist with rollback ----

    /// Happy path: a coordinator's raised hand reaches `sessions.json` through the
    /// single save-lock acquisition.
    #[tokio::test]
    async fn raise_hand_and_persist_writes_raised_state_to_disk() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "powershell.exe".to_string(),
                Vec::new(),
                "C:\\tmp".to_string(),
                None,
                None,
                Vec::new(),
                true, // coordinator
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create_session should succeed");
        let now = chrono::DateTime::parse_from_rfc3339("2026-06-30T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);

        let outcome =
            raise_hand_and_persist_to_dir_result(&mgr, session.id, now, temp.path(), None)
                .await
                .expect("raise-hand persist should succeed");
        assert!(matches!(outcome, RaiseHandPersistOutcome::Raised(_)));

        // Live state is raised...
        let live = mgr.list_sessions().await;
        let live_comm = live[0].communication.as_ref().expect("live communication");
        assert_eq!(live_comm.kind, SessionCommunicationKind::RaiseHand);
        assert!(live_comm.visible);

        // ...and so is the durable snapshot.
        let saved =
            std::fs::read_to_string(temp.path().join("sessions.json")).expect("read sessions.json");
        let rows: Vec<PersistedSession> = serde_json::from_str(&saved).expect("deserialize");
        assert_eq!(rows.len(), 1);
        let comm = rows[0]
            .communication
            .as_ref()
            .expect("communication persisted");
        assert_eq!(comm.kind, SessionCommunicationKind::RaiseHand);
        assert!(comm.visible);
    }

    /// HIGH grinch fix: when the snapshot save fails, the live raise is rolled back
    /// under the same lock, so a raise that did not survive is visible nowhere
    /// (memory or disk). Without the rollback, `list-sessions` could report
    /// `raisedHand:true` for a raise that never persisted.
    #[tokio::test]
    async fn raise_hand_and_persist_rolls_back_live_state_on_save_failure() {
        let temp = tempfile::tempdir().expect("tempdir");
        // Point the "dir" at an existing FILE so `save_sessions_to_dir`'s
        // `create_dir_all` fails and the save errors out deterministically.
        let file_as_dir = temp.path().join("sessions-dir-is-a-file");
        std::fs::write(&file_as_dir, "not a directory").expect("write file target");

        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "powershell.exe".to_string(),
                Vec::new(),
                "C:\\tmp".to_string(),
                None,
                None,
                Vec::new(),
                true,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create_session should succeed");

        let result = raise_hand_and_persist_to_dir_result(
            &mgr,
            session.id,
            chrono::Utc::now(),
            &file_as_dir,
            None,
        )
        .await;
        assert!(result.is_err(), "save into a file path must fail");

        let live = mgr.list_sessions().await;
        assert!(
            live[0].communication.is_none(),
            "raise must be rolled back after a failed persist"
        );
    }

    /// HIGH grinch fix: the raise mutation is gated by `sessions_save_lock()`. While
    /// another persistence caller holds it, the raise is not even applied (let alone
    /// persisted), so no concurrent persist can snapshot a raised-but-unpersisted
    /// state.
    #[tokio::test]
    async fn raise_hand_and_persist_waits_for_save_lock_before_mutating() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "powershell.exe".to_string(),
                Vec::new(),
                "C:\\tmp".to_string(),
                None,
                None,
                Vec::new(),
                true,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create_session should succeed");

        let guard = sessions_save_lock().lock().await;
        let mut fut = Box::pin(raise_hand_and_persist_to_dir_result(
            &mgr,
            session.id,
            chrono::Utc::now(),
            temp.path(),
            None,
        ));
        let timed_out = tokio::time::timeout(Duration::from_millis(25), &mut fut)
            .await
            .is_err();
        assert!(
            timed_out,
            "raise-hand persist should wait for the save lock"
        );

        // The mutation has NOT happened yet: the helper is parked at the lock.
        assert!(
            mgr.list_sessions().await[0].communication.is_none(),
            "raise must not be applied while the save lock is held elsewhere"
        );

        drop(guard);
        let outcome = fut.await.expect("raise-hand persist should succeed");
        assert!(matches!(outcome, RaiseHandPersistOutcome::Raised(_)));
        assert!(mgr.list_sessions().await[0].communication.is_some());
    }

    /// (#747) Acceptance criterion 5's persist-restore-persist half: a hand
    /// raised and persisted in run A re-applies onto run B's dormant (Exited)
    /// record via `restore_communication` and survives B's own persist, so a
    /// SECOND restart still sees it. The user-input clear tail is already
    /// pinned by `clear_user_input_transitions_persists_both_cleared_fields`.
    #[tokio::test]
    async fn dormant_restore_round_trip_preserves_visible_raise_hand() {
        let temp = tempfile::tempdir().expect("tempdir");

        // Run A: live coordinator raises its hand; #698 persists it durably.
        let mgr_a = SessionManager::new();
        let session_a = mgr_a
            .create_session(
                "claude".to_string(),
                Vec::new(),
                "C:\\tmp".to_string(),
                None,
                None,
                Vec::new(),
                true,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create_session should succeed");
        let raise_time = chrono::DateTime::parse_from_rfc3339("2026-06-30T11:00:00+00:00")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let outcome = raise_hand_and_persist_to_dir_result(
            &mgr_a,
            session_a.id,
            raise_time,
            temp.path(),
            None,
        )
        .await
        .expect("raise+persist should succeed");
        assert!(matches!(outcome, RaiseHandPersistOutcome::Raised(_)));

        let rows = load_sessions_raw_from_dir_for_test(temp.path());
        assert_eq!(rows.len(), 1);
        let persisted_hand = rows[0]
            .communication
            .clone()
            .expect("run A must persist the visible hand");
        assert_eq!(persisted_hand.kind, SessionCommunicationKind::RaiseHand);
        assert!(persisted_hand.visible);

        // Run B (simulated relaunch, default settings): the defer arm creates
        // the record, marks it Exited, then re-applies the persisted hand.
        let mgr_b = SessionManager::new();
        let session_b = mgr_b
            .create_session(
                rows[0].shell.clone(),
                rows[0].shell_args.clone(),
                rows[0].working_directory.clone(),
                None,
                None,
                Vec::new(),
                true,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create_session should succeed");
        mgr_b.mark_exited(session_b.id, 0).await;
        assert!(
            mgr_b
                .restore_communication(session_b.id, persisted_hand.clone())
                .await,
            "dormant coordinator must accept the restored hand"
        );

        persist_current_state_to_dir_result(&mgr_b, temp.path())
            .await
            .expect("run B persist should succeed");

        let rows_b = load_sessions_raw_from_dir_for_test(temp.path());
        assert_eq!(rows_b.len(), 1);
        assert!(
            matches!(rows_b[0].status, Some(SessionStatus::Exited(0))),
            "run B must persist the dormant status, got {:?}",
            rows_b[0].status
        );
        let hand_b = rows_b[0]
            .communication
            .as_ref()
            .expect("the restored hand must survive run B's persist");
        assert_eq!(hand_b.kind, SessionCommunicationKind::RaiseHand);
        assert!(hand_b.visible);
        assert_eq!(
            hand_b.updated_at, persisted_hand.updated_at,
            "the original raise time must survive the round trip"
        );
    }

    // ---- #698 grinch MEDIUM: single-critical-section user-input clear ----

    /// MEDIUM grinch fix: both user-input transitions (re-arm `start_fresh_on_restore`
    /// + lower raise-hand) clear together and the cleared state reaches disk.
    #[tokio::test]
    async fn clear_user_input_transitions_persists_both_cleared_fields() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "powershell.exe".to_string(),
                Vec::new(),
                "C:\\tmp".to_string(),
                None,
                None,
                Vec::new(),
                true,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create_session should succeed");
        mgr.set_start_fresh_on_restore(session.id, true).await;
        mgr.raise_hand(session.id, chrono::Utc::now())
            .await
            .expect("raise_hand should succeed");

        let cleared = clear_user_input_transitions_and_persist_to_dir_result(
            &mgr,
            session.id,
            true,
            Some(temp.path()),
            None,
        )
        .await
        .expect("clear+persist should succeed");
        assert!(cleared.cleared_start_fresh);
        assert!(cleared.cleared_raise_hand);

        // Live state cleared.
        let live = mgr.list_sessions().await;
        assert!(!live[0].start_fresh_on_restore);
        assert!(live[0].communication.is_none());

        // Durable snapshot cleared.
        let saved =
            std::fs::read_to_string(temp.path().join("sessions.json")).expect("read sessions.json");
        let rows: Vec<PersistedSession> = serde_json::from_str(&saved).expect("deserialize");
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].start_fresh_on_restore);
        assert!(rows[0].communication.is_none());
    }

    /// MEDIUM grinch fix: a real user message lowers the hand and re-arms the fresh
    /// intent even if the snapshot save fails (no rollback). The in-memory clear must
    /// stand; the next persist reconciles disk.
    #[tokio::test]
    async fn clear_user_input_transitions_keeps_clear_on_save_failure() {
        let temp = tempfile::tempdir().expect("tempdir");
        let file_as_dir = temp.path().join("sessions-dir-is-a-file");
        std::fs::write(&file_as_dir, "not a directory").expect("write file target");

        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "powershell.exe".to_string(),
                Vec::new(),
                "C:\\tmp".to_string(),
                None,
                None,
                Vec::new(),
                true,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create_session should succeed");
        mgr.set_start_fresh_on_restore(session.id, true).await;
        mgr.raise_hand(session.id, chrono::Utc::now())
            .await
            .expect("raise_hand should succeed");

        let result = clear_user_input_transitions_and_persist_to_dir_result(
            &mgr,
            session.id,
            true,
            Some(&file_as_dir),
            None,
        )
        .await;
        assert!(result.is_err(), "save into a file path must fail");

        // No rollback: the user typed, so the clear stands in memory.
        let live = mgr.list_sessions().await;
        assert!(!live[0].start_fresh_on_restore);
        assert!(live[0].communication.is_none());
    }

    /// MEDIUM grinch fix: the user-input clear mutation is gated by
    /// `sessions_save_lock()`, so a concurrent persist cannot snapshot or write a
    /// half-cleared state between the mutation and its save.
    #[tokio::test]
    async fn clear_user_input_transitions_waits_for_save_lock_before_mutating() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "powershell.exe".to_string(),
                Vec::new(),
                "C:\\tmp".to_string(),
                None,
                None,
                Vec::new(),
                true,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create_session should succeed");
        mgr.set_start_fresh_on_restore(session.id, true).await;
        mgr.raise_hand(session.id, chrono::Utc::now())
            .await
            .expect("raise_hand should succeed");

        let guard = sessions_save_lock().lock().await;
        let mut fut = Box::pin(clear_user_input_transitions_and_persist_to_dir_result(
            &mgr,
            session.id,
            true,
            Some(temp.path()),
            None,
        ));
        let timed_out = tokio::time::timeout(Duration::from_millis(25), &mut fut)
            .await
            .is_err();
        assert!(timed_out, "user-input clear should wait for the save lock");

        // The mutation has NOT happened yet: both fields are still set.
        let parked = mgr.list_sessions().await;
        assert!(
            parked[0].start_fresh_on_restore,
            "fresh intent must remain until the lock frees"
        );
        assert!(
            parked[0].communication.is_some(),
            "raise must remain until the lock frees"
        );

        drop(guard);
        let cleared = fut.await.expect("clear+persist should succeed");
        assert!(cleared.cleared_start_fresh && cleared.cleared_raise_hand);
        let after = mgr.list_sessions().await;
        assert!(!after[0].start_fresh_on_restore);
        assert!(after[0].communication.is_none());
    }

    // ---- (#756) fresh-intent stamp/drop persist wrappers ----

    /// (#756) The stamp persists `startFreshOnRestore: true` exactly once: the
    /// first call transitions and saves, the second returns Ok(false) (no
    /// rewrite needed).
    #[tokio::test]
    async fn set_start_fresh_persists_true_transition_exactly_once() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "powershell.exe".to_string(),
                Vec::new(),
                "C:\\tmp".to_string(),
                None,
                None,
                Vec::new(),
                true,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create_session should succeed");

        let stamped = write_start_fresh_and_persist_to_dir_result(
            &mgr,
            session.id,
            true,
            Some(temp.path()),
            None,
        )
        .await
        .expect("stamp+persist should succeed");
        assert!(
            stamped,
            "first stamp must report the false -> true transition"
        );

        let saved =
            std::fs::read_to_string(temp.path().join("sessions.json")).expect("read sessions.json");
        let rows: Vec<PersistedSession> = serde_json::from_str(&saved).expect("deserialize");
        assert_eq!(rows.len(), 1);
        assert!(
            rows[0].start_fresh_on_restore,
            "stamp must reach the snapshot"
        );

        // Second stamp is a no-op: Ok(false), and the file is not rewritten.
        std::fs::remove_file(temp.path().join("sessions.json")).expect("remove snapshot");
        let again = write_start_fresh_and_persist_to_dir_result(
            &mgr,
            session.id,
            true,
            Some(temp.path()),
            None,
        )
        .await
        .expect("idempotent stamp should succeed");
        assert!(!again, "second stamp must be Ok(false)");
        assert!(
            !temp.path().join("sessions.json").exists(),
            "a no-op stamp must not save"
        );
    }

    /// (#756) The drop persists the true -> false transition.
    #[tokio::test]
    async fn clear_start_fresh_persists_false_transition() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "powershell.exe".to_string(),
                Vec::new(),
                "C:\\tmp".to_string(),
                None,
                None,
                Vec::new(),
                true,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create_session should succeed");
        mgr.set_start_fresh_on_restore(session.id, true).await;

        let dropped = write_start_fresh_and_persist_to_dir_result(
            &mgr,
            session.id,
            false,
            Some(temp.path()),
            None,
        )
        .await
        .expect("drop+persist should succeed");
        assert!(dropped, "drop must report the true -> false transition");

        let live = mgr.list_sessions().await;
        assert!(!live[0].start_fresh_on_restore);
        let saved =
            std::fs::read_to_string(temp.path().join("sessions.json")).expect("read sessions.json");
        let rows: Vec<PersistedSession> = serde_json::from_str(&saved).expect("deserialize");
        assert_eq!(rows.len(), 1);
        assert!(
            !rows[0].start_fresh_on_restore,
            "drop must reach the snapshot"
        );
    }

    /// (#756) Dropping an unset record is Ok(false) and does not save.
    #[tokio::test]
    async fn clear_start_fresh_on_unset_record_is_noop_and_does_not_save() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "powershell.exe".to_string(),
                Vec::new(),
                "C:\\tmp".to_string(),
                None,
                None,
                Vec::new(),
                true,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create_session should succeed");

        let dropped = write_start_fresh_and_persist_to_dir_result(
            &mgr,
            session.id,
            false,
            Some(temp.path()),
            None,
        )
        .await
        .expect("no-op drop should succeed");
        assert!(!dropped, "drop on an unset record must be Ok(false)");
        assert!(
            !temp.path().join("sessions.json").exists(),
            "a no-op drop must not save"
        );
    }

    #[test]
    fn filter_sessions_for_project_paths_drops_coordinator_and_non_coordinator_orphans() {
        let project_paths = vec!["C:/projects/current".to_string()];
        let sessions = vec![
            PersistedSession {
                last_prompt: None,
                name: "kept-coordinator".into(),
                working_directory: "C:/projects/current/.ac/wg-1/__agent_tech-lead".into(),
                is_coordinator: true,
                status: Some(SessionStatus::Running),
                ..Default::default()
            },
            PersistedSession {
                last_prompt: None,
                name: "orphan-coordinator".into(),
                working_directory: "C:/projects/removed/.ac/wg-1/__agent_tech-lead".into(),
                is_coordinator: true,
                status: Some(SessionStatus::Running),
                ..Default::default()
            },
            PersistedSession {
                last_prompt: None,
                name: "orphan-member".into(),
                working_directory: "C:/projects/removed/.ac/wg-1/__agent_dev-rust".into(),
                is_coordinator: false,
                status: Some(SessionStatus::Exited(0)),
                ..Default::default()
            },
        ];

        let filtered = filter_sessions_for_project_paths(sessions, &project_paths);

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "kept-coordinator");
    }

    #[test]
    fn session_retention_project_paths_includes_archived_paths() {
        let settings = AppSettings {
            project_paths: vec!["A".to_string()],
            archived_project_paths: vec!["B".to_string()],
            ..AppSettings::default()
        };

        assert_eq!(
            session_retention_project_paths(&settings),
            vec!["A".to_string(), "B".to_string()]
        );
    }

    #[test]
    fn is_under_normalized_archived_roots_returns_false_for_empty_roots() {
        assert!(!is_under_normalized_archived_roots(
            "Z:/does/not/exist/.ac/wg-1/__agent_dev",
            &[]
        ));
    }

    #[test]
    fn is_under_normalized_archived_roots_short_circuits_before_canonicalizing_path() {
        NORMALIZE_CALLS.with(|calls| calls.set(0));

        assert!(!is_under_normalized_archived_roots(
            "Z:/does/not/exist/x",
            &[]
        ));

        assert_eq!(
            NORMALIZE_CALLS.with(|calls| calls.get()),
            0,
            "empty archived roots must not canonicalize path"
        );

        NORMALIZE_CALLS.with(|calls| calls.set(0));
        assert!(!is_under_normalized_archived_roots(
            "Z:/does/not/exist/x",
            &["z:/somewhere".to_string()]
        ));
        assert_eq!(
            NORMALIZE_CALLS.with(|calls| calls.get()),
            1,
            "non-empty roots must canonicalize path exactly once"
        );
    }

    #[test]
    fn is_under_normalized_archived_roots_matches_nested_dir() {
        let temp = tempfile::tempdir().expect("tempdir");
        let archived = temp.path().join("archived");
        let agent = archived.join(".ac").join("wg-1").join("__agent_dev");
        std::fs::create_dir_all(&agent).expect("create archived agent");
        let roots = normalize_project_roots(&[archived.to_string_lossy().to_string()]);

        assert!(is_under_normalized_archived_roots(
            &agent.to_string_lossy(),
            &roots
        ));
    }

    #[test]
    fn is_under_normalized_archived_roots_ignores_unnormalized_roots() {
        let temp = tempfile::tempdir().expect("tempdir");
        let archived = temp.path().join("archived");
        let agent = archived.join(".ac").join("wg-1").join("__agent_dev");
        std::fs::create_dir_all(&agent).expect("create archived agent");
        let raw_root = archived.join(".").to_string_lossy().to_string();

        assert!(!is_under_normalized_archived_roots(
            &agent.to_string_lossy(),
            &[raw_root]
        ));
    }

    #[test]
    fn normalize_project_roots_drops_blank_entries_and_normalizes_each() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project = temp.path().join("current");
        std::fs::create_dir_all(&project).expect("create project");
        let root_with_dot = project.join(".");
        let roots = normalize_project_roots(&[
            "".to_string(),
            "  ".to_string(),
            root_with_dot.to_string_lossy().to_string(),
        ]);

        assert_eq!(
            roots,
            vec![super::normalize_for_project_compare(project.as_path())]
        );
    }

    #[test]
    fn filter_sessions_for_project_paths_keeps_archived_when_called_with_retention_paths() {
        let temp = tempfile::tempdir().expect("tempdir");
        let active = temp.path().join("active");
        let archived = temp.path().join("archived");
        let orphan = temp.path().join("orphan");
        let active_agent = active.join(".ac").join("wg-1").join("__agent_active");
        let archived_agent = archived.join(".ac").join("wg-1").join("__agent_archived");
        let orphan_agent = orphan.join(".ac").join("wg-1").join("__agent_orphan");
        std::fs::create_dir_all(&active_agent).expect("create active agent");
        std::fs::create_dir_all(&archived_agent).expect("create archived agent");
        std::fs::create_dir_all(&orphan_agent).expect("create orphan agent");
        let retention_paths = vec![
            active.to_string_lossy().to_string(),
            archived.to_string_lossy().to_string(),
        ];
        let sessions = vec![
            PersistedSession {
                name: "active".into(),
                working_directory: active_agent.to_string_lossy().to_string(),
                ..Default::default()
            },
            PersistedSession {
                name: "archived".into(),
                working_directory: archived_agent.to_string_lossy().to_string(),
                ..Default::default()
            },
            PersistedSession {
                name: "orphan".into(),
                working_directory: orphan_agent.to_string_lossy().to_string(),
                ..Default::default()
            },
        ];

        let filtered = filter_sessions_for_project_paths(sessions, &retention_paths);

        let names: Vec<&str> = filtered
            .iter()
            .map(|session| session.name.as_str())
            .collect();
        assert_eq!(names, vec!["active", "archived"]);
    }

    #[test]
    fn filter_sessions_for_project_paths_matches_root_with_dot_segment() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project = temp.path().join("current");
        let agent = project.join(".ac").join("wg-1").join("__agent_keep");
        std::fs::create_dir_all(&agent).expect("create agent");
        let root_with_dot = project.join(".");
        let sessions = vec![PersistedSession {
            name: "keep".into(),
            working_directory: agent.to_string_lossy().to_string(),
            ..Default::default()
        }];

        let filtered = filter_sessions_for_project_paths(
            sessions,
            &[root_with_dot.to_string_lossy().to_string()],
        );

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "keep");
    }

    #[test]
    fn filter_sessions_for_normalized_roots_does_not_normalize_its_roots() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project = temp.path().join("current");
        let agent = project.join(".ac").join("wg-1").join("__agent_drop");
        std::fs::create_dir_all(&agent).expect("create agent");
        let raw_root = project.join(".").to_string_lossy().to_string();
        let sessions = vec![PersistedSession {
            name: "drop".into(),
            working_directory: agent.to_string_lossy().to_string(),
            ..Default::default()
        }];

        let filtered = filter_sessions_for_normalized_roots(sessions, &[raw_root]);

        assert!(filtered.is_empty());
    }

    #[test]
    fn filter_sessions_for_project_paths_normalizes_its_roots() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project = temp.path().join("current");
        let agent = project.join(".ac").join("wg-1").join("__agent_keep");
        std::fs::create_dir_all(&agent).expect("create agent");
        let raw_root = project.join(".").to_string_lossy().to_string();
        let sessions = vec![PersistedSession {
            name: "keep".into(),
            working_directory: agent.to_string_lossy().to_string(),
            ..Default::default()
        }];

        let filtered = filter_sessions_for_project_paths(sessions, &[raw_root]);

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "keep");
    }

    #[tokio::test]
    async fn filter_sessions_for_project_paths_blocking_runs_off_the_calling_thread() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project = temp.path().join("current");
        let agent = project.join(".ac").join("wg-1").join("__agent_keep");
        std::fs::create_dir_all(&agent).expect("create agent");
        let calling_thread = std::thread::current().id();
        let slot =
            FILTER_PROJECT_PATHS_THREAD_IDS.get_or_init(|| std::sync::Mutex::new(Vec::new()));
        slot.lock().expect("thread id mutex poisoned").clear();

        let filtered = filter_sessions_for_project_paths_blocking(
            vec![PersistedSession {
                name: "keep".into(),
                working_directory: agent.to_string_lossy().to_string(),
                ..Default::default()
            }],
            &[project.to_string_lossy().to_string()],
        )
        .await
        .expect("filter sessions");
        let recorded = slot.lock().expect("thread id mutex poisoned").clone();

        assert_eq!(filtered.len(), 1);
        assert!(!recorded.is_empty(), "filter thread id should be recorded");
        assert!(
            !recorded.contains(&calling_thread),
            "filter ran on the calling thread"
        );
    }

    #[tokio::test]
    async fn filter_sessions_for_project_paths_blocking_surfaces_join_error() {
        let result = filter_sessions_for_project_paths_blocking(
            vec![PersistedSession {
                name: "__panic_filter_for_test__".into(),
                working_directory: "C:/projects/current/.ac/wg-1/__agent_dev".into(),
                ..Default::default()
            }],
            &["C:/projects/current".to_string()],
        )
        .await;

        let Err(err) = result else {
            panic!("worker panic must surface as JoinError");
        };
        assert!(err.contains("session filter task failed"), "{err}");
    }

    #[tokio::test]
    async fn load_sessions_purging_outside_project_paths_returns_unfiltered_on_filter_error() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keep = PersistedSession {
            name: "__panic_filter_for_test__".into(),
            working_directory: "C:/projects/current/.ac/wg-1/__agent_dev".into(),
            ..Default::default()
        };
        let other = PersistedSession {
            name: "other".into(),
            working_directory: "C:/projects/other/.ac/wg-1/__agent_other".into(),
            ..Default::default()
        };
        save_sessions_to_dir(temp.path(), &[keep.clone(), other.clone()]).expect("seed sessions");

        let loaded = load_sessions_purging_outside_project_paths_in_dir(
            temp.path(),
            &["C:/projects/current".to_string()],
        )
        .await;

        let names: Vec<&str> = loaded.iter().map(|session| session.name.as_str()).collect();
        assert_eq!(names, vec![keep.name.as_str(), other.name.as_str()]);
        let saved = load_sessions_raw_from_dir_for_test(temp.path());
        assert_eq!(saved.len(), 2);
    }

    #[tokio::test]
    async fn purge_sessions_outside_project_paths_rewrites_sessions_json() {
        let temp = tempfile::tempdir().expect("tempdir");
        let current = temp.path().join("current");
        let removed = temp.path().join("removed");
        let current_agent = current.join(".ac").join("wg-1").join("__agent_keep");
        let removed_agent = removed.join(".ac").join("wg-1").join("__agent_old");
        std::fs::create_dir_all(&current_agent).expect("create current agent");
        std::fs::create_dir_all(&removed_agent).expect("create removed agent");

        let sessions = vec![
            PersistedSession {
                last_prompt: None,
                name: "keep".into(),
                working_directory: current_agent.to_string_lossy().to_string(),
                ..Default::default()
            },
            PersistedSession {
                last_prompt: None,
                name: "drop".into(),
                working_directory: removed_agent.to_string_lossy().to_string(),
                ..Default::default()
            },
        ];
        save_sessions_to_dir(temp.path(), &sessions).expect("seed sessions");

        let project_paths = vec![current.to_string_lossy().to_string()];
        let filtered = purge_sessions_outside_project_paths_in_dir(temp.path(), &project_paths)
            .await
            .expect("purge sessions");

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "keep");

        let saved =
            std::fs::read_to_string(temp.path().join("sessions.json")).expect("read sessions");
        let rows: Vec<PersistedSession> = serde_json::from_str(&saved).expect("parse sessions");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "keep");
    }

    #[tokio::test]
    async fn purge_sessions_outside_project_paths_keeps_archived_session_when_retention_paths_used()
    {
        let temp = tempfile::tempdir().expect("tempdir");
        let active = temp.path().join("active");
        let archived = temp.path().join("archived");
        let orphan = temp.path().join("orphan");
        let active_agent = active.join(".ac").join("wg-1").join("__agent_active");
        let archived_agent = archived.join(".ac").join("wg-1").join("__agent_archived");
        let orphan_agent = orphan.join(".ac").join("wg-1").join("__agent_orphan");
        std::fs::create_dir_all(&active_agent).expect("create active agent");
        std::fs::create_dir_all(&archived_agent).expect("create archived agent");
        std::fs::create_dir_all(&orphan_agent).expect("create orphan agent");
        let sessions = vec![
            PersistedSession {
                name: "active".into(),
                working_directory: active_agent.to_string_lossy().to_string(),
                ..Default::default()
            },
            PersistedSession {
                name: "archived".into(),
                working_directory: archived_agent.to_string_lossy().to_string(),
                ..Default::default()
            },
            PersistedSession {
                name: "orphan".into(),
                working_directory: orphan_agent.to_string_lossy().to_string(),
                ..Default::default()
            },
        ];
        save_sessions_to_dir(temp.path(), &sessions).expect("seed sessions");
        let retention_paths = vec![
            active.to_string_lossy().to_string(),
            archived.to_string_lossy().to_string(),
        ];

        let filtered = purge_sessions_outside_project_paths_in_dir(temp.path(), &retention_paths)
            .await
            .expect("purge sessions");

        let names: Vec<&str> = filtered
            .iter()
            .map(|session| session.name.as_str())
            .collect();
        assert_eq!(names, vec!["active", "archived"]);
        let saved =
            std::fs::read_to_string(temp.path().join("sessions.json")).expect("read sessions");
        let rows: Vec<PersistedSession> = serde_json::from_str(&saved).expect("parse sessions");
        let saved_names: Vec<&str> = rows.iter().map(|session| session.name.as_str()).collect();
        assert_eq!(saved_names, vec!["active", "archived"]);
    }

    /// #698 grinch HIGH regression — the orphan purge must take
    /// `sessions_save_lock()` and re-read `sessions.json` INSIDE that lock, so it
    /// can never overwrite a raise-hand that another locked writer persisted
    /// after the purge began. We drive the exact interleaving deterministically:
    ///   1. seed A (retained, no communication) + B (removable),
    ///   2. hold the save lock and start the purge; it must park at the lock
    ///      WITHOUT having rewritten the file (B still on disk = no stale
    ///      pre-read followed by a stale write),
    ///   3. while the lock is held, overwrite the file with A carrying a visible
    ///      raiseHand (what a raise-hand persist would have produced),
    ///   4. release the lock; the purge re-reads the fresh state, keeps A, drops
    ///      B, and the persisted raiseHand SURVIVES.
    ///
    /// The pre-fix code (load+save outside the lock) would have written its stale
    /// pre-read here, dropping A's raiseHand. Note `save_sessions_to_dir` (step 3)
    /// takes only the sync `SAVE_SESSIONS_LOCK`, never this async lock, so seeding
    /// the file while holding `guard` cannot deadlock.
    #[tokio::test]
    async fn purge_reads_under_save_lock_and_preserves_concurrent_raise_hand() {
        let temp = tempfile::tempdir().expect("tempdir");
        let current = temp.path().join("current");
        let removed = temp.path().join("removed");
        let current_agent = current.join(".ac").join("wg-1").join("__agent_keep");
        let removed_agent = removed.join(".ac").join("wg-1").join("__agent_old");
        std::fs::create_dir_all(&current_agent).expect("create current agent");
        std::fs::create_dir_all(&removed_agent).expect("create removed agent");

        let kept = PersistedSession {
            name: "keep".into(),
            working_directory: current_agent.to_string_lossy().to_string(),
            is_coordinator: true,
            status: Some(SessionStatus::Running),
            ..Default::default()
        };
        let removable = PersistedSession {
            name: "drop".into(),
            working_directory: removed_agent.to_string_lossy().to_string(),
            ..Default::default()
        };
        save_sessions_to_dir(temp.path(), &[kept.clone(), removable.clone()]).expect("seed");

        let project_paths = vec![current.to_string_lossy().to_string()];

        // Hold the save lock so the purge cannot proceed past its lock acquisition.
        let guard = sessions_save_lock().lock().await;
        let mut purge = Box::pin(purge_sessions_outside_project_paths_in_dir(
            temp.path(),
            &project_paths,
        ));
        let timed_out = tokio::time::timeout(Duration::from_millis(25), &mut purge)
            .await
            .is_err();
        assert!(
            timed_out,
            "purge must wait for the save lock before reading or writing"
        );

        // The purge has NOT rewritten the file yet (it has not even read it):
        // the removable session is still present on disk.
        let parked: Vec<PersistedSession> = serde_json::from_str(
            &std::fs::read_to_string(temp.path().join("sessions.json")).expect("read parked"),
        )
        .expect("parse parked");
        assert_eq!(
            parked.len(),
            2,
            "purge must not write while parked at the lock"
        );

        // Simulate a raise-hand that persisted A with a visible raiseHand while
        // the purge was parked.
        let mut raised = kept.clone();
        raised.communication = Some(SessionCommunication {
            kind: SessionCommunicationKind::RaiseHand,
            visible: true,
            updated_at: "2026-06-30T13:00:00+00:00".into(),
        });
        save_sessions_to_dir(temp.path(), &[raised, removable]).expect("persist raise-hand");

        drop(guard);
        let filtered = purge.await.expect("purge should succeed");

        // The purge kept A, dropped B, and preserved the raise-hand it re-read
        // under the lock.
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "keep");
        let saved: Vec<PersistedSession> = serde_json::from_str(
            &std::fs::read_to_string(temp.path().join("sessions.json")).expect("read final"),
        )
        .expect("parse final");
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].name, "keep");
        let comm = saved[0]
            .communication
            .as_ref()
            .expect("raise-hand must survive the purge");
        assert_eq!(comm.kind, SessionCommunicationKind::RaiseHand);
        assert!(comm.visible);
    }

    #[test]
    fn project_path_comparison_is_boundary_safe() {
        let project_paths = vec!["C:/repo/foo".to_string()];

        assert!(working_directory_under_any_project_path(
            "C:/repo/foo/.ac/wg-1/__agent_a",
            &project_paths
        ));
        assert!(!working_directory_under_any_project_path(
            "C:/repo/foobar/.ac/wg-1/__agent_a",
            &project_paths
        ));
    }

    #[cfg(windows)]
    #[test]
    fn project_path_comparison_is_case_insensitive_on_windows() {
        let project_paths = vec![r"C:\Users\Maria\Project".to_string()];

        assert!(working_directory_under_any_project_path(
            r"c:\users\maria\project\.ac\wg-1\__agent_a",
            &project_paths
        ));
    }

    #[cfg(windows)]
    #[test]
    fn project_path_comparison_handles_windows_verbatim_cwd() {
        let project_paths = vec![r"C:\Users\Maria\Project".to_string()];

        assert!(working_directory_under_any_project_path(
            r"\\?\C:\Users\Maria\Project\.ac\wg-1\__agent_a",
            &project_paths
        ));
    }

    #[cfg(windows)]
    #[test]
    fn deduplicate_treats_windows_verbatim_and_ordinary_cwd_as_same_key() {
        let rows = vec![
            PersistedSession {
                name: "verbatim".to_string(),
                working_directory: r"\\?\C:\repo\.ac\wg-1\__agent_a".to_string(),
                was_active: false,
                ..Default::default()
            },
            PersistedSession {
                name: "ordinary".to_string(),
                working_directory: r"C:\repo\.ac\wg-1\__agent_a".to_string(),
                was_active: true,
                ..Default::default()
            },
        ];

        let deduped = deduplicate(rows);

        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].name, "ordinary");
    }

    #[cfg(windows)]
    #[test]
    fn load_sessions_from_path_normalizes_windows_verbatim_paths() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("sessions.json");
        let sessions = serde_json::json!([
            {
                "name": "wg-1/a",
                "shell": "cmd",
                "shellArgs": [],
                "workingDirectory": r"\\?\C:\repo\.ac\wg-1\__agent_a",
                "gitRepos": [
                    {
                        "label": "repo",
                        "sourcePath": r"\\?\UNC\server\share\repo",
                        "branch": null
                    }
                ]
            }
        ]);
        std::fs::write(&path, sessions.to_string()).expect("write sessions");

        let loaded = load_sessions_from_path(&path);

        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].working_directory, r"C:\repo\.ac\wg-1\__agent_a");
        assert_eq!(loaded[0].git_repos[0].source_path, r"\\server\share\repo");
    }

    #[test]
    fn strip_auto_injected_args_removes_direct_gemini_resume_latest() {
        let stripped = strip_auto_injected_args(
            "gemini",
            &[
                "--resume".to_string(),
                "latest".to_string(),
                "-m".to_string(),
                "gpt-5".to_string(),
            ],
        );
        assert_eq!(stripped, vec!["-m".to_string(), "gpt-5".to_string()]);
    }

    #[test]
    fn strip_auto_injected_args_removes_cmd_gemini_resume_latest() {
        let stripped = strip_auto_injected_args(
            "cmd.exe",
            &[
                "/C".to_string(),
                "gemini".to_string(),
                "--resume".to_string(),
                "latest".to_string(),
                "-m".to_string(),
                "gpt-5".to_string(),
            ],
        );
        assert_eq!(
            stripped,
            vec![
                "/C".to_string(),
                "gemini".to_string(),
                "-m".to_string(),
                "gpt-5".to_string()
            ]
        );
    }

    #[test]
    fn strip_auto_injected_args_removes_embedded_cmd_gemini_resume_latest() {
        let stripped = strip_auto_injected_args(
            "cmd.exe",
            &[
                "/K".to_string(),
                "git pull && gemini --resume latest -m gpt-5".to_string(),
            ],
        );
        assert_eq!(
            stripped,
            vec!["/K".to_string(), "git pull && gemini -m gpt-5".to_string()]
        );
    }

    #[test]
    fn strip_auto_injected_args_removes_direct_claude_continue() {
        let stripped = strip_auto_injected_args(
            "claude",
            &["--continue".to_string(), "--search".to_string()],
        );
        assert_eq!(stripped, vec!["--search".to_string()]);
    }

    #[test]
    fn strip_auto_injected_args_removes_cmd_claude_continue() {
        let stripped = strip_auto_injected_args(
            "cmd.exe",
            &[
                "/C".to_string(),
                "claude".to_string(),
                "--continue".to_string(),
            ],
        );
        assert_eq!(stripped, vec!["/C".to_string(), "claude".to_string()]);
    }

    #[test]
    fn strip_auto_injected_args_removes_direct_claude_session_id() {
        // (#756) Rider belt: the launcher-minted identity pair is stripped from
        // the saved recipe (a replayed stale --session-id hard-fails the spawn).
        let stripped = strip_auto_injected_args(
            "claude",
            &[
                "--dangerously-skip-permissions".to_string(),
                "--session-id".to_string(),
                "7f9e4a10-2b3c-4d5e-8f90-1a2b3c4d5e6f".to_string(),
            ],
        );
        assert_eq!(stripped, vec!["--dangerously-skip-permissions".to_string()]);

        // The joined form is removed too.
        let joined = strip_auto_injected_args(
            "claude",
            &[
                "--session-id=7f9e4a10-2b3c-4d5e-8f90-1a2b3c4d5e6f".to_string(),
                "--search".to_string(),
            ],
        );
        assert_eq!(joined, vec!["--search".to_string()]);
    }

    #[test]
    fn strip_auto_injected_args_removes_embedded_cmd_claude_session_id() {
        // (#756) cmd-embedded tail loses BOTH injected flags (--continue and the
        // rider pair).
        let stripped = strip_auto_injected_args(
            "cmd.exe",
            &[
                "/C".to_string(),
                "npx claude --continue --session-id 7f9e4a10-2b3c-4d5e-8f90-1a2b3c4d5e6f"
                    .to_string(),
            ],
        );
        assert_eq!(stripped, vec!["/C".to_string(), "npx claude".to_string()]);
    }

    #[test]
    fn strip_auto_injected_args_preserves_session_id_without_uuid_value() {
        // (#756) Pins the UUID gate: a user-authored --session-id with a
        // non-UUID value is never eaten.
        let args = vec!["--session-id".to_string(), "not-a-uuid".to_string()];
        assert_eq!(strip_auto_injected_args("claude", &args), args);
    }

    #[test]
    fn strip_auto_injected_args_preserves_user_authored_direct_claude_prompt_file() {
        let stripped = strip_auto_injected_args(
            "claude",
            &[
                "--append-system-prompt-file".to_string(),
                "C:\\temp\\ctx.md".to_string(),
                "--search".to_string(),
            ],
        );
        assert_eq!(
            stripped,
            vec![
                "--append-system-prompt-file".to_string(),
                "C:\\temp\\ctx.md".to_string(),
                "--search".to_string()
            ]
        );
    }

    #[test]
    fn strip_auto_injected_args_preserves_embedded_claude_prompt_file_with_spaces() {
        let stripped = strip_auto_injected_args(
            "cmd.exe",
            &[
                "/K".to_string(),
                "claude --continue --append-system-prompt-file \"C:\\Program Files\\ctx.md\" --search".to_string(),
            ],
        );
        assert_eq!(
            stripped,
            vec![
                "/K".to_string(),
                "claude --append-system-prompt-file \"C:\\Program Files\\ctx.md\" --search"
                    .to_string(),
            ]
        );
    }

    #[test]
    fn strip_auto_injected_args_removes_direct_codex_resume_last() {
        let stripped = strip_auto_injected_args(
            "codex",
            &[
                "resume".to_string(),
                "--last".to_string(),
                "-m".to_string(),
                "gpt-5".to_string(),
            ],
        );
        assert_eq!(stripped, vec!["-m".to_string(), "gpt-5".to_string()]);
    }

    #[test]
    fn strip_auto_injected_args_removes_cmd_codex_resume_last() {
        let stripped = strip_auto_injected_args(
            "cmd.exe",
            &[
                "/C".to_string(),
                "codex".to_string(),
                "resume".to_string(),
                "--last".to_string(),
                "-m".to_string(),
                "gpt-5".to_string(),
            ],
        );
        assert_eq!(
            stripped,
            vec![
                "/C".to_string(),
                "codex".to_string(),
                "-m".to_string(),
                "gpt-5".to_string(),
            ]
        );
    }

    #[test]
    fn strip_auto_injected_args_removes_embedded_cmd_codex_resume_last() {
        let stripped = strip_auto_injected_args(
            "cmd.exe",
            &[
                "/K".to_string(),
                "git pull && codex resume --last -m gpt-5".to_string(),
            ],
        );
        assert_eq!(
            stripped,
            vec!["/K".to_string(), "git pull && codex -m gpt-5".to_string(),]
        );
    }

    #[test]
    fn strip_auto_injected_args_leaves_unrelated_commands_unchanged() {
        let args = vec!["-NoLogo".to_string()];
        assert_eq!(strip_auto_injected_args("powershell.exe", &args), args);
    }

    // ── Issue #186 — wrapper-basename Claude detection in the stripper ──

    #[test]
    fn strip_auto_injected_args_strips_continue_for_wrapper_basename() {
        // claude-mb invoked directly: `--continue` must be stripped from the
        // saved recipe even though the executable's stem is "claude-mb".
        let stripped = strip_auto_injected_args(
            "claude-mb",
            &[
                "--dangerously-skip-permissions".to_string(),
                "--effort".to_string(),
                "max".to_string(),
                "--continue".to_string(),
            ],
        );
        assert_eq!(
            stripped,
            vec![
                "--dangerously-skip-permissions".to_string(),
                "--effort".to_string(),
                "max".to_string(),
            ]
        );
    }

    #[test]
    fn strip_auto_injected_args_strips_continue_for_cmd_wrapped_basename() {
        // cmd.exe /K claude-mb ... --continue → strip --continue.
        let stripped = strip_auto_injected_args(
            "cmd.exe",
            &[
                "/K".to_string(),
                "claude-mb".to_string(),
                "--effort".to_string(),
                "max".to_string(),
                "--continue".to_string(),
            ],
        );
        assert_eq!(
            stripped,
            vec![
                "/K".to_string(),
                "claude-mb".to_string(),
                "--effort".to_string(),
                "max".to_string(),
            ]
        );
    }

    #[test]
    fn strip_auto_injected_args_strips_continue_for_embedded_cmd_wrapped_basename() {
        // cmd.exe /K "claude-mb --effort max --continue" → strip --continue.
        let stripped = strip_auto_injected_args(
            "cmd.exe",
            &[
                "/K".to_string(),
                "claude-mb --effort max --continue".to_string(),
            ],
        );
        assert_eq!(
            stripped,
            vec!["/K".to_string(), "claude-mb --effort max".to_string(),]
        );
    }

    /// Validation #17: single-repo legacy → one SessionRepo; legacy fields cleared.
    #[test]
    fn legacy_migration_single_repo_shape() {
        let mut ps = PersistedSession {
            last_prompt: None,
            name: "sess-a".into(),
            shell: "cmd".into(),
            shell_args: vec![],
            working_directory: "C:/x".into(),
            was_active: false,
            git_repos: vec![],
            is_coordinator: false,
            is_root_agent: false,
            agent_id: None,
            agent_label: None,
            requested_profile: None,
            telegram_bot_id: None,
            was_detached: false,
            detached_geometry: None,
            git_branch_source: Some("C:/repos/agentscommander".into()),
            git_branch_prefix: Some("agentscommander".into()),
            id: None,
            status: None,
            waiting_for_input: None,
            created_at: None,
            ..Default::default()
        };

        // Mimic the upgrade pass in load_sessions (single-repo branch).
        if ps.git_repos.is_empty() {
            match (ps.git_branch_source.take(), ps.git_branch_prefix.take()) {
                (Some(source), Some(prefix)) if prefix != "multi-repo" => {
                    ps.git_repos.push(crate::session::session::SessionRepo {
                        label: prefix,
                        source_path: source,
                        branch: None,
                        dirty: None,
                    });
                }
                _ => {}
            }
        }

        assert_eq!(ps.git_repos.len(), 1);
        assert_eq!(ps.git_repos[0].label, "agentscommander");
        assert_eq!(ps.git_repos[0].source_path, "C:/repos/agentscommander");
        assert!(ps.git_branch_source.is_none());
        assert!(ps.git_branch_prefix.is_none());
    }

    /// Issue #248 — status round-trips through serialize/deserialize.
    /// Locks the field against future "ignored on restore" misreads now that
    /// `should_wake_on_restore` in `lib.rs` consumes it.
    #[test]
    fn issue_248_status_round_trips_through_persistence() {
        let cases = [SessionStatus::Exited(0), SessionStatus::Running];
        for status in cases {
            let ps = PersistedSession {
                last_prompt: None,
                name: "coord-x".into(),
                shell: "claude".into(),
                shell_args: vec![],
                working_directory: "C:/proj/.ac/_agent_architect".into(),
                was_active: true,
                git_repos: vec![],
                is_coordinator: true,
                is_root_agent: false,
                agent_id: Some("aid-arch".into()),
                agent_label: Some("Architect".into()),
                requested_profile: None,
                telegram_bot_id: None,
                was_detached: false,
                detached_geometry: None,
                git_branch_source: None,
                git_branch_prefix: None,
                id: Some("uuid".into()),
                status: Some(status.clone()),
                waiting_for_input: Some(false),
                created_at: Some("2026-05-17T00:00:00Z".into()),
                ..Default::default()
            };
            let json = serde_json::to_string(&ps).expect("serialize");
            let back: PersistedSession = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back.status, Some(status));
        }
    }

    #[test]
    fn communication_round_trips_when_present() {
        let ps = PersistedSession {
            name: "coord-x".into(),
            shell: "claude".into(),
            shell_args: vec![],
            working_directory: "C:/proj/.ac/wg-1-dev-team/__agent_tech-lead".into(),
            communication: Some(SessionCommunication {
                kind: SessionCommunicationKind::RaiseHand,
                visible: true,
                updated_at: "2026-06-30T11:00:00+00:00".into(),
            }),
            ..Default::default()
        };

        let json = serde_json::to_value(&ps).expect("serialize");
        assert_eq!(json["communication"]["kind"], "raiseHand");
        assert_eq!(json["communication"]["visible"], true);
        let back: PersistedSession = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back.communication, ps.communication);
    }

    #[test]
    fn is_root_agent_defaults_false_for_legacy_json() {
        let json = r#"{
            "name": "legacy",
            "shell": "cmd",
            "shellArgs": [],
            "workingDirectory": "C:/x"
        }"#;

        let back: PersistedSession = serde_json::from_str(json).expect("deserialize");

        assert!(!back.is_root_agent);
    }

    #[test]
    fn is_root_agent_round_trips_true() {
        let ps = PersistedSession {
            last_prompt: None,
            name: "Root Agent".into(),
            shell: "codex".into(),
            shell_args: vec![],
            working_directory: "C:/tools/.agentscommander/ac-root-agent".into(),
            is_root_agent: true,
            ..Default::default()
        };

        let json = serde_json::to_value(&ps).expect("serialize");
        assert_eq!(json["isRootAgent"], true);
        let back: PersistedSession = serde_json::from_value(json).expect("deserialize");
        assert!(back.is_root_agent);
    }

    /// Legacy "multi-repo" prefix → git_repos stays empty; legacy fields cleared.
    #[test]
    fn legacy_migration_multi_repo_shape() {
        let mut ps = PersistedSession {
            last_prompt: None,
            name: "sess-multi".into(),
            shell: "cmd".into(),
            shell_args: vec![],
            working_directory: "C:/x".into(),
            was_active: false,
            git_repos: vec![],
            is_coordinator: false,
            is_root_agent: false,
            agent_id: None,
            agent_label: None,
            requested_profile: None,
            telegram_bot_id: None,
            was_detached: false,
            detached_geometry: None,
            git_branch_source: None,
            git_branch_prefix: Some("multi-repo".into()),
            id: None,
            status: None,
            waiting_for_input: None,
            created_at: None,
            ..Default::default()
        };

        if ps.git_repos.is_empty() {
            match (ps.git_branch_source.take(), ps.git_branch_prefix.take()) {
                (Some(source), Some(prefix)) if prefix != "multi-repo" => {
                    ps.git_repos.push(crate::session::session::SessionRepo {
                        label: prefix,
                        source_path: source,
                        branch: None,
                        dirty: None,
                    });
                }
                _ => {}
            }
        }

        assert!(ps.git_repos.is_empty());
        assert!(ps.git_branch_source.is_none());
        assert!(ps.git_branch_prefix.is_none());
    }

    /// #280 §3.1 — happy path: rename a real tmp file over a real dst.
    /// Must succeed on the first attempt (no INFO emission, no diagnostic
    /// context returned).
    #[test]
    fn rename_with_retry_succeeds_on_first_attempt() {
        let tmp = tempfile::tempdir().expect("tmp");
        let src = tmp.path().join("a.tmp");
        let dst = tmp.path().join("a");
        std::fs::write(&src, b"payload").expect("seed src");
        assert!(rename_with_retry(&src, &dst).is_ok());
        assert!(dst.exists());
        assert!(!src.exists());
    }

    /// #280 §3.1 — the retry loop exhausts all attempts and returns the
    /// diagnostic context. We force consistent failure by pointing `tmp`
    /// at a non-existent path (`NotFound` on both Windows and Unix).
    #[test]
    fn rename_with_retry_returns_diagnostics_after_exhaustion() {
        let tmp = tempfile::tempdir().expect("tmp");
        let src = tmp.path().join("does-not-exist.tmp");
        let dst = tmp.path().join("dst");
        let result = rename_with_retry(&src, &dst);
        let (msg, diag) = result.expect_err("should fail every attempt");
        assert!(!msg.is_empty(), "error message must not be empty");
        assert_eq!(diag.attempts, RENAME_ATTEMPTS);
        assert_eq!(diag.pid, std::process::id());
        assert!(!diag.tmp_exists_before);
        assert!(!diag.final_exists_before);
        // Worst-case duration is bounded by the sum of backoffs (260 ms +
        // syscall noise). Generous upper bound to keep the test stable on
        // slow CI.
        assert!(diag.duration < std::time::Duration::from_secs(2));
    }

    /// #291 — direct regression for the shared-`sessions.json.tmp` race.
    ///
    /// An `Arc<Barrier>` forces all writer threads to enter the
    /// write+rename critical section simultaneously, maximizing the
    /// chance that the old buggy code (shared temp filename, no mutex)
    /// would interleave such that one caller's rename consumed the temp
    /// file before another caller's rename could run, surfacing as
    /// Windows `ERROR_FILE_NOT_FOUND` (os error 2). With the fix
    /// (mutex + per-call unique temp filenames):
    ///   1. every concurrent save returns Ok,
    ///   2. the final file is a valid full snapshot (not torn JSON), and
    ///   3. no `.tmp` files are left behind.
    ///
    /// Without the barrier this test would have a much weaker race-
    /// detection signal: on fast SSDs each thread's write+rename can
    /// complete before the next thread is even scheduled, so the
    /// pre-fix code might pass by accident.
    #[test]
    fn save_sessions_concurrent_calls_all_succeed_with_valid_snapshot_and_no_stragglers() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let tmp = tempfile::tempdir().expect("tmp");
        let dir = Arc::new(tmp.path().to_path_buf());

        let writers = 16;
        let barrier = Arc::new(Barrier::new(writers));
        let mut handles = Vec::with_capacity(writers);
        for i in 0..writers {
            let dir = Arc::clone(&dir);
            let barrier = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                let sessions = vec![PersistedSession {
                    last_prompt: None,
                    name: format!("sess-{}", i),
                    shell: "cmd".into(),
                    shell_args: vec![],
                    working_directory: format!("C:/x/{}", i),
                    ..Default::default()
                }];
                // Synchronize: all threads enter the critical section together,
                // maximizing the chance of catching the historical race.
                barrier.wait();
                super::save_sessions_to_dir(&dir, &sessions)
            }));
        }

        // Drain all join handles before asserting so the tempdir is not
        // dropped while threads are still writing into it.
        let mut results = Vec::with_capacity(writers);
        for h in handles {
            results.push(h.join().expect("thread panicked"));
        }

        // (1) Every concurrent save returns Ok.
        for (i, r) in results.iter().enumerate() {
            assert!(r.is_ok(), "writer {} failed: {:?}", i, r.as_ref().err());
        }

        // (2) The final file is a valid full snapshot from exactly one
        //     writer (last-writer-wins under the mutex's serialization).
        let final_path = dir.join("sessions.json");
        assert!(final_path.exists(), "sessions.json must exist after writes");
        let contents = std::fs::read_to_string(&final_path).expect("read final");
        let parsed: Vec<PersistedSession> =
            serde_json::from_str(&contents).expect("final file must be valid JSON, not torn");
        assert_eq!(parsed.len(), 1, "snapshot must contain exactly one session");
        assert!(
            parsed[0].name.starts_with("sess-"),
            "final session name must be one of the writers' inputs, got '{}'",
            parsed[0].name
        );

        // (3) No stranded `.tmp` files. Each save consumed its own unique
        //     temp filename via rename; failure paths would have run the
        //     best-effort `remove_file` cleanup.
        let mut stragglers = Vec::new();
        for entry in std::fs::read_dir(dir.as_path()).expect("readdir") {
            let entry = entry.expect("entry");
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".tmp") {
                stragglers.push(name);
            }
        }
        assert!(
            stragglers.is_empty(),
            "expected no .tmp stragglers after concurrent saves, found {:?}",
            stragglers
        );
    }

    /// #291 — a single save round-trips: file lands at `sessions.json`,
    /// deserializes back to the input, and no `.tmp` file is left behind.
    #[test]
    fn save_sessions_to_dir_round_trips_and_cleans_up_temp() {
        let tmp = tempfile::tempdir().expect("tmp");
        let sessions = vec![PersistedSession {
            last_prompt: None,
            name: "solo".into(),
            shell: "claude".into(),
            shell_args: vec!["--print".into()],
            working_directory: "C:/proj".into(),
            ..Default::default()
        }];

        super::save_sessions_to_dir(tmp.path(), &sessions).expect("save");

        let path = tmp.path().join("sessions.json");
        let contents = std::fs::read_to_string(&path).expect("read");
        let back: Vec<PersistedSession> = serde_json::from_str(&contents).expect("parse");
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].name, "solo");

        for entry in std::fs::read_dir(tmp.path()).expect("readdir") {
            let name = entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned();
            assert!(
                !name.ends_with(".tmp"),
                "found leftover temp file: {}",
                name
            );
        }
    }

    #[tokio::test]
    async fn aggregate_persistence_snapshot_never_observes_half_of_removal_and_selection() {
        let manager = Arc::new(SessionManager::new());
        let first = manager
            .create_session(
                "shell-a".to_string(),
                Vec::new(),
                "C:/a".to_string(),
                None,
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        let second = manager
            .create_session(
                "shell-b".to_string(),
                Vec::new(),
                "C:/b".to_string(),
                None,
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        let first_id = first.id.to_string();
        let second_id = second.id.to_string();
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let writer_manager = Arc::clone(&manager);
        let writer_barrier = Arc::clone(&barrier);
        let writer = tokio::spawn(async move {
            writer_barrier.wait().await;
            writer_manager
                .destroy_session(second.id)
                .await
                .expect("atomic fixture destroy");
        });

        barrier.wait().await;
        for _ in 0..128 {
            let snapshot = snapshot_sessions(&manager).await;
            let active = snapshot
                .iter()
                .filter(|session| session.was_active)
                .collect::<Vec<_>>();
            assert_eq!(active.len(), 1, "snapshot must have exactly one active row");
            let ids = snapshot
                .iter()
                .filter_map(|session| session.id.as_deref())
                .collect::<Vec<_>>();
            assert!(ids.contains(&active[0].id.as_deref().unwrap()));
            let old = snapshot.len() == 2 && active[0].id.as_deref() == Some(second_id.as_str());
            let new = snapshot.len() == 1 && active[0].id.as_deref() == Some(first_id.as_str());
            assert!(old || new, "observed half-committed persistence snapshot");
            tokio::task::yield_now().await;
        }
        writer.await.unwrap();
    }
}
