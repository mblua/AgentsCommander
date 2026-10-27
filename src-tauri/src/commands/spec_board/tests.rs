use super::*;
use crate::spec_board::atomic_write_utf8;
use std::sync::Arc;
use tempfile::TempDir;

fn test_settings(paths: Vec<PathBuf>) -> AppSettings {
    AppSettings {
        project_paths: paths
            .into_iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect(),
        ..AppSettings::default()
    }
}

async fn test_state_with_doc(
    tmp: &TempDir,
    content: &str,
    dirty: bool,
) -> (SpecBoardState, String, PathBuf) {
    let repo = tmp.path().join("repo-Example");
    let path = repo.join("specs").join("mermaids").join("diagram.mmd");
    tokio::fs::create_dir_all(path.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&path, content).await.unwrap();
    let doc_id = uuid::Uuid::new_v4().to_string();
    let snapshot_dir = repo
        .join(".ac")
        .join("mermaids")
        .join("snapshots")
        .join(&doc_id);
    let draft_dir = repo
        .join(".ac")
        .join("mermaids")
        .join("drafts")
        .join(&doc_id);
    tokio::fs::create_dir_all(&snapshot_dir).await.unwrap();
    tokio::fs::create_dir_all(&draft_dir).await.unwrap();
    let document = SpecBoardDocument {
        doc_id: doc_id.clone(),
        repo_root: path_to_string(&repo),
        path: Some(path_to_string(&path)),
        file_kind: SpecBoardFileKind::Mermaid,
        content: content.to_string(),
        diagram_source: content.to_string(),
        dirty,
        conflict: None,
        version_index: 0,
        version_count: 0,
        updated_at_ms: now_ms(),
    };
    let mut manager = SpecBoardManager::new();
    manager.upsert_document(document, Some(hash_str(content)), snapshot_dir, draft_dir);
    (Arc::new(tokio::sync::RwLock::new(manager)), doc_id, path)
}

#[test]
fn mermaid_files_parse_as_full_source() {
    let src = "flowchart TD\nA-->B\n";
    assert_eq!(
        extract_diagram_source(src, SpecBoardFileKind::Mermaid).unwrap(),
        src
    );
}

#[test]
fn markdown_with_one_mermaid_block_parses() {
    let src = "# Spec\n\n```mermaid title\nflowchart TD\nA-->B\n```\n";
    assert_eq!(
        extract_diagram_source(src, SpecBoardFileKind::Markdown).unwrap(),
        "flowchart TD\nA-->B\n"
    );
}

#[test]
fn markdown_rejects_zero_or_multiple_mermaid_blocks() {
    assert!(
        extract_diagram_source("```rust\nmermaid\n```\n", SpecBoardFileKind::Markdown).is_err()
    );
    assert!(extract_diagram_source(
        "```mermaid\nA\n```\n```mermaid\nB\n```\n",
        SpecBoardFileKind::Markdown
    )
    .is_err());
}

#[test]
fn markdown_supports_tilde_longer_uppercase_and_info() {
    assert_eq!(
        extract_diagram_source("~~~~MERMAID title\nA\n~~~~\n", SpecBoardFileKind::Markdown)
            .unwrap(),
        "A\n"
    );
    assert_eq!(
        extract_diagram_source("````mermaid\nB\n````\n", SpecBoardFileKind::Markdown).unwrap(),
        "B\n"
    );
}

#[test]
fn markdown_rejects_unterminated_fence() {
    assert!(extract_diagram_source("```mermaid\nA\n", SpecBoardFileKind::Markdown).is_err());
}

#[tokio::test]
async fn path_validation_accepts_repo_specs_mermaid() {
    let tmp = TempDir::new().unwrap();
    let tmp_root = std::fs::canonicalize(tmp.path()).unwrap();
    let repo = tmp_root.join("repo-Example");
    tokio::fs::create_dir_all(repo.join("specs").join("mermaids"))
        .await
        .unwrap();
    let settings = test_settings(vec![tmp_root]);
    let target = repo.join("specs").join("mermaids").join("diagram.mmd");
    let validated = validate_new_or_existing_user_path(&target, &settings)
        .await
        .unwrap();
    assert_eq!(
        path_compare_key(&validated.repo_root),
        path_compare_key(&repo)
    );
}

#[tokio::test]
async fn path_validation_preserves_missing_intermediate_dirs() {
    let tmp = TempDir::new().unwrap();
    let tmp_root = std::fs::canonicalize(tmp.path()).unwrap();
    let repo = tmp_root.join("repo-Example");
    tokio::fs::create_dir_all(repo.join("specs")).await.unwrap();
    let settings = test_settings(vec![tmp_root]);
    let target = repo
        .join("specs")
        .join("new-folder")
        .join("nested")
        .join("diagram.mmd");

    let validated = validate_new_or_existing_user_path(&target, &settings)
        .await
        .unwrap();

    assert_eq!(path_compare_key(&validated.path), path_compare_key(&target));
    assert_eq!(
        path_compare_key(&validated.repo_root),
        path_compare_key(&repo)
    );
}

