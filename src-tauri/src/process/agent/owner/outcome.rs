/// Outcome of one probe process. `Failed` carries the sanitized detail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeOutcome {
    Version(String),
    Failed(String),
}

/// Outcome of a probe whose process owner observes a retained cancellation
/// signal. Cancelled and `CleanupFailed` are returned only after every native
/// process/handle and pipe reader is definitively settled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CancellableProbeOutcome {
    Completed(ProbeOutcome),
    Cancelled,
    CleanupFailed(String),
}
