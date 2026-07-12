use std::any::Any;
use std::path::PathBuf;

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::errors::AppError;
use crate::pty::output::{PtyOutputTarget, PtyScreenSnapshot};
use crate::resource_monitor::{ResourceLaunchRegistration, ResourceLogicalAgentSlot};
use crate::session::profile::{CodingAgentKind, IdleTuning};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionBackendKind {
    #[default]
    LocalProcess,
    ContainerTransport,
}

/// #973 - the terminal size a session's PTY is opened at.
///
/// AC used to open every ConPTY at a hardcoded 120x30 and let the frontend correct it a few
/// hundred milliseconds later. That correction lands in the middle of a coding agent's TUI
/// startup, and a resize there makes Codex redraw its still-empty viewport and lose the
/// wakeup for the content that becomes ready right after: the terminal stays blank, alive,
/// until any key is pressed. Measured, outside AC: a resize burst at 250 ms hangs it 8/10;
/// opening at the right size and never resizing hangs it 0/10.
///
/// So the size the view has already fitted to is handed down at spawn, and no resize has to
/// happen at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PtyViewport {
    pub cols: u16,
    pub rows: u16,
}

impl PtyViewport {
    /// What AC has always opened PTYs at. Used by every caller with no view of its own:
    /// the startup-restore loop, the delivery loop, the phone mailbox, the CLI, tests.
    /// Those sessions are not attached to a terminal, so nothing resizes them, so they were
    /// never exposed to this bug (18/18 of them painted in the user's production log).
    pub const DEFAULT: Self = Self {
        cols: 120,
        rows: 30,
    };

    /// A size the frontend fitted before the session existed.
    ///
    /// A zero dimension is not hypothetical: xterm's `fit()` really does return one while
    /// its container is still being laid out. Opening a 0-column ConPTY is worse than the
    /// bug we are fixing, so a degenerate size warns and falls back instead of failing the
    /// spawn.
    pub fn from_fit(cols: u16, rows: u16) -> Self {
        if cols == 0 || rows == 0 {
            log::warn!(
                "[pty] ignoring degenerate fitted viewport {cols}x{rows}, opening at {}x{}",
                Self::DEFAULT.cols,
                Self::DEFAULT.rows
            );
            return Self::DEFAULT;
        }
        Self { cols, rows }
    }
}

pub struct BackendSpawnSpec {
    pub id: Uuid,
    /// #942 - the configured coding-agent PROFILE id (`settings.agents[].id`, an
    /// opaque string like `agent_1782513272568_0`), or None. It is NOT the CLI:
    /// several profiles can run the same `codex` binary, and a coding agent can be
    /// launched with no profile id at all. Diagnostics log it, and never key on it.
    pub agent_id: Option<String>,
    /// #942 - the CLI actually being launched, from the canonical detector
    /// (`CodingAgentKind::detect`). This is the identity the diagnostics key on: the
    /// stall predicate and the "concurrent startups on the shared ~/.codex" counter.
    pub coding_agent: Option<CodingAgentKind>,
    pub cmd: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub selected_cwd: Option<String>,
    pub cols: u16,
    pub rows: u16,
    pub container_image: Option<String>,
    pub configured_env: Vec<(String, String)>,
    pub env_remove_keys: Vec<String>,
    pub env_unset: Vec<String>,
    pub extra_env: Vec<(String, String)>,
    pub idle_tuning: IdleTuning,
    pub output_target: PtyOutputTarget,
    pub resource_registration: Option<ResourceLaunchRegistration>,
    pub logical_resource_slot: Option<ResourceLogicalAgentSlot>,
    /// #930 - resolved host-credential copy-in for container sessions. None for
    /// local-process sessions and when copy-in is disabled/not applicable.
    pub container_credential: Option<crate::pty::container_credentials::ContainerCredentialPlan>,
    /// #935 - read-write repo bind mounts for container sessions. Empty for
    /// local-process sessions and when the replica has no admissible repos.
    pub container_repo_mounts: Vec<crate::pty::container_repos::ContainerRepoMount>,
}

pub trait PtyBackend: Any + Send + Sync {
    fn as_any(&self) -> &dyn Any;

    fn spawn(&self, spec: BackendSpawnSpec) -> BoxFuture<'_, Result<(), AppError>>;

    fn write(&self, id: Uuid, data: &[u8]) -> Result<(), AppError>;

    fn resize(&self, id: Uuid, cols: u16, rows: u16) -> Result<(), AppError>;

    fn kill(&self, id: Uuid) -> Result<(), AppError>;

    fn has_session(&self, id: Uuid) -> bool;

    fn get_screen_snapshot(&self, id: Uuid) -> Option<PtyScreenSnapshot>;

    fn get_pty_size(&self, id: Uuid) -> Option<(u16, u16)>;

    fn register_response_watcher(
        &self,
        session_id: Uuid,
        request_id: String,
        response_dir: PathBuf,
    );

    fn terminate_job_for_session(&self, id: Uuid) -> bool;

    /// #942 - tag an imminent AC stop with the liveness of the child BEFORE any process
    /// is touched, for callers that are about to kill outside the PTY layer (the resource
    /// monitor kills a process tree by pid). Diagnostics only; the default no-op covers
    /// backends with no local child (container transport).
    fn publish_stop_witness(&self, _id: Uuid, _source: &str) {}

    fn kill_all_jobs(&self) -> (usize, usize);
}
