//! #3948 - `GitEvents` port ordering through the real `GitWatcher::poll`.

use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::task::Poll;
use std::time::Duration;

use futures::FutureExt;
use uuid::Uuid;

use crate::pty::backend::SessionBackendKind;
use crate::pty::git_watcher::{publish_git_status, GitEvents, GitState, GitStatus, GitWatcher};
use crate::session::manager::{acquire_carrier_mutation_permit, SessionManager};
use crate::session::session::SessionRepo;

type Manager = Arc<tokio::sync::RwLock<SessionManager>>;

/// Records every notification. With `probe`, checks lock/state/cache ordering at
/// callback entry, failing immediately instead of waiting or retrying.
struct Recorder {
    manager: Manager,
    watcher: OnceLock<Weak<GitWatcher>>,
    probe: Option<Expected>,
    calls: Mutex<Vec<(Uuid, Vec<SessionRepo>)>>,
}

struct Expected {
    repos: Vec<SessionRepo>,
    generation: u64,
    old_cache: Vec<SessionRepo>,
}

struct Port(Arc<Recorder>);

impl GitEvents for Port {
    fn session_git_repos(&self, id: Uuid, repos: Vec<SessionRepo>) {
        if let Some(expected) = &self.0.probe {
            probe_at_callback(&self.0, expected, id);
        }
        self.0.calls.lock().unwrap().push((id, repos));
    }
}

fn probe_at_callback(recorder: &Recorder, expected: &Expected, id: Uuid) {
    // Exclusive acquisition is the only proof that poll's outer read guard is gone.
    let write = recorder
        .manager
        .try_write()
        .expect("outer manager read guard still held at notification");
    drop(write);
    let rows = {
        let mgr = recorder
            .manager
            .try_read()
            .expect("manager read after write probe");
        mgr.get_sessions_repos()
            .now_or_never()
            .expect("session state read must not be pending")
    };
    let (_, repos, generation) = rows
        .into_iter()
        .find(|(row_id, _, _)| *row_id == id)
        .expect("session row");
    assert_eq!(
        repos, expected.repos,
        "CAS-written repos visible at notification"
    );
    assert_eq!(
        generation, expected.generation,
        "CAS generation visible at notification"
    );
    let watcher = recorder
        .watcher
        .get()
        .and_then(Weak::upgrade)
        .expect("watcher alive during poll");
    let cache = match watcher.cache.try_lock() {
        Ok(cache) => cache,
        Err(err) => panic!("cache lock unavailable at notification: {err}"),
    };
    assert_eq!(
        cache.get(&id),
        Some(&expected.old_cache),
        "cache insert must follow the notification"
    );
}

fn repo(label: &str, path: &str) -> SessionRepo {
    SessionRepo {
        label: label.into(),
        source_path: path.into(),
        branch: None,
        dirty: None,
    }
}

