use clap::Args;
use serde::{ser::SerializeMap, Deserialize, Serialize, Serializer};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};
use terminal_snapshot_renderer::{
    encode_ui_terminal_snapshot, ProtocolError, UiTerminalSelectionMode,
    UiTerminalSnapshotDocument,
};
use uuid::Uuid;

use crate::pty::manager::{PtyManager, PtySnapshotRouteProof, UiTerminalCaptureError};
use crate::session::manager::{SessionManager, TerminalSnapshotSessionFact};
use crate::session::selection::{SelectionCoordinator, SessionSelection};
use crate::shutdown::ShutdownSignal;
use crate::testability::window_placement::TESTABLE_EXE_NAME;
use crate::DetachedSessionsState;

pub const UI_AUTOMATION_DIR: &str = crate::config::instance_artifacts::UI_AUTOMATION_DIR_NAME;
pub const SESSION_FILE: &str = "session.json";
pub const REQUESTS_DIR: &str = "requests";
pub const RESPONSES_DIR: &str = "responses";
pub const ENV_ENABLE: &str = "AC_UI_AUTOMATION";
pub const BACKEND_AUTOMATION_WINDOW: &str = "__backend";
pub const RESOURCE_WATCHDOG_BACKEND_SELECTOR: &str = "resourceMonitor.watchdog";
const UI_AUTOMATION_LOG_TARGET: &str = "agentscommander_lib::ui_automation";

#[derive(Clone, Copy)]
enum UiAutomationLogEvent {
    InitializeFailed,
    ConfigIdentityFailed,
    PollFailed,
    RequestReadFailed,
    BackendResponseWriteFailed,
}

impl UiAutomationLogEvent {
    fn as_str(self) -> &'static str {
        match self {
            Self::InitializeFailed => "initialize_failed",
            Self::ConfigIdentityFailed => "config_identity_failed",
            Self::PollFailed => "poll_failed",
            Self::RequestReadFailed => "request_read_failed",
            Self::BackendResponseWriteFailed => "backend_response_write_failed",
        }
    }
}

fn ui_automation_log_message(
    event: UiAutomationLogEvent,
    request_id: Option<&str>,
    error: &'static str,
) -> String {
    match request_id {
        Some(request_id) => format!(
            "[ui-automation] event={} request={} error={error}",
            event.as_str(),
            request_id
        ),
        None => format!("[ui-automation] event={} error={error}", event.as_str()),
    }
}

const DEFAULT_TIMEOUT_MS: u64 = 5_000;
const MAX_TIMEOUT_MS: u64 = 60_000;
const POLL_MS: u64 = 50;
const FS_RETRY_COUNT: usize = 8;
const FS_RETRY_DELAY_MS: u64 = 25;
const SESSION_READ_RETRY_COUNT: usize = 8;
const SESSION_READ_RETRY_DELAY_MS: u64 = 25;
const CLI_MAX_AVAILABLE_TARGETS: usize = 8;
const CLI_MAX_TARGET_TEXT_CHARS: usize = 80;
const MAX_WINDOW_LABEL_BYTES: usize = 128;
const MAX_SELECTOR_BYTES: usize = 256;
const MAX_PREFIX_BYTES: usize = 256;
const MAX_ROLE_BYTES: usize = 32;
const MAX_STATE_PREDICATE_BYTES: usize = 64;
const MAX_TEXT_PREDICATE_CHARS: usize = 80;
const MAX_TEXT_PREDICATE_BYTES: usize = 320;
const MAX_VALUE_BYTES: usize = 16_384;
const MAX_SESSION_FILE_BYTES: usize = 16_384;
const MAX_REQUEST_FILE_BYTES: usize = 32_768;
const MAX_RESPONSE_JSON_BYTES: usize = 2_097_152;
const MAX_STDOUT_JSON_BYTES: usize = 2_097_152;
const MAX_REGISTERED_WINDOWS: usize = 32;
const MAX_PENDING_TOTAL: usize = 32;
const MAX_PENDING_PER_WINDOW: usize = 8;
const MAX_PENDING_TERMINAL_SNAPSHOTS: usize = 2;
const MAX_REQUEST_FILES_PER_SCAN: usize = 64;
const MAX_LIST_RETURN_TARGETS: usize = 50;
const MAX_LIST_SCAN_TARGETS: usize = 1_000;
const MAX_LIST_SCAN_ELEMENTS: usize = 20_000;
const MAX_LIST_OPEN_ROOTS: usize = 64;
const MAX_TERMINAL_ROWS: usize = 200;
const MAX_TERMINAL_COLUMNS: usize = 500;
const MAX_TERMINAL_CELLS: usize = 100_000;
const PROTOCOL_SCHEMA_VERSION: u32 = 1;
const SUPPORTED_ROLES: [&str; 25] = [
    "agent-preset",
    "alert",
    "button",
    "cell",
    "checkbox",
    "combobox",
    "dialog",
    "group",
    "input",
    "list",
    "menu",
    "menuitem",
    "metric",
    "overlay",
    "region",
    "row",
    "searchbox",
    "separator",
    "spinbutton",
    "status",
    "surface",
    "tab",
    "text",
    "textbox",
    "toolbar",
];
const CAPABILITY_ACTIONS: [&str; 10] = [
    "query",
    "list",
    "wait",
    "click",
    "contextClick",
    "hover",
    "focus",
    "setValue",
    "typeText",
    "backend",
];
const CAPABILITY_WAIT_PREDICATES: [&str; 8] = [
    "state", "text", "enabled", "disabled", "selected", "expanded", "focused", "absent",
];
const CAPABILITY_BACKEND_SELECTORS: [&str; 2] = [
    RESOURCE_WATCHDOG_BACKEND_SELECTOR,
    "terminal.snapshot",
];

pub trait AutomationConfigWitness: Send + Sync + 'static {
    fn canonical_path(&self) -> &Path;
    fn object_parts(&self) -> (u64, u64);
    fn verify_current(&self) -> bool;
}

pub trait InstanceIsolationTestHooks: Send + Sync + 'static {
    fn after_ui_cli_context_acquired_before_logger(&self) {}
    fn before_ui_cli_logger_config_phase(&self) {}
    fn before_config_writer(&self) {}
    fn after_owned_artifacts_published(&self) {}
}

pub struct NoopInstanceIsolationTestHooks;

impl InstanceIsolationTestHooks for NoopInstanceIsolationTestHooks {}

pub struct UiCliDispatchContext {
    config_witness: Arc<dyn AutomationConfigWitness>,
    identity_failed: AtomicBool,
}

impl UiCliDispatchContext {
    pub fn new(config_witness: Arc<dyn AutomationConfigWitness>) -> Self {
        Self {
            config_witness,
            identity_failed: AtomicBool::new(false),
        }
    }

    pub fn verify_current(&self) -> bool {
        if self.identity_failed.load(Ordering::SeqCst) {
            return false;
        }
        if self.config_witness.verify_current() {
            true
        } else {
            self.identity_failed.store(true, Ordering::SeqCst);
            false
        }
    }

    fn canonical_path(&self) -> &Path {
        self.config_witness.canonical_path()
    }

    fn with_owned_automation_fs<T>(
        &self,
        operation: impl FnOnce(&Path) -> Result<T, Value>,
    ) -> Result<T, Value> {
        if !self.verify_current() {
            return Err(automation_config_identity_unavailable_error());
        }
        operation(self.canonical_path())
    }
}

#[derive(Debug, Clone)]
struct LiveProcessIdentity {
    executable: PathBuf,
    started_at_unix_ms: i64,
}

trait LiveProcessIdentityProbe {
    fn probe(&self, pid: u32) -> Option<LiveProcessIdentity>;
}

trait SessionLoadTestHooks: Send + Sync {
    fn after_session_bytes_read(&self) {}
}

trait TerminalCaptureHooks: Send + Sync {
    fn after_detached_guard_acquired(&self) {}
    fn before_capture(&self) {}
    fn after_capture_before_owner_revalidation(&self) {}
    fn block_capture(&self) {}
}

struct NoopTerminalCaptureHooks;

impl TerminalCaptureHooks for NoopTerminalCaptureHooks {}

struct OsLiveProcessIdentityProbe;

#[derive(Debug, Args)]
pub struct UiQueryArgs {
    #[arg(long, default_value = "main")]
    pub window: String,
    #[arg(long)]
    pub selector: String,
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_MS)]
    pub timeout_ms: u64,
}

#[derive(Debug, Args)]
pub struct UiClickArgs {
    #[arg(long, default_value = "main")]
    pub window: String,
    #[arg(long)]
    pub selector: String,
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_MS)]
    pub timeout_ms: u64,
}

#[derive(Debug, Args)]
pub struct UiContextClickArgs {
    #[arg(long, default_value = "main")]
    pub window: String,
    #[arg(long)]
    pub selector: String,
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_MS)]
    pub timeout_ms: u64,
}

#[derive(Debug, Args)]
pub struct UiHoverArgs {
    #[arg(long, default_value = "main")]
    pub window: String,
    /// Required unless --leave.
    #[arg(long, required_unless_present = "leave")]
    pub selector: Option<String>,
    /// Park the pointer nowhere: fire the leave chain on whatever is currently hovered
    /// (up to <html>, relatedTarget null) and clear the sticky pointer. Target-free on
    /// purpose: the thing you want to release is normally already gone (the menu was torn
    /// down) or re-minted by <For>, and a cleanup step that fails when its subject is
    /// missing is not a cleanup step. Never fails.
    #[arg(long, conflicts_with = "selector")]
    pub leave: bool,
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_MS)]
    pub timeout_ms: u64,
}

#[derive(Debug, Args)]
pub struct UiSetArgs {
    #[arg(long, default_value = "main")]
    pub window: String,
    #[arg(long)]
    pub selector: String,
    #[arg(long)]
    pub value: String,
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_MS)]
    pub timeout_ms: u64,
}

#[derive(Debug, Args)]
pub struct UiTypeArgs {
    #[arg(long, default_value = "main")]
    pub window: String,
    #[arg(long)]
    pub selector: String,
    #[arg(long)]
    pub value: String,
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_MS)]
    pub timeout_ms: u64,
}

#[derive(Debug, Args)]
pub struct UiBackendArgs {
    #[arg(long)]
    pub selector: String,
    #[arg(long)]
    pub window: Option<String>,
    #[arg(long)]
    pub session: Option<String>,
    #[arg(long)]
    pub value: Option<String>,
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_MS)]
    pub timeout_ms: u64,
}

#[derive(Debug, Args)]
pub struct UiWaitArgs {
    #[arg(long, default_value = "main")]
    pub window: String,
    #[arg(long)]
    pub selector: String,
    #[arg(long)]
    pub state: Vec<String>,
    #[arg(long)]
    pub text: Vec<String>,
    #[arg(long)]
    pub enabled: bool,
    #[arg(long)]
    pub disabled: bool,
    #[arg(long)]
    pub selected: Vec<String>,
    #[arg(long)]
    pub expanded: Vec<String>,
    #[arg(long)]
    pub focused: Vec<String>,
    #[arg(long)]
    pub absent: bool,
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_MS)]
    pub timeout_ms: u64,
}

#[derive(Debug, Args)]
pub struct UiCapabilitiesArgs {
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_MS)]
    pub timeout_ms: u64,
}

#[derive(Debug, Args)]
pub struct UiListArgs {
    #[arg(long, default_value = "main")]
    pub window: String,
    #[arg(long)]
    pub prefix: Option<String>,
    #[arg(long)]
    pub role: Option<String>,
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_MS)]
    pub timeout_ms: u64,
}

