use std::net::SocketAddr;
use std::path::Path;
use std::time::{Duration, Instant};

use axum::body::{to_bytes, Body};
use axum::extract::{ConnectInfo, State};
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::api::auth::SCOPE_WINDOW_CAPTURE;
use crate::api::handlers::authenticated_request::{
    AuthenticatedRequestError, InitialAuthenticatedRequest,
};
use crate::api::schema::{
    WindowApiError, WindowApiSuccess, WindowBoundsResponse, WindowCaptureRequest,
    WindowListRequest, WindowListResponse, WindowTargetResponse,
};
use crate::api::window_target_registry::RegisteredWindow;
use crate::api::ApiState;
use crate::window_capture::{
    CaptureCancellation, CaptureOptions, DiscoveryFilter, MonitorIntersection, WindowCapture,
    WindowCaptureError, WindowTargetId,
};

const MAX_WINDOW_REQUEST_BYTES: usize = 16 * 1024;
const LIST_TIMEOUT: Duration = Duration::from_secs(10);

pub async fn list(
    State(state): State<ApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    request: Request<Body>,
) -> Response {
    let (parts, body) = request.into_parts();
    if parts.uri.query().is_some() {
        return error_response(StatusCode::BAD_REQUEST, "invalid_request");
    }
    let admission = match admit(&state, address, &parts.headers).await {
        Ok(admission) => admission,
        Err(response) => return response,
    };
    let request = match decode_list_request(body).await {
        Ok(request) => request,
        Err(response) => return response,
    };
    let permit = match state.window_capture_admission.try_acquire_discovery() {
        Ok(permit) => permit,
        Err(error) => return capture_error_response(error),
    };
    let cancellation = CaptureCancellation::new(Instant::now() + LIST_TIMEOUT);
    let filter = DiscoveryFilter {
        process_id: request.process_id,
        include_nonvisible: request.include_nonvisible,
    };
    let discovered = match tokio::task::spawn_blocking(move || {
        let _permit = permit;
        WindowCapture.discover(filter, &cancellation)
    })
    .await
    {
        Ok(Ok(discovered)) => discovered,
        Ok(Err(error)) => return capture_error_response(error),
        Err(_) => return capture_error_response(WindowCaptureError::CaptureUnsupported),
    };
    let caller_binding = admission.caller_binding(&state.caller_binding_salt);
    let targets = {
        let mut registry = match state.window_target_registry.lock() {
            Ok(registry) => registry,
            Err(_) => return capture_error_response(WindowCaptureError::CaptureBusy),
        };
        match registry.replace_for_caller(caller_binding, discovered) {
            Ok(targets) => targets,
            Err(error) => return capture_error_response(error),
        }
    };

    let response = WindowListResponse {
        targets: targets.into_iter().map(map_registered_window).collect(),
        expires_in_ms: 60_000,
    };
    (StatusCode::OK, Json(WindowApiSuccess::new(response))).into_response()
}

pub async fn capture(
    State(state): State<ApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    request: Request<Body>,
) -> Response {
    let (parts, body) = request.into_parts();
    if parts.uri.query().is_some() {
        return error_response(StatusCode::BAD_REQUEST, "invalid_request");
    }
    let admission = match admit(&state, address, &parts.headers).await {
        Ok(admission) => admission,
        Err(response) => return response,
    };
    let request = match decode_capture_request(body).await {
        Ok(request) => request,
        Err(response) => return response,
    };
    let target_id = match WindowTargetId::parse(&request.target) {
        Ok(target_id) => target_id,
        Err(error) => return capture_error_response(error),
    };
    let options = match CaptureOptions::from_seconds(request.timeout_seconds) {
        Ok(options) => options,
        Err(error) => return capture_error_response(error),
    };
    let permit = match state.window_capture_admission.try_acquire_capture() {
        Ok(permit) => permit,
        Err(error) => return capture_error_response(error),
    };
    let caller_binding = admission.caller_binding(&state.caller_binding_salt);
    let fingerprint = {
        let mut registry = match state.window_target_registry.lock() {
            Ok(registry) => registry,
            Err(_) => return capture_error_response(WindowCaptureError::CaptureBusy),
        };
        if let Err(error) = registry.preflight_for_caller(caller_binding, &target_id) {
            return capture_error_response(error);
        }
        match registry.consume_for_caller(caller_binding, &target_id) {
            Ok(fingerprint) => fingerprint,
            Err(error) => return capture_error_response(error),
        }
    };
    let cancellation = CaptureCancellation::new(Instant::now() + options.timeout);
    let output = request.output;
    match tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let capture = WindowCapture;
        let prepared = capture.prepare_output(Path::new(&output))?;
        capture.capture_to_pending_artifact(fingerprint, prepared, options, &cancellation)
    })
    .await
    {
        Ok(Err(error)) => capture_error_response(error),
        Ok(Ok(pending)) => {
            WindowCapture.abort_pending(pending);
            capture_error_response(WindowCaptureError::CaptureUnsupported)
        }
        Err(_) => capture_error_response(WindowCaptureError::CaptureUnsupported),
    }
}