async fn create(manager: &Manager, repos: Vec<SessionRepo>) -> (Uuid, u64) {
    let mgr = manager.read().await;
    let session = mgr
        .create_session(
            "cmd".into(),
            vec![],
            "C:/tmp".into(),
            None,
            None,
            repos,
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create_session");
    let generation = mgr.get_git_repos_gen(session.id).await.expect("generation");
    (session.id, generation)
}

fn recorder(manager: &Manager, probe: Option<Expected>) -> Arc<Recorder> {
    Arc::new(Recorder {
        manager: Arc::clone(manager),
        watcher: OnceLock::new(),
        probe,
        calls: Mutex::new(Vec::new()),
    })
}

fn watcher(manager: &Manager, recorder: &Arc<Recorder>, state: Arc<GitState>) -> Arc<GitWatcher> {
    let watcher = GitWatcher::with_state(Arc::clone(manager), Port(Arc::clone(recorder)), state);
    assert!(recorder.watcher.set(Arc::downgrade(&watcher)).is_ok());
    watcher
}

fn publish_main_dirty(path: &str, state: &GitState) -> SessionRepo {
    let status = GitStatus {
        branch: Some("main".into()),
        dirty: true,
    };
    publish_git_status(path, Some(status), state);
    SessionRepo {
        branch: Some("main".into()),
        dirty: Some(true),
        ..repo("A", path)
    }
}

async fn session_row(manager: &Manager, id: Uuid) -> (Vec<SessionRepo>, u64) {
    let rows = manager.read().await.get_sessions_repos().await;
    let (_, repos, generation) = rows.into_iter().find(|(row_id, ..)| *row_id == id).unwrap();
    (repos, generation)
}

#[tokio::test]
async fn issue_3948_successful_poll_notifies_after_cas_and_guard_release_before_cache() {
    let manager: Manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let path = "/test-3948/success/repo-A";
    let (id, gen0) = create(&manager, vec![repo("A", path)]).await;
    let state = Arc::new(GitState::new());
    let expected = publish_main_dirty(path, &state);
    let old = vec![repo("A-old", path)];
    let recorder = recorder(
        &manager,
        Some(Expected {
            repos: vec![expected.clone()],
            generation: gen0.wrapping_add(1),
            old_cache: old.clone(),
        }),
    );
    let watcher = watcher(&manager, &recorder, state);
    watcher.cache.lock().unwrap().insert(id, old);

    watcher.poll().await;
    assert_eq!(
        *recorder.calls.lock().unwrap(),
        vec![(id, vec![expected.clone()])]
    );
    assert_eq!(
        watcher.cache.lock().unwrap().get(&id),
        Some(&vec![expected.clone()])
    );
    assert_eq!(
        session_row(&manager, id).await,
        (vec![expected], gen0.wrapping_add(1))
    );

    watcher.poll().await;
    assert_eq!(
        recorder.calls.lock().unwrap().len(),
        1,
        "identical poll is suppressed"
    );
}

#[tokio::test]
async fn issue_3948_empty_repos_evict_cache_without_notification() {
    let manager: Manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let (id, _) = create(&manager, vec![]).await;
    let recorder = recorder(&manager, None);
    let watcher = watcher(&manager, &recorder, Arc::new(GitState::new()));
    let seeded = vec![repo("A-old", "/test-3948/empty/repo-A")];
    watcher.cache.lock().unwrap().insert(id, seeded);

    watcher.poll().await;
    assert!(recorder.calls.lock().unwrap().is_empty());
    assert!(!watcher.cache.lock().unwrap().contains_key(&id));
}

#[tokio::test]
async fn issue_3948_stale_cas_keeps_newer_state_and_drops_cache_without_notification() {
    let manager: Manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let path = "/test-3948/miss/repo-A";
    let (id, gen0) = create(&manager, vec![repo("A", path)]).await;
    let state = Arc::new(GitState::new());
    publish_main_dirty(path, &state);
    let recorder = recorder(&manager, None);
    let watcher = watcher(&manager, &recorder, state);
    watcher
        .cache
        .lock()
        .unwrap()
        .insert(id, vec![repo("A-old", path)]);

    let permit = acquire_carrier_mutation_permit().await;
    let mut poll = Box::pin(watcher.poll());
    match futures::poll!(poll.as_mut()) {
        Poll::Pending => {}
        Poll::Ready(()) => panic!("poll must block on the held carrier permit"),
    }
    let newer = vec![repo("A-newer", "/test-3948/miss/repo-A-newer")];
    manager
        .read()
        .await
        .set_git_repos_with_carrier_permit(&permit, id, newer.clone())
        .await;
    drop(permit);
    tokio::time::timeout(Duration::from_secs(5), poll)
        .await
        .expect("poll finishes after the permit is released");

    assert!(
        recorder.calls.lock().unwrap().is_empty(),
        "stale CAS must not notify"
    );
    assert_eq!(
        session_row(&manager, id).await,
        (newer, gen0.wrapping_add(1))
    );
    assert!(!watcher.cache.lock().unwrap().contains_key(&id));
}

#[tokio::test]
async fn issue_3948_headless_poll_updates_state_and_cache_without_transport() {
    let manager: Manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let path = "/test-3948/headless/repo-A";
    let (id, gen0) = create(&manager, vec![repo("A", path)]).await;
    let watcher = GitWatcher::new_headless_for_tests(Arc::clone(&manager));
    assert!(watcher.events.is_none());
    let expected = publish_main_dirty(path, &watcher.git_state);

    watcher.poll().await;
    assert_eq!(
        watcher.cache.lock().unwrap().get(&id),
        Some(&vec![expected.clone()])
    );
    assert_eq!(
        session_row(&manager, id).await,
        (vec![expected], gen0.wrapping_add(1))
    );
}
