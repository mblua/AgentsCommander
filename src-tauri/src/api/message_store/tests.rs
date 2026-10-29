mod compaction;
mod enqueue;
mod migration;
mod recovery;

use super::*;
use chrono::{DateTime, Utc};
use rusqlite::params;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::time::Duration;
use uuid::Uuid;

#[test]
fn message_path_target_root_file_is_io() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    std::fs::write(&root, b"file").unwrap();
    assert!(matches!(
        PtyInputTargetGate::new(root),
        Err(MessageStoreError::Io(_))
    ));
}

#[test]
fn message_path_lock_directory_file_is_unsafe() {
    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path();
    std::fs::write(parent.join("pty-input-locks"), b"file").unwrap();
    assert!(crate::path_identity::verify_directory(parent).is_ok());
    assert!(matches!(
        try_stripe_lock_at(parent, "v", false),
        Err(MessageStoreError::UnsafePath)
    ));
}

#[test]
fn message_path_sqlite_wal_directory_is_unsafe() {
    let temp = tempfile::tempdir().unwrap();
    let db = temp.path().join(DB_FILENAME);
    std::fs::create_dir(sqlite_sidecar_paths(&db)[0].clone()).unwrap();
    assert!(matches!(
        verify_existing_sqlite_files(&db),
        Err(MessageStoreError::UnsafePath)
    ));
    assert!(crate::path_identity::verify_directory(temp.path()).is_ok());
    assert!(matches!(
        MessageStore::open(db.clone()),
        Err(MessageStoreError::UnsafePath)
    ));
    assert!(matches!(std::fs::symlink_metadata(&db),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound));
}

#[test]
fn message_path_store_parent_file_is_io() {
    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path().join("parent");
    std::fs::write(&parent, b"file").unwrap();
    assert!(matches!(
        MessageStore::open(parent.join(DB_FILENAME)),
        Err(MessageStoreError::Io(_))
    ));
}

fn store() -> MessageStore {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join(DB_FILENAME);
    let store = MessageStore::open(path).unwrap();
    std::mem::forget(dir);
    store
}

fn request(op: &str, body: &str) -> EnqueueRequest {
    EnqueueRequest {
        sender_fqn: "proj:wg-1/a".into(),
        target_fqn: "proj:wg-1/b".into(),
        op_id: op.into(),
        content_type: DEFAULT_CONTENT_TYPE.into(),
        body: body.into(),
        source_plane: "api_inline".into(),
        source_ref: None,
    }
}

fn pty_request(op_id: &str, text: &str) -> PtyInputEnqueueRequest {
    let now = Utc::now();
    PtyInputEnqueueRequest {
        injection_id: op_id.to_string(),
        sender_fqn: "proj:wg-1-team/lead".into(),
        target_fqn: "proj:wg-1-team/dev".into(),
        op_id: op_id.to_string(),
        nonce_sha256: sha256_hex(Uuid::new_v4().to_string().as_bytes()),
        request_fingerprint: sha256_hex(format!("request:{text}").as_bytes()),
        confirmation_tag: None,
        requested_agent_id: None,
        payload: text.as_bytes().to_vec(),
        source_plane: crate::phone::types::PtyInputSourcePlane::ContainerApi,
        sender_incarnation_fingerprint: "a".repeat(64),
        sender_identity_fingerprint: "b".repeat(64),
        target_identity_fingerprint: "c".repeat(64),
        authority_session_id: Uuid::new_v4().to_string(),
        authority_client_id: Some("client".into()),
        authority_client_generation: Some(Uuid::new_v4().to_string()),
        issued_at: crate::phone::types::canonical_pty_timestamp(now),
        expires_at: crate::phone::types::canonical_pty_timestamp(
            now + chrono::Duration::minutes(10),
        ),
    }
}

fn enqueue_claim_actuating(store: &MessageStore, sender_index: usize) -> (String, DateTime<Utc>) {
    let id = Uuid::new_v4().to_string();
    let mut request = pty_request(&id, "exact text");
    request.sender_fqn = format!("proj:wg-1-team/lead-{sender_index}");
    request.request_fingerprint =
        sha256_hex(format!("request:{sender_index}:{}", request.op_id).as_bytes());
    store.enqueue_pty_input(request).unwrap();
    let transition_at = Utc::now();
    store
        .claim_pty_input(
            crate::phone::types::PtyInputSourcePlane::ContainerApi,
            Some(&id),
            &format!("lease-{id}"),
            transition_at,
        )
        .unwrap()
        .unwrap();
    store
        .begin_pty_actuating(
            &id,
            &format!("lease-{id}"),
            &Uuid::new_v4().to_string(),
            "localProcess",
            transition_at,
        )
        .unwrap();
    (id, transition_at)
}
