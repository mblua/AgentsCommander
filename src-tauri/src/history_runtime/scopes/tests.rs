use std::sync::atomic::AtomicU64;

use super::*;
use crate::context_history::config::ConfigStatus;

const WD: &str = "/p/proj/.ac/room-1-x/__agent_dev";
const FOREIGN: &str = "/q/other/.ac/room-1-x/__agent_dev";

/// Sessions the manager would show; `vanished` ones are still copied by a
/// launch read but already gone for the liveness re-check.
#[derive(Default)]
struct Source {
    present: Mutex<HashMap<Uuid, SessionLaunch>>,
    vanished: Mutex<HashSet<Uuid>>,
}

impl Source {
    fn add(&self, working_directory: &str, root: bool, exited: bool) -> Uuid {
        let session_id = Uuid::new_v4();
        let launch = SessionLaunch {
            session_id,
            working_directory: working_directory.to_string(),
            shell: "claude".to_string(),
            effective_shell_args: None,
            is_root_agent: root,
            exited,
        };
        self.present.lock().unwrap().insert(session_id, launch);
        session_id
    }

    fn remove(&self, session_id: Uuid) {
        self.present.lock().unwrap().remove(&session_id);
    }
}

impl SessionLaunchSource for Source {
    fn launches<'a>(
        &'a self,
        ids: &'a [Uuid],
    ) -> BoxFuture<'a, Option<Vec<Option<SessionLaunch>>>> {
        Box::pin(async move {
            let present = self.present.lock().unwrap();
            Some(ids.iter().map(|id| present.get(id).cloned()).collect())
        })
    }

    fn live_ids<'a>(&'a self, ids: &'a [Uuid]) -> BoxFuture<'a, Option<HashSet<Uuid>>> {
        Box::pin(async move {
            let present = self.present.lock().unwrap();
            let vanished = self.vanished.lock().unwrap();
            let live = |id: &&Uuid| {
                present.get(id).is_some_and(|launch| !launch.exited) && !vanished.contains(id)
            };
            Some(ids.iter().filter(live).copied().collect())
        })
    }
}

/// Counts team discoveries: one per resolved session.
#[derive(Default)]
struct Env {
    teams: AtomicU64,
}

impl ProjectEnvironment for Env {
    fn load_config(&self, _ac_root: &Path) -> LoadedHistoryConfig {
        LoadedHistoryConfig {
            config: HistoryConfig::DISABLED,
            status: ConfigStatus::Loaded,
        }
    }
    fn authoritative_ac_root(&self, _ac_root: &Path) -> bool {
        true
    }
    fn teams(&self, _project_path: &Path) -> Vec<TeamMembers> {
        self.teams.fetch_add(1, Ordering::SeqCst);
        Vec::new()
    }
    fn open_writer(
        &self,
        _ac_root: &Path,
        _config: &HistoryConfig,
    ) -> Result<Box<dyn HistoryAppend>, StoreError> {
        Err(StoreError::RetryableBusy)
    }
}

fn state() -> (Arc<Source>, Arc<Env>, HistoryScopeState) {
    let source = Arc::new(Source::default());
    let env = Arc::new(Env::default());
    let state = HistoryScopeState::new(source.clone(), env.clone());
    (source, env, state)
}

fn project() -> Vec<PathBuf> {
    vec![PathBuf::from("/p/proj")]
}

fn teams(env: &Env) -> u64 {
    env.teams.load(Ordering::SeqCst)
}

impl HistoryScopeState {
    fn len(&self) -> usize {
        self.cache().len()
    }
}

#[test]
fn root_sessions_resolve_excluded_from_the_same_resolver() {
    let env = Env::default();
    let launch = |root| SessionLaunch {
        session_id: Uuid::new_v4(),
        working_directory: WD.to_string(),
        shell: "claude".to_string(),
        effective_shell_args: None,
        is_root_agent: root,
        exited: false,
    };
    assert!(matches!(
        resolve_launch(&launch(false), &env),
        ScopeResolution::Scoped(_)
    ));
    assert_eq!(
        resolve_launch(&launch(true), &env),
        ScopeResolution::Excluded("root_agent")
    );
}