#[tokio::test]
async fn path_validation_rejects_ac_and_outside_paths() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo-Example");
    tokio::fs::create_dir_all(&repo).await.unwrap();
    let settings = test_settings(vec![tmp.path().to_path_buf()]);
    assert!(
        validate_new_or_existing_user_path(&repo.join(".ac/_agent_x/Role.md"), &settings)
            .await
            .is_err()
    );
    assert!(validate_new_or_existing_user_path(
        &repo.join(".ac/wg-1-dev-team/messaging/file.md"),
        &settings
    )
    .await
    .is_err());
    assert!(
        validate_new_or_existing_user_path(&tmp.path().join("outside.mmd"), &settings)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn path_validation_rejects_extension_and_traversal() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo-Example");
    tokio::fs::create_dir_all(&repo).await.unwrap();
    let settings = test_settings(vec![tmp.path().to_path_buf()]);
    assert!(
        validate_new_or_existing_user_path(&repo.join("bad.txt"), &settings)
            .await
            .is_err()
    );
    assert!(
        validate_new_or_existing_user_path(&repo.join("..").join("escape.mmd"), &settings)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn atomic_write_utf8_replaces_existing_file() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("diagram.mmd");
    tokio::fs::write(&path, "flowchart TD\nA-->B\n")
        .await
        .unwrap();

    atomic_write_utf8(&path, "flowchart TD\nA-->C\n")
        .await
        .unwrap();

    assert_eq!(
        tokio::fs::read_to_string(&path).await.unwrap(),
        "flowchart TD\nA-->C\n"
    );
}

#[tokio::test]
async fn corrupt_snapshot_does_not_break_listing() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("snapshots");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    tokio::fs::write(dir.join("bad.json"), "{").await.unwrap();
    let snap = SnapshotFile {
        id: "1".into(),
        label: "Initial 1".into(),
        created_at_ms: 1,
        sequence: 1,
        source: SpecBoardSnapshotSource::Initial,
        content: "flowchart TD\nA-->B\n".into(),
        diagram_source: "flowchart TD\nA-->B\n".into(),
        file_kind: SpecBoardFileKind::Mermaid,
        path: None,
        hash: hash_str("flowchart TD\nA-->B\n"),
    };
    write_snapshot_atomic(&dir, &snap).await.unwrap();
    let listed = list_snapshots_from_dir(&dir).await.unwrap();
    assert_eq!(listed.len(), 1);
}

#[tokio::test]
async fn snapshot_collision_does_not_overwrite() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("snapshots");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let mut snap = SnapshotFile {
        id: "same".into(),
        label: "Initial".into(),
        created_at_ms: 1,
        sequence: 1,
        source: SpecBoardSnapshotSource::Initial,
        content: "A".into(),
        diagram_source: "A".into(),
        file_kind: SpecBoardFileKind::Mermaid,
        path: None,
        hash: hash_str("A"),
    };
    write_snapshot_atomic(&dir, &snap).await.unwrap();
    snap.sequence = 2;
    snap.content = "B".into();
    snap.hash = hash_str("B");
    write_snapshot_atomic(&dir, &snap).await.unwrap();
    let listed = list_snapshots_from_dir(&dir).await.unwrap();
    assert_eq!(listed.len(), 2);
}

#[tokio::test]
async fn clean_external_update_replaces_content() {
    let tmp = TempDir::new().unwrap();
    let (state, doc_id, _) = test_state_with_doc(&tmp, "flowchart TD\nA-->B\n", false).await;
    let external = "flowchart TD\nA-->C\n".to_string();
    let event = handle_external_content(
        state.clone(),
        &doc_id,
        external.clone(),
        hash_str(&external),
    )
    .await
    .unwrap();
    assert!(matches!(event, Some(WatchEvent::Changed(_))));
    let mgr = state.read().await;
    let doc = mgr.get_document(&doc_id).unwrap();
    assert_eq!(doc.content, external);
    assert!(doc.conflict.is_none());
    assert!(!doc.dirty);
}

