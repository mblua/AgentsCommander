use std::{
    future::Future,
    net::{IpAddr, SocketAddr},
    pin::Pin,
    sync::Arc,
};

use axum::{
    body::Body,
    extract::{ConnectInfo, Extension, OriginalUri, State},
    http::{header, HeaderMap, StatusCode},
    response::Response,
    routing::{get as axum_get, MethodRouter},
};

use crate::{
    api::{
        audit::{
            record_window_screenshot_result, WindowScreenshotAuditResult,
            WindowScreenshotAuditStatus,
        },
        error::{ApiError, WindowScreenshotApiError},
        handlers::authenticate_window_screenshot_fresh,
        ApiState, WindowScreenshotAdmissionError, WindowScreenshotLease,
    },
    screenshot::{capture_window_png, WindowScreenshotCaptureError},
};

pub(crate) const WINDOW_SCREENSHOT_ROUTE: &str = "/api/v1/windows/{window_id}/screenshot";

#[cfg(test)]
pub(crate) type WindowScreenshotCaptureFutureForTest = CaptureFuture;

#[cfg(test)]
pub(crate) fn mount_window_screenshot_route_for_test(
    capture_factory: std::sync::Arc<
        dyn Fn(String, WindowScreenshotLease) -> CaptureFuture + Send + Sync,
    >,
) -> MethodRouter<ApiState> {
    mount_window_screenshot_route(capture_factory)
}

#[cfg(test)]
pub(crate) async fn get_with_capture_for_test<F>(
    state: ApiState,
    headers: HeaderMap,
    addr: SocketAddr,
    original_uri: OriginalUri,
    capture_factory: F,
) -> Result<Response, ApiError>
where
    F: Fn(String, WindowScreenshotLease) -> CaptureFuture + Send + Sync + 'static,
{
    get_with_capture(
        state,
        headers,
        addr,
        original_uri,
        std::sync::Arc::new(capture_factory),
    )
    .await
}

type CaptureFuture =
    Pin<Box<dyn Future<Output = Result<Vec<u8>, WindowScreenshotCaptureError>> + Send>>;
type CaptureFactory = Arc<dyn Fn(String, WindowScreenshotLease) -> CaptureFuture + Send + Sync>;

pub(crate) fn route() -> MethodRouter<ApiState> {
    mount_window_screenshot_route(Arc::new(|window_id, lease| {
        Box::pin(capture_window_png(window_id, lease))
    }))
}

fn mount_window_screenshot_route(capture_factory: CaptureFactory) -> MethodRouter<ApiState> {
    axum_get(get).layer(Extension(capture_factory))
}

pub(crate) async fn get(
    State(state): State<ApiState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    original_uri: OriginalUri,
    Extension(capture_factory): Extension<CaptureFactory>,
) -> Result<Response, ApiError> {
    get_with_capture(state, headers, addr, original_uri, capture_factory).await
}