#[derive(Debug, Args)]
pub struct UiFocusArgs {
    #[arg(long, default_value = "main")]
    pub window: String,
    #[arg(long)]
    pub selector: String,
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_MS)]
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UiAutomationSession {
    pub schema_version: u32,
    pub instance_id: String,
    pub pid: u32,
    pub token: String,
    pub exe_path: String,
    pub config_dir: String,
    pub window_inventory: UiAutomationWindowInventory,
    pub window_labels: Vec<String>,
    pub ready_window_labels: Vec<String>,
    pub started_at_unix_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UiAutomationWindowInventory {
    pub status: WindowInventoryStatus,
    pub observed_count: u32,
    pub limit: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum WindowInventoryStatus {
    Ready,
    Overflow,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UiAutomationRequest {
    pub schema_version: u32,
    pub instance_id: String,
    pub pid: u32,
    pub started_at_unix_ms: i64,
    pub request_id: String,
    pub token: String,
    pub exe_path: String,
    pub config_dir: String,
    pub window: String,
    pub action: UiAutomationAction,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub selector: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_window: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<UiTerminalSessionSelector>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_unix_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "camelCase", deny_unknown_fields)]
pub enum UiTerminalSessionSelector {
    Active,
    Explicit { id: String },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum UiAutomationAction {
    Query,
    List,
    Click,
    ContextClick,
    Hover,
    SetValue,
    TypeText,
    Focus,
    Backend,
}

impl UiAutomationAction {
    /// #944 - exhaustive on purpose. A new variant is a COMPILE error here, and the
    /// `None` terminator bounds the walk, so a variant cannot be silently left out of
    /// the iteration the way a hand-written `[UiAutomationAction; N]` array lets it be
    /// (name the variant in the forced match arm, forget the array, and the parity test
    /// still passes).
    #[cfg(test)]
    fn next_variant(self) -> Option<Self> {
        Some(match self {
            Self::Query => Self::List,
            Self::List => Self::Click,
            Self::Click => Self::ContextClick,
            Self::ContextClick => Self::Hover,
            Self::Hover => Self::SetValue,
            Self::SetValue => Self::TypeText,
            Self::TypeText => Self::Focus,
            Self::Focus => Self::Backend,
            Self::Backend => return None,
        })
    }

    /// Residual hole, stated so nobody trusts this further than it goes: the walk is
    /// SEEDED by hand at `Self::Query`. A variant inserted at the HEAD (before `Query`,
    /// wired `New => Query`) is compile-forced into `next_variant` and into
    /// `action_wire_name`, and yet is never yielded here, so the parity test below would
    /// not see it. It only bites a Rust-ONLY head-insertion: add the same member to the
    /// types.ts union and the set comparison goes red at once. Appending a variant (the
    /// normal case, and what #944 itself did) is fully covered. Closing the hole outright
    /// needs a derive macro (strum) or a hand-written length, and a hand-written length is
    /// the exact weakness this walk replaced.
    #[cfg(test)]
    fn all() -> impl Iterator<Item = Self> {
        std::iter::successors(Some(Self::Query), |action| action.next_variant())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct UiAutomationResponse {
    pub ok: bool,
    pub request_id: String,
    pub window: String,
    pub action: UiAutomationAction,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub selector: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_windows: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(default)]
    pub active_test_id: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filters: Option<UiListFilters>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub targets: Option<Vec<UiListTarget>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_count_exact: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub returned_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan: Option<UiListScan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_snapshot: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UiListFilters {
    pub prefix: Option<String>,
    pub role: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UiListTarget {
    pub test_id: String,
    pub role: Option<String>,
    pub state: Option<String>,
    pub visible: bool,
    pub disabled: bool,
    pub checked: Option<bool>,
    pub selected: Option<bool>,
    pub pressed: Option<bool>,
    pub expanded: Option<bool>,
    pub focused: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UiListScan {
    pub elements: usize,
    pub element_limit: usize,
    pub targets: usize,
    pub target_limit: usize,
    pub open_roots: usize,
    pub open_root_limit: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
struct PendingRequest {
    request: UiAutomationRequest,
    response_path: PathBuf,
    inflight_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalTaskPhase {
    Running,
    Joining,
}

struct TerminalTaskControl {
    cancelled: Arc<AtomicBool>,
    phase: TerminalTaskPhase,
    handle: Option<JoinHandle<()>>,
}

struct TerminalTaskStartGate {
    open: Mutex<bool>,
    wake: Condvar,
}

impl TerminalTaskStartGate {
    fn closed() -> Self {
        Self {
            open: Mutex::new(false),
            wake: Condvar::new(),
        }
    }

    fn wait(&self) {
        let mut open = self.open.lock().unwrap_or_else(|error| error.into_inner());
        while !*open {
            open = self
                .wake
                .wait(open)
                .unwrap_or_else(|error| error.into_inner());
        }
    }

    fn open(&self) {
        *self.open.lock().unwrap_or_else(|error| error.into_inner()) = true;
        self.wake.notify_one();
    }
}

#[derive(Debug, Clone)]
enum TerminalOwnerWitness {
    Main {
        owner_window: String,
        generation: u64,
        session_id: Uuid,
        selection: SessionSelection,
    },
    Detached {
        owner_window: String,
        generation: u64,
        session_id: Uuid,
    },
}

impl TerminalOwnerWitness {
    fn owner_window(&self) -> &str {
        match self {
            Self::Main { owner_window, .. } | Self::Detached { owner_window, .. } => owner_window,
        }
    }

    fn generation(&self) -> u64 {
        match self {
            Self::Main { generation, .. } | Self::Detached { generation, .. } => *generation,
        }
    }

    fn session_id(&self) -> Uuid {
        match self {
            Self::Main { session_id, .. } | Self::Detached { session_id, .. } => *session_id,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct TerminalSnapshotTaskError {
    code: &'static str,
    message: &'static str,
}

impl TerminalSnapshotTaskError {
    const fn new(code: &'static str, message: &'static str) -> Self {
        Self { code, message }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WindowInventoryEntry {
    ready: bool,
    generation: u64,
}

#[derive(Debug)]
struct WindowInventory {
    next_generation: u64,
    entries: HashMap<String, WindowInventoryEntry>,
    status: WindowInventoryStatus,
}

impl WindowInventory {
    fn initial() -> Self {
        let mut inventory = Self {
            next_generation: 1,
            entries: HashMap::new(),
            status: WindowInventoryStatus::Ready,
        };
        let generation = inventory.take_generation();
        inventory.entries.insert(
            "main".to_string(),
            WindowInventoryEntry {
                ready: false,
                generation,
            },
        );
        inventory
    }

    fn take_generation(&mut self) -> u64 {
        let generation = self.next_generation;
        self.next_generation = self.next_generation.saturating_add(1);
        generation
    }

    fn mark_ready(&mut self, label: &str) {
        let status_before = self.status;
        let generation = self.take_generation();
        self.entries
            .entry(label.to_string())
            .and_modify(|entry| {
                entry.ready = true;
                entry.generation = generation;
            })
            .or_insert(WindowInventoryEntry {
                ready: true,
                generation,
            });
        self.update_status();
        if status_before != WindowInventoryStatus::Overflow
            && self.status == WindowInventoryStatus::Overflow
        {
            self.bump_all_generations();
        }
    }

    fn sync(&mut self, live_labels: Vec<String>) -> bool {
        let live = live_labels.into_iter().collect::<HashSet<_>>();
        let before_status = self.status;
        let before_entries = self.entries.clone();
        self.entries.retain(|label, _| live.contains(label));
        for label in live {
            if !self.entries.contains_key(&label) {
                let generation = self.take_generation();
                self.entries.insert(
                    label,
                    WindowInventoryEntry {
                        ready: false,
                        generation,
                    },
                );
            }
        }
        let status_before = self.status;
        self.update_status();
        if status_before != WindowInventoryStatus::Overflow
            && self.status == WindowInventoryStatus::Overflow
        {
            self.bump_all_generations();
        }
        before_status != self.status || before_entries != self.entries
    }

    fn bump_all_generations(&mut self) {
        let labels = self.entries.keys().cloned().collect::<Vec<_>>();
        for label in labels {
            let generation = self.take_generation();
            if let Some(entry) = self.entries.get_mut(&label) {
                entry.generation = generation;
            }
        }
    }

    fn update_status(&mut self) {
        self.status = if self.entries.len() > MAX_REGISTERED_WINDOWS {
            WindowInventoryStatus::Overflow
        } else {
            WindowInventoryStatus::Ready
        };
    }

    fn snapshot(&self) -> (WindowInventoryStatus, Vec<String>, Vec<String>) {
        if self.status == WindowInventoryStatus::Overflow {
            return (self.status, Vec::new(), Vec::new());
        }
        let mut labels = self.entries.keys().cloned().collect::<Vec<_>>();
        labels.sort();
        let mut ready = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.ready)
            .map(|(label, _)| label.clone())
            .collect::<Vec<_>>();
        ready.sort();
        (self.status, labels, ready)
    }

    fn is_ready(&self, label: &str) -> bool {
        self.status == WindowInventoryStatus::Ready
            && self.entries.get(label).is_some_and(|entry| entry.ready)
    }

    fn ready_generation(&self, label: &str) -> Option<u64> {
        (self.status == WindowInventoryStatus::Ready)
            .then(|| self.entries.get(label))
            .flatten()
            .filter(|entry| entry.ready)
            .map(|entry| entry.generation)
    }

    fn is_overflow(&self) -> bool {
        self.status == WindowInventoryStatus::Overflow
    }

    fn observed_count(&self) -> u32 {
        u32::try_from(self.entries.len()).unwrap_or(u32::MAX)
    }
}

struct RequestScanCursor {
    automation_dir: RetainedAutomationDirectory,
    requests_dir: RetainedAutomationDirectory,
    entries: fs::ReadDir,
}

#[derive(Clone)]
pub struct UiAutomationState {
    inner: Arc<UiAutomationInner>,
}

struct UiAutomationInner {
    enabled: bool,
    token: String,
    instance_id: String,
    config_dir: PathBuf,
    config_witness: Option<Arc<dyn AutomationConfigWitness>>,
    automation_dir: PathBuf,
    session_path: PathBuf,
    window_inventory: Mutex<WindowInventory>,
    session_snapshot_write: Mutex<()>,
    pending: Mutex<HashMap<String, PendingRequest>>,
    request_scan_cursor: Mutex<Option<RequestScanCursor>>,
    terminal_task_permits: Arc<tokio::sync::Semaphore>,
    terminal_task_admission_closed: AtomicBool,
    terminal_tasks: Mutex<HashMap<String, TerminalTaskControl>>,
    available: AtomicBool,
    app_handle: Mutex<Option<AppHandle>>,
    exe_path: String,
    started_at_unix_ms: i64,
}

impl UiAutomationState {
    pub fn new(
        enabled: bool,
        config_dir: PathBuf,
        config_witness: Option<Arc<dyn AutomationConfigWitness>>,
    ) -> Result<Self, &'static str> {
        Self::new_with_process_probe(
            enabled,
            config_dir,
            config_witness,
            &OsLiveProcessIdentityProbe,
        )
    }

    fn new_with_process_probe(
        enabled: bool,
        config_dir: PathBuf,
        config_witness: Option<Arc<dyn AutomationConfigWitness>>,
        process_probe: &dyn LiveProcessIdentityProbe,
    ) -> Result<Self, &'static str> {
        let (config_dir, started_at_unix_ms, exe_path) = if enabled {
            let witness = config_witness
                .as_ref()
                .ok_or("automation_config_identity_unavailable")?;
            if !witness.verify_current()
                || !paths_equal_for_compare(witness.canonical_path(), &config_dir)
            {
                return Err("automation_config_identity_unavailable");
            }
            let startup_executable = canonical_for_compare(
                &std::env::current_exe().map_err(|_| "automation_session_stale")?,
            );
            let identity = process_probe
                .probe(std::process::id())
                .ok_or("automation_session_stale")?;
            if identity.started_at_unix_ms <= 0
                || !paths_equal_for_compare(&identity.executable, &startup_executable)
            {
                return Err("automation_session_stale");
            }
            (
                witness.canonical_path().to_path_buf(),
                identity.started_at_unix_ms,
                startup_executable.to_string_lossy().into_owned(),
            )
        } else {
            (config_dir, 0, current_exe_path_string())
        };
        let automation_dir = config_dir.join(UI_AUTOMATION_DIR);
        let session_path = automation_dir.join(SESSION_FILE);
        Ok(Self {
            inner: Arc::new(UiAutomationInner {
                enabled,
                token: if enabled {
                    Uuid::new_v4().to_string()
                } else {
                    String::new()
                },
                instance_id: if enabled {
                    Uuid::new_v4().to_string()
                } else {
                    String::new()
                },
                config_dir,
                config_witness,
                automation_dir,
                session_path,
                window_inventory: Mutex::new(WindowInventory::initial()),
                session_snapshot_write: Mutex::new(()),
                pending: Mutex::new(HashMap::new()),
                request_scan_cursor: Mutex::new(None),
                terminal_task_permits: Arc::new(tokio::sync::Semaphore::new(
                    MAX_PENDING_TERMINAL_SNAPSHOTS,
                )),
                terminal_task_admission_closed: AtomicBool::new(false),
                terminal_tasks: Mutex::new(HashMap::new()),
                available: AtomicBool::new(enabled),
                app_handle: Mutex::new(None),
                exe_path,
                started_at_unix_ms,
            }),
        })
    }

    pub fn enabled(&self) -> bool {
        self.inner.enabled && self.inner.available.load(Ordering::SeqCst)
    }

    pub fn start(
        &self,
        app: AppHandle,
        shutdown: ShutdownSignal,
        after_owned_artifacts_published: impl FnOnce(),
    ) {
        if self.inner.enabled {
            *self
                .inner
                .app_handle
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = Some(app.clone());
        }

        if self
            .publish_owned_artifacts(after_owned_artifacts_published)
            .is_err()
        {
            self.mark_unavailable();
            log::error!(
                target: UI_AUTOMATION_LOG_TARGET,
                "{}",
                ui_automation_log_message(
                    UiAutomationLogEvent::InitializeFailed,
                    None,
                    "automation_filesystem_error",
                )
            );
            return;
        }
        if !self.inner.enabled {
            return;
        }

        let state = self.clone();
        let shutdown = shutdown.token().clone();
        tauri::async_runtime::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(POLL_MS));
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = interval.tick() => state.poll_once(&app),
                }
            }
        });
    }

    fn publish_owned_artifacts(
        &self,
        after_owned_artifacts_published: impl FnOnce(),
    ) -> io::Result<()> {
        if self.inner.enabled {
            self.initialize_files()?;
            self.inner.available.store(true, Ordering::SeqCst);
        }
        after_owned_artifacts_published();
        Ok(())
    }

    pub fn cleanup_session_file(&self) {
        if self.inner.enabled {
            self.cleanup_owned_automation_files();
        }
    }

    pub fn close_and_join_terminal_tasks(&self) {
        {
            let tasks = self
                .inner
                .terminal_tasks
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            self.inner
                .terminal_task_admission_closed
                .store(true, Ordering::SeqCst);
            for control in tasks.values() {
                control.cancelled.store(true, Ordering::SeqCst);
            }
        }

        loop {
            let next = {
                let mut tasks = self
                    .inner
                    .terminal_tasks
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if tasks.is_empty() {
                    None
                } else {
                    Some(tasks.iter_mut().find_map(|(request_id, control)| {
                        control.handle.take().map(|handle| {
                            control.phase = TerminalTaskPhase::Joining;
                            (request_id.clone(), handle)
                        })
                    }))
                }
            };
            let Some(next) = next else {
                break;
            };
            let Some((request_id, handle)) = next else {
                std::thread::yield_now();
                continue;
            };
            let _ = handle.join();
            self.inner
                .terminal_tasks
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remove(&request_id);
        }
        self.inner
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
    }

    fn reap_finished_terminal_tasks(&self) {
        loop {
            let next = {
                let mut tasks = self
                    .inner
                    .terminal_tasks
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                tasks.iter_mut().find_map(|(request_id, control)| {
                    if control.phase == TerminalTaskPhase::Running
                        && control.handle.as_ref().is_some_and(JoinHandle::is_finished)
                    {
                        control.phase = TerminalTaskPhase::Joining;
                        control
                            .handle
                            .take()
                            .map(|handle| (request_id.clone(), handle))
                    } else {
                        None
                    }
                })
            };
            let Some((request_id, handle)) = next else {
                return;
            };
            let _ = handle.join();
            self.inner
                .terminal_tasks
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remove(&request_id);
        }
    }

    fn verify_config_ownership(&self) -> bool {
        self.inner
            .config_witness
            .as_ref()
            .is_some_and(|witness| witness.verify_current())
    }

    fn transition_config_identity_unavailable(&self) {
        if !self.inner.available.swap(false, Ordering::SeqCst) {
            return;
        }
        {
            let tasks = self
                .inner
                .terminal_tasks
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            self.inner
                .terminal_task_admission_closed
                .store(true, Ordering::SeqCst);
            for control in tasks.values() {
                control.cancelled.store(true, Ordering::SeqCst);
            }
            self.inner
                .pending
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clear();
        }
        log::error!(
            target: UI_AUTOMATION_LOG_TARGET,
            "{}",
            ui_automation_log_message(
                UiAutomationLogEvent::ConfigIdentityFailed,
                None,
                "automation_config_identity_unavailable",
            )
        );
        if let Some(app) = self
            .inner
            .app_handle
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .cloned()
        {
            app.exit(1);
        }
    }

    fn with_owned_automation_fs<T>(
        &self,
        operation: impl FnOnce() -> io::Result<T>,
    ) -> io::Result<T> {
        if !self.verify_config_ownership() {
            self.transition_config_identity_unavailable();
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "automation_config_identity_unavailable",
            ));
        }
        operation()
    }

    fn cleanup_owned_automation_files(&self) {
        // The cursor owns non-delete-share directory handles on Windows. Drop
        // them while the config witness is still live and before removing the
        // mailbox tree they enumerate.
        *self
            .inner
            .request_scan_cursor
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        if !self.verify_config_ownership() {
            self.transition_config_identity_unavailable();
            return;
        }
        let _ = self.with_owned_automation_fs(|| {
            cleanup_stale_automation_files(&self.inner.automation_dir)
        });
        let _ = self.with_owned_automation_fs(|| retry_remove_file(&self.inner.session_path));
    }

    pub fn mark_frontend_ready(
        &self,
        caller_label: &str,
        claimed_label: Option<&str>,
    ) -> Result<(), String> {
        if !self.enabled() {
            return Err("automation_not_enabled".to_string());
        }
        if let Some(claimed_label) = claimed_label {
            if claimed_label != caller_label {
                return Err("frontend_ready_window_mismatch".to_string());
            }
        }
        self.inner
            .window_inventory
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .mark_ready(caller_label);
        self.write_session_snapshot().map_err(|e| e.to_string())
    }

    pub fn complete(&self, caller_label: &str, result: UiAutomationResponse) -> Result<(), String> {
        self.complete_with_now(caller_label, result, now_unix_ms)
    }

    fn complete_with_now(
        &self,
        caller_label: &str,
        result: UiAutomationResponse,
        now: impl Fn() -> i64,
    ) -> Result<(), String> {
        self.complete_with_now_and_precommit_hook(caller_label, result, now, || {})
    }

    fn complete_with_now_and_precommit_hook(
        &self,
        caller_label: &str,
        result: UiAutomationResponse,
        now: impl Fn() -> i64,
        before_commit: impl FnOnce(),
    ) -> Result<(), String> {
        if !self.enabled() {
            return Err("automation_not_enabled".to_string());
        }

        let (pending, expired_when_removed) = {
            let mut pending = self.inner.pending.lock().unwrap_or_else(|e| e.into_inner());
            let expired = pending
                .get(&result.request_id)
                .is_some_and(|entry| request_expired(&entry.request, now()));
            (pending.remove(&result.request_id), expired)
        };
        let Some(pending) = pending else {
            return Err("unknown_request_id".to_string());
        };

        let response = if caller_label != pending.request.window
            || result.window != pending.request.window
            || result.action != pending.request.action
            || result.selector != pending.request.selector
        {
            UiAutomationResponse::error_for_request(
                &pending.request,
                "completion_mismatch",
                "Frontend completion did not match the pending automation request.",
            )
        } else {
            result
        };

        let expired_at_publish = if expired_when_removed {
            let expired = expired_response_for_request(&pending.request);
            self.with_owned_automation_fs(|| {
                write_json_atomic_new(&pending.response_path, &expired)
            })
            .map_err(|error| error.to_string())?;
            true
        } else {
            let expired_at_commit = AtomicBool::new(false);
            let committed = self
                .with_owned_automation_fs(|| {
                    write_json_atomic_new_with_precommit(&pending.response_path, &response, || {
                        before_commit();
                        let expired = request_expired(&pending.request, now());
                        expired_at_commit.store(expired, Ordering::SeqCst);
                        Ok(!expired)
                    })
                })
                .map_err(|error| error.to_string())?;
            if committed {
                false
            } else if expired_at_commit.load(Ordering::SeqCst) {
                let expired = expired_response_for_request(&pending.request);
                self.with_owned_automation_fs(|| {
                    write_json_atomic_new(&pending.response_path, &expired)
                })
                .map_err(|error| error.to_string())?;
                true
            } else {
                return Err("automation_filesystem_error".to_string());
            }
        };
        let _ = self.with_owned_automation_fs(|| retry_remove_file(&pending.inflight_path));
        if expired_at_publish {
            Err("request_expired".to_string())
        } else {
            Ok(())
        }
    }

    fn initialize_files(&self) -> io::Result<()> {
        self.with_owned_automation_fs(|| {
            prepare_automation_mailbox_tree(&self.inner.automation_dir, true)
        })?;
        self.write_session_snapshot()
    }

    fn mark_unavailable(&self) {
        self.inner.available.store(false, Ordering::SeqCst);
    }

    fn write_session_snapshot(&self) -> io::Result<()> {
        let committed = self.write_session_snapshot_with_commit_decision(|| Ok(true))?;
        if committed {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "atomic publication cancelled",
            ))
        }
    }

    fn write_session_snapshot_with_commit_decision(
        &self,
        commit_decision: impl FnOnce() -> io::Result<bool>,
    ) -> io::Result<bool> {
        // Serialize snapshot capture with publication. Taking this lock before
        // reading the inventory prevents an older captured generation from
        // resuming after a newer writer and replacing its session snapshot.
        let _write = self
            .inner
            .session_snapshot_write
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let (status, window_labels, ready_window_labels, observed_count) = {
            let inventory = self
                .inner
                .window_inventory
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let (status, labels, ready) = inventory.snapshot();
            (status, labels, ready, inventory.observed_count())
        };
        let session = UiAutomationSession {
            schema_version: PROTOCOL_SCHEMA_VERSION,
            instance_id: self.inner.instance_id.clone(),
            pid: std::process::id(),
            token: self.inner.token.clone(),
            exe_path: self.inner.exe_path.clone(),
            config_dir: self.inner.config_dir.to_string_lossy().into_owned(),
            window_inventory: UiAutomationWindowInventory {
                status,
                observed_count,
                limit: MAX_REGISTERED_WINDOWS as u32,
            },
            window_labels,
            ready_window_labels,
            started_at_unix_ms: self.inner.started_at_unix_ms,
        };
        self.with_owned_automation_fs(|| {
            write_json_atomic(&self.inner.session_path, &session, true, commit_decision)
        })
    }

    fn poll_once(&self, app: &AppHandle) {
        if self.poll_once_inner(app).is_err() {
            log::warn!(
                target: UI_AUTOMATION_LOG_TARGET,
                "{}",
                ui_automation_log_message(
                    UiAutomationLogEvent::PollFailed,
                    None,
                    "automation_filesystem_error",
                )
            );
        }
    }

    fn poll_once_inner(&self, app: &AppHandle) -> io::Result<()> {
        self.reap_finished_terminal_tasks();
        self.sync_live_window_labels(available_window_labels(app))?;
        self.expire_pending_requests();

        let entries = match self.request_scan_batch() {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };

        for request_file in entries {
            if self.is_pending(&request_file.request_id) {
                continue;
            }
            self.process_request_file(app, request_file);
        }

        Ok(())
    }

    fn request_scan_batch(&self) -> io::Result<Vec<RequestFile>> {
        self.request_scan_batch_with_cleanup(remove_invalid_request_entry)
    }

    fn request_scan_batch_with_cleanup(
        &self,
        mut cleanup_invalid: impl FnMut(&Path) -> io::Result<()>,
    ) -> io::Result<Vec<RequestFile>> {
        let requests_dir = self.requests_dir();
        let paths = self.with_owned_automation_fs(|| {
            let mut scan = self
                .inner
                .request_scan_cursor
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if scan.is_none() {
                let automation_dir =
                    retain_automation_directory_no_follow(&self.inner.automation_dir)?;
                let requests_guard = retain_automation_directory_no_follow(&requests_dir)?;
                automation_dir.verify_current()?;
                scan.replace(RequestScanCursor {
                    automation_dir,
                    requests_dir: requests_guard,
                    entries: fs::read_dir(&requests_dir)?,
                });
            }

            let (mut paths, exhausted) = {
                let cursor = scan.as_mut().expect("request scan cursor initialized");
                cursor.automation_dir.verify_current()?;
                cursor.requests_dir.verify_current()?;
                let mut paths = Vec::with_capacity(MAX_REQUEST_FILES_PER_SCAN);
                let mut exhausted = false;
                while paths.len() < MAX_REQUEST_FILES_PER_SCAN {
                    match cursor.entries.next() {
                        Some(Ok(entry)) => paths.push(entry.path()),
                        Some(Err(error)) => return Err(error),
                        None => {
                            exhausted = true;
                            break;
                        }
                    }
                }
                (paths, exhausted)
            };
            if exhausted {
                *scan = None;
            }
            paths.sort();
            Ok(paths)
        })?;
        let mut requests = Vec::with_capacity(paths.len());
        for path in paths {
            if let Some(request) = RequestFile::from_path(&path) {
                requests.push(request);
            } else {
                let cleanup = self.with_owned_automation_fs(|| cleanup_invalid(&path));
                if cleanup.is_err() && !self.enabled() {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "automation_config_identity_unavailable",
                    ));
                }
            }
        }
        Ok(requests)
    }

    fn sync_live_window_labels(&self, live_labels: Vec<String>) -> io::Result<()> {
        let (changed, overflow, observed_count) = {
            let mut inventory = self
                .inner
                .window_inventory
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let changed = inventory.sync(live_labels);
            (changed, inventory.is_overflow(), inventory.observed_count())
        };

        if changed {
            self.write_session_snapshot()?;
        }
        if overflow {
            self.cancel_pending_for_window_overflow(observed_count);
        }
        Ok(())
    }

    fn cancel_pending_for_window_overflow(&self, observed_count: u32) {
        let pending = {
            let tasks = self
                .inner
                .terminal_tasks
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            for control in tasks.values() {
                control.cancelled.store(true, Ordering::SeqCst);
            }
            self.inner
                .pending
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .drain()
                .map(|(_, pending)| pending)
                .collect::<Vec<_>>()
        };
        for pending in pending {
            let mut response = UiAutomationResponse::error_for_request(
                &pending.request,
                "registered_window_limit_exceeded",
                "The running GUI registered more automation windows than supported.",
            );
            response.diagnostics = Some(json!({
                "observedCount": observed_count,
                "limit": MAX_REGISTERED_WINDOWS,
            }));
            let _ = self.with_owned_automation_fs(|| {
                write_json_atomic_new(&pending.response_path, &response)
            });
            let _ = self.with_owned_automation_fs(|| retry_remove_file(&pending.inflight_path));
        }
    }

    fn expire_pending_requests(&self) {
        let now = now_unix_ms();
        let expired = {
            let tasks = self
                .inner
                .terminal_tasks
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let mut pending_map = self.inner.pending.lock().unwrap_or_else(|e| e.into_inner());
            let expired_ids: Vec<String> = pending_map
                .iter()
                .filter(|(_, pending)| request_expired(&pending.request, now))
                .map(|(request_id, _)| request_id.clone())
                .collect();
            for request_id in &expired_ids {
                if let Some(control) = tasks.get(request_id) {
                    control.cancelled.store(true, Ordering::SeqCst);
                }
            }
            expired_ids
                .into_iter()
                .filter_map(|request_id| pending_map.remove(&request_id))
                .collect::<Vec<_>>()
        };

        for pending in expired {
            let response = expired_response_for_request(&pending.request);
            let response_exists = self
                .with_owned_automation_fs(|| pending.response_path.try_exists())
                .unwrap_or(true);
            if !response_exists {
                let _ = self.with_owned_automation_fs(|| {
                    write_json_atomic_new(&pending.response_path, &response)
                });
            }
            let _ = self.with_owned_automation_fs(|| retry_remove_file(&pending.inflight_path));
        }
    }

    fn process_request_file(&self, app: &AppHandle, request_file: RequestFile) {
        let raw = match self.with_owned_automation_fs(|| {
            read_bounded_regular_file(&request_file.path, MAX_REQUEST_FILE_BYTES)
        }) {
            Ok(raw) => raw,
            Err(e) => {
                if e.kind() == io::ErrorKind::InvalidData
                    && e.to_string() == "request_too_large"
                {
                    let response = UiAutomationResponse::minimal_error(
                        &request_file.request_id,
                        "main",
                        UiAutomationAction::Query,
                        "",
                        "request_too_large",
                        "Automation request file exceeded its byte limit.",
                    );
                    let _ = self.write_direct_response(&request_file, &response);
                    return;
                }
                log::warn!(
                    target: UI_AUTOMATION_LOG_TARGET,
                    "{}",
                    ui_automation_log_message(
                        UiAutomationLogEvent::RequestReadFailed,
                        None,
                        "automation_filesystem_error",
                    )
                );
                return;
            }
        };
        let request: UiAutomationRequest = match serde_json::from_str(&raw) {
            Ok(request) => request,
            Err(_) => {
                let response = UiAutomationResponse::minimal_error(
                    &request_file.request_id,
                    "main",
                    UiAutomationAction::Query,
                    "",
                    "malformed_request",
                    "Automation request file was not valid protocol JSON.",
                );
                let _ = self.write_direct_response(&request_file, &response);
                return;
            }
        };

        if request.request_id != request_file.request_id {
            let response = UiAutomationResponse::error_for_request(
                &request,
                "malformed_request",
                "Request id did not match its request filename.",
            );
            let _ = self.write_direct_response(&request_file, &response);
            return;
        }

        if let Err((error, message)) = validate_request_shape_and_limits(&request) {
            let response = UiAutomationResponse::error_for_request(&request, error, message);
            let _ = self.write_direct_response(&request_file, &response);
            return;
        }

        if request_expired(&request, now_unix_ms()) {
            let response = expired_response_for_request(&request);
            let _ = self.write_direct_response(&request_file, &response);
            return;
        }

        if let Err((error, message)) = self.validate_request_claims(&request, &request_file.path) {
            let response = UiAutomationResponse::error_for_request(&request, error, message);
            let _ = self.write_direct_response(&request_file, &response);
            return;
        }

        if is_backend_automation_window(&request.window) {
            self.process_backend_request_file(app, &request_file, &request);
            return;
        }

        let available_windows = available_window_labels(app);
        if !available_windows
            .iter()
            .any(|label| label == &request.window)
        {
            let mut response = UiAutomationResponse::error_for_request(
                &request,
                "window_unavailable",
                "Requested automation window is not live.",
            );
            response.available_windows = Some(available_windows);
            let _ = self.write_direct_response(&request_file, &response);
            return;
        }

        if !self.is_window_ready(&request.window) {
            return;
        }

        let inflight_path = match self.ensure_inflight(&request_file) {
            Ok(path) => path,
            Err(_) => {
                let mut response = UiAutomationResponse::error_for_request(
                    &request,
                    "automation_filesystem_error",
                    "Failed to mark automation request as inflight.",
                );
                response.diagnostics = Some(json!({
                    "operation": "rename_request_inflight",
                }));
                let _ = self.write_direct_response(&request_file, &response);
                return;
            }
        };

        if request_expired(&request, now_unix_ms()) {
            let response = expired_response_for_request(&request);
            let _ = self.with_owned_automation_fs(|| {
                write_json_atomic_new(&self.response_path(&request.request_id), &response)
            });
            let _ = self.with_owned_automation_fs(|| retry_remove_file(&inflight_path));
            return;
        }

        let response_path = self.response_path(&request.request_id);
        {
            let mut pending_map = self.inner.pending.lock().unwrap_or_else(|e| e.into_inner());
            if pending_limit_reached_in(&pending_map, &request.window) {
                drop(pending_map);
                self.finish_claimed_request(
                    &request,
                    &inflight_path,
                    &response_path,
                    UiAutomationResponse::error_for_request(
                        &request,
                        "automation_flooded",
                        "UI automation pending request capacity is exhausted.",
                    ),
                );
                return;
            }
            pending_map.insert(
                request.request_id.clone(),
                PendingRequest {
                    request: request.clone(),
                    response_path,
                    inflight_path,
                },
            );
        }

        if app
            .emit_to(&request.window, "ui_automation_request", &request)
            .is_err()
        {
            let pending = {
                let mut pending_map = self.inner.pending.lock().unwrap_or_else(|e| e.into_inner());
                pending_map.remove(&request.request_id)
            };
            if let Some(pending) = pending {
                let mut response = UiAutomationResponse::error_for_request(
                    &request,
                    "webview_unavailable",
                    "Failed to emit automation request to the requested WebView.",
                );
                response.available_windows = Some(available_window_labels(app));
                let _ = self.with_owned_automation_fs(|| {
                    write_json_atomic_new(&pending.response_path, &response)
                });
                let _ = self.with_owned_automation_fs(|| retry_remove_file(&pending.inflight_path));
            }
        }
    }

    fn process_backend_request_file(
        &self,
        app: &AppHandle,
        request_file: &RequestFile,
        request: &UiAutomationRequest,
    ) {
        if request.selector == "terminal.snapshot" {
            self.process_terminal_snapshot_request_file(app, request_file, request);
            return;
        }
        let inflight_path = match self.ensure_inflight(request_file) {
            Ok(path) => path,
            Err(_) => {
                let response = UiAutomationResponse::error_for_request(
                    request,
                    "automation_filesystem_error",
                    "Failed to mark backend automation request as inflight.",
                );
                let _ = self.write_direct_response(request_file, &response);
                return;
            }
        };
        let response_path = self.response_path(&request.request_id);
        {
            let mut pending_map = self
                .inner
                .pending
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if pending_limit_reached_in(&pending_map, &request.window) {
                drop(pending_map);
                self.finish_claimed_request(
                    request,
                    &inflight_path,
                    &response_path,
                    UiAutomationResponse::error_for_request(
                        request,
                        "automation_flooded",
                        "UI automation pending request capacity is exhausted.",
                    ),
                );
                return;
            }
            pending_map.insert(
                request.request_id.clone(),
                PendingRequest {
                    request: request.clone(),
                    response_path,
                    inflight_path,
                },
            );
        }

        let state = self.clone();
        let app = app.clone();
        let request = request.clone();
        tauri::async_runtime::spawn(async move {
            let response = handle_backend_request(&app, &request).await;
            let pending = state
                .inner
                .pending
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remove(&request.request_id);
            let Some(pending) = pending else {
                return;
            };
            if state
                .with_owned_automation_fs(|| write_json_atomic_new(&pending.response_path, &response))
                .is_err()
            {
                log::warn!(
                    target: UI_AUTOMATION_LOG_TARGET,
                    "{}",
                    ui_automation_log_message(
                        UiAutomationLogEvent::BackendResponseWriteFailed,
                        Some(&request.request_id),
                        "automation_filesystem_error",
                    )
                );
            }
            let _ = state.with_owned_automation_fs(|| retry_remove_file(&pending.inflight_path));
        });
    }

    fn process_terminal_snapshot_request_file(
        &self,
        app: &AppHandle,
        request_file: &RequestFile,
        request: &UiAutomationRequest,
    ) {
        self.reap_finished_terminal_tasks();
        let inflight_path = match self.ensure_inflight(request_file) {
            Ok(path) => path,
            Err(_) => {
                let response = UiAutomationResponse::error_for_request(
                    request,
                    "automation_filesystem_error",
                    "Failed to mark terminal automation request as inflight.",
                );
                let _ = self.write_direct_response(request_file, &response);
                return;
            }
        };
        let response_path = self.response_path(&request.request_id);
        let owner_window = request
            .owner_window
            .as_deref()
            .unwrap_or(BACKEND_AUTOMATION_WINDOW);

        let gate = Arc::new(TerminalTaskStartGate::closed());
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut tasks = self
            .inner
            .terminal_tasks
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if self
            .inner
            .terminal_task_admission_closed
            .load(Ordering::SeqCst)
        {
            drop(tasks);
            self.finish_claimed_request(
                request,
                &inflight_path,
                &response_path,
                UiAutomationResponse::error_for_request(
                    request,
                    "terminal_snapshot_unavailable",
                    "Terminal snapshot service is unavailable.",
                ),
            );
            return;
        }
        let permit = match Arc::clone(&self.inner.terminal_task_permits).try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                drop(tasks);
                self.finish_claimed_request(
                    request,
                    &inflight_path,
                    &response_path,
                    UiAutomationResponse::error_for_request(
                        request,
                        "automation_flooded",
                        "UI automation pending request capacity is exhausted.",
                    ),
                );
                return;
            }
        };
        let mut pending = self
            .inner
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if pending_limit_reached_in(&pending, owner_window) {
            drop(pending);
            drop(tasks);
            drop(permit);
            self.finish_claimed_request(
                request,
                &inflight_path,
                &response_path,
                UiAutomationResponse::error_for_request(
                    request,
                    "automation_flooded",
                    "UI automation pending request capacity is exhausted.",
                ),
            );
            return;
        }

        let worker_gate = Arc::clone(&gate);
        let worker_cancelled = Arc::clone(&cancelled);
        let state = self.clone();
        let app = app.clone();
        let worker_request = request.clone();
        let handle = std::thread::Builder::new()
            .name(format!("ui-terminal-{}", request.request_id))
            .spawn(move || {
                let _permit = permit;
                worker_gate.wait();
                if worker_cancelled.load(Ordering::SeqCst) {
                    return;
                }
                let result = catch_unwind(AssertUnwindSafe(|| {
                    run_terminal_snapshot_task(
                        &state,
                        &app,
                        &worker_request,
                        &worker_cancelled,
                    )
                }))
                .unwrap_or_else(|_| {
                    Err(TerminalSnapshotTaskError::new(
                        "terminal_snapshot_unavailable",
                        "Terminal snapshot capture was unavailable.",
                    ))
                });
                if worker_cancelled.load(Ordering::SeqCst) {
                    return;
                }
                let response = match result {
                    Ok(snapshot) => {
                        UiAutomationResponse::terminal_success(&worker_request, snapshot)
                    }
                    Err(error) => UiAutomationResponse::error_for_request(
                        &worker_request,
                        error.code,
                        error.message,
                    ),
                };
                state.publish_terminal_task_response(
                    &worker_request.request_id,
                    &worker_cancelled,
                    response,
                );
            });
        let handle = match handle {
            Ok(handle) => handle,
            Err(_) => {
                drop(pending);
                drop(tasks);
                self.finish_claimed_request(
                    request,
                    &inflight_path,
                    &response_path,
                    UiAutomationResponse::error_for_request(
                        request,
                        "terminal_snapshot_unavailable",
                        "Terminal snapshot service is unavailable.",
                    ),
                );
                return;
            }
        };

        pending.insert(
            request.request_id.clone(),
            PendingRequest {
                request: request.clone(),
                response_path,
                inflight_path,
            },
        );
        tasks.insert(
            request.request_id.clone(),
            TerminalTaskControl {
                cancelled,
                phase: TerminalTaskPhase::Running,
                handle: Some(handle),
            },
        );
        drop(pending);
        drop(tasks);
        gate.open();
    }

    fn finish_claimed_request(
        &self,
        _request: &UiAutomationRequest,
        inflight_path: &Path,
        response_path: &Path,
        response: UiAutomationResponse,
    ) {
        let _ = self.with_owned_automation_fs(|| write_json_atomic_new(response_path, &response));
        let _ = self.with_owned_automation_fs(|| retry_remove_file(inflight_path));
    }

    fn publish_terminal_task_response(
        &self,
        request_id: &str,
        cancelled: &AtomicBool,
        response: UiAutomationResponse,
    ) {
        self.publish_terminal_task_response_with_now(request_id, cancelled, response, now_unix_ms);
    }

    fn publish_terminal_task_response_with_now(
        &self,
        request_id: &str,
        cancelled: &AtomicBool,
        response: UiAutomationResponse,
        now: impl Fn() -> i64,
    ) {
        self.publish_terminal_task_response_with_now_and_precommit_hook(
            request_id,
            cancelled,
            response,
            now,
            || {},
        );
    }

    fn publish_terminal_task_response_with_now_and_precommit_hook(
        &self,
        request_id: &str,
        cancelled: &AtomicBool,
        response: UiAutomationResponse,
        now: impl Fn() -> i64,
        before_commit: impl FnOnce(),
    ) {
        let (pending, expired_when_removed) = {
            let tasks = self
                .inner
                .terminal_tasks
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if cancelled.load(Ordering::SeqCst) || !tasks.contains_key(request_id) {
                return;
            }
            let mut pending = self
                .inner
                .pending
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let expired = pending
                .get(request_id)
                .is_some_and(|entry| request_expired(&entry.request, now()));
            if expired {
                cancelled.store(true, Ordering::SeqCst);
            }
            (pending.remove(request_id), expired)
        };
        let Some(pending) = pending else {
            return;
        };
        if cancelled.load(Ordering::SeqCst) && !expired_when_removed {
            return;
        }
        let wrote = if expired_when_removed {
            let expired = expired_response_for_request(&pending.request);
            self.with_owned_automation_fs(|| {
                write_json_atomic_new(&pending.response_path, &expired)?;
                Ok(true)
            })
        } else {
            let expired_at_commit = AtomicBool::new(false);
            let cancelled_at_commit = AtomicBool::new(false);
            let staged = self.with_owned_automation_fs(|| {
                write_json_atomic_new_with_precommit(&pending.response_path, &response, || {
                    before_commit();
                    let expired = request_expired(&pending.request, now());
                    if expired {
                        expired_at_commit.store(true, Ordering::SeqCst);
                        cancelled.store(true, Ordering::SeqCst);
                    }
                    let is_cancelled = cancelled.load(Ordering::SeqCst);
                    cancelled_at_commit.store(is_cancelled && !expired, Ordering::SeqCst);
                    Ok(!expired && !is_cancelled)
                })
            });
            match staged {
                Ok(true) => Ok(true),
                Ok(false) if expired_at_commit.load(Ordering::SeqCst) => {
                    let expired = expired_response_for_request(&pending.request);
                    self.with_owned_automation_fs(|| {
                        write_json_atomic_new(&pending.response_path, &expired)?;
                        Ok(true)
                    })
                }
                Ok(false) if cancelled_at_commit.load(Ordering::SeqCst) => Ok(false),
                other => other,
            }
        };
        if wrote.is_err() {
            log::warn!(
                target: UI_AUTOMATION_LOG_TARGET,
                "{}",
                ui_automation_log_message(
                    UiAutomationLogEvent::BackendResponseWriteFailed,
                    Some(request_id),
                    "automation_filesystem_error",
                )
            );
        }
        if matches!(wrote, Ok(true)) {
            let _ = self.with_owned_automation_fs(|| retry_remove_file(&pending.inflight_path));
        }
    }

    fn is_pending(&self, request_id: &str) -> bool {
        self.inner
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(request_id)
    }

    fn validate_request_claims(
        &self,
        request: &UiAutomationRequest,
        request_path: &Path,
    ) -> Result<(), (&'static str, &'static str)> {
        if !current_exe_is_testable() {
            return Err((
                "refusing_non_testeable_binary",
                "UI automation is only available from agentscommander_testeable.exe.",
            ));
        }
        if request.token != self.inner.token {
            return Err((
                "automation_token_mismatch",
                "Automation token did not match the running GUI session.",
            ));
        }
        if request.instance_id != self.inner.instance_id {
            return Err((
                "automation_instance_mismatch",
                "Automation instance did not match the running GUI session.",
            ));
        }
        if request.pid != std::process::id() {
            return Err((
                "automation_pid_mismatch",
                "Automation PID did not match the running GUI process.",
            ));
        }
        if request.started_at_unix_ms != self.inner.started_at_unix_ms {
            return Err((
                "automation_session_stale",
                "Automation process creation time did not match the running GUI process.",
            ));
        }
        let live = OsLiveProcessIdentityProbe
            .probe(std::process::id())
            .ok_or((
                "automation_session_stale",
                "Could not prove the running GUI process identity.",
            ))?;
        if live.started_at_unix_ms != self.inner.started_at_unix_ms {
            return Err((
                "automation_session_stale",
                "Automation process creation time no longer identifies the running GUI process.",
            ));
        }
        if !paths_equal_for_compare(&live.executable, Path::new(&self.inner.exe_path))
            || !paths_equal_for_compare(Path::new(&request.exe_path), Path::new(&self.inner.exe_path))
        {
            return Err((
                "automation_executable_mismatch",
                "Automation executable did not match the running GUI process.",
            ));
        }
        let mailbox_config = request_path
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .ok_or((
                "automation_config_mismatch",
                "Automation mailbox was outside the running configuration.",
            ))?;
        if !paths_equal_for_compare(Path::new(&request.config_dir), &self.inner.config_dir)
            || !paths_equal_for_compare(mailbox_config, &self.inner.config_dir)
        {
            return Err((
                "automation_config_mismatch",
                "Automation configuration did not match the running GUI session.",
            ));
        }
        let inventory = self
            .inner
            .window_inventory
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if inventory.status == WindowInventoryStatus::Overflow {
            return Err((
                "registered_window_limit_exceeded",
                "The running GUI registered more automation windows than supported.",
            ));
        }
        Ok(())
    }

    fn is_window_ready(&self, window: &str) -> bool {
        self.inner
            .window_inventory
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_ready(window)
    }

    fn ensure_inflight(&self, request_file: &RequestFile) -> io::Result<PathBuf> {
        match request_file.kind {
            RequestFileKind::Inflight => Ok(request_file.path.clone()),
            RequestFileKind::Ready => {
                let inflight_path = self.inflight_path(&request_file.request_id);
                self.with_owned_automation_fs(|| {
                    retry_rename(&request_file.path, &inflight_path)
                })?;
                Ok(inflight_path)
            }
        }
    }

    fn write_direct_response(
        &self,
        request_file: &RequestFile,
        response: &UiAutomationResponse,
    ) -> io::Result<()> {
        let response_path = self.response_path(&request_file.request_id);
        self.with_owned_automation_fs(|| write_json_atomic_new(&response_path, response))?;
        let _ = self.with_owned_automation_fs(|| retry_remove_file(&request_file.path));
        Ok(())
    }

    fn requests_dir(&self) -> PathBuf {
        self.inner.automation_dir.join(REQUESTS_DIR)
    }

    fn responses_dir(&self) -> PathBuf {
        self.inner.automation_dir.join(RESPONSES_DIR)
    }

    fn inflight_path(&self, request_id: &str) -> PathBuf {
        self.requests_dir()
            .join(format!("{request_id}.inflight.json"))
    }

    fn response_path(&self, request_id: &str) -> PathBuf {
        self.responses_dir().join(format!("{request_id}.json"))
    }
}

impl UiAutomationResponse {
    fn terminal_success(request: &UiAutomationRequest, terminal_snapshot: Value) -> Self {
        Self {
            ok: true,
            request_id: request.request_id.clone(),
            window: request.window.clone(),
            action: request.action,
            selector: request.selector.clone(),
            target: None,
            error: None,
            message: None,
            available: None,
            diagnostics: None,
            available_windows: None,
            timeout_ms: None,
            phase: None,
            active_test_id: Value::Null,
            filters: None,
            targets: None,
            matched_count: None,
            matched_count_exact: None,
            returned_count: None,
            limit: None,
            truncated: None,
            scan: None,
            terminal_snapshot: Some(terminal_snapshot),
        }
    }

    fn minimal_error(
        request_id: &str,
        window: &str,
        action: UiAutomationAction,
        selector: &str,
        error: &str,
        message: &str,
    ) -> Self {
        Self {
            ok: false,
            request_id: request_id.to_string(),
            window: window.to_string(),
            action,
            selector: selector.to_string(),
            target: None,
            error: Some(error.to_string()),
            message: Some(message.to_string()),
            available: None,
            diagnostics: None,
            available_windows: None,
            timeout_ms: None,
            phase: None,
            active_test_id: Value::Null,
            filters: None,
            targets: None,
            matched_count: None,
            matched_count_exact: None,
            returned_count: None,
            limit: None,
            truncated: None,
            scan: None,
            terminal_snapshot: None,
        }
    }

    fn error_for_request(request: &UiAutomationRequest, error: &str, message: &str) -> Self {
        Self::minimal_error(
            &request.request_id,
            &request.window,
            request.action,
            &request.selector,
            error,
            message,
        )
    }
}

fn pending_limit_reached_in(pending: &HashMap<String, PendingRequest>, window: &str) -> bool {
    pending.len() >= MAX_PENDING_TOTAL
        || pending
            .values()
            .filter(|entry| {
                entry
                    .request
                    .owner_window
                    .as_deref()
                    .unwrap_or(&entry.request.window)
                    == window
            })
            .count()
            >= MAX_PENDING_PER_WINDOW
}

fn is_uuid_v4(value: &str) -> bool {
    Uuid::parse_str(value)
        .ok()
        .is_some_and(|id| id.get_version() == Some(uuid::Version::Random))
}

fn validate_request_shape_and_limits(
    request: &UiAutomationRequest,
) -> Result<(), (&'static str, &'static str)> {
    if request.schema_version != PROTOCOL_SCHEMA_VERSION
        || !is_uuid_v4(&request.request_id)
        || !is_uuid_v4(&request.instance_id)
        || request.pid == 0
        || request.started_at_unix_ms <= 0
        || request.exe_path.is_empty()
        || request.config_dir.is_empty()
        || request.window.is_empty()
    {
        return Err((
            "automation_protocol_mismatch",
            "Automation request schema or identity shape was invalid.",
        ));
    }
    validate_request_action_shape(request)?;
    if request.window.as_bytes().len() > MAX_WINDOW_LABEL_BYTES {
        return Err(("window_too_large", "Automation window label exceeded its limit."));
    }
    if request
        .owner_window
        .as_ref()
        .is_some_and(|value| value.is_empty() || value.as_bytes().len() > MAX_WINDOW_LABEL_BYTES)
    {
        return Err(("window_too_large", "Automation owner window label exceeded its limit."));
    }
    if request.session.as_ref().is_some_and(|session| match session {
        UiTerminalSessionSelector::Active => false,
        UiTerminalSessionSelector::Explicit { id } => !is_uuid_v4(id),
    }) {
        return Err((
            "invalid_terminal_session",
            "Terminal session selection was invalid.",
        ));
    }
    if request.selector.as_bytes().len() > MAX_SELECTOR_BYTES {
        return Err(("selector_too_large", "Automation selector exceeded its limit."));
    }
    if request
        .prefix
        .as_ref()
        .is_some_and(|value| value.as_bytes().len() > MAX_PREFIX_BYTES)
    {
        return Err(("prefix_too_large", "Automation list prefix exceeded its limit."));
    }
    if request
        .role
        .as_ref()
        .is_some_and(|value| value.as_bytes().len() > MAX_ROLE_BYTES)
    {
        return Err(("role_too_large", "Automation role exceeded its limit."));
    }
    if request.role.as_ref().is_some_and(|role| {
        !SUPPORTED_ROLES.iter().any(|supported| supported == &role.as_str())
    }) {
        return Err(("invalid_role", "Automation role was not supported."));
    }
    if request
        .value
        .as_ref()
        .is_some_and(|value| value.as_bytes().len() > MAX_VALUE_BYTES)
    {
        return Err(("value_too_large", "Automation value exceeded its limit."));
    }
    Ok(())
}

