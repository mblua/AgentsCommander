use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use chrono::{DateTime, Utc};

use super::{GhCommandSpec, RemoteActivityPayload};

/// The emitter seam's boxed shape, named so the struct field is not a
/// `clippy::type_complexity` trigger.
pub(crate) type Emitter = Box<dyn FnMut(&RemoteActivityPayload) + Send>;

pub(crate) struct GhCallOutput {
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) success: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum FailureKind {
    Timeout,
    RateLimited,
    SecondaryRateLimited,
    NotAuthenticated,
    Incomplete,
    Other,
}

pub(crate) type GhFuture =
    Pin<Box<dyn std::future::Future<Output = Result<GhCallOutput, FailureKind>> + Send>>;
pub(crate) type GhSpawner = Arc<dyn Fn(GhCommandSpec) -> GhFuture + Send + Sync>;

pub(crate) type LocalGitFuture =
    Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send>>;
pub(crate) type LocalGitRunner = Arc<dyn Fn(String, Vec<String>) -> LocalGitFuture + Send + Sync>;

pub(crate) type GhProbe = Arc<dyn Fn() -> Option<PathBuf> + Send + Sync>;

/// Returns a fraction in [0.0, 1.0). Production draws from `getrandom`; tests
/// pin it so every backoff assertion is exact.
pub(crate) type JitterSource = Arc<dyn Fn() -> f64 + Send + Sync>;

/// The process seams plus the jitter source. Production wires them in
/// `production_seams`; tests inject recorded results so no test ever spawns
/// `gh` or touches the network.
pub(crate) struct RemoteSweeperSeams {
    pub(crate) probe: GhProbe,
    pub(crate) spawner: GhSpawner,
    pub(crate) local_git: LocalGitRunner,
    pub(crate) jitter: JitterSource,
    /// Destination directory for the neutral snapshot, or `None` when the host
    /// wired no instance config directory. Captured once at construction; see
    /// [`SnapshotPersistence`].
    pub(crate) snapshot_dir: Option<PathBuf>,
    /// Reads the publication instant written as `generatedAt`, invoked once per
    /// round immediately before the blocking write.
    pub(crate) publication_clock: PublicationClock,
}

/// The publication clock seam. Production is `Utc::now`; tests inject a fixed
/// instant so freshness assertions are exact.
pub(crate) type PublicationClock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

/// Program-independent failure classification, from the text `gh` prints.
/// Deliberately textual: `gh api` reports HTTP outcomes as exit status 1 plus a
/// message, and the exact argument list is pinned by test 1, so no `--include`
/// is available to read headers.
pub(crate) fn failure_kind(output: &GhCallOutput) -> FailureKind {
    let text = output.stderr.to_ascii_lowercase();
    if text.contains("secondary rate limit") || text.contains("abuse detection") {
        FailureKind::SecondaryRateLimited
    } else if text.contains("rate limit") || text.contains("x-ratelimit-remaining: 0") {
        FailureKind::RateLimited
    } else if text.contains("not logged into any github hosts") || text.contains("http 401") {
        FailureKind::NotAuthenticated
    } else {
        FailureKind::Other
    }
}
