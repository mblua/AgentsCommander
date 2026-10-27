use super::{
    hash_prefix, hash_str, now_ms, SnapshotFile, SpecBoardSnapshotSource,
    MAX_SNAPSHOT_CONTENT_BYTES,
};
use crate::spec_board::{ensure_snapshot_directory, prune_snapshots};
pub(super) use crate::spec_board::{
    list_snapshots_from_dir, read_snapshot_files, write_snapshot_atomic,
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
    ensure_snapshot_directory(&snapshot_dir).await?;
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
