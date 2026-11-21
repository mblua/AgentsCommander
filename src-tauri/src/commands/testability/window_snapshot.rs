//! #4248 edge adapter for `window.snapshot`: renders the `main` WebView to a PNG in-process
//! through WebKitGTK on Linux. It never changes window state (no focus, show, move or resize).
use crate::testability::ui_automation::window_snapshot::{
    WindowSnapshotError, WindowSnapshotHost, WindowSnapshotPng,
};
use futures::future::BoxFuture;
#[cfg(target_os = "linux")]
use gio::prelude::CancellableExt;
#[cfg(target_os = "linux")]
use std::time::Duration;
#[cfg(target_os = "linux")]
use tauri::Manager;
#[cfg(target_os = "linux")]
use webkit2gtk::{SnapshotOptions, SnapshotRegion, WebViewExt};

type SnapshotResult = Result<WindowSnapshotPng, WindowSnapshotError>;

/// Native cancel on the GTK thread; below the 4 s core await limit.
#[cfg(target_os = "linux")]
const NATIVE_CANCEL_AFTER: Duration = Duration::from_secs(3);

#[cfg(not(target_os = "linux"))]
impl WindowSnapshotHost for tauri::AppHandle {
    fn snapshot_main_window(&self) -> BoxFuture<'_, SnapshotResult> {
        Box::pin(async { Err(WindowSnapshotError::Unsupported) })
    }
}

#[cfg(target_os = "linux")]
impl WindowSnapshotHost for tauri::AppHandle {
    fn snapshot_main_window(&self) -> BoxFuture<'_, SnapshotResult> {
        Box::pin(async move {
            let window = self
                .get_webview_window("main")
                .ok_or(WindowSnapshotError::WindowNotFound)?;
            let (sender, receiver) = tokio::sync::oneshot::channel();
            // The closure runs on the GTK main thread.
            window
                .with_webview(move |platform| start_snapshot(&platform.inner(), sender))
                .map_err(|error| WindowSnapshotError::Failed(error.to_string()))?;
            receiver.await.unwrap_or_else(|_closed| {
                Err(WindowSnapshotError::Failed(
                    "snapshot callback dropped".to_owned(),
                ))
            })
        })
    }
}

#[cfg(target_os = "linux")]
fn start_snapshot(
    view: &webkit2gtk::WebView,
    sender: tokio::sync::oneshot::Sender<SnapshotResult>,
) {
    let cancellable = gio::Cancellable::new();
    let on_timer = cancellable.clone();
    // D8: the one-shot source is never taken back. If the snapshot ends first, the late
    // cancel() hits this request's own finished Cancellable and does nothing.
    let _ = glib::timeout_add_local_once(NATIVE_CANCEL_AFTER, move || on_timer.cancel());
    let on_done = cancellable.clone();
    let (region, options) = (SnapshotRegion::Visible, SnapshotOptions::NONE);
    view.snapshot(region, options, Some(&cancellable), move |result| {
        // A closed channel means the caller stopped waiting; nothing to do.
        let _ = sender.send(snapshot_result(result, &on_done));
    });
}

#[cfg(target_os = "linux")]
fn snapshot_result(
    result: Result<cairo::Surface, glib::Error>,
    cancellable: &gio::Cancellable,
) -> SnapshotResult {
    let failed = WindowSnapshotError::Failed;
    let surface = match result {
        Ok(surface) => surface,
        Err(_error) if cancellable.is_cancelled() => return Err(WindowSnapshotError::Timeout),
        Err(error) => return Err(failed(error.to_string())),
    };
    let image = cairo::ImageSurface::try_from(surface)
        .map_err(|_surface| failed("unsupported snapshot surface type".to_owned()))?;
    let size = |value: i32| u32::try_from(value).map_err(|error| failed(format!("size: {error}")));
    let (width, height) = (size(image.width())?, size(image.height())?);
    let mut png = Vec::new();
    image
        .write_to_png(&mut png)
        .map_err(|error| failed(format!("png encode: {error}")))?;
    Ok(WindowSnapshotPng { png, width, height })
}