fn validate_request_action_shape(
    request: &UiAutomationRequest,
) -> Result<(), (&'static str, &'static str)> {
    let malformed = || {
        (
            "malformed_request",
            "Automation request fields did not match the selected action.",
        )
    };
    match request.action {
        UiAutomationAction::List => {
            if !request.selector.is_empty()
                || request.value.is_some()
                || request.owner_window.is_some()
                || request.session.is_some()
            {
                return Err(malformed());
            }
        }
        UiAutomationAction::Backend => {
            if request.selector.is_empty() || request.prefix.is_some() || request.role.is_some() {
                return Err(malformed());
            }
            match request.selector.as_str() {
                "terminal.snapshot" => {
                    if request.value.is_some()
                        || request.owner_window.is_none()
                        || request.session.is_none()
                    {
                        return Err(malformed());
                    }
                }
                RESOURCE_WATCHDOG_BACKEND_SELECTOR => {
                    if request.owner_window.is_some() || request.session.is_some() {
                        return Err(malformed());
                    }
                }
                _ => {
                    if request.owner_window.is_some() || request.session.is_some() {
                        return Err(malformed());
                    }
                }
            }
        }
        UiAutomationAction::SetValue | UiAutomationAction::TypeText => {
            if request.selector.is_empty()
                || request.value.is_none()
                || request.prefix.is_some()
                || request.role.is_some()
                || request.owner_window.is_some()
                || request.session.is_some()
            {
                return Err(malformed());
            }
        }
        UiAutomationAction::Hover => {
            if request.prefix.is_some()
                || request.role.is_some()
                || request.owner_window.is_some()
                || request.session.is_some()
                || (request.selector.is_empty() && request.value.as_deref() != Some("leave"))
            {
                return Err(malformed());
            }
        }
        UiAutomationAction::Query
        | UiAutomationAction::Click
        | UiAutomationAction::ContextClick
        | UiAutomationAction::Focus => {
            if request.selector.is_empty()
                || request.value.is_some()
                || request.prefix.is_some()
                || request.role.is_some()
                || request.owner_window.is_some()
                || request.session.is_some()
            {
                return Err(malformed());
            }
        }
    }
    Ok(())
}

#[derive(Debug)]
struct CliRequest {
    window: String,
    selector: String,
    prefix: Option<String>,
    role: Option<String>,
    owner_window: Option<String>,
    session: Option<UiTerminalSessionSelector>,
    action: UiAutomationAction,
    value: Option<String>,
    timeout_ms: u64,
}

pub fn execute_query(context: &UiCliDispatchContext, args: UiQueryArgs) -> i32 {
    execute_cli(context, CliRequest {
        window: args.window,
        selector: args.selector,
        prefix: None,
        role: None,
        owner_window: None,
        session: None,
        action: UiAutomationAction::Query,
        value: None,
        timeout_ms: args.timeout_ms,
    })
}

pub fn execute_click(context: &UiCliDispatchContext, args: UiClickArgs) -> i32 {
    execute_cli(context, CliRequest {
        window: args.window,
        selector: args.selector,
        prefix: None,
        role: None,
        owner_window: None,
        session: None,
        action: UiAutomationAction::Click,
        value: None,
        timeout_ms: args.timeout_ms,
    })
}

pub fn execute_context_click(context: &UiCliDispatchContext, args: UiContextClickArgs) -> i32 {
    execute_cli(context, CliRequest {
        window: args.window,
        selector: args.selector,
        prefix: None,
        role: None,
        owner_window: None,
        session: None,
        action: UiAutomationAction::ContextClick,
        value: None,
        timeout_ms: args.timeout_ms,
    })
}

pub fn execute_hover(context: &UiCliDispatchContext, args: UiHoverArgs) -> i32 {
    execute_cli(context, CliRequest {
        window: args.window,
        // Empty for the target-free leave form. The bridge intercepts `value == "leave"`
        // before it resolves any selector, and the frontend echoes the request's selector
        // back, so the window/action/selector equality check in `complete()` still matches.
        // No line number on purpose: that pointer read :317-321, then :361-364, and rotted
        // both times inside the very commit that wrote it (this file grew above it each
        // time). `fn complete` is the stable anchor; a number here is a comment that lies
        // on a schedule.
        selector: args.selector.unwrap_or_default(),
        prefix: None,
        role: None,
        owner_window: None,
        session: None,
        action: UiAutomationAction::Hover,
        value: args.leave.then(|| "leave".to_string()),
        timeout_ms: args.timeout_ms,
    })
}

pub fn execute_set(context: &UiCliDispatchContext, args: UiSetArgs) -> i32 {
    execute_cli(context, CliRequest {
        window: args.window,
        selector: args.selector,
        prefix: None,
        role: None,
        owner_window: None,
        session: None,
        action: UiAutomationAction::SetValue,
        value: Some(args.value),
        timeout_ms: args.timeout_ms,
    })
}

pub fn execute_type(context: &UiCliDispatchContext, args: UiTypeArgs) -> i32 {
    execute_cli(context, CliRequest {
        window: args.window,
        selector: args.selector,
        prefix: None,
        role: None,
        owner_window: None,
        session: None,
        action: UiAutomationAction::TypeText,
        value: Some(args.value),
        timeout_ms: args.timeout_ms,
    })
}

pub fn execute_backend(context: &UiCliDispatchContext, args: UiBackendArgs) -> i32 {
    let UiBackendArgs {
        selector,
        window,
        session,
        value,
        timeout_ms,
    } = args;
    let malformed = || {
        preflight_error(
            "malformed_request",
            "Automation request fields did not match the selected action.",
            None,
        )
    };
    let (owner_window, session, value) = match selector.as_str() {
        "terminal.snapshot" => {
            let Some(owner_window) = window else {
                print_stdout_value(&malformed());
                return 1;
            };
            if value.is_some() {
                print_stdout_value(&malformed());
                return 1;
            }
            let session = match session.as_deref() {
                None | Some("active") => UiTerminalSessionSelector::Active,
                Some(value) => match Uuid::parse_str(value) {
                    Ok(id) if id.get_version() == Some(uuid::Version::Random) => {
                        UiTerminalSessionSelector::Explicit { id: id.to_string() }
                    }
                    _ => {
                        print_stdout_value(&preflight_error(
                            "invalid_terminal_session",
                            "Terminal session selection was invalid.",
                            None,
                        ));
                        return 1;
                    }
                },
            };
            (Some(owner_window), Some(session), None)
        }
        RESOURCE_WATCHDOG_BACKEND_SELECTOR => {
            if window.is_some() || session.is_some() {
                print_stdout_value(&malformed());
                return 1;
            }
            (None, None, value)
        }
        _ => (None, None, value),
    };
    execute_cli(context, CliRequest {
        window: BACKEND_AUTOMATION_WINDOW.to_string(),
        selector,
        prefix: None,
        role: None,
        owner_window,
        session,
        action: UiAutomationAction::Backend,
        value,
        timeout_ms,
    })
}

pub fn execute_wait(context: &UiCliDispatchContext, args: UiWaitArgs) -> i32 {
    let predicates = match normalize_wait_predicates(&args) {
        Ok(predicates) => predicates,
        Err(error) => {
            print_stdout_value(&error);
            return 1;
        }
    };
    let predicate_kinds = predicates
        .iter()
        .map(NormalizedWaitPredicate::kind)
        .collect::<Vec<_>>();
    let legacy = predicates.is_empty();
    let absence = matches!(predicates.as_slice(), [NormalizedWaitPredicate::Absent]);
    if let Err(error) = ensure_current_exe_is_testable()
        .and_then(|_| load_session_for_cli(context, &args.window).map(|_| ()))
    {
        print_stdout_value(&error);
        return 1;
    }
    let outer_request_id = Uuid::new_v4().to_string();
    let started = Instant::now();
    let deadline = Instant::now() + Duration::from_millis(args.timeout_ms);
    let mut attempts = 0_u32;
    let mut last_target = Value::Null;
    let mut last_observation: Option<&'static str> = None;

    loop {
        let remaining_ms = deadline
            .checked_duration_since(Instant::now())
            .map(|d| d.as_millis().min(u128::from(u64::MAX)) as u64)
            .unwrap_or(0);
        if remaining_ms == 0 {
            let timeout = UiWaitOutput {
                ok: false,
                request_id: outer_request_id,
                window: args.window,
                action: "query",
                selector: args.selector,
                target: last_target,
                error: Some("timeout".to_string()),
                message: Some(
                    if legacy {
                        "Automation wait timed out before the selector became available."
                    } else {
                        "Automation wait timed out before the requested condition was met."
                    }
                    .to_string(),
                ),
                available: None,
                diagnostics: None,
                available_windows: None,
                timeout_ms: Some(args.timeout_ms),
                phase: Some("wait_condition_not_met".to_string()),
                kind: "ui-wait",
                command: "ui-wait",
                predicates: predicate_kinds,
                attempts,
                elapsed_ms: args.timeout_ms,
                last_observation,
            };
            print_stdout_json(&timeout);
            return 1;
        }

        let input = CliRequest {
            window: args.window.clone(),
            selector: args.selector.clone(),
            prefix: None,
            role: None,
            owner_window: None,
            session: None,
            action: UiAutomationAction::Query,
            value: None,
            timeout_ms: remaining_ms.min(500),
        };

        attempts = attempts.saturating_add(1);
        match run_cli_request(context, &input) {
            Ok(response) => {
                let observation = wait_observation(&response);
                let decision = if legacy {
                    if response.ok {
                        WaitDecision::Success(response.target.clone().unwrap_or(Value::Null))
                    } else {
                        WaitDecision::Retry(observation)
                    }
                } else if absence {
                    match (response.ok, response.error.as_deref()) {
                        (false, Some("missing_selector")) => WaitDecision::Success(Value::Null),
                        (true, _) => WaitDecision::Retry("target_present"),
                        (
                            false,
                            Some("target_hidden" | "duplicate_selector" | "request_expired"),
                        ) => WaitDecision::Retry(observation),
                        _ => WaitDecision::Fail,
                    }
                } else if response.ok {
                    let target = response.target.clone().unwrap_or(Value::Null);
                    if wait_predicates_match(&predicates, &target) {
                        WaitDecision::Success(target)
                    } else {
                        WaitDecision::Retry("predicate_mismatch")
                    }
                } else if matches!(
                    response.error.as_deref(),
                    Some(
                        "missing_selector"
                            | "target_hidden"
                            | "duplicate_selector"
                            | "request_expired"
                    )
                ) {
                    WaitDecision::Retry(observation)
                } else {
                    WaitDecision::Fail
                };

                match decision {
                    WaitDecision::Success(target) => {
                        let output = UiWaitOutput {
                            ok: true,
                            request_id: outer_request_id,
                            window: args.window,
                            action: "query",
                            selector: args.selector,
                            target,
                            error: None,
                            message: None,
                            available: None,
                            diagnostics: None,
                            available_windows: None,
                            timeout_ms: None,
                            phase: None,
                            kind: "ui-wait",
                            command: "ui-wait",
                            predicates: predicate_kinds,
                            attempts,
                            elapsed_ms: elapsed_ms(started),
                            last_observation: None,
                        };
                        print_stdout_json(&output);
                        return 0;
                    }
                    WaitDecision::Retry(observation) => {
                        last_target = response.target.unwrap_or(Value::Null);
                        last_observation = Some(observation);
                    }
                    WaitDecision::Fail => {
                        let output = UiWaitOutput {
                            ok: false,
                            request_id: outer_request_id,
                            window: args.window,
                            action: "query",
                            selector: args.selector,
                            target: response.target.unwrap_or(Value::Null),
                            error: response.error,
                            message: response.message,
                            available: response.available,
                            diagnostics: response.diagnostics,
                            available_windows: response.available_windows,
                            timeout_ms: response.timeout_ms,
                            phase: response.phase,
                            kind: "ui-wait",
                            command: "ui-wait",
                            predicates: predicate_kinds,
                            attempts,
                            elapsed_ms: elapsed_ms(started),
                            last_observation: Some(observation),
                        };
                        print_stdout_json(&output);
                        return 1;
                    }
                }
            }
            Err(error) => {
                print_stdout_value(&error);
                return 1;
            }
        }

        std::thread::sleep(Duration::from_millis(POLL_MS));
    }
}

#[derive(Debug, Clone)]
enum NormalizedWaitPredicate {
    State(String),
    Text(String),
    Enabled,
    Disabled,
    Selected(bool),
    Expanded(bool),
    Focused(bool),
    Absent,
}

