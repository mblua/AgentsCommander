use super::*;

#[test]
fn absent_tracker_stays_optional() {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    assert!(app.handle().response_tracker().is_none());
}

#[test]
fn registered_tracker_borrows_the_live_state() {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let handle = app.handle();
    let tracker = ResponseCloseState::default();
    assert!(handle.manage(tracker.clone()));
    let managed = handle.state::<ResponseCloseState>();
    for _ in 0..2 {
        let observed = handle.response_tracker().unwrap();
        assert!(std::ptr::eq(observed, managed.inner()));
        assert!(observed.same_state(&tracker));
        assert_eq!(observed.wiring_identity(), tracker.wiring_identity());
    }
    tracker.fail_closed();
    assert!(!handle.response_tracker().unwrap().is_healthy());
}
