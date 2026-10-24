use super::*;
use axum::response::IntoResponse;

#[tokio::test]
async fn window_screenshot_errors_use_typed_codes() {
    let cases = [
        (
            WindowScreenshotApiError::InvalidWindowId,
            StatusCode::BAD_REQUEST,
            "invalid_window_id",
        ),
        (
            WindowScreenshotApiError::WindowNotFound,
            StatusCode::NOT_FOUND,
            "window_not_found",
        ),
        (
            WindowScreenshotApiError::CaptureBusy,
            StatusCode::TOO_MANY_REQUESTS,
            "capture_busy",
        ),
        (
            WindowScreenshotApiError::CaptureTooLarge,
            StatusCode::PAYLOAD_TOO_LARGE,
            "capture_too_large",
        ),
        (
            WindowScreenshotApiError::CaptureUnavailable,
            StatusCode::SERVICE_UNAVAILABLE,
            "capture_unavailable",
        ),
    ];

    for (error, expected_status, expected_code) in cases {
        let response = ApiError::WindowScreenshot(error).into_response();
        assert_eq!(response.status(), expected_status);
        let body = match axum::body::to_bytes(response.into_body(), 8 * 1024).await {
            Ok(body) => body,
            Err(error) => panic!("error response body must be readable: {error}"),
        };
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains(expected_code), "{body}");
    }
}
