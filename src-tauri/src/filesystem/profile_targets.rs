//! Physical profile replica directory discovery.

use std::path::{Path, PathBuf};

pub(crate) enum ProfileDirectoryEvent {
    Candidate(PathBuf),
    Fault { warning: String, scope: PathBuf },
}

pub(crate) fn visit_room_replicas(wg_dir: &Path, mut visit: impl FnMut(ProfileDirectoryEvent)) {
    let entries = match std::fs::read_dir(wg_dir) {
        Ok(entries) => entries,
        Err(e) => {
            visit(ProfileDirectoryEvent::Fault {
                warning: format!("Failed to read room '{}': {}", wg_dir.display(), e),
                scope: wg_dir.to_path_buf(),
            });
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                visit(ProfileDirectoryEvent::Fault {
                    warning: format!(
                        "Failed to read an entry in room '{}': {}",
                        wg_dir.display(),
                        e
                    ),
                    scope: wg_dir.to_path_buf(),
                });
                continue;
            }
        };
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(e) => {
                visit(ProfileDirectoryEvent::Fault {
                    warning: format!(
                        "Failed to inspect an entry in room '{}': {}",
                        wg_dir.display(),
                        e
                    ),
                    scope: wg_dir.to_path_buf(),
                });
                continue;
            }
        };
        if !file_type.is_dir() {
            continue;
        }
        let name = entry.file_name();
        if name.to_string_lossy().starts_with("__agent_") {
            visit(ProfileDirectoryEvent::Candidate(entry.path()));
        }
    }
}

pub(crate) fn visit_kind_replicas(
    ac_root: &Path,
    is_room: impl Fn(&str) -> bool,
    mut visit: impl FnMut(ProfileDirectoryEvent),
) {
    let entries = match std::fs::read_dir(ac_root) {
        Ok(entries) => entries,
        Err(e) => {
            visit(ProfileDirectoryEvent::Fault {
                warning: format!("Failed to read workspace '{}': {}", ac_root.display(), e),
                scope: ac_root.to_path_buf(),
            });
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                visit(ProfileDirectoryEvent::Fault {
                    warning: format!(
                        "Failed to read an entry in workspace '{}': {}",
                        ac_root.display(),
                        e
                    ),
                    scope: ac_root.to_path_buf(),
                });
                continue;
            }
        };
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(e) => {
                visit(ProfileDirectoryEvent::Fault {
                    warning: format!(
                        "Failed to inspect an entry in workspace '{}': {}",
                        ac_root.display(),
                        e
                    ),
                    scope: ac_root.to_path_buf(),
                });
                continue;
            }
        };
        if !file_type.is_dir() || !is_room(&entry.file_name().to_string_lossy()) {
            continue;
        }
        visit_room_replicas(&entry.path(), &mut visit);
    }
}
