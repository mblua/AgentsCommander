use std::path::Path;

use super::{
    commit_temp_file, hash_prefix, hash_str, now_ms, SnapshotFile, SpecBoardSnapshot,
    SpecBoardSnapshotSource, MAX_SNAPSHOTS, MAX_SNAPSHOT_CONTENT_BYTES, MAX_SNAPSHOT_TOTAL_BYTES,
};
use crate::SpecBoardState;

pub(super) async fn create_snapshot_for_doc(
    state: SpecBoardState,
    doc_id: &str,
    source: SpecBoardSnapshotSource,
) -> Result<(), String> {
    let (snapshot_dir, snapshot, skip_for_size) = {
        let mut mgr = state.write().await;
        let doc = mgr
            .docs
            .get_mut(doc_id)
            .ok_or_else(|| format!("Spec board document not found: {}", doc_id))?;
        if doc.document.content.len() > MAX_SNAPSHOT_CONTENT_BYTES {
            log::warn!(
                "[spec-board] skipping oversized snapshot for {}",
                doc.document.doc_id
            );
            (doc.snapshot_dir.clone(), None, true)
        } else {
            doc.snapshot_sequence += 1;
            let created_at_ms = now_ms();
            let hash = hash_str(&doc.document.content);
            let id = format!(
                "{}-{}-{}",
                created_at_ms,
                doc.snapshot_sequence,
                hash_prefix(&hash)
            );
            let label = format!("{:?} {}", source, doc.snapshot_sequence);
            let snapshot = SnapshotFile {
                id,
                label,
                created_at_ms,
                sequence: doc.snapshot_sequence,
                source,
                content: doc.document.content.clone(),
                diagram_source: doc.document.diagram_source.clone(),
                file_kind: doc.document.file_kind,
                path: doc.document.path.clone(),
                hash,
            };
            (doc.snapshot_dir.clone(), Some(snapshot), false)
        }
    };

    if skip_for_size {
        return Ok(());
    }
    let snapshot = snapshot.expect("snapshot present when not skipped");
    tokio::fs::create_dir_all(&snapshot_dir)
        .await
        .map_err(|e| format!("Failed to create snapshot directory: {}", e))?;
    write_snapshot_atomic(&snapshot_dir, &snapshot).await?;
    prune_snapshots(&snapshot_dir).await?;

    let snapshots = list_snapshots_from_dir(&snapshot_dir).await?;
    let mut mgr = state.write().await;
    if let Some(doc) = mgr.docs.get_mut(doc_id) {
        doc.document.version_count = snapshots.len();
        doc.document.version_index = snapshots.len().saturating_sub(1);
    }
    Ok(())
}

pub(super) async fn write_snapshot_atomic(
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
    tokio::fs::write(&temp, bytes)
        .await
        .map_err(|e| format!("Failed to write snapshot: {}", e))?;
    commit_temp_file(&temp, &target, "snapshot").await?;
    Ok(())
}

pub(super) async fn list_snapshots_from_dir(dir: &Path) -> Result<Vec<SpecBoardSnapshot>, String> {
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

pub(super) async fn read_snapshot_files(dir: &Path) -> Result<Vec<SnapshotFile>, String> {
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("Failed to read snapshot directory: {}", e)),
    };
    let mut files = Vec::new();
    let mut corrupt = 0usize;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| format!("Failed to read snapshot entry: {}", e))?
    {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        match tokio::fs::read(&path).await {
            Ok(bytes) => match serde_json::from_slice::<SnapshotFile>(&bytes) {
                Ok(snapshot) => files.push(snapshot),
                Err(e) => {
                    corrupt += 1;
                    log::warn!(
                        "[spec-board] skipping corrupt snapshot {}: {}",
                        path.display(),
                        e
                    );
                }
            },
            Err(e) => {
                corrupt += 1;
                log::warn!(
                    "[spec-board] skipping unreadable snapshot {}: {}",
                    path.display(),
                    e
                );
            }
        }
    }
    if corrupt > 0 {
        log::warn!("[spec-board] skipped {} corrupt snapshot(s)", corrupt);
    }
    files.sort_by_key(|s| (s.created_at_ms, s.sequence));
    Ok(files)
}

pub(super) async fn prune_snapshots(dir: &Path) -> Result<(), String> {
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("Failed to read snapshot directory: {}", e)),
    };
    let mut files = Vec::new();
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| format!("Failed to read snapshot entry: {}", e))?
    {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let metadata = entry
            .metadata()
            .await
            .map_err(|e| format!("Failed to read snapshot metadata: {}", e))?;
        files.push((path, metadata.len(), metadata.modified().ok()));
    }
    files.sort_by_key(|(_, _, modified)| *modified);
    let mut total: u64 = files.iter().map(|(_, len, _)| *len).sum();
    while files.len() > MAX_SNAPSHOTS || total > MAX_SNAPSHOT_TOTAL_BYTES {
        let (path, len, _) = files.remove(0);
        if let Err(e) = tokio::fs::remove_file(&path).await {
            log::warn!(
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