async fn admit(
    state: &ApiState,
    address: SocketAddr,
    headers: &axum::http::HeaderMap,
) -> Result<InitialAuthenticatedRequest, Response> {
    crate::api::handlers::authenticated_request::pre_admit(
        state,
        address.ip(),
        headers,
        SCOPE_WINDOW_CAPTURE,
    )
    .await
    .map_err(admission_error_response)
}

async fn decode_list_request(body: Body) -> Result<WindowListRequest, Response> {
    let bytes = to_bytes(body, MAX_WINDOW_REQUEST_BYTES)
        .await
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid_request"))?;
    let request = serde_json::from_slice::<WindowListRequest>(&bytes)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid_request"))?;
    request
        .validate()
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid_request"))?;
    Ok(request)
}

async fn decode_capture_request(body: Body) -> Result<WindowCaptureRequest, Response> {
    let bytes = to_bytes(body, MAX_WINDOW_REQUEST_BYTES)
        .await
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid_request"))?;
    let request = serde_json::from_slice::<WindowCaptureRequest>(&bytes)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid_request"))?;
    request
        .validate()
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid_request"))?;
    Ok(request)
}

fn admission_error_response(error: AuthenticatedRequestError) -> Response {
    match error {
        AuthenticatedRequestError::ScopeDenied => {
            error_response(StatusCode::FORBIDDEN, "scope_denied")
        }
        AuthenticatedRequestError::RateLimited
        | AuthenticatedRequestError::AuthenticationFailed
        | AuthenticatedRequestError::BoundAuthority(_) => {
            error_response(StatusCode::UNAUTHORIZED, "authentication_failed")
        }
        AuthenticatedRequestError::ServiceUnavailable => {
            error_response(StatusCode::SERVICE_UNAVAILABLE, "authentication_failed")
        }
    }
}

fn capture_error_response(error: WindowCaptureError) -> Response {
    let status = match error {
        WindowCaptureError::InvalidRequest | WindowCaptureError::OutputDenied => {
            StatusCode::BAD_REQUEST
        }
        WindowCaptureError::ScopeDenied => StatusCode::FORBIDDEN,
        WindowCaptureError::CaptureBusy => StatusCode::TOO_MANY_REQUESTS,
        WindowCaptureError::OutputExists => StatusCode::CONFLICT,
        WindowCaptureError::CaptureUnsupported => StatusCode::NOT_IMPLEMENTED,
        WindowCaptureError::CaptureDeviceLost => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::BAD_REQUEST,
    };
    error_response(status, error.stable_code())
}

fn error_response(status: StatusCode, code: &'static str) -> Response {
    (
        status,
        Json(WindowApiError::new(
            code,
            "window operation was not completed",
        )),
    )
        .into_response()
}

fn map_registered_window(window: RegisteredWindow) -> WindowTargetResponse {
    let diagnostics = window.diagnostics;
    WindowTargetResponse {
        target: window.target_id.to_string(),
        pid: diagnostics.process_id,
        process: diagnostics.process_name,
        title: diagnostics.title,
        class: diagnostics.class_name,
        bounds: diagnostics.bounds.map(|bounds| WindowBoundsResponse {
            left: bounds.left,
            top: bounds.top,
            right: bounds.right,
            bottom: bounds.bottom,
        }),
        session: diagnostics.session_id,
        visible: diagnostics.visible,
        minimized: diagnostics.minimized,
        cloaked: diagnostics.cloaked,
        foreground: diagnostics.foreground,
        protected: diagnostics.protected,
        monitor_intersection: match diagnostics.monitor_intersection {
            MonitorIntersection::IntersectsMonitor => "intersects_monitor".to_string(),
            MonitorIntersection::OffScreen => "offscreen".to_string(),
        },
        warnings: diagnostics.warnings,
    }
}
