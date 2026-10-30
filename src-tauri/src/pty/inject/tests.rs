use super::*;
use crate::config::settings::{new_settings_state, AppSettings, SettingsState};
use crate::pty::backend::{BackendSpawnSpec, PtyBackend, SessionBackendKind};
use chrono::Utc;
use std::collections::VecDeque;

mod capability;
mod clear_watch;
mod hold;
mod seam;
mod submit;

#[derive(Default)]
struct RecordingBackend {
    writes: Mutex<Vec<(Uuid, Vec<u8>)>>,
    watch_barrier: Mutex<Option<Arc<tokio::sync::Barrier>>>,
    frames: Mutex<VecDeque<Option<Vec<String>>>>,
    capture_hook: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    write_faults: Mutex<VecDeque<bool>>,
    watch_timing: Mutex<Option<(std::time::Duration, std::time::Duration)>>,
    /// #2586 - scripted screen rows. `None` keeps the `SessionOver` answer,
    /// `Some(None)` answers `Unavailable`, `Some(Some(rows))` answers `Rows`.
    screen_rows: Mutex<Option<Option<Vec<String>>>>,
}

impl PtyBackend for RecordingBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn spawn(
        &self,
        _spec: BackendSpawnSpec,
    ) -> futures::future::BoxFuture<'_, Result<(), crate::errors::AppError>> {
        Box::pin(async { Ok(()) })
    }

    fn write(
        &self,
        _authority: &crate::pty::manager::BackendWriteAuthority,
        id: Uuid,
        data: &[u8],
    ) -> Result<(), crate::errors::AppError> {
        self.writes.lock().unwrap().push((id, data.to_vec()));
        if self
            .write_faults
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(false)
        {
            return Err(crate::errors::AppError::PtyError(
                "scripted write failure".into(),
            ));
        }
        Ok(())
    }

    fn resize(&self, _id: Uuid, _cols: u16, _rows: u16) -> Result<(), crate::errors::AppError> {
        Ok(())
    }

    fn kill(&self, _id: Uuid) -> Result<(), crate::errors::AppError> {
        Ok(())
    }

    fn has_session(&self, _id: Uuid) -> bool {
        true
    }

    fn get_screen_snapshot(&self, _id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
        None
    }

    fn get_pty_size(&self, _id: Uuid) -> Option<(u16, u16)> {
        None
    }

    fn get_screen_rows(&self, _id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
        use crate::pty::context_scrape::ScreenRowsRead;
        let hook = self.capture_hook.lock().unwrap().take();
        if let Some(hook) = hook {
            hook();
        }
        if let Some(frame) = self.frames.lock().unwrap().pop_front() {
            return match frame {
                Some(rows) => ScreenRowsRead::Rows(rows),
                None => ScreenRowsRead::Unavailable,
            };
        }
        match self.screen_rows.lock().unwrap().clone() {
            None => ScreenRowsRead::SessionOver,
            Some(None) => ScreenRowsRead::Unavailable,
            Some(Some(rows)) => ScreenRowsRead::Rows(rows),
        }
    }

    fn register_response_watcher(
        &self,
        _session_id: Uuid,
        _request_id: String,
        _response_dir: std::path::PathBuf,
    ) {
    }

    fn terminate_job_for_session(&self, _id: Uuid) -> bool {
        false
    }

    fn kill_all_jobs(&self) -> (usize, usize) {
        (0, 0)
    }
}

