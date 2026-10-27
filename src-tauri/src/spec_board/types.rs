use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SpecBoardFileKind {
    Mermaid,
    Markdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpecBoardSnapshot {
    pub id: String,
    pub label: String,
    pub created_at_ms: i64,
    pub source: SpecBoardSnapshotSource,
    pub content: String,
    pub diagram_source: String,
    pub hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SpecBoardSnapshotSource {
    Initial,
    Open,
    Edit,
    Save,
    External,
    Checkout,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SnapshotFile {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) created_at_ms: i64,
    pub(crate) sequence: u64,
    pub(crate) source: SpecBoardSnapshotSource,
    pub(crate) content: String,
    pub(crate) diagram_source: String,
    pub(crate) file_kind: SpecBoardFileKind,
    pub(crate) path: Option<String>,
    pub(crate) hash: String,
}
