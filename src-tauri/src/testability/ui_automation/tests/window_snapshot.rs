use super::*;
use crate::testability::ui_automation::window_snapshot::{
    handle_window_snapshot_request, WindowSnapshotError, WindowSnapshotHost, WindowSnapshotPng,
    MAX_SNAPSHOT_PNG_BYTES,
};

type SnapshotResult = Result<WindowSnapshotPng, WindowSnapshotError>;

/// Returns a copy of its result on every call and counts the calls.
struct FakeSnapshotHost(SnapshotResult, AtomicUsize);

impl FakeSnapshotHost {
    fn new(result: SnapshotResult) -> Self {
        Self(result, AtomicUsize::new(0))
    }

    fn png(png: Vec<u8>, width: u32, height: u32) -> Self {
        Self::new(Ok(WindowSnapshotPng { png, width, height }))
    }

    fn calls(&self) -> usize {
        self.1.load(Ordering::SeqCst)
    }
}

impl WindowSnapshotHost for FakeSnapshotHost {
    fn snapshot_main_window(&self) -> BoxFuture<'_, SnapshotResult> {
        self.1.fetch_add(1, Ordering::SeqCst);
        let result = match &self.0 {
            Ok(snapshot) => Ok(WindowSnapshotPng {
                png: snapshot.png.clone(),
                width: snapshot.width,
                height: snapshot.height,
            }),
            Err(error) => Err(error.clone()),
        };
        Box::pin(async move { result })
    }
}

impl backend_watchdog::BackendWatchdogHost for FakeSnapshotHost {
    fn resource_monitor(&self) -> Option<Arc<crate::resource_monitor::ResourceMonitorState>> {
        None
    }

    fn selection_coordinator(&self) -> Option<SelectionCoordinator> {
        None
    }
}

/// A host whose snapshot never finishes.
struct PendingSnapshotHost;

impl WindowSnapshotHost for PendingSnapshotHost {
    fn snapshot_main_window(&self) -> BoxFuture<'_, SnapshotResult> {
        Box::pin(futures::future::pending())
    }
}

fn snapshot_request(value: Option<String>) -> UiAutomationRequest {
    UiAutomationRequest {
        request_id: Uuid::new_v4().to_string(),
        token: "token".to_string(),
        window: BACKEND_AUTOMATION_WINDOW.to_string(),
        action: UiAutomationAction::Backend,
        selector: WINDOW_SNAPSHOT_BACKEND_SELECTOR.to_string(),
        value,
        expires_at_unix_ms: Some(now_unix_ms() + 1_000),
    }
}

fn absolute_png(dir: &tempfile::TempDir, name: &str) -> PathBuf {
    dir.path().join(name)
}

fn value_of(path: &Path) -> Option<String> {
    Some(path.to_string_lossy().into_owned())
}

#[tokio::test]
async fn snapshot_missing_value_is_invalid() {
    let host = FakeSnapshotHost::png(vec![1, 2, 3], 1, 1);
    let response = handle_window_snapshot_request(&host, &snapshot_request(None)).await;
    assert!(!response.ok);
    assert_eq!(response.error.as_deref(), Some("invalid_value"));
    assert_eq!(host.calls(), 0);
}

#[tokio::test]
async fn snapshot_relative_path_is_invalid() {
    let host = FakeSnapshotHost::png(vec![1, 2, 3], 1, 1);
    let request = snapshot_request(Some("relative.png".to_string()));
    let response = handle_window_snapshot_request(&host, &request).await;
    assert_eq!(response.error.as_deref(), Some("invalid_value"));
    assert_eq!(host.calls(), 0);
}

#[tokio::test]
async fn snapshot_non_png_is_invalid() {
    let dir = tempfile::tempdir().expect("tempdir");
    let host = FakeSnapshotHost::png(vec![1, 2, 3], 1, 1);
    for name in ["out.txt", "out", "out.png.bak"] {
        let request = snapshot_request(value_of(&absolute_png(&dir, name)));
        let response = handle_window_snapshot_request(&host, &request).await;
        assert_eq!(response.error.as_deref(), Some("invalid_value"), "{name}");
    }
    assert_eq!(host.calls(), 0);
}

