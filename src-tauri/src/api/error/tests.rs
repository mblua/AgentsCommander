use super::*;

#[test]
fn scrub_redacts_windows_drive_path() {
    let s = "failed to open C:\\Users\\maria\\repo\\messaging\\x.md now";
    let scrubbed = scrub_host_paths(s);
    assert!(!scrubbed.contains("C:\\Users"));
    assert!(scrubbed.contains("<path>"));
    assert!(scrubbed.contains("failed to open"));
    assert!(scrubbed.contains("now"));
}

#[test]
fn scrub_redacts_forward_slash_drive_path() {
    let s = "outbox D:/data/outbox failed";
    let scrubbed = scrub_host_paths(s);
    assert!(!scrubbed.contains("D:/data"));
    assert!(scrubbed.contains("<path>"));
}

#[test]
fn scrub_leaves_non_paths_intact() {
    let s = "routing rejected: 'proj:wg-1/dev-rust' cannot reach 'proj:wg-1/tech-lead'";
    assert_eq!(scrub_host_paths(s), s);
}

#[test]
fn scrub_does_not_treat_url_as_drive_path() {
    // `https://host` has no drive letter (h,t,t,p,s before `:`), so it stays.
    let s = "see https://example.com/x for detail";
    assert_eq!(scrub_host_paths(s), s);
}

#[test]
fn error_status_mapping() {
    assert_eq!(
        ApiError::BadRequest("x".into()).status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        ApiError::Unauthorized("x".into()).status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        ApiError::Forbidden("x".into()).status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        ApiError::NotFound("x".into()).status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        ApiError::Conflict("x".into()).status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        ApiError::PayloadTooLarge("x".into()).status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(
        ApiError::Rejected("x".into()).status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        ApiError::TooManyRequests("x".into()).status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        ApiError::ServiceUnavailable("x".into()).status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    for (reason, status) in [
        (
            crate::phone::types::PtyInputReasonCode::InvalidText,
            StatusCode::BAD_REQUEST,
        ),
        (
            crate::phone::types::PtyInputReasonCode::PayloadTooLarge,
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
        (
            crate::phone::types::PtyInputReasonCode::ApiClientStale,
            StatusCode::UNAUTHORIZED,
        ),
        (
            crate::phone::types::PtyInputReasonCode::TargetOutOfScope,
            StatusCode::FORBIDDEN,
        ),
        (
            crate::phone::types::PtyInputReasonCode::Busy,
            StatusCode::CONFLICT,
        ),
        (
            crate::phone::types::PtyInputReasonCode::CapacityExceeded,
            StatusCode::TOO_MANY_REQUESTS,
        ),
        (
            crate::phone::types::PtyInputReasonCode::StoreTransient,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            crate::phone::types::PtyInputReasonCode::MenuGuardBlocked,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
    ] {
        let error = ApiError::PtyInput(crate::phone::types::PtyInputFailure::reject(reason));
        assert_eq!(error.status(), status);
        assert_eq!(
            error.code(),
            crate::phone::types::pty_input_reason_code_name(reason)
        );
    }
    assert_eq!(
        ApiError::Internal("x".into()).status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
}

mod window_screenshot;
