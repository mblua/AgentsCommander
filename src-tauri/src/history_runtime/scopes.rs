//! Session scopes for context history (#3532 P14).
//!
//! Owns the collaborator ports (launch facts, project environment, append)
//! and [`HistoryScopeState`], the one live, bounded cache of each runtime
//! session's resolved scope shared by capture and the sidebar. A session's
//! first visible resolution wins and is never recomputed; absent or pending
//! sessions are never memoized, so the next sample or query reads them again.
//! No Tauri, no `AppHandle` and no import of the parent runtime.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use futures::future::BoxFuture;
use uuid::Uuid;

use crate::context_history::config::{load_history_config, HistoryConfig, LoadedHistoryConfig};
use crate::context_history::observation::{
    parse_history_layout, parse_launch_metadata, resolve_history_scope, LaunchMetadata, ScopeInput,
    ScopeResolution, TeamMembers,
};
use crate::context_history::store::{AppendOutcome, HistoryWriter, StoreError};
use crate::context_history::HistoryRecord;
use crate::path_identity::paths_equivalent;

/// Largest launch-facts batch one request may ask for.
pub(crate) const LAUNCH_BATCH_MAX: usize = 100;
/// Live scope entries kept at once, like the actor's live state.
pub(crate) const SCOPE_CAPACITY: usize = 4096;

/// The launch facts of one present session, copied out of the session manager.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionLaunch {
    pub(crate) session_id: Uuid,
    pub(crate) working_directory: String,
    pub(crate) shell: String,
    pub(crate) effective_shell_args: Option<Vec<String>>,
    pub(crate) is_root_agent: bool,
    /// The session exited; its facts still prove where it lived.
    pub(crate) exited: bool,
}

/// Where scopes read live sessions. Each call copies under one short read and
/// releases every lock before returning; `None` refuses an invalid request.
pub(crate) trait SessionLaunchSource: Send + Sync {
    /// Facts of 1..=100 unique ids in request order; `None` entries are absent
    /// or still pending create.
    fn launches<'a>(&'a self, ids: &'a [Uuid])
        -> BoxFuture<'a, Option<Vec<Option<SessionLaunch>>>>;
    /// The ids among 0..=4096 unique `ids` that are present, visible and not
    /// exited.
    fn live_ids<'a>(&'a self, ids: &'a [Uuid]) -> BoxFuture<'a, Option<HashSet<Uuid>>>;
}

/// Project-level reads: configuration, team snapshot, AC-root authority and
/// the writer. Every method may touch disk and runs off the async workers.
pub(crate) trait ProjectEnvironment: Send + Sync + 'static {
    fn load_config(&self, ac_root: &Path) -> LoadedHistoryConfig;
    fn authoritative_ac_root(&self, ac_root: &Path) -> bool;
    fn teams(&self, project_path: &Path) -> Vec<TeamMembers>;
    fn open_writer(
        &self,
        ac_root: &Path,
        config: &HistoryConfig,
    ) -> Result<Box<dyn HistoryAppend>, StoreError>;
}

/// The append half of the store, boxed so tests can script outcomes.
pub(crate) trait HistoryAppend: Send {
    fn append(
        &mut self,
        records: &[HistoryRecord],
        now_utc_ms: i64,
    ) -> Result<AppendOutcome, StoreError>;
}

impl HistoryAppend for HistoryWriter {
    fn append(
        &mut self,
        records: &[HistoryRecord],
        now_utc_ms: i64,
    ) -> Result<AppendOutcome, StoreError> {
        HistoryWriter::append(self, records, now_utc_ms)
    }
}

/// Production environment: the real config owner, team discovery and store.
pub(crate) struct DiskProjectEnvironment;

impl ProjectEnvironment for DiskProjectEnvironment {
    fn load_config(&self, ac_root: &Path) -> LoadedHistoryConfig {
        load_history_config(ac_root)
    }

    fn authoritative_ac_root(&self, ac_root: &Path) -> bool {
        crate::config::ac_root::ensure_authoritative_ac_root(ac_root).is_ok()
    }

    fn teams(&self, project_path: &Path) -> Vec<TeamMembers> {
        let mut teams = Vec::new();
        crate::config::teams::discover_teams_in_project_filtered(project_path, &mut teams, |_| {
            true
        });
        teams
            .into_iter()
            .map(|team| TeamMembers {
                team_id: team.name,
                member_paths: team
                    .agent_paths
                    .into_iter()
                    .flatten()
                    .chain(team.coordinator_path)
                    .filter_map(|path| path.to_str().map(str::to_string))
                    .collect(),
            })
            .collect()
    }

