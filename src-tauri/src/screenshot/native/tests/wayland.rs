//! #2774 Wayland portal capture tests (no D-Bus: the pick result is injected).

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use super::super::portal::{cached_result, HostRecord};
use super::super::wayland_shortcut::press_may_cancel;
use super::*;
use crate::screenshot::{
    OverlayWindowSpec, ScreenshotEvents, ScreenshotHotkeyOwner, ScreenshotHotkeyState,
    ShortcutRegistrar,
};

type BoxFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

#[derive(Clone)]
struct WaylandHost {
    capture: ScreenshotCaptureState,
    hotkey: ScreenshotHotkeyState,
    display: DisplayEnvSnapshot,
    log: Arc<Mutex<Vec<String>>>,
    messages: Arc<Mutex<Vec<String>>>,
    spawned: Arc<Mutex<Vec<BoxFuture>>>,
}

impl WaylandHost {
    fn new(display: DisplayEnvSnapshot, state: ScreenshotCaptureLifecycle) -> Self {
        Self {
            capture: Arc::new(tokio::sync::Mutex::new(state)),
            hotkey: Arc::new(Mutex::new(ScreenshotHotkeyRuntime::default())),
            display,
            log: Arc::default(),
            messages: Arc::default(),
            spawned: Arc::default(),
        }
    }

    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }

    fn push(&self, entry: &str) {
        self.log.lock().unwrap().push(entry.to_string());
    }

    async fn drain(&self) {
        let futures: Vec<BoxFuture> = self.spawned.lock().unwrap().drain(..).collect();
        for future in futures {
            future.await;
        }
    }

    async fn state_id(&self) -> Option<Uuid> {
        match &*self.capture.lock().await {
            ScreenshotCaptureLifecycle::Idle => None,
            ScreenshotCaptureLifecycle::Starting(s) => Some(s.id),
            other => panic!("unexpected state {}", lifecycle_name(other)),
        }
    }
}

fn lifecycle_name(state: &ScreenshotCaptureLifecycle) -> &'static str {
    match state {
        ScreenshotCaptureLifecycle::Idle => "Idle",
        ScreenshotCaptureLifecycle::Starting(_) => "Starting",
        ScreenshotCaptureLifecycle::Active(_) => "Active",
        ScreenshotCaptureLifecycle::Finishing(_) => "Finishing",
    }
}

fn wayland() -> DisplayEnvSnapshot {
    DisplayEnvSnapshot {
        xdg_session_type: Some("wayland".to_string()),
        wayland_display: Some("wayland-1".to_string()),
        wayland_socket: None,
        default_socket_present: true,
    }
}

fn x11() -> DisplayEnvSnapshot {
    DisplayEnvSnapshot {
        xdg_session_type: Some("x11".to_string()),
        wayland_display: None,
        wayland_socket: None,
        default_socket_present: false,
    }
}

impl ScreenshotHotkeyOwner for WaylandHost {
    fn hotkey_state(&self) -> ScreenshotHotkeyState {
        Arc::clone(&self.hotkey)
    }
}

impl ScreenshotEvents for WaylandHost {
    fn capture_saved(&self, _result: ScreenshotCaptureResult) {
        self.push("saved");
    }

    fn capture_failed(&self, event: ScreenshotCaptureEvent) {
        self.messages.lock().unwrap().push(event.message);
        self.push("failed");
    }
}

impl ScreenshotHost for WaylandHost {
    fn capture_state(&self) -> ScreenshotCaptureState {
        Arc::clone(&self.capture)
    }

    fn selection(&self) -> crate::session::selection::SelectionCoordinator {
        panic!("wayland test must not access selection")
    }

    fn sessions(&self) -> Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>> {
        panic!("wayland test must not access sessions")
    }

    fn available_monitors(&self) -> Vec<MonitorGeometry> {
        panic!("wayland test must not enumerate monitors")
    }

