use std::path::{Path, PathBuf};

use crate::filesystem::config_locations;

pub const CANONICAL_AC_ROOT_DIR: &str = ".ac";
pub const AC_ROOT_DIR_NAMES: [&str; 1] = [CANONICAL_AC_ROOT_DIR];

pub fn canonical_ac_root_label() -> &'static str {
    CANONICAL_AC_ROOT_DIR
}

pub fn ac_root_for_project(project: &Path) -> PathBuf {
    project.join(CANONICAL_AC_ROOT_DIR)
}

pub fn existing_ac_root(project: &Path) -> Option<PathBuf> {
    let canonical = ac_root_for_project(project);
    if config_locations::ac_root_is_directory(&canonical) {
        return Some(canonical);
    }

    None
}

pub fn has_ac_root(project: &Path) -> bool {
    existing_ac_root(project).is_some()
}

pub fn is_ac_root_name(name: &str) -> bool {
    AC_ROOT_DIR_NAMES
        .iter()
        .any(|candidate| name.eq_ignore_ascii_case(candidate))
}

pub fn find_ac_root_segment(parts: &[&str]) -> Option<usize> {
    parts.iter().rposition(|part| is_ac_root_name(part))
}

pub fn find_ac_root_ancestor(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|ancestor| {
            ancestor
                .file_name()
                .and_then(|name| name.to_str())
                .map(is_ac_root_name)
                .unwrap_or(false)
        })
        .map(Path::to_path_buf)
}

fn same_existing_path(a: &Path, b: &Path) -> bool {
    match (
        config_locations::canonical_ac_root(a),
        config_locations::canonical_ac_root(b),
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

pub fn ensure_authoritative_ac_root(ac_root: &Path) -> Result<(), String> {
    let ac_root_name = ac_root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            format!(
                "Project AC Root path '{}' has no valid directory name",
                ac_root.display()
            )
        })?;
    if !is_ac_root_name(ac_root_name) {
        return Err(format!(
            "Project AC Root path '{}' is not a Project AC Root directory",
            ac_root.display()
        ));
    }

    let project_dir = ac_root.parent().ok_or_else(|| {
        format!(
            "Project AC Root path '{}' has no parent project directory",
            ac_root.display()
        )
    })?;
    let Some(authoritative) = existing_ac_root(project_dir) else {
        return Err(format!(
            "project '{}' has no Project AC Root directory",
            project_dir.display()
        ));
    };

    if same_existing_path(ac_root, &authoritative) {
        Ok(())
    } else {
        Err(format!(
            "Project AC Root '{}' rejected because authoritative Project AC Root '{}' exists",
            ac_root.display(),
            authoritative.display()
        ))
    }
}

/// The WG-replica path layout `<project_dir>/<workspace>/wg-*/__agent_*`, as
/// resolved by [`wg_replica_layout_from_agent_dir`]. Absolute paths, except the
/// two names, which the shape checks already proved are valid UTF-8.
pub struct WgReplicaLayout {
    /// Agent name with the `__agent_` prefix stripped.
    pub agent_name: String,
    /// The `wg-<N>-*` directory name.
    pub wg_name: String,
    /// Absolute path to the `wg-<N>-*` directory.
    pub wg_dir: PathBuf,
    /// Absolute path to the Project AC Root directory.
    pub ac_root: PathBuf,
    /// Absolute path to the project directory (parent of the Project AC Root).
    pub project_dir: PathBuf,
}

/// Given an already-canonicalized agent replica directory (the `__agent_*`
/// folder), walk up the WG-replica layout `__agent_* -> wg-* -> <workspace> ->
/// <project>` and return its pieces.
///
/// Returns `Ok(None)` when `agent_dir` is not a WG-replica shape (wrong name
/// prefix, missing ancestor, or non-authoritative workspace name), and `Err`
/// only when the workspace fails [`ensure_authoritative_ac_root`].
///
/// Single source for the walk-up shared by `cli::send::derive_root_project_dir`,
/// `cli::list_peers::detect_wg_replica`, and
/// `phone::mailbox::derive_project_from_outbox_path`. Each caller canonicalizes
/// its own input (an agent root, or an outbox file walked up to the agent dir)
/// and applies its own final projection (full project path, project name, or the
/// richer struct); the shared shape checks live here so the three cannot drift.
pub fn wg_replica_layout_from_agent_dir(
    agent_dir: &Path,
) -> Result<Option<WgReplicaLayout>, String> {
    let Some(agent_name) = agent_dir
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("__agent_"))
    else {
        return Ok(None);
    };
    let agent_name = agent_name.to_string();

    let Some(wg_dir) = agent_dir.parent() else {
        return Ok(None);
    };
    let Some(wg_name) = wg_dir.file_name().and_then(|name| name.to_str()) else {
        return Ok(None);
    };
    if !crate::config::entity_prefix::has_entity_prefix(wg_name) {
        return Ok(None);
    }

    let Some(ac_root) = wg_dir.parent() else {
        return Ok(None);
    };
    if !ac_root
        .file_name()
        .and_then(|name| name.to_str())
        .map(is_ac_root_name)
        .unwrap_or(false)
    {
        return Ok(None);
    }
    ensure_authoritative_ac_root(ac_root)?;

    let Some(project_dir) = ac_root.parent() else {
        return Ok(None);
    };

    Ok(Some(WgReplicaLayout {
        agent_name,
        wg_name: wg_name.to_string(),
        wg_dir: wg_dir.to_path_buf(),
        ac_root: ac_root.to_path_buf(),
        project_dir: project_dir.to_path_buf(),
    }))
}

#[cfg(test)]
mod tests;