    fn open_writer(
        &self,
        ac_root: &Path,
        config: &HistoryConfig,
    ) -> Result<Box<dyn HistoryAppend>, StoreError> {
        Ok(Box::new(HistoryWriter::open(ac_root, config)?))
    }
}

fn is_root_session(launch: &SessionLaunch) -> bool {
    launch.is_root_agent
        || crate::config::root_agent::is_root_agent_dir_name(&launch.working_directory)
        || crate::config::root_agent::is_root_agent_path(&launch.working_directory)
}

/// Resolve the scope of one session from an already-copied launch snapshot.
fn resolve_launch(launch: &SessionLaunch, env: &dyn ProjectEnvironment) -> ScopeResolution {
    let is_root_agent = is_root_session(launch);
    let (authoritative_ac_root, teams) = match parse_history_layout(&launch.working_directory) {
        Ok(layout) if !is_root_agent => (
            env.authoritative_ac_root(Path::new(&layout.ac_root)),
            env.teams(Path::new(&layout.project_path)),
        ),
        _ => (false, Vec::new()),
    };
    resolve_history_scope(&ScopeInput {
        working_directory: &launch.working_directory,
        is_root_agent,
        authoritative_ac_root,
        teams: &teams,
    })
}

/// Whether this saturation starts a new episode, the only one that warns;
/// an admission resets `saturated`.
pub(crate) fn first_in_episode(saturated: &mut bool) -> bool {
    !std::mem::replace(saturated, true)
}

/// One session's resolution: its scope and the immutable launch facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedScope {
    pub(crate) scope: ScopeResolution,
    pub(crate) launch: LaunchMetadata,
}

/// The outcome of resolving one runtime session.
#[derive(Debug, Clone)]
pub(crate) enum ScopeLookup {
    /// Absent, pending or exited: nothing visible to classify, nothing kept.
    Absent,
    Resolved(Arc<ResolvedScope>),
    /// Every cached session is still live; the caller treats it as unknown.
    Saturated,
}

/// Why a sidebar scope request was refused as a whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SidebarScopeError {
    /// Not 1..=100 unique ids.
    InvalidRequest,
    /// A present session does not provably belong to the requested project.
    ForeignSession(Uuid),
}

/// The shared, bounded scope owner of capture and sidebar.
pub(crate) struct HistoryScopeState {
    sessions: Arc<dyn SessionLaunchSource>,
    env: Arc<dyn ProjectEnvironment>,
    cache: Mutex<HashMap<Uuid, Arc<ResolvedScope>>>,
    saturated: AtomicBool,
}

impl HistoryScopeState {
    pub(crate) fn new(
        sessions: Arc<dyn SessionLaunchSource>,
        env: Arc<dyn ProjectEnvironment>,
    ) -> Self {
        Self {
            sessions,
            env,
            cache: Mutex::new(HashMap::new()),
            saturated: AtomicBool::new(false),
        }
    }

    pub(crate) fn sessions(&self) -> &Arc<dyn SessionLaunchSource> {
        &self.sessions
    }

    pub(crate) fn env(&self) -> &Arc<dyn ProjectEnvironment> {
        &self.env
    }

    fn cache(&self) -> MutexGuard<'_, HashMap<Uuid, Arc<ResolvedScope>>> {
        self.cache.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn cached(&self, session_id: Uuid) -> Option<Arc<ResolvedScope>> {
        self.cache().get(&session_id).cloned()
    }

    /// Forget a session that ended; a new runtime id resolves again.
    pub(crate) fn retire(&self, session_id: Uuid) {
        self.cache().remove(&session_id);
    }

    /// The scope of one runtime session: cached, or resolved once from its
    /// visible launch facts. Absent, pending and exited sessions are never kept.
    pub(crate) async fn resolve(&self, session_id: Uuid) -> ScopeLookup {
        if let Some(cached) = self.cached(session_id) {
            return ScopeLookup::Resolved(cached);
        }
        let launch = self.sessions.launches(&[session_id]).await;
        match launch.and_then(|mut facts| facts.pop().flatten()) {
            Some(launch) if !launch.exited => self.resolve_present(launch).await,
            _ => ScopeLookup::Absent,
        }
    }

