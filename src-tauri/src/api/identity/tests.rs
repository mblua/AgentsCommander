use super::*;
use crate::api::auth::ApiClient;

fn client_with_root(root: &str, bound_fqn: &str) -> ApiClient {
    ApiClient {
        client_id: "c1".into(),
        label: "l".into(),
        token_hash: "sha256:x".into(),
        bound_fqn: bound_fqn.into(),
        bound_root: root.into(),
        scopes: vec!["send".into()],
        issued_at: "2026-01-01T00:00:00Z".into(),
        expires_at: None,
        revoked: false,
        bound_session_id: None,
        credential_generation: None,
    }
}

#[test]
fn initial_snapshot_credential_debug_omits_auth_and_path_canaries() {
    const AUTH_CANARY: &str = "AUTH_1173_API_G2D6";
    const PATH_CANARY: &str = r"C:\PATH_1173_API_G2D6\replica";
    let proof = InitialApiCredentialProof {
        client_id: AUTH_CANARY.to_string(),
        bound_root: PATH_CANARY.to_string(),
        bound_session_id: AUTH_CANARY.to_string(),
        credential_generation: AUTH_CANARY.to_string(),
        presented_token_hash: AUTH_CANARY.to_string(),
    };
    let diagnostic = format!("{proof:?}");
    assert!(!diagnostic.contains(AUTH_CANARY));
    assert!(!diagnostic.contains(PATH_CANARY));
    assert_eq!(diagnostic, "InitialApiCredentialProof { .. }");
}

#[test]
fn missing_bound_root_is_401() {
    let c = client_with_root("C:/definitely/not/a/real/replica/path/xyz", "proj:wg-1/dev");
    let err = resolve_from(&c).unwrap_err();
    assert_eq!(err.status(), axum::http::StatusCode::UNAUTHORIZED);
}

#[test]
fn final_registry_bound_root_requires_the_same_filesystem_object() {
    let temp = tempfile::TempDir::new().unwrap();
    let root = temp.path().join("bound-root");
    let retired = temp.path().join("retired-root");
    std::fs::create_dir(&root).unwrap();
    let expected = crate::path_identity::verify_directory(&root).unwrap();
    let client = client_with_root(&root.to_string_lossy(), "project:wg-1-team/coordinator");
    assert!(verify_fresh_bound_root(&client, expected.object_id, &expected).is_some());

    std::fs::rename(&root, &retired).unwrap();
    std::fs::create_dir(&root).unwrap();
    assert!(verify_fresh_bound_root(&client, expected.object_id, &expected).is_none());
}

#[test]
fn wg_replica_root_resolves_to_fqn() {
    // Build a real WG-replica layout so agent_fqn_from_path yields a
    // non-root FQN and the existence check passes.
    let temp = tempfile::TempDir::new().unwrap();
    let replica = temp
        .path()
        .join("proj-x")
        .join(".ac")
        .join("wg-1-team")
        .join("__agent_dev-rust");
    std::fs::create_dir_all(&replica).unwrap();
    let root = replica.to_string_lossy().to_string();
    let c = client_with_root(&root, "stale-hint");
    let from = resolve_from(&c).expect("existing replica should resolve");
    assert!(from.ends_with("/dev-rust"), "got: {}", from);
    assert_ne!(from, crate::config::root_agent::ROOT_AGENT_SENDER);
}

/// #1614 requirement (D), section 9.1 "API and snapshot fixture twins".
///
/// This file takes ZERO production edits: it carries no prefix predicate
/// and treats the FQN as opaque. That claim is what this twin turns from
/// assumed into asserted. The `wg-` fixture above is deliberately NOT
/// converted (Rule P2): a legacy fixture is the only way dual-prefix
/// acceptance is testable at all, so the Room case is added BESIDE it.
#[test]
fn room_replica_root_resolves_to_fqn() {
    let temp = tempfile::TempDir::new().unwrap();
    let replica = temp
        .path()
        .join("proj-x")
        .join(".ac")
        .join("room-1-team")
        .join("__agent_dev-rust");
    std::fs::create_dir_all(&replica).unwrap();
    let root = replica.to_string_lossy().to_string();
    let c = client_with_root(&root, "stale-hint");
    let from = resolve_from(&c).expect("existing Room replica should resolve");
    assert!(from.ends_with("/dev-rust"), "got: {}", from);
    assert!(
        from.contains("room-1-team"),
        "the FQN carries the literal directory name: {}",
        from
    );
    assert_ne!(from, crate::config::root_agent::ROOT_AGENT_SENDER);
}

/// #1614 (D): a Room replica and a legacy Workgroup replica at the same
/// slot number are two DISTINCT peers, because every identity path keys on
/// the full directory name and never on the prefix or the number (the
/// mixed root of section 5.11, residual R1).
#[test]
fn room_and_legacy_replicas_at_the_same_slot_resolve_to_distinct_fqns() {
    let temp = tempfile::TempDir::new().unwrap();
    let ac = temp.path().join("proj-x").join(".ac");
    let mut resolved = Vec::new();
    for entity in ["wg-1-team", "room-1-team"] {
        let replica = ac.join(entity).join("__agent_dev-rust");
        std::fs::create_dir_all(&replica).unwrap();
        let c = client_with_root(&replica.to_string_lossy(), "stale-hint");
        resolved.push(resolve_from(&c).expect("replica should resolve"));
    }
    assert_ne!(
        resolved[0], resolved[1],
        "a Workgroup and a Room at slot 1 must be two different peers"
    );
    assert!(resolved[0].contains("wg-1-team"), "got: {}", resolved[0]);
    assert!(resolved[1].contains("room-1-team"), "got: {}", resolved[1]);
}