#[tokio::test]
async fn sidebar_scopes_work_without_capture_and_keep_unknowns_neutral() {
    let (source, _env, state) = state();
    let local = source.add(WD, false, false);
    let root = source.add(WD, true, false);
    let exited = source.add(WD, false, true);
    let absent = Uuid::new_v4();
    let scopes = state
        .snapshot_sidebar_scopes(&project(), &[local, root, exited, absent])
        .await
        .expect("local request");
    let ids: Vec<Uuid> = scopes.iter().map(|(id, _)| *id).collect();
    assert_eq!(ids, [local, root, exited, absent], "request order");
    assert!(matches!(
        scopes[0].1.as_deref().map(|scope| &scope.scope),
        Some(ScopeResolution::Scoped(_))
    ));
    assert!(scopes[1..].iter().all(|(_, scope)| scope.is_none()));
    assert_eq!(state.len(), 1, "only the visible session is kept");
}

#[tokio::test]
async fn a_foreign_session_refuses_the_whole_request_before_teams() {
    let (source, env, state) = state();
    let local = source.add(WD, false, false);
    let foreign = source.add(FOREIGN, false, false);
    let refused = state
        .snapshot_sidebar_scopes(&project(), &[local, foreign])
        .await;
    assert_eq!(refused, Err(SidebarScopeError::ForeignSession(foreign)));
    assert_eq!((teams(&env), state.len()), (0, 0));
}

#[tokio::test]
async fn malformed_batches_are_refused() {
    let (_source, env, state) = state();
    let id = Uuid::new_v4();
    let too_many: Vec<Uuid> = (0..=LAUNCH_BATCH_MAX).map(|_| Uuid::new_v4()).collect();
    for ids in [Vec::new(), vec![id, id], too_many] {
        let refused = state.snapshot_sidebar_scopes(&project(), &ids).await;
        assert_eq!(refused, Err(SidebarScopeError::InvalidRequest));
    }
    assert_eq!(teams(&env), 0);
}

#[tokio::test]
async fn the_first_resolution_wins_until_the_session_retires() {
    let (source, env, state) = state();
    let id = source.add(WD, false, false);
    let first = state.resolve(id).await;
    let again = state.resolve(id).await;
    let (ScopeLookup::Resolved(first), ScopeLookup::Resolved(again)) = (first, again) else {
        panic!("resolved twice");
    };
    assert!(Arc::ptr_eq(&first, &again));
    assert_eq!(teams(&env), 1, "one discovery per runtime id");
    state.retire(id);
    assert!(matches!(state.resolve(id).await, ScopeLookup::Resolved(_)));
    assert_eq!(teams(&env), 2);
}

#[tokio::test]
async fn absent_pending_and_exited_sessions_are_never_kept() {
    let (source, _env, state) = state();
    let exited = source.add(WD, false, true);
    for id in [Uuid::new_v4(), exited] {
        assert!(matches!(state.resolve(id).await, ScopeLookup::Absent));
    }
    let later = Uuid::new_v4();
    assert!(matches!(state.resolve(later).await, ScopeLookup::Absent));
    assert_eq!(state.len(), 0, "no tombstone for an absent id");
    source.present.lock().unwrap().insert(
        later,
        SessionLaunch {
            session_id: later,
            working_directory: WD.to_string(),
            shell: "claude".to_string(),
            effective_shell_args: None,
            is_root_agent: false,
            exited: false,
        },
    );
    assert!(matches!(
        state.resolve(later).await,
        ScopeLookup::Resolved(_)
    ));
}

#[tokio::test]
async fn a_session_gone_before_publication_is_not_inserted() {
    let (source, env, state) = state();
    let id = source.add(WD, false, false);
    source.vanished.lock().unwrap().insert(id);
    assert!(matches!(state.resolve(id).await, ScopeLookup::Absent));
    assert_eq!((teams(&env), state.len()), (1, 0));
}

#[tokio::test]
async fn capacity_retires_only_absent_sessions_and_never_a_live_one() {
    let (source, _env, state) = state();
    let filled: Vec<Uuid> = (0..SCOPE_CAPACITY)
        .map(|_| source.add(WD, false, false))
        .collect();
    for id in &filled {
        assert!(matches!(state.resolve(*id).await, ScopeLookup::Resolved(_)));
    }
    let extra = source.add(WD, false, false);
    assert!(matches!(state.resolve(extra).await, ScopeLookup::Saturated));
    assert_eq!(state.len(), SCOPE_CAPACITY, "no live session evicted");
    for id in &filled {
        source.remove(*id);
    }
    let fresh = source.add(WD, false, false);
    assert!(matches!(
        state.resolve(fresh).await,
        ScopeLookup::Resolved(_)
    ));
    assert_eq!(state.len(), 1, "absent entries retired under pressure");
}

#[tokio::test]
async fn live_ids_accepts_an_empty_request() {
    let (source, _env, _state) = state();
    assert_eq!(source.live_ids(&[]).await, Some(HashSet::new()));
}
