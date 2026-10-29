use super::record::PersistedSession;
use std::path::Path;

pub(super) fn strip_long_prefix_str(s: &str) -> String {
    crate::path_utils::normalize_windows_verbatim_path(s)
}

#[cfg(test)]
pub(super) static FILTER_PROJECT_PATHS_THREAD_IDS: std::sync::OnceLock<
    std::sync::Mutex<Vec<std::thread::ThreadId>>,
> = std::sync::OnceLock::new();

#[cfg(test)]
thread_local! {
    pub(super) static NORMALIZE_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn reset_normalize_call_count() {
    NORMALIZE_CALLS.with(|calls| calls.set(0));
}

#[cfg(test)]
pub(crate) fn normalize_call_count() -> usize {
    NORMALIZE_CALLS.with(|calls| calls.get())
}

pub(super) fn normalize_for_project_compare(path: &Path) -> String {
    #[cfg(test)]
    NORMALIZE_CALLS.with(|calls| calls.set(calls.get() + 1));
    let path = crate::filesystem::directories::canonical_session_project_path(path);
    let mut s = strip_long_prefix_str(&path.to_string_lossy()).replace('\\', "/");
    while s.ends_with('/') && s.len() > 1 {
        s.pop();
    }
    if cfg!(windows) {
        s.make_ascii_lowercase();
    }
    s
}

pub(super) fn path_is_under_or_equal(candidate: &str, root: &str) -> bool {
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

pub(super) fn working_directory_under_any_normalized_root(cwd: &str, roots: &[String]) -> bool {
    roots
        .iter()
        .any(|project| path_is_under_or_equal(cwd, project))
}

/// #881: paths that may retain persisted sessions. Active projects plus archived
/// projects are both registered from the session store's point of view; only
/// active projects should be used for discovery and background project work.
pub(crate) fn session_retention_project_paths(
    settings: &crate::config::settings::AppSettings,
) -> Result<Vec<String>, String> {
    if let Some(message) = &settings.project_paths_persistence_error {
        return Err(message.clone());
    }
    let mut paths = settings.project_paths.clone();
    paths.extend(settings.archived_project_paths.iter().cloned());
    Ok(paths)
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

pub(super) fn is_root_persisted_session(session: &PersistedSession) -> bool {
    session.is_root_agent
        || crate::config::root_agent::is_root_agent_dir_name(&session.working_directory)
}

/// §1295 5.6 — one orphaned session, already classified as a drop by
/// `partition_orphaned_sessions`. `cwd_exists` is filled by the caller (a
/// `Path::exists` probe, one per drop, inside the same blocking chunk) so the
/// partition itself stays pure (zero fs calls).
#[derive(Clone)]
pub(crate) struct OrphanDrop {
    pub session: PersistedSession,
    pub cwd_exists: bool,
}

/// §1295 — a single pass that partitions a session snapshot into the kept set
/// (root-agent keep + under-roots keep, exactly today's filter predicate) and
/// the orphaned drops. Pure: performs ZERO fs calls (S2/B). The `#[cfg(test)]`
/// panic hook and `FILTER_PROJECT_PATHS_THREAD_IDS` thread-id recording survive
/// here so both the keep-set filter and the persist-time prepare inherit them.
pub(crate) fn partition_orphaned_sessions(
    sessions: Vec<PersistedSession>,
    normalized_roots: &[String],
) -> (Vec<PersistedSession>, Vec<OrphanDrop>) {
    #[cfg(test)]
    if sessions
        .iter()
        .any(|session| session.name == "__panic_filter_for_test__")
    {
        panic!("test-only session filter panic");
    }

    let total = sessions.len();
    let mut kept = Vec::with_capacity(total);
    let mut drops = Vec::new();
    for session in sessions {
        if is_root_persisted_session(&session) {
            kept.push(session);
            continue;
        }
        let cwd = normalize_for_project_compare(Path::new(&session.working_directory));
        if working_directory_under_any_normalized_root(&cwd, normalized_roots) {
            kept.push(session);
        } else {
            drops.push(OrphanDrop {
                session,
                cwd_exists: false,
            });
        }
    }
    (kept, drops)
}

pub(super) fn filter_sessions_for_project_paths(
    sessions: Vec<PersistedSession>,
    project_paths: &[String],
) -> Vec<PersistedSession> {
    let roots = normalize_project_roots(project_paths);
    filter_sessions_for_normalized_roots(sessions, &roots)
}

/// §1295 — keep-set filter (unchanged behavior). It now DELEGATES to
/// `partition_orphaned_sessions`; the per-row orphan WARN (:450) and the INFO
/// summary (:461) moved to the shared dedup layer (`note_orphan_cwd` +
/// `handle_orphan_drops`), which is where drop semantics now live.
pub(super) fn filter_sessions_for_normalized_roots(
    sessions: Vec<PersistedSession>,
    roots: &[String],
) -> Vec<PersistedSession> {
    partition_orphaned_sessions(sessions, roots).0
}

#[cfg(test)]
pub(super) fn record_filter_project_paths_thread_id() {
    let slot = FILTER_PROJECT_PATHS_THREAD_IDS.get_or_init(|| std::sync::Mutex::new(Vec::new()));
    slot.lock()
        .expect("thread id mutex poisoned")
        .push(std::thread::current().id());
}

pub(super) async fn filter_sessions_for_project_paths_blocking(
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

/// Structural (non-canonicalizing) form of a raw path for archived-root membership: the
/// same separator normalization as `normalize_for_project_compare` (long-prefix strip,
/// `/` separators, trailing-slash trim, Windows case-fold) WITHOUT the `canonicalize`
/// step. The gate's archived exemption is intentionally existence-independent, so it must
/// stay deterministic whether or not the archived root (or a symlink it resolves through)
/// exists on disk.
pub(super) fn normalize_archived_root_for_compare(path: &str) -> String {
    let mut s = strip_long_prefix_str(path).replace('\\', "/");
    while s.ends_with('/') && s.len() > 1 {
        s.pop();
    }
    if cfg!(windows) {
        s.make_ascii_lowercase();
    }
    s
}

/// Structural (raw, non-canonicalizing) containment of `path` under one of `roots`,
/// returning the first matching raw root. Used ONLY by the archived-root gate exemption
/// (§1295 5.1a rule 2), where membership in the archived list is the retention signal and
/// must hold regardless of on-disk existence. Canonicalizing here would be wrong: when an
/// archived root resolves through a symlink/junction but a cwd below it does not yet exist,
/// `canonicalize(root)` resolves the symlink while `canonicalize(cwd)` falls back to the raw
/// path, so the canonical root no longer prefixes the raw cwd and the exemption would
/// falsely fail (CI-observed on Ubuntu with a symlinked temp root).
pub(super) fn archived_root_containing_raw(path: &str, roots: &[String]) -> Option<String> {
    let cwd = normalize_archived_root_for_compare(path);
    roots.iter().find_map(|raw| {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        let root = normalize_archived_root_for_compare(trimmed);
        if path_is_under_or_equal(&cwd, &root) {
            Some(raw.clone())
        } else {
            None
        }
    })
}

/// §1295 5.1a — creation-time gate predicate (pure). Rules, in order:
///   1. Root-agent path -> Ok (the directory is AC-owned).
///   2. Under an archived root -> Ok with the existence check WAIVED
///      (temp-unmount / drive-down safety; `enforce_unarchived_for_spawn`
///      auto-unarchives on an actual spawn). The membership check is structural
///      (raw-string, non-canonicalizing) so it is deterministic across platforms
///      and independent of symlink resolution or `Path::exists()`.
///   3. Outside every registered (active + archived) root -> Err with stable
///      prefix `sessionCreateBlocked:` and fragment "outside all registered
///      projects".
///   4. Registered but the directory does not exist (deleted workgroup or
///      unmounted dir) -> Err with the same prefix and fragment "does not
///      exist on disk".
///   5. Otherwise Ok.
///
/// The error strings surface verbatim through the existing String plumbing.
pub(crate) fn validate_session_creation_cwd(
    cwd: &str,
    retained_project_paths: &[String],
    archived_project_paths: &[String],
) -> Result<(), String> {
    // 1. Root-agent path is exempt (the archive-gate exceptions cover it too).
    if crate::config::root_agent::is_root_agent_path(cwd) {
        return Ok(());
    }
    // 2. Archived root (structural membership; existence deliberately waived for
    //    unmount/drive-down, so a missing cwd under an archived root is Ok).
    if archived_root_containing_raw(cwd, archived_project_paths).is_some() {
        return Ok(());
    }
    // 3. Outside every registered (active + archived) root.
    let retained_roots = normalize_project_roots(retained_project_paths);
    let cwd_norm = normalize_for_project_compare(Path::new(cwd));
    if !working_directory_under_any_normalized_root(&cwd_norm, &retained_roots) {
        return Err(format!(
            "sessionCreateBlocked: cwd '{}' is outside all registered projects",
            cwd
        ));
    }
    // 4. Registered but the directory does not exist.
    if !crate::filesystem::directories::persisted_session_cwd_exists(Path::new(cwd)) {
        return Err(format!(
            "sessionCreateBlocked: cwd '{}' does not exist on disk",
            cwd
        ));
    }
    // 5. Ok.
    Ok(())
}

/// §1295 5.5 — creation-gate enforcement rides the call stack (S3): NO process-
/// global toggle. `Skip` is constructed only by test code; production builds
/// compile only the `Enforce` path via `default_creation_gate_enforcement()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CreationGateEnforcement {
    Enforce,
    #[allow(dead_code)] // constructed only by test code (§5.5); production sees only Enforce
    Skip,
}

/// §1295 5.5 — returns `Skip` under `cfg!(test)` and `Enforce` otherwise, so
/// existing test fixtures that call the wrappers do not trip the gate while a
/// concurrent or parallel test can still opt in by calling the impl (or
/// `with_intent`) directly with `Enforce`.
pub(crate) fn default_creation_gate_enforcement() -> CreationGateEnforcement {
    if cfg!(test) {
        CreationGateEnforcement::Skip
    } else {
        CreationGateEnforcement::Enforce
    }
}

/// §1295 5.1 — run the creation gate. Reads the settings once (cloned read
/// guard), computes retained + archived roots, and runs the pure predicate in
/// `tokio::task::spawn_blocking`. `Skip` short-circuits to Ok.
pub(crate) async fn enforce_creation_gate(
    settings: &crate::config::settings::SettingsState,
    cwd: &str,
    enforcement: CreationGateEnforcement,
) -> Result<(), String> {
    match enforcement {
        CreationGateEnforcement::Skip => Ok(()),
        CreationGateEnforcement::Enforce => {
            let cfg = settings.read().await;
            let retained = session_retention_project_paths(&cfg)?;
            let archived = cfg.archived_project_paths.clone();
            drop(cfg);
            let cwd = cwd.to_string();
            tokio::task::spawn_blocking(move || {
                validate_session_creation_cwd(&cwd, &retained, &archived)
            })
            .await
            .map_err(|e| format!("session creation gate task failed: {}", e))?
        }
    }
}
