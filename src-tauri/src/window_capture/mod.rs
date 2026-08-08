mod types;

#[allow(unused_imports)]
pub(crate) use types::{
    CaptureCancellation, CaptureMetadata, CaptureOptions, CaptureSupportLevel, DiscoveredWindow,
    MonitorIntersection, PendingCaptureArtifact, PreparedOutput, TargetFingerprint, WindowBounds,
    WindowCaptureError, WindowDiagnostics, WindowTargetId, WindowVisibility, TARGET_TTL,
};

#[cfg(test)]
pub(crate) use types::fixture_discovered_window;
