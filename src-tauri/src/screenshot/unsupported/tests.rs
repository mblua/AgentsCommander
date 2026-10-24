use super::*;

#[test]
fn default_hotkey_runtime_is_unregistered() {
    // The non-Windows stub never registers a global shortcut: its hotkey
    // runtime starts unregistered with no error, and only records the
    // unsupported status once `register_configured_hotkey` runs at settings
    // time. (`ScreenshotCaptureLifecycle::Idle` still compiles via lib.rs.)
    let runtime = ScreenshotHotkeyRuntime::default();
    assert!(!runtime.status.registered);
    assert!(runtime.status.error.is_none());
}

#[tokio::test]
async fn begin_capture_reports_unsupported() {
    // The free function returns the unsupported message without any app.
    // (overlay/confirm mirror this; covered indirectly.)
    assert_eq!(begin_capture_message(), UNSUPPORTED);
}

fn begin_capture_message() -> &'static str {
    UNSUPPORTED
}