impl NormalizedWaitPredicate {
    fn kind(&self) -> UiWaitPredicateKind {
        match self {
            Self::State(_) => UiWaitPredicateKind::State,
            Self::Text(_) => UiWaitPredicateKind::Text,
            Self::Enabled => UiWaitPredicateKind::Enabled,
            Self::Disabled => UiWaitPredicateKind::Disabled,
            Self::Selected(_) => UiWaitPredicateKind::Selected,
            Self::Expanded(_) => UiWaitPredicateKind::Expanded,
            Self::Focused(_) => UiWaitPredicateKind::Focused,
            Self::Absent => UiWaitPredicateKind::Absent,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum UiWaitPredicateKind {
    State,
    Text,
    Enabled,
    Disabled,
    Selected,
    Expanded,
    Focused,
    Absent,
}

impl UiWaitPredicateKind {
    #[cfg(test)]
    fn next_variant(self) -> Option<Self> {
        Some(match self {
            Self::State => Self::Text,
            Self::Text => Self::Enabled,
            Self::Enabled => Self::Disabled,
            Self::Disabled => Self::Selected,
            Self::Selected => Self::Expanded,
            Self::Expanded => Self::Focused,
            Self::Focused => Self::Absent,
            Self::Absent => return None,
        })
    }

    #[cfg(test)]
    fn all() -> impl Iterator<Item = Self> {
        std::iter::successors(Some(Self::State), |kind| kind.next_variant())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UiWaitOutput {
    ok: bool,
    request_id: String,
    window: String,
    action: &'static str,
    selector: String,
    target: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    available: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    diagnostics: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    available_windows: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timeout_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    phase: Option<String>,
    kind: &'static str,
    command: &'static str,
    predicates: Vec<UiWaitPredicateKind>,
    attempts: u32,
    elapsed_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_observation: Option<&'static str>,
}

enum WaitDecision {
    Success(Value),
    Retry(&'static str),
    Fail,
}

fn normalize_wait_predicates(args: &UiWaitArgs) -> Result<Vec<NormalizedWaitPredicate>, Value> {
    let invalid = || {
        preflight_error(
            "invalid_predicate_combination",
            "UI wait predicates were repeated, invalid, or mutually incompatible.",
            None,
        )
    };
    if args.timeout_ms == 0
        || args.timeout_ms > MAX_TIMEOUT_MS
        || args.selector.is_empty()
        || args.state.len() > 1
        || args.text.len() > 1
        || args.selected.len() > 1
        || args.expanded.len() > 1
        || args.focused.len() > 1
        || (args.enabled && args.disabled)
    {
        return Err(invalid());
    }
    if args.selector.as_bytes().len() > MAX_SELECTOR_BYTES {
        return Err(preflight_error(
            "selector_too_large",
            "Automation selector exceeded its limit.",
            None,
        ));
    }
    if args.state.first().is_some_and(|state| state.as_bytes().len() > MAX_STATE_PREDICATE_BYTES)
        || args.text.first().is_some_and(|text| {
            text.as_bytes().len() > MAX_TEXT_PREDICATE_BYTES
                || text.chars().count() > MAX_TEXT_PREDICATE_CHARS
        })
    {
        return Err(preflight_error(
            "predicate_too_large",
            "Automation wait predicate exceeded its limit.",
            None,
        ));
    }
    if args.state.first().is_some_and(|state| !safe_state_token(state)) {
        return Err(invalid());
    }
    let parse_bool = |values: &[String]| -> Result<Option<bool>, Value> {
        match values.first().map(String::as_str) {
            None => Ok(None),
            Some("true") => Ok(Some(true)),
            Some("false") => Ok(Some(false)),
            Some(_) => Err(invalid()),
        }
    };
    let selected = parse_bool(&args.selected)?;
    let expanded = parse_bool(&args.expanded)?;
    let focused = parse_bool(&args.focused)?;
    let has_target_predicate = !args.state.is_empty()
        || !args.text.is_empty()
        || args.enabled
        || args.disabled
        || selected.is_some()
        || expanded.is_some()
        || focused.is_some();
    if args.absent && has_target_predicate {
        return Err(invalid());
    }

    let mut predicates = Vec::new();
    if let Some(state) = args.state.first() {
        predicates.push(NormalizedWaitPredicate::State(state.clone()));
    }
    if let Some(text) = args.text.first() {
        predicates.push(NormalizedWaitPredicate::Text(text.clone()));
    }
    if args.enabled {
        predicates.push(NormalizedWaitPredicate::Enabled);
    }
    if args.disabled {
        predicates.push(NormalizedWaitPredicate::Disabled);
    }
    if let Some(selected) = selected {
        predicates.push(NormalizedWaitPredicate::Selected(selected));
    }
    if let Some(expanded) = expanded {
        predicates.push(NormalizedWaitPredicate::Expanded(expanded));
    }
    if let Some(focused) = focused {
        predicates.push(NormalizedWaitPredicate::Focused(focused));
    }
    if args.absent {
        predicates.push(NormalizedWaitPredicate::Absent);
    }
    Ok(predicates)
}

fn wait_predicates_match(predicates: &[NormalizedWaitPredicate], target: &Value) -> bool {
    let Some(target) = target.as_object() else {
        return false;
    };
    predicates.iter().all(|predicate| match predicate {
        NormalizedWaitPredicate::State(expected) => {
            target.get("state").and_then(Value::as_str) == Some(expected.as_str())
        }
        NormalizedWaitPredicate::Text(expected) => {
            target.get("text").and_then(Value::as_str) == Some(expected.as_str())
        }
        NormalizedWaitPredicate::Enabled => {
            target.get("disabled").and_then(Value::as_bool) == Some(false)
        }
        NormalizedWaitPredicate::Disabled => {
            target.get("disabled").and_then(Value::as_bool) == Some(true)
        }
        NormalizedWaitPredicate::Selected(expected) => {
            target.get("selected").and_then(Value::as_bool) == Some(*expected)
        }
        NormalizedWaitPredicate::Expanded(expected) => {
            target.get("expanded").and_then(Value::as_bool) == Some(*expected)
        }
        NormalizedWaitPredicate::Focused(expected) => {
            target.get("focused").and_then(Value::as_bool) == Some(*expected)
        }
        NormalizedWaitPredicate::Absent => false,
    })
}

fn wait_observation(response: &UiAutomationResponse) -> &'static str {
    if response.ok {
        return "target_present";
    }
    match response.error.as_deref() {
        Some("missing_selector") => "missing_selector",
        Some("target_hidden") => "target_hidden",
        Some("duplicate_selector") => "duplicate_selector",
        Some("request_expired") => "request_expired",
        Some("target_obscured") => "target_obscured",
        Some("target_disabled") => "target_disabled",
        Some("timeout") => "attempt_timeout",
        _ => "bridge_error",
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

fn execute_cli(context: &UiCliDispatchContext, input: CliRequest) -> i32 {
    match run_cli_request(context, &input) {
        Ok(response) if response.ok => {
            print_stdout_json(&response);
            0
        }
        Ok(response) => {
            print_stdout_json(&response);
            1
        }
        Err(error) => {
            print_stdout_value(&error);
            1
        }
    }
}

pub fn execute_capabilities(
    context: &UiCliDispatchContext,
    args: UiCapabilitiesArgs,
) -> i32 {
    if let Err(error) = ensure_current_exe_is_testable() {
        print_stdout_value(&error);
        return 1;
    }
    if args.timeout_ms == 0 || args.timeout_ms > MAX_TIMEOUT_MS {
        print_stdout_value(&preflight_error(
            "invalid_timeout",
            "Automation timeout must be between 1 and 60000 milliseconds.",
            None,
        ));
        return 1;
    }
    match load_session_for_cli(context, BACKEND_AUTOMATION_WINDOW) {
        Ok(session) => {
            let ready = session
                .ready_window_labels
                .iter()
                .cloned()
                .collect::<HashSet<_>>();
            let windows = session
                .window_labels
                .iter()
                .map(|label| UiCapabilityWindow {
                    label: label.clone(),
                    ready: ready.contains(label),
                })
                .collect();
            print_stdout_json(&UiCapabilitiesOutput {
                ok: true,
                kind: "ui-capabilities",
                schema_version: PROTOCOL_SCHEMA_VERSION,
                pid: session.pid,
                actions: &CAPABILITY_ACTIONS,
                wait_predicates: &CAPABILITY_WAIT_PREDICATES,
                roles: &SUPPORTED_ROLES,
                backend_selectors: &CAPABILITY_BACKEND_SELECTORS,
                windows,
                limits: UiCapabilityLimits::contract(),
            });
            0
        }
        Err(error) => {
            print_stdout_value(&error);
            1
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UiCapabilityWindow {
    label: String,
    ready: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UiCapabilitiesOutput {
    ok: bool,
    kind: &'static str,
    schema_version: u32,
    pid: u32,
    actions: &'static [&'static str],
    wait_predicates: &'static [&'static str],
    roles: &'static [&'static str],
    backend_selectors: &'static [&'static str],
    windows: Vec<UiCapabilityWindow>,
    limits: UiCapabilityLimits,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UiCapabilityLimits {
    default_timeout_ms: u64,
    max_timeout_ms: u64,
    poll_interval_ms: u64,
    window_label_bytes: usize,
    selector_bytes: usize,
    prefix_bytes: usize,
    role_bytes: usize,
    state_predicate_bytes: usize,
    text_predicate_chars: usize,
    text_predicate_bytes: usize,
    value_bytes: usize,
    session_file_bytes: usize,
    request_file_bytes: usize,
    response_json_bytes: usize,
    stdout_json_bytes: usize,
    registered_windows: usize,
    pending_total: usize,
    pending_per_window: usize,
    pending_terminal_snapshots: usize,
    request_files_per_scan: usize,
    diagnostic_targets: usize,
    target_text_chars: usize,
    list_return_targets: usize,
    list_scan_targets: usize,
    list_scan_elements: usize,
    list_open_roots: usize,
    terminal_rows: usize,
    terminal_columns: usize,
    terminal_cells: usize,
}

impl UiCapabilityLimits {
    fn contract() -> Self {
        Self {
            default_timeout_ms: DEFAULT_TIMEOUT_MS,
            max_timeout_ms: MAX_TIMEOUT_MS,
            poll_interval_ms: POLL_MS,
            window_label_bytes: MAX_WINDOW_LABEL_BYTES,
            selector_bytes: MAX_SELECTOR_BYTES,
            prefix_bytes: MAX_PREFIX_BYTES,
            role_bytes: MAX_ROLE_BYTES,
            state_predicate_bytes: MAX_STATE_PREDICATE_BYTES,
            text_predicate_chars: MAX_TEXT_PREDICATE_CHARS,
            text_predicate_bytes: MAX_TEXT_PREDICATE_BYTES,
            value_bytes: MAX_VALUE_BYTES,
            session_file_bytes: MAX_SESSION_FILE_BYTES,
            request_file_bytes: MAX_REQUEST_FILE_BYTES,
            response_json_bytes: MAX_RESPONSE_JSON_BYTES,
            stdout_json_bytes: MAX_STDOUT_JSON_BYTES,
            registered_windows: MAX_REGISTERED_WINDOWS,
            pending_total: MAX_PENDING_TOTAL,
            pending_per_window: MAX_PENDING_PER_WINDOW,
            pending_terminal_snapshots: MAX_PENDING_TERMINAL_SNAPSHOTS,
            request_files_per_scan: MAX_REQUEST_FILES_PER_SCAN,
            diagnostic_targets: CLI_MAX_AVAILABLE_TARGETS,
            target_text_chars: CLI_MAX_TARGET_TEXT_CHARS,
            list_return_targets: MAX_LIST_RETURN_TARGETS,
            list_scan_targets: MAX_LIST_SCAN_TARGETS,
            list_scan_elements: MAX_LIST_SCAN_ELEMENTS,
            list_open_roots: MAX_LIST_OPEN_ROOTS,
            terminal_rows: MAX_TERMINAL_ROWS,
            terminal_columns: MAX_TERMINAL_COLUMNS,
            terminal_cells: MAX_TERMINAL_CELLS,
        }
    }
}

pub fn execute_list(context: &UiCliDispatchContext, args: UiListArgs) -> i32 {
    let expected_prefix = args.prefix.clone();
    let expected_role = args.role.clone();
    let input = CliRequest {
        window: args.window,
        selector: String::new(),
        prefix: args.prefix,
        role: args.role,
        owner_window: None,
        session: None,
        action: UiAutomationAction::List,
        value: None,
        timeout_ms: args.timeout_ms,
    };
    match run_cli_request(context, &input) {
        Ok(response) if response.ok => match ui_list_output(
            &response,
            expected_prefix.as_deref(),
            expected_role.as_deref(),
        ) {
            Ok(output) => {
                print_stdout_json(&output);
                0
            }
            Err(error) => {
                print_stdout_json(&error);
                1
            }
        },
        Ok(response) => {
            print_stdout_json(&response);
            1
        }
        Err(error) => {
            print_stdout_value(&error);
            1
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UiListOutput {
    ok: bool,
    request_id: String,
    window: String,
    action: &'static str,
    filters: UiListFilters,
    targets: Vec<UiListTarget>,
    matched_count: usize,
    matched_count_exact: bool,
    returned_count: usize,
    limit: usize,
    truncated: bool,
    scan: UiListScan,
    active_test_id: Option<String>,
}

fn safe_public_test_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX_SELECTOR_BYTES
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..].iter().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(*byte, b'.' | b'_' | b':' | b'-')
        })
}

fn safe_state_token(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX_STATE_PREDICATE_BYTES
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..].iter().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(*byte, b'.' | b'_' | b':' | b'-')
        })
}

fn ui_list_output(
    response: &UiAutomationResponse,
    expected_prefix: Option<&str>,
    expected_role: Option<&str>,
) -> Result<UiListOutput, UiAutomationResponse> {
    let invalid = || {
        UiAutomationResponse::minimal_error(
            &response.request_id,
            &response.window,
            UiAutomationAction::List,
            "",
            "automation_protocol_mismatch",
            "Automation list response did not match the bounded public projection contract.",
        )
    };
    let filters = response.filters.clone().ok_or_else(&invalid)?;
    let targets = response.targets.clone().ok_or_else(&invalid)?;
    let matched_count = response.matched_count.ok_or_else(&invalid)?;
    let matched_count_exact = response.matched_count_exact.ok_or_else(&invalid)?;
    let returned_count = response.returned_count.ok_or_else(&invalid)?;
    let limit = response.limit.ok_or_else(&invalid)?;
    let truncated = response.truncated.ok_or_else(&invalid)?;
    let scan = response.scan.clone().ok_or_else(&invalid)?;
    let active_test_id = match &response.active_test_id {
        Value::Null => None,
        Value::String(value) if safe_public_test_id(value) => Some(value.clone()),
        _ => return Err(invalid()),
    };
    if filters.prefix.as_deref() != expected_prefix
        || filters.role.as_deref() != expected_role
        || filters.role.as_ref().is_some_and(|role| {
            !SUPPORTED_ROLES.iter().any(|supported| supported == &role.as_str())
        })
        || limit != MAX_LIST_RETURN_TARGETS
        || returned_count != targets.len()
        || returned_count > limit
        || matched_count < returned_count
        || matched_count_exact != !scan.truncated
        || truncated != (scan.truncated || matched_count > limit)
        || scan.element_limit != MAX_LIST_SCAN_ELEMENTS
        || scan.target_limit != MAX_LIST_SCAN_TARGETS
        || scan.open_root_limit != MAX_LIST_OPEN_ROOTS
        || scan.elements > scan.element_limit
        || scan.targets > scan.target_limit
        || scan.open_roots > scan.open_root_limit
    {
        return Err(invalid());
    }
    if targets.iter().any(|target| {
        !safe_public_test_id(&target.test_id)
            || target.role.as_ref().is_some_and(|role| {
                !SUPPORTED_ROLES.iter().any(|supported| supported == &role.as_str())
            })
            || target
                .state
                .as_ref()
                .is_some_and(|state| !safe_state_token(state))
    }) {
        return Err(invalid());
    }
    if targets.windows(2).any(|pair| {
        let left = (&pair[0].test_id, pair[0].role.as_deref());
        let right = (&pair[1].test_id, pair[1].role.as_deref());
        left > right
    }) {
        return Err(invalid());
    }
    Ok(UiListOutput {
        ok: true,
        request_id: response.request_id.clone(),
        window: response.window.clone(),
        action: "list",
        filters,
        targets,
        matched_count,
        matched_count_exact,
        returned_count,
        limit,
        truncated,
        scan,
        active_test_id,
    })
}

pub fn execute_focus(context: &UiCliDispatchContext, args: UiFocusArgs) -> i32 {
    execute_cli(context, CliRequest {
        window: args.window,
        selector: args.selector,
        prefix: None,
        role: None,
        owner_window: None,
        session: None,
        action: UiAutomationAction::Focus,
        value: None,
        timeout_ms: args.timeout_ms,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackendWatchdogMode {
    Sample,
    Warn,
    KillGroup,
    Tick,
    /// #1151 - runs the production quarantine-retry gate and loop, so native verification
    /// of the orphan path is a deterministic, assertable artifact rather than log scraping
    /// behind a 15 second backoff.
    QuarantineRetry,
}

impl BackendWatchdogMode {
    fn parse(value: Option<&str>) -> Result<Self, String> {
        match value.unwrap_or("sample") {
            "sample" => Ok(Self::Sample),
            "warn" => Ok(Self::Warn),
            "killGroup" | "kill-group" => Ok(Self::KillGroup),
            "tick" => Ok(Self::Tick),
            "quarantineRetry" | "quarantine-retry" => Ok(Self::QuarantineRetry),
            other => Err(format!(
                "Unsupported resource monitor watchdog mode '{other}'. Expected sample, warn, killGroup, tick, or quarantineRetry."
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Sample => "sample",
            Self::Warn => "warn",
            Self::KillGroup => "killGroup",
            Self::Tick => "tick",
            Self::QuarantineRetry => "quarantineRetry",
        }
    }
}

fn terminal_unavailable() -> TerminalSnapshotTaskError {
    TerminalSnapshotTaskError::new(
        "terminal_snapshot_unavailable",
        "Terminal snapshot capture was unavailable.",
    )
}

fn terminal_stale() -> TerminalSnapshotTaskError {
    TerminalSnapshotTaskError::new(
        "terminal_session_stale",
        "Terminal session ownership changed during capture.",
    )
}

fn terminal_missing() -> TerminalSnapshotTaskError {
    TerminalSnapshotTaskError::new(
        "terminal_session_missing",
        "Terminal session was not found.",
    )
}

fn terminal_owner_mismatch() -> TerminalSnapshotTaskError {
    TerminalSnapshotTaskError::new(
        "terminal_session_owner_mismatch",
        "Terminal session did not belong to the requested owner window.",
    )
}

fn terminal_owner_unsupported() -> TerminalSnapshotTaskError {
    TerminalSnapshotTaskError::new(
        "terminal_owner_window_unsupported",
        "Terminal owner window was not supported.",
    )
}

fn terminal_too_large() -> TerminalSnapshotTaskError {
    TerminalSnapshotTaskError::new(
        "terminal_snapshot_too_large",
        "Terminal snapshot exceeded its size limit.",
    )
}

fn ready_window_generation(state: &UiAutomationState, label: &str) -> Option<u64> {
    state
        .inner
        .window_inventory
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .ready_generation(label)
}

fn detached_session_from_owner_label(label: &str) -> Option<Uuid> {
    let simple = label.strip_prefix("terminal-")?;
    if simple.len() != 32
        || !simple
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    Uuid::parse_str(simple).ok()
}

fn selection_snapshot_blocking(
    selection_coordinator: &SelectionCoordinator,
) -> Result<SessionSelection, TerminalSnapshotTaskError> {
    tauri::async_runtime::block_on(selection_coordinator.snapshot()).map_err(|_| terminal_unavailable())
}

fn resolve_terminal_owner_witness_blocking(
    state: &UiAutomationState,
    request: &UiAutomationRequest,
    selection_coordinator: &SelectionCoordinator,
    detached_sessions: &DetachedSessionsState,
) -> Result<TerminalOwnerWitness, TerminalSnapshotTaskError> {
    let owner_window = request
        .owner_window
        .as_deref()
        .ok_or_else(terminal_owner_unsupported)?;
    let generation = ready_window_generation(state, owner_window).ok_or_else(terminal_stale)?;
    let session_selector = request.session.as_ref().ok_or_else(terminal_owner_mismatch)?;

    if owner_window == "main" {
        let selection = selection_snapshot_blocking(selection_coordinator)?;
        let selected_id = selection.id().ok_or_else(terminal_missing)?;
        if !selection.has_pty() || !selection.displayable() || selection.detached() {
            return Err(terminal_owner_mismatch());
        }
        if let UiTerminalSessionSelector::Explicit { id } = session_selector {
            let requested_id = Uuid::parse_str(id).map_err(|_| terminal_owner_mismatch())?;
            if requested_id != selected_id {
                return Err(terminal_owner_mismatch());
            }
        }
        return Ok(TerminalOwnerWitness::Main {
            owner_window: owner_window.to_string(),
            generation,
            session_id: selected_id,
            selection,
        });
    }

    let label_id = detached_session_from_owner_label(owner_window)
        .ok_or_else(terminal_owner_unsupported)?;
    if let UiTerminalSessionSelector::Explicit { id } = session_selector {
        let requested_id = Uuid::parse_str(id).map_err(|_| terminal_owner_mismatch())?;
        if requested_id != label_id {
            return Err(terminal_owner_mismatch());
        }
    }
    let detached = detached_sessions.lock().map_err(|_| terminal_unavailable())?;
    if !detached.contains(&label_id) {
        return Err(terminal_stale());
    }
    drop(detached);
    Ok(TerminalOwnerWitness::Detached {
        owner_window: owner_window.to_string(),
        generation,
        session_id: label_id,
    })
}

fn terminal_session_fact_blocking(
    session_manager: &SessionManager,
    session_id: Uuid,
) -> Result<TerminalSnapshotSessionFact, TerminalSnapshotTaskError> {
    debug_assert!(
        tokio::runtime::Handle::try_current().is_err(),
        "terminal snapshot blocking lookup entered a Tokio runtime"
    );
    if tokio::runtime::Handle::try_current().is_ok() {
        return Err(terminal_unavailable());
    }
    session_manager
        .terminal_snapshot_session_fact_by_id_blocking(session_id)
        .ok_or_else(terminal_missing)
}

fn revalidate_main_owner(
    state: &UiAutomationState,
    witness: &TerminalOwnerWitness,
    selection_coordinator: &SelectionCoordinator,
) -> Result<(), TerminalSnapshotTaskError> {
    let TerminalOwnerWitness::Main {
        owner_window,
        generation,
        selection,
        ..
    } = witness
    else {
        return Err(terminal_stale());
    };
    if ready_window_generation(state, owner_window) != Some(*generation)
        || selection_snapshot_blocking(selection_coordinator)? != *selection
    {
        return Err(terminal_stale());
    }
    Ok(())
}

fn encode_terminal_snapshot_model(
    request: &UiAutomationRequest,
    witness: &TerminalOwnerWitness,
    fact: &TerminalSnapshotSessionFact,
    route_proof: &PtySnapshotRouteProof,
    model: &terminal_snapshot_renderer::TerminalScreenModel,
) -> Result<Value, TerminalSnapshotTaskError> {
    let rows = usize::from(model.screen.dimensions.rows);
    let columns = usize::from(model.screen.dimensions.columns);
    let cells = rows.checked_mul(columns).ok_or_else(terminal_too_large)?;
    if rows == 0
        || columns == 0
        || rows > MAX_TERMINAL_ROWS
        || columns > MAX_TERMINAL_COLUMNS
        || cells > MAX_TERMINAL_CELLS
    {
        return Err(terminal_too_large());
    }
    if model.session.id != witness.session_id().to_string()
        || !route_proof.ui_model_backend_matches(model, fact.backend_kind)
    {
        return Err(terminal_stale());
    }
    let selection_mode = match request.session {
        Some(UiTerminalSessionSelector::Active) => UiTerminalSelectionMode::Active,
        Some(UiTerminalSessionSelector::Explicit { .. }) => UiTerminalSelectionMode::Explicit,
        None => return Err(terminal_owner_mismatch()),
    };
    let bytes = encode_ui_terminal_snapshot(
        witness.owner_window().to_string(),
        selection_mode,
        model,
        MAX_RESPONSE_JSON_BYTES,
    )
    .map_err(|error| match error {
        ProtocolError::TooLarge => terminal_too_large(),
        ProtocolError::Invalid | ProtocolError::InvalidPng | ProtocolError::Serialization => {
            terminal_unavailable()
        }
    })?;
    serde_json::from_slice(&bytes).map_err(|_| terminal_unavailable())
}

fn capture_terminal_model(
    route_proof: &PtySnapshotRouteProof,
    fact: &TerminalSnapshotSessionFact,
    scope: &crate::config::teams::VerifiedSessionScope,
) -> Result<Arc<terminal_snapshot_renderer::TerminalScreenModel>, TerminalSnapshotTaskError> {
    match route_proof.capture_verified_for_ui(
        fact.backend_kind,
        &scope.cwd,
        scope.replica.as_ref(),
    ) {
        Ok(model) => Ok(model),
        Err(UiTerminalCaptureError::TooLarge) => Err(terminal_too_large()),
        Err(UiTerminalCaptureError::Unavailable) => Err(terminal_unavailable()),
    }
}

fn run_terminal_snapshot_task(
    state: &UiAutomationState,
    app: &AppHandle,
    request: &UiAutomationRequest,
    cancelled: &AtomicBool,
) -> Result<Value, TerminalSnapshotTaskError> {
    run_terminal_snapshot_task_with_hooks(state, app, request, cancelled, &NoopTerminalCaptureHooks)
}

fn with_detached_membership_guard_caught<T>(
    detached_sessions: &DetachedSessionsState,
    operation: impl FnOnce(&HashSet<Uuid>) -> Result<T, TerminalSnapshotTaskError>,
) -> Result<T, TerminalSnapshotTaskError> {
    let guard = detached_sessions
        .lock()
        .map_err(|_| terminal_unavailable())?;
    let outcome = catch_unwind(AssertUnwindSafe(|| operation(&guard)));
    drop(guard);
    match outcome {
        Ok(result) => result,
        Err(_) => Err(terminal_unavailable()),
    }
}

fn run_terminal_snapshot_task_with_hooks(
    state: &UiAutomationState,
    app: &AppHandle,
    request: &UiAutomationRequest,
    cancelled: &AtomicBool,
    hooks: &dyn TerminalCaptureHooks,
) -> Result<Value, TerminalSnapshotTaskError> {
    if cancelled.load(Ordering::SeqCst) {
        return Err(terminal_unavailable());
    }

    let session_manager_state = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .inner()
        .clone();
    let session_manager = tauri::async_runtime::block_on(async move {
        session_manager_state.read().await.clone()
    });
    let pty_manager = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    let selection_coordinator = app.state::<SelectionCoordinator>().inner().clone();
    let detached_sessions = app.state::<DetachedSessionsState>().inner().clone();

    let witness = resolve_terminal_owner_witness_blocking(
        state,
        request,
        &selection_coordinator,
        &detached_sessions,
    )?;
    if cancelled.load(Ordering::SeqCst) {
        return Err(terminal_unavailable());
    }
    let fact = terminal_session_fact_blocking(&session_manager, witness.session_id())?;
    let scope = crate::config::teams::verified_session_scope_from_cwd(Path::new(
        &fact.working_directory,
    ))
    .map_err(|_| terminal_unavailable())?;
    let route_proof = PtyManager::snapshot_route_proof(&pty_manager, witness.session_id())
        .map_err(|_| terminal_unavailable())?;
    if route_proof.backend_kind() != fact.backend_kind
        || !route_proof.matches_current_for_ui(
            fact.backend_kind,
            &scope.cwd,
            scope.replica.as_ref(),
        )
    {
        return Err(terminal_stale());
    }

    match &witness {
        TerminalOwnerWitness::Main { .. } => {
            revalidate_main_owner(state, &witness, &selection_coordinator)?;
            if terminal_session_fact_blocking(&session_manager, witness.session_id())? != fact {
                return Err(terminal_stale());
            }
            if cancelled.load(Ordering::SeqCst) {
                return Err(terminal_unavailable());
            }
            hooks.before_capture();
            hooks.block_capture();
            let model = capture_terminal_model(&route_proof, &fact, &scope)?;
            hooks.after_capture_before_owner_revalidation();
            if cancelled.load(Ordering::SeqCst) {
                return Err(terminal_unavailable());
            }
            revalidate_main_owner(state, &witness, &selection_coordinator)?;
            if terminal_session_fact_blocking(&session_manager, witness.session_id())? != fact
                || !route_proof.matches_current_for_ui(
                    fact.backend_kind,
                    &scope.cwd,
                    scope.replica.as_ref(),
                )
            {
                return Err(terminal_stale());
            }
            encode_terminal_snapshot_model(request, &witness, &fact, &route_proof, &model)
        }
        TerminalOwnerWitness::Detached { session_id, .. } => {
            with_detached_membership_guard_caught(&detached_sessions, |guard| {
                if cancelled.load(Ordering::SeqCst)
                    || !guard.contains(session_id)
                    || ready_window_generation(state, witness.owner_window())
                        != Some(witness.generation())
                {
                    return Err(terminal_stale());
                }
                hooks.after_detached_guard_acquired();
                if terminal_session_fact_blocking(&session_manager, *session_id)? != fact
                    || !route_proof.matches_current_for_ui(
                        fact.backend_kind,
                        &scope.cwd,
                        scope.replica.as_ref(),
                    )
                {
                    return Err(terminal_stale());
                }
                hooks.before_capture();
                hooks.block_capture();
                let model = capture_terminal_model(&route_proof, &fact, &scope)?;
                hooks.after_capture_before_owner_revalidation();
                if cancelled.load(Ordering::SeqCst)
                    || terminal_session_fact_blocking(&session_manager, *session_id)? != fact
                    || ready_window_generation(state, witness.owner_window())
                        != Some(witness.generation())
                    || !guard.contains(session_id)
                    || !route_proof.matches_current_for_ui(
                        fact.backend_kind,
                        &scope.cwd,
                        scope.replica.as_ref(),
                    )
                {
                    return Err(terminal_stale());
                }
                encode_terminal_snapshot_model(request, &witness, &fact, &route_proof, &model)
            })
        }
    }
}

async fn handle_backend_request(
    app: &AppHandle,
    request: &UiAutomationRequest,
) -> UiAutomationResponse {
    if request.action != UiAutomationAction::Backend {
        return UiAutomationResponse::error_for_request(
            request,
            "unsupported_action",
            "Backend automation window only supports backend requests.",
        );
    }

    match request.selector.as_str() {
        RESOURCE_WATCHDOG_BACKEND_SELECTOR => {
            handle_resource_watchdog_backend_request(app, request).await
        }
        _ => UiAutomationResponse::error_for_request(
            request,
            "unsupported_selector",
            "Backend automation selector is not supported.",
        ),
    }
}

async fn handle_resource_watchdog_backend_request(
    app: &AppHandle,
    request: &UiAutomationRequest,
) -> UiAutomationResponse {
    let cfg = crate::config::settings::load_settings();
    handle_resource_watchdog_backend_request_with_config(app, request, &cfg).await
}

async fn handle_resource_watchdog_backend_request_with_config<R: tauri::Runtime>(
    app: &AppHandle<R>,
    request: &UiAutomationRequest,
    cfg: &crate::config::settings::AppSettings,
) -> UiAutomationResponse {
    let mode = match BackendWatchdogMode::parse(request.value.as_deref()) {
        Ok(mode) => mode,
        Err(message) => {
            return UiAutomationResponse::error_for_request(request, "invalid_value", &message);
        }
    };

    let Some(monitor) = app.try_state::<Arc<crate::resource_monitor::ResourceMonitorState>>()
    else {
        return UiAutomationResponse::error_for_request(
            request,
            "backend_state_missing",
            "Resource monitor state is not registered.",
        );
    };

    let limits = crate::resource_monitor::ResourceLimits::from(cfg);
    let resource_monitor_enabled = monitor.is_effectively_enabled(cfg.resource_monitor_enabled);
    let snapshot = monitor.snapshot(limits);
    let groups = snapshot
        .groups
        .iter()
        .filter_map(|group| {
            Uuid::parse_str(&group.session_id)
                .ok()
                .map(|session_id| (session_id, group.clone()))
        })
        .collect::<Vec<_>>();
    let decisions = crate::resource_monitor::watchdog::evaluate_watchdog_groups(&groups, limits);
    let warn_matches = decisions
        .iter()
        .filter(|decision| decision.warn_required)
        .count();
    let kill_matches = decisions
        .iter()
        .filter(|decision| decision.kill_required)
        .count();

    let kill_enabled = resource_monitor_enabled
        && (matches!(mode, BackendWatchdogMode::KillGroup)
            || (matches!(mode, BackendWatchdogMode::Tick)
                && cfg.resource_watchdog_action
                    == crate::config::settings::ResourceWatchdogAction::KillGroup));

    let mut kill_results = Vec::new();
    if kill_enabled {
        for decision in decisions.iter().filter(|decision| decision.kill_required) {
            let Some(coordinator) =
                app.try_state::<crate::session::selection::SelectionCoordinator>()
            else {
                kill_results.push(json!({
                    "ok": false,
                    "sessionId": decision.session_id,
                    "message": "selectionCoordinatorUnavailable",
                }));
                continue;
            };
            match coordinator
                .watchdog_resource_kill(decision.session_id)
                .await
            {
                Ok(crate::session::selection::WatchdogKillOutcome::Completed(result)) => {
                    kill_results.push(json!({
                        "ok": true,
                        "result": result,
                    }))
                }
                // #1151 - `alreadyPending` keeps its value and type byte-identical so every
                // existing automation assertion still passes; `reason` is the additive
                // discriminator that makes the split observable from outside the process.
                Ok(crate::session::selection::WatchdogKillOutcome::AlreadyInFlight) => kill_results
                    .push(json!({
                        "ok": true,
                        "sessionId": decision.session_id,
                        "alreadyPending": true,
                        "reason": "alreadyInFlight",
                    })),
                Ok(crate::session::selection::WatchdogKillOutcome::NoPublicSession) => kill_results
                    .push(json!({
                        "ok": true,
                        "sessionId": decision.session_id,
                        "alreadyPending": true,
                        "reason": "noPublicSession",
                    })),
                Err(message) => kill_results.push(json!({
                    "ok": false,
                    "sessionId": decision.session_id,
                    "message": message,
                })),
            }
        }
    }

    // #1151 - run the PRODUCTION quarantine-retry gate and loop, not a mirrored copy, so
    // these reports are evidence about shipped dispatch logic. The coordinator is resolved
    // ONCE before the call, because the loop takes it by reference for every group.
    let mut quarantine_retries = Vec::new();
    if resource_monitor_enabled
        && matches!(
            mode,
            BackendWatchdogMode::QuarantineRetry | BackendWatchdogMode::Tick
        )
    {
        match app.try_state::<crate::session::selection::SelectionCoordinator>() {
            Some(coordinator) => {
                for report in crate::resource_monitor::watchdog::run_quarantine_retries(
                    &monitor,
                    coordinator.inner(),
                    &groups,
                )
                .await
                {
                    quarantine_retries.push(serde_json::to_value(&report).unwrap_or_else(
                        |error| json!({ "ok": false, "message": error.to_string() }),
                    ));
                }
            }
            None => quarantine_retries.push(json!({
                "ok": false,
                "message": "selectionCoordinatorUnavailable",
            })),
        }
    }

    let state = if !resource_monitor_enabled {
        "disabled"
    } else if !kill_results.is_empty() {
        "enforcing"
    } else if kill_matches > 0 {
        "critical"
    } else if warn_matches > 0 {
        "warn"
    } else {
        "ok"
    };

    let snapshot_diagnostics = if monitor.supports_process_tree_enforcement() {
        json!({
            "capturedAt": snapshot.captured_at,
            "overallState": snapshot.overall_state,
            "activeAgentGroups": snapshot.active_agent_groups,
            "appPrivateBytes": snapshot.app_private_bytes,
            "networkState": snapshot.network_state,
            "networkSummary": snapshot.network_summary,
            "warnings": snapshot.warnings,
        })
    } else {
        json!({
            "overallState": snapshot.overall_state,
            "activeAgentGroups": snapshot.active_agent_groups,
            "appPrivateBytes": snapshot.app_private_bytes,
            "networkState": snapshot.network_state,
            "networkSummary": snapshot.network_summary,
            "warnings": snapshot.warnings,
        })
    };

    UiAutomationResponse {
        ok: true,
        request_id: request.request_id.clone(),
        window: request.window.clone(),
        action: request.action,
        selector: request.selector.clone(),
        target: Some(json!({
            "testId": request.selector,
            "role": "backend",
            "state": state,
            "tag": "backend",
            "text": format!(
                "resource monitor watchdog {}: {} group(s), {} warn match(es), {} kill match(es)",
                mode.as_str(),
                groups.len(),
                warn_matches,
                kill_matches
            ),
            "visible": true,
            "disabled": false,
        })),
        error: None,
        message: None,
        available: None,
        diagnostics: Some(json!({
            "mode": mode.as_str(),
            "configuredAction": cfg.resource_watchdog_action,
            "resourceMonitorEnabled": resource_monitor_enabled,
            "killApplied": kill_enabled,
            "limits": {
                "maxConcurrentAgentGroups": limits.max_concurrent_agent_processes,
                "groupWarnPrivateBytes": limits.group_warn_private_bytes,
                "groupKillPrivateBytes": limits.group_kill_private_bytes,
                "processKillPrivateBytes": limits.process_kill_private_bytes,
            },
            // #1151 - `snapshot` is captured BEFORE the action loop and is deliberately
            // left that way. Every post-retry assertion reads `quarantineRetries[i]`
            // instead; reading `snapshot.activeAgentGroups` after a successful reclaim
            // still shows the pre-action figure and produces a false FAIL.
            "snapshot": snapshot_diagnostics,
            "decisions": decisions,
            "killResults": kill_results,
            "quarantineRetries": quarantine_retries,
        })),
        available_windows: None,
        timeout_ms: None,
        phase: None,
        active_test_id: Value::Null,
        filters: None,
        targets: None,
        matched_count: None,
        matched_count_exact: None,
        returned_count: None,
        limit: None,
        truncated: None,
        scan: None,
        terminal_snapshot: None,
    }
}

fn run_cli_request(
    context: &UiCliDispatchContext,
    input: &CliRequest,
) -> Result<UiAutomationResponse, Value> {
    ensure_current_exe_is_testable()?;
    validate_cli_request(input)?;

    if !context.verify_current() {
        return Err(automation_config_identity_unavailable_error());
    }
    let config_dir = context.canonical_path().to_path_buf();
    let automation_dir = config_dir.join(UI_AUTOMATION_DIR);
    let session_path = automation_dir.join(SESSION_FILE);
    let session = load_session_for_cli(context, &input.window)?;

    context.with_owned_automation_fs(|_| {
        prepare_automation_mailbox_tree(&automation_dir, false).map_err(|_| {
            preflight_error(
                "automation_filesystem_error",
                "Failed to prepare automation mailbox directories.",
                None,
            )
        })
    })?;

    let request_id = Uuid::new_v4().to_string();
    let request = UiAutomationRequest {
        schema_version: PROTOCOL_SCHEMA_VERSION,
        instance_id: session.instance_id,
        pid: session.pid,
        started_at_unix_ms: session.started_at_unix_ms,
        request_id: request_id.clone(),
        token: session.token,
        exe_path: session.exe_path,
        config_dir: session.config_dir,
        window: input.window.clone(),
        action: input.action,
        selector: input.selector.clone(),
        prefix: input.prefix.clone(),
        role: input.role.clone(),
        owner_window: input.owner_window.clone(),
        session: input.session.clone(),
        value: input.value.clone(),
        expires_at_unix_ms: Some(request_expires_at_unix_ms(input.timeout_ms)),
    };

    let request_path = automation_dir
        .join(REQUESTS_DIR)
        .join(format!("{request_id}.json"));
    let inflight_path = automation_dir
        .join(REQUESTS_DIR)
        .join(format!("{request_id}.inflight.json"));
    let response_path = automation_dir
        .join(RESPONSES_DIR)
        .join(format!("{request_id}.json"));

    context.with_owned_automation_fs(|_| {
        write_json_atomic_new(&request_path, &request).map_err(|_| {
            response_error_value(
                &request,
                "automation_filesystem_error",
                "Failed to write automation request file.",
                None,
            )
        })
    })?;

    let deadline = Instant::now() + Duration::from_millis(input.timeout_ms);
    loop {
        let response_exists = context.with_owned_automation_fs(|_| {
            Ok(response_path.try_exists().unwrap_or(false))
        })?;
        if response_exists {
            let raw = context.with_owned_automation_fs(|_| {
                read_bounded_regular_file(&response_path, MAX_RESPONSE_JSON_BYTES).map_err(
                    |error| {
                        let (code, message) = if error.kind() == io::ErrorKind::InvalidData
                            && error.to_string() == "response_too_large"
                        {
                            (
                                "response_too_large",
                                "Automation response file exceeded its byte limit.",
                            )
                        } else {
                            (
                                "automation_filesystem_error",
                                "Failed to read automation response file.",
                            )
                        };
                        response_error_value(&request, code, message, None)
                    },
                )
            })?;
            let response: UiAutomationResponse = serde_json::from_str(&raw).map_err(|_| {
                response_error_value(
                    &request,
                    "automation_filesystem_error",
                    "Automation response file was not valid JSON.",
                    None,
                )
            })?;
            let correlation = validate_response_correlation(&request, &response);
            context.with_owned_automation_fs(|_| {
                retry_remove_file(&response_path).map_err(|_| automation_config_identity_unavailable_error())
            })?;
            context.with_owned_automation_fs(|_| {
                retry_remove_file(&request_path).map_err(|_| automation_config_identity_unavailable_error())
            })?;
            context.with_owned_automation_fs(|_| {
                retry_remove_file(&inflight_path).map_err(|_| automation_config_identity_unavailable_error())
            })?;
            correlation?;
            return Ok(sanitize_response_for_cli(response));
        }

        if Instant::now() >= deadline {
            let mut timeout = UiAutomationResponse::error_for_request(
                &request,
                "timeout",
                "Automation request timed out before the GUI returned a response.",
            );
            timeout.timeout_ms = Some(input.timeout_ms);
            timeout.phase = Some(timeout_phase(
                context,
                &request_path,
                &inflight_path,
                &session_path,
                &input.window,
            )?);
            context.with_owned_automation_fs(|_| {
                retry_remove_file(&request_path).map_err(|_| automation_config_identity_unavailable_error())
            })?;
            context.with_owned_automation_fs(|_| {
                retry_remove_file(&inflight_path).map_err(|_| automation_config_identity_unavailable_error())
            })?;
            return Ok(timeout);
        }

        std::thread::sleep(Duration::from_millis(POLL_MS));
    }
}

fn validate_response_correlation(
    request: &UiAutomationRequest,
    response: &UiAutomationResponse,
) -> Result<(), Value> {
    let mismatch = || {
        response_error_value(
            request,
            "automation_protocol_mismatch",
            "Automation response did not match the pending request contract.",
            None,
        )
    };
    if response.request_id != request.request_id
        || response.window != request.window
        || response.action != request.action
        || response.selector != request.selector
        || (response.ok && (response.error.is_some() || response.message.is_some()))
        || (!response.ok && (response.error.is_none() || response.message.is_none()))
    {
        return Err(mismatch());
    }
    let has_list_payload = response.filters.is_some()
        || response.targets.is_some()
        || response.matched_count.is_some()
        || response.matched_count_exact.is_some()
        || response.returned_count.is_some()
        || response.limit.is_some()
        || response.truncated.is_some()
        || response.scan.is_some();
    if request.action == UiAutomationAction::List {
        if response.ok != has_list_payload
            || response.target.is_some()
            || response.available.is_some()
            || response.diagnostics.is_some()
            || response.available_windows.is_some()
            || response.timeout_ms.is_some()
            || response.phase.is_some()
            || response.terminal_snapshot.is_some()
        {
            return Err(mismatch());
        }
    } else if has_list_payload {
        return Err(mismatch());
    }
    if response.available.is_some()
        && !matches!(
            response.error.as_deref(),
            Some("missing_selector" | "duplicate_selector")
        )
    {
        return Err(mismatch());
    }
    let terminal_request = request.action == UiAutomationAction::Backend
        && request.selector == "terminal.snapshot";
    if terminal_request {
        if response.ok != response.terminal_snapshot.is_some()
            || response.target.is_some()
            || response.available.is_some()
            || response.diagnostics.is_some()
            || response.available_windows.is_some()
            || !response.active_test_id.is_null()
        {
            return Err(mismatch());
        }
        if let Some(snapshot) = response.terminal_snapshot.as_ref() {
            let document = serde_json::from_value::<UiTerminalSnapshotDocument>(snapshot.clone())
                .map_err(|_| mismatch())?;
            let expected_mode = match request.session.as_ref() {
                Some(UiTerminalSessionSelector::Active) => UiTerminalSelectionMode::Active,
                Some(UiTerminalSessionSelector::Explicit { .. }) => {
                    UiTerminalSelectionMode::Explicit
                }
                None => return Err(mismatch()),
            };
            if document.schema_version != PROTOCOL_SCHEMA_VERSION
                || document.kind != "ui-terminal-snapshot"
                || request.owner_window.as_deref() != Some(document.owner_window.as_str())
                || document.selection_mode != expected_mode
            {
                return Err(mismatch());
            }
            let expected_session = match request.session.as_ref() {
                Some(UiTerminalSessionSelector::Explicit { id }) => Some(id.clone()),
                Some(UiTerminalSessionSelector::Active) => request
                    .owner_window
                    .as_deref()
                    .and_then(detached_session_from_owner_label)
                    .map(|id| id.to_string())
                    .filter(|_| request.owner_window.as_deref() != Some("main")),
                None => None,
            };
            if expected_session.is_some_and(|id| document.session.id != id) {
                return Err(mismatch());
            }
        }
    } else if response.terminal_snapshot.is_some() {
        return Err(mismatch());
    }
    if !matches!(response.active_test_id, Value::Null | Value::String(_)) {
        return Err(mismatch());
    }
    Ok(())
}

fn validate_cli_request(input: &CliRequest) -> Result<(), Value> {
    if input.timeout_ms == 0 || input.timeout_ms > MAX_TIMEOUT_MS {
        return Err(preflight_error(
            "invalid_timeout",
            "Automation timeout must be between 1 and 60000 milliseconds.",
            None,
        ));
    }
    if input.window.as_bytes().len() > MAX_WINDOW_LABEL_BYTES {
        return Err(preflight_error(
            "window_too_large",
            "Automation window label exceeded its limit.",
            None,
        ));
    }
    if input.owner_window.as_ref().is_some_and(|value| {
        value.is_empty() || value.as_bytes().len() > MAX_WINDOW_LABEL_BYTES
    }) {
        return Err(preflight_error(
            "window_too_large",
            "Automation owner window label exceeded its limit.",
            None,
        ));
    }
    if input.session.as_ref().is_some_and(|session| match session {
        UiTerminalSessionSelector::Active => false,
        UiTerminalSessionSelector::Explicit { id } => !is_uuid_v4(id),
    }) {
        return Err(preflight_error(
            "invalid_terminal_session",
            "Terminal session selection was invalid.",
            None,
        ));
    }
    if input.selector.as_bytes().len() > MAX_SELECTOR_BYTES {
        return Err(preflight_error(
            "selector_too_large",
            "Automation selector exceeded its limit.",
            None,
        ));
    }
    if input
        .prefix
        .as_ref()
        .is_some_and(|value| value.as_bytes().len() > MAX_PREFIX_BYTES)
    {
        return Err(preflight_error(
            "prefix_too_large",
            "Automation list prefix exceeded its limit.",
            None,
        ));
    }
    if input
        .role
        .as_ref()
        .is_some_and(|value| value.as_bytes().len() > MAX_ROLE_BYTES)
    {
        return Err(preflight_error(
            "role_too_large",
            "Automation role exceeded its limit.",
            None,
        ));
    }
    if input.role.as_ref().is_some_and(|role| {
        !SUPPORTED_ROLES.iter().any(|supported| supported == &role.as_str())
    }) {
        return Err(preflight_error(
            "invalid_role",
            "Automation role was not supported.",
            None,
        ));
    }
    if input
        .value
        .as_ref()
        .is_some_and(|value| value.as_bytes().len() > MAX_VALUE_BYTES)
    {
        return Err(preflight_error(
            "value_too_large",
            "Automation value exceeded its limit.",
            None,
        ));
    }
    let fields_match_action = match input.action {
        UiAutomationAction::List => {
            input.selector.is_empty()
                && input.value.is_none()
                && input.owner_window.is_none()
                && input.session.is_none()
        }
        UiAutomationAction::Backend if input.selector == "terminal.snapshot" => {
            input.value.is_none() && input.owner_window.is_some() && input.session.is_some()
        }
        UiAutomationAction::Backend => input.owner_window.is_none() && input.session.is_none(),
        UiAutomationAction::SetValue | UiAutomationAction::TypeText => {
            !input.selector.is_empty()
                && input.value.is_some()
                && input.owner_window.is_none()
                && input.session.is_none()
        }
        UiAutomationAction::Hover => {
            input.owner_window.is_none()
                && input.session.is_none()
                && (!input.selector.is_empty() || input.value.as_deref() == Some("leave"))
        }
        UiAutomationAction::Query
        | UiAutomationAction::Click
        | UiAutomationAction::ContextClick
        | UiAutomationAction::Focus => {
            !input.selector.is_empty()
                && input.value.is_none()
                && input.owner_window.is_none()
                && input.session.is_none()
        }
    };
    if !fields_match_action {
        return Err(preflight_error(
            "malformed_request",
            "Automation request fields did not match the selected action.",
            None,
        ));
    }
    Ok(())
}

pub fn resolve_enabled_from_cli_or_env(
    ui_automation: bool,
    testable_artifact: bool,
) -> Result<bool, String> {
    let env_enabled = std::env::var(ENV_ENABLE)
        .ok()
        .is_some_and(|value| value == "1");
    let requested = ui_automation || env_enabled;
    if !requested {
        return Ok(false);
    }
    if !testable_artifact {
        return Err(refusing_non_testeable_binary_error().to_string());
    }
    Ok(true)
}

pub fn existing_enabled_session_for_current_config() -> bool {
    let Some(config_dir) = crate::config::config_dir() else {
        return false;
    };
    let session_path = config_dir.join(UI_AUTOMATION_DIR).join(SESSION_FILE);
    let Ok(raw) = read_session_file_with_retry(&session_path) else {
        return false;
    };
    let Ok(session) = serde_json::from_str::<UiAutomationSession>(&raw) else {
        return false;
    };
    validate_session_liveness(
        &session,
        &config_dir,
        &OsLiveProcessIdentityProbe,
    )
    .is_ok()
}

pub fn automation_not_enabled_json() -> String {
    compact_preflight_json(&automation_not_enabled_error())
}

fn automation_not_enabled_error() -> Value {
    preflight_error(
        "automation_not_enabled",
        "A testable GUI is already running without UI automation enabled. Restart it with --ui-automation or AC_UI_AUTOMATION=1.",
        None,
    )
}

fn load_session_for_cli(
    context: &UiCliDispatchContext,
    requested_window: &str,
) -> Result<UiAutomationSession, Value> {
    load_session_for_cli_with_process_probe(
        context,
        requested_window,
        &OsLiveProcessIdentityProbe,
        None,
    )
}

fn load_session_for_cli_with_process_probe(
    context: &UiCliDispatchContext,
    requested_window: &str,
    process_probe: &dyn LiveProcessIdentityProbe,
    hooks: Option<&dyn SessionLoadTestHooks>,
) -> Result<UiAutomationSession, Value> {
    let path = context
        .canonical_path()
        .join(UI_AUTOMATION_DIR)
        .join(SESSION_FILE);
    let raw = match context.with_owned_automation_fs(|_| {
        read_session_file_with_retry(&path).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                json!({ "kind": "not_found" })
            } else if error.kind() == io::ErrorKind::InvalidData
                && error.to_string() == "session_file_too_large"
            {
                json!({ "kind": "too_large" })
            } else {
                preflight_error(
                    "automation_filesystem_error",
                    "Failed to read UI automation session file.",
                    None,
                )
            }
        })
    }) {
        Ok(raw) => raw,
        Err(error) if error.get("kind").and_then(Value::as_str) == Some("not_found") => {
            let (error, message) = match crate::config::daemon_pid::detect_daemon_state() {
                crate::config::daemon_pid::DaemonState::Running { .. } => {
                    return Err(automation_not_enabled_error());
                }
                _ => (
                    "automation_session_missing",
                    "No UI automation session file exists for this testable binary.",
                ),
            };
            return Err(preflight_error(error, message, None));
        }
        Err(error) if error.get("kind").and_then(Value::as_str) == Some("too_large") => {
            return Err(preflight_error(
                "session_file_too_large",
                "UI automation session file exceeded its byte limit.",
                None,
            ));
        }
        Err(error) => return Err(error),
    };

    if let Some(hooks) = hooks {
        hooks.after_session_bytes_read();
    }

    let session: UiAutomationSession =
        serde_json::from_str(&raw).map_err(|_| automation_session_stale_error())?;

    validate_session_liveness(&session, context.canonical_path(), process_probe)?;
    if session.window_inventory.status == WindowInventoryStatus::Overflow {
        return Err(registered_window_limit_error(
            session.window_inventory.limit,
            session.window_inventory.observed_count,
        ));
    }
    if !is_backend_automation_window(requested_window)
        && !session
            .window_labels
            .iter()
            .any(|label| label == requested_window)
    {
        let mut value = preflight_error(
            "window_unavailable",
            "Requested automation window is not registered in the running session.",
            None,
        );
        if let Some(obj) = value.as_object_mut() {
            obj.insert("availableWindows".to_string(), json!(session.window_labels));
        }
        return Err(value);
    }
    Ok(session)
}

fn validate_session_liveness(
    session: &UiAutomationSession,
    expected_config_dir: &Path,
    process_probe: &dyn LiveProcessIdentityProbe,
) -> Result<(), Value> {
    if !session_inventory_is_coherent(session)
        || session.schema_version != PROTOCOL_SCHEMA_VERSION
        || session.pid == 0
        || session.started_at_unix_ms <= 0
        || !is_uuid_v4(&session.instance_id)
        || !is_uuid_v4(&session.token)
        || session.exe_path.is_empty()
        || session.config_dir.is_empty()
        || !paths_equal_for_compare(Path::new(&session.config_dir), expected_config_dir)
    {
        return Err(automation_session_stale_error());
    }

    let current_executable = std::env::current_exe()
        .ok()
        .map(|path| canonical_for_compare(&path))
        .ok_or_else(automation_session_stale_error)?;
    if !paths_equal_for_compare(Path::new(&session.exe_path), &current_executable) {
        return Err(automation_session_stale_error());
    }

    let identity = process_probe
        .probe(session.pid)
        .ok_or_else(automation_session_stale_error)?;
    if identity.started_at_unix_ms != session.started_at_unix_ms
        || !paths_equal_for_compare(&identity.executable, Path::new(&session.exe_path))
    {
        return Err(automation_session_stale_error());
    }
    Ok(())
}

fn session_inventory_is_coherent(session: &UiAutomationSession) -> bool {
    fn sorted_unique_bounded(labels: &[String]) -> bool {
        labels.iter().all(|label| {
            !label.is_empty() && label.as_bytes().len() <= MAX_WINDOW_LABEL_BYTES
        }) && labels.windows(2).all(|pair| pair[0] < pair[1])
    }

    if session.window_inventory.limit != MAX_REGISTERED_WINDOWS as u32 {
        return false;
    }
    match session.window_inventory.status {
        WindowInventoryStatus::Ready => {
            session.window_inventory.observed_count
                == u32::try_from(session.window_labels.len()).unwrap_or(u32::MAX)
                && session.window_labels.len() <= MAX_REGISTERED_WINDOWS
                && sorted_unique_bounded(&session.window_labels)
                && sorted_unique_bounded(&session.ready_window_labels)
                && session
                    .ready_window_labels
                    .iter()
                    .all(|ready| session.window_labels.binary_search(ready).is_ok())
        }
        WindowInventoryStatus::Overflow => {
            session.window_inventory.observed_count > MAX_REGISTERED_WINDOWS as u32
                && session.window_labels.is_empty()
                && session.ready_window_labels.is_empty()
        }
    }
}

fn ensure_current_exe_is_testable() -> Result<(), Value> {
    if current_exe_is_testable() {
        Ok(())
    } else {
        Err(refusing_non_testeable_binary_error())
    }
}

pub fn current_exe_is_testable() -> bool {
    cfg!(feature = "testable-ui-automation")
        && std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .is_some_and(|name| name == TESTABLE_EXE_NAME)
}

struct OrderedPreflightValue<'a>(&'a serde_json::Map<String, Value>);

impl Serialize for OrderedPreflightValue<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for key in ["ok", "error", "message"] {
            if let Some(value) = self.0.get(key) {
                map.serialize_entry(key, value)?;
            }
        }
        for (key, value) in self.0 {
            if !matches!(key.as_str(), "ok" | "error" | "message") {
                map.serialize_entry(key, value)?;
            }
        }
        map.end()
    }
}

fn compact_preflight_json(value: &Value) -> String {
    match value.as_object() {
        Some(object) => serde_json::to_string(&OrderedPreflightValue(object)),
        None => serde_json::to_string(value),
    }
    .expect("serializing a serde_json::Value cannot fail")
}

fn preflight_error(error: &str, message: &str, diagnostics: Option<Value>) -> Value {
    let mut value = json!({
        "ok": false,
        "error": error,
        "message": message,
    });
    if let Some(diagnostics) = diagnostics {
        value["diagnostics"] = diagnostics;
    }
    value
}

fn refusing_non_testeable_binary_error() -> Value {
    preflight_error(
        "refusing_non_testeable_binary",
        "UI automation is only available from agentscommander_testeable.exe.",
        None,
    )
}

pub fn refusing_non_testeable_binary_json() -> String {
    compact_preflight_json(&refusing_non_testeable_binary_error())
}

fn automation_config_identity_unavailable_error() -> Value {
    preflight_error(
        "automation_config_identity_unavailable",
        "Could not prove the testable configuration directory identity.",
        None,
    )
}

pub fn automation_config_identity_unavailable_json() -> String {
    compact_preflight_json(&automation_config_identity_unavailable_error())
}

pub fn execute_missing_cli_context() -> i32 {
    print_stdout_value(&automation_config_identity_unavailable_error());
    1
}

pub fn automation_session_missing_json() -> String {
    compact_preflight_json(&preflight_error(
        "automation_session_missing",
        "No UI automation session file exists for this testable binary.",
        None,
    ))
}

fn automation_session_stale_error() -> Value {
    preflight_error(
        "automation_session_stale",
        "The UI automation session no longer identifies the live GUI process.",
        None,
    )
}

fn registered_window_limit_error(limit: u32, observed_count: u32) -> Value {
    json!({
        "ok": false,
        "error": "registered_window_limit_exceeded",
        "message": "The running GUI registered more automation windows than the supported limit.",
        "limit": limit,
        "observedCount": observed_count,
    })
}

fn response_error_value(
    request: &UiAutomationRequest,
    error: &str,
    message: &str,
    diagnostics: Option<Value>,
) -> Value {
    let mut response = UiAutomationResponse::error_for_request(request, error, message);
    response.diagnostics = diagnostics;
    serde_json::to_value(response).unwrap_or_else(|_| {
        json!({
            "ok": false,
            "error": "json_serialize_failed",
            "message": "Failed to serialize automation response."
        })
    })
}

fn request_expires_at_unix_ms(timeout_ms: u64) -> i64 {
    let timeout_ms = timeout_ms.min(i64::MAX as u64) as i64;
    now_unix_ms().saturating_add(timeout_ms)
}

fn request_expired(request: &UiAutomationRequest, now_ms: i64) -> bool {
    request
        .expires_at_unix_ms
        .is_some_and(|expires_at| expires_at <= now_ms)
}

fn expired_response_for_request(request: &UiAutomationRequest) -> UiAutomationResponse {
    let mut response = UiAutomationResponse::error_for_request(
        request,
        "request_expired",
        "Automation request expired before the GUI emitted it to the frontend.",
    );
    response.diagnostics = Some(json!({ "expiresAtUnixMs": request.expires_at_unix_ms }));
    response
}

fn sanitize_response_for_cli(mut response: UiAutomationResponse) -> UiAutomationResponse {
    let truncation = response.available.as_mut().and_then(|available| {
        let total = available.len();
        for target in available.iter_mut() {
            sanitize_target_for_cli(target);
        }
        if total > CLI_MAX_AVAILABLE_TARGETS {
            available.truncate(CLI_MAX_AVAILABLE_TARGETS);
            Some((total, CLI_MAX_AVAILABLE_TARGETS))
        } else {
            None
        }
    });

    if let Some((total, limit)) = truncation {
        add_cli_truncation_diagnostics(&mut response, total, limit);
    }

    response
}

fn sanitize_target_for_cli(target: &mut Value) {
    let Some(obj) = target.as_object_mut() else {
        return;
    };
    let Some(Value::String(text)) = obj.get_mut("text") else {
        return;
    };
    if text.chars().count() <= CLI_MAX_TARGET_TEXT_CHARS {
        return;
    }
    let mut truncated = text
        .chars()
        .take(CLI_MAX_TARGET_TEXT_CHARS)
        .collect::<String>();
    truncated.push_str("...");
    *text = truncated;
}

fn add_cli_truncation_diagnostics(
    response: &mut UiAutomationResponse,
    available_total: usize,
    available_limit: usize,
) {
    let mut diagnostics = response
        .diagnostics
        .take()
        .filter(|value| value.is_object())
        .and_then(|value| match value {
            Value::Object(map) => Some(map),
            _ => None,
        })
        .unwrap_or_default();

    diagnostics.insert("availableTotal".to_string(), json!(available_total));
    diagnostics.insert("availableLimit".to_string(), json!(available_limit));
    diagnostics.insert("availableTruncated".to_string(), json!(true));
    response.diagnostics = Some(Value::Object(diagnostics));
}

fn print_stdout_json<T: Serialize>(value: &T) {
    match serde_json::to_string(value) {
        Ok(json) => print_stdout_compact(json),
        Err(_) => crate::cli_println!(
            "{{\"ok\":false,\"error\":\"json_serialize_failed\",\"message\":\"Failed to serialize automation response.\"}}"
        ),
    }
}

fn print_stdout_value(value: &Value) {
    print_stdout_compact(compact_preflight_json(value));
}

fn print_stdout_compact(json: String) {
    if json.len() <= MAX_STDOUT_JSON_BYTES {
        crate::cli_println!("{json}");
    } else {
        crate::cli_println!(
            "{{\"ok\":false,\"error\":\"output_too_large\",\"message\":\"Automation output exceeded its byte limit.\"}}"
        );
    }
}

fn timeout_phase(
    context: &UiCliDispatchContext,
    request_path: &Path,
    inflight_path: &Path,
    session_path: &Path,
    window: &str,
) -> Result<String, Value> {
    if context.with_owned_automation_fs(|_| Ok(request_path.try_exists().unwrap_or(false)))? {
        return Ok("awaiting_gui_poller".to_string());
    }
    if context.with_owned_automation_fs(|_| Ok(inflight_path.try_exists().unwrap_or(false)))? {
        if is_backend_automation_window(window) {
            return Ok("awaiting_backend_response".to_string());
        }
        if session_ready_for_window(context, session_path, window)? {
            Ok("awaiting_frontend_response".to_string())
        } else {
            Ok("awaiting_frontend_ready".to_string())
        }
    } else {
        Ok("awaiting_gui_poller".to_string())
    }
}

fn session_ready_for_window(
    context: &UiCliDispatchContext,
    path: &Path,
    window: &str,
) -> Result<bool, Value> {
    if is_backend_automation_window(window) {
        return Ok(true);
    }
    let raw = context.with_owned_automation_fs(|_| {
        read_session_file_with_retry(path).map_err(|_| automation_config_identity_unavailable_error())
    })?;
    Ok(serde_json::from_str::<UiAutomationSession>(&raw)
        .ok()
        .is_some_and(|session| {
            session
                .ready_window_labels
                .iter()
                .any(|label| label == window)
        }))
}

fn is_backend_automation_window(window: &str) -> bool {
    window == BACKEND_AUTOMATION_WINDOW
}

fn write_json_atomic_new<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let committed = write_json_atomic(path, value, false, || Ok(true))?;
    if committed {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "atomic publication cancelled",
        ))
    }
}

