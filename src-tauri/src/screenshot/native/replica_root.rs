//! Replica-root resolution and symlink guard.

use std::path::{Path, PathBuf};

// ── Replica-root resolution + symlink guard ────────────────────────────────

/// True iff `meta` is a symlink or (Windows) any reparse point (junction).
/// Inlined from the private `session_context::is_link_or_reparse` per its own
/// doc-comment, which asks callers to inline rather than widen its API.
pub(super) fn is_link_or_reparse(meta: &std::fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    // Junctions and reparse points do not exist outside Windows, so the
    // `is_symlink()` check above is the complete Linux answer. `meta` stays used
    // on every target, so no `unused` lint fires.
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

/// Walk up `start` to the owning `wg-*/__agent_*` replica directory by name.
/// Pure path logic (no filesystem access): a session launched from
/// `__agent_x\child` resolves to `__agent_x`. Returns `None` for ad-hoc CWDs,
/// `repo-*` siblings, project roots, and root-agent paths.
pub(super) fn find_replica_root(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start);
    while let Some(dir) = current {
        if let Some(name) = dir.file_name().and_then(|n| n.to_str()) {
            if name.starts_with("__agent_") {
                let parent_is_wg = dir
                    .parent()
                    .and_then(|p| p.file_name())
                    .and_then(|n| n.to_str())
                    .map(crate::config::entity_prefix::has_entity_prefix)
                    .unwrap_or(false);
                if parent_is_wg {
                    return Some(dir.to_path_buf());
                }
            }
        }
        current = dir.parent();
    }
    None
}

/// Resolve and validate the only legal write target for a capture: the canonical
/// owning WG replica root of the active session's CWD. Rejects non-replica CWDs,
/// symlinks/reparse points, and roots that fail WG replica identity validation.
pub(super) fn resolve_session_replica_root(working_directory: &str) -> Result<PathBuf, String> {
    // Canonicalize the session CWD BEFORE walking ancestors so `..` and symlink
    // components cannot spoof a `__agent_*` ancestor. Example spoof:
    // `...\wg-6\__agent_x\..\repo-AgentsCommander` resolves to the repo sibling
    // (which must be rejected), yet a raw name-walk would see the `__agent_x`
    // ancestor and save there. Canonicalize requires the path to exist, which is
    // always true for a live session's CWD; a missing/unresolvable CWD is rejected.
    let canonical_cwd = std::fs::canonicalize(working_directory)
        .map_err(|e| format!("Cannot resolve the active session directory: {e}"))?;
    let root = find_replica_root(&canonical_cwd).ok_or_else(|| {
        "The active session is not inside a room replica directory, so there is no place to \
            save the screenshot"
            .to_string()
    })?;

    let meta = std::fs::symlink_metadata(&root)
        .map_err(|e| format!("Cannot read the replica directory: {e}"))?;
    if is_link_or_reparse(&meta) {
        return Err("The replica directory is a symlink or reparse point".to_string());
    }
    if !meta.is_dir() {
        return Err("The replica path is not a directory".to_string());
    }

    let canonical = std::fs::canonicalize(&root)
        .map_err(|e| format!("Cannot canonicalize the replica directory: {e}"))?;

    // Structure + persisted-identity validation against the existing helpers.
    crate::config::replica_identity::expected_wg_replica_identity(&canonical)?;
    crate::config::replica_identity::read_wg_replica_config_read_only(&canonical)?;
    Ok(canonical)
}

/// Re-run the FULL start-time validation on the target immediately before creating
/// the PNG, because the directory could have changed since capture start. This is
/// the symlink/reparse + directory guard, plus a re-canonicalization and a re-run
/// of the WG replica structure + persisted-identity checks. It closes the window
/// where the replica root is deleted and recreated as a plain, non-replica
/// directory between capture and confirm (which the final parent-containment check
/// alone would still accept). `target` is the canonical root resolved at start.
pub(super) fn revalidate_replica_target(target: &Path) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(target)
        .map_err(|e| format!("Cannot read the replica directory: {e}"))?;
    if is_link_or_reparse(&meta) {
        return Err("The replica directory is a symlink or reparse point".to_string());
    }
    if !meta.is_dir() {
        return Err("The replica path is not a directory".to_string());
    }

    // `target` was already canonical at start; if re-canonicalizing yields a
    // different path the directory was replaced under us, so reject.
    let canonical = std::fs::canonicalize(target)
        .map_err(|e| format!("Cannot canonicalize the replica directory: {e}"))?;
    if canonical.as_path() != target {
        return Err("The replica directory changed between capture and save".to_string());
    }

    // Same structure + persisted-identity validation used at capture start.
    crate::config::replica_identity::expected_wg_replica_identity(&canonical)?;
    crate::config::replica_identity::read_wg_replica_config_read_only(&canonical)?;
    Ok(())
}
