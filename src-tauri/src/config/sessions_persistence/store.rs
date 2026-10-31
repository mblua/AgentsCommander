//! Runtime IO and path owner for persisted session snapshots.

use super::record::{
    capture_binding_origins, deduplicate, normalize_persisted_session_paths, PersistedSession,
};
use crate::session::session::TEMP_SESSION_PREFIX;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

const SNAPSHOT_BASENAME: &str = "sessions.json";

pub(super) fn snapshot_path(dir: &Path) -> PathBuf {
    dir.join(SNAPSHOT_BASENAME)
}

pub(super) fn sessions_path() -> Option<PathBuf> {
    super::super::config_dir().map(|d| snapshot_path(&d))
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

#[cfg(any(test, debug_assertions))]
pub fn load_sessions_raw_from_dir_for_test(dir: &Path) -> Vec<PersistedSession> {
    load_sessions_raw_from_path(&snapshot_path(dir))
}

pub(super) fn load_sessions_raw_from_path(path: &Path) -> Vec<PersistedSession> {
    if !path.exists() {
        return vec![];
    }
    match crate::filesystem::read_session_container_text(path) {
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

#[cfg(test)]
pub(crate) fn load_sessions_from_dir(dir: &Path) -> Vec<PersistedSession> {
    load_sessions_from_path(&snapshot_path(dir))
}

pub(super) fn load_sessions_from_path(path: &Path) -> Vec<PersistedSession> {
    load_sessions_from_path_with_settings(path, None)
}

pub(super) fn load_sessions_from_path_with_settings(
    path: &Path,
    settings: Option<&super::super::settings::AppSettings>,
) -> Vec<PersistedSession> {
    if !path.exists() {
        return vec![];
    }

    match crate::filesystem::read_session_container_text(path) {
        Ok(contents) => match serde_json::from_str::<Vec<PersistedSession>>(&contents) {
            Ok(mut sessions) => {
                capture_binding_origins(path, contents.as_bytes(), &mut sessions);
                if let Some(settings) = settings {
                    for ps in &mut sessions {
                        if let Err(error) = ps.capture_binding_seed(settings) {
                            log::error!(
                                "[sessions] binding seed unavailable for '{}': {}",
                                ps.name,
                                error
                            );
                        }
                    }
                }
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

                upgrade_legacy_session_repos(&mut deduped);

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

fn upgrade_legacy_session_repos(sessions: &mut [PersistedSession]) {
    // Legacy-schema upgrade: run AFTER deduplicate() so each row's legacy
    // payload travels with its own entry. `.take()` clears the Options and
    // `skip_serializing_if` in PersistedSession elides them on next save.
    for ps in sessions.iter_mut() {
        if !ps.git_repos.is_empty() {
            // Already new-schema; drop any ghost legacy values.
            ps.git_branch_source = None;
            ps.git_branch_prefix = None;
            continue;
        }
        match (ps.git_branch_source.take(), ps.git_branch_prefix.take()) {
            (Some(source), Some(prefix)) if prefix != "multi-repo" => {
                log::info!(
                    "[sessions] Upgrading legacy single-repo session '{}' → \
                     git_repos[1]={{label:{}, source:{}}}",
                    ps.name,
                    prefix,
                    source
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
                let label = dir.strip_prefix("repo-").map(str::to_string).unwrap_or(dir);
                log::warn!(
                    "[sessions] Upgrading legacy session '{}' with source but no \
                     prefix; synthesized label '{}'",
                    ps.name,
                    label
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
                    "[sessions] Legacy multi-repo session '{}' → git_repos left empty; \
                     DiscoveryBranchWatcher will backfill",
                    ps.name
                );
            }
            (None, Some(other)) => {
                log::warn!(
                    "[sessions] Legacy session '{}' has unknown prefix '{}' without \
                     source; dropping",
                    ps.name,
                    other
                );
            }
            (None, None) => {}
            (Some(_), Some(_)) => {
                // prefix == "multi-repo" with a source — ambiguous legacy shape.
                log::warn!(
                    "[sessions] Legacy session '{}' had source + multi-repo prefix; \
                     leaving git_repos empty for discovery backfill",
                    ps.name
                );
            }
        }
    }
}

/// #280 §3.1 — diagnostic context captured when an atomic rename exhausts
/// its retry budget. Surfaced in the caller's error string so a single
/// ERROR line carries enough state to investigate the AV / Indexer / second
/// instance contention pattern.
#[derive(Debug)]
pub(crate) struct RenameDiagnostics {
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Existing cfg(test) seed error formatting reads this diagnostic field"
        )
    )]
    pub(super) op_id: u64,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Existing cfg(test) seed error formatting reads this diagnostic field"
        )
    )]
    pub(super) pid: u32,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Existing cfg(test) seed error formatting reads this diagnostic field"
        )
    )]
    pub(super) instance_id: String,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Existing cfg(test) seed error formatting reads this diagnostic field"
        )
    )]
    pub(super) tmp_exists_before: bool,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Existing cfg(test) seed error formatting reads this diagnostic field"
        )
    )]
    pub(super) final_exists_before: bool,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Existing cfg(test) seed error formatting reads this diagnostic field"
        )
    )]
    pub(super) attempts: u32,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Existing cfg(test) seed error formatting reads this diagnostic field"
        )
    )]
    pub(super) last_os_error: Option<i32>,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Existing cfg(test) seed error formatting reads this diagnostic field"
        )
    )]
    pub(super) duration: std::time::Duration,
}

