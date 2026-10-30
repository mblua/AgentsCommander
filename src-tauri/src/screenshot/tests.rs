use super::*;

#[test]
fn accepts_ctrl_q_case_insensitive() {
    for value in ["Ctrl+Q", "ctrl+q", "Control+Q", "CTRL+Q", "  Ctrl + q "] {
        let parsed = parse_screenshot_hotkey(value)
            .unwrap_or_else(|e| panic!("expected '{value}' to parse: {e}"));
        assert_eq!(parsed.modifier, HotkeyModifier::Ctrl);
        assert_eq!(parsed.key, 'Q', "value {value}");
    }
}

#[test]
fn accepts_alphanumeric_keys() {
    assert_eq!(parse_screenshot_hotkey("Ctrl+1").unwrap().key, '1');
    assert_eq!(parse_screenshot_hotkey("Control+a").unwrap().key, 'A');
}

#[test]
fn rejects_empty_and_whitespace() {
    assert!(parse_screenshot_hotkey("").is_err());
    assert!(parse_screenshot_hotkey("   ").is_err());
}

#[test]
fn rejects_missing_modifier_or_key() {
    assert!(parse_screenshot_hotkey("Q").is_err());
    assert!(parse_screenshot_hotkey("Ctrl+").is_err());
    assert!(parse_screenshot_hotkey("+Q").is_err());
}

#[test]
fn rejects_unknown_modifier() {
    assert!(parse_screenshot_hotkey("Hyper+Q").is_err());
    assert!(parse_screenshot_hotkey("Alt+Q").is_err());
    assert!(parse_screenshot_hotkey("Shift+Q").is_err());
}

#[test]
fn rejects_multi_modifier_and_multi_key() {
    // MVP accepts exactly one modifier plus one key.
    assert!(parse_screenshot_hotkey("Ctrl+Shift+Q").is_err());
    assert!(parse_screenshot_hotkey("Ctrl+QQ").is_err());
    assert!(parse_screenshot_hotkey("Ctrl+F1").is_err());
}

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
#[derive(Clone)]
struct ConfirmationHost {
    state: ScreenshotCaptureState,
    capture_id: uuid::Uuid,
    finishing: uuid::Uuid,
    order: Arc<std::sync::Mutex<Vec<&'static str>>>,
    fail_clipboard: bool,
    partial: Option<std::path::PathBuf>,
}

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
impl ConfirmationHost {
    fn new(capture_id: uuid::Uuid, finishing: uuid::Uuid, fail_clipboard: bool) -> Self {
        Self {
            state: Arc::new(tokio::sync::Mutex::new(
                ScreenshotCaptureLifecycle::Finishing(finishing),
            )),
            capture_id,
            finishing,
            order: Arc::new(std::sync::Mutex::new(Vec::new())),
            fail_clipboard,
            partial: None,
        }
    }

    fn record(&self, step: &'static str) {
        let state = self
            .state
            .try_lock()
            .expect("completion holds no lifecycle lock");
        assert!(matches!(
            &*state,
            ScreenshotCaptureLifecycle::Finishing(id) if *id == self.finishing
        ));
        drop(state);
        self.order.lock().unwrap().push(step);
    }
}

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
impl ScreenshotHotkeyOwner for ConfirmationHost {
    fn hotkey_state(&self) -> ScreenshotHotkeyState {
        panic!("completion must not access hotkey state")
    }
}

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
impl ScreenshotEvents for ConfirmationHost {
    fn capture_saved(&self, result: ScreenshotCaptureResult) {
        assert_eq!(result.path, "synthetic.png");
        self.record("saved");
    }

    fn capture_failed(&self, event: ScreenshotCaptureEvent) {
        assert_eq!(event.message, "synthetic save failure");
        self.record("failed");
    }
}

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
impl ScreenshotHost for ConfirmationHost {
    fn capture_state(&self) -> ScreenshotCaptureState {
        Arc::clone(&self.state)
    }

    fn selection(&self) -> crate::session::selection::SelectionCoordinator {
        panic!("completion must not access selection")
    }

