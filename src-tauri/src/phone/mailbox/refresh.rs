use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectRefreshEventPayload {
    pub(crate) id: String,
    pub(crate) project_path: String,
    pub(crate) changed_path: Option<String>,
    pub(crate) changed_name: Option<String>,
    pub(crate) reason: String,
}

#[derive(Debug, Default)]
pub(crate) struct ProjectRefreshPollBatch {
    pub(crate) payloads: Vec<ProjectRefreshEventPayload>,
    pub(crate) processed_paths: Vec<PathBuf>,
}