/// #280 §3.1 — number of `std::fs::rename` attempts. With the
/// `BACKOFFS_MS = [10, 50, 200]` schedule below, this gives a worst-case
/// 260 ms blocking window before surfacing the error. G-MED-2 fix:
/// 4 attempts so all three backoff entries are actually used (with
/// `ATTEMPTS = 3` the 200 ms entry would be dead code).
pub(super) const RENAME_ATTEMPTS: u32 = 4;

/// #280 §3.1 — backoff schedule between rename attempts in milliseconds.
/// Tuned for Windows AV / Indexer holds which typically clear within
/// ~50 ms but occasionally take >100 ms on cold caches. The terminal
/// attempt has no backoff (we already failed `RENAME_ATTEMPTS - 1` times).
pub(super) const RENAME_BACKOFFS_MS: [u64; 3] = [10, 50, 200];

/// Borrowed process-lifetime state; only write::snapshot_store constructs this owner.
pub(super) struct SnapshotStore<'a> {
    lock: &'a Mutex<()>,
    save_op_id: &'a AtomicU64,
    rename_op_id: &'a AtomicU64,
}

pub(super) struct SnapshotLeafGuard<'a> {
    _inner: MutexGuard<'a, ()>,
}

impl<'a> SnapshotStore<'a> {
    pub(super) fn new(
        lock: &'a Mutex<()>,
        save_op_id: &'a AtomicU64,
        rename_op_id: &'a AtomicU64,
    ) -> Self {
        Self {
            lock,
            save_op_id,
            rename_op_id,
        }
    }

    #[cfg(test)]
    pub(super) fn lock_snapshot(&self) -> SnapshotLeafGuard<'a> {
        SnapshotLeafGuard {
            _inner: self.lock.lock().unwrap_or_else(|error| error.into_inner()),
        }
    }

    pub(super) fn try_lock_snapshot(&self) -> Option<SnapshotLeafGuard<'a>> {
        self.lock
            .try_lock()
            .ok()
            .map(|inner| SnapshotLeafGuard { _inner: inner })
    }

    pub(super) fn replace_prepared_once(
        &self,
        write: &PreparedSessionsWrite,
    ) -> std::io::Result<()> {
        crate::filesystem::replace_session_container_file(&write.tmp_path, &write.path)
    }

    pub(super) fn prepare(
        &self,
        dir: &Path,
        sessions: &[PersistedSession],
    ) -> Result<PreparedSessionsWrite, String> {
        crate::filesystem::ensure_session_container_directory(dir)
            .map_err(|error| format!("Failed to create config directory: {}", error))?;
        let json = serde_json::to_string_pretty(sessions)
            .map_err(|error| format!("Failed to serialize sessions: {}", error))?;
        let op_id = self.save_op_id.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let tmp_path = dir.join(format!("{SNAPSHOT_BASENAME}.{}.{}.tmp", pid, op_id));
        if let Err(error) = crate::filesystem::write_session_container_stage(&tmp_path, json) {
            let _ = crate::filesystem::remove_session_container_stage(&tmp_path);
            return Err(format!("Failed to write temp sessions file: {}", error));
        }
        let path = snapshot_path(dir);
        Ok(PreparedSessionsWrite {
            final_existed: path.exists(),
            path,
            tmp_path,
            op_id,
            pid,
            instance: crate::config::agent_local_dir_name().to_owned(),
            started: std::time::Instant::now(),
        })
    }

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
    pub(super) fn rename_with_retry(
        &self,
        tmp: &Path,
        dst: &Path,
    ) -> Result<(), (String, RenameDiagnostics)> {
        let op_id = self.rename_op_id.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let instance_id = crate::config::agent_local_dir_name();
        let start = std::time::Instant::now();
        let tmp_exists_before = tmp.exists();
        let final_exists_before = dst.exists();

        let mut last_err: Option<std::io::Error> = None;
        for attempt in 0..RENAME_ATTEMPTS {
            match crate::filesystem::replace_session_container_file(tmp, dst) {
                Ok(()) => {
                    if attempt > 0 {
                        log::info!(
                        "[sessions] rename succeeded after retry — op_id={} pid={} instance={} \
                         attempt={}/{} duration={:?}",
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
                        "[sessions] rename attempt {}/{} failed — op_id={} pid={} os_error={:?} \
                     kind={:?}",
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
}

pub(super) struct PreparedSessionsWrite {
    pub(super) path: PathBuf,
    pub(super) tmp_path: PathBuf,
    pub(super) op_id: u64,
    pub(super) pid: u32,
    pub(super) instance: String,
    pub(super) started: std::time::Instant,
    pub(super) final_existed: bool,
}

impl PreparedSessionsWrite {
    pub(super) fn terminal_error(&self, error: &std::io::Error) -> String {
        format!(
            "Failed to rename sessions file: {} [op_id={} pid={} instance={} attempts={} \
             tmp_existed_before={} final_existed_before={} os_error={:?} duration={:?}]",
            error,
            self.op_id,
            self.pid,
            self.instance,
            RENAME_ATTEMPTS,
            true,
            self.final_existed,
            error.raw_os_error(),
            self.started.elapsed(),
        )
    }
}

impl Drop for PreparedSessionsWrite {
    fn drop(&mut self) {
        // The unique temporary name belongs to this operation only. After
        // rename it is absent; cleanup never removes the published file.
        let _ = crate::filesystem::remove_session_container_stage(&self.tmp_path);
    }
}

pub(super) fn readback_snapshot(path: &Path) -> Option<Vec<PersistedSession>> {
    crate::filesystem::read_session_container_bytes(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}