async fn typing_hold_app(
    shell: &str,
) -> (
    tauri::App<tauri::test::MockRuntime>,
    Uuid,
    Arc<RecordingBackend>,
) {
    let session_manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let session = session_manager
        .read()
        .await
        .create_session(
            shell.to_string(),
            Vec::new(),
            "C:\\test".to_string(),
            None,
            None,
            Vec::new(),
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    let id = session.id;
    let backend = Arc::new(RecordingBackend::default());
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
    pty.lock()
        .unwrap()
        .record_route(id, SessionBackendKind::LocalProcess);
    let settings_state: SettingsState = new_settings_state(AppSettings::default());
    let app = tauri::test::mock_builder()
        .manage(session_manager)
        .manage(pty)
        .manage(crate::pty::input_activity::new_typing_hold_state())
        .manage(settings_state)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    (app, id, backend)
}

fn recorded_writes(backend: &Arc<RecordingBackend>, id: Uuid) -> Vec<Vec<u8>> {
    backend
        .writes
        .lock()
        .unwrap()
        .iter()
        .filter(|(write_id, _)| *write_id == id)
        .map(|(_, bytes)| bytes.clone())
        .collect()
}

// #2586 - submit-seam observation sink. Keyed by session id so parallel
// tests never read each other's observations.
static SEAM_OBSERVATIONS: Mutex<Vec<(Uuid, String)>> = Mutex::new(Vec::new());

pub(super) fn record_seam_observation(session_id: Uuid, observation: String) {
    SEAM_OBSERVATIONS
        .lock()
        .unwrap()
        .push((session_id, observation));
}

pub(crate) fn configure_clear_watch(watch: &mut super::clear_watch::ClearWatch) {
    if let Some(timing) = watch
        .backend()
        .as_any()
        .downcast_ref::<RecordingBackend>()
        .and_then(|backend| *backend.watch_timing.lock().unwrap())
    {
        watch.test_timing(timing.0, timing.1);
    }
    let barrier = watch
        .backend()
        .as_any()
        .downcast_ref::<RecordingBackend>()
        .and_then(|backend| backend.watch_barrier.lock().unwrap().clone());
    watch.test_barrier(barrier);
}

#[derive(Clone, Copy)]
enum WatchCase {
    Healthy,
    FailureT1,
    FailureAfterT2,
    BaselineUnavailable,
    IntermediateUnavailable,
    FinalUnavailable,
    RequiredEnterFault,
    RedundantEnterFault,
    Legacy,
    WrongProvider,
    WrongCommand,
}

struct WatchRun {
    result: Result<(), String>,
    writes: Vec<Vec<u8>>,
    callbacks: usize,
    armed: bool,
}

async fn run_clear_watch(case: WatchCase) -> WatchRun {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    let shell = if matches!(case, WatchCase::WrongProvider) {
        "claude"
    } else {
        "codex"
    };
    let (app, id, backend) = typing_hold_app(shell).await;
    *backend.screen_rows.lock().unwrap() = Some(Some(Vec::new()));
    *backend.watch_timing.lock().unwrap() =
        Some((Duration::from_millis(1700), Duration::from_millis(10)));
    let failure = || {
        Some(vec![
            "■ Failed to start a fresh session through the app server:".to_string(),
        ])
    };
    let empty = || Some(Vec::new());
    let frames = match case {
        WatchCase::FailureT1 => vec![empty(), failure()],
        WatchCase::FailureAfterT2 => vec![empty(), empty(), empty(), failure()],
        WatchCase::BaselineUnavailable => vec![None],
        WatchCase::IntermediateUnavailable => vec![empty(), None],
        WatchCase::FinalUnavailable => {
            let mut frames = vec![empty(); 3];
            // The remaining loop's first frame is unavailable; a delayed final frame
            // is separately proven through the synthetic deadline tests.
            frames.push(None);
            frames
        }
        _ => Vec::new(),
    };
    *backend.frames.lock().unwrap() = frames.into();
    if matches!(
        case,
        WatchCase::RequiredEnterFault | WatchCase::RedundantEnterFault
    ) {
        *backend.write_faults.lock().unwrap() = if matches!(case, WatchCase::RequiredEnterFault) {
            vec![false, true].into()
        } else {
            vec![false, false, true].into()
        };
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let callback_calls = Arc::clone(&calls);
    let text = if matches!(case, WatchCase::WrongCommand) {
        "/compact"
    } else {
        "/clear"
    };
    let result = if matches!(case, WatchCase::Legacy) {
        inject_peer_wake_with_current_facts(
            app.handle(),
            id,
            text,
            "watch",
            SettleReadiness::Ready,
            |_, _| Ok(()),
        )
        .await
    } else {
        inject_peer_codex_clear_with_current_facts(
            app.handle(),
            id,
            text,
            "watch",
            SettleReadiness::Ready,
            |_, _| Ok(()),
            move || {
                callback_calls.fetch_add(1, Ordering::SeqCst);
            },
        )
        .await
    };
    let manager = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();
    let armed = manager.get_session(id).await.unwrap().agent_turn_armed;
    WatchRun {
        result,
        writes: recorded_writes(&backend, id),
        callbacks: calls.load(Ordering::SeqCst),
        armed,
    }
}

async fn clear_watch_concurrency(remove: bool, poison: bool, cancel: bool) -> Result<(), String> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    let (app, id, backend) = typing_hold_app("codex").await;
    *backend.screen_rows.lock().unwrap() = Some(Some(Vec::new()));
    *backend.watch_timing.lock().unwrap() =
        Some((Duration::from_millis(1700), Duration::from_millis(10)));
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    *backend.watch_barrier.lock().unwrap() = Some(Arc::clone(&barrier));
    let submitted = Arc::new(tokio::sync::Notify::new());
    let notice = Arc::clone(&submitted);
    let calls = Arc::new(AtomicUsize::new(0));
    let callback_calls = Arc::clone(&calls);
    let manager = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    let sessions = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();
    let mut future = Box::pin(inject_peer_codex_clear_with_current_facts(
        app.handle(),
        id,
        "/clear",
        "watch",
        SettleReadiness::Ready,
        |_, _| Ok(()),
        move || {
            callback_calls.fetch_add(1, Ordering::SeqCst);
            notice.notify_one();
        },
    ));
    let baseline_refs = Arc::strong_count(&backend);
    let parallel = async {
        submitted.notified().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(sessions.get_session(id).await.unwrap().agent_turn_armed);
        assert_eq!(
            recorded_writes(&backend, id),
            vec![b"/clear".to_vec(), vec![13], vec![13]]
        );
        for bytes in [b"k".as_slice(), b"\x03", b"\x1b", b"web", b"privileged"] {
            tokio::time::timeout(
                Duration::from_millis(500),
                test_input_write(&manager, id, bytes),
            )
            .await
            .expect("writer blocked by observation")
            .unwrap();
        }
        if !remove && !poison && !cancel {
            tokio::time::timeout(
                Duration::from_secs(5),
                inject_text_into_session(app.handle(), id, "second"),
            )
            .await
            .expect("direct submission waited for watch")
            .unwrap();
        }
        let close = tokio::time::timeout(
            Duration::from_millis(500),
            test_input_write(&manager, id, &[b'e', b'x', b'i', b't', 13, 10]),
        )
        .await;
        assert!(
            matches!(close, Ok(Ok(()))),
            "close writer missed observer budget"
        );
        let other = Uuid::new_v4();
        manager
            .lock()
            .unwrap()
            .record_route(other, SessionBackendKind::LocalProcess);
        tokio::time::timeout(
            Duration::from_millis(500),
            test_input_write(&manager, other, b"other"),
        )
        .await
        .unwrap()
        .unwrap();
        if remove {
            manager
                .lock()
                .unwrap()
                .try_remove_route_if_kind(id, SessionBackendKind::LocalProcess)
                .expect("observation must not pin route lifecycle");
        }
        if poison {
            manager.lock().unwrap().poison_route_registry_for_test();
        }
        if !cancel {
            barrier.wait().await;
        }
    };
    if cancel {
        tokio::select! {
            result = &mut future => panic!("watch ended before cancellation: {result:?}"),
            () = parallel => {}
        }
        drop(future);
        assert_eq!(Arc::strong_count(&backend), baseline_refs);
        test_input_write(&manager, id, b"after cancel")
            .await
            .unwrap();
        return Err("cancelled".into());
    }
    let (result, ()) = tokio::join!(&mut future, parallel);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let writes = recorded_writes(&backend, id);
    let mut expected = vec![
        vec![b'k'],
        vec![3],
        vec![27],
        b"web".to_vec(),
        b"privileged".to_vec(),
    ];
    if !remove && !poison {
        expected.extend([b"second".to_vec(), vec![13], vec![13]]);
    }
    expected.push(vec![b'e', b'x', b'i', b't', 13, 10]);
    assert_eq!(&writes[3..], expected);
    if poison {
        manager
            .lock()
            .unwrap()
            .clear_route_registry_poison_for_test();
    }
    result
}

async fn capture_watch_route(replace: bool, remove: bool, poison: bool) -> bool {
    let (app, id, backend) = typing_hold_app("codex").await;
    *backend.screen_rows.lock().unwrap() = Some(Some(Vec::new()));
    let manager = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    let capture_manager = Arc::clone(&manager);
    *backend.capture_hook.lock().unwrap() = Some(Box::new(move || {
        if replace || remove {
            let manager = capture_manager.lock().unwrap();
            manager
                .try_remove_route_if_kind(id, SessionBackendKind::LocalProcess)
                .unwrap();
            if replace {
                manager.record_route(id, SessionBackendKind::LocalProcess);
            }
        } else if poison {
            capture_manager
                .lock()
                .unwrap()
                .poison_route_registry_for_test();
        } else {
            std::thread::sleep(std::time::Duration::from_millis(125));
        }
    }));
    let rows = test_capture(
        &manager,
        id,
        backend.as_ref(),
        !replace && !remove && !poison,
    )
    .await;
    if poison {
        manager
            .lock()
            .unwrap()
            .clear_route_registry_poison_for_test();
    }
    rows.is_some()
}