    fn with_shortcuts<T, F: FnOnce(&dyn ShortcutRegistrar) -> T>(&self, _callback: F) -> T {
        panic!("wayland test must not access shortcuts")
    }

    fn display_snapshot(&self) -> DisplayEnvSnapshot {
        self.display.clone()
    }

    fn open_overlay(&self, _spec: &OverlayWindowSpec) -> Result<(), String> {
        panic!("wayland test must not create overlays")
    }

    fn destroy_capture_overlays(&self, _capture_id: Uuid) -> usize {
        0
    }

    fn copy_path(&self, _path: String) -> Result<(), String> {
        self.push("clipboard");
        Ok(())
    }

    fn focus_capture_failure(&self) {
        self.push("focus");
    }

    fn spawn_background(&self, future: BoxFuture) {
        self.spawned.lock().unwrap().push(future);
    }
}

fn starting(root: &Path) -> ScreenshotCaptureStarting {
    ScreenshotCaptureStarting {
        id: Uuid::new_v4(),
        target_session_id: Uuid::new_v4(),
        target_session_name: "shot".to_string(),
        target_directory: root.to_path_buf(),
        created_at: Utc::now(),
    }
}

fn png(path: &Path, w: u32, h: u32) {
    image::RgbaImage::from_pixel(w, h, image::Rgba([1, 2, 3, 255]))
        .save_with_format(path, image::ImageFormat::Png)
        .unwrap();
}

/// A Wayland host in `Starting` for a fresh capture id.
fn host_starting(root: &Path) -> (WaylandHost, Uuid) {
    let s = starting(root);
    let id = s.id;
    let host = WaylandHost::new(wayland(), ScreenshotCaptureLifecycle::Starting(s));
    (host, id)
}

fn screenshot_files(root: &Path) -> Vec<PathBuf> {
    match std::fs::read_dir(root.join("screenshots")) {
        Ok(dir) => dir.map(|e| e.unwrap().path()).collect(),
        Err(_) => Vec::new(),
    }
}

#[tokio::test]
async fn cancel_returns_to_idle_silently() {
    let (host, id) = host_starting(Path::new("unused"));
    let out = finish_portal_pick(&host, id, Ok(None), 1024).await;
    assert!(matches!(out, Ok(BeginOutcome::Cancelled)));
    assert_eq!(host.state_id().await, None);
    assert!(host.log().is_empty());
}

#[tokio::test]
async fn portal_error_returns_to_idle_and_err() {
    let (host, id) = host_starting(Path::new("unused"));
    let out = finish_portal_pick(&host, id, Err("x".to_string()), 1024).await;
    let Err(message) = out else {
        panic!("expected Err")
    };
    assert!(message.contains('x'), "{message}");
    assert_eq!(host.state_id().await, None);
}

#[tokio::test]
async fn valid_png_is_saved() {
    let (_temp, root) = super::screenshot_replica_fixture();
    let source_dir = tempfile::tempdir().unwrap();
    let source = source_dir.path().join("portal.png");
    png(&source, 4, 4);
    let before = std::fs::read(&source).unwrap();
    let (host, id) = host_starting(&root);

    let out = finish_portal_pick(&host, id, Ok(Some(source.clone())), 1024 * 1024).await;

    assert!(matches!(out, Ok(BeginOutcome::Started)));
    let files = screenshot_files(&root);
    assert_eq!(files.len(), 1, "{files:?}");
    assert_eq!(files[0].extension().unwrap(), "png");
    assert_eq!(host.log(), ["clipboard", "saved"]);
    assert_eq!(host.state_id().await, None);
    assert_eq!(std::fs::read(&source).unwrap(), before);
}

