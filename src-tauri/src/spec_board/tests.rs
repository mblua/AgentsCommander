use super::*;
use crate::spec_board::types::{SpecBoardFileKind, SpecBoardSnapshotSource};
use tempfile::TempDir;

#[tokio::test]
async fn snapshot_directory_errors_preserve_non_not_found() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("snapshots");
    assert!(list_snapshots_from_dir(&dir).await.unwrap().is_empty());
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let snapshot = SnapshotFile {
        id: "known-id".into(),
        label: "Initial 1".into(),
        created_at_ms: 42,
        sequence: 1,
        source: SpecBoardSnapshotSource::Initial,
        content: "flowchart TD\nA-->B\n".into(),
        diagram_source: "flowchart TD\nA-->B\n".into(),
        file_kind: SpecBoardFileKind::Mermaid,
        path: None,
        hash: "known-hash".into(),
    };
    tokio::fs::write(
        dir.join("42-000001-initial.json"),
        serde_json::to_vec_pretty(&snapshot).unwrap(),
    )
    .await
    .unwrap();
    let listed = list_snapshots_from_dir(&dir).await.unwrap();
    assert_eq!(listed.len(), 1);
    let actual = &listed[0];
    assert_eq!(actual.id, "known-id");
    assert_eq!(actual.label, "Initial 1");
    assert_eq!(actual.created_at_ms, 42);
    assert_eq!(actual.source, SpecBoardSnapshotSource::Initial);
    assert_eq!(actual.content, "flowchart TD\nA-->B\n");
    assert_eq!(actual.diagram_source, "flowchart TD\nA-->B\n");
    assert_eq!(actual.hash, "known-hash");

    let invalid = tmp.path().join("invalid\0path");
    let error = tokio::fs::read_dir(&invalid).await.unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert_eq!(
        list_snapshots_from_dir(&invalid).await.unwrap_err(),
        format!("Failed to read snapshot directory: {}", error)
    );
}

#[tokio::test]
async fn snapshot_pruning_preserves_count_and_total_without_mtime_assumptions() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    let contents = b"known snapshot bytes";
    for index in 0..105 {
        tokio::fs::write(dir.join(format!("{index}.json")), contents)
            .await
            .unwrap();
    }
    prune_snapshots(dir).await.unwrap();
    let files = filesystem::snapshot_file_metadata(dir).await.unwrap();
    assert_eq!(files.len(), 100);
    assert_eq!(
        files.iter().map(|(_, len, _)| *len).sum::<u64>(),
        100 * contents.len() as u64
    );
    for (path, _, _) in files {
        assert_eq!(tokio::fs::read(path).await.unwrap(), contents);
    }
}
