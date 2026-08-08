mod artifact;
mod types;

#[cfg(windows)]
mod windows;

use std::path::Path;

#[allow(unused_imports)]
pub(crate) use types::{
    CaptureCancellation, CaptureMetadata, CaptureOptions, CaptureSupportLevel, DiscoveredWindow,
    DiscoveryFilter, MonitorIntersection, PendingCaptureArtifact, PreparedOutput,
    TargetFingerprint, WindowBounds, WindowCaptureError, WindowDiagnostics, WindowTargetId,
    WindowVisibility, TARGET_TTL,
};

#[cfg(test)]
pub(crate) use types::fixture_discovered_window;

/// The only native capture seam. Transport modules can pass it opaque values,
/// but cannot supply an HWND, credentials, or an API request.
#[derive(Clone, Copy, Default)]
pub(crate) struct WindowCapture;

impl WindowCapture {
    pub(crate) fn discover(
        self,
        filter: DiscoveryFilter,
        cancellation: &CaptureCancellation,
    ) -> Result<Vec<DiscoveredWindow>, WindowCaptureError> {
        if cancellation.is_cancelled() {
            return Err(WindowCaptureError::CaptureTimeout);
        }

        #[cfg(windows)]
        {
            windows::discover(filter, cancellation)
        }

        #[cfg(not(windows))]
        {
            let _ = filter;
            Err(WindowCaptureError::CaptureUnsupported)
        }
    }

    pub(crate) fn prepare_output(
        self,
        output: &Path,
    ) -> Result<PreparedOutput, WindowCaptureError> {
        let _ = output;
        Err(WindowCaptureError::CaptureUnsupported)
    }

    pub(crate) fn capture_to_pending_artifact(
        self,
        fingerprint: TargetFingerprint,
        prepared_output: PreparedOutput,
        options: CaptureOptions,
        cancellation: &CaptureCancellation,
    ) -> Result<PendingCaptureArtifact, WindowCaptureError> {
        let _ = (fingerprint, prepared_output, options);
        if cancellation.is_cancelled() {
            return Err(WindowCaptureError::CaptureTimeout);
        }
        Err(WindowCaptureError::CaptureUnsupported)
    }

    pub(crate) fn abort_pending(self, pending: PendingCaptureArtifact) {
        let _ = pending;
    }
}
