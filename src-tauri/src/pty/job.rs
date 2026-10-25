//! Compatibility exports for native process-tree ownership.

pub(crate) use crate::process::host::job::SettlementAttempt;

#[cfg(windows)]
pub use crate::process::host::job::JobObject;

#[cfg(windows)]
pub(crate) use crate::process::host::job::{spawn_suspended_contained, ContainedSpawnError};

#[cfg(all(test, windows))]
pub(crate) use crate::process::host::job::{
    with_contained_spawn_test_hook, with_contained_spawn_test_hook_after_spawns, InjectedFailure,
    SpawnTestHook,
};

#[cfg(not(windows))]
pub use crate::process::host::job::JobObject;
