//! Owns the path, publication and recognition of experiment replica roles.

use std::path::{Path, PathBuf};

use super::{directories, documents};

const ROLE_FILE_NAME: &str = "Role.md";

fn role_path(replica_dir: &Path) -> PathBuf {
    replica_dir.join(ROLE_FILE_NAME)
}

pub(crate) fn write(
    replica_dir: &Path,
    bytes: &[u8],
) -> Result<(), documents::ExperimentArtifactFailure> {
    documents::replace_experiment_artifact(&role_path(replica_dir), bytes)
}

pub(crate) fn existing_in_room(room: &Path, variant_agent_name: &str) -> Option<PathBuf> {
    let replica = room.join(format!("__agent_{}", variant_agent_name));
    let role = role_path(&replica);
    if directories::experiment_path_exists(&role) {
        Some(role)
    } else {
        None
    }
}

pub(crate) fn is_replica_role_file(path: &Path, room_prefixes: &[&str]) -> bool {
    let components: Vec<String> = path
        .components()
        .filter_map(|c| c.as_os_str().to_str().map(str::to_string))
        .collect();
    components.windows(4).any(|w| {
        w[0] == ".ac"
            && room_prefixes.iter().any(|prefix| w[1].starts_with(*prefix))
            && w[2].starts_with("__agent_")
            && w[3] == ROLE_FILE_NAME
    })
}

#[cfg(test)]
mod tests;
