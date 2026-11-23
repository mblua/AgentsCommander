//! `window.snapshot` backend selector. Checks the output path, asks the host for an
//! app-rendered PNG of the `main` window and writes it. The host adapter is in the edge.
use super::{UiAutomationRequest, UiAutomationResponse};
#[cfg(any(target_os = "windows", target_os = "linux"))]
use crate::filesystem::documents;
use futures::future::BoxFuture;
use serde_json::json;
use std::path::Path;
use std::time::Duration;

/// Outer await limit; the adapter cancels natively after 3 s, the CLI waits 5 s by default.
const SNAPSHOT_AWAIT_LIMIT: Duration = Duration::from_secs(4);
pub(crate) const MAX_SNAPSHOT_PNG_BYTES: usize = 33_554_432;

// Only the Linux adapter builds these; tests build them on every OS.
#[cfg_attr(
    not(target_os = "linux"),
    allow(dead_code, reason = "only the Linux adapter builds it outside tests")
)]
pub(crate) struct WindowSnapshotPng {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

#[cfg_attr(
    not(target_os = "linux"),
    allow(dead_code, reason = "only the Linux adapter builds it outside tests")
)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WindowSnapshotError {
    WindowNotFound,
    // Built only by the non-Linux adapter.
    #[cfg_attr(
        target_os = "linux",
        allow(dead_code, reason = "built only by the non-Linux adapter")
    )]
    Unsupported,
    Timeout,
    Failed(String),
}

pub(crate) trait WindowSnapshotHost {
    fn snapshot_main_window(&self)
        -> BoxFuture<'_, Result<WindowSnapshotPng, WindowSnapshotError>>;
}

pub(crate) async fn handle_window_snapshot_request<H: WindowSnapshotHost>(
    app: &H,
    request: &UiAutomationRequest,
) -> UiAutomationResponse {
    let Some(path) = request
        .value
        .as_deref()
        .map(Path::new)
        .filter(|path| is_png_output(path))
    else {
        return UiAutomationResponse::error_for_request(
            request,
            "invalid_value",
            "window.snapshot needs an absolute output path ending in .png.",
        );
    };

    let snapshot =
        match tokio::time::timeout(SNAPSHOT_AWAIT_LIMIT, app.snapshot_main_window()).await {
            Err(_elapsed) => return timeout_response(request),
            Ok(Err(error)) => return host_error_response(request, error),
            Ok(Ok(snapshot)) => snapshot,
        };

    if snapshot.png.len() > MAX_SNAPSHOT_PNG_BYTES {
        return UiAutomationResponse::error_for_request(
            request,
            "snapshot_too_large",
            &format!(
                "Snapshot PNG is {} bytes; the limit is {MAX_SNAPSHOT_PNG_BYTES} bytes.",
                snapshot.png.len()
            ),
        );
    }

    if let Some(response) = write_snapshot_error(request, path, &snapshot.png) {
        return response;
    }

    ok_response(request, path, &snapshot)
}

fn is_png_output(path: &Path) -> bool {
    path.is_absolute()
        && path
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
}

fn timeout_response(request: &UiAutomationRequest) -> UiAutomationResponse {
    UiAutomationResponse::error_for_request(
        request,
        "snapshot_timeout",
        "The window snapshot did not finish in time.",
    )
}

fn host_error_response(
    request: &UiAutomationRequest,
    error: WindowSnapshotError,
) -> UiAutomationResponse {
    match error {
        WindowSnapshotError::WindowNotFound => UiAutomationResponse::error_for_request(
            request,
            "window_not_found",
            "The main window is not open.",
        ),
        WindowSnapshotError::Unsupported => UiAutomationResponse::error_for_request(
            request,
            "snapshot_unsupported",
            "Window snapshots are not supported on this platform.",
        ),
        WindowSnapshotError::Timeout => timeout_response(request),
        WindowSnapshotError::Failed(message) => {
            let mut response = UiAutomationResponse::error_for_request(
                request,
                "snapshot_failed",
                "The window snapshot failed.",
            );
            response.diagnostics = Some(json!({ "message": message }));
            response
        }
    }
}

/// Writes the PNG; returns the error response when the write fails.
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn write_snapshot_error(
    request: &UiAutomationRequest,
    path: &Path,
    png: &[u8],
) -> Option<UiAutomationResponse> {
    let error = documents::write_window_screenshot(path, png).err()?;
    Some(UiAutomationResponse::error_for_request(
        request,
        "output_write_failed",
        &format!("Could not write the snapshot PNG: {error}"),
    ))
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
fn write_snapshot_error(
    request: &UiAutomationRequest,
    _path: &Path,
    _png: &[u8],
) -> Option<UiAutomationResponse> {
    Some(UiAutomationResponse::error_for_request(
        request,
        "snapshot_unsupported",
        "Window snapshots are not supported on this platform.",
    ))
}

fn ok_response(
    request: &UiAutomationRequest,
    path: &Path,
    snapshot: &WindowSnapshotPng,
) -> UiAutomationResponse {
    let (width, height) = (snapshot.width, snapshot.height);
    UiAutomationResponse {
        ok: true,
        request_id: request.request_id.clone(),
        window: request.window.clone(),
        action: request.action,
        selector: request.selector.clone(),
        target: Some(json!({
            "testId": request.selector,
            "role": "backend",
            "state": "captured",
            "tag": "backend",
            "text": format!("window snapshot main {width}x{height}"),
            "visible": true,
            "disabled": false,
        })),
        error: None,
        message: None,
        available: None,
        diagnostics: Some(json!({
            "window": "main",
            "path": path.to_string_lossy(),
            "width": width,
            "height": height,
            "bytes": snapshot.png.len(),
            "source": "webkitgtk",
        })),
        available_windows: None,
        timeout_ms: None,
        phase: None,
    }
}
