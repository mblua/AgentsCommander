use super::*;
use crate::errors::AppError;
use crate::phone::mailbox::pty_claim::MailboxTestHooks;
use crate::pty::backend::{BackendSpawnSpec, PtyBackend, SessionBackendKind};
use crate::pty::context_scrape::ScreenRowsRead;
use crate::pty::input_route::BackendWriteAuthority;
use crate::pty::manager::PtyManager;
use crate::pty::output::PtyScreenSnapshot;
use futures::future::BoxFuture;
use std::collections::HashSet;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::RwLock;

const CWD: &str = "C:/lookup-project/.ac/room-22-team/__agent_member";
const FQN: &str = "lookup-project:room-22-team/member";

struct LookupState {
    sessions: Arc<RwLock<SessionManager>>,
    routes: Arc<Mutex<PtyManager>>,
    backend: Arc<PresenceBackend>,
    accesses: Mutex<Vec<&'static str>>,
    forbid_routes: bool,
}

impl LookupState {
    fn new(catalog_held: bool, forbid_routes: bool) -> Self {
        let sessions = Arc::new(RwLock::new(SessionManager::new()));
        let backend = Arc::new(PresenceBackend {
            sessions: Arc::clone(&sessions),
            catalog_held,
            live: Mutex::new(HashSet::new()),
            checks: Mutex::new(Vec::new()),
        });
        let routes = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        Self {
            sessions,
            routes,
            backend,
            accesses: Mutex::new(Vec::new()),
            forbid_routes,
        }
    }

    async fn row(&self, name: &str, exited: bool, root: bool, live: bool) -> Uuid {
        let mgr = self.sessions.read().await;
        let row = mgr
            .create_session(
                "shell".into(),
                Vec::new(),
                CWD.into(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        mgr.rename_session(row.id, name.into()).await.unwrap();
        if exited {
            mgr.mark_exited(row.id, 7).await;
        }
        if root {
            mgr.set_is_root_agent(row.id, true).await;
        }
        self.routes
            .lock()
            .unwrap()
            .record_route(row.id, SessionBackendKind::LocalProcess);
        if live {
            self.backend.live.lock().unwrap().insert(row.id);
        }
        row.id
    }

    fn accesses(&self) -> Vec<&'static str> {
        std::mem::take(&mut *self.accesses.lock().unwrap())
    }
}

impl PtyAuthorityState for LookupState {
    fn pty_sessions(&self) -> &Arc<RwLock<SessionManager>> {
        self.accesses.lock().unwrap().push("sessions");
        &self.sessions
    }

    fn pty_routes(&self) -> &Arc<Mutex<PtyManager>> {
        assert!(!self.forbid_routes, "unexpected routes getter");
        if self.backend.catalog_held {
            assert!(
                self.sessions.try_write().is_ok(),
                "routes resolved under catalog lock"
            );
        }
        self.accesses.lock().unwrap().push("routes");
        &self.routes
    }

    fn pty_settings(&self) -> &SettingsState {
        panic!("lookup must never request settings");
    }
}

struct PresenceBackend {
    sessions: Arc<RwLock<SessionManager>>,
    catalog_held: bool,
    live: Mutex<HashSet<Uuid>>,
    checks: Mutex<Vec<Uuid>>,
}

impl PtyBackend for PresenceBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn spawn(&self, _spec: BackendSpawnSpec) -> BoxFuture<'_, Result<(), AppError>> {
        panic!("unexpected spawn");
    }

    fn write(
        &self,
        _authority: &BackendWriteAuthority,
        _id: Uuid,
        _data: &[u8],
    ) -> Result<(), AppError> {
        panic!("unexpected write");
    }

    fn resize(&self, _id: Uuid, _cols: u16, _rows: u16) -> Result<(), AppError> {
        panic!("unexpected resize");
    }

    fn kill(&self, _id: Uuid) -> Result<(), AppError> {
        panic!("unexpected kill");
    }

    fn has_session(&self, id: Uuid) -> bool {
        assert_eq!(
            self.sessions.try_write().is_err(),
            self.catalog_held,
            "catalog guard lifetime at PTY presence check"
        );
        self.checks.lock().unwrap().push(id);
        self.live.lock().unwrap().contains(&id)
    }

    fn get_screen_snapshot(&self, _id: Uuid) -> Option<PtyScreenSnapshot> {
        panic!("unexpected snapshot");
    }

    fn get_pty_size(&self, _id: Uuid) -> Option<(u16, u16)> {
        panic!("unexpected size");
    }

    fn get_screen_rows(&self, _id: Uuid) -> ScreenRowsRead {
        panic!("unexpected rows");
    }

    fn register_response_watcher(&self, _id: Uuid, _request: String, _dir: std::path::PathBuf) {
        panic!("unexpected watcher");
    }

    fn terminate_job_for_session(&self, _id: Uuid) -> bool {
        panic!("unexpected termination");
    }

    fn kill_all_jobs(&self) -> (usize, usize) {
        panic!("unexpected termination");
    }
}