fn write_json_atomic_new_with_precommit<T: Serialize>(
    path: &Path,
    value: &T,
    before_commit: impl FnOnce() -> io::Result<bool>,
) -> io::Result<bool> {
    write_json_atomic(path, value, false, before_commit)
}

#[cfg(test)]
fn write_json_atomic_replace<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let committed = write_json_atomic(path, value, true, || Ok(true))?;
    if committed {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "atomic publication cancelled",
        ))
    }
}

#[cfg(test)]
fn bounded_sorted_request_paths<I>(entries: I) -> io::Result<Vec<PathBuf>>
where
    I: Iterator<Item = io::Result<PathBuf>>,
{
    let mut paths = entries
        .take(MAX_REQUEST_FILES_PER_SCAN)
        .collect::<io::Result<Vec<_>>>()?;
    paths.sort();
    Ok(paths)
}

fn remove_invalid_request_entry(path: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.is_dir() {
        retry_fs(|| fs::remove_dir(path))
    } else {
        retry_remove_file(path)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OpenedObjectIdentity {
    volume: u64,
    file: u64,
    links: u64,
}

impl OpenedObjectIdentity {
    fn same_object(self, other: Self) -> bool {
        self.volume == other.volume && self.file == other.file
    }
}

#[derive(Debug, Clone, Copy)]
struct AtomicCommitGenerations {
    source: OpenedObjectIdentity,
    destination: Option<OpenedObjectIdentity>,
}

struct RetainedAutomationDirectory {
    path: PathBuf,
    handle: File,
    identity: OpenedObjectIdentity,
}

impl RetainedAutomationDirectory {
    fn verify_current(&self) -> io::Result<()> {
        validate_opened_directory(&self.handle)?;
        if !opened_object_identity(&self.handle)?.same_object(self.identity) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "unsafe_automation_directory",
            ));
        }
        let current = open_automation_directory_no_follow(&self.path)?;
        if !opened_object_identity(&current)?.same_object(self.identity) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "unsafe_automation_directory",
            ));
        }
        Ok(())
    }

    fn sync_best_effort(&self) {
        let _ = self.handle.sync_all();
    }
}

struct RetainedAutomationDirectoryChain {
    root: RetainedAutomationDirectory,
    child: Option<RetainedAutomationDirectory>,
}

impl RetainedAutomationDirectoryChain {
    fn verify_current(&self) -> io::Result<()> {
        self.root.verify_current()?;
        if let Some(child) = self.child.as_ref() {
            child.verify_current()?;
            self.root.verify_current()?;
        }
        Ok(())
    }

    fn sync_best_effort(&self) {
        if let Some(child) = self.child.as_ref() {
            child.sync_best_effort();
        }
        self.root.sync_best_effort();
    }
}

struct AtomicWriteDirectories {
    staging: RetainedAutomationDirectoryChain,
    destination: Option<RetainedAutomationDirectoryChain>,
}

impl AtomicWriteDirectories {
    fn verify_current(&self) -> io::Result<()> {
        self.staging.verify_current()?;
        if let Some(destination) = self.destination.as_ref() {
            destination.verify_current()?;
            self.staging.verify_current()?;
        }
        Ok(())
    }

    fn sync_best_effort(&self) {
        if let Some(destination) = self.destination.as_ref() {
            destination.sync_best_effort();
        }
        self.staging.sync_best_effort();
    }
}

fn retain_automation_directory_no_follow(path: &Path) -> io::Result<RetainedAutomationDirectory> {
    let handle = open_automation_directory_no_follow(path)?;
    let identity = opened_object_identity(&handle)?;
    Ok(RetainedAutomationDirectory {
        path: path.to_path_buf(),
        handle,
        identity,
    })
}

fn open_automation_directory_no_follow(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
            FILE_SHARE_WRITE,
        };
        options
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path)?;
    validate_opened_directory(&file)?;
    Ok(file)
}

fn validate_opened_directory(file: &File) -> io::Result<fs::Metadata> {
    let metadata = file.metadata()?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unsafe_automation_directory",
        ));
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "unsafe_automation_directory",
            ));
        }
    }
    Ok(metadata)
}

#[cfg(unix)]
fn opened_object_identity(file: &File) -> io::Result<OpenedObjectIdentity> {
    use std::os::unix::fs::MetadataExt;

    let metadata = file.metadata()?;
    Ok(OpenedObjectIdentity {
        volume: metadata.dev(),
        file: metadata.ino(),
        links: metadata.nlink(),
    })
}

#[cfg(target_os = "windows")]
fn opened_object_identity(file: &File) -> io::Result<OpenedObjectIdentity> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };

    let mut information = unsafe { std::mem::zeroed::<BY_HANDLE_FILE_INFORMATION>() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut information) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(OpenedObjectIdentity {
        volume: u64::from(information.dwVolumeSerialNumber),
        file: (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow),
        links: u64::from(information.nNumberOfLinks),
    })
}

#[cfg(not(any(unix, target_os = "windows")))]
fn opened_object_identity(file: &File) -> io::Result<OpenedObjectIdentity> {
    let metadata = file.metadata()?;
    Ok(OpenedObjectIdentity {
        volume: 0,
        file: metadata.len(),
        links: 1,
    })
}

fn retain_path_directory_chain(path: &Path) -> io::Result<RetainedAutomationDirectoryChain> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing parent directory"))?;
    let is_mailbox_child = parent
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| matches!(name, REQUESTS_DIR | RESPONSES_DIR));
    if is_mailbox_child {
        let automation_dir = parent.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "unsafe_automation_directory",
            )
        })?;
        let root = retain_automation_directory_no_follow(automation_dir)?;
        let child = retain_automation_directory_no_follow(parent)?;
        root.verify_current()?;
        Ok(RetainedAutomationDirectoryChain {
            root,
            child: Some(child),
        })
    } else {
        Ok(RetainedAutomationDirectoryChain {
            root: retain_automation_directory_no_follow(parent)?,
            child: None,
        })
    }
}

fn retain_atomic_write_directories(
    destination: &Path,
    staging: &Path,
) -> io::Result<AtomicWriteDirectories> {
    let staging_chain = retain_path_directory_chain(staging)?;
    let destination_chain = if destination.parent() == staging.parent() {
        None
    } else {
        Some(retain_path_directory_chain(destination)?)
    };
    let directories = AtomicWriteDirectories {
        staging: staging_chain,
        destination: destination_chain,
    };
    directories.verify_current()?;
    Ok(directories)
}

fn create_or_retain_automation_directory(path: &Path) -> io::Result<RetainedAutomationDirectory> {
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    retain_automation_directory_no_follow(path)
}

fn prepare_automation_mailbox_tree(automation_dir: &Path, reset: bool) -> io::Result<()> {
    let automation = create_or_retain_automation_directory(automation_dir)?;
    let child_paths = [
        automation_dir.join(REQUESTS_DIR),
        automation_dir.join(RESPONSES_DIR),
    ];
    if reset {
        for child_path in &child_paths {
            match fs::symlink_metadata(child_path) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
                Ok(_) => {
                    let child = retain_automation_directory_no_follow(child_path)?;
                    child.verify_current()?;
                    drop(child);
                    retry_remove_dir_all(child_path)?;
                }
            }
        }
    }
    let requests = create_or_retain_automation_directory(&child_paths[0])?;
    let responses = create_or_retain_automation_directory(&child_paths[1])?;
    automation.verify_current()?;
    requests.verify_current()?;
    responses.verify_current()?;
    Ok(())
}

fn read_session_file_with_retry(path: &Path) -> io::Result<String> {
    let mut last_not_found = None;
    for attempt in 0..SESSION_READ_RETRY_COUNT {
        match read_bounded_regular_file(path, MAX_SESSION_FILE_BYTES) {
            Ok(raw) => return Ok(raw),
            Err(e)
                if e.kind() == io::ErrorKind::NotFound
                    && attempt + 1 < SESSION_READ_RETRY_COUNT =>
            {
                last_not_found = Some(e);
                std::thread::sleep(Duration::from_millis(SESSION_READ_RETRY_DELAY_MS));
            }
            Err(e) => return Err(e),
        }
    }
    Err(last_not_found.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("session file not found: {}", path.display()),
        )
    }))
}

fn read_bounded_regular_file(path: &Path, limit: usize) -> io::Result<String> {
    read_bounded_regular_file_with_hook(path, limit, || {})
}

fn read_bounded_regular_file_with_hook(
    path: &Path,
    limit: usize,
    after_open: impl FnOnce(),
) -> io::Result<String> {
    let directories = retain_path_directory_chain(path)?;
    let limit_code = if limit == MAX_SESSION_FILE_BYTES {
        "session_file_too_large"
    } else if limit == MAX_REQUEST_FILE_BYTES {
        "request_too_large"
    } else {
        "response_too_large"
    };
    let mut file = open_regular_file_no_follow(path)?;
    let metadata = validate_opened_regular_file(&file)?;
    let length = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
    if length > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            limit_code,
        ));
    }
    after_open();
    let mut bytes = Vec::with_capacity(limit.min(64 * 1024).saturating_add(1));
    Read::by_ref(&mut file)
        .take(limit.saturating_add(1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(io::Error::new(io::ErrorKind::InvalidData, limit_code));
    }
    validate_opened_regular_file(&file)?;
    directories.verify_current()?;
    let reopened = open_regular_file_no_follow(path)?;
    if opened_object_identity(&reopened)? != opened_object_identity(&file)? {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unsafe_automation_file",
        ));
    }
    directories.verify_current()?;
    String::from_utf8(bytes).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid_utf8"))
}

fn write_json_atomic<T: Serialize>(
    path: &Path,
    value: &T,
    replace: bool,
    before_commit: impl FnOnce() -> io::Result<bool>,
) -> io::Result<bool> {
    let tmp_path = atomic_write_temp_path(path);
    write_json_atomic_with_temp_path_and_precommit(
        path,
        value,
        replace,
        &tmp_path,
        atomic_commit_opened_temp,
        before_commit,
    )
}

#[cfg(test)]
fn write_json_atomic_with_temp_path<T, F>(
    path: &Path,
    value: &T,
    replace: bool,
    tmp_path: &Path,
    replace_existing: F,
) -> io::Result<()>
where
    T: Serialize,
    F: FnOnce(&Path, &Path) -> io::Result<()>,
{
    let committed = write_json_atomic_with_temp_path_and_precommit(
        path,
        value,
        replace,
        tmp_path,
        move |temp_file,
              source,
              destination,
              should_replace,
              directories,
              generations,
              commit_decision| {
            if should_replace {
                verify_atomic_commit_paths(
                    temp_file,
                    source,
                    destination,
                    directories,
                    generations,
                )?;
                if !commit_decision()? {
                    return Ok(false);
                }
                replace_existing(source, destination)?;
                Ok(true)
            } else {
                atomic_commit_opened_temp(
                    temp_file,
                    source,
                    destination,
                    false,
                    directories,
                    generations,
                    commit_decision,
                )
            }
        },
        || Ok(true),
    )?;
    if committed {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "atomic publication cancelled",
        ))
    }
}

fn write_json_atomic_with_temp_path_and_precommit<T, F, C>(
    path: &Path,
    value: &T,
    replace: bool,
    tmp_path: &Path,
    commit: F,
    before_commit: C,
) -> io::Result<bool>
where
    T: Serialize,
    F: FnOnce(
        &File,
        &Path,
        &Path,
        bool,
        &AtomicWriteDirectories,
        AtomicCommitGenerations,
        &mut dyn FnMut() -> io::Result<bool>,
    ) -> io::Result<bool>,
    C: FnOnce() -> io::Result<bool>,
{
    let limit = if path.file_name().and_then(|name| name.to_str()) == Some(SESSION_FILE) {
        MAX_SESSION_FILE_BYTES
    } else if path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        == Some(RESPONSES_DIR)
    {
        MAX_RESPONSE_JSON_BYTES
    } else {
        MAX_REQUEST_FILE_BYTES
    };
    let encoded = serde_json::to_vec(value)?;
    if encoded.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            if limit == MAX_SESSION_FILE_BYTES {
                "session_file_too_large"
            } else if limit == MAX_RESPONSE_JSON_BYTES {
                "response_too_large"
            } else {
                "request_too_large"
            },
        ));
    }
    let directories = retain_atomic_write_directories(path, tmp_path)?;

    if !replace {
        ensure_path_absent_no_follow(path)?;
    }

    let mut file = open_new_atomic_temp_file_no_follow(tmp_path)?;
    file.write_all(&encoded)?;
    file.sync_all()?;
    validate_opened_regular_file(&file)?;
    let temp_identity = opened_object_identity(&file)?;
    if temp_identity.links != 1 {
        drop(file);
        let _ = retry_remove_file_if_same_object(tmp_path, temp_identity);
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unsafe_automation_file",
        ));
    }

    let destination = if replace {
        match open_regular_file_no_follow(path) {
            Ok(file) => Some((opened_object_identity(&file)?, file)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => {
                drop(file);
                let _ = retry_remove_file_if_same_object(tmp_path, temp_identity);
                return Err(error);
            }
        }
    } else {
        None
    };

    let verify_inputs = || {
        directories.verify_current()?;
        let current_temp = open_regular_file_no_follow(tmp_path)?;
        if opened_object_identity(&current_temp)? != temp_identity {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "unsafe_automation_file",
            ));
        }
        if let Some((expected, _retained)) = destination.as_ref() {
            let current = open_regular_file_no_follow(path)?;
            if opened_object_identity(&current)? != *expected {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "unsafe_automation_file",
                ));
            }
        } else {
            ensure_path_absent_no_follow(path)?;
        }
        directories.verify_current()?;
        Ok(())
    };

    let generations = AtomicCommitGenerations {
        source: temp_identity,
        destination: destination.as_ref().map(|(identity, _)| *identity),
    };
    let mut before_commit = Some(before_commit);
    let mut commit_decision = || {
        let decision = before_commit
            .take()
            .ok_or_else(|| io::Error::other("atomic commit decision already consumed"))?;
        decision()
    };
    let result = (|| {
        // Re-prove every retained input, then transfer the final decision into
        // the native commit closure. That closure performs its own last path
        // generation check and invokes the decision only at the atomic boundary.
        verify_inputs()?;
        if !commit(
            &file,
            tmp_path,
            path,
            replace,
            &directories,
            generations,
            &mut commit_decision,
        )? {
            return Ok(false);
        }
        validate_opened_regular_file(&file)?;
        let published = open_regular_file_no_follow(path)?;
        if !opened_object_identity(&published)?.same_object(temp_identity) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "unsafe_automation_file",
            ));
        }
        directories.sync_best_effort();
        Ok(true)
    })();
    drop(destination);
    drop(file);
    if !matches!(result, Ok(true)) {
        let _ = retry_remove_file_if_same_object(tmp_path, temp_identity);
    }
    result
}

fn atomic_write_temp_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let staging_parent = match parent.file_name().and_then(|name| name.to_str()) {
        Some(REQUESTS_DIR | RESPONSES_DIR) => parent.parent().unwrap_or(parent),
        _ => parent,
    };
    staging_parent.join(format!(
        "~ac-ui-{file_name}-{}.tmp",
        Uuid::new_v4().simple()
    ))
}

fn ensure_path_absent_no_follow(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("destination already exists: {}", path.display()),
        )),
    }
}

fn apply_no_follow_open_flags(options: &mut OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        options
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
}

fn open_regular_file_no_follow(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    apply_no_follow_open_flags(&mut options);
    let file = options.open(path)?;
    validate_opened_regular_file(&file)?;
    Ok(file)
}

#[cfg(not(target_os = "windows"))]
fn open_new_regular_file_no_follow(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    apply_no_follow_open_flags(&mut options);
    let file = options.open(path)?;
    validate_opened_regular_file(&file)?;
    Ok(file)
}

#[cfg(target_os = "windows")]
fn open_new_atomic_temp_file_no_follow(path: &Path) -> io::Result<File> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::FromRawHandle;

    const DELETE_ACCESS: u32 = 0x0001_0000;
    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const CREATE_NEW: u32 = 1;
    const FILE_ATTRIBUTE_NORMAL: u32 = 0x0000_0080;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const INVALID_HANDLE_VALUE: isize = -1;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateFileW(
            file_name: *const u16,
            desired_access: u32,
            share_mode: u32,
            security_attributes: *const std::ffi::c_void,
            creation_disposition: u32,
            flags_and_attributes: u32,
            template_file: *mut std::ffi::c_void,
        ) -> *mut std::ffi::c_void;
    }

    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ | GENERIC_WRITE | DELETE_ACCESS,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if handle as isize == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_handle(handle) };
    validate_opened_regular_file(&file)?;
    Ok(file)
}

#[cfg(not(target_os = "windows"))]
fn open_new_atomic_temp_file_no_follow(path: &Path) -> io::Result<File> {
    open_new_regular_file_no_follow(path)
}

fn validate_opened_regular_file(file: &File) -> io::Result<fs::Metadata> {
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unsafe_automation_file",
        ));
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "unsafe_automation_file",
            ));
        }
    }
    Ok(metadata)
}

fn verify_atomic_commit_paths(
    source: &File,
    source_path: &Path,
    destination: &Path,
    directories: &AtomicWriteDirectories,
    generations: AtomicCommitGenerations,
) -> io::Result<()> {
    directories.verify_current()?;
    validate_opened_regular_file(source)?;
    if !opened_object_identity(source)?.same_object(generations.source) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unsafe_automation_file",
        ));
    }
    let current_source = open_regular_file_no_follow(source_path)?;
    if !opened_object_identity(&current_source)?.same_object(generations.source) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unsafe_automation_file",
        ));
    }
    match generations.destination {
        Some(expected) => {
            let current_destination = open_regular_file_no_follow(destination)?;
            if !opened_object_identity(&current_destination)?.same_object(expected) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "unsafe_automation_file",
                ));
            }
        }
        None => ensure_path_absent_no_follow(destination)?,
    }
    directories.verify_current()
}

