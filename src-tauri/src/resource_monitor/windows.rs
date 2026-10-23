use super::registry::{ProcessTreeBackend, ResourceError};
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
