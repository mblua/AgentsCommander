use super::*;
use crate::config::settings::AppSettings;
use crate::pty::git_watcher::GitWatcher;
use crate::pty::idle_detector::IdleDetector;
use crate::web::broadcast::{WsBroadcaster, WsOutMsg};
use serde_json::{json, Value};

fn archive_test_session(
    name: &str,
    working_directory: &str,
    status: SessionStatus,
    is_root_agent: bool,
) -> SessionInfo {
    SessionInfo {
        id: Uuid::new_v4().to_string(),
        name: name.to_string(),
        shell: "shell".to_string(),
        shell_args: Vec::new(),
        backend_kind: crate::pty::backend::SessionBackendKind::LocalProcess,
        effective_shell_args: None,
        created_at: "2026-01-01T00:00:00Z".to_string(),
        working_directory: working_directory.to_string(),
        status,
        waiting_for_input: false,
        communication: None,
        pending_review: false,
        last_prompt: None,
        agent_id: None,
        agent_label: None,
        git_repos: Vec::new(),
        workgroup_task: None,
        is_coordinator: false,
        is_root_agent,
        token: "token".to_string(),
        agent_kind: None,
        requested_profile: None,
        effective_profile: None,
        profile_fallback_chain: Vec::new(),
        profile_fallback_applied: false,
        match_tier: None,
        original_profile_letter: None,
        effective_codex_home: None,
        profile_content_hash: None,
        trusted_configured_spawn: false,
        profile_outdated: false,
        telegram_bot_id: None,
        was_detached: false,
        detached_geometry: None,
        start_fresh_on_restore: false,
        context_percent: None,
    }
}

fn archive_scope() -> (tempfile::TempDir, String, String, String) {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project");
    let agent = project.join(".ac").join("wg-1").join("__agent_dev");
    let other = temp
        .path()
        .join("other")
        .join(".ac")
        .join("wg-1")
        .join("__agent_other");
    std::fs::create_dir_all(&agent).expect("agent");
    std::fs::create_dir_all(&other).expect("other");
    (
        temp,
        project.to_string_lossy().to_string(),
        agent.to_string_lossy().to_string(),
        other.to_string_lossy().to_string(),
    )
}

type ArchiveCommandApp = (
    tauri::App,
    SettingsState,
    tokio::sync::mpsc::Receiver<WsOutMsg>,
    Arc<tokio::sync::RwLock<SessionManager>>,
    Arc<Mutex<PtyManager>>,
);

fn archive_command_app(settings: AppSettings) -> ArchiveCommandApp {
    let settings_state: SettingsState = crate::config::settings::new_settings_state(settings);
    let broadcaster = WsBroadcaster::new();
    let receiver = broadcaster.subscribe();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let app = crate::test_support::test_builder()
        .manage(settings_state.clone())
        .manage(broadcaster.clone())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build archive command test app");
    let app_handle = app.handle().clone();
    let idle_detector = IdleDetector::new(|_| {}, |_| {});
    let git_watcher = GitWatcher::new(Arc::clone(&session_mgr), app_handle);
    let pty_mgr = Arc::new(Mutex::new(PtyManager::new(
        Arc::new(Mutex::new(HashMap::new())),
        idle_detector,
        git_watcher,
        Some(broadcaster),
        None,
    )));
    (app, settings_state, receiver, session_mgr, pty_mgr)
}

#[derive(Default)]
struct FlippingLiveBackend {
    calls: std::sync::atomic::AtomicUsize,
}

impl crate::pty::backend::PtyBackend for FlippingLiveBackend {
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
        _authority: &crate::pty::manager::BackendWriteAuthority,
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

    fn has_session(&self, _id: Uuid) -> bool {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0
    }

    fn get_screen_snapshot(&self, _id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
        None
    }

    fn get_pty_size(&self, _id: Uuid) -> Option<(u16, u16)> {
        None
    }

    fn get_screen_rows(&self, _id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
        crate::pty::context_scrape::ScreenRowsRead::SessionOver
    }

    fn register_response_watcher(
        &self,
        _session_id: Uuid,
        _request_id: String,
        _response_dir: PathBuf,
    ) {
    }

    fn terminate_job_for_session(&self, _id: Uuid) -> bool {
        false
    }

    fn kill_all_jobs(&self) -> (usize, usize) {
        (0, 0)
    }
}

fn recv_ws_event(rx: &mut tokio::sync::mpsc::Receiver<WsOutMsg>) -> Value {
    match rx.try_recv().expect("broadcast event") {
        WsOutMsg::Text(text) => serde_json::from_str::<Value>(&text).expect("parse event"),
        other @ WsOutMsg::Binary(_) => panic!("expected text event, got {other:?}"),
    }
}

/// #1077: the display-canonical form of an existing directory - exactly what
/// the six-field decoder selects for a valid project. Storing this (instead
/// of the raw temp path) keeps the runtime-path assertions platform-robust:
/// on Windows it equals the raw path, on Linux/CI it is the symlink-resolved
/// canonical path the decoder actually publishes.
fn canonical_display(dir: &Path) -> String {
    crate::config::projects::display_canonical(
        &std::fs::canonicalize(dir).unwrap().to_string_lossy(),
    )
}

/// The Windows 8.3 short-name form of an existing path, or the path itself
/// when the volume has 8.3 generation disabled.
#[cfg(windows)]
fn short_path(p: &Path) -> PathBuf {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;
    let wide: Vec<u16> = p.as_os_str().encode_wide().chain([0]).collect();
    let len = unsafe { GetShortPathNameW(wide.as_ptr(), std::ptr::null_mut(), 0) };
    if len == 0 {
        return p.to_path_buf();
    }
    let mut buf = vec![0u16; len as usize];
    let written = unsafe { GetShortPathNameW(wide.as_ptr(), buf.as_mut_ptr(), len) };
    if written == 0 {
        return p.to_path_buf();
    }
    buf.truncate(written as usize);
    PathBuf::from(std::ffi::OsString::from_wide(&buf))
}

/// CWD is process-wide; serialize tests that intentionally use relative paths.
struct CwdGuard(PathBuf);
impl Drop for CwdGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.0);
    }
}

static CWD_MUTEX: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();

async fn cwd_lock() -> tokio::sync::MutexGuard<'static, ()> {
    CWD_MUTEX
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await
}

fn archive_snapshot(
    sessions: Vec<(SessionInfo, bool)>,
    pending: Vec<PendingSpawn>,
) -> ArchiveSnapshot {
    ArchiveSnapshot { sessions, pending }
}

// #2434 - instructions-filename rows shared by discovery and gitignore_layout.
fn floor_names() -> Vec<String> {
    vec!["CLAUDE.md".into(), "AGENTS.md".into()]
}

mod archive;
mod discovery;
mod discovery_payload;
mod gitignore_layout;
mod gitignore_rules;
mod project;
mod snapshot_null;
mod snapshot_refresh;