fn retry_remove_file_if_same_object(
    path: &Path,
    expected: OpenedObjectIdentity,
) -> io::Result<()> {
    let current = match open_regular_file_no_follow(path) {
        Ok(current) => current,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !opened_object_identity(&current)?.same_object(expected) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unsafe_automation_file",
        ));
    }
    drop(current);
    retry_remove_file(path)
}

#[cfg(target_os = "windows")]
fn atomic_commit_opened_temp(
    source: &File,
    source_path: &Path,
    destination: &Path,
    replace: bool,
    directories: &AtomicWriteDirectories,
    generations: AtomicCommitGenerations,
    commit_decision: &mut dyn FnMut() -> io::Result<bool>,
) -> io::Result<bool> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileRenameInfo, FileRenameInfoEx, SetFileInformationByHandle, FILE_RENAME_INFO,
    };

    const FILE_RENAME_FLAG_REPLACE_IF_EXISTS: u32 = 0x0000_0001;
    const FILE_RENAME_FLAG_POSIX_SEMANTICS: u32 = 0x0000_0002;

    let destination_wide = destination.as_os_str().encode_wide().collect::<Vec<_>>();
    let file_name_offset = std::mem::offset_of!(FILE_RENAME_INFO, FileName);
    let buffer_size = std::mem::size_of::<FILE_RENAME_INFO>()
        .checked_add(
            destination_wide
                .len()
                .saturating_sub(1)
                .saturating_mul(std::mem::size_of::<u16>()),
        )
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "destination too long"))?;
    let buffer_size_u32 = u32::try_from(buffer_size)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "destination too long"))?;
    let file_name_length = u32::try_from(destination_wide.len().saturating_mul(2))
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "destination too long"))?;
    let mut buffer = vec![0_u8; buffer_size];
    let (information_class, flags) = if replace {
        (
            FileRenameInfoEx,
            FILE_RENAME_FLAG_REPLACE_IF_EXISTS | FILE_RENAME_FLAG_POSIX_SEMANTICS,
        )
    } else {
        (FileRenameInfo, 0)
    };
    unsafe {
        let information = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
        information.cast::<u32>().write(flags);
        (*information).RootDirectory = std::ptr::null_mut();
        (*information).FileNameLength = file_name_length;
        std::ptr::copy_nonoverlapping(
            destination_wide.as_ptr(),
            buffer.as_mut_ptr().add(file_name_offset).cast::<u16>(),
            destination_wide.len(),
        );
    }
    verify_atomic_commit_paths(
        source,
        source_path,
        destination,
        directories,
        generations,
    )?;
    if !commit_decision()? {
        return Ok(false);
    }
    let renamed = unsafe {
        SetFileInformationByHandle(
            source.as_raw_handle() as _,
            information_class,
            buffer.as_ptr().cast(),
            buffer_size_u32,
        )
    };
    if renamed == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(true)
    }
}

#[cfg(unix)]
fn atomic_commit_opened_temp(
    source: &File,
    source_path: &Path,
    destination: &Path,
    replace: bool,
    directories: &AtomicWriteDirectories,
    generations: AtomicCommitGenerations,
    commit_decision: &mut dyn FnMut() -> io::Result<bool>,
) -> io::Result<bool> {
    let parent = destination
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing parent directory"))?;
    let anchor = parent.join(format!(
        "~ac-ui-commit-{}.tmp",
        Uuid::new_v4().simple()
    ));
    // Link to an unpublished name, then prove that name is the retained source
    // generation. A source rebind before this link fails the proof; a rebind
    // afterward cannot change the inode pinned by the commit anchor.
    fs::hard_link(source_path, &anchor)?;
    let anchor_file = match open_regular_file_no_follow(&anchor) {
        Ok(anchor_file) => anchor_file,
        Err(error) => {
            let _ = retry_remove_file(&anchor);
            return Err(error);
        }
    };
    if !opened_object_identity(&anchor_file)?.same_object(generations.source) {
        drop(anchor_file);
        let _ = retry_remove_file(&anchor);
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unsafe_automation_file",
        ));
    }
    drop(anchor_file);

    let result = (|| {
        verify_atomic_commit_paths(
            source,
            source_path,
            destination,
            directories,
            generations,
        )?;
        if !commit_decision()? {
            return Ok(false);
        }
        if replace {
            fs::rename(&anchor, destination)?;
        } else {
            fs::hard_link(&anchor, destination)?;
            let _ = retry_remove_file(&anchor);
        }
        Ok(true)
    })();
    if !matches!(result, Ok(true)) {
        let _ = retry_remove_file(&anchor);
    } else {
        let _ = retry_remove_file_if_same_object(source_path, generations.source);
    }
    result
}

#[cfg(not(any(unix, target_os = "windows")))]
fn atomic_commit_opened_temp(
    _source: &File,
    _source_path: &Path,
    _destination: &Path,
    _replace: bool,
    _directories: &AtomicWriteDirectories,
    _generations: AtomicCommitGenerations,
    _commit_decision: &mut dyn FnMut() -> io::Result<bool>,
) -> io::Result<bool> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "atomic opened-file commit is unavailable",
    ))
}

fn retry_rename(from: &Path, to: &Path) -> io::Result<()> {
    retry_fs(|| fs::rename(from, to))
}

fn retry_remove_file(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
        Ok(_) => {}
    }
    retry_fs(|| fs::remove_file(path))
}

fn retry_remove_dir_all(path: &Path) -> io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    retry_fs(|| fs::remove_dir_all(path))
}

fn retry_fs(mut op: impl FnMut() -> io::Result<()>) -> io::Result<()> {
    let mut last_err = None;
    for attempt in 0..FS_RETRY_COUNT {
        match op() {
            Ok(()) => return Ok(()),
            Err(e) if is_transient_fs_error(&e) && attempt + 1 < FS_RETRY_COUNT => {
                last_err = Some(e);
                std::thread::sleep(Duration::from_millis(FS_RETRY_DELAY_MS));
            }
            Err(e) => return Err(e),
        }
    }
    Err(last_err.unwrap_or_else(|| io::Error::other("filesystem retry failed")))
}

fn is_transient_fs_error(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::PermissionDenied || matches!(e.raw_os_error(), Some(32) | Some(33))
}

fn cleanup_stale_automation_files(automation_dir: &Path) -> io::Result<()> {
    let requests = automation_dir.join(REQUESTS_DIR);
    let responses = automation_dir.join(RESPONSES_DIR);
    if requests.exists() {
        retry_remove_dir_all(&requests)?;
    }
    if responses.exists() {
        retry_remove_dir_all(&responses)?;
    }
    Ok(())
}

fn available_window_labels(app: &AppHandle) -> Vec<String> {
    let mut labels: Vec<String> = app.webview_windows().keys().cloned().collect();
    labels.sort();
    labels
}

fn current_exe_path_string() -> String {
    std::env::current_exe()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| String::new())
}

fn paths_equal_for_compare(a: &Path, b: &Path) -> bool {
    let a = canonical_for_compare(a);
    let b = canonical_for_compare(b);
    #[cfg(target_os = "windows")]
    {
        a.to_string_lossy()
            .eq_ignore_ascii_case(&b.to_string_lossy())
    }
    #[cfg(not(target_os = "windows"))]
    {
        a == b
    }
}

fn canonical_for_compare(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[cfg(target_os = "windows")]
struct OwnedLiveProcessHandle(windows_sys::Win32::Foundation::HANDLE);

#[cfg(target_os = "windows")]
impl Drop for OwnedLiveProcessHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(target_os = "windows")]
impl LiveProcessIdentityProbe for OsLiveProcessIdentityProbe {
    fn probe(&self, pid: u32) -> Option<LiveProcessIdentity> {
        use windows_sys::Win32::Foundation::{FILETIME, WAIT_TIMEOUT};
        use windows_sys::Win32::System::Threading::{
            GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, WaitForSingleObject,
            PROCESS_QUERY_LIMITED_INFORMATION,
        };

        const SYNCHRONIZE_ACCESS: u32 = 0x0010_0000;
        const WINDOWS_TO_UNIX_EPOCH_100NS: u64 = 116_444_736_000_000_000;
        const TICKS_PER_MILLISECOND: u64 = 10_000;

        unsafe {
            let handle = OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE_ACCESS,
                0,
                pid,
            );
            if handle.is_null() {
                return None;
            }
            let handle = OwnedLiveProcessHandle(handle);
            if WaitForSingleObject(handle.0, 0) != WAIT_TIMEOUT {
                return None;
            }

            let mut executable_buf = vec![0u16; 32_768];
            let mut executable_len = executable_buf.len() as u32;
            if QueryFullProcessImageNameW(
                handle.0,
                0,
                executable_buf.as_mut_ptr(),
                &mut executable_len,
            ) == 0
                || executable_len == 0
            {
                return None;
            }

            let mut creation = FILETIME {
                dwLowDateTime: 0,
                dwHighDateTime: 0,
            };
            let mut exit = FILETIME {
                dwLowDateTime: 0,
                dwHighDateTime: 0,
            };
            let mut kernel = FILETIME {
                dwLowDateTime: 0,
                dwHighDateTime: 0,
            };
            let mut user = FILETIME {
                dwLowDateTime: 0,
                dwHighDateTime: 0,
            };
            if GetProcessTimes(
                handle.0,
                &mut creation,
                &mut exit,
                &mut kernel,
                &mut user,
            ) == 0
            {
                return None;
            }
            if WaitForSingleObject(handle.0, 0) != WAIT_TIMEOUT {
                return None;
            }

            let creation_ticks =
                (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime);
            let unix_ticks = creation_ticks.checked_sub(WINDOWS_TO_UNIX_EPOCH_100NS)?;
            let unix_ms = unix_ticks / TICKS_PER_MILLISECOND;
            let started_at_unix_ms = i64::try_from(unix_ms).ok()?;
            if started_at_unix_ms <= 0 {
                return None;
            }

            Some(LiveProcessIdentity {
                executable: PathBuf::from(String::from_utf16_lossy(
                    &executable_buf[..executable_len as usize],
                )),
                started_at_unix_ms,
            })
        }
    }
}

#[cfg(not(target_os = "windows"))]
impl LiveProcessIdentityProbe for OsLiveProcessIdentityProbe {
    fn probe(&self, _pid: u32) -> Option<LiveProcessIdentity> {
        None
    }
}

#[cfg(target_os = "windows")]
pub fn pid_is_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_ACCESS_DENIED, FALSE, STILL_ACTIVE,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid);
        if handle.is_null() {
            return GetLastError() == ERROR_ACCESS_DENIED;
        }
        let mut code: u32 = 0;
        let got_code = GetExitCodeProcess(handle, &mut code);
        CloseHandle(handle);
        got_code != 0 && code == STILL_ACTIVE as u32
    }
}

#[cfg(not(target_os = "windows"))]
pub fn pid_is_alive(_pid: u32) -> bool {
    true
}

#[derive(Debug)]
struct RequestFile {
    path: PathBuf,
    request_id: String,
    kind: RequestFileKind,
}

#[derive(Debug, Clone, Copy)]
enum RequestFileKind {
    Ready,
    Inflight,
}

