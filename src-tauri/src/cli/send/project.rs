//! Project lookup for `send`: derives the project directory from an agent root that lives
//! inside a room replica. A thin wrapper over
//! `config::ac_root::wg_replica_layout_from_agent_dir`; it owns no state and does not
//! decide routing.

use std::path::Path;

/// If `root` lives inside `<project_dir>/<Project AC Root>/wg-<N>-*/__agent_*/`,
/// return `project_dir` as a UTF-8 `String` (`None` if the shape does not match
/// or `project_dir` is not valid UTF-8, matching `list_peers::detect_wg_replica`
/// which also uses `to_str()` rather than `to_string_lossy()`).
///
/// Thin wrapper over `config::ac_root::wg_replica_layout_from_agent_dir`, the
/// single source of the WG-replica walk-up shared with
/// `list_peers::detect_wg_replica` and
/// `phone::mailbox::derive_project_from_outbox_path`, so `send` resolves WG-peer
/// targets against the same source `list-peers` reports as `reachable: true`.
pub(crate) fn derive_root_project_dir(root: &str) -> Result<Option<String>, String> {
    let Some(canon) =
        crate::filesystem::directories::canonical_cli_discovery_path(Path::new(root)).ok()
    else {
        return Ok(None);
    };
    match crate::config::ac_root::wg_replica_layout_from_agent_dir(&canon)? {
        Some(layout) => Ok(layout
            .project_dir
            .to_str()
            .map(std::string::ToString::to_string)),
        None => Ok(None),
    }
}
