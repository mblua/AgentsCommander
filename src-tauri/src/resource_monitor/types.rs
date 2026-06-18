use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessIdentity {
    pub pid: u32,
    pub creation_time_100ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservedProcess {
    pub identity: ProcessIdentity,
    pub parent_pid: Option<u32>,
    pub parent_identity: Option<ProcessIdentity>,
    pub exe_name: String,
    pub depth: u32,
    pub private_bytes: Option<u64>,
    pub working_set_bytes: Option<u64>,
    pub cpu_percent: Option<f64>,
    pub kill_allowed: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ObservedProcessTree {
    pub processes: Vec<ObservedProcess>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessMemory {
    pub private_bytes: Option<u64>,
    pub working_set_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminateOutcome {
    Terminated,
    AlreadyGone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResourceOverallState {
    Ok,
    Warn,
    Critical,
    Enforcing,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResourceNetworkState {
    Unknown,
    Observed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResourceGroupState {
    Running,
    Terminating,
    Terminated,
    Quarantined,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResourceKillReason {
    User,
    Watchdog,
    SessionDestroy,
    AppShutdown,
    SpawnRollback,
}

impl ResourceKillReason {
    pub fn as_log_reason(self) -> &'static str {
        match self {
            ResourceKillReason::User => "user",
            ResourceKillReason::Watchdog => "watchdog",
            ResourceKillReason::SessionDestroy => "session-destroy",
            ResourceKillReason::AppShutdown => "app-shutdown",
            ResourceKillReason::SpawnRollback => "spawn-rollback",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceProcessSnapshot {
    pub identity: ProcessIdentity,
    pub pid: u32,
    pub parent_pid: Option<u32>,
    pub name: String,
    pub exe_name: String,
    pub private_bytes: Option<u64>,
    pub working_set_bytes: Option<u64>,
    pub cpu_percent: Option<f64>,
    pub owned: bool,
    pub kill_allowed: bool,
    pub depth: u32,
}

impl From<&ObservedProcess> for ResourceProcessSnapshot {
    fn from(value: &ObservedProcess) -> Self {
        Self {
            identity: value.identity,
            pid: value.identity.pid,
            parent_pid: value.parent_pid,
            name: value.exe_name.clone(),
            exe_name: value.exe_name.clone(),
            private_bytes: value.private_bytes,
            working_set_bytes: value.working_set_bytes,
            cpu_percent: value.cpu_percent,
            owned: true,
            kill_allowed: value.kill_allowed,
            depth: value.depth,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceAgentGroupSnapshot {
    pub session_id: String,
    pub name: String,
    /// #516 - human-readable workgroup label (e.g. "wg-5-dev-team"), or `None`
    /// for non-WG launches. Root-agent groups carry "Root agent".
    pub workgroup: Option<String>,
    /// #516 - human-readable agent name (e.g. "dev-rust"), or `None` when the
    /// launch cwd carries no replica identity.
    pub agent: Option<String>,
    pub root_pid: u32,
    pub root_identity: ProcessIdentity,
    pub state: ResourceGroupState,
    pub descendants_observed: bool,
    pub process_count: usize,
    pub private_bytes: Option<u64>,
    pub working_set_bytes: Option<u64>,
    pub cpu_percent: Option<f64>,
    pub network_state: ResourceNetworkState,
    pub network_summary: String,
    pub processes: Vec<ResourceProcessSnapshot>,
    pub kill_allowed: bool,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceSnapshot {
    pub captured_at: DateTime<Utc>,
    pub overall_state: ResourceOverallState,
    pub monitor_enabled: bool,
    pub active_agent_groups: usize,
    pub max_concurrent_agent_groups: u32,
    pub app_private_bytes: Option<u64>,
    pub app_working_set_bytes: Option<u64>,
    pub network_state: ResourceNetworkState,
    pub network_summary: String,
    pub groups: Vec<ResourceAgentGroupSnapshot>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceKillRequest {
    pub session_id: String,
    pub reason: ResourceKillReason,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceKillResult {
    pub session_id: String,
    pub state: ResourceGroupState,
    pub killed_processes: Vec<ProcessIdentity>,
    pub quarantined: bool,
    pub message: String,
}

#[derive(Debug, Clone, Copy)]
pub struct ResourceLimits {
    pub monitor_enabled: bool,
    pub max_concurrent_agent_processes: u32,
    pub group_warn_private_bytes: u64,
    pub group_kill_private_bytes: u64,
    pub process_kill_private_bytes: u64,
}

impl From<&crate::config::settings::AppSettings> for ResourceLimits {
    fn from(value: &crate::config::settings::AppSettings) -> Self {
        Self {
            monitor_enabled: value.resource_monitor_enabled,
            max_concurrent_agent_processes: value.max_concurrent_agent_processes,
            group_warn_private_bytes: value.agent_group_warn_private_bytes,
            group_kill_private_bytes: value.agent_group_kill_private_bytes,
            process_kill_private_bytes: value.agent_process_kill_private_bytes,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResourceLaunchMetadata {
    pub session_id: Uuid,
    pub name: String,
    pub agent_id: Option<String>,
    pub agent_label: Option<String>,
    /// #516 - workgroup label derived from the spawn cwd (e.g. "wg-5-dev-team"),
    /// "Root agent" for root-agent launches, or `None` for non-WG launches.
    pub workgroup: Option<String>,
    /// #516 - agent name derived from the spawn cwd (e.g. "dev-rust"), or `None`.
    pub agent: Option<String>,
}
