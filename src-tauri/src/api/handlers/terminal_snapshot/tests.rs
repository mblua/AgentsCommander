use super::*;

#[test]
fn headers_are_strict_and_no_store_is_unconditional() {
    let mut headers = HeaderMap::new();
    headers.insert(header::AUTHORIZATION, "Bearer secret".parse().unwrap());
    headers.insert(
        header::CONTENT_TYPE,
        "application/json; charset=utf-8".parse().unwrap(),
    );
    assert!(validate_request_headers(&headers).is_ok());
    headers.append(header::CONTENT_TYPE, "application/json".parse().unwrap());
    assert!(validate_request_headers(&headers).is_err());

    let response = error_response(TerminalSnapshotReasonCode::InvalidRequest);
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(response.headers()[header::PRAGMA], "no-cache");
}

#[tokio::test]
async fn authenticated_body_collection_obeys_the_server_deadline() {
    let stream = futures_util::stream::pending::<Result<axum::body::Bytes, std::io::Error>>();
    let body = Body::from_stream(stream);
    let result = collect_request_body(
        body,
        std::time::Instant::now() + std::time::Duration::from_millis(20),
    )
    .await;
    assert!(matches!(
        result,
        Err(TerminalSnapshotReasonCode::SnapshotTimeout)
    ));
}

#[test]
fn fixed_status_reason_mapping_is_preserved() {
    for reason in [
        TerminalSnapshotReasonCode::InvalidRequest,
        TerminalSnapshotReasonCode::RequesterUnavailable,
        TerminalSnapshotReasonCode::TerminalSnapshotsDisabled,
        TerminalSnapshotReasonCode::NotAuthorized,
        TerminalSnapshotReasonCode::TargetUnavailable,
        TerminalSnapshotReasonCode::SnapshotUnavailable,
        TerminalSnapshotReasonCode::SnapshotTooLarge,
        TerminalSnapshotReasonCode::AuthorityChanged,
        TerminalSnapshotReasonCode::RateLimited,
        TerminalSnapshotReasonCode::SnapshotTimeout,
        TerminalSnapshotReasonCode::ServiceUnavailable,
        TerminalSnapshotReasonCode::RenderFailed,
        TerminalSnapshotReasonCode::UnsafePath,
        TerminalSnapshotReasonCode::ResponseUnavailable,
        TerminalSnapshotReasonCode::Internal,
    ] {
        let response = error_response(reason);
        assert_eq!(
            response.status().as_u16(),
            reason.http_status().expect("server reason has status")
        );
    }
}
