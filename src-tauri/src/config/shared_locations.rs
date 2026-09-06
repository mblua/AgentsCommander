//! Filesystem locations that AgentsCommander creates and shares between agents:
//! four project-level directories under a project's `.ac` root, and one room-level
//! directory under a room root. #1795.
//!
//! Leaf module by design: it depends on `std` only. `config::session_context`,
//! `config::seeded_context_templates` and `commands::entity_creation` all call into
//! it, and because it calls nothing back it can never join or grow a dependency
//! cycle. See section 9 of plans/1795-golden-rule-shared-locations.md.

use std::path::Path;

/// Shared directories created directly under a project's `.ac` root. Every agent in
/// the project may read and write inside them. The order here is the render order of
/// Golden Rule entry 5.
pub(crate) const PROJECT_SHARED_DIRS: &[&str] = &["plans", "tools", "errors", "project-shared"];

/// Shared directory created directly under a room root. Every agent in that room may
/// read and write inside it.
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
