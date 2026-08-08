use super::{CaptureCancellation, DiscoveredWindow, DiscoveryFilter, WindowCaptureError};

pub(super) fn discover(
    filter: DiscoveryFilter,
    cancellation: &CaptureCancellation,
) -> Result<Vec<DiscoveredWindow>, WindowCaptureError> {
    let _ = (filter, cancellation);
    Err(WindowCaptureError::CaptureUnsupported)
}
