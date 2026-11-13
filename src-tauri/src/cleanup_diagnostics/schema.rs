//! Typed diagnostic facts; no registry, PTY, filesystem or authority dependencies.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub(crate) const COVERAGE: &str = "reaper_individual_only";
pub(crate) const EXCLUDED: [&str; 6] = [
    "Job-first",
    "child",
    "Job-close",
    "PTY-close",
    "shutdown_bulk",
    "crash",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct ProcessIdentity {
    pub pid: u32,
    pub start_time: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Binding {
    pub run_id: Uuid,
    pub session_id: Uuid,
    pub root: Option<ProcessIdentity>,
    pub registration_generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IdentityStatus {
    Same,
    Reused,
    Gone,
    Unreadable,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CurrentIdentity {
    pub status: IdentityStatus,
    pub identity: Option<ProcessIdentity>,
    pub executable: Option<String>,
    pub os_error: Option<u32>,
}

impl CurrentIdentity {
    pub(crate) fn unknown() -> Self {
        Self {
            status: IdentityStatus::Unknown,
            identity: None,
            executable: None,
            os_error: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ObservedProcess {
    pub identity: ProcessIdentity,
    pub parent_pid: Option<u32>,
    pub parent_identity: Option<ProcessIdentity>,
    pub executable: Option<String>,
    pub depth: u32,
    pub kill_allowed: bool,
    pub sample_id: Option<Uuid>,
    /// Only known edges; absence never invents a historical ancestor.
    pub chain: Vec<ProcessIdentity>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Provenance {
    Retained,
    Fresh,
    Both,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SelectionRow {
    pub index: usize,
    pub expected: ProcessIdentity,
    pub retained: Option<ObservedProcess>,
    pub fresh: Option<ObservedProcess>,
    pub provenance: Provenance,
    pub rule: String,
    pub current: CurrentIdentity,
    /// Exact identity matches describe inclusion, never ownership.
    pub matches_app: Option<bool>,
    pub matches_other_roots: Vec<ProcessIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RawApiResult {
    /// None means the API was not called; never infer a call from preparation.
    pub returned: Option<bool>,
    pub os_error: Option<u32>,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum ExitObservation {
    Exited { code: Option<u32> },
    Timeout,
    Error { os_error: Option<u32> },
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Summary {
    pub partial: bool,
    pub evidence_gap: bool,
    pub killed_processes: usize,
    /// A footer precedes registry accounting; it cannot confirm that commit.
    pub accounting: String,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub(crate) enum EvidenceEvent {
    OperationBegin,
    Target {
        row: Box<SelectionRow>,
    },
    SelectionComplete {
        count: usize,
    },
    PreCall {
        index: usize,
        precheck: CurrentIdentity,
        final_handle: CurrentIdentity,
    },
    ApiReturn {
        index: usize,
        result: RawApiResult,
    },
    ExitObserved {
        index: usize,
        exit: ExitObservation,
    },
    OperationComplete {
        summary: Summary,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EvidenceRecord {
    pub seq: u64,
    #[serde(flatten)]
    pub event: EvidenceEvent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EvidenceError {
    pub stage: String,
    pub message: String,
    pub effects_started: bool,
    pub known_api_return: Option<RawApiResult>,
    pub known_exit: Option<ExitObservation>,
}

impl EvidenceError {
    pub(crate) fn new(stage: &str, message: impl ToString) -> Self {
        Self {
            stage: stage.into(),
            message: message.to_string(),
            effects_started: false,
            known_api_return: None,
            known_exit: None,
        }
    }
}

impl std::fmt::Display for EvidenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.stage, self.message)
    }
}

impl std::error::Error for EvidenceError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    Inspect,
    ArmNextCleanup,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Receipt {
    pub status: String,
    pub operation_id: Option<Uuid>,
    pub path: Option<String>,
    pub evidence_gap: bool,
    pub errors: Vec<String>,
}
