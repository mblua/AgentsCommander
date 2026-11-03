use std::sync::Arc;

use super::registry::{ProcessTreeBackend, ResourceError, ResourceMonitorState};
use super::types::{
    ObservedProcess, ObservedProcessTree, ProcessIdentity, ProcessMemory, TerminateOutcome,
};

#[cfg(windows)]
mod platform;

#[cfg(not(windows))]
mod stub;

#[cfg(windows)]
pub use platform::PlatformProcessTreeBackend;

#[cfg(not(windows))]
pub use stub::PlatformProcessTreeBackend;

impl ResourceMonitorState {
    #[cfg(test)]
    pub fn new() -> Self {
        Self::with_spawn_registry(Arc::new(crate::pty::spawn_diagnostics::SpawnRegistry::new()))
    }

    pub fn with_spawn_registry(spawn: Arc<crate::pty::spawn_diagnostics::SpawnRegistry>) -> Self {
        Self::with_backend_and_spawn_registry(Arc::new(PlatformProcessTreeBackend::new()), spawn)
    }

    // P03 wires this additive constructor; old composition remains disabled in P02.
    #[allow(dead_code)]
    pub(crate) fn with_spawn_registry_and_diagnostics(
        spawn: Arc<crate::pty::spawn_diagnostics::SpawnRegistry>,
        diagnostics: Arc<crate::cleanup_diagnostics::store::Diagnostics>,
    ) -> Self {
        Self::with_backend_spawn_and_diagnostics(
            Arc::new(PlatformProcessTreeBackend::new()),
            spawn,
            diagnostics,
        )
    }
}
