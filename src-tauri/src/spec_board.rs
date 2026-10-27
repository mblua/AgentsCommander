//! Persisted spec board paths and formats; excludes window state and transport.

use std::path::{Path, PathBuf};

use crate::filesystem::spec_board as filesystem;
use types::{SnapshotFile, SpecBoardSnapshot};

pub(crate) const MAX_FILE_BYTES: u64 = 1_048_576;
const MAX_SNAPSHOTS: usize = 100;
const MAX_SNAPSHOT_TOTAL_BYTES: u64 = 25 * 1_048_576;

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

pub(crate) async fn ensure_snapshot_directory(dir: &Path) -> Result<(), String> {
    filesystem::create_spec_directory(dir)
        .await
        .map_err(|e| format!("Failed to create snapshot directory: {}", e))
}

pub(crate) async fn initialize_document_dirs(
    repo_root: &Path,
    doc_id: &str,
) -> Result<(PathBuf, PathBuf), String> {
    let (snapshot_dir, draft_dir) = internal_dirs(repo_root, doc_id).await?;
    ensure_snapshot_directory(&snapshot_dir).await?;
    filesystem::create_spec_directory(&draft_dir)
        .await
        .map_err(|e| format!("Failed to create draft directory: {}", e))?;
    Ok((snapshot_dir, draft_dir))
}

pub(crate) async fn prepare_default_spec_directory(repo: &Path) -> Result<PathBuf, String> {
    let default_dir = repo.join("specs").join("mermaids");
    filesystem::create_spec_directory(&default_dir)
        .await
        .map_err(|e| format!("Failed to create default spec directory: {}", e))?;
    Ok(default_dir)
}

pub(crate) async fn read_open_spec(path: &Path) -> Result<String, String> {
    let metadata = filesystem::spec_path_metadata(path)
        .await
        .map_err(|e| format!("Failed to inspect spec file: {}", e))?;
    if metadata.len() > MAX_FILE_BYTES {
        return Err(format!(
            "Spec file is too large. Maximum size is {} bytes.",
            MAX_FILE_BYTES
        ));
    }
    let bytes = filesystem::read_spec_bytes(path)
        .await
        .map_err(|e| format!("Failed to read spec file: {}", e))?;
    let content =
        String::from_utf8(bytes).map_err(|_| "Spec file must be valid UTF-8".to_string())?;
    Ok(content)
}

pub(crate) async fn read_saved_spec(path: &Path) -> Result<String, String> {
    let final_bytes = filesystem::read_spec_bytes(path)
        .await
        .map_err(|e| format!("Failed to read saved spec file: {}", e))?;
    String::from_utf8(final_bytes).map_err(|_| "Saved spec file is not UTF-8".to_string())
}

pub(crate) async fn read_watched_spec(path: &Path) -> std::io::Result<Vec<u8>> {
    filesystem::read_spec_bytes(path).await
}

pub(crate) async fn save_spec_content(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        filesystem::create_spec_directory(parent)
            .await
            .map_err(|e| format!("Failed to create spec directory: {}", e))?;
    }
    atomic_write_utf8(path, content).await
}

pub(crate) async fn atomic_write_utf8(path: &Path, content: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Spec path has no parent directory".to_string())?;
    let temp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("spec-board"),
        uuid::Uuid::new_v4()
    ));
    filesystem::write_spec_stage(&temp, content)
        .await
        .map_err(|e| format!("Failed to write temporary spec file: {}", e))?;
    commit_temp_file(&temp, path, "spec file").await?;
    Ok(())
}

async fn commit_temp_file(temp: &Path, target: &Path, label: &str) -> Result<(), String> {
    filesystem::commit_spec_stage(temp, target)
        .await
        .map_err(|e| format!("Failed to commit {}: {}", label, e))
}

pub(crate) async fn metadata_fingerprint(
    path: &Path,
) -> Result<(u64, Option<std::time::SystemTime>), String> {
    let meta = filesystem::spec_path_metadata(path)
        .await
        .map_err(|e| format!("Failed to inspect watched file: {}", e))?;
    Ok((meta.len(), meta.modified().ok()))
}

pub(crate) async fn write_snapshot_atomic(
    dir: &Path,
    snapshot: &SnapshotFile,
) -> Result<(), String> {
    let source = format!("{:?}", snapshot.source).to_ascii_lowercase();
    let filename = format!(
        "{}-{:06}-{}.json",
        snapshot.created_at_ms, snapshot.sequence, source
    );
    let target = dir.join(filename);
    let temp = dir.join(format!(
        ".{}.{}.tmp",
        snapshot.created_at_ms,
        uuid::Uuid::new_v4()
    ));
    let bytes = serde_json::to_vec_pretty(snapshot)
        .map_err(|e| format!("Failed to serialize snapshot: {}", e))?;
    filesystem::write_spec_stage(&temp, bytes)
        .await
        .map_err(|e| format!("Failed to write snapshot: {}", e))?;
    commit_temp_file(&temp, &target, "snapshot").await?;
    Ok(())
}

pub(crate) async fn list_snapshots_from_dir(dir: &Path) -> Result<Vec<SpecBoardSnapshot>, String> {
    let files = read_snapshot_files(dir).await?;
    Ok(files
        .into_iter()
        .map(|s| SpecBoardSnapshot {
            id: s.id,
            label: s.label,
            created_at_ms: s.created_at_ms,
            source: s.source,
            content: s.content,
            diagram_source: s.diagram_source,
            hash: s.hash,
        })
        .collect())
}

pub(crate) async fn read_snapshot_files(dir: &Path) -> Result<Vec<SnapshotFile>, String> {
    let mut files = Vec::new();
    let mut corrupt = 0usize;
    filesystem::visit_snapshot_contents(dir, |path, bytes| match bytes {
        Ok(bytes) => match serde_json::from_slice::<SnapshotFile>(&bytes) {
            Ok(snapshot) => files.push(snapshot),
            Err(e) => {
                corrupt += 1;
                log::warn!(
                    target: "agentscommander_lib::commands::spec_board::snapshots",
                    "[spec-board] skipping corrupt snapshot {}: {}",
                    path.display(),
                    e
                );
            }
        },
        Err(e) => {
            corrupt += 1;
            log::warn!(
                    target: "agentscommander_lib::commands::spec_board::snapshots",
                "[spec-board] skipping unreadable snapshot {}: {}",
                path.display(),
                e
            );
        }
    })
    .await?;
    if corrupt > 0 {
        log::warn!(
            target: "agentscommander_lib::commands::spec_board::snapshots",
            "[spec-board] skipped {} corrupt snapshot(s)",
            corrupt
        );
    }
    files.sort_by_key(|s| (s.created_at_ms, s.sequence));
    Ok(files)
}

pub(crate) async fn prune_snapshots(dir: &Path) -> Result<(), String> {
    let mut files = filesystem::snapshot_file_metadata(dir).await?;
    files.sort_by_key(|(_, _, modified)| *modified);
    let mut total: u64 = files.iter().map(|(_, len, _)| *len).sum();
    while files.len() > MAX_SNAPSHOTS || total > MAX_SNAPSHOT_TOTAL_BYTES {
        let (path, len, _) = files.remove(0);
        if let Err(e) = filesystem::remove_spec_snapshot(&path).await {
            log::warn!(
                        target: "agentscommander_lib::commands::spec_board::snapshots",
                "[spec-board] failed to prune snapshot {}: {}",
                path.display(),
                e
            );
            break;
        }
        total = total.saturating_sub(len);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
