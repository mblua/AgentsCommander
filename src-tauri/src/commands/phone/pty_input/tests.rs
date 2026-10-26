use super::*;
use crate::errors::AppError;
use crate::pty::backend::{BackendSpawnSpec, PtyBackend};
use std::sync::atomic::AtomicBool;
use uuid::Uuid;

struct NoopBackend;

impl PtyBackend for NoopBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn spawn(&self, _: BackendSpawnSpec) -> futures::future::BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async { Ok(()) })
    }

    fn write(
        &self,
        _: &crate::pty::input_route::BackendWriteAuthority,
        _: Uuid,
        _: &[u8],
    ) -> Result<(), AppError> {
        Ok(())
    }

    fn resize(&self, _: Uuid, _: u16, _: u16) -> Result<(), AppError> {
        Ok(())
    }

    fn kill(&self, _: Uuid) -> Result<(), AppError> {
        Ok(())
    }

    fn has_session(&self, _: Uuid) -> bool {
        false
    }

    fn get_screen_snapshot(&self, _: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
        None
    }

    fn get_pty_size(&self, _: Uuid) -> Option<(u16, u16)> {
        None
    }

    fn get_screen_rows(&self, _: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
        crate::pty::context_scrape::ScreenRowsRead::SessionOver
    }

    fn register_response_watcher(&self, _: Uuid, _: String, _: std::path::PathBuf) {}

    fn terminate_job_for_session(&self, _: Uuid) -> bool {
        false
    }

    fn kill_all_jobs(&self) -> (usize, usize) {
        (0, 0)
    }
}

#[test]
fn empty_registry_preserves_optional_absence_and_required_panics() {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let handle = app.handle();
    assert!(handle.pty_restore().is_none());
    assert!(handle.pty_purge().is_none());
    for getter in [
        (|h: &tauri::AppHandle<tauri::test::MockRuntime>| {
            let _ = h.pty_sessions();
        }) as fn(&tauri::AppHandle<tauri::test::MockRuntime>),
        |h| {
            let _ = h.pty_routes();
        },
        |h| {
            let _ = h.pty_settings();
        },
        |h| {
            let _ = h.pty_readiness();
        },
    ] {
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| getter(handle))).is_err());
    }
}

#[tokio::test]
async fn managed_getters_borrow_live_objects_without_cloning_arcs() {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let handle = app.handle();
    assert!(handle.pty_restore().is_none());
    assert!(handle.pty_purge().is_none());
    let sessions = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let routes = Arc::new(Mutex::new(PtyManager::new_for_test(Arc::new(NoopBackend))));
    let settings = crate::config::settings::new_settings_state(Default::default());
    let readiness = IdleDetector::new(|_| {}, |_| {});
    let restore = Arc::new(RestoreInProgress(AtomicBool::new(false)));
    let purge = Arc::new(PurgeGuard::default());
    assert!(handle.manage(Arc::clone(&sessions)));
    assert!(handle.manage(Arc::clone(&routes)));
    assert!(handle.manage(settings.clone()));
    assert!(handle.manage(Arc::clone(&readiness)));
    assert!(handle.manage(Arc::clone(&restore)));
    assert!(handle.manage(Arc::clone(&purge)));
    let counts = [
        Arc::strong_count(&sessions),
        Arc::strong_count(&routes),
        Arc::strong_count(&settings),
        Arc::strong_count(&readiness),
        Arc::strong_count(&restore),
        Arc::strong_count(&purge),
    ];
    for _ in 0..2 {
        assert!(Arc::ptr_eq(handle.pty_sessions(), &sessions));
        assert!(Arc::ptr_eq(handle.pty_routes(), &routes));
        assert!(Arc::ptr_eq(handle.pty_settings(), &settings));
        assert!(Arc::ptr_eq(handle.pty_readiness(), &readiness));
        assert!(Arc::ptr_eq(handle.pty_restore().unwrap(), &restore));
        assert!(Arc::ptr_eq(handle.pty_purge().unwrap(), &purge));
        assert!(std::ptr::eq(handle.pty_sessions(), handle.pty_sessions()));
        assert!(std::ptr::eq(handle.pty_routes(), handle.pty_routes()));
        assert!(std::ptr::eq(handle.pty_settings(), handle.pty_settings()));
        assert!(std::ptr::eq(handle.pty_readiness(), handle.pty_readiness()));
        assert!(std::ptr::eq(
            handle.pty_restore().unwrap(),
            handle.pty_restore().unwrap()
        ));
        assert!(std::ptr::eq(
            handle.pty_purge().unwrap(),
            handle.pty_purge().unwrap()
        ));
    }
    assert_eq!(
        counts,
        [
            Arc::strong_count(&sessions),
            Arc::strong_count(&routes),
            Arc::strong_count(&settings),
            Arc::strong_count(&readiness),
            Arc::strong_count(&restore),
            Arc::strong_count(&purge),
        ]
    );
    restore.0.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(handle
        .pty_restore()
        .unwrap()
        .0
        .load(std::sync::atomic::Ordering::SeqCst));
    settings.write().await.project_paths.push("live".into());
    assert_eq!(
        handle.pty_settings().read().await.project_paths,
        vec!["live"]
    );
}