async fn get_with_capture(
    state: ApiState,
    headers: HeaderMap,
    addr: SocketAddr,
    original_uri: OriginalUri,
    capture_factory: CaptureFactory,
) -> Result<Response, ApiError> {
    let ip: IpAddr = addr.ip();
    let preflight_guard = authenticate_window_screenshot_fresh(&state, &headers, ip).await?;
    drop(preflight_guard);

    let window_id = match extract_raw_window_id(&original_uri).and_then(parse_window_id) {
        Ok(window_id) => window_id,
        Err(error) => {
            record_final(WindowScreenshotAuditStatus::InvalidWindowId);
            return Err(error);
        }
    };

    let admission = match state.window_screenshot_limiter.try_admit() {
        Ok(admission) => admission,
        Err(WindowScreenshotAdmissionError::CaptureBusy) => {
            return Err(audited_error(
                WindowScreenshotApiError::CaptureBusy,
                WindowScreenshotAuditStatus::CaptureBusy,
            ));
        }
    };
    let active = match state.window_screenshot_limiter.acquire_active().await {
        Ok(active) => active,
        Err(WindowScreenshotAdmissionError::CaptureBusy) => {
            return Err(audited_error(
                WindowScreenshotApiError::CaptureBusy,
                WindowScreenshotAuditStatus::CaptureBusy,
            ));
        }
    };

    let launch_guard = authenticate_window_screenshot_fresh(&state, &headers, ip).await?;
    drop(launch_guard);

    let lease = WindowScreenshotLease::new(admission, active);

    let png = match capture_factory(window_id, lease).await {
        Ok(png) => png,
        Err(WindowScreenshotCaptureError::NotFound) => {
            return Err(audited_error(
                WindowScreenshotApiError::WindowNotFound,
                WindowScreenshotAuditStatus::WindowNotFound,
            ));
        }
        Err(WindowScreenshotCaptureError::TooLarge) => {
            return Err(audited_error(
                WindowScreenshotApiError::CaptureTooLarge,
                WindowScreenshotAuditStatus::CaptureTooLarge,
            ));
        }
        Err(WindowScreenshotCaptureError::Unavailable) => {
            return Err(audited_error(
                WindowScreenshotApiError::CaptureUnavailable,
                WindowScreenshotAuditStatus::CaptureUnavailable,
            ));
        }
    };

    let response = match Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "image/png")
        .header(header::CONTENT_LENGTH, png.len().to_string())
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(png))
    {
        Ok(response) => response,
        Err(_) => {
            return Err(audited_error(
                WindowScreenshotApiError::CaptureUnavailable,
                WindowScreenshotAuditStatus::CaptureUnavailable,
            ));
        }
    };
    record_final(WindowScreenshotAuditStatus::Succeeded);
    Ok(response)
}

fn extract_raw_window_id(original_uri: &OriginalUri) -> Result<&str, ApiError> {
    const PREFIX: &str = "/api/v1/windows/";
    const SUFFIX: &str = "/screenshot";

    let path = original_uri.0.path();
    let raw_window_id = path
        .strip_prefix(PREFIX)
        .and_then(|path| path.strip_suffix(SUFFIX))
        .filter(|segment| !segment.is_empty() && !segment.contains('/'));
    raw_window_id.ok_or_else(invalid_window_id_error)
}

fn parse_window_id(raw_window_id: &str) -> Result<String, ApiError> {
    let bytes = raw_window_id.as_bytes();
    if bytes.is_empty()
        || bytes.len() > 20
        || !bytes.iter().all(u8::is_ascii_digit)
        || (bytes.len() > 1 && bytes[0] == b'0')
        || raw_window_id.parse::<u64>().is_err()
    {
        return Err(invalid_window_id_error());
    }
    Ok(raw_window_id.to_string())
}

fn invalid_window_id_error() -> ApiError {
    ApiError::WindowScreenshot(WindowScreenshotApiError::InvalidWindowId)
}

fn audited_error(error: WindowScreenshotApiError, status: WindowScreenshotAuditStatus) -> ApiError {
    record_final(status);
    ApiError::WindowScreenshot(error)
}

fn record_final(status: WindowScreenshotAuditStatus) {
    record_window_screenshot_result("window_screenshot", &WindowScreenshotAuditResult { status });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_window_id_validation_rejects_noncanonical_values() {
        for raw_window_id in [
            "",
            "-1",
            "+1",
            " 1",
            "1 ",
            "01",
            "18446744073709551616",
            "123456789012345678901",
            "%30",
            "abc",
        ] {
            assert!(parse_window_id(raw_window_id).is_err(), "{raw_window_id}");
        }
        assert!(matches!(parse_window_id("0"), Ok(value) if value == "0"));
        assert!(matches!(
            parse_window_id("18446744073709551615"),
            Ok(value) if value == "18446744073709551615"
        ));
    }

    #[test]
    fn raw_window_id_extraction_preserves_percent_encoding() {
        let uri: axum::http::Uri = match "/api/v1/windows/%FF/screenshot".parse() {
            Ok(uri) => uri,
            Err(error) => panic!("test URI must parse: {error}"),
        };
        let original_uri = OriginalUri(uri);
        assert!(matches!(extract_raw_window_id(&original_uri), Ok("%FF")));
        assert!(parse_window_id("%FF").is_err());
    }
}
