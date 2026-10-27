//! Persisted spec board paths and formats; excludes window state and transport.

use std::path::{Path, PathBuf};

pub(crate) mod paths;
pub(crate) mod types;

pub(crate) async fn internal_dirs(
    repo_root: &Path,
    doc_id: &str,
) -> Result<(PathBuf, PathBuf), String> {
    let root = paths::canonicalize_existing_dir(repo_root).await?;
    let base = root.join(".ac").join("mermaids");
    Ok((
        base.join("snapshots").join(doc_id),
        base.join("drafts").join(doc_id),
    ))
}
