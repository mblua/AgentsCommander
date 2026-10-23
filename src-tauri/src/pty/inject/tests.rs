use super::*;
use crate::config::settings::{new_settings_state, AppSettings, SettingsState};
use crate::pty::backend::{BackendSpawnSpec, PtyBackend, SessionBackendKind};
use chrono::Utc;
use std::collections::VecDeque;

mod capability;
mod hold;
mod seam;
mod submit;

#[derive(Default)]
struct RecordingBackend {
    writes: Mutex<Vec<(Uuid, Vec<u8>)>>,
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
