//! Filesystem locations that AgentsCommander creates and shares between agents:
//! four project-level directories under a project's `.ac` root, and one room-level
//! directory under a room root. #1795.
//!
//! Leaf module by design: it depends on `std` only. `config::session_context`,
//! `config::seeded_context_templates` and `commands::entity_creation` all call into
//! it, and because it calls nothing back it can never join or grow a dependency
//! cycle. See section 9 of .ac/plans/1795-golden-rule-shared-locations.md.

use std::path::Path;

use crate::filesystem::config_locations;

/// Shared directories created directly under a project's `.ac` root. Every agent in
/// the project may read and write inside them. The order here is the render order of
/// Golden Rule entry 5.
pub(crate) const PROJECT_SHARED_DIRS: &[&str] = &["plans", "tools", "errors", "project-shared"];

/// Shared directory created directly under a room root. Every agent in that room may
/// read and write inside it.
pub(crate) const PROJECT_SKILLS_DIR: &str = "project-skills";

/// Inspect before traversal; occupied paths are never repaired or replaced.
pub(crate) fn ensure_project_skills_dir(ac_root: &Path) -> std::io::Result<()> {
    ensure_skills_leaf(ac_root, PROJECT_SKILLS_DIR)
}

pub(crate) fn ensure_team_skills_dir(team_root: &Path) -> std::io::Result<()> {
    inspect_ordinary_directory(team_root)?;
    ensure_skills_leaf(team_root, "team-skills")
}

fn inspect_ordinary_directory(path: &Path) -> std::io::Result<()> {
    let metadata = config_locations::shared_directory_metadata(path)?;
    let linked = metadata.file_type().is_symlink();
    #[cfg(windows)]
    let linked = {
        use std::os::windows::fs::MetadataExt;
        linked || metadata.file_attributes() & 0x400 != 0
    };
    if linked {
        return Err(std::io::Error::other(
            "linked/reparse directory is not allowed",
        ));
    }
    if !metadata.is_dir() {
        return Err(std::io::Error::other("not an ordinary directory"));
    }
    Ok(())
}

fn ensure_skills_leaf(parent: &Path, name: &str) -> std::io::Result<()> {
    let path = parent.join(name);
    match inspect_ordinary_directory(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match config_locations::create_shared_directory(&path) {
                Ok(()) => inspect_ordinary_directory(&path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    inspect_ordinary_directory(&path)
                }
                Err(error) => Err(error),
            }
        }
        Err(error) => Err(error),
    }
}

pub(crate) const ROOM_SHARED_DIR: &str = "room-shared";

pub(crate) fn create_project_shared_dirs(ac_root: &Path) -> std::io::Result<()> {
    for sub in PROJECT_SHARED_DIRS {
        config_locations::ensure_shared_directories(&ac_root.join(sub))?;
    }
    Ok(())
}

pub(crate) fn create_room_shared_dir(room_root: &Path) -> std::io::Result<()> {
    config_locations::ensure_shared_directories(&room_root.join(ROOM_SHARED_DIR))
}

#[cfg(test)]
mod tests;
