use std::any::Any;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::errors::AppError;
use crate::pty::output::PtyScreenSnapshot;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionBackendKind {
    #[default]
    LocalProcess,
}

pub trait PtyBackend: Any + Send + Sync {
    fn as_any(&self) -> &dyn Any;

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