#[tokio::test]
async fn superseded_pick_saves_nothing() {
    let (_temp, root) = super::screenshot_replica_fixture();
    let source_dir = tempfile::tempdir().unwrap();
    let source = source_dir.path().join("portal.png");
    png(&source, 4, 4);
    let (host, other_id) = host_starting(&root);

    let out = finish_portal_pick(&host, Uuid::new_v4(), Ok(Some(source)), 1024 * 1024).await;

    assert!(matches!(out, Ok(BeginOutcome::Busy(None))));
    assert!(!root.join("screenshots").exists());
    assert_eq!(host.state_id().await, Some(other_id));
}

#[tokio::test]
async fn unreadable_sources_fail_and_focus() {
    let (_temp, root) = super::screenshot_replica_fixture();
    let source_dir = tempfile::tempdir().unwrap();
    let dir = source_dir.path();
    let as_dir = dir.join("a-directory");
    std::fs::create_dir(&as_dir).unwrap();
    let garbage = dir.join("garbage.png");
    std::fs::write(&garbage, b"not an image").unwrap();
    let thin = dir.join("thin.png");
    png(&thin, 1, 5);
    let big = dir.join("big.png");
    png(&big, 4, 4);
    // User-granted exception (#2774 P02 only): a symlink to a valid PNG.
    let link = dir.join("link.png");
    std::os::unix::fs::symlink(&big, &link).unwrap();

    let cases: [(PathBuf, u64, &str); 5] = [
        (link, 1024 * 1024, "not a regular file"),
        (as_dir, 1024 * 1024, "not a regular file"),
        (garbage, 1024 * 1024, "not a readable image"),
        (thin, 1024 * 1024, "Selection is too small"),
        (big, 10, "too large"),
    ];
    for (source, max_bytes, expected) in cases {
        let (host, id) = host_starting(&root);
        let out = finish_portal_pick(&host, id, Ok(Some(source)), max_bytes).await;
        assert!(matches!(out, Ok(BeginOutcome::Started)), "{expected}");
        assert!(host
            .log()
            .ends_with(&["failed".to_string(), "focus".to_string()]));
        let messages = host.messages.lock().unwrap().clone();
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert!(messages[0].contains(expected), "{messages:?} vs {expected}");
        assert_eq!(host.state_id().await, None);
        assert!(screenshot_files(&root).is_empty(), "{expected}");
    }
}

#[test]
fn press_may_cancel_rule() {
    let id = Uuid::new_v4();
    assert_eq!(press_may_cancel(true, Some(id)), None);
    assert_eq!(press_may_cancel(false, Some(id)), Some(id));
    assert_eq!(press_may_cancel(true, None), None);
}

#[tokio::test]
async fn repeat_press_on_wayland_never_cancels() {
    let (host, id) = host_starting(Path::new("unused"));
    begin_capture_from_hotkey(host.clone());
    host.drain().await;
    assert_eq!(host.state_id().await, Some(id));
    assert!(host.log().is_empty());
}

#[tokio::test]
async fn repeat_press_on_x11_still_cancels() {
    let s = starting(Path::new("unused"));
    let host = WaylandHost::new(x11(), ScreenshotCaptureLifecycle::Starting(s));
    begin_capture_from_hotkey(host.clone());
    host.drain().await;
    assert_eq!(host.state_id().await, None);
}

#[test]
fn host_record_is_reused_only_for_the_same_owner() {
    let ok = HostRecord {
        owner: ":1.5".to_string(),
        result: Ok(()),
    };
    let failed = HostRecord {
        owner: ":1.5".to_string(),
        result: Err("x".to_string()),
    };
    assert_eq!(cached_result(None, ":1.5"), None);
    assert_eq!(
        cached_result(Some(&ok), ":1.5"),
        Some(Ok(":1.5".to_string()))
    );
    assert_eq!(cached_result(Some(&ok), ":1.9"), None);
    assert_eq!(
        cached_result(Some(&failed), ":1.5"),
        Some(Err("x".to_string()))
    );
    assert_eq!(cached_result(Some(&failed), ":1.9"), None);
}
