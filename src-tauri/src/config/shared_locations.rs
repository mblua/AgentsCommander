//! Filesystem locations that AgentsCommander creates and shares between agents:
//! four project-level directories under a project's `.ac` root, and one room-level
//! directory under a room root. #1795.
//!
//! Leaf module by design: it depends on `std` only. `config::session_context`,
//! `config::seeded_context_templates` and `commands::entity_creation` all call into
//! it, and because it calls nothing back it can never join or grow a dependency
//! cycle. See section 9 of .ac/plans/1795-golden-rule-shared-locations.md.

use std::path::Path;

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
    let metadata = std::fs::symlink_metadata(path)?;
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
            match std::fs::create_dir(&path) {
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
        std::fs::create_dir_all(ac_root.join(sub))?;
    }
    Ok(())
}

pub(crate) fn create_room_shared_dir(room_root: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(room_root.join(ROOM_SHARED_DIR))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_2868_team_leaf_requires_existing_ordinary_parent() {
        let temp = tempfile::tempdir().unwrap();
        let team = temp.path().join("_team_example");
        assert!(ensure_team_skills_dir(&team).is_err());
        assert!(!team.exists());
        std::fs::write(&team, "KEEP_PARENT").unwrap();
        assert!(ensure_team_skills_dir(&team).is_err());
        assert_eq!(std::fs::read_to_string(&team).unwrap(), "KEEP_PARENT");
        std::fs::remove_file(&team).unwrap();
        std::fs::create_dir(&team).unwrap();
        ensure_team_skills_dir(&team).unwrap();
        ensure_team_skills_dir(&team).unwrap();
        assert!(team.join("team-skills").is_dir());
        assert!(!team.join("config.json").exists());
    }

    #[test]
    fn issue_2868_project_skills_missing_ordinary_and_file_are_not_clobbered() {
        let temp = tempfile::tempdir().unwrap();
        let ac = temp.path().join(".ac");
        assert!(ensure_project_skills_dir(&ac).is_err());
        assert!(!ac.exists(), "helper must not create ancestors");
        std::fs::create_dir(&ac).unwrap();
        ensure_project_skills_dir(&ac).unwrap();
        ensure_project_skills_dir(&ac).unwrap();
        let source = ac.join("project-skills");
        assert!(source.is_dir());
        std::fs::remove_dir(&source).unwrap();
        std::fs::write(&source, "OCCUPIED").unwrap();
        assert_eq!(
            ensure_project_skills_dir(&ac).unwrap_err().to_string(),
            "not an ordinary directory"
        );
        assert_eq!(std::fs::read_to_string(&source).unwrap(), "OCCUPIED");
    }

    /// #1795 `T5`. The four names are asserted as LITERALS and the constant's
    /// length is asserted separately. Iterating `PROJECT_SHARED_DIRS` to build the
    /// expectation would make the oracle move with the constant, so control `C5`
    /// (`"errors"` -> `"error"`) would leave this test green.
    #[test]
    fn create_project_shared_dirs_creates_all_four_and_is_idempotent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");

        assert_eq!(PROJECT_SHARED_DIRS.len(), 4);

        for run in 1..=2 {
            create_project_shared_dirs(&ac_root).expect("create project shared dirs");
            for name in ["plans", "tools", "errors", "project-shared"] {
                let path = ac_root.join(name);
                assert!(path.is_dir(), "run {run}: `{name}` must be a directory");
            }
        }
    }

    /// #1795 `T6a`. Same shape as `T5`, on the single room-level name.
    #[test]
    fn create_room_shared_dir_creates_and_is_idempotent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let room_root = temp.path().join("room-19-dev-team");

        for run in 1..=2 {
            create_room_shared_dir(&room_root).expect("create room shared dir");
            let path = room_root.join("room-shared");
            assert!(
                path.is_dir(),
                "run {run}: `room-shared` must be a directory"
            );
        }
    }
}
