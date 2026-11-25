//! Missing managed state in a pure tick: `tick` returns the absent type's name and keeps
//! dedup, sessions, markers and PTY; the AppHandle cases live in `events::session::tests`.

use super::*;

const LEAD: &str = "myproj:wg-1-team/lead";

#[tokio::test]
async fn tick_with_missing_state_keeps_dedup_sessions_markers_and_pty() {
    use crate::pty::backend::PtyBackend;
    let fixture = auto_close_fixture().await;
    let services = &fixture.services;
    services
        .settings
        .write()
        .await
        .coordinator_auto_close_enabled = false;
    let mut emitted = HashMap::new();
    assert_eq!(tick(services, &mut emitted).await, Ok(()));
    services
        .clocks
        .lock()
        .unwrap()
        .mark_auto_closed(LEAD, ts(1_700_000_500));
    let (dedup, clocks) = (emitted.clone(), services.clocks.lock().unwrap().snapshot());
    let sessions = services.manager.read().await.list_sessions().await.len();
    assert_eq!(sessions, 2);

    let missing = std::any::type_name::<Arc<IdleDetector>>();
    *services.missing.lock().unwrap() = Some(missing);
    assert_eq!(tick(services, &mut emitted).await, Err(missing));
    assert_eq!(emitted, dedup);
    assert_eq!(services.clocks.lock().unwrap().snapshot(), clocks);
    assert_eq!(
        services.manager.read().await.list_sessions().await.len(),
        sessions
    );
    assert!(fixture.backend.has_session(fixture.coordinator_id));
    assert!(fixture.backend.has_session(fixture.member_id));

    *services.missing.lock().unwrap() = None;
    assert_eq!(tick(services, &mut emitted).await, Ok(()));
    let first = DateTime::from_timestamp(fixture.anchor.timestamp(), 0).unwrap();
    assert_eq!(
        *services.notices.lock().unwrap(),
        vec![Badge::Idle(fixture.cwd.clone(), first)],
        "valid, missing, valid with the same anchor publishes once"
    );
    assert!(services.saved.lock().unwrap().is_empty());
    services.coordinator.close_and_join().await;
}
