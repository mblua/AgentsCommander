//! Physical observations for entity discovery; excludes entity naming rules.
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

pub(crate) fn room_slot_candidates(root: &Path) -> io::Result<Vec<io::Result<OsString>>> {
    Ok(std::fs::read_dir(root)?
        .filter_map(|entry| match entry {
            Ok(entry) if entry.path().is_dir() => Some(Ok(entry.file_name())),
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
        .collect())
}

pub(crate) fn team_room_candidates(root: &Path) -> io::Result<Vec<io::Result<PathBuf>>> {
    Ok(std::fs::read_dir(root)?
        .filter_map(|entry| match entry {
            Ok(entry) if entry.path().is_dir() => Some(Ok(entry.path())),
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
        .collect())
}

pub(crate) fn replica_candidates(root: &Path) -> io::Result<Vec<io::Result<PathBuf>>> {
    Ok(std::fs::read_dir(root)?
        .filter_map(|entry| match entry {
            Ok(entry) if entry.path().is_dir() => Some(Ok(entry.path())),
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
        .collect())
}

pub(crate) fn workgroup_candidates(root: &Path) -> io::Result<Vec<io::Result<PathBuf>>> {
    Ok(std::fs::read_dir(root)?
        .filter_map(|entry| match entry {
            Ok(entry) if entry.path().is_dir() => Some(Ok(entry.path())),
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
        .collect())
}

pub(crate) fn team_directory_present(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn replica_role_present(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn clone_target_present(path: &Path) -> bool {
    path.exists()
}
