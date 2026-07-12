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