    fn sessions(&self) -> Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>> {
        panic!("completion must not access sessions")
    }

    fn available_monitors(&self) -> Vec<MonitorGeometry> {
        panic!("completion must not enumerate monitors")
    }

    fn with_shortcuts<T, F: FnOnce(&dyn ShortcutRegistrar) -> T>(&self, _callback: F) -> T {
        panic!("completion must not access shortcuts")
    }

    #[cfg(target_os = "linux")]
    fn display_snapshot(&self) -> DisplayEnvSnapshot {
        panic!("completion must not access display state")
    }

    fn open_overlay(&self, _spec: &OverlayWindowSpec) -> Result<(), String> {
        panic!("completion must not create overlays")
    }

    fn destroy_capture_overlays(&self, capture_id: uuid::Uuid) -> usize {
        assert_eq!(capture_id, self.capture_id);
        if let Some(partial) = &self.partial {
            assert!(
                !partial.exists(),
                "partial cleanup must precede overlay teardown"
            );
        }
        self.record("destroy");
        1
    }

    fn copy_path(&self, path: String) -> Result<(), String> {
        assert_eq!(path, "synthetic.png");
        self.record("clipboard");
        if self.fail_clipboard {
            Err("synthetic clipboard failure".into())
        } else {
            Ok(())
        }
    }

    fn focus_capture_failure(&self) {
        panic!("confirmation must not focus a window")
    }

    fn spawn_background(
        &self,
        _future: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>,
    ) {
        panic!("completion must not spawn native work")
    }
}

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn confirmation_success_keeps_finishing_through_teardown() {
    for fail_clipboard in [false, true] {
        let id = uuid::Uuid::new_v4();
        let host = ConfirmationHost::new(id, id, fail_clipboard);
        let saved = ScreenshotCaptureResult {
            path: "synthetic.png".into(),
            session_id: "synthetic-session".into(),
            session_name: "synthetic".into(),
        };
        let result = super::native::complete_confirmation(&host, &host.state, id, Ok(saved))
            .await
            .expect("clipboard failure must not fail a completed save");
        assert_eq!(result.path, "synthetic.png");
        assert_eq!(
            *host.order.lock().unwrap(),
            ["clipboard", "saved", "destroy"]
        );
        assert!(matches!(
            &*host.state.lock().await,
            ScreenshotCaptureLifecycle::Idle
        ));
    }
}

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn confirmation_failure_keeps_finishing_through_teardown() {
    let temp = tempfile::tempdir().expect("synthetic partial directory");
    let partial = temp.path().join("partial.tmp");
    std::fs::write(&partial, "synthetic partial").expect("synthetic partial file");
    let id = uuid::Uuid::new_v4();
    let mut host = ConfirmationHost::new(id, id, false);
    host.partial = Some(partial.clone());
    let result = super::native::complete_confirmation(
        &host,
        &host.state,
        id,
        Err((Some(partial), "synthetic save failure".into())),
    )
    .await;
    assert_eq!(result.unwrap_err(), "synthetic save failure");
    assert_eq!(*host.order.lock().unwrap(), ["destroy", "failed"]);
    assert!(matches!(
        &*host.state.lock().await,
        ScreenshotCaptureLifecycle::Idle
    ));
}

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn confirmation_stale_id_preserves_other_finishing_capture() {
    let id = uuid::Uuid::new_v4();
    let other = uuid::Uuid::new_v4();
    let host = ConfirmationHost::new(id, other, false);
    let saved = ScreenshotCaptureResult {
        path: "synthetic.png".into(),
        session_id: "synthetic-session".into(),
        session_name: "synthetic".into(),
    };
    super::native::complete_confirmation(&host, &host.state, id, Ok(saved))
        .await
        .expect("already-computed result");
    assert_eq!(
        *host.order.lock().unwrap(),
        ["clipboard", "saved", "destroy"]
    );
    assert!(matches!(
        &*host.state.lock().await,
        ScreenshotCaptureLifecycle::Finishing(current) if *current == other
    ));
}
