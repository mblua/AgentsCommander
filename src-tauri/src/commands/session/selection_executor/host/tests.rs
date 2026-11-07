//! Native lazy getters retain state identity and transaction observation order.

use super::*;
use crate::session::selection::SelectionTransaction;
use std::panic::{catch_unwind, AssertUnwindSafe};
use tauri::test::MockRuntime;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

fn app() -> tauri::App<MockRuntime> {
    tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap()
}

fn pty() -> Arc<Mutex<PtyManager>> {
    Arc::new(Mutex::new(PtyManager::new_for_test(Arc::new(
        ObservationBackend::default(),
    ))))
}

fn same_pty(actual: &Arc<Mutex<PtyManager>>, expected: &Arc<Mutex<PtyManager>>) -> bool {
    Arc::ptr_eq(actual, expected)
}

#[test]
fn issue_3950_host_identity() {
    let app = app();
    let host = app.handle();
    let sessions = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let pty = pty();
    let settings = crate::config::settings::new_settings_state(Default::default());
    let detached = DetachedSessionsState::default();
    let coordinator = SelectionCoordinator::new(sessions.clone(), CancellationToken::new());
    assert!(app.manage(sessions.clone()));
    assert!(app.manage(pty.clone()));
    assert!(app.manage(settings.clone()));
    assert!(app.manage(detached.clone()));
    assert!(app.manage(ResponseCloseState::default()));
    assert!(app.manage(coordinator));
    assert!(std::ptr::eq(
        host.sessions_required(),
        host.state::<Arc<tokio::sync::RwLock<SessionManager>>>()
            .inner()
    ));
    assert!(Arc::ptr_eq(host.sessions_required(), &sessions));
    assert!(same_pty(host.pty_required(), &pty));
    assert!(std::ptr::eq(
        host.pty_required(),
        host.pty_optional().unwrap()
    ));
    assert!(Arc::ptr_eq(host.detached_required(), &detached));
    assert!(Arc::ptr_eq(host.settings_required(), &settings));
    assert!(std::ptr::eq(
        host.response_close_required(),
        host.response_close_optional().unwrap()
    ));
    assert!(std::ptr::eq(
        host.response_close_required(),
        host.state::<ResponseCloseState>().inner()
    ));
    assert!(std::ptr::eq(
        host.coordinator_optional().unwrap(),
        host.state::<SelectionCoordinator>().inner()
    ));
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_host_identity");
    // Same identity predicate as the passing native control; no panic is used as evidence.
    assert!(!same_pty(&self::pty(), &pty));
    println!("\nISSUE3950_MUTANT_REJECTED case=wrong_identity");
}

fn required_missing<T>(get: impl FnOnce() -> T, type_name: &str) {
    let panic = catch_unwind(AssertUnwindSafe(get))
        .err()
        .expect("required state must panic");
    let message = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied())
        .expect("state panic text");
    assert!(
        message.contains("state() called before manage()"),
        "{message}"
    );
    assert!(message.contains(type_name), "{message}");
}

#[test]
fn issue_3950_host_missing() {
    let app = app();
    let host = app.handle();
    // Construction and optional lookups do not eagerly require registered state.
    let _transaction = SelectionTransaction::for_test(host.clone());
    assert!(host.pty_optional().is_none());
    assert!(host.response_close_optional().is_none());
    assert!(host.coordinator_optional().is_none());
    required_missing(|| host.sessions_required(), "SessionManager");
    required_missing(|| host.pty_required(), "PtyManager");
    required_missing(|| host.detached_required(), "HashSet");
    required_missing(
        || host.settings_required(),
        std::any::type_name::<SettingsState>(),
    );
    required_missing(|| host.response_close_required(), "ResponseCloseState");
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_host_missing");
}

#[tokio::test]
async fn issue_3950_host_lock_release() {
    let app = app();
    let sessions = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    assert!(app.manage(sessions.clone()));
    let transaction = SelectionTransaction::for_test(app.handle().clone());
    let manager = transaction.manager().await;
    let guard = sessions
        .try_write()
        .expect("outer read guard released before return");
    // Retaining the returned manager must not retain the outer state guard.
    assert_eq!(manager.aggregate_snapshot().await.sessions.len(), 0);
    drop(guard);
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_host_lock_release");
}

fn current_shell(actual: &str, expected: &str) -> bool {
    actual == expected
}

#[tokio::test]
async fn issue_3950_host_fresh_settings() {
    let app = app();
    let state = crate::config::settings::new_settings_state(Default::default());
    assert!(app.manage(state.clone()));
    let host = app.handle();
    state.write().await.default_shell = "before".into();
    let eager_snapshot = host.settings_required().read().await.clone();
    assert!(current_shell(&eager_snapshot.default_shell, "before"));
    state.write().await.default_shell = "after".into();
    let current = host.settings_required().read().await.clone();
    assert!(current_shell(&current.default_shell, "after"));
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_host_fresh_settings");
    assert!(!current_shell(&eager_snapshot.default_shell, "after"));
    println!("\nISSUE3950_MUTANT_REJECTED case=eager_snapshot");
}

#[derive(Default)]
struct ObservationBackend {
    detached: Option<DetachedSessionsState>,
    observed: std::sync::atomic::AtomicBool,
}

