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
    pub fn new() -> Self {
        Self::with_backend(Arc::new(PlatformProcessTreeBackend::new()))
    }
}
