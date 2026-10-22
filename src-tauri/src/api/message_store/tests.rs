mod compaction;
mod enqueue;
mod migration;
mod recovery;

use super::*;
use uuid::Uuid;

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
