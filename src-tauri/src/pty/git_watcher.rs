use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::{AppHandle, Emitter};
use uuid::Uuid;

use crate::session::manager::SessionManager;

const POLL_INTERVAL: Duration = Duration::from_secs(5);

pub struct GitWatcher {
    session_manager: Arc<tokio::sync::RwLock<SessionManager>>,
    app_handle: AppHandle,
    cache: Mutex<HashMap<Uuid, Option<String>>>,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct GitBranchPayload {
    session_id: String,
    branch: Option<String>,
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

    pub fn start(self: &Arc<Self>) {
        let watcher = Arc::clone(self);
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime for GitWatcher");
            rt.block_on(async move {
                loop {
                    tokio::time::sleep(POLL_INTERVAL).await;
                    watcher.poll().await;
                }
            });
        });
    }

    pub fn remove_session(&self, id: Uuid) {
        self.cache.lock().unwrap().remove(&id);
    }

    async fn poll(&self) {
        let dirs = {
            let mgr = self.session_manager.read().await;
            mgr.get_sessions_directories().await
        };

        for (id, working_dir) in dirs {
            let branch = Self::detect_branch(&working_dir).await;

            // Check cache and update - lock scope kept short and before any .await
            let changed = {
                let cache = self.cache.lock().unwrap();
                cache.get(&id) != Some(&branch)
            };

            if changed {
                // Update session manager
                {
                    let mgr = self.session_manager.read().await;
                    mgr.set_git_branch(id, branch.clone()).await;
                }

                // Emit event to frontend
                let _ = self.app_handle.emit(
                    "session_git_branch",
                    GitBranchPayload {
                        session_id: id.to_string(),
                        branch: branch.clone(),
                    },
                );

                self.cache.lock().unwrap().insert(id, branch);
            }
        }
    }

    async fn detect_branch(working_dir: &str) -> Option<String> {
        #[cfg(windows)]
        const CREATE_NO_WINDOW: u32 = 0x08000000;

        let mut cmd = tokio::process::Command::new("git");
        cmd.args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(working_dir);

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
