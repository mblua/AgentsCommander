//! Runtime reader and path owner for persisted session snapshots.

use super::record::{
    capture_binding_origins, deduplicate, normalize_persisted_session_paths, PersistedSession,
};
use crate::session::session::TEMP_SESSION_PREFIX;
use std::path::{Path, PathBuf};

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
                            let label =
                                dir.strip_prefix("repo-").map(str::to_string).unwrap_or(dir);
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