#[tokio::test]
async fn snapshot_host_errors_map_to_codes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cases = [
        (WindowSnapshotError::WindowNotFound, "window_not_found"),
        (WindowSnapshotError::Unsupported, "snapshot_unsupported"),
        (WindowSnapshotError::Timeout, "snapshot_timeout"),
        (
            WindowSnapshotError::Failed("boom".to_string()),
            "snapshot_failed",
        ),
    ];
    for (error, code) in cases {
        let host = FakeSnapshotHost::new(Err(error));
        let request = snapshot_request(value_of(&absolute_png(&dir, "out.PNG")));
        let response = handle_window_snapshot_request(&host, &request).await;
        assert!(!response.ok, "{code}");
        assert_eq!(response.error.as_deref(), Some(code));
        assert_eq!(host.calls(), 1, "{code}");
        if code == "snapshot_failed" {
            let message = response.diagnostics.as_ref().map(|value| &value["message"]);
            assert_eq!(message, Some(&json!("boom")));
        }
    }
    assert!(!absolute_png(&dir, "out.PNG").exists());
}

#[tokio::test]
async fn snapshot_oversize_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = absolute_png(&dir, "big.png");
    let host = FakeSnapshotHost::png(vec![0; MAX_SNAPSHOT_PNG_BYTES + 1], 10, 10);
    let response = handle_window_snapshot_request(&host, &snapshot_request(value_of(&path))).await;
    assert_eq!(response.error.as_deref(), Some("snapshot_too_large"));
    assert!(!path.exists());
    assert_eq!(host.calls(), 1);
}

#[tokio::test(start_paused = true)]
async fn snapshot_never_resolving_host_times_out() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = absolute_png(&dir, "late.png");
    let request = snapshot_request(value_of(&path));
    let response = handle_window_snapshot_request(&PendingSnapshotHost, &request).await;
    assert_eq!(response.error.as_deref(), Some("snapshot_timeout"));
    assert!(!path.exists());
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
#[tokio::test]
async fn snapshot_ok_writes_exact_bytes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = absolute_png(&dir, "main.png");
    let png = b"\x89PNG\r\n\x1a\nfake".to_vec();
    let host = FakeSnapshotHost::png(png.clone(), 640, 480);
    let response = handle_window_snapshot_request(&host, &snapshot_request(value_of(&path))).await;
    assert!(response.ok, "{:?}", response.error);
    assert_eq!(std::fs::read(&path).expect("read png"), png);
    let target = response.target.expect("target");
    assert_eq!(target["state"], json!("captured"));
    assert_eq!(target["text"], json!("window snapshot main 640x480"));
    let diagnostics = response.diagnostics.expect("diagnostics");
    assert_eq!(diagnostics["window"], json!("main"));
    assert_eq!(diagnostics["path"], json!(path.to_string_lossy()));
    assert_eq!(diagnostics["width"], json!(640));
    assert_eq!(diagnostics["height"], json!(480));
    assert_eq!(diagnostics["bytes"], json!(png.len()));
    assert_eq!(diagnostics["source"], json!("webkitgtk"));
    assert_eq!(host.calls(), 1);
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
#[tokio::test]
async fn snapshot_missing_parent_is_write_failure() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("missing").join("main.png");
    let host = FakeSnapshotHost::png(vec![1, 2, 3], 1, 1);
    let response = handle_window_snapshot_request(&host, &snapshot_request(value_of(&path))).await;
    assert_eq!(response.error.as_deref(), Some("output_write_failed"));
    assert_eq!(host.calls(), 1);
}

#[tokio::test]
async fn backend_dispatch_routes_window_snapshot() {
    let host = FakeSnapshotHost::png(vec![1, 2, 3], 1, 1);
    let request = snapshot_request(Some("relative.png".to_string()));
    let response = backend_watchdog::handle_backend_request(&host, &request).await;
    assert_eq!(response.error.as_deref(), Some("invalid_value"));

    let dir = tempfile::tempdir().expect("tempdir");
    let request = snapshot_request(value_of(&absolute_png(&dir, "out.png")));
    let response = backend_watchdog::handle_backend_request(&host, &request).await;
    assert_ne!(response.error.as_deref(), Some("unsupported_selector"));
    assert_eq!(host.calls(), 1);

    let mut unknown = snapshot_request(None);
    unknown.selector = "window.unknown".to_string();
    let response = backend_watchdog::handle_backend_request(&host, &unknown).await;
    assert_eq!(response.error.as_deref(), Some("unsupported_selector"));
    assert_eq!(host.calls(), 1);
}
