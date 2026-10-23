use super::*;

// #2482 - the quota engine's registration and retirement at the two chokepoints. The
// watcher engine is built from three no-op fakes: these tests assert REGISTRATION, never
// a tick, so the engine is never started and the fakes are never called.
struct NoopWatcherBackends;
impl crate::pty::watchers::WatcherBackendSource for NoopWatcherBackends {
    fn reader_for(&self, _id: Uuid) -> Option<crate::pty::watchers::SessionFrameReader> {
        None
    }
    fn liveness(&self, _id: Uuid) -> crate::pty::context_scrape::ContextSessionLiveness {
        crate::pty::context_scrape::ContextSessionLiveness::Unavailable
    }
}
struct NoopWatcherPatterns;
impl crate::pty::watchers::WatcherPatternSource for NoopWatcherPatterns {
    fn resolve(
        &self,
    ) -> futures::future::BoxFuture<
        '_,
        Option<HashMap<String, crate::pty::watchers::AgentResolution>>,
    > {
        Box::pin(async { Some(HashMap::new()) })
    }
}
struct NoopWatcherSink;
impl crate::pty::watchers::WatcherEventSink for NoopWatcherSink {
    fn emit(&self, _batch: crate::pty::watchers::WatcherMatchBatch) {}
}

fn watcher_engine() -> Arc<crate::pty::watchers::WatcherEngine> {
    crate::pty::watchers::WatcherEngine::new(
        Arc::new(NoopWatcherBackends),
        Arc::new(NoopWatcherPatterns),
        Arc::new(NoopWatcherSink),
        Arc::new(crate::pty::watchers::history::WatcherHistory::default()),
    )
}

fn quota_app(
    quota: &crate::pty::agent_quota::test_support::QuotaHarness,
) -> tauri::App<tauri::test::MockRuntime> {
    tauri::test::mock_builder()
        .manage(Arc::clone(&quota.engine))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build a mock app")
}

#[test]
fn register_session_samplers_registers_the_quota_engine_when_managed() {
    let quota = crate::pty::agent_quota::test_support::QuotaHarness::new();
    let app = quota_app(&quota);
    let id = Uuid::new_v4();

    super::super::register_session_samplers(app.handle(), id, Some("claude".to_string()));

    assert!(quota.engine.is_session_registered(id));
}

#[test]
fn register_session_samplers_with_no_agent_registers_no_quota_engine() {
    let quota = crate::pty::agent_quota::test_support::QuotaHarness::new();
    let app = quota_app(&quota);
    let id = Uuid::new_v4();

    super::super::register_session_samplers(app.handle(), id, None);

    assert!(!quota.engine.is_session_registered(id));
}

#[test]
fn register_session_samplers_does_not_panic_when_the_quota_engine_is_unmanaged() {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build a mock app");

    super::super::register_session_samplers(
        app.handle(),
        Uuid::new_v4(),
        Some("claude".to_string()),
    );
    super::super::purge_session_side_state(app.handle(), Uuid::new_v4());
}

#[test]
fn purge_session_side_state_retires_the_quota_engine() {
    let quota = crate::pty::agent_quota::test_support::QuotaHarness::new();
    let app = quota_app(&quota);
    let id = Uuid::new_v4();
    super::super::register_session_samplers(app.handle(), id, Some("claude".to_string()));
    assert!(quota.engine.is_session_registered(id));

    super::super::purge_session_side_state(app.handle(), id);

    assert!(!quota.engine.is_session_registered(id));
}

/// A regression net on the `agent_id` clone chain: a quota consumer inserted beside the
/// watcher block is exactly how the watcher registration would stop receiving an id.
#[test]
fn register_session_samplers_still_registers_the_watcher_engine() {
    let quota = crate::pty::agent_quota::test_support::QuotaHarness::new();
    let watchers = watcher_engine();
    let app = tauri::test::mock_builder()
        .manage(Arc::clone(&quota.engine))
        .manage(Arc::clone(&watchers))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build a mock app");
    let id = Uuid::new_v4();

    super::super::register_session_samplers(app.handle(), id, Some("claude".to_string()));

    assert!(watchers.is_session_registered(id));
    assert!(quota.engine.is_session_registered(id));
}
