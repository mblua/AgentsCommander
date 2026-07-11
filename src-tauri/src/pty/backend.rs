use std::any::Any;
use std::path::PathBuf;

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::errors::AppError;
use crate::pty::output::{PtyOutputTarget, PtyScreenSnapshot};
use crate::resource_monitor::{ResourceLaunchRegistration, ResourceLogicalAgentSlot};
use crate::session::profile::IdleTuning;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionBackendKind {
    #[default]
    LocalProcess,
    ContainerTransport,
}

pub struct BackendSpawnSpec {
    pub id: Uuid,
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

    fn kill_all_jobs(&self) -> (usize, usize);
}