    async fn is_live(&self, session_id: Uuid) -> bool {
        let live = self.sessions.live_ids(&[session_id]).await;
        live.is_some_and(|live| live.contains(&session_id))
    }

    /// Resolve off the async workers, then publish only if still visible.
    async fn resolve_present(&self, launch: SessionLaunch) -> ScopeLookup {
        let session_id = launch.session_id;
        let env = Arc::clone(&self.env);
        let resolved = tokio::task::spawn_blocking(move || {
            let args = launch.effective_shell_args.as_deref();
            ResolvedScope {
                scope: resolve_launch(&launch, env.as_ref()),
                launch: parse_launch_metadata(&launch.shell, args),
            }
        })
        .await;
        let Ok(resolved) = resolved else {
            return ScopeLookup::Absent;
        };
        if !self.is_live(session_id).await {
            return ScopeLookup::Absent;
        }
        self.admit(session_id, Arc::new(resolved)).await
    }

    fn try_insert(&self, session_id: Uuid, resolved: &Arc<ResolvedScope>) -> Option<ScopeLookup> {
        let mut cache = self.cache();
        if let Some(first) = cache.get(&session_id) {
            return Some(ScopeLookup::Resolved(Arc::clone(first)));
        }
        if cache.len() >= SCOPE_CAPACITY {
            return None;
        }
        cache.insert(session_id, Arc::clone(resolved));
        self.saturated.store(false, Ordering::Relaxed);
        Some(ScopeLookup::Resolved(Arc::clone(resolved)))
    }

    /// Admit a resolution; at capacity, retire only cached sessions the
    /// manager no longer shows, never a live one, and try once more.
    async fn admit(&self, session_id: Uuid, resolved: Arc<ResolvedScope>) -> ScopeLookup {
        if let Some(admitted) = self.try_insert(session_id, &resolved) {
            return admitted;
        }
        let candidates: Vec<Uuid> = self.cache().keys().take(SCOPE_CAPACITY).copied().collect();
        if let Some(live) = self.sessions.live_ids(&candidates).await {
            let mut cache = self.cache();
            for candidate in candidates.iter().filter(|id| !live.contains(id)) {
                cache.remove(candidate);
            }
        }
        if let Some(admitted) = self.try_insert(session_id, &resolved) {
            return admitted;
        }
        if !self.saturated.swap(true, Ordering::Relaxed) {
            log::warn!("[context-history] scope capacity reached by live sessions; scope unknown");
        }
        ScopeLookup::Saturated
    }

    /// The scopes the sidebar shows for 1..=100 unique runtime ids of one
    /// project, from the same owner capture uses; `None` is unknown. Any
    /// present session outside `project_paths`, Root included, refuses the whole request
    /// before discovery, teams or store. Works with capture disabled.
    // P05 consumer; retained scoped compatibility allowance
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) async fn snapshot_sidebar_scopes(
        &self,
        project_paths: &[PathBuf],
        ids: &[Uuid],
    ) -> Result<Vec<(Uuid, Option<Arc<ResolvedScope>>)>, SidebarScopeError> {
        let unique = ids.iter().collect::<HashSet<_>>().len() == ids.len();
        if ids.is_empty() || ids.len() > LAUNCH_BATCH_MAX || !unique {
            return Err(SidebarScopeError::InvalidRequest);
        }
        let launches = self.sessions.launches(ids).await;
        let launches = launches.ok_or(SidebarScopeError::InvalidRequest)?;
        let local = |launch: &SessionLaunch| {
            parse_history_layout(&launch.working_directory).is_ok_and(|layout| {
                let project = Path::new(&layout.project_path);
                project_paths
                    .iter()
                    .any(|path| paths_equivalent(project, path))
            })
        };
        let mut present = launches.iter().flatten();
        if let Some(launch) = present.find(|launch| !local(launch)) {
            return Err(SidebarScopeError::ForeignSession(launch.session_id));
        }
        let mut scopes = Vec::with_capacity(ids.len());
        for (session_id, launch) in ids.iter().copied().zip(launches) {
            let scope = match launch {
                Some(launch) if !launch.exited && !is_root_session(&launch) => {
                    match self.resolve(session_id).await {
                        ScopeLookup::Resolved(scope) => Some(scope),
                        ScopeLookup::Absent | ScopeLookup::Saturated => None,
                    }
                }
                _ => None,
            };
            scopes.push((session_id, scope));
        }
        Ok(scopes)
    }
}

#[cfg(test)]
mod tests;