#[tokio::test]
async fn empty_live_lookup_never_resolves_routes() {
    let state = LookupState::new(false, true);
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        MailboxPoller::delivery_only_for_tests().find_live_candidates(&state, FQN),
    )
    .await
    .unwrap();
    assert_eq!(result, (Vec::new(), false));
    assert_eq!(state.accesses(), ["sessions"]);
}

#[tokio::test]
async fn active_and_all_only_observe_catalog_and_keep_exact_matches() {
    let state = LookupState::new(false, true);
    let poller = MailboxPoller::delivery_only_for_tests();
    assert_eq!(poller.find_active_session(&state, FQN).await, None);
    assert!(poller.find_all_sessions(&state, FQN).await.is_empty());
    assert_eq!(state.accesses(), ["sessions", "sessions"]);
    let temp = format!("{}member", crate::session::session::TEMP_SESSION_PREFIX);
    let temporary = state.row(&temp, false, false, true).await;
    let exited = state.row("exited", true, false, false).await;
    let running = state.row("running", false, false, true).await;
    assert_eq!(poller.find_active_session(&state, FQN).await, Some(running));
    let mut all = poller.find_all_sessions(&state, FQN).await;
    all.sort();
    let mut expected = vec![temporary, exited, running];
    expected.sort();
    assert_eq!(all, expected);
    assert!(poller
        .find_all_sessions(&state, "other:room-22-team/member")
        .await
        .is_empty());
    assert_eq!(state.accesses(), ["sessions", "sessions", "sessions"]);
}

#[tokio::test]
async fn root_resolves_routes_before_catalog_read_even_when_empty() {
    let state = LookupState::new(true, false);
    assert_eq!(
        tokio::time::timeout(
            Duration::from_secs(5),
            MailboxPoller::delivery_only_for_tests().find_root_session_candidate(&state),
        )
        .await
        .unwrap(),
        None
    );
    assert_eq!(state.accesses(), ["sessions", "routes"]);
}

#[tokio::test]
async fn root_holds_catalog_during_presence_and_retains_exited_record() {
    let state = LookupState::new(true, false);
    let id = state.row("root", false, true, true).await;
    let poller = MailboxPoller::delivery_only_for_tests();
    assert_eq!(
        tokio::time::timeout(
            Duration::from_secs(5),
            poller.find_root_session_candidate(&state)
        )
        .await
        .unwrap(),
        Some((id, SessionStatus::Active))
    );
    assert_eq!(state.accesses(), ["sessions", "routes"]);
    assert_eq!(*state.backend.checks.lock().unwrap(), [id]);
    state.sessions.read().await.mark_exited(id, 7).await;
    assert_eq!(poller.find_root_session_candidate(&state).await, None);
    assert!(state.sessions.read().await.get_session(id).await.is_some());
}

#[tokio::test]
async fn live_releases_catalog_before_presence_and_keeps_sort_phantom_exited_rules() {
    let state = LookupState::new(false, false);
    let temp = format!("{}member", crate::session::session::TEMP_SESSION_PREFIX);
    let temporary = state.row(&temp, false, false, true).await;
    let exited = state.row("exited", true, false, false).await;
    let phantom = state.row("phantom", false, false, false).await;
    let running = state.row("running", false, false, true).await;
    let (rows, had_match) = tokio::time::timeout(
        Duration::from_secs(5),
        MailboxPoller::delivery_only_for_tests().find_live_candidates(&state, FQN),
    )
    .await
    .unwrap();
    assert!(had_match);
    assert_eq!(
        rows,
        vec![
            (running, SessionStatus::Running),
            (exited, SessionStatus::Exited(7)),
            (temporary, SessionStatus::Active),
        ]
    );
    assert_eq!(
        state.accesses(),
        ["sessions", "routes", "routes", "routes", "routes"]
    );
    let checks = state.backend.checks.lock().unwrap();
    assert_eq!(checks.len(), 4);
    assert!(checks.contains(&phantom));
}

#[tokio::test]
async fn scripted_presence_returns_before_routes_getter() {
    let state = LookupState::new(false, true);
    let id = state.row("hooked", false, false, false).await;
    let hooks = MailboxTestHooks::default();
    let mut poller = MailboxPoller::delivery_only_for_tests();
    poller.test_hooks = Some(hooks.clone());
    for present in [true, false] {
        hooks.pty_presence.lock().unwrap().insert(id, present);
        assert_eq!(poller.has_pty_session_for_wake(&state, id).await, present);
        let (rows, had_match) = poller.find_live_candidates(&state, FQN).await;
        assert!(had_match);
        assert_eq!(rows.len(), usize::from(present));
        assert_eq!(state.accesses(), ["sessions"]);
    }
    assert!(state.backend.checks.lock().unwrap().is_empty());
}