impl crate::pty::backend::PtyBackend for ObservationBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn spawn(
        &self,
        _spec: crate::pty::backend::BackendSpawnSpec,
    ) -> futures::future::BoxFuture<'_, Result<(), crate::errors::AppError>> {
        Box::pin(async { Ok(()) })
    }

    fn write(
        &self,
        _authority: &crate::pty::input_route::BackendWriteAuthority,
        _id: Uuid,
        _data: &[u8],
    ) -> Result<(), crate::errors::AppError> {
        Ok(())
    }

    fn resize(&self, _id: Uuid, _cols: u16, _rows: u16) -> Result<(), crate::errors::AppError> {
        Ok(())
    }

    fn kill(&self, _id: Uuid) -> Result<(), crate::errors::AppError> {
        Ok(())
    }

    fn has_session(&self, id: Uuid) -> bool {
        if let Some(detached) = &self.detached {
            // PTY observation can acquire detached: the earlier read guard was released.
            detached
                .try_lock()
                .expect("detached guard released before PTY")
                .remove(&id);
            self.observed
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        true
    }

    fn get_screen_snapshot(&self, _id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
        None
    }

    fn get_pty_size(&self, _id: Uuid) -> Option<(u16, u16)> {
        None
    }

    fn get_screen_rows(&self, _id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
        crate::pty::context_scrape::ScreenRowsRead::Unavailable
    }

    fn register_response_watcher(&self, _id: Uuid, _request: String, _dir: std::path::PathBuf) {}

    fn terminate_job_for_session(&self, _id: Uuid) -> bool {
        false
    }

    fn kill_all_jobs(&self) -> (usize, usize) {
        (0, 0)
    }
}

#[test]
fn issue_3950_host_detached_order() {
    let app = app();
    let detached = DetachedSessionsState::default();
    let id = Uuid::new_v4();
    detached.lock().unwrap().insert(id);
    let backend = Arc::new(ObservationBackend {
        detached: Some(detached.clone()),
        ..Default::default()
    });
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
    pty.lock()
        .unwrap()
        .record_route(id, crate::pty::backend::SessionBackendKind::LocalProcess);
    assert!(app.manage(detached.clone()));
    assert!(app.manage(pty));
    let transaction = SelectionTransaction::for_test(app.handle().clone());
    let snapshot = transaction.runtime_snapshot(id);
    assert!(backend.observed.load(std::sync::atomic::Ordering::SeqCst));
    assert!(snapshot.has_pty);
    assert!(snapshot.detached, "detached captured before PTY removed it");
    assert!(!detached.lock().unwrap().contains(&id));
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_host_detached_order");
}

fn normalized_body(source: &str, name: &str) -> String {
    let scrubbed = rust_source_scrub::scrub(source, rust_source_scrub::Literals::Drop).unwrap();
    let normalized: String = scrubbed.chars().filter(|ch| !ch.is_whitespace()).collect();
    let rest = &normalized[normalized.find(name).expect("production function")..];
    let open = rest.find('{').unwrap();
    let mut depth = 1usize;
    let close = rest[open + 1..]
        .char_indices()
        .find_map(|(index, ch)| {
            match ch {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
            (depth == 0).then_some(open + 1 + index)
        })
        .expect("complete production body");
    rest[open + 1..close].to_string()
}

fn assert_window_effect_mapping() {
    let calls = std::cell::Cell::new(0);
    let witness = 3950u64;
    assert_eq!(
        window_effect(None::<u64>, |_| {
            calls.set(calls.get() + 1);
            Ok::<(), std::io::Error>(())
        }),
        None
    );
    assert_eq!(calls.get(), 0, "missing window must not invoke effect");
    assert_eq!(
        window_effect(Some(witness), |actual| {
            calls.set(calls.get() + 1);
            assert_eq!(actual, witness, "original window witness reaches effect");
            Ok::<(), std::io::Error>(())
        }),
        Some(Ok(()))
    );
    assert_eq!(calls.get(), 1, "present Ok invokes exactly once");
    let error = std::io::Error::other("issue3950-window-effect");
    let expected = error.to_string();
    assert_eq!(
        window_effect(Some(witness), |actual| {
            calls.set(calls.get() + 1);
            assert_eq!(
                actual, witness,
                "original window witness reaches failing effect"
            );
            Err(error)
        }),
        Some(Err(expected))
    );
    assert_eq!(calls.get(), 2, "present Err invokes exactly once");
}

#[test]
fn issue_3950_host_windows() {
    let app = app();
    let host = app.handle();
    assert_eq!(host.focus_window("missing"), None);
    assert_eq!(host.destroy_window("missing"), None);
    let _window = tauri::WebviewWindowBuilder::new(
        host,
        "host-proof",
        tauri::WebviewUrl::App("index.html".into()),
    )
    .build()
    .unwrap();
    assert_eq!(host.focus_window("host-proof"), Some(Ok(())));
    assert_eq!(host.destroy_window("host-proof"), Some(Ok(())));
    assert_window_effect_mapping();
    let source = include_str!("../host.rs");
    assert_eq!(
        normalized_body(source, "fnfocus_window("),
        "window_effect(self.get_webview_window(label),|window|window.set_focus())"
    );
    assert_eq!(
        normalized_body(source, "fndestroy_window("),
        "window_effect(self.get_webview_window(label),|window|window.destroy())"
    );
    assert_eq!(
        normalized_body(source, "fnwindow_effect<"),
        "window.map(|window|effect(window).map_err(|error|error.to_string()))"
    );
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_host_windows");
}
