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
