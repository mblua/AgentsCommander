use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::window_capture::WindowCaptureError;

/// Daemon-wide bounds for native discovery and capture work. Each handler
/// acquires a permit before it starts a blocking task and keeps it until that
/// task owns no native or temporary-artifact resource.
pub(crate) struct WindowCaptureAdmission {
    discovery: Arc<Semaphore>,
    capture: Arc<Semaphore>,
}

impl Default for WindowCaptureAdmission {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowCaptureAdmission {
    pub(crate) fn new() -> Self {
        Self {
            discovery: Arc::new(Semaphore::new(4)),
            capture: Arc::new(Semaphore::new(1)),
        }
    }

    pub(crate) fn try_acquire_discovery(&self) -> Result<OwnedSemaphorePermit, WindowCaptureError> {
        Arc::clone(&self.discovery)
            .try_acquire_owned()
            .map_err(|_| WindowCaptureError::CaptureBusy)
    }

    pub(crate) fn try_acquire_capture(&self) -> Result<OwnedSemaphorePermit, WindowCaptureError> {
        Arc::clone(&self.capture)
            .try_acquire_owned()
            .map_err(|_| WindowCaptureError::CaptureBusy)
    }
}
