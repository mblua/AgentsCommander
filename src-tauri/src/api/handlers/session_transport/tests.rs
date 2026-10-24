use super::*;
use crate::api::auth::{ApiClientStore, FreshRegistryError, REGISTRY_FILENAME};
use crate::pty::idle_detector::IdleDetector;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[test]
fn uniform_transport_auth_failure_is_generic_401() {
    let response = uniform_transport_auth_failure().into_response();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[test]
fn transport_dependency_unavailable_is_retry_class_503() {
    let response = transport_dependency_unavailable().into_response();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[test]
fn fresh_registry_dependency_failures_are_retry_class_not_credential_failures() {
    // A dependency failure of the privileged registry lookup (contention or
    // an internal error) must never be reported to a container bridge as a
    // credential failure. Both classify as retry-class unavailability.
    assert!(matches!(
        classify_fresh_transport_auth(Err(FreshRegistryError::Contended)),
        FreshTransportAuth::DependencyUnavailable
    ));
    assert!(matches!(
        classify_fresh_transport_auth(Err(FreshRegistryError::Internal)),
        FreshTransportAuth::DependencyUnavailable
    ));
    // A registry that is readable but holds no matching credential is a real
    // credential failure.
    assert!(matches!(
        classify_fresh_transport_auth(Ok(None)),
        FreshTransportAuth::InvalidToken
    ));
}

#[tokio::test]
async fn held_registry_lock_makes_transport_auth_retry_class_with_executor_progress() {
    let dir = tempfile::TempDir::new().unwrap();
    // Hold the dedicated registry lock so the offloaded fresh lookup that the
    // transport boundary awaits observes cross-process contention.
    let _held = crate::api::auth::hold_registry_lock_for_test(dir.path());
    let store = Arc::new(ApiClientStore::new(dir.path().join(REGISTRY_FILENAME)));

    let lookup = {
        let store = Arc::clone(&store);
        tokio::spawn(async move {
            store
                .authenticate_pty_input_fresh_offloaded("presented-token".to_string())
                .await
        })
    };

    // The async executor must keep running while the blocking registry lookup
    // polls the held lock on the blocking pool.
    tokio::time::timeout(Duration::from_millis(100), async {
        tokio::time::sleep(Duration::from_millis(10)).await;
    })
    .await
    .expect("the async executor must progress while the registry lock is held");

    let result = lookup.await.unwrap();
    assert!(
        matches!(&result, Err(FreshRegistryError::Contended)),
        "a held registry lock must surface as retry-class contention, not a missing credential"
    );

    // The transport boundary classifies that contention as retry-class service
    // unavailability (503), never credential failure (401).
    assert!(matches!(
        classify_fresh_transport_auth(result),
        FreshTransportAuth::DependencyUnavailable
    ));
    assert_eq!(
        transport_dependency_unavailable().into_response().status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn validate_hello_protocol_mismatch_names_image_remedy() {
    let output_senders: crate::telegram::manager::OutputSenderMap =
        Arc::new(Mutex::new(HashMap::new()));
    let idle_detector = IdleDetector::new(|_| {}, |_| {});
    let backend = ContainerTransportBackend::new(output_senders, idle_detector, None, None);
    let session_id = Uuid::new_v4();
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    let reported_version = TRANSPORT_PROTOCOL_VERSION - 1;
    let text = serde_json::json!({
        "type": "hello",
        "version": reported_version,
        "sessionId": session_id,
        "root": "C:/root"
    })
    .to_string();

    let err = validate_hello(
        Message::Text(text.into()),
        &backend,
        session_id,
        "C:/root",
        tx,
    )
    .await
    .expect_err("protocol mismatch");

    assert!(matches!(
        err,
        HelloRejection::ProtocolMismatch {
            expected: TRANSPORT_PROTOCOL_VERSION,
            reported
        } if reported == reported_version
    ));
    assert_eq!(
        err.message(),
        format!(
            "container transport protocol mismatch: host expects protocol {} but container \
            image reported protocol {}. Rebuild or re-pull the AgentsCommander container image \
            so it contains the current session-bridge.",
            TRANSPORT_PROTOCOL_VERSION, reported_version
        )
    );
}

#[test]
fn protocol_mismatch_warning_payload_uses_session_env_warning_contract() {
    let session_id = Uuid::nil();
    let rejection = HelloRejection::ProtocolMismatch {
        expected: 2,
        reported: 1,
    };

    let warning = rejection
        .session_warning(session_id)
        .expect("protocol mismatch warning");

    assert_eq!(warning.session_id, session_id.to_string());
    assert_eq!(warning.key, CONTAINER_TRANSPORT_PROTOCOL_KEY);
    assert_eq!(warning.kind, WARNING_KIND_PROTOCOL_MISMATCH);
    assert_eq!(
        warning.message,
        "container transport protocol mismatch: host expects protocol 2 but container image \
        reported protocol 1. Rebuild or re-pull the AgentsCommander container image so it \
        contains the current session-bridge."
    );
}