#[tokio::test]
async fn dirty_external_update_creates_conflict() {
    let tmp = TempDir::new().unwrap();
    let (state, doc_id, _) = test_state_with_doc(&tmp, "flowchart TD\nA-->B\n", true).await;
    let local = {
        let mut mgr = state.write().await;
        let doc = mgr.docs.get_mut(&doc_id).unwrap();
        doc.document.content = "flowchart TD\nA-->LOCAL\n".to_string();
        doc.document.content.clone()
    };
    let external = "flowchart TD\nA-->C\n".to_string();
    let event = handle_external_content(
        state.clone(),
        &doc_id,
        external.clone(),
        hash_str(&external),
    )
    .await
    .unwrap();
    let Some(WatchEvent::Conflict(payload)) = event else {
        panic!("expected conflict event");
    };
    assert_eq!(payload.doc_id, doc_id);
    assert_eq!(payload.content, local);
    assert_eq!(
        payload.conflict.as_ref().unwrap().pending_external_content,
        external
    );
    let mgr = state.read().await;
    let doc = mgr.get_document(&doc_id).unwrap();
    assert_eq!(doc.content, local);
    assert_eq!(doc.conflict.unwrap().pending_external_content, external);
}

#[tokio::test]
async fn checkout_snapshot_marks_document_dirty_without_disk_write() {
    let tmp = TempDir::new().unwrap();
    let (state, doc_id, path) = test_state_with_doc(&tmp, "flowchart TD\nA-->B\n", false).await;
    {
        let mut mgr = state.write().await;
        let doc = mgr.docs.get_mut(&doc_id).unwrap();
        doc.document.content = "flowchart TD\nA-->C\n".to_string();
        doc.document.diagram_source = doc.document.content.clone();
    }
    create_snapshot_for_doc(state.clone(), &doc_id, SpecBoardSnapshotSource::Edit)
        .await
        .unwrap();
    let snapshot_id =
        list_snapshots_from_dir(&state.read().await.docs.get(&doc_id).unwrap().snapshot_dir)
            .await
            .unwrap()[0]
            .id
            .clone();
    let snapshot_dir = {
        let mgr = state.read().await;
        mgr.docs.get(&doc_id).unwrap().snapshot_dir.clone()
    };
    let snapshots = read_snapshot_files(&snapshot_dir).await.unwrap();
    let snapshot = snapshots.into_iter().find(|s| s.id == snapshot_id).unwrap();
    {
        let mut mgr = state.write().await;
        let doc = mgr.docs.get_mut(&doc_id).unwrap();
        doc.document.content = snapshot.content;
        doc.document.diagram_source = snapshot.diagram_source;
        doc.document.dirty = doc
            .disk_hash
            .as_ref()
            .is_none_or(|disk| *disk != hash_str(&doc.document.content));
    }
    assert_eq!(
        tokio::fs::read_to_string(&path).await.unwrap(),
        "flowchart TD\nA-->B\n"
    );
    assert!(state.read().await.get_document(&doc_id).unwrap().dirty);
}

#[tokio::test]
async fn oversized_snapshot_is_skipped_without_losing_edits() {
    let tmp = TempDir::new().unwrap();
    let (state, doc_id, _) = test_state_with_doc(&tmp, "flowchart TD\nA-->B\n", true).await;
    let oversized = "A".repeat(MAX_SNAPSHOT_CONTENT_BYTES + 1);
    {
        let mut mgr = state.write().await;
        let doc = mgr.docs.get_mut(&doc_id).unwrap();
        doc.document.content = oversized.clone();
        doc.document.diagram_source = oversized.clone();
    }
    create_snapshot_for_doc(state.clone(), &doc_id, SpecBoardSnapshotSource::Edit)
        .await
        .unwrap();
    let snapshot_dir = {
        let mgr = state.read().await;
        mgr.docs.get(&doc_id).unwrap().snapshot_dir.clone()
    };
    assert!(list_snapshots_from_dir(&snapshot_dir)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        state
            .read()
            .await
            .get_document(&doc_id)
            .unwrap()
            .content
            .len(),
        oversized.len()
    );
}

#[tokio::test]
async fn concurrent_update_list_and_close_do_not_hang() {
    let tmp = TempDir::new().unwrap();
    let (state, doc_id, _) = test_state_with_doc(&tmp, "flowchart TD\nA-->B\n", true).await;
    let snapshot_dir = {
        let mgr = state.read().await;
        mgr.docs.get(&doc_id).unwrap().snapshot_dir.clone()
    };
    let update_state = state.clone();
    let update_id = doc_id.clone();
    let update = tokio::spawn(async move {
        {
            let mut mgr = update_state.write().await;
            let doc = mgr.docs.get_mut(&update_id).unwrap();
            doc.document.content = "flowchart TD\nA-->C\n".to_string();
            doc.document.diagram_source = doc.document.content.clone();
        }
        create_snapshot_for_doc(update_state, &update_id, SpecBoardSnapshotSource::Edit).await
    });
    let list = tokio::spawn(async move { list_snapshots_from_dir(&snapshot_dir).await });
    let close_state = state.clone();
    let close_id = doc_id.clone();
    let close = tokio::spawn(async move {
        let mut mgr = close_state.write().await;
        mgr.close(&close_id);
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        let _ = update.await;
        let _ = list.await;
        let _ = close.await;
    })
    .await
    .expect("concurrent operations hung");
}
