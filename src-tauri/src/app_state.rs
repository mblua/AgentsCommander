//! Shared Tauri state types.

use std::collections::HashSet;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

/// Tracks which sessions are currently detached into their own windows.
pub type DetachedSessionsState = Arc<Mutex<HashSet<uuid::Uuid>>>;

/// Serializes the config-seed critical section (`perform_config_seed`) for a
/// replica, so two concurrent same-replica spawns cannot clobber each other's
/// in-flight `<dest>.acseed-*` scratch during `clear_stale_seed_scratch`
/// (grinch HIGH-1; see `config/config_seed.rs` CONCURRENCY CONTRACT). Acquired
/// in `commands::session.rs` around the seed swap.
pub type ConfigSeedLockState = Arc<tokio::sync::Mutex<()>>;

/// §224 A.2.5 / G1 — set true while the post-startup session-restore loop is
/// running (`lib.rs` setup task that calls `create_session_inner` for every
/// persisted session). Read by `mailbox::handle_close_session` to decide
/// whether `session_ids.is_empty()` means "no live session for this FQN" or
/// "restore loop hasn't reached this session yet — retry briefly."
pub struct RestoreInProgress(pub AtomicBool);

/// Process-lived owners shared by every runtime and explicit component constructor.
#[derive(Clone)]
pub struct AppOwners {
    pub response_effects: Arc<crate::session::selection::response_close::ResponseEffectRegistry>,
    pub coordinator_order: Arc<crate::config::coordinator_clocks::CoordinatorCreateOrder>,
    pub spawn: Arc<crate::pty::spawn_diagnostics::SpawnRegistry>,
    pub messages: Arc<crate::config::injected_messages::MessageWarnings>,
    pub remote: Arc<crate::pty::remote_watcher::RemoteActivityState>,
    pub git: Arc<crate::pty::git_watcher::GitState>,
    pub activity: Arc<crate::config::activity_log::ActivityCoalescer>,
    pub capture: Arc<crate::capture::state::CaptureState>,
    #[cfg(windows)]
    pub git_guard: Arc<crate::pty::local_backend::GitGuardTempSequence>,
}

#[allow(clippy::new_without_default)]
impl AppOwners {
    pub fn new() -> Self {
        Self {
            response_effects: Arc::new(
                crate::session::selection::response_close::ResponseEffectRegistry::new(),
            ),
            coordinator_order: Arc::new(
                crate::config::coordinator_clocks::CoordinatorCreateOrder::new(),
            ),
            spawn: Arc::new(crate::pty::spawn_diagnostics::SpawnRegistry::new()),
            messages: Arc::new(crate::config::injected_messages::MessageWarnings::new()),
            remote: Arc::new(crate::pty::remote_watcher::RemoteActivityState::new()),
            git: Arc::new(crate::pty::git_watcher::GitState::new()),
            activity: Arc::new(crate::config::activity_log::ActivityCoalescer::new()),
            capture: Arc::new(crate::capture::state::CaptureState::new()),
            #[cfg(windows)]
            git_guard: Arc::new(crate::pty::local_backend::GitGuardTempSequence::new()),
        }
    }
}
