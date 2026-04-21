use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::join_all;
use tauri::{AppHandle, Emitter};
use uuid::Uuid;

use crate::session::manager::SessionManager;
use crate::session::session::SessionRepo;

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const DETECT_TIMEOUT: Duration = Duration::from_secs(2);

pub struct GitWatcher {
    session_manager: Arc<tokio::sync::RwLock<SessionManager>>,
    app_handle: AppHandle,
    /// Last-emitted per-repo state keyed by session id. Equality gate for `session_git_repos`.
    /// `Vec` equality is order-sensitive; callers preserve replica config.json `repos` order.
    cache: Mutex<HashMap<Uuid, Vec<SessionRepo>>>,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct GitReposPayload {
    session_id: String,
    repos: Vec<SessionRepo>,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CoordinatorChangedPayload {
    pub session_id: String,
    pub is_coordinator: bool,
}

impl GitWatcher {
    pub fn new(
        session_manager: Arc<tokio::sync::RwLock<SessionManager>>,
        app_handle: AppHandle,
    ) -> Arc<Self> {
        Arc::new(Self {
            session_manager,
            app_handle,
            cache: Mutex::new(HashMap::new()),
        })
    }

    pub fn start(self: &Arc<Self>, shutdown: crate::shutdown::ShutdownSignal) {
        let watcher = Arc::clone(self);
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new()
                .expect("Failed to create tokio runtime for GitWatcher");
            rt.block_on(async move {
                loop {
                    tokio::select! {
                        biased;
                        _ = shutdown.token().cancelled() => {
                            log::info!("[GitWatcher] Shutdown signal received, stopping");
                            break;
                        }
                        _ = tokio::time::sleep(POLL_INTERVAL) => {
                            watcher.poll().await;
                        }
                    }
                }
            });
        });
    }

    pub fn remove_session(&self, id: Uuid) {
        self.cache.lock().unwrap().remove(&id);
    }

    /// Force a re-emit on the next tick for `id`. Called by
    /// `refresh_git_repos_for_sessions` callers so their newly-written `git_repos`
    /// isn't silently skipped by a stale cache hit.
    pub fn invalidate_session_cache(&self, id: Uuid) {
        self.cache.lock().unwrap().remove(&id);
    }

    async fn poll(&self) {
        let sessions: Vec<(Uuid, Vec<SessionRepo>, u64)> = {
            let mgr = self.session_manager.read().await;
            mgr.get_sessions_repos().await
        };

        for (id, repos, gen_snapshot) in sessions {
            if repos.is_empty() {
                // Nothing to watch. If cache still has this id, clear it so a later
                // "repos appeared" transition re-emits.
                let mut cache = self.cache.lock().unwrap();
                cache.remove(&id);
                continue;
            }

            // Parallelize per-repo detection (Grinch #16). Each call bounded by 2s
            // (detect_branch_with_timeout). Without join_all, worst-case per poll is
            // M*N*2s under simultaneous stalls.
            let branches: Vec<Option<String>> = join_all(
                repos
                    .iter()
                    .map(|r| Self::detect_branch_with_timeout(&r.source_path)),
            )
            .await;

            let refreshed: Vec<SessionRepo> = repos
                .iter()
                .zip(branches.into_iter())
                .map(|(r, branch)| SessionRepo {
                    label: r.label.clone(),
                    source_path: r.source_path.clone(),
                    branch,
                })
                .collect();

            let changed = {
                let cache = self.cache.lock().unwrap();
                cache.get(&id) != Some(&refreshed)
            };

            if changed {
                // CAS write — if a refresh bumped the gen between our snapshot and now,
                // the write + emit are skipped. Prevents the stale-overwrite race (Grinch #14).
                let wrote = {
                    let mgr = self.session_manager.read().await;
                    mgr.set_git_repos_if_gen(id, refreshed.clone(), gen_snapshot)
                        .await
                };

                if wrote {
                    let _ = self.app_handle.emit(
                        "session_git_repos",
                        GitReposPayload {
                            session_id: id.to_string(),
                            repos: refreshed.clone(),
                        },
                    );
                    self.cache.lock().unwrap().insert(id, refreshed);
                } else {
                    log::debug!(
                        "[GitWatcher] gen mismatch on session {} — refresh landed during poll; skipping stale emit",
                        id
                    );
                    // Invalidate our cache so the next tick re-evaluates against the refreshed list.
                    self.cache.lock().unwrap().remove(&id);
                }
            }
        }
    }

    /// Detect branch via `git rev-parse`, bounded by a 2s timeout. On timeout the
    /// pending future is dropped; `.kill_on_drop(true)` ensures the child `git.exe`
    /// is terminated so repeated polls can't leak processes.
    async fn detect_branch_with_timeout(working_dir: &str) -> Option<String> {
        match tokio::time::timeout(DETECT_TIMEOUT, Self::detect_branch(working_dir)).await {
            Ok(result) => result,
            Err(_) => {
                log::warn!(
                    "[GitWatcher] detect_branch timed out for {} (>{}s); treating as no-branch",
                    working_dir,
                    DETECT_TIMEOUT.as_secs()
                );
                None
            }
        }
    }

    async fn detect_branch(working_dir: &str) -> Option<String> {
        #[cfg(windows)]
        const CREATE_NO_WINDOW: u32 = 0x08000000;

        let mut cmd = tokio::process::Command::new("git");
        cmd.args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(working_dir)
            .kill_on_drop(true);
        if let Some(git_ceiling_dirs) =
            crate::config::session_context::git_ceiling_directories_for_session_root(working_dir)
        {
            cmd.env("GIT_CEILING_DIRECTORIES", git_ceiling_dirs);
        }

        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);

        let output = cmd.output().await;

        match output {
            Ok(out) if out.status.success() => {
                let branch = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if branch.is_empty() || branch == "HEAD" {
                    None
                } else {
                    Some(branch)
                }
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CAS semantic guard (validation #18): a stale `expected_gen` must fail and leave state untouched.
    #[tokio::test]
    async fn set_git_repos_if_gen_rejects_stale_gen() {
        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "cmd".into(),
                vec![],
                "C:/tmp".into(),
                None,
                None,
                vec![SessionRepo {
                    label: "A".into(),
                    source_path: "C:/a".into(),
                    branch: None,
                }],
                false,
            )
            .await
            .expect("create_session");

        let id = session.id;
        let gen0 = mgr.get_git_repos_gen(id).await.unwrap();

        // Simulate a refresh landing: bump gen by writing new repos.
        mgr.set_git_repos(
            id,
            vec![SessionRepo {
                label: "A2".into(),
                source_path: "C:/a2".into(),
                branch: None,
            }],
        )
        .await;

        // Stale write with pre-refresh gen must fail.
        let wrote = mgr
            .set_git_repos_if_gen(
                id,
                vec![SessionRepo {
                    label: "A-stale".into(),
                    source_path: "C:/a-stale".into(),
                    branch: Some("main".into()),
                }],
                gen0,
            )
            .await;
        assert!(!wrote, "stale gen must be rejected");

        // State is still the refreshed list (label "A2"), not the stale one.
        let gen_now = mgr.get_git_repos_gen(id).await.unwrap();
        assert_ne!(gen_now, gen0);

        // Write with current gen succeeds.
        let wrote2 = mgr
            .set_git_repos_if_gen(
                id,
                vec![SessionRepo {
                    label: "A2".into(),
                    source_path: "C:/a2".into(),
                    branch: Some("feature".into()),
                }],
                gen_now,
            )
            .await;
        assert!(wrote2, "current gen must succeed");
    }
}