impl RequestFile {
    fn from_path(path: &Path) -> Option<Self> {
        let name = path.file_name()?.to_str()?;
        if name.ends_with(".tmp") {
            return None;
        }

        let (request_id, kind) = if let Some(id) = name.strip_suffix(".inflight.json") {
            (id, RequestFileKind::Inflight)
        } else {
            (name.strip_suffix(".json")?, RequestFileKind::Ready)
        };

        if Uuid::parse_str(request_id).is_err() {
            return None;
        }

        Some(Self {
            path: path.to_path_buf(),
            request_id: request_id.to_string(),
            kind,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::settings::{AppSettings, ResourceWatchdogAction};
    use crate::resource_monitor::registry::{
        ProcessTreeBackend, ResourceError, ResourceLaunchRegistration,
    };
    use crate::resource_monitor::types::{
        ObservedProcess, ObservedProcessTree, ProcessIdentity, ProcessMemory,
        ResourceLaunchMetadata, TerminateOutcome,
    };
    use crate::session::manager::SessionManager;
    use crate::session::selection::{
        CriticalAdmissionKind, SelectionCoordinator, WatchdogKillOutcome,
    };
    use crate::web::broadcast::WsBroadcaster;
    use crate::DetachedSessionsState;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio_util::sync::CancellationToken;

    struct TestAutomationConfigWitness {
        canonical_path: PathBuf,
    }

    impl AutomationConfigWitness for TestAutomationConfigWitness {
        fn canonical_path(&self) -> &Path {
            &self.canonical_path
        }

        fn object_parts(&self) -> (u64, u64) {
            (1, 1)
        }

        fn verify_current(&self) -> bool {
            self.canonical_path.is_dir()
        }
    }

    struct TestCurrentProcessProbe {
        executable: PathBuf,
    }

    impl LiveProcessIdentityProbe for TestCurrentProcessProbe {
        fn probe(&self, pid: u32) -> Option<LiveProcessIdentity> {
            (pid == std::process::id()).then(|| LiveProcessIdentity {
                executable: self.executable.clone(),
                started_at_unix_ms: 1,
            })
        }
    }

    struct UnavailableProcessProbe;

    impl LiveProcessIdentityProbe for UnavailableProcessProbe {
        fn probe(&self, _pid: u32) -> Option<LiveProcessIdentity> {
            None
        }
    }

    struct FixedProcessProbe {
        pid: u32,
        executable: PathBuf,
        started_at_unix_ms: i64,
    }

    impl LiveProcessIdentityProbe for FixedProcessProbe {
        fn probe(&self, pid: u32) -> Option<LiveProcessIdentity> {
            (pid == self.pid).then(|| LiveProcessIdentity {
                executable: self.executable.clone(),
                started_at_unix_ms: self.started_at_unix_ms,
            })
        }
    }

    struct PublishSessionAfterRead {
        path: PathBuf,
        session: UiAutomationSession,
    }

    impl SessionLoadTestHooks for PublishSessionAfterRead {
        fn after_session_bytes_read(&self) {
            write_json_atomic_replace(&self.path, &self.session)
                .expect("publish replacement singleton session");
        }
    }

    struct PanicAfterDetachedGuard;

    impl TerminalCaptureHooks for PanicAfterDetachedGuard {
        fn after_detached_guard_acquired(&self) {
            panic!("injected detached capture panic");
        }
    }

    fn test_automation_state(config_dir: &Path) -> UiAutomationState {
        fs::create_dir_all(config_dir).unwrap();
        let canonical_path = fs::canonicalize(config_dir).unwrap();
        let witness: Arc<dyn AutomationConfigWitness> = Arc::new(TestAutomationConfigWitness {
            canonical_path: canonical_path.clone(),
        });
        let executable = canonical_for_compare(&std::env::current_exe().unwrap());
        UiAutomationState::new_with_process_probe(
            true,
            canonical_path,
            Some(witness),
            &TestCurrentProcessProbe { executable },
        )
        .unwrap()
    }

    #[cfg(target_os = "windows")]
    fn create_test_reparse(link: &Path, target: &Path) {
        let output = std::process::Command::new("cmd")
            .args(["/D", "/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .expect("run mklink");
        assert!(
            output.status.success(),
            "create junction failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(unix)]
    fn create_test_reparse(link: &Path, target: &Path) {
        std::os::unix::fs::symlink(target, link).expect("create symlink");
    }

    #[cfg(target_os = "windows")]
    fn remove_test_reparse(link: &Path) {
        fs::remove_dir(link).expect("remove junction");
    }

    #[cfg(unix)]
    fn remove_test_reparse(link: &Path) {
        fs::remove_file(link).expect("remove symlink");
    }

    #[test]
    fn enabled_state_process_probe_failure_is_filesystem_silent_and_stale() {
        let tmp = tempfile::tempdir().unwrap();
        let canonical_path = fs::canonicalize(tmp.path()).unwrap();
        let witness: Arc<dyn AutomationConfigWitness> = Arc::new(TestAutomationConfigWitness {
            canonical_path: canonical_path.clone(),
        });

        let result = UiAutomationState::new_with_process_probe(
            true,
            canonical_path,
            Some(witness),
            &UnavailableProcessProbe,
        );

        assert!(matches!(result, Err("automation_session_stale")));
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 0);
    }

    #[test]
    fn ownership_barrier_observes_the_published_automation_singleton() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_automation_state(tmp.path());
        let hook_fired = AtomicBool::new(false);

        state
            .publish_owned_artifacts(|| {
                let raw = fs::read_to_string(&state.inner.session_path)
                    .expect("session must exist before the ownership barrier");
                let session: UiAutomationSession =
                    serde_json::from_str(&raw).expect("published session schema");
                assert_eq!(session.instance_id, state.inner.instance_id);
                hook_fired.store(true, Ordering::SeqCst);
            })
            .unwrap();

        assert!(hook_fired.load(Ordering::SeqCst));
    }

    #[test]
    fn managed_automation_types_are_send_sync_static() {
        fn assert_send_sync_static<T: Send + Sync + 'static>() {}
        assert_send_sync_static::<Arc<dyn AutomationConfigWitness>>();
        assert_send_sync_static::<Arc<dyn InstanceIsolationTestHooks>>();
        assert_send_sync_static::<UiCliDispatchContext>();
        assert_send_sync_static::<UiAutomationState>();
        assert_send_sync_static::<TerminalTaskControl>();
        assert_send_sync_static::<Arc<dyn TerminalCaptureHooks>>();
    }

    #[test]
    fn detached_capture_panic_drops_guard_normally_without_poisoning_consumers() {
        let session_id = Uuid::new_v4();
        let detached_sessions: DetachedSessionsState =
            Arc::new(Mutex::new(HashSet::from([session_id])));
        let hooks = PanicAfterDetachedGuard;

        let result = with_detached_membership_guard_caught(&detached_sessions, |guard| {
            assert!(guard.contains(&session_id));
            hooks.after_detached_guard_acquired();
            Ok(())
        });

        assert!(matches!(
            result,
            Err(TerminalSnapshotTaskError {
                code: "terminal_snapshot_unavailable",
                ..
            })
        ));
        assert!(!detached_sessions.is_poisoned());
        let mut consumer = detached_sessions.lock().unwrap();
        assert!(consumer.remove(&session_id));
        assert!(consumer.insert(session_id));
        assert!(consumer.contains(&session_id));
    }

    #[test]
    fn window_inventory_is_complete_or_overflow_and_recovers_without_stale_count() {
        let mut inventory = WindowInventory::initial();
        let labels_32 = (0..32).map(|index| format!("window-{index:02}")).collect::<Vec<_>>();
        assert!(inventory.sync(labels_32.clone()));
        assert_eq!(inventory.status, WindowInventoryStatus::Ready);
        assert_eq!(inventory.observed_count(), 32);
        assert_eq!(inventory.snapshot().1, labels_32);

        let generation_before_overflow = inventory.entries["window-00"].generation;
        let labels_33 = (0..33).map(|index| format!("window-{index:02}")).collect::<Vec<_>>();
        assert!(inventory.sync(labels_33));
        assert_eq!(inventory.status, WindowInventoryStatus::Overflow);
        assert_eq!(inventory.observed_count(), 33);
        assert_eq!(inventory.snapshot().1, Vec::<String>::new());
        assert!(inventory.entries["window-00"].generation > generation_before_overflow);

        let swapped_33 = (1..34).map(|index| format!("window-{index:02}")).collect::<Vec<_>>();
        assert!(inventory.sync(swapped_33));
        assert_eq!(inventory.observed_count(), 33);
        assert!(!inventory.entries.contains_key("window-00"));
        assert!(inventory.entries.contains_key("window-33"));

        let recovered = (2..34).map(|index| format!("window-{index:02}")).collect::<Vec<_>>();
        assert!(inventory.sync(recovered.clone()));
        assert_eq!(inventory.status, WindowInventoryStatus::Ready);
        assert_eq!(inventory.snapshot().1, recovered);
    }

    #[test]
    fn session_inventory_contract_rejects_partial_unsorted_and_mismatched_shapes() {
        let mut session = UiAutomationSession {
            schema_version: PROTOCOL_SCHEMA_VERSION,
            instance_id: Uuid::new_v4().to_string(),
            pid: 1,
            token: Uuid::new_v4().to_string(),
            exe_path: "fixture.exe".to_string(),
            config_dir: "fixture-config".to_string(),
            window_inventory: UiAutomationWindowInventory {
                status: WindowInventoryStatus::Ready,
                observed_count: 2,
                limit: MAX_REGISTERED_WINDOWS as u32,
            },
            window_labels: vec!["main".to_string(), "resource-monitor".to_string()],
            ready_window_labels: vec!["main".to_string()],
            started_at_unix_ms: 1,
        };
        assert!(session_inventory_is_coherent(&session));

        session.window_labels.swap(0, 1);
        assert!(!session_inventory_is_coherent(&session));
        session.window_labels.sort();
        session.window_inventory.observed_count = 1;
        assert!(!session_inventory_is_coherent(&session));

        session.window_inventory = UiAutomationWindowInventory {
            status: WindowInventoryStatus::Overflow,
            observed_count: 33,
            limit: MAX_REGISTERED_WINDOWS as u32,
        };
        assert!(!session_inventory_is_coherent(&session));
        session.window_labels.clear();
        session.ready_window_labels.clear();
        assert!(session_inventory_is_coherent(&session));
    }

    #[test]
    fn session_post_read_race_preserves_replacement_and_next_load_reaches_it() {
        let temp = tempfile::tempdir().unwrap();
        let config_dir = fs::canonicalize(temp.path()).unwrap();
        let session_path = config_dir.join(UI_AUTOMATION_DIR).join(SESSION_FILE);
        fs::create_dir(session_path.parent().unwrap()).unwrap();
        let executable = canonical_for_compare(&std::env::current_exe().unwrap());
        let pid = 42_424;
        let make_session =
            |instance_id: &str, token: &str, started_at_unix_ms| UiAutomationSession {
                schema_version: PROTOCOL_SCHEMA_VERSION,
                instance_id: instance_id.to_string(),
                pid,
                token: token.to_string(),
                exe_path: executable.to_string_lossy().into_owned(),
                config_dir: config_dir.to_string_lossy().into_owned(),
                window_inventory: UiAutomationWindowInventory {
                    status: WindowInventoryStatus::Ready,
                    observed_count: 1,
                    limit: MAX_REGISTERED_WINDOWS as u32,
                },
                window_labels: vec!["main".to_string()],
                ready_window_labels: vec!["main".to_string()],
                started_at_unix_ms,
            };
        let stale_f1 = make_session(
            "00000000-0000-4000-8000-000000000151",
            "00000000-0000-4000-8000-000000000152",
            111,
        );
        let live_f2 = make_session(
            "00000000-0000-4000-8000-000000000153",
            "00000000-0000-4000-8000-000000000154",
            222,
        );
        write_json_atomic_replace(&session_path, &stale_f1).unwrap();
        let expected_f2_bytes = serde_json::to_vec(&live_f2).unwrap();
        let hooks = PublishSessionAfterRead {
            path: session_path.clone(),
            session: live_f2.clone(),
        };
        let probe = FixedProcessProbe {
            pid,
            executable,
            started_at_unix_ms: 222,
        };
        let witness: Arc<dyn AutomationConfigWitness> = Arc::new(TestAutomationConfigWitness {
            canonical_path: config_dir,
        });
        let context = UiCliDispatchContext::new(witness);

        let stale = load_session_for_cli_with_process_probe(&context, "main", &probe, Some(&hooks))
            .unwrap_err();
        assert_eq!(stale["error"], "automation_session_stale");
        assert_eq!(fs::read(&session_path).unwrap(), expected_f2_bytes);

        let loaded =
            load_session_for_cli_with_process_probe(&context, "main", &probe, None).unwrap();
        assert_eq!(loaded.instance_id, live_f2.instance_id);
        assert_eq!(fs::read(&session_path).unwrap(), expected_f2_bytes);
    }

    #[test]
    fn preflight_error_compact_lines_are_byte_exact() {
        assert_eq!(
            format!("{}\n", refusing_non_testeable_binary_json()),
            concat!(
                "{\"ok\":false,\"error\":\"refusing_non_testeable_binary\",",
                "\"message\":\"UI automation is only available from ",
                "agentscommander_testeable.exe.\"}\n"
            )
        );
        assert_eq!(
            format!("{}\n", automation_session_missing_json()),
            concat!(
                "{\"ok\":false,\"error\":\"automation_session_missing\",",
                "\"message\":\"No UI automation session file exists for this ",
                "testable binary.\"}\n"
            )
        );
        assert_eq!(
            format!(
                "{}\n",
                compact_preflight_json(&automation_session_stale_error())
            ),
            concat!(
                "{\"ok\":false,\"error\":\"automation_session_stale\",",
                "\"message\":\"The UI automation session no longer identifies ",
                "the live GUI process.\"}\n"
            )
        );
    }

    #[test]
    fn list_output_compact_line_is_byte_exact() {
        let prefix = "AC_UI_PREFIX_00000000-0000-4000-8000-000000000155";
        let response = UiAutomationResponse {
            ok: true,
            request_id: "00000000-0000-4000-8000-000000000155".to_string(),
            window: "main".to_string(),
            action: UiAutomationAction::List,
            selector: String::new(),
            target: None,
            error: None,
            message: None,
            available: None,
            diagnostics: None,
            available_windows: None,
            timeout_ms: None,
            phase: None,
            active_test_id: Value::Null,
            filters: Some(UiListFilters {
                prefix: Some(prefix.to_string()),
                role: None,
            }),
            targets: Some(Vec::new()),
            matched_count: Some(0),
            matched_count_exact: Some(true),
            returned_count: Some(0),
            limit: Some(MAX_LIST_RETURN_TARGETS),
            truncated: Some(false),
            scan: Some(UiListScan {
                elements: 0,
                element_limit: MAX_LIST_SCAN_ELEMENTS,
                targets: 0,
                target_limit: MAX_LIST_SCAN_TARGETS,
                open_roots: 1,
                open_root_limit: MAX_LIST_OPEN_ROOTS,
                truncated: false,
            }),
            terminal_snapshot: None,
        };
        let output = ui_list_output(&response, Some(prefix), None).unwrap();
        let line = format!("{}\n", serde_json::to_string(&output).unwrap());

        assert_eq!(
            line,
            concat!(
                "{\"ok\":true,\"requestId\":\"00000000-0000-4000-8000-000000000155\",",
                "\"window\":\"main\",\"action\":\"list\",\"filters\":{\"prefix\":",
                "\"AC_UI_PREFIX_00000000-0000-4000-8000-000000000155\",\"role\":null},",
                "\"targets\":[],\"matchedCount\":0,\"matchedCountExact\":true,",
                "\"returnedCount\":0,\"limit\":50,\"truncated\":false,\"scan\":{",
                "\"elements\":0,\"elementLimit\":20000,\"targets\":0,\"targetLimit\":1000,",
                "\"openRoots\":1,\"openRootLimit\":64,\"truncated\":false},",
                "\"activeTestId\":null}\n"
            )
        );
    }

    #[test]
    fn wait_output_compact_lines_are_byte_exact_and_value_free() {
        let success = UiWaitOutput {
            ok: true,
            request_id: "00000000-0000-4000-8000-000000000153".to_string(),
            window: "main".to_string(),
            action: "query",
            selector: "fixture.absent".to_string(),
            target: Value::Null,
            error: None,
            message: None,
            available: None,
            diagnostics: None,
            available_windows: None,
            timeout_ms: None,
            phase: None,
            kind: "ui-wait",
            command: "ui-wait",
            predicates: vec![UiWaitPredicateKind::Absent],
            attempts: 2,
            elapsed_ms: 50,
            last_observation: None,
        };
        let timeout = UiWaitOutput {
            ok: false,
            request_id: "00000000-0000-4000-8000-000000000154".to_string(),
            window: "main".to_string(),
            action: "query",
            selector: "fixture.missing".to_string(),
            target: Value::Null,
            error: Some("timeout".to_string()),
            message: Some(
                "Automation wait timed out before the requested condition was met.".to_string(),
            ),
            available: None,
            diagnostics: None,
            available_windows: None,
            timeout_ms: Some(150),
            phase: Some("wait_condition_not_met".to_string()),
            kind: "ui-wait",
            command: "ui-wait",
            predicates: vec![
                UiWaitPredicateKind::State,
                UiWaitPredicateKind::Text,
                UiWaitPredicateKind::Disabled,
                UiWaitPredicateKind::Selected,
                UiWaitPredicateKind::Expanded,
                UiWaitPredicateKind::Focused,
            ],
            attempts: 3,
            elapsed_ms: 150,
            last_observation: Some("missing_selector"),
        };
        let success_line = format!("{}\n", serde_json::to_string(&success).unwrap());
        let timeout_line = format!("{}\n", serde_json::to_string(&timeout).unwrap());

        assert_eq!(
            success_line,
            concat!(
                "{\"ok\":true,\"requestId\":\"00000000-0000-4000-8000-000000000153\",",
                "\"window\":\"main\",\"action\":\"query\",\"selector\":\"fixture.absent\",",
                "\"target\":null,\"kind\":\"ui-wait\",\"command\":\"ui-wait\",",
                "\"predicates\":[\"absent\"],\"attempts\":2,\"elapsedMs\":50}\n"
            )
        );
        assert_eq!(
            timeout_line,
            concat!(
                "{\"ok\":false,\"requestId\":\"00000000-0000-4000-8000-000000000154\",",
                "\"window\":\"main\",\"action\":\"query\",\"selector\":\"fixture.missing\",",
                "\"target\":null,\"error\":\"timeout\",\"message\":",
                "\"Automation wait timed out before the requested condition was met.\",",
                "\"timeoutMs\":150,\"phase\":\"wait_condition_not_met\",\"kind\":\"ui-wait\",",
                "\"command\":\"ui-wait\",\"predicates\":[\"state\",\"text\",\"disabled\",",
                "\"selected\",\"expanded\",\"focused\"],\"attempts\":3,\"elapsedMs\":150,",
                "\"lastObservation\":\"missing_selector\"}\n"
            )
        );
        assert!(!timeout_line.contains("secret-state"));
        assert!(!timeout_line.contains("AC_UI_PREDICATE_"));
    }

    #[test]
    fn wait_predicate_kind_inventory_matches_capabilities_in_wire_order() {
        let wire = UiWaitPredicateKind::all()
            .map(|kind| {
                serde_json::to_value(kind)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            wire,
            CAPABILITY_WAIT_PREDICATES
                .iter()
                .map(|value| (*value).to_string())
                .collect::<Vec<_>>()
        );
    }

    #[derive(Default)]
    struct UnsupportedAutomationBackend {
        observe_tree_calls: AtomicUsize,
        observe_identity_calls: AtomicUsize,
        terminate_verified_calls: AtomicUsize,
        current_process_memory_calls: AtomicUsize,
    }

    impl ProcessTreeBackend for UnsupportedAutomationBackend {
        fn supports_process_tree_enforcement(&self) -> bool {
            false
        }

        fn observe_tree(
            &self,
            _root: ProcessIdentity,
        ) -> Result<ObservedProcessTree, ResourceError> {
            self.observe_tree_calls.fetch_add(1, Ordering::SeqCst);
            Ok(ObservedProcessTree::default())
        }

        fn observe_identity(&self, _pid: u32) -> Result<Option<ProcessIdentity>, ResourceError> {
            self.observe_identity_calls.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        }

        fn terminate_verified(
            &self,
            _process: &ObservedProcess,
        ) -> Result<TerminateOutcome, ResourceError> {
            self.terminate_verified_calls.fetch_add(1, Ordering::SeqCst);
            Ok(TerminateOutcome::AlreadyGone)
        }

        fn current_process_memory(&self) -> Result<ProcessMemory, ResourceError> {
            self.current_process_memory_calls
                .fetch_add(1, Ordering::SeqCst);
            Ok(ProcessMemory::default())
        }
    }

    /// #1151 - supported process-tree backend for the automation tests. `observe_tree`
    /// synthesises a root plus one child from the requested root identity, so every
    /// registered group gets distinct identities and a private-bytes total above the
    /// kill thresholds these tests configure. A pid marked stubborn fails
    /// `terminate_verified`, which is what parks a group in `Quarantined`; marking it
    /// gone afterwards is what lets a later retry verify the cleanup.
    #[derive(Default)]
    struct AutomationProcessBackend {
        stubborn: Mutex<HashSet<u32>>,
        gone: Mutex<HashSet<u32>>,
        observe_tree_calls: AtomicUsize,
        observe_identity_calls: AtomicUsize,
        terminate_verified_calls: AtomicUsize,
    }

    impl AutomationProcessBackend {
        fn identity(pid: u32) -> ProcessIdentity {
            ProcessIdentity {
                pid,
                creation_time_100ns: u64::from(pid),
            }
        }

        fn child_pid(root_pid: u32) -> u32 {
            root_pid + 1
        }

        fn observed(pid: u32, depth: u32, parent_pid: Option<u32>) -> ObservedProcess {
            ObservedProcess {
                identity: Self::identity(pid),
                parent_pid,
                parent_identity: parent_pid.map(Self::identity),
                exe_name: format!("p{pid}"),
                depth,
                private_bytes: Some(5_000),
                working_set_bytes: Some(5_000),
                cpu_percent: None,
                kill_allowed: true,
            }
        }

        fn mark_stubborn(&self, pid: u32) {
            self.stubborn.lock().unwrap().insert(pid);
        }

        fn mark_gone(&self, pid: u32) {
            self.stubborn.lock().unwrap().remove(&pid);
            self.gone.lock().unwrap().insert(pid);
        }
    }

    impl ProcessTreeBackend for AutomationProcessBackend {
        fn observe_tree(
            &self,
            root: ProcessIdentity,
        ) -> Result<ObservedProcessTree, ResourceError> {
            self.observe_tree_calls.fetch_add(1, Ordering::SeqCst);
            let gone = self.gone.lock().unwrap();
            let child_pid = Self::child_pid(root.pid);
            let mut processes = Vec::new();
            if !gone.contains(&root.pid) {
                processes.push(Self::observed(root.pid, 0, None));
            }
            if !gone.contains(&child_pid) {
                processes.push(Self::observed(child_pid, 1, Some(root.pid)));
            }
            let errors = if gone.contains(&root.pid) {
                vec![format!("root pid {} was not in process snapshot", root.pid)]
            } else {
                Vec::new()
            };
            Ok(ObservedProcessTree { processes, errors })
        }

        fn observe_identity(&self, pid: u32) -> Result<Option<ProcessIdentity>, ResourceError> {
            self.observe_identity_calls.fetch_add(1, Ordering::SeqCst);
            if self.gone.lock().unwrap().contains(&pid) {
                Ok(None)
            } else {
                Ok(Some(Self::identity(pid)))
            }
        }

        fn terminate_verified(
            &self,
            process: &ObservedProcess,
        ) -> Result<TerminateOutcome, ResourceError> {
            self.terminate_verified_calls.fetch_add(1, Ordering::SeqCst);
            let pid = process.identity.pid;
            if self.gone.lock().unwrap().contains(&pid) {
                return Ok(TerminateOutcome::AlreadyGone);
            }
            if self.stubborn.lock().unwrap().contains(&pid) {
                return Err(ResourceError::Message(format!(
                    "pid {pid}: process still alive after terminate"
                )));
            }
            self.gone.lock().unwrap().insert(pid);
            Ok(TerminateOutcome::Terminated)
        }

        fn current_process_memory(&self) -> Result<ProcessMemory, ResourceError> {
            Ok(ProcessMemory::default())
        }
    }

    fn automation_settings() -> AppSettings {
        AppSettings {
            resource_monitor_enabled: true,
            resource_watchdog_action: ResourceWatchdogAction::KillGroup,
            max_concurrent_agent_processes: 4,
            agent_group_warn_private_bytes: 100,
            agent_group_kill_private_bytes: 200,
            agent_process_kill_private_bytes: 300,
            ..AppSettings::default()
        }
    }

    fn register_automation_group(
        monitor: &crate::resource_monitor::ResourceMonitorState,
        limits: crate::resource_monitor::ResourceLimits,
        session_id: Uuid,
        root_pid: u32,
    ) -> ProcessIdentity {
        let permit = monitor.try_reserve_agent_slot(limits).unwrap().unwrap();
        let mut registration = ResourceLaunchRegistration::new(
            monitor.clone(),
            permit,
            ResourceLaunchMetadata {
                session_id,
                name: "agent".to_string(),
                agent_id: None,
                agent_label: None,
                workgroup: None,
                agent: None,
                project: None,
            },
        );
        registration
            .register_root_pid(root_pid)
            .expect("register automation root pid")
            .expect("supported backend observes the root identity")
    }

    fn watchdog_backend_request(mode: &str) -> UiAutomationRequest {
        UiAutomationRequest {
            schema_version: PROTOCOL_SCHEMA_VERSION,
            instance_id: Uuid::new_v4().to_string(),
            pid: 1,
            started_at_unix_ms: 1,
            request_id: Uuid::new_v4().to_string(),
            token: "token".to_string(),
            exe_path: "test-executable".to_string(),
            config_dir: "test-config".to_string(),
            window: BACKEND_AUTOMATION_WINDOW.to_string(),
            action: UiAutomationAction::Backend,
            selector: RESOURCE_WATCHDOG_BACKEND_SELECTOR.to_string(),
            prefix: None,
            role: None,
            owner_window: None,
            session: None,
            value: Some(mode.to_string()),
            expires_at_unix_ms: Some(now_unix_ms() + 1_000),
        }
    }

    fn sample_request(request_id: String, selector: &str) -> UiAutomationRequest {
        UiAutomationRequest {
            schema_version: PROTOCOL_SCHEMA_VERSION,
            instance_id: Uuid::new_v4().to_string(),
            pid: 1,
            started_at_unix_ms: 1,
            request_id,
            token: "token".to_string(),
            exe_path: "test-executable".to_string(),
            config_dir: "test-config".to_string(),
            window: "main".to_string(),
            action: UiAutomationAction::Query,
            selector: selector.to_string(),
            prefix: None,
            role: None,
            owner_window: None,
            session: None,
            value: None,
            expires_at_unix_ms: Some(now_unix_ms() + 1_000),
        }
    }

    fn terminal_request(request_id: String) -> UiAutomationRequest {
        let mut request = sample_request(request_id, "terminal.snapshot");
        request.window = BACKEND_AUTOMATION_WINDOW.to_string();
        request.action = UiAutomationAction::Backend;
        request.owner_window = Some("main".to_string());
        request.session = Some(UiTerminalSessionSelector::Active);
        request
    }

    fn action_wire_name(action: UiAutomationAction) -> &'static str {
        match action {
            UiAutomationAction::Query => "query",
            UiAutomationAction::List => "list",
            UiAutomationAction::Click => "click",
            UiAutomationAction::ContextClick => "contextClick",
            UiAutomationAction::Hover => "hover",
            UiAutomationAction::SetValue => "setValue",
            UiAutomationAction::TypeText => "typeText",
            UiAutomationAction::Focus => "focus",
            UiAutomationAction::Backend => "backend",
        }
    }

    #[tokio::test]
    async fn unsupported_backend_reports_disabled_and_never_kills() {
        let backend = Arc::new(UnsupportedAutomationBackend::default());
        let monitor = Arc::new(crate::resource_monitor::ResourceMonitorState::with_backend(
            backend.clone() as Arc<dyn ProcessTreeBackend>,
        ));
        let app = tauri::test::mock_builder()
            .manage(monitor)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build unsupported resource-watchdog automation app");
        let cfg = AppSettings {
            resource_monitor_enabled: true,
            resource_watchdog_action: ResourceWatchdogAction::KillGroup,
            max_concurrent_agent_processes: 7,
            agent_group_warn_private_bytes: 101,
            agent_group_kill_private_bytes: 202,
            agent_process_kill_private_bytes: 303,
            ..AppSettings::default()
        };

        for mode in ["sample", "warn", "killGroup", "tick", "quarantineRetry"] {
            let request = watchdog_backend_request(mode);

            let response =
                handle_resource_watchdog_backend_request_with_config(app.handle(), &request, &cfg)
                    .await;

            assert!(response.ok);
            assert_eq!(
                response.target,
                Some(json!({
                    "testId": RESOURCE_WATCHDOG_BACKEND_SELECTOR,
                    "role": "backend",
                    "state": "disabled",
                    "tag": "backend",
                    "text": format!(
                        "resource monitor watchdog {mode}: 0 group(s), 0 warn match(es), 0 kill match(es)"
                    ),
                    "visible": true,
                    "disabled": false,
                }))
            );
            assert_eq!(
                response.diagnostics,
                Some(json!({
                    "mode": mode,
                    "configuredAction": cfg.resource_watchdog_action,
                    "resourceMonitorEnabled": false,
                    "killApplied": false,
                    "limits": {
                        "maxConcurrentAgentGroups": 7,
                        "groupWarnPrivateBytes": 101,
                        "groupKillPrivateBytes": 202,
                        "processKillPrivateBytes": 303,
                    },
                    "snapshot": {
                        "overallState": "unknown",
                        "activeAgentGroups": 0,
                        "appPrivateBytes": null,
                        "networkState": "unknown",
                        "networkSummary": "Socket attribution unavailable",
                        "warnings": [],
                    },
                    "decisions": [],
                    "killResults": [],
                    "quarantineRetries": [],
                }))
            );
        }

        assert_eq!(backend.observe_tree_calls.load(Ordering::SeqCst), 0);
        assert_eq!(backend.observe_identity_calls.load(Ordering::SeqCst), 0);
        assert_eq!(backend.terminate_verified_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            backend.current_process_memory_calls.load(Ordering::SeqCst),
            0
        );
    }

    // #1151 (A3) - the automation `killResults` contract for the two split outcomes.
    // `alreadyPending` keeps its value and type byte-identical so no existing automation
    // breaks; `reason` is the additive discriminator that makes the split observable.
    #[tokio::test]
    async fn watchdog_kill_results_preserve_already_pending_contract() {
        use crate::pty::backend::SessionBackendKind;

        let backend = Arc::new(AutomationProcessBackend::default());
        let monitor = Arc::new(crate::resource_monitor::ResourceMonitorState::with_backend(
            backend.clone() as Arc<dyn ProcessTreeBackend>,
        ));
        let cfg = automation_settings();
        let limits = crate::resource_monitor::ResourceLimits::from(&cfg);

        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let live = manager
            .read()
            .await
            .create_session(
                "shell".to_string(),
                Vec::new(),
                "C:/automation-kill-results".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();

        // One group whose public row exists (the in-flight case) and one whose row never
        // did (the orphan case). Both are Running with private bytes over the kill limit,
        // so the hook produces a kill decision for each.
        let orphan_id = Uuid::new_v4();
        register_automation_group(&monitor, limits, live.id, 5_100);
        register_automation_group(&monitor, limits, orphan_id, 5_200);

        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(Arc::clone(&monitor))
            .manage(coordinator.clone())
            .manage(DetachedSessionsState::default())
            .manage(WsBroadcaster::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build kill-results automation app");
        coordinator.start(app.handle().clone()).unwrap();
        let restore = coordinator.submit_restore_first().await.unwrap();

        // Fill the queue so the first watchdog kill parks holding ONLY its critical key,
        // without ever enqueueing a job.
        let mut reservations = Vec::new();
        while let Ok(reservation) = coordinator.reserve_auto_close() {
            reservations.push(reservation);
        }
        let waiter = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move { coordinator.watchdog_resource_kill(live.id).await })
        };
        tokio::time::timeout(Duration::from_secs(1), async {
            while !coordinator
                .critical_key_registered_for_test(live.id, CriticalAdmissionKind::WatchdogKill)
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the parked watchdog kill registers its critical key");

        assert!(matches!(
            coordinator.watchdog_resource_kill(live.id).await.unwrap(),
            WatchdogKillOutcome::AlreadyInFlight
        ));
        assert!(matches!(
            coordinator.watchdog_resource_kill(orphan_id).await.unwrap(),
            WatchdogKillOutcome::NoPublicSession
        ));

        let response = handle_resource_watchdog_backend_request_with_config(
            app.handle(),
            &watchdog_backend_request("killGroup"),
            &cfg,
        )
        .await;
        assert!(response.ok);
        let diagnostics = response.diagnostics.expect("watchdog diagnostics");
        let kill_results = diagnostics["killResults"]
            .as_array()
            .expect("killResults array")
            .clone();
        assert_eq!(kill_results.len(), 2);
        let entry_for = |session_id: Uuid| {
            kill_results
                .iter()
                .find(|entry| entry["sessionId"] == json!(session_id))
                .cloned()
                .unwrap_or_else(|| panic!("kill result for session {session_id}"))
        };
        assert_eq!(
            entry_for(live.id),
            json!({
                "ok": true,
                "sessionId": live.id,
                "alreadyPending": true,
                "reason": "alreadyInFlight",
            })
        );
        assert_eq!(
            entry_for(orphan_id),
            json!({
                "ok": true,
                "sessionId": orphan_id,
                "alreadyPending": true,
                "reason": "noPublicSession",
            })
        );

        restore.finish();
        coordinator.close_and_join().await;
        drop(reservations);
        let _ = waiter.await;
    }

    /// #1151 - registers a group, quarantines its cleanup on a stubborn child, and leaves
    /// the retry backoff already satisfied. No public session row is ever created, which
    /// is precisely what makes the group an orphan.
    fn orphan_automation_group(
        monitor: &crate::resource_monitor::ResourceMonitorState,
        backend: &AutomationProcessBackend,
        limits: crate::resource_monitor::ResourceLimits,
        session_id: Uuid,
        root_pid: u32,
    ) -> ProcessIdentity {
        backend.mark_stubborn(AutomationProcessBackend::child_pid(root_pid));
        let root = register_automation_group(monitor, limits, session_id, root_pid);
        let quarantine = monitor
            .kill_group(
                session_id,
                crate::resource_monitor::types::ResourceKillReason::SessionDestroy,
            )
            .expect("first cleanup runs");
        assert!(quarantine.quarantined);
        monitor.test_backdate_quarantine_retry(session_id);
        root
    }

    async fn automation_app_with_coordinator(
        monitor: Arc<crate::resource_monitor::ResourceMonitorState>,
    ) -> (tauri::App<tauri::test::MockRuntime>, SelectionCoordinator) {
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(monitor)
            .manage(coordinator.clone())
            .manage(DetachedSessionsState::default())
            .manage(WsBroadcaster::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build quarantine-retry automation app");
        coordinator
            .start(app.handle().clone())
            .expect("start quarantine-retry coordinator");
        coordinator
            .submit_restore_first()
            .await
            .expect("restore-first admitted")
            .finish();
        (app, coordinator)
    }

    // #1151 (A1) - the deterministic native artifact for a successful reclaim. Everything
    // is read from quarantineRetries[0], NEVER from diagnostics.snapshot, which is captured
    // before the action loop and would still read activeAgentGroups = 1 on a correct build.
    #[tokio::test]
    async fn quarantine_retry_mode_reports_completed_orphan_cleanup() {
        let backend = Arc::new(AutomationProcessBackend::default());
        let monitor = Arc::new(crate::resource_monitor::ResourceMonitorState::with_backend(
            backend.clone() as Arc<dyn ProcessTreeBackend>,
        ));
        let cfg = automation_settings();
        let limits = crate::resource_monitor::ResourceLimits::from(&cfg);

        let session_id = Uuid::new_v4();
        let root = orphan_automation_group(&monitor, &backend, limits, session_id, 6_100);
        assert_eq!(monitor.active_agent_groups(), 1);
        // The blocker finally exits, which is the only thing a retry can discover.
        backend.mark_gone(AutomationProcessBackend::child_pid(6_100));

        let (app, coordinator) = automation_app_with_coordinator(Arc::clone(&monitor)).await;
        let response = handle_resource_watchdog_backend_request_with_config(
            app.handle(),
            &watchdog_backend_request("quarantineRetry"),
            &cfg,
        )
        .await;

        assert!(response.ok);
        let diagnostics = response.diagnostics.expect("watchdog diagnostics");
        let retries = diagnostics["quarantineRetries"]
            .as_array()
            .expect("quarantineRetries array");
        assert_eq!(retries.len(), 1);
        assert_eq!(retries[0]["sessionId"], json!(session_id));
        assert_eq!(retries[0]["path"], json!("orphan"));
        assert_eq!(retries[0]["rootPid"], json!(root.pid));
        assert_eq!(retries[0]["quarantined"], json!(false));
        assert_eq!(retries[0]["stillCountsTowardAdmission"], json!(false));
        assert_eq!(retries[0]["activeAgentGroups"], json!(0));
        // The pre-action snapshot deliberately still shows the group counted.
        assert_eq!(diagnostics["snapshot"]["activeAgentGroups"], json!(1));
        coordinator.close_and_join().await;
    }

    // #1151 (A2) - the safety artifact. An owned descendant that is still unverifiable
    // keeps blocking capacity, and the report says so instead of claiming a reclaim.
    #[tokio::test]
    async fn quarantine_retry_mode_reports_still_blocked_group() {
        let backend = Arc::new(AutomationProcessBackend::default());
        let monitor = Arc::new(crate::resource_monitor::ResourceMonitorState::with_backend(
            backend.clone() as Arc<dyn ProcessTreeBackend>,
        ));
        let cfg = automation_settings();
        let limits = crate::resource_monitor::ResourceLimits::from(&cfg);

        let session_id = Uuid::new_v4();
        // The blocker is deliberately NOT marked gone.
        orphan_automation_group(&monitor, &backend, limits, session_id, 6_200);

        let (app, coordinator) = automation_app_with_coordinator(Arc::clone(&monitor)).await;
        let response = handle_resource_watchdog_backend_request_with_config(
            app.handle(),
            &watchdog_backend_request("quarantineRetry"),
            &cfg,
        )
        .await;

        let diagnostics = response.diagnostics.expect("watchdog diagnostics");
        let retries = diagnostics["quarantineRetries"]
            .as_array()
            .expect("quarantineRetries array");
        assert_eq!(retries.len(), 1);
        assert_eq!(retries[0]["path"], json!("orphan"));
        assert_eq!(retries[0]["quarantined"], json!(true));
        assert_eq!(retries[0]["stillCountsTowardAdmission"], json!(true));
        assert_eq!(retries[0]["activeAgentGroups"], json!(1));
        assert_eq!(monitor.active_agent_groups(), 1);
        coordinator.close_and_join().await;
    }

    // #1151 (A4) - an unsupported backend stays at zero admission and zero enforcement
    // calls in the new mode too.
    #[tokio::test]
    async fn unsupported_backend_quarantine_retry_is_disabled() {
        let backend = Arc::new(UnsupportedAutomationBackend::default());
        let monitor = Arc::new(crate::resource_monitor::ResourceMonitorState::with_backend(
            backend.clone() as Arc<dyn ProcessTreeBackend>,
        ));
        let cfg = automation_settings();

        let (app, coordinator) = automation_app_with_coordinator(Arc::clone(&monitor)).await;
        let response = handle_resource_watchdog_backend_request_with_config(
            app.handle(),
            &watchdog_backend_request("quarantineRetry"),
            &cfg,
        )
        .await;

        assert!(response.ok);
        assert_eq!(response.target.expect("target")["state"], json!("disabled"));
        let diagnostics = response.diagnostics.expect("watchdog diagnostics");
        assert_eq!(diagnostics["quarantineRetries"], json!([]));
        assert_eq!(backend.observe_tree_calls.load(Ordering::SeqCst), 0);
        assert_eq!(backend.observe_identity_calls.load(Ordering::SeqCst), 0);
        assert_eq!(backend.terminate_verified_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            backend.current_process_memory_calls.load(Ordering::SeqCst),
            0
        );
        coordinator.close_and_join().await;
    }

    #[test]
    fn action_serializes_camel_case() {
        for action in UiAutomationAction::all() {
            assert_eq!(
                serde_json::to_string(&action).unwrap(),
                format!("\"{}\"", action_wire_name(action))
            );
        }
    }

    #[test]
    fn every_webview_response_serializes_nullable_active_test_id_explicitly() {
        for action in UiAutomationAction::all()
            .filter(|action| *action != UiAutomationAction::Backend)
        {
            let response = UiAutomationResponse::minimal_error(
                "00000000-0000-4000-8000-000000000159",
                "main",
                action,
                if action == UiAutomationAction::List {
                    ""
                } else {
                    "fixture.target"
                },
                "missing_selector",
                "fixture",
            );
            let encoded = serde_json::to_value(response).unwrap();
            assert_eq!(
                encoded.get("activeTestId"),
                Some(&Value::Null),
                "{action:?} omitted its required nullable activeTestId"
            );
        }
    }

    /// #944 - the Rust enum and the `UiAutomationAction` union in `src/shared/types.ts`
    /// are two closed lists that MUST agree. They are not generated from one another
    /// (no ts-rs / typeshare / specta in this crate), so nothing but this test stops
    /// them drifting.
    ///
    /// Drift is NOT a crash, and an earlier draft of this comment claimed it was: the
    /// request file is written and read by the same Rust enum, so serde never sees an
    /// action it does not know. What actually happens is worse in one way. A Rust-only
    /// addition ships a verb that exists, is documented, and fails 100% of the time at
    /// runtime, because the bridge's catch-all (automation-bridge.ts:181-188) answers
    /// `unsupported_action`. Today that is caught only by a human running the acceptance
    /// runbook. This test moves it to `cargo test`.
    #[test]
    fn ui_automation_action_wire_names_match_typescript_union() {
        // First test in this crate to read OUTSIDE the crate root. `CARGO_MANIFEST_DIR`
        // expands at COMPILE time to the absolute path of src-tauri, so the process cwd
        // is irrelevant, and CI checks out the full tree (no sparse-checkout).
        let types_ts = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/shared/types.ts"),
        )
        .expect("read src/shared/types.ts");

        let start = types_ts
            .find("export type UiAutomationAction =")
            .expect("UiAutomationAction union not found in types.ts");

        // Strip `//` comments FIRST, before anything else reads this text. Two hazards,
        // and the ORDER is what defuses them: a comment inside the union carrying a quote
        // would inject a phantom member, and one carrying a `;` would truncate the union
        // early. The first revision of this test stripped AFTER `find(';')`, which left the
        // truncation hazard fully live while this comment claimed it was handled. That is
        // precisely the defect #944 exists to kill (a comment asserting a protection the
        // code does not have), so it is called out here rather than quietly repaired:
        // strip, THEN find the terminator. Both are false REDs, and a parity test that
        // cries wolf is a parity test somebody deletes.
        let stripped: String = types_ts[start..]
            .lines()
            .map(|line| line.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");

        let end = stripped
            .find(';')
            .expect("unterminated UiAutomationAction union");

        let members: HashSet<String> = stripped[..end]
            .split('"')
            .skip(1)
            .step_by(2)
            .map(|s| s.to_string())
            .collect();

        assert!(
            !members.is_empty(),
            "parsed zero members from the UiAutomationAction union; the parser or the union format changed"
        );

        let rust: HashSet<String> = UiAutomationAction::all()
            .map(|action| action_wire_name(action).to_string())
            .collect();

        assert_eq!(
            members, rust,
            "UiAutomationAction is out of sync between src/shared/types.ts and ui_automation.rs"
        );

        assert_eq!(
            types_ts.matches("activeTestId: string | null;").count(),
            3,
            "every TypeScript UiAutomationResponse union arm must require nullable activeTestId"
        );
    }

    #[test]
    fn request_file_parses_ready_and_inflight_names() {
        let id = Uuid::new_v4().to_string();
        let ready = RequestFile::from_path(Path::new(&format!("{id}.json"))).unwrap();
        assert_eq!(ready.request_id, id);
        matches!(ready.kind, RequestFileKind::Ready);

        let inflight = RequestFile::from_path(Path::new(&format!("{id}.inflight.json"))).unwrap();
        assert_eq!(inflight.request_id, id);
        matches!(inflight.kind, RequestFileKind::Inflight);

        assert!(RequestFile::from_path(Path::new("not-a-uuid.json")).is_none());
        assert!(RequestFile::from_path(Path::new(&format!("{id}.tmp"))).is_none());
    }

    #[test]
    fn request_expiry_uses_optional_deadline() {
        let mut request = sample_request(Uuid::new_v4().to_string(), "target");
        request.expires_at_unix_ms = None;
        assert!(!request_expired(&request, 100));

        request.expires_at_unix_ms = Some(99);
        assert!(request_expired(&request, 100));

        request.expires_at_unix_ms = Some(101);
        assert!(!request_expired(&request, 100));
    }

    #[test]
    fn complete_rejects_unknown_request_id() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_automation_state(tmp.path());
        let response = UiAutomationResponse::minimal_error(
            &Uuid::new_v4().to_string(),
            "main",
            UiAutomationAction::Query,
            "target",
            "frontend_error",
            "frontend failed",
        );

        assert_eq!(
            state.complete("main", response).unwrap_err(),
            "unknown_request_id"
        );
    }

    #[test]
    fn frontend_ready_registers_dynamic_caller_label() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_automation_state(tmp.path());
        state.initialize_files().unwrap();

        state
            .mark_frontend_ready("resource-monitor", Some("resource-monitor"))
            .unwrap();

        let raw = fs::read_to_string(&state.inner.session_path).unwrap();
        let session: UiAutomationSession = serde_json::from_str(&raw).unwrap();
        assert_eq!(
            session.window_labels,
            vec!["main".to_string(), "resource-monitor".to_string()]
        );
        assert_eq!(
            session.ready_window_labels,
            vec!["resource-monitor".to_string()]
        );
    }

    #[test]
    fn live_window_sync_prunes_closed_dynamic_windows() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_automation_state(tmp.path());
        state.initialize_files().unwrap();
        state
            .mark_frontend_ready("resource-monitor", Some("resource-monitor"))
            .unwrap();

        state
            .sync_live_window_labels(vec!["main".to_string()])
            .unwrap();

        let raw = fs::read_to_string(&state.inner.session_path).unwrap();
        let session: UiAutomationSession = serde_json::from_str(&raw).unwrap();
        assert_eq!(session.window_labels, vec!["main".to_string()]);
        assert!(session.ready_window_labels.is_empty());
    }

    #[test]
    fn complete_writes_completion_mismatch_response() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_automation_state(tmp.path());
        fs::create_dir_all(state.requests_dir()).unwrap();
        fs::create_dir_all(state.responses_dir()).unwrap();

        let request_id = Uuid::new_v4().to_string();
        let request = sample_request(request_id.clone(), "expected");
        let response_path = state.response_path(&request_id);
        let inflight_path = state.inflight_path(&request_id);
        fs::write(&inflight_path, "{}").unwrap();
        state.inner.pending.lock().unwrap().insert(
            request_id.clone(),
            PendingRequest {
                request,
                response_path: response_path.clone(),
                inflight_path: inflight_path.clone(),
            },
        );

        let result = UiAutomationResponse {
            ok: true,
            request_id,
            window: "main".to_string(),
            action: UiAutomationAction::Query,
            selector: "different".to_string(),
            target: Some(json!({ "testId": "different" })),
            error: None,
            message: None,
            available: None,
            diagnostics: None,
            available_windows: None,
            timeout_ms: None,
            phase: None,
            active_test_id: Value::Null,
            filters: None,
            targets: None,
            matched_count: None,
            matched_count_exact: None,
            returned_count: None,
            limit: None,
            truncated: None,
            scan: None,
            terminal_snapshot: None,
        };

        state.complete("main", result).unwrap();

        let raw = fs::read_to_string(response_path).unwrap();
        let written: UiAutomationResponse = serde_json::from_str(&raw).unwrap();
        assert!(!written.ok);
        assert_eq!(written.error.as_deref(), Some("completion_mismatch"));
        assert!(!inflight_path.exists());
    }

    #[test]
    fn complete_preserves_correlated_obscured_click_and_focus_failures() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_automation_state(tmp.path());
        fs::create_dir_all(state.requests_dir()).unwrap();
        fs::create_dir_all(state.responses_dir()).unwrap();

        for action in [UiAutomationAction::Click, UiAutomationAction::Focus] {
            let request_id = Uuid::new_v4().to_string();
            let mut request = sample_request(request_id.clone(), "actionBar.resourceMonitor");
            request.action = action;
            request.expires_at_unix_ms = Some(now_unix_ms() + 10_000);
            let response_path = state.response_path(&request_id);
            let inflight_path = state.inflight_path(&request_id);
            fs::write(&inflight_path, "{}").unwrap();
            state.inner.pending.lock().unwrap().insert(
                request_id.clone(),
                PendingRequest {
                    request: request.clone(),
                    response_path: response_path.clone(),
                    inflight_path: inflight_path.clone(),
                },
            );

            let response = UiAutomationResponse::minimal_error(
                &request_id,
                "main",
                action,
                "actionBar.resourceMonitor",
                "target_obscured",
                "The requested automation target is obscured.",
            );
            state.complete("main", response).unwrap();

            let written: UiAutomationResponse =
                serde_json::from_slice(&fs::read(&response_path).unwrap()).unwrap();
            let cli_response = sanitize_response_for_cli(written);
            assert!(!cli_response.ok);
            assert_eq!(cli_response.request_id, request_id);
            assert_eq!(cli_response.window, "main");
            assert_eq!(cli_response.action, action);
            assert_eq!(cli_response.selector, "actionBar.resourceMonitor");
            assert_eq!(cli_response.error.as_deref(), Some("target_obscured"));
            assert_ne!(
                cli_response.error.as_deref(),
                Some("automation_protocol_mismatch")
            );
            assert!(cli_response.available.is_none());
            assert!(cli_response.diagnostics.is_none());
            assert!(!inflight_path.exists());

            let encoded = serde_json::to_value(cli_response).unwrap();
            assert!(encoded.get("available").is_none());
            assert!(encoded.get("diagnostics").is_none());
        }
    }

    #[test]
    fn typed_webview_failures_respect_the_exact_available_allowlist() {
        let typed_failures = [
            (UiAutomationAction::Query, "target_hidden"),
            (UiAutomationAction::Click, "target_obscured"),
            (UiAutomationAction::Focus, "target_obscured"),
            (UiAutomationAction::Click, "target_disabled"),
            (UiAutomationAction::Focus, "target_stale"),
            (UiAutomationAction::Focus, "target_not_focusable"),
            (UiAutomationAction::Focus, "focus_failed"),
            (UiAutomationAction::Click, "request_expired"),
            (UiAutomationAction::Query, "timeout"),
            (UiAutomationAction::Query, "unsupported_action"),
            (UiAutomationAction::Hover, "value_not_supported"),
            (UiAutomationAction::Query, "automation_bridge_exception"),
        ];

        for (action, error) in typed_failures {
            let mut request = sample_request(Uuid::new_v4().to_string(), "fixture.target");
            request.action = action;
            let response = UiAutomationResponse::minimal_error(
                &request.request_id,
                "main",
                action,
                "fixture.target",
                error,
                "typed frontend failure",
            );

            validate_response_correlation(&request, &response)
                .unwrap_or_else(|mismatch| panic!("{error} degraded to {mismatch}"));
            let cli_response = sanitize_response_for_cli(response);
            assert_eq!(cli_response.error.as_deref(), Some(error));
            assert!(cli_response.available.is_none());
        }

        for error in ["missing_selector", "duplicate_selector"] {
            let request = sample_request(Uuid::new_v4().to_string(), "fixture.target");
            let mut response = UiAutomationResponse::minimal_error(
                &request.request_id,
                "main",
                UiAutomationAction::Query,
                "fixture.target",
                error,
                "public discovery failure",
            );
            response.available = Some(vec![json!({
                "testId": "fixture.public",
                "visible": true,
                "disabled": false,
            })]);

            validate_response_correlation(&request, &response).unwrap();
            let cli_response = sanitize_response_for_cli(response);
            assert_eq!(cli_response.available.as_ref().map(Vec::len), Some(1));
            assert_eq!(
                cli_response.available.as_ref().unwrap()[0]["testId"],
                json!("fixture.public")
            );
        }

        let request = sample_request(Uuid::new_v4().to_string(), "fixture.target");
        let mut leaked = UiAutomationResponse::minimal_error(
            &request.request_id,
            "main",
            UiAutomationAction::Query,
            "fixture.target",
            "target_hidden",
            "typed frontend failure",
        );
        leaked.available = Some(vec![json!({ "testId": "fixture.must-not-leak" })]);
        let mismatch = validate_response_correlation(&request, &leaked).unwrap_err();
        assert_eq!(mismatch["error"], "automation_protocol_mismatch");
    }

    #[test]
    fn webview_completion_loses_when_clock_advances_inside_the_native_commit_closure() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_automation_state(tmp.path());
        fs::create_dir_all(state.requests_dir()).unwrap();
        fs::create_dir_all(state.responses_dir()).unwrap();
        let request_id = Uuid::new_v4().to_string();
        let mut request = sample_request(request_id.clone(), "expected");
        request.expires_at_unix_ms = Some(100);
        let response_path = state.response_path(&request_id);
        let inflight_path = state.inflight_path(&request_id);
        fs::write(&inflight_path, "{}").unwrap();
        state.inner.pending.lock().unwrap().insert(
            request_id.clone(),
            PendingRequest {
                request,
                response_path: response_path.clone(),
                inflight_path: inflight_path.clone(),
            },
        );
        let result = UiAutomationResponse {
            ok: true,
            request_id,
            window: "main".to_string(),
            action: UiAutomationAction::Query,
            selector: "expected".to_string(),
            target: Some(json!({"testId": "must-not-publish-success"})),
            error: None,
            message: None,
            available: None,
            diagnostics: None,
            available_windows: None,
            timeout_ms: None,
            phase: None,
            active_test_id: Value::Null,
            filters: None,
            targets: None,
            matched_count: None,
            matched_count_exact: None,
            returned_count: None,
            limit: None,
            truncated: None,
            scan: None,
            terminal_snapshot: None,
        };
        let clock = AtomicUsize::new(99);
        let clock_calls = AtomicUsize::new(0);
        let now = || {
            clock_calls.fetch_add(1, Ordering::SeqCst);
            clock.load(Ordering::SeqCst) as i64
        };

        assert_eq!(
            state
                .complete_with_now_and_precommit_hook("main", result, now, || {
                    assert_eq!(clock_calls.load(Ordering::SeqCst), 1);
                    assert!(!response_path.exists());
                    clock.store(100, Ordering::SeqCst);
                })
                .unwrap_err(),
            "request_expired"
        );

        let raw = fs::read_to_string(response_path).unwrap();
        assert!(!raw.contains("must-not-publish-success"));
        let written: UiAutomationResponse = serde_json::from_str(&raw).unwrap();
        assert_eq!(written.error.as_deref(), Some("request_expired"));
        assert!(written.target.is_none());
        assert!(!inflight_path.exists());
        assert_eq!(clock_calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn expire_pending_requests_writes_timeout_response() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_automation_state(tmp.path());
        fs::create_dir_all(state.requests_dir()).unwrap();
        fs::create_dir_all(state.responses_dir()).unwrap();

        let request_id = Uuid::new_v4().to_string();
        let mut request = sample_request(request_id.clone(), "missing.target");
        request.expires_at_unix_ms = Some(now_unix_ms() - 1);
        let response_path = state.response_path(&request_id);
        let inflight_path = state.inflight_path(&request_id);
        fs::write(&inflight_path, "{}").unwrap();
        state.inner.pending.lock().unwrap().insert(
            request_id,
            PendingRequest {
                request,
                response_path: response_path.clone(),
                inflight_path: inflight_path.clone(),
            },
        );

        state.expire_pending_requests();

        let raw = fs::read_to_string(response_path).unwrap();
        let written: UiAutomationResponse = serde_json::from_str(&raw).unwrap();
        assert!(!written.ok);
        assert_eq!(written.error.as_deref(), Some("request_expired"));
        assert!(!inflight_path.exists());
        assert!(state.inner.pending.lock().unwrap().is_empty());
    }

    #[test]
    fn terminal_completion_loses_when_clock_advances_inside_the_native_commit_closure() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_automation_state(tmp.path());
        fs::create_dir_all(state.requests_dir()).unwrap();
        fs::create_dir_all(state.responses_dir()).unwrap();
        let request_id = Uuid::new_v4().to_string();
        let mut request = terminal_request(request_id.clone());
        request.expires_at_unix_ms = Some(100);
        let response = UiAutomationResponse::terminal_success(
            &request,
            json!({"mustNotPublish": "captured terminal"}),
        );
        let response_path = state.response_path(&request_id);
        let inflight_path = state.inflight_path(&request_id);
        fs::write(&inflight_path, "{}").unwrap();
        state.inner.pending.lock().unwrap().insert(
            request_id.clone(),
            PendingRequest {
                request,
                response_path: response_path.clone(),
                inflight_path: inflight_path.clone(),
            },
        );
        let cancelled = Arc::new(AtomicBool::new(false));
        state.inner.terminal_tasks.lock().unwrap().insert(
            request_id.clone(),
            TerminalTaskControl {
                cancelled: Arc::clone(&cancelled),
                phase: TerminalTaskPhase::Running,
                handle: None,
            },
        );
        let clock = AtomicUsize::new(99);
        let clock_calls = AtomicUsize::new(0);
        let now = || {
            clock_calls.fetch_add(1, Ordering::SeqCst);
            clock.load(Ordering::SeqCst) as i64
        };

        state.publish_terminal_task_response_with_now_and_precommit_hook(
            &request_id,
            cancelled.as_ref(),
            response,
            now,
            || {
                assert_eq!(clock_calls.load(Ordering::SeqCst), 1);
                assert!(!response_path.exists());
                clock.store(100, Ordering::SeqCst);
            },
        );

        let raw = fs::read_to_string(response_path).unwrap();
        assert!(!raw.contains("captured terminal"));
        let written: UiAutomationResponse = serde_json::from_str(&raw).unwrap();
        assert_eq!(written.error.as_deref(), Some("request_expired"));
        assert!(written.terminal_snapshot.is_none());
        assert!(cancelled.load(Ordering::SeqCst));
        assert!(!inflight_path.exists());
        assert_eq!(clock_calls.load(Ordering::SeqCst), 2);
        state
            .inner
            .terminal_tasks
            .lock()
            .unwrap()
            .remove(&request_id);
    }

    #[test]
    fn expired_terminal_task_keeps_its_permit_and_handle_until_real_thread_exit() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_automation_state(tmp.path());
        fs::create_dir_all(state.requests_dir()).unwrap();
        fs::create_dir_all(state.responses_dir()).unwrap();
        let request_id = Uuid::new_v4().to_string();
        let mut request = terminal_request(request_id.clone());
        request.expires_at_unix_ms = Some(now_unix_ms() - 1);
        let response_path = state.response_path(&request_id);
        let inflight_path = state.inflight_path(&request_id);
        fs::write(&inflight_path, "{}").unwrap();

        let permit = Arc::clone(&state.inner.terminal_task_permits)
            .try_acquire_owned()
            .unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            let _permit = permit;
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx.recv().unwrap();
        state.inner.pending.lock().unwrap().insert(
            request_id.clone(),
            PendingRequest {
                request,
                response_path: response_path.clone(),
                inflight_path: inflight_path.clone(),
            },
        );
        state.inner.terminal_tasks.lock().unwrap().insert(
            request_id.clone(),
            TerminalTaskControl {
                cancelled: Arc::clone(&cancelled),
                phase: TerminalTaskPhase::Running,
                handle: Some(handle),
            },
        );

        state.expire_pending_requests();

        assert!(cancelled.load(Ordering::SeqCst));
        assert!(state
            .inner
            .terminal_tasks
            .lock()
            .unwrap()
            .contains_key(&request_id));
        assert_eq!(state.inner.terminal_task_permits.available_permits(), 1);
        assert!(!inflight_path.exists());
        let response: UiAutomationResponse =
            serde_json::from_str(&fs::read_to_string(response_path).unwrap()).unwrap();
        assert_eq!(response.error.as_deref(), Some("request_expired"));

        release_tx.send(()).unwrap();
        while !state
            .inner
            .terminal_tasks
            .lock()
            .unwrap()
            .get(&request_id)
            .and_then(|control| control.handle.as_ref())
            .is_some_and(JoinHandle::is_finished)
        {
            std::thread::yield_now();
        }
        state.reap_finished_terminal_tasks();
        assert!(state.inner.terminal_tasks.lock().unwrap().is_empty());
        assert_eq!(state.inner.terminal_task_permits.available_permits(), 2);
    }

    #[test]
    fn terminal_shutdown_seals_admission_and_joins_before_registry_removal() {
        fn assert_send_sync_static<T: Send + Sync + 'static>() {}
        assert_send_sync_static::<UiAutomationState>();
        assert_send_sync_static::<TerminalTaskControl>();

        let tmp = tempfile::tempdir().unwrap();
        let state = test_automation_state(tmp.path());
        let request_id = Uuid::new_v4().to_string();
        let permit = Arc::clone(&state.inner.terminal_task_permits)
            .try_acquire_owned()
            .unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            let _permit = permit;
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx.recv().unwrap();
        state.inner.terminal_tasks.lock().unwrap().insert(
            request_id.clone(),
            TerminalTaskControl {
                cancelled: Arc::clone(&cancelled),
                phase: TerminalTaskPhase::Running,
                handle: Some(handle),
            },
        );

        let closing_state = state.clone();
        let (closed_tx, closed_rx) = std::sync::mpsc::channel();
        let closer = std::thread::spawn(move || {
            closing_state.close_and_join_terminal_tasks();
            closed_tx.send(()).unwrap();
        });
        while !cancelled.load(Ordering::SeqCst) {
            std::thread::yield_now();
        }
        assert!(self::std_channel_is_empty(&closed_rx));
        assert!(state
            .inner
            .terminal_task_admission_closed
            .load(Ordering::SeqCst));
        assert!(state
            .inner
            .terminal_tasks
            .lock()
            .unwrap()
            .contains_key(&request_id));
        assert_eq!(state.inner.terminal_task_permits.available_permits(), 1);

        release_tx.send(()).unwrap();
        closed_rx.recv().unwrap();
        closer.join().unwrap();
        assert!(state.inner.terminal_tasks.lock().unwrap().is_empty());
        assert_eq!(state.inner.terminal_task_permits.available_permits(), 2);
    }

    fn std_channel_is_empty(receiver: &std::sync::mpsc::Receiver<()>) -> bool {
        matches!(receiver.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty))
    }

