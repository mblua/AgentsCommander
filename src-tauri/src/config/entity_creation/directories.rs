//! Entity directory rules; delegates physical traversal and deletion.
#[cfg(test)]
pub(crate) use crate::filesystem::projects::entities::deletion::is_file_in_use_error;
#[cfg(test)]
pub(crate) use crate::filesystem::projects::entities::deletion::metadata_parts_are_link_or_reparse;
#[cfg(test)]
pub(crate) use crate::filesystem::projects::entities::deletion::FILE_ATTRIBUTE_REPARSE_POINT;
pub(crate) use crate::filesystem::projects::entities::deletion::{
    canonical_delete_path_or_original, inspect_team_for_delete, is_rename_blocked_by_handle,
    metadata_is_link_or_reparse, remove_replica_directory, remove_room_orphan,
    remove_staged_agent_target, remove_team_directory, rename_room_for_delete,
    restore_agent_target, stage_agent_target, validate_delete_root, ReplicaRemovalOutcome,
    WgDeleteOutcome,
};

use super::names::validate_existing_name;
use crate::filesystem::projects::entities::directories as disk;
use std::collections::HashSet;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

pub(crate) fn strict_team_candidates(root: &Path) -> Result<Vec<PathBuf>, String> {
    strict_teams_from_observations(disk::strict_team_candidates(root))
}

pub(crate) fn strict_teams_from_observations(
    entries: io::Result<impl IntoIterator<Item = io::Result<PathBuf>>>,
) -> Result<Vec<PathBuf>, String> {
    let entries = entries.map_err(|e| {
        format!(
            "Cannot read Project AC Root directory for agent delete: {}",
            e
        )
    })?;
    let mut paths = Vec::new();
    for entry in entries {
        let path = entry.map_err(|e| {
            format!(
                "Cannot read directory entry during agent delete scan: {}",
                e
            )
        })?;
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("_team_"))
        {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

pub(crate) fn inspect_agent_replica(path: &Path) -> io::Result<()> {
    disk::inspect_agent_replica(path)
}

pub(crate) fn room_slot_candidates(root: &Path) -> Vec<u32> {
    room_slots_from_observations(disk::room_slot_candidates(root).unwrap_or_default())
}

fn room_slots_from_observations(
    entries: impl IntoIterator<Item = io::Result<OsString>>,
) -> Vec<u32> {
    entries
        .into_iter()
        .flatten()
        .filter_map(|name| {
            let name = name.to_string_lossy();
            let rest = name.strip_prefix(crate::config::entity_prefix::ROOM_DIR_PREFIX)?;
            let (middle, team) = rest.split_once('-')?;
            if team.is_empty() {
                return None;
            }
            middle.parse::<u32>().ok()
        })
        .collect()
}

pub(crate) fn determine_next_wg_number(root: &Path) -> u32 {
    let taken: HashSet<u32> = room_slot_candidates(root).into_iter().collect();
    (1u32..=u32::MAX).find(|n| !taken.contains(n)).unwrap_or(1)
}

pub(crate) fn team_room_candidates(root: &Path, team_name: &str) -> Vec<PathBuf> {
    team_rooms_from_observations(
        disk::team_room_candidates(root).unwrap_or_default(),
        team_name,
    )
}

fn team_rooms_from_observations(
    entries: impl IntoIterator<Item = io::Result<PathBuf>>,
    team_name: &str,
) -> Vec<PathBuf> {
    let suffix = format!("-{}", team_name);
    entries
        .into_iter()
        .flatten()
        .filter(|path| {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            crate::config::entity_prefix::strip_entity_prefix(&name)
                .and_then(|rest| rest.strip_suffix(&suffix))
                .is_some_and(|middle| middle.parse::<u32>().is_ok())
        })
        .collect()
}

pub(crate) fn replica_candidates(root: &Path) -> io::Result<Vec<PathBuf>> {
    Ok(replicas_from_observations(disk::replica_candidates(root)?))
}

fn replicas_from_observations(
    entries: impl IntoIterator<Item = io::Result<PathBuf>>,
) -> Vec<PathBuf> {
    entries
        .into_iter()
        .flatten()
        .filter(|path| {
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .starts_with("__agent_")
        })
        .collect()
}

pub(crate) fn workgroup_candidates(root: &Path) -> Vec<PathBuf> {
    workgroups_from_observations(disk::workgroup_candidates(root).unwrap_or_default())
}

fn workgroups_from_observations(
    entries: impl IntoIterator<Item = io::Result<PathBuf>>,
) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = entries
        .into_iter()
        .flatten()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| parse_team_from_workgroup_name(name).is_ok())
        })
        .collect();
    dirs.sort_by(|a, b| {
        a.file_name()
            .unwrap_or_default()
            .cmp(b.file_name().unwrap_or_default())
    });
    dirs
}

pub(crate) fn parse_team_from_workgroup_name(workgroup_name: &str) -> Result<String, String> {
    let rest = crate::config::entity_prefix::strip_entity_prefix(workgroup_name)
        .ok_or_else(|| format!("Invalid room name '{}'", workgroup_name))?;
    let Some((number, team)) = rest.split_once('-') else {
        return Err(format!("Invalid room name '{}'", workgroup_name));
    };
    let parsed = number
        .parse::<u32>()
        .map_err(|_| format!("Invalid room number in '{}'", workgroup_name))?;
    if parsed == 0 || team.is_empty() {
        return Err(format!("Invalid room name '{}'", workgroup_name));
    }
    validate_existing_name(team, "Team")?;
    Ok(team.to_string())
}

pub(crate) fn team_directory_present(path: &Path) -> bool {
    disk::team_directory_present(path)
}

pub(crate) fn replica_role_present(path: &Path) -> bool {
    disk::replica_role_present(path)
}

pub(crate) fn clone_target_present(path: &Path) -> bool {
    disk::clone_target_present(path)
}

pub(crate) fn agent_matrix_candidates(root: &Path) -> impl Iterator<Item = PathBuf> {
    disk::agent_matrix_candidates(root)
        .into_iter()
        .flatten()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("_agent_"))
        })
}

pub(crate) fn agent_matrix_role(matrix: &Path) -> Option<String> {
    let role = matrix.join("Role.md");
    if disk::agent_matrix_role_present(&role) {
        crate::filesystem::documents::read_repository_role(&role).ok()
    } else {
        None
    }
}

#[cfg(test)]
mod tests;
