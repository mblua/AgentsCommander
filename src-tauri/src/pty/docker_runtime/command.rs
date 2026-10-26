use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use uuid::Uuid;

use crate::errors::AppError;
use crate::pty::container_runtime::ContainerRuntimeHandle;

pub(super) fn normalized_app_error_text(error: &AppError) -> String {
    match error {
        AppError::PtyError(message) => message.clone(),
        _ => error.to_string(),
    }
}

#[derive(Default)]
pub(super) struct RetainedDockerStartCleanupRegistry {
    pub(super) entries: Mutex<HashMap<Uuid, Arc<RetainedDockerStartCleanup>>>,
}

pub(super) struct RetainedDockerStartCleanup {
    pub(super) handle: ContainerRuntimeHandle,
    pub(super) state: Mutex<RetainedDockerStartCleanupState>,
}

#[derive(Default)]
pub(super) struct RetainedDockerStartCleanupState {
    pub(super) in_flight: bool,
    pub(super) last_error: Option<String>,
}
