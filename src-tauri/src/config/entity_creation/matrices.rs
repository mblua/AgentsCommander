//! Matrix layouts and defaults; excludes template resolution.
use crate::filesystem::projects::entities::directories;
use std::path::Path;

pub(crate) const AGENT_MATRIX_DIRS: &[&str] = &["memory", "plans", "skills", "inbox", "outbox"];

pub(crate) const AGENT_REPLICA_DIRS: &[&str] = &["inbox", "outbox"];

pub(crate) fn create_agent_matrix_subdirs(
    agent_dir: &Path,
) -> Result<(), (&'static str, std::io::Error)> {
    for &sub in AGENT_MATRIX_DIRS {
        directories::ensure_matrix_directory(&agent_dir.join(sub)).map_err(|e| (sub, e))?;
    }
    Ok(())
}

/// Create the full Agent Matrix layout, including the root directory.
///
/// The returned tag is `"agent_dir"` for root failures or the failing subdir
/// name for child directory failures, so callers can preserve diagnostics.
pub(crate) fn create_agent_matrix_layout(
    agent_dir: &Path,
) -> Result<(), (&'static str, std::io::Error)> {
    directories::ensure_matrix_directory(agent_dir).map_err(|e| ("agent_dir", e))?;
    create_agent_matrix_subdirs(agent_dir)
}

/// Create a new Agent Matrix layout after atomically claiming the root.
///
/// Unlike `create_agent_matrix_layout`, this fails if the root already exists.
/// Use it for user-facing creation paths so concurrent creates cannot overwrite
/// each other's Role.md or config.json.
pub(crate) fn create_new_agent_matrix_layout(
    agent_dir: &Path,
) -> Result<(), (&'static str, std::io::Error)> {
    directories::claim_matrix_directory(agent_dir).map_err(|e| ("agent_dir", e))?;
    create_agent_matrix_subdirs(agent_dir)
}

/// Create the full workgroup replica layout, including the root directory.
///
/// Canonical state is intentionally absent here; replicas reference the origin
/// Agent Matrix through `config.json`.
pub(crate) fn create_agent_replica_layout(
    replica_dir: &Path,
) -> Result<(), (&'static str, std::io::Error)> {
    directories::ensure_matrix_directory(replica_dir).map_err(|e| ("replica_dir", e))?;
    for &sub in AGENT_REPLICA_DIRS {
        directories::ensure_matrix_directory(&replica_dir.join(sub)).map_err(|e| (sub, e))?;
    }
    Ok(())
}

pub(crate) fn default_agent_matrix_config() -> serde_json::Value {
    serde_json::json!({
        "tooling": {},
        "context": ["$AGENTSCOMMANDER_CONTEXT", super::roles::ROLE_FILE_NAME],
    })
}

pub(crate) fn matrix_directory_present(path: &Path) -> bool {
    directories::matrix_directory_present(path)
}

pub(crate) fn write_copy_default_config<W>(write: W) -> Result<(), String>
where
    W: FnOnce(&Path, &[u8]) -> Result<(), String>,
{
    let bytes =
        serde_json::to_vec_pretty(&default_agent_matrix_config()).map_err(|e| e.to_string())?;
    write(Path::new("config.json"), &bytes)
}
