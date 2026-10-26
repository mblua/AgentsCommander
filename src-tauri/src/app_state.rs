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
    #[cfg(windows)]
    pub git_guard: Arc<crate::pty::local_backend::GitGuardTempSequence>,
}

#[allow(clippy::new_without_default)]
impl AppOwners {
    pub fn new() -> Self {
        Self {
            #[cfg(windows)]
            git_guard: Arc::new(crate::pty::local_backend::GitGuardTempSequence::new()),
        }
    }
}