    #[test]
    fn request_scan_collection_does_no_more_than_sixty_four_units_of_work() {
        let visited = AtomicUsize::new(0);
        let entries = (0..1_000).map(|index| {
            visited.fetch_add(1, Ordering::SeqCst);
            Ok(PathBuf::from(format!("{index:04}.invalid")))
        });

        let paths = bounded_sorted_request_paths(entries).unwrap();

        assert_eq!(paths.len(), MAX_REQUEST_FILES_PER_SCAN);
        assert_eq!(visited.load(Ordering::SeqCst), MAX_REQUEST_FILES_PER_SCAN);
    }

    #[test]
    fn request_atomic_temp_is_staged_outside_the_scanned_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let requests = tmp.path().join(REQUESTS_DIR);
        let request_path = requests.join(format!("{}.json", Uuid::new_v4()));

        let temp_path = atomic_write_temp_path(&request_path);

        assert_eq!(temp_path.parent(), Some(tmp.path()));
        assert_ne!(temp_path.parent(), Some(requests.as_path()));
    }

    #[test]
    fn invalid_scan_batch_cannot_permanently_starve_a_later_valid_request() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_automation_state(tmp.path());
        fs::create_dir_all(state.requests_dir()).unwrap();
        for index in 0..MAX_REQUEST_FILES_PER_SCAN {
            fs::write(
                state.requests_dir().join(format!("0000-{index:04}.invalid")),
                "invalid",
            )
            .unwrap();
        }
        let request_id = Uuid::new_v4().to_string();
        fs::write(
            state.requests_dir().join(format!("{request_id}.json")),
            "{}",
        )
        .unwrap();

        let mut observed = false;
        for _ in 0..=2 {
            let batch = state.request_scan_batch().unwrap();
            if batch.iter().any(|request| request.request_id == request_id) {
                observed = true;
                break;
            }
        }

        assert!(observed, "valid request remained starved after invalid cleanup");
        assert_eq!(
            fs::read_dir(state.requests_dir())
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().ends_with(".invalid"))
                .count(),
            0
        );
    }

    #[test]
    fn nonempty_invalid_directory_does_not_discard_a_valid_request_in_the_batch() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_automation_state(tmp.path());
        fs::create_dir_all(state.requests_dir()).unwrap();
        let invalid = state.requests_dir().join("0000-invalid");
        fs::create_dir(&invalid).unwrap();
        fs::write(invalid.join("child"), "must remain bounded").unwrap();
        let request_id = Uuid::new_v4().to_string();
        fs::write(
            state.requests_dir().join(format!("{request_id}.json")),
            "{}",
        )
        .unwrap();

        let batch = state.request_scan_batch().unwrap();

        assert!(batch.iter().any(|request| request.request_id == request_id));
        assert_eq!(
            fs::read_to_string(invalid.join("child")).unwrap(),
            "must remain bounded"
        );
    }

    #[test]
    fn delete_denied_invalid_entries_do_bounded_work_and_allow_repeated_valid_progress() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_automation_state(tmp.path());
        fs::create_dir_all(state.requests_dir()).unwrap();
        for index in 0..MAX_REQUEST_FILES_PER_SCAN {
            fs::write(
                state
                    .requests_dir()
                    .join(format!("0000-{index:04}.invalid")),
                "invalid",
            )
            .unwrap();
        }
        let cleanup_calls = AtomicUsize::new(0);
        let denied_cleanup = |_path: &Path| {
            cleanup_calls.fetch_add(1, Ordering::SeqCst);
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "injected delete denial",
            ))
        };

        for generation in 0..2 {
            let request_id = Uuid::new_v4().to_string();
            let request_path = state.requests_dir().join(format!("{request_id}.json"));
            fs::write(&request_path, "{}").unwrap();
            let before = cleanup_calls.load(Ordering::SeqCst);
            let mut observed = false;
            for _ in 0..=3 {
                let batch = state
                    .request_scan_batch_with_cleanup(&denied_cleanup)
                    .unwrap();
                if batch.iter().any(|request| request.request_id == request_id) {
                    observed = true;
                    break;
                }
            }
            assert!(observed, "valid generation {generation} remained starved");
            assert!(
                cleanup_calls.load(Ordering::SeqCst) - before <= MAX_REQUEST_FILES_PER_SCAN * 4
            );
            fs::remove_file(request_path).unwrap();
        }

        assert_eq!(
            fs::read_dir(state.requests_dir()).unwrap().count(),
            MAX_REQUEST_FILES_PER_SCAN
        );
    }

    #[test]
    fn shutdown_cleanup_drops_an_exact_batch_live_scan_cursor_before_removal() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_automation_state(tmp.path());
        state.initialize_files().unwrap();
        for index in 0..MAX_REQUEST_FILES_PER_SCAN {
            fs::write(
                state
                    .requests_dir()
                    .join(format!("0000-{index:04}.invalid")),
                "delete-denied-during-scan",
            )
            .unwrap();
        }
        let denied_cleanup = |_path: &Path| {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "injected delete denial",
            ))
        };

        assert!(state
            .request_scan_batch_with_cleanup(denied_cleanup)
            .unwrap()
            .is_empty());
        assert!(state
            .inner
            .request_scan_cursor
            .lock()
            .unwrap()
            .is_some());
        fs::write(state.responses_dir().join("owned-response.invalid"), "owned").unwrap();
        assert!(state.inner.session_path.is_file());

        state.cleanup_owned_automation_files();

        assert!(state
            .inner
            .request_scan_cursor
            .lock()
            .unwrap()
            .is_none());
        assert!(!state.requests_dir().exists());
        assert!(!state.responses_dir().exists());
        assert!(!state.inner.session_path.exists());
        assert_eq!(
            fs::read_dir(&state.inner.automation_dir).unwrap().count(),
            0,
            "all owned artifacts must be removed while the state witness is live"
        );
        assert!(state.verify_config_ownership());
    }

    #[test]
    fn atomic_singleton_replacement_preserves_old_bytes_when_commit_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(SESSION_FILE);
        let temp_path = tmp.path().join("forced-session.tmp");
        let old = br#"{"generation":"old"}"#;
        fs::write(&path, old).unwrap();

        let result = write_json_atomic_with_temp_path(
            &path,
            &json!({"generation": "new"}),
            true,
            &temp_path,
            |_, _| Err(io::Error::other("forced atomic replacement failure")),
        );

        assert!(result.is_err());
        assert_eq!(fs::read(&path).unwrap(), old);
        assert!(!temp_path.exists());
    }

    #[test]
    fn atomic_singleton_replacement_commits_one_complete_generation() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(SESSION_FILE);
        write_json_atomic_replace(&path, &json!({"generation": "old"})).unwrap();
        write_json_atomic_replace(&path, &json!({"generation": "new"})).unwrap();

        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(path).unwrap()).unwrap(),
            json!({"generation": "new"})
        );
    }

    #[test]
    fn session_snapshot_serialization_spans_the_real_commit_closure() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_automation_state(tmp.path());
        state.initialize_files().unwrap();
        let (at_commit_tx, at_commit_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let first_state = state.clone();
        let first = std::thread::spawn(move || {
            first_state
                .write_session_snapshot_with_commit_decision(|| {
                    at_commit_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(true)
                })
                .unwrap()
        });
        at_commit_rx.recv().unwrap();

        {
            let mut inventory = state.inner.window_inventory.lock().unwrap();
            inventory.sync(vec!["main".to_string(), "newer".to_string()]);
            inventory.mark_ready("newer");
        }
        let (second_started_tx, second_started_rx) = std::sync::mpsc::channel();
        let (second_done_tx, second_done_rx) = std::sync::mpsc::channel();
        let second_state = state.clone();
        let second = std::thread::spawn(move || {
            second_started_tx.send(()).unwrap();
            second_state.write_session_snapshot().unwrap();
            second_done_tx.send(()).unwrap();
        });
        second_started_rx.recv().unwrap();
        assert!(matches!(
            second_done_rx.recv_timeout(Duration::from_millis(100)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));

        release_tx.send(()).unwrap();
        assert!(first.join().unwrap());
        second_done_rx.recv().unwrap();
        second.join().unwrap();

        let session: UiAutomationSession =
            serde_json::from_slice(&fs::read(&state.inner.session_path).unwrap()).unwrap();
        assert!(session.window_labels.iter().any(|label| label == "newer"));
        assert!(session
            .ready_window_labels
            .iter()
            .any(|label| label == "newer"));
    }

    #[cfg(any(unix, target_os = "windows"))]
    #[test]
    fn bounded_read_rejects_precreated_and_swapped_reparse_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let foreign = tmp.path().join("foreign");
        fs::create_dir(&foreign).unwrap();
        let sentinel = foreign.join("sentinel.txt");
        fs::write(&sentinel, "untouched").unwrap();

        let precreated = tmp.path().join("precreated.json");
        create_test_reparse(&precreated, &foreign);
        assert!(read_bounded_regular_file(&precreated, 1_024).is_err());
        assert_eq!(fs::read_to_string(&sentinel).unwrap(), "untouched");
        remove_test_reparse(&precreated);

        let swapped = tmp.path().join("swapped.json");
        let opened_generation = tmp.path().join("opened-generation.json");
        fs::write(&swapped, "safe").unwrap();
        let result = read_bounded_regular_file_with_hook(&swapped, 1_024, || {
            fs::rename(&swapped, &opened_generation).unwrap();
            create_test_reparse(&swapped, &foreign);
        });
        assert!(result.is_err());
        assert_eq!(fs::read_to_string(&opened_generation).unwrap(), "safe");
        assert_eq!(fs::read_to_string(&sentinel).unwrap(), "untouched");
        remove_test_reparse(&swapped);
    }

    #[cfg(any(unix, target_os = "windows"))]
    #[test]
    fn atomic_temp_creation_rejects_a_precreated_reparse_without_touching_either_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(SESSION_FILE);
        let foreign = tmp.path().join("foreign-temp-target");
        fs::create_dir(&foreign).unwrap();
        let sentinel = foreign.join("sentinel.txt");
        fs::write(&sentinel, "untouched").unwrap();
        fs::write(&path, br#"{"generation":"old"}"#).unwrap();
        let temp_path = tmp.path().join("precreated.tmp");
        create_test_reparse(&temp_path, &foreign);
        let replacement_called = AtomicBool::new(false);

        let result = write_json_atomic_with_temp_path(
            &path,
            &json!({"generation": "new"}),
            true,
            &temp_path,
            |_, _| {
                replacement_called.store(true, Ordering::SeqCst);
                Ok(())
            },
        );

        assert!(result.is_err());
        assert!(!replacement_called.load(Ordering::SeqCst));
        assert_eq!(fs::read_to_string(&path).unwrap(), r#"{"generation":"old"}"#);
        assert_eq!(fs::read_to_string(&sentinel).unwrap(), "untouched");
        remove_test_reparse(&temp_path);
    }

    #[cfg(any(unix, target_os = "windows"))]
    #[test]
    fn atomic_commit_revalidates_a_rebound_source_inside_the_real_commit_closure() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(SESSION_FILE);
        let temp_path = tmp.path().join("retained-temp.tmp");
        let parked_path = tmp.path().join("parked-temp.json");
        let foreign = tmp.path().join("foreign-temp-swap");
        fs::create_dir(&foreign).unwrap();
        let sentinel = foreign.join("sentinel.txt");
        fs::write(&sentinel, "foreign-untouched").unwrap();
        fs::write(&path, br#"{"generation":"old"}"#).unwrap();
        let swap_was_denied = AtomicBool::new(false);
        let commit_decision_reached = AtomicBool::new(false);

        let result = write_json_atomic_with_temp_path_and_precommit(
            &path,
            &json!({"generation": "safe-new"}),
            true,
            &temp_path,
            |source,
             source_path,
             destination,
             replace,
             directories,
             generations,
             commit_decision| {
                let swap = fs::rename(&temp_path, &parked_path);
                #[cfg(target_os = "windows")]
                {
                    assert!(swap.is_err(), "retained source handle allowed a temp swap");
                    swap_was_denied.store(true, Ordering::SeqCst);
                }
                #[cfg(unix)]
                {
                    swap.unwrap();
                    create_test_reparse(&temp_path, &foreign);
                }
                atomic_commit_opened_temp(
                    source,
                    source_path,
                    destination,
                    replace,
                    directories,
                    generations,
                    commit_decision,
                )
            },
            || {
                commit_decision_reached.store(true, Ordering::SeqCst);
                Ok(true)
            },
        );

        #[cfg(target_os = "windows")]
        {
            assert!(result.unwrap());
            assert!(swap_was_denied.load(Ordering::SeqCst));
            assert!(commit_decision_reached.load(Ordering::SeqCst));
            assert_eq!(
                serde_json::from_slice::<Value>(&fs::read(&path).unwrap()).unwrap(),
                json!({"generation": "safe-new"})
            );
            assert!(!parked_path.exists());
        }
        #[cfg(unix)]
        {
            assert!(result.is_err());
            assert!(!commit_decision_reached.load(Ordering::SeqCst));
            assert_eq!(
                fs::read_to_string(&path).unwrap(),
                r#"{"generation":"old"}"#
            );
            assert_eq!(
                serde_json::from_slice::<Value>(&fs::read(&parked_path).unwrap()).unwrap(),
                json!({"generation": "safe-new"})
            );
            if temp_path.exists() {
                remove_test_reparse(&temp_path);
            }
        }
        assert_eq!(fs::read_to_string(&sentinel).unwrap(), "foreign-untouched");
    }

    #[test]
    fn atomic_commit_never_overwrites_a_later_destination_generation_inside_commit_closure() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(SESSION_FILE);
        let temp_path = tmp.path().join("retained-destination.tmp");
        let parked_path = tmp.path().join("parked-destination.json");
        let old = br#"{"generation":"old"}"#;
        let foreign = br#"{"generation":"foreign-untouched"}"#;
        fs::write(&path, old).unwrap();
        let commit_decision_reached = AtomicBool::new(false);

        let result = write_json_atomic_with_temp_path_and_precommit(
            &path,
            &json!({"generation": "safe-new"}),
            true,
            &temp_path,
            |source,
             source_path,
             destination,
             replace,
             directories,
             generations,
             commit_decision| {
                fs::rename(&path, &parked_path).unwrap();
                fs::write(&path, foreign).unwrap();
                atomic_commit_opened_temp(
                    source,
                    source_path,
                    destination,
                    replace,
                    directories,
                    generations,
                    commit_decision,
                )
            },
            || {
                commit_decision_reached.store(true, Ordering::SeqCst);
                Ok(true)
            },
        );

        assert!(result.is_err());
        assert!(!commit_decision_reached.load(Ordering::SeqCst));
        assert_eq!(fs::read(&parked_path).unwrap(), old);
        assert_eq!(fs::read(&path).unwrap(), foreign);
        assert!(!temp_path.exists());
    }

    #[cfg(any(unix, target_os = "windows"))]
    #[test]
    fn atomic_write_rejects_intermediate_automation_and_response_reparses() {
        let tmp = tempfile::tempdir().unwrap();
        let config = tmp.path().join("config");
        let foreign = tmp.path().join("foreign-mailbox");
        fs::create_dir(&config).unwrap();
        fs::create_dir(&foreign).unwrap();
        fs::create_dir(foreign.join(RESPONSES_DIR)).unwrap();
        let sentinel = foreign.join("sentinel.txt");
        fs::write(&sentinel, "foreign-untouched").unwrap();

        let automation_dir = config.join(UI_AUTOMATION_DIR);
        create_test_reparse(&automation_dir, &foreign);
        let through_automation = automation_dir
            .join(RESPONSES_DIR)
            .join(format!("{}.json", Uuid::new_v4()));
        assert!(write_json_atomic_new(&through_automation, &json!({"foreign": true})).is_err());
        assert_eq!(fs::read_to_string(&sentinel).unwrap(), "foreign-untouched");
        assert_eq!(
            fs::read_dir(foreign.join(RESPONSES_DIR)).unwrap().count(),
            0
        );
        remove_test_reparse(&automation_dir);

        fs::create_dir(&automation_dir).unwrap();
        fs::create_dir(automation_dir.join(REQUESTS_DIR)).unwrap();
        let responses = automation_dir.join(RESPONSES_DIR);
        create_test_reparse(&responses, &foreign);
        let through_responses = responses.join(format!("{}.json", Uuid::new_v4()));
        assert!(write_json_atomic_new(&through_responses, &json!({"foreign": true})).is_err());
        assert_eq!(fs::read_to_string(&sentinel).unwrap(), "foreign-untouched");
        assert_eq!(
            fs::read_dir(&foreign)
                .unwrap()
                .filter_map(Result::ok)
                .filter(
                    |entry| entry.file_name() != "responses" && entry.file_name() != "sentinel.txt"
                )
                .count(),
            0
        );
        remove_test_reparse(&responses);
    }

    #[test]
    fn session_read_retries_transient_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(SESSION_FILE);
        let writer_path = path.clone();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(SESSION_READ_RETRY_DELAY_MS));
            fs::write(writer_path, "{\"ok\":true}").unwrap();
        });

        let raw = read_session_file_with_retry(&path).unwrap();
        writer.join().unwrap();
        assert_eq!(raw, "{\"ok\":true}");
    }

    #[test]
    fn initialization_failure_can_disable_state() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_automation_state(tmp.path());
        fs::create_dir_all(&state.inner.automation_dir).unwrap();
        fs::write(state.requests_dir(), "not a directory").unwrap();

        assert!(state.initialize_files().is_err());
        state.mark_unavailable();
        assert!(!state.enabled());
    }

    #[test]
    fn cleanup_stale_automation_files_removes_request_and_response_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let automation_dir = tmp.path().join(UI_AUTOMATION_DIR);
        let requests_dir = automation_dir.join(REQUESTS_DIR);
        let responses_dir = automation_dir.join(RESPONSES_DIR);
        fs::create_dir_all(&requests_dir).unwrap();
        fs::create_dir_all(&responses_dir).unwrap();
        fs::write(requests_dir.join("stale.json"), "{}").unwrap();
        fs::write(responses_dir.join("stale.json"), "{}").unwrap();

        cleanup_stale_automation_files(&automation_dir).unwrap();

        assert!(!requests_dir.exists());
        assert!(!responses_dir.exists());
    }

    #[test]
    fn timeout_phase_uses_ready_window_labels_for_inflight() {
        let tmp = tempfile::tempdir().unwrap();
        let context = UiCliDispatchContext::new(Arc::new(TestAutomationConfigWitness {
            canonical_path: fs::canonicalize(tmp.path()).unwrap(),
        }));
        let request_path = tmp.path().join("request.json");
        let inflight_path = tmp.path().join("request.inflight.json");
        let session_path = tmp.path().join(SESSION_FILE);
        fs::write(&request_path, "{}").unwrap();
        assert_eq!(
            timeout_phase(&context, &request_path, &inflight_path, &session_path, "main"),
            Ok("awaiting_gui_poller".to_string())
        );

        fs::remove_file(&request_path).unwrap();
        fs::write(&inflight_path, "{}").unwrap();
        fs::write(
            &session_path,
            serde_json::to_string(&UiAutomationSession {
                schema_version: PROTOCOL_SCHEMA_VERSION,
                instance_id: Uuid::new_v4().to_string(),
                pid: std::process::id(),
                token: "token".to_string(),
                exe_path: current_exe_path_string(),
                config_dir: tmp.path().to_string_lossy().into_owned(),
                window_inventory: UiAutomationWindowInventory {
                    status: WindowInventoryStatus::Ready,
                    observed_count: 1,
                    limit: MAX_REGISTERED_WINDOWS as u32,
                },
                window_labels: vec!["main".to_string()],
                ready_window_labels: vec![],
                started_at_unix_ms: 1,
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            timeout_phase(&context, &request_path, &inflight_path, &session_path, "main"),
            Ok("awaiting_frontend_ready".to_string())
        );

        fs::write(
            &session_path,
            serde_json::to_string(&UiAutomationSession {
                schema_version: PROTOCOL_SCHEMA_VERSION,
                instance_id: Uuid::new_v4().to_string(),
                pid: std::process::id(),
                token: "token".to_string(),
                exe_path: current_exe_path_string(),
                config_dir: tmp.path().to_string_lossy().into_owned(),
                window_inventory: UiAutomationWindowInventory {
                    status: WindowInventoryStatus::Ready,
                    observed_count: 1,
                    limit: MAX_REGISTERED_WINDOWS as u32,
                },
                window_labels: vec!["main".to_string()],
                ready_window_labels: vec!["main".to_string()],
                started_at_unix_ms: 1,
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            timeout_phase(&context, &request_path, &inflight_path, &session_path, "main"),
            Ok("awaiting_frontend_response".to_string())
        );
    }
}
