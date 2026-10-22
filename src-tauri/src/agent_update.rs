//! Issue #1327 - startup coding-agent update flow.
//!
//! At GUI startup a detached task (spawned in `lib.rs` setup, BEFORE the restore
//! task is submitted) plans and runs the per-command `updateCommands` sequences
//! for every coding agent the user REGISTERED (`AppSettings.agents[].command`)
//! AND enabled through the per-command policy map
//! `AppSettings.agent_auto_update_by_command`. Commands never asked
//! about get a first-time SI/NO prompt (asked once, never again, default No).
//! Updates run in parallel ACROSS commands (each command's sequence stays
//! strictly ordered) with a splash overlay on the sidebar while they run, and a
//! sticky red error toast per failing command.
//!
//! Every retained target is probed before and after its updater sequence. The
//! updater still decides what work to perform; the two bounded probes report
//! whether the installed version changed without changing updater admission.
//!
//! `AgentUpdateGate` is the process-local blocker every session open waits on
//! (`create_session_inner_impl`, the single chokepoint). The gate is released by
//! an independently owned `PassSupervisor` only after every target has published
//! a terminal event and every process owner has proved native-tree settlement.
//! Dropping the public await cannot orphan the pass or release the gate early.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsStr;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::future::{BoxFuture, Shared};
use futures::FutureExt;
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::agent_version::{
    probe_version_cancellable_retained, probe_version_in, version_probe_args, AgentInstallCache,
    CancellableProbeOutcome, Completion, InstallState, InstallStatus, ProbeOutcome, ProbeTicket,
    RetainedProbeOwner, Scheduling, INSTALL_CACHE_TTL, PROBE_TIMEOUT,
};
use crate::config::agent_command::{
    is_bare_program_token, normalize_legacy_agent_command, resolve_program, resolve_program_in,
};
use crate::config::agent_path::{effective_search_path, warm_login_shell_path};
use crate::config::coding_agents_catalog::{
    load_catalog_for_settings, primary_project_root, CatalogUnavailable, CodingAgentDefinition,
};
use crate::config::settings::SettingsState;
use crate::web::broadcast::WsBroadcaster;
use crate::web::event_broadcast::broadcast_all;

/// Per-prompt cap. On expiry nothing is persisted (the question is asked again
/// next boot) and the command is skipped this boot.
const PROMPT_TIMEOUT: Duration = Duration::from_secs(60);
/// Per-update-command-step cap. On expiry the whole tree is killed and the step
/// is reported failed; startup continues.
const UPDATE_STEP_TIMEOUT: Duration = Duration::from_secs(300);
/// Captured output tail kept for logging (log-only, never sent to the UI).
const OUTPUT_TAIL_BYTES: usize = 8 * 1024;
const PROCESS_SETTLEMENT_WINDOW: Duration = Duration::from_secs(10);
const PROCESS_SETTLEMENT_POLL: Duration = Duration::from_millis(25);
const CANCELLATION_CLEANUP_ERROR: &str = "Cancellation cleanup did not complete.";

/// One command's update outcome, shown in the sidebar as a red toast on failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentUpdateOutcome {
    Succeeded,
    #[default]
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentUpdateChange {
    Changed,
    Unchanged,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentUpdateCancelDisposition {
    Requested,
    AlreadyRequested,
    AlreadyTerminal,
    NotInPass,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUpdateCancelResponse {
    pub command: String,
    pub disposition: AgentUpdateCancelDisposition,
}

impl Serialize for AgentUpdateCancelResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut state = serializer.serialize_struct("AgentUpdateCancelResponse", 2)?;
        state.serialize_field("command", &self.command)?;
        state.serialize_field("disposition", &self.disposition)?;
        state.end()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUpdateCancelAllResponse {
    pub requested: Vec<AgentUpdateCommandRef>,
    pub already_requested: Vec<AgentUpdateCommandRef>,
    pub already_terminal: Vec<AgentUpdateCommandRef>,
}

impl Serialize for AgentUpdateCancelAllResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // `serde_json::Value` uses its configured map order while Tauri emits
        // this typed response directly. Pin one explicit wire order shared by
        // both paths so their actual bytes match without test canonicalization.
        let mut state = serializer.serialize_struct("AgentUpdateCancelAllResponse", 3)?;
        state.serialize_field("alreadyRequested", &self.already_requested)?;
        state.serialize_field("alreadyTerminal", &self.already_terminal)?;
        state.serialize_field("requested", &self.requested)?;
        state.end()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUpdateCancellationChanged {
    pub cancel_requested: Vec<AgentUpdateCommandRef>,
    pub cancel_all_requested: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUpdateResult {
    pub command: String,
    pub label: String,
    pub ok: bool,
    #[serde(default)]
    pub outcome: AgentUpdateOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub install_before: Option<InstallState>,
    #[serde(default)]
    pub install_after: Option<InstallState>,
    #[serde(default)]
    pub change: AgentUpdateChange,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_error: Option<String>,
}

/// Snapshot of the whole startup run, served to a late-mounting sidebar.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUpdateStatus {
    pub in_progress: bool,
    /// The currently registered-but-unanswered prompt (the sequential prompt
    /// phase has AT MOST one), so a late mount restores a prompt that was
    /// emitted before its listeners registered. Never resurrects a dead prompt:
    /// `drop_pending`, `resolve_answer`, and `mark_finished` all clear it.
    pub prompt: Option<AgentUpdatePrompt>,
    pub results: Vec<AgentUpdateResult>,
    /// #1551 - commands whose update sequence is running, in start order.
    pub running: Vec<AgentUpdateCommandRef>,
    /// #1551 - policy recorded by the winning answer per prompted command this boot (resolving or late).
    pub answered: BTreeMap<String, bool>,
    /// #1551 - the agents of this boot's pass in pass order (catalog order); pruned of prompts
    /// answered No or expired; kept after the pass for the summary.
    pub nodes: Vec<AgentUpdateNode>,
    #[serde(default)]
    pub verifying: Vec<AgentUpdateCommandRef>,
    #[serde(default)]
    pub cancel_requested: Vec<AgentUpdateCommandRef>,
    #[serde(default)]
    pub cancel_all_requested: bool,
}

/// The pending SI/NO question for one command.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUpdatePrompt {
    pub command: String,
    pub label: String,
}

/// #1551 - the identity of one command of this boot's pass, carried by the
/// closure, skip, and running-set payloads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUpdateCommandRef {
    pub command: String,
    pub label: String,
}

/// #1551 - one agent of this boot's pass in pass order; install_before is the
/// pre-update probe result once it ran (never cached, seq 0).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUpdateNode {
    pub command: String,
    pub label: String,
    pub update_commands: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_before: Option<InstallState>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TargetWork {
    PreProbe,
    UpdaterStep(usize),
    PostProbe,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TargetPhase {
    Pending,
    Prompting,
    Probing,
    Starting(TargetWork, u64),
    SpawnCommitted(TargetWork, u64),
    Running(usize, u64),
    Verifying,
    Cleanup(TargetWork, u64),
    Terminal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TargetClaim {
    Open,
    CancellationReserved,
}

#[derive(Default)]
struct AsyncFence {
    open: AtomicBool,
    notify: tokio::sync::Notify,
}

impl AsyncFence {
    fn open(&self) {
        if !self.open.swap(true, Ordering::AcqRel) {
            self.notify.notify_waiters();
        }
    }

    fn is_open(&self) -> bool {
        self.open.load(Ordering::Acquire)
    }

    async fn wait(&self) {
        loop {
            let notified = self.notify.notified();
            if self.is_open() {
                return;
            }
            notified.await;
        }
    }
}

/// Opens response-release fences on normal return and on future cancellation.
/// A disconnected transport therefore cannot strand a terminalizer forever
/// after it reserved cancellation but before it returned a response.
struct ResponseReleaseGuard {
    fences: Vec<Arc<AsyncFence>>,
}

impl ResponseReleaseGuard {
    fn new(fences: Vec<Arc<AsyncFence>>) -> Self {
        Self { fences }
    }

    fn open_all(&mut self) {
        for fence in self.fences.drain(..) {
            fence.open();
        }
    }

    fn release(mut self) {
        self.open_all();
    }
}

impl Drop for ResponseReleaseGuard {
    fn drop(&mut self) {
        self.open_all();
    }
}

struct TargetControl {
    command_ref: AgentUpdateCommandRef,
    phase: TargetPhase,
    claim: TargetClaim,
    next_epoch: u64,
    cancel: tokio::sync::watch::Sender<bool>,
    acceptance: Arc<AsyncFence>,
    response_release: Arc<AsyncFence>,
    terminal_publication: Arc<AsyncFence>,
    terminal_emitted: bool,
    supervisor_failure: bool,
    #[cfg(test)]
    step_admission_hook: Option<StepAdmissionTestHook>,
    #[cfg(test)]
    response_release_hook: Option<ResponseReleaseTestHook>,
    #[cfg(test)]
    post_probe_settled_hook: Option<PostProbeSettledTestHook>,
}

#[cfg(test)]
struct StepAdmissionTestHook {
    after_begin: Option<tokio::sync::oneshot::Sender<()>>,
    release_commit: Option<tokio::sync::oneshot::Receiver<()>>,
    after_commit: Option<tokio::sync::oneshot::Sender<()>>,
    release_spawn: Option<tokio::sync::oneshot::Receiver<()>>,
}

#[cfg(test)]
struct ResponseReleaseTestHook {
    ready: Option<tokio::sync::oneshot::Sender<()>>,
    release: Option<tokio::sync::oneshot::Receiver<()>>,
}

#[cfg(test)]
struct PostProbeSettledTestHook {
    ready: Option<tokio::sync::oneshot::Sender<()>>,
    release: Option<tokio::sync::oneshot::Receiver<()>>,
}

impl TargetControl {
    fn new(command_ref: AgentUpdateCommandRef) -> Self {
        let (cancel, _receiver) = tokio::sync::watch::channel(false);
        Self {
            command_ref,
            phase: TargetPhase::Pending,
            claim: TargetClaim::Open,
            next_epoch: 0,
            cancel,
            acceptance: Arc::new(AsyncFence::default()),
            response_release: Arc::new(AsyncFence::default()),
            terminal_publication: Arc::new(AsyncFence::default()),
            terminal_emitted: false,
            supervisor_failure: false,
            #[cfg(test)]
            step_admission_hook: None,
            #[cfg(test)]
            response_release_hook: None,
            #[cfg(test)]
            post_probe_settled_hook: None,
        }
    }
}

enum StepAdmission {
    Cancelled,
    Committed {
        epoch: u64,
        cancel: tokio::sync::watch::Receiver<bool>,
    },
}

enum TerminalPublish {
    Published,
    CancellationReserved,
    AlreadyTerminal,
}

/// Process-local gate: blocks every session open until the startup update run
/// finishes or times out. Managed as `Arc<AgentUpdateGate>`.
pub struct AgentUpdateGate {
    state: Mutex<GateState>,
    release: tokio::sync::Notify,
    /// #1551 - serializes `answer_prompt` (classify -> persist -> settle) per gate.
    /// A tokio mutex because it is held across the persist await; the std `state`
    /// lock is never held across an await.
    answer_serial: tokio::sync::Mutex<()>,
    #[cfg(test)]
    wait_registered: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

/// #1551 - a registered-but-unanswered prompt: the answer channel plus the label
/// the closure event has to carry.
struct PendingPrompt {
    tx: tokio::sync::oneshot::Sender<bool>,
    label: String,
}

struct GateState {
    started: bool,
    finished: bool,
    results: Vec<AgentUpdateResult>,
    /// Commands prompted this boot (answer validity + late-answer persistence).
    prompted: HashSet<String>,
    /// Registered-but-unanswered prompts, keyed by command.
    pending: HashMap<String, PendingPrompt>,
    /// Currently registered-but-unanswered prompt, for the snapshot.
    pending_prompt: Option<AgentUpdatePrompt>,
    /// #1551 - commands whose update sequence is running, in start order.
    running: Vec<AgentUpdateCommandRef>,
    /// #1551 - the policy the winning answer recorded, per prompted command.
    answered: BTreeMap<String, bool>,
    /// #1551 - this boot's pass, in pass order (catalog order).
    nodes: Vec<AgentUpdateNode>,
    controls: HashMap<String, TargetControl>,
    cancel_all_requested: bool,
}

/// #1551 - where one prompted command stands, as `answer_prompt` classifies it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptState {
    NotPrompted,
    Pending,
    Expired,
    Cancelled,
    Answered(bool),
}

/// #1551 - a pending prompt taken out of the gate by an answer. Its sender is
/// handed back UNSENT so the caller can emit the closure BEFORE releasing the
/// prompt loop.
pub struct ClaimedPrompt {
    closed: AgentUpdateCommandRef,
    tx: tokio::sync::oneshot::Sender<bool>,
    skipped: Option<AgentUpdateCommandRef>,
}

impl ClaimedPrompt {
    pub fn closed(&self) -> &AgentUpdateCommandRef {
        &self.closed
    }

    pub fn skipped(&self) -> Option<&AgentUpdateCommandRef> {
        self.skipped.as_ref()
    }

    /// The ONLY way to release the prompt loop from a claim; consumed exactly
    /// once. `false` iff the receiver is already gone (the prompt timed out
    /// while this answer was persisting).
    pub fn deliver(self, enabled: bool) -> bool {
        self.tx.send(enabled).is_ok()
    }
}

/// #1551 - the outcome of recording an answer inside the gate's serial section.
pub enum AnswerClaim {
    Claimed(ClaimedPrompt),
    Recorded,
}

impl AgentUpdateGate {
    pub fn new() -> Self {
        AgentUpdateGate {
            state: Mutex::new(GateState {
                started: false,
                finished: false,
                results: Vec::new(),
                prompted: HashSet::new(),
                pending: HashMap::new(),
                pending_prompt: None,
                running: Vec::new(),
                answered: BTreeMap::new(),
                nodes: Vec::new(),
                controls: HashMap::new(),
                cancel_all_requested: false,
            }),
            release: tokio::sync::Notify::new(),
            answer_serial: tokio::sync::Mutex::new(()),
            #[cfg(test)]
            wait_registered: Mutex::new(None),
        }
    }

    pub fn mark_started(&self) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).started = true;
    }

    /// Register the prompt for `command` and return the answer channel. The
    /// command is recorded as prompted (so a late answer still persists) and
    /// becomes the snapshot's pending prompt.
    pub fn register_prompt(
        &self,
        command: &str,
        label: &str,
    ) -> tokio::sync::oneshot::Receiver<bool> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        debug_assert!(
            !state.pending.contains_key(command),
            "duplicate prompt for {command}"
        );
        state.prompted.insert(command.to_string());
        let (tx, rx) = tokio::sync::oneshot::channel();
        state.pending.insert(
            command.to_string(),
            PendingPrompt {
                tx,
                label: label.to_string(),
            },
        );
        state.pending_prompt = Some(AgentUpdatePrompt {
            command: command.to_string(),
            label: label.to_string(),
        });
        if let Some(control) = state.controls.get_mut(command) {
            if control.claim == TargetClaim::Open && control.phase != TargetPhase::Terminal {
                control.phase = TargetPhase::Prompting;
            }
        }
        rx
    }

    /// #1551 - the single removal of a pending entry: takes it out of the map and
    /// clears the snapshot's prompt when it names `command`. Every remover
    /// (`drop_pending`, `resolve_answer`, `claim_answer`) goes through here, so
    /// the entry has exactly one owner.
    fn take_pending(state: &mut GateState, command: &str) -> Option<PendingPrompt> {
        let taken = state.pending.remove(command);
        if state
            .pending_prompt
            .as_ref()
            .is_some_and(|p| p.command == command)
        {
            state.pending_prompt = None;
        }
        taken
    }

    /// Timeout path: the prompt expired without an answer. Clears the pending
    /// prompt so a snapshot cannot resurrect it. #1551: returns the closed
    /// prompt's reference iff THIS call removed the entry, and the caller then
    /// owns the single `agent_update_prompt_closed` emission for that prompt.
    pub fn drop_pending(&self, command: &str) -> Option<AgentUpdateCommandRef> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        Self::take_pending(&mut state, command).map(|pending| AgentUpdateCommandRef {
            command: command.to_string(),
            label: pending.label,
        })
    }

    /// Deliver the user's answer. Returns `true` ONLY when a live receiver
    /// accepted the answer (the update runs this boot); `false` when nothing
    /// was pending (late answer) or the receiver was dropped by the prompt
    /// timeout (round-3 F1 pin: a dead receiver must never report "applied
    /// this boot").
    ///
    /// #1551: kept for its pinned semantics; production answers go through
    /// `answer_prompt`, which records the policy and owns the closure.
    pub fn resolve_answer(&self, command: &str, enabled: bool) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(pending) = Self::take_pending(&mut state, command) else {
            return false;
        };
        pending.tx.send(enabled).is_ok()
    }

    /// #1551 - one lock take, never sends. Records the persisted policy and, when
    /// the prompt is still pending, CLAIMS it: the entry leaves the map here, so
    /// the closure has exactly one owner, and the sender is handed back unsent so
    /// the caller can emit the closure BEFORE releasing the loop. It records
    /// UNCONDITIONALLY, which is why only `answer_prompt` may call it in
    /// production, after classifying inside the serial section.
    pub fn claim_answer(&self, command: &str, enabled: bool) -> AnswerClaim {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let taken = Self::take_pending(&mut state, command);
        state.answered.insert(command.to_string(), enabled);
        match taken {
            Some(pending) => {
                let closed = AgentUpdateCommandRef {
                    command: command.to_string(),
                    label: pending.label,
                };
                let skipped = if enabled {
                    if let Some(control) = state.controls.get_mut(command) {
                        control.phase = TargetPhase::Pending;
                    }
                    None
                } else {
                    state.controls.remove(command);
                    state
                        .nodes
                        .iter()
                        .position(|node| node.command == command)
                        .map(|index| {
                            let node = state.nodes.remove(index);
                            AgentUpdateCommandRef {
                                command: node.command,
                                label: node.label,
                            }
                        })
                };
                AnswerClaim::Claimed(ClaimedPrompt {
                    closed,
                    tx: pending.tx,
                    skipped,
                })
            }
            None => AnswerClaim::Recorded,
        }
    }

    /// #1551 - classification for `answer_prompt`, one lock take. `answered` is
    /// checked first: once a policy is recorded, no further answer may persist.
    pub fn prompt_state(&self, command: &str) -> PromptState {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(value) = state.answered.get(command) {
            return PromptState::Answered(*value);
        }
        if state.pending.contains_key(command) {
            return PromptState::Pending;
        }
        if state.prompted.contains(command) {
            if state.controls.get(command).is_some_and(|control| {
                control.claim == TargetClaim::CancellationReserved
                    || control.phase == TargetPhase::Terminal
            }) {
                return PromptState::Cancelled;
            }
            return PromptState::Expired;
        }
        PromptState::NotPrompted
    }

    /// #1551 - one command's update sequence starts: it joins `running` (once),
    /// its node records the pre-update install state, and the node is returned as
    /// the `agent_update_command_started` payload. A command with no node (only
    /// reachable from a test) yields a synthesized node that is NOT inserted.
    pub fn mark_command_started(
        &self,
        command: &str,
        label: &str,
        install_before: Option<InstallState>,
    ) -> AgentUpdateNode {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.running.iter().any(|entry| entry.command == command) {
            state.running.push(AgentUpdateCommandRef {
                command: command.to_string(),
                label: label.to_string(),
            });
        }
        if let Some(node) = state.nodes.iter_mut().find(|node| node.command == command) {
            if install_before.is_some() {
                node.install_before = install_before;
            }
            return node.clone();
        }
        AgentUpdateNode {
            command: command.to_string(),
            label: label.to_string(),
            update_commands: Vec::new(),
            install_before,
        }
    }

    /// #1551 - one command's update sequence ended: it leaves `running` and its
    /// result is upserted, in ONE lock take, so no snapshot can observe a command
    /// in both `running` and `results`.
    pub fn mark_command_finished(&self, result: AgentUpdateResult) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let command = result.command.clone();
        state.running.retain(|entry| entry.command != command);
        if let Some(existing) = state
            .results
            .iter_mut()
            .find(|existing| existing.command == command)
        {
            *existing = result;
        } else {
            state.results.push(result);
        }
        if let Some(control) = state.controls.get_mut(&command) {
            control.phase = TargetPhase::Terminal;
        }
    }

    /// #1551 - round 5: the pass starts with its node set in ONE lock take, so no
    /// snapshot can observe started without nodes.
    pub fn mark_started_with_nodes(&self, nodes: Vec<AgentUpdateNode>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.started = true;
        state.cancel_all_requested = false;
        state.controls = nodes
            .iter()
            .map(|node| {
                let command_ref = AgentUpdateCommandRef {
                    command: node.command.clone(),
                    label: node.label.clone(),
                };
                (node.command.clone(), TargetControl::new(command_ref))
            })
            .collect();
        state.nodes = nodes;
    }

    /// #1551 - round 5: a prompted target leaves the pass (answer No, or expiry).
    /// Returns the ref iff THIS call removed the node, so the loop emits
    /// `agent_update_command_skipped` exactly once per skipped command.
    pub fn mark_command_skipped(&self, command: &str) -> Option<AgentUpdateCommandRef> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let index = state
            .nodes
            .iter()
            .position(|node| node.command == command)?;
        let node = state.nodes.remove(index);
        state.controls.remove(command);
        Some(AgentUpdateCommandRef {
            command: node.command,
            label: node.label,
        })
    }

    /// #1551 - true once `mark_finished` ran: at the end of a real pass, or at the
    /// quiet-boot guard drop. Never reset.
    pub fn is_finished(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .finished
    }

    pub fn was_prompted(&self, command: &str) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .prompted
            .contains(command)
    }

    /// Store the final pass snapshot while admission remains closed. The sole
    /// supervisor calls this before emitting `agent_updates_finished`.
    fn store_final_results(&self, results: Vec<AgentUpdateResult>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.pending.clear();
        state.pending_prompt = None;
        state.running.clear();
        state.results = results;
    }

    /// Atomically open terminal fences and make finished visible, then notify
    /// admission waiters. This is intentionally separate from result storage so
    /// the pass-finished event is enqueued before any waiter can return.
    fn release_finished(&self) {
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            for control in state.controls.values_mut() {
                control.phase = TargetPhase::Terminal;
                control.terminal_publication.open();
            }
            state.finished = true;
        }
        self.release.notify_waiters();
    }

    /// Legacy/test convenience for callers that do not emit a pass-finished
    /// event. Production finalization uses the split store/emit/release sequence.
    pub fn mark_finished(&self, results: Vec<AgentUpdateResult>) {
        self.store_final_results(results);
        self.release_finished();
    }

    pub fn snapshot(&self) -> AgentUpdateStatus {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let ordered_controls = state
            .nodes
            .iter()
            .filter_map(|node| state.controls.get(&node.command));
        let verifying = ordered_controls
            .clone()
            .filter(|control| {
                matches!(
                    control.phase,
                    TargetPhase::Verifying | TargetPhase::Cleanup(TargetWork::PostProbe, _)
                )
            })
            .map(|control| control.command_ref.clone())
            .collect();
        let cancel_requested = ordered_controls
            .filter(|control| {
                control.claim == TargetClaim::CancellationReserved
                    && control.phase != TargetPhase::Terminal
            })
            .map(|control| control.command_ref.clone())
            .collect();
        AgentUpdateStatus {
            in_progress: state.started && !state.finished,
            prompt: state.pending_prompt.clone(),
            results: state.results.clone(),
            running: state.running.clone(),
            answered: state.answered.clone(),
            nodes: state.nodes.clone(),
            verifying,
            cancel_requested,
            cancel_all_requested: state.cancel_all_requested,
        }
    }

    fn begin_step(
        &self,
        command: &str,
        work: TargetWork,
    ) -> Option<(u64, tokio::sync::watch::Receiver<bool>)> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let control = state.controls.get_mut(command)?;
        if control.phase == TargetPhase::Terminal {
            return None;
        }
        if control.claim == TargetClaim::CancellationReserved {
            control.acceptance.open();
            return None;
        }
        control.next_epoch += 1;
        let epoch = control.next_epoch;
        control.phase = TargetPhase::Starting(work, epoch);
        Some((epoch, control.cancel.subscribe()))
    }

    fn commit_step(&self, command: &str, work: TargetWork, epoch: u64) -> StepAdmission {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(control) = state.controls.get_mut(command) else {
            return StepAdmission::Cancelled;
        };
        if control.claim == TargetClaim::CancellationReserved {
            control.acceptance.open();
            return StepAdmission::Cancelled;
        }
        if control.phase != TargetPhase::Starting(work, epoch) {
            return StepAdmission::Cancelled;
        }
        control.phase = TargetPhase::SpawnCommitted(work, epoch);
        StepAdmission::Committed {
            epoch,
            cancel: control.cancel.subscribe(),
        }
    }

    #[cfg(test)]
    async fn hold_after_step_begin_for_test(&self, command: &str) {
        let (ready, release) = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            let hook = state
                .controls
                .get_mut(command)
                .and_then(|control| control.step_admission_hook.as_mut());
            match hook {
                Some(hook) => (hook.after_begin.take(), hook.release_commit.take()),
                None => (None, None),
            }
        };
        if let Some(ready) = ready {
            let _ = ready.send(());
        }
        if let Some(release) = release {
            let _ = release.await;
        }
    }

    #[cfg(test)]
    async fn hold_after_step_commit_for_test(&self, command: &str) {
        let (ready, release) = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            let hook = state
                .controls
                .get_mut(command)
                .and_then(|control| control.step_admission_hook.as_mut());
            match hook {
                Some(hook) => (hook.after_commit.take(), hook.release_spawn.take()),
                None => (None, None),
            }
        };
        if let Some(ready) = ready {
            let _ = ready.send(());
        }
        if let Some(release) = release {
            let _ = release.await;
        }
    }

    #[cfg(test)]
    async fn hold_response_release_for_test(&self, command: &str) {
        let (ready, release) = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            let hook = state
                .controls
                .get_mut(command)
                .and_then(|control| control.response_release_hook.as_mut());
            match hook {
                Some(hook) => (hook.ready.take(), hook.release.take()),
                None => (None, None),
            }
        };
        if let Some(ready) = ready {
            let _ = ready.send(());
        }
        if let Some(release) = release {
            let _ = release.await;
        }
    }

    #[cfg(test)]
    async fn hold_after_post_probe_settled_for_test(&self, command: &str) {
        let (ready, release) = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            let hook = state
                .controls
                .get_mut(command)
                .and_then(|control| control.post_probe_settled_hook.as_mut());
            match hook {
                Some(hook) => (hook.ready.take(), hook.release.take()),
                None => (None, None),
            }
        };
        if let Some(ready) = ready {
            let _ = ready.send(());
        }
        if let Some(release) = release {
            let _ = release.await;
        }
    }

    fn owner_installed(&self, command: &str, work: TargetWork, epoch: u64) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        {
            let Some(control) = state.controls.get_mut(command) else {
                return;
            };
            if control.phase != TargetPhase::SpawnCommitted(work, epoch) {
                return;
            }
            control.phase = match work {
                TargetWork::PreProbe => TargetPhase::Probing,
                TargetWork::UpdaterStep(step) => TargetPhase::Running(step, epoch),
                TargetWork::PostProbe => TargetPhase::Verifying,
            };
        }
        if work == TargetWork::PostProbe {
            state.running.retain(|entry| entry.command != command);
        }
    }

    fn owner_observed_cancellation(&self, command: &str, work: TargetWork, epoch: u64) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(control) = state.controls.get_mut(command) {
            if control.claim == TargetClaim::CancellationReserved {
                control.phase = TargetPhase::Cleanup(work, epoch);
                control.acceptance.open();
            }
        }
    }

    /// Complete the committed-to-owner handoff when spawn/resolve returned
    /// without ever constructing a process owner. This is deliberately distinct
    /// from `owner_installed`/`owner_returned`: a rejected launch must remain in
    /// `SpawnCommitted` and must not become visibly running/verifying.
    fn completed_without_owner(&self, command: &str, work: TargetWork, epoch: u64) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(control) = state.controls.get_mut(command) else {
            return false;
        };
        if control.phase != TargetPhase::SpawnCommitted(work, epoch) {
            return control.claim == TargetClaim::CancellationReserved;
        }
        if control.claim == TargetClaim::CancellationReserved {
            control.phase = TargetPhase::Cleanup(work, epoch);
            control.acceptance.open();
            true
        } else {
            false
        }
    }

    fn owner_returned(&self, command: &str, work: TargetWork, epoch: u64) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(control) = state.controls.get_mut(command) else {
            return false;
        };
        if control.claim == TargetClaim::CancellationReserved {
            control.phase = TargetPhase::Cleanup(work, epoch);
            control.acceptance.open();
            true
        } else {
            false
        }
    }

    fn cancellation_reserved(&self, command: &str) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .controls
            .get(command)
            .is_some_and(|control| control.claim == TargetClaim::CancellationReserved)
    }

    fn acknowledge_unowned_cancellation(&self, command: &str) {
        if let Some(control) = self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .controls
            .get(command)
        {
            if control.claim == TargetClaim::CancellationReserved {
                control.acceptance.open();
            }
        }
    }

    async fn wait_response_release_if_cancelled(&self, command: &str) {
        let fence = self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .controls
            .get(command)
            .filter(|control| control.claim == TargetClaim::CancellationReserved)
            .map(|control| Arc::clone(&control.response_release));
        if let Some(fence) = fence {
            fence.wait().await;
        }
    }

    fn publish_terminal(&self, result: AgentUpdateResult, allow_reserved: bool) -> TerminalPublish {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let command = result.command.clone();
        let Some(control) = state.controls.get(&command) else {
            return TerminalPublish::AlreadyTerminal;
        };
        if control.phase == TargetPhase::Terminal {
            return TerminalPublish::AlreadyTerminal;
        }
        if control.claim == TargetClaim::CancellationReserved && !allow_reserved {
            return TerminalPublish::CancellationReserved;
        }
        debug_assert!(
            control.claim != TargetClaim::CancellationReserved
                || control.response_release.is_open(),
            "a cancellation terminal result cannot precede its response release"
        );
        state.running.retain(|entry| entry.command != command);
        if let Some(existing) = state
            .results
            .iter_mut()
            .find(|existing| existing.command == command)
        {
            *existing = result;
        } else {
            state.results.push(result);
        }
        let order: HashMap<String, usize> = state
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (node.command.clone(), index))
            .collect();
        state.results.sort_by_key(|result| {
            order
                .get(result.command.as_str())
                .copied()
                .unwrap_or(usize::MAX)
        });
        if let Some(control) = state.controls.get_mut(&command) {
            control.phase = TargetPhase::Terminal;
        }
        TerminalPublish::Published
    }

    fn command_event_emitted(&self, command: &str) {
        if let Some(control) = self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .controls
            .get_mut(command)
        {
            if !control.terminal_emitted {
                control.terminal_emitted = true;
                control.terminal_publication.open();
            }
        }
    }

    async fn wait_all_terminal_publications(&self) {
        let fences: Vec<Arc<AsyncFence>> = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state
                .nodes
                .iter()
                .filter_map(|node| state.controls.get(&node.command))
                .map(|control| Arc::clone(&control.terminal_publication))
                .collect()
        };
        for fence in fences {
            fence.wait().await;
        }
    }

    fn supervisor_failure_reserved(&self, command: &str) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .controls
            .get(command)
            .is_some_and(|control| control.supervisor_failure)
    }

    fn reserve_supervisor_failure(&self) -> Option<AgentUpdateCommandRef> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let commands: Vec<String> = state
            .nodes
            .iter()
            .map(|node| node.command.clone())
            .collect();
        for command in commands {
            if let Some(control) = state.controls.get_mut(&command) {
                if control.phase == TargetPhase::Terminal {
                    continue;
                }
                control.supervisor_failure = true;
                let reserved_here = control.claim == TargetClaim::Open;
                if reserved_here {
                    control.claim = TargetClaim::CancellationReserved;
                }
                let _ = control.cancel.send(true);
                // Only a supervisor-created reservation has no external RPC
                // response owner. A live row/batch reservation keeps this fence
                // until its ResponseReleaseGuard reaches the return tail.
                if reserved_here {
                    control.response_release.open();
                }
                if matches!(
                    control.phase,
                    TargetPhase::Pending | TargetPhase::Prompting | TargetPhase::Starting(_, _)
                ) {
                    control.acceptance.open();
                }
            }
        }
        let pending_command = state
            .pending_prompt
            .as_ref()
            .map(|prompt| prompt.command.clone());
        pending_command.and_then(|command| {
            Self::take_pending(&mut state, &command).map(|pending| AgentUpdateCommandRef {
                command,
                label: pending.label,
            })
        })
    }

    fn unfinished_nodes(&self) -> Vec<AgentUpdateNode> {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state
            .nodes
            .iter()
            .filter(|node| {
                state
                    .controls
                    .get(&node.command)
                    .is_some_and(|control| control.phase != TargetPhase::Terminal)
            })
            .cloned()
            .collect()
    }

    /// Await gate release. Race-free for late waiters: the notification is
    /// registered BEFORE the finished check, so a `mark_finished` between the
    /// two cannot be missed.
    pub async fn wait_until_done(&self) {
        loop {
            let notified = self.release.notified();
            #[cfg(test)]
            if let Some(registered) = self
                .wait_registered
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take()
            {
                let _ = registered.send(());
            }
            if self
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .finished
            {
                return;
            }
            notified.await;
        }
    }
}

impl Default for AgentUpdateGate {
    fn default() -> Self {
        Self::new()
    }
}

fn cancellation_snapshot(state: &GateState) -> AgentUpdateCancellationChanged {
    AgentUpdateCancellationChanged {
        cancel_requested: state
            .nodes
            .iter()
            .filter_map(|node| state.controls.get(&node.command))
            .filter(|control| {
                control.claim == TargetClaim::CancellationReserved
                    && control.phase != TargetPhase::Terminal
            })
            .map(|control| control.command_ref.clone())
            .collect(),
        cancel_all_requested: state.cancel_all_requested,
    }
}

pub async fn cancel_update(
    app: &AppHandle,
    gate: &AgentUpdateGate,
    command: String,
) -> AgentUpdateCancelResponse {
    let serial = gate.answer_serial.lock().await;
    let mut acceptance = None;
    let mut response_release = None;
    let mut terminal_publication = None;
    let mut closed_prompt = None;
    let mut changed = None;
    let disposition = {
        let mut state = gate.state.lock().unwrap_or_else(|error| error.into_inner());
        let Some((phase, claim)) = state
            .controls
            .get(&command)
            .map(|control| (control.phase, control.claim))
        else {
            drop(state);
            drop(serial);
            return AgentUpdateCancelResponse {
                command,
                disposition: AgentUpdateCancelDisposition::NotInPass,
            };
        };
        if phase == TargetPhase::Terminal {
            terminal_publication = state
                .controls
                .get(&command)
                .map(|control| Arc::clone(&control.terminal_publication));
            AgentUpdateCancelDisposition::AlreadyTerminal
        } else if claim == TargetClaim::CancellationReserved {
            if let Some(control) = state.controls.get(&command) {
                acceptance = Some(Arc::clone(&control.acceptance));
                response_release = Some(Arc::clone(&control.response_release));
            }
            AgentUpdateCancelDisposition::AlreadyRequested
        } else {
            if let Some(control) = state.controls.get_mut(&command) {
                control.claim = TargetClaim::CancellationReserved;
                let _ = control.cancel.send(true);
                if matches!(
                    phase,
                    TargetPhase::Pending | TargetPhase::Prompting | TargetPhase::Starting(_, _)
                ) {
                    control.acceptance.open();
                }
                acceptance = Some(Arc::clone(&control.acceptance));
                response_release = Some(Arc::clone(&control.response_release));
            }
            if let Some(pending) = AgentUpdateGate::take_pending(&mut state, &command) {
                closed_prompt = Some(AgentUpdateCommandRef {
                    command: command.clone(),
                    label: pending.label,
                });
                drop(pending.tx);
            }
            changed = Some(cancellation_snapshot(&state));
            AgentUpdateCancelDisposition::Requested
        }
    };

    let release_guard = ResponseReleaseGuard::new(response_release.into_iter().collect());

    if let Some(closed) = closed_prompt {
        emit_all(app, "agent_update_prompt_closed", json!(closed));
    }
    if let Some(snapshot) = changed {
        emit_all(app, "agent_update_cancellation_changed", json!(snapshot));
    }
    drop(serial);

    if let Some(fence) = acceptance {
        fence.wait().await;
    }
    if let Some(fence) = terminal_publication {
        fence.wait().await;
    }
    #[cfg(test)]
    gate.hold_response_release_for_test(&command).await;
    release_guard.release();
    AgentUpdateCancelResponse {
        command,
        disposition,
    }
}

pub async fn cancel_all_updates(
    app: &AppHandle,
    gate: &AgentUpdateGate,
) -> AgentUpdateCancelAllResponse {
    let serial = gate.answer_serial.lock().await;
    let mut requested = Vec::new();
    let mut already_requested = Vec::new();
    let mut already_terminal = Vec::new();
    let mut requested_waits = Vec::new();
    let mut already_waits = Vec::new();
    let mut terminal_waits = Vec::new();
    let mut closed_prompt = None;
    let changed = {
        let mut state = gate.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.nodes.is_empty() {
            drop(state);
            drop(serial);
            return AgentUpdateCancelAllResponse {
                requested,
                already_requested,
                already_terminal,
            };
        }
        let was_latched = state.cancel_all_requested;
        state.cancel_all_requested = true;
        let commands: Vec<String> = state
            .nodes
            .iter()
            .map(|node| node.command.clone())
            .collect();
        let mut prompt_to_close = None;
        for command in commands {
            let Some((phase, claim, command_ref)) = state
                .controls
                .get(&command)
                .map(|control| (control.phase, control.claim, control.command_ref.clone()))
            else {
                continue;
            };
            if phase == TargetPhase::Terminal {
                already_terminal.push(command_ref);
                if let Some(control) = state.controls.get(&command) {
                    terminal_waits.push(Arc::clone(&control.terminal_publication));
                }
            } else if claim == TargetClaim::CancellationReserved {
                already_requested.push(command_ref);
                if let Some(control) = state.controls.get(&command) {
                    already_waits.push((
                        Arc::clone(&control.acceptance),
                        Arc::clone(&control.response_release),
                    ));
                }
            } else if let Some(control) = state.controls.get_mut(&command) {
                control.claim = TargetClaim::CancellationReserved;
                let _ = control.cancel.send(true);
                if matches!(
                    phase,
                    TargetPhase::Pending | TargetPhase::Prompting | TargetPhase::Starting(_, _)
                ) {
                    control.acceptance.open();
                }
                requested_waits.push((
                    Arc::clone(&control.acceptance),
                    Arc::clone(&control.response_release),
                ));
                requested.push(command_ref);
                if phase == TargetPhase::Prompting {
                    prompt_to_close = Some(command.clone());
                }
            }
        }
        if let Some(command) = prompt_to_close {
            if let Some(pending) = AgentUpdateGate::take_pending(&mut state, &command) {
                closed_prompt = Some(AgentUpdateCommandRef {
                    command,
                    label: pending.label,
                });
                drop(pending.tx);
            }
        }
        if !was_latched || !requested.is_empty() {
            Some(cancellation_snapshot(&state))
        } else {
            None
        }
    };

    let release_guard = ResponseReleaseGuard::new(
        requested_waits
            .iter()
            .chain(already_waits.iter())
            .map(|(_, release)| Arc::clone(release))
            .collect(),
    );

    if let Some(closed) = closed_prompt {
        emit_all(app, "agent_update_prompt_closed", json!(closed));
    }
    if let Some(snapshot) = changed {
        emit_all(app, "agent_update_cancellation_changed", json!(snapshot));
    }
    drop(serial);

    for (acceptance, _) in &requested_waits {
        acceptance.wait().await;
    }
    for (acceptance, _) in &already_waits {
        acceptance.wait().await;
    }
    for terminal in terminal_waits {
        terminal.wait().await;
    }
    release_guard.release();
    AgentUpdateCancelAllResponse {
        requested,
        already_requested,
        already_terminal,
    }
}

/// One command's update run: the shell sequences to execute and where.
#[derive(Debug, Clone)]
pub struct UpdateTarget {
    pub command: String,
    pub label: String,
    pub commands: Vec<String>,
    pub cwd: PathBuf,
}

/// The startup plan: what to prompt about and what to run.
pub struct AgentUpdatePlan {
    pub prompts: Vec<UpdateTarget>,
    pub updates: Vec<UpdateTarget>,
}

/// #1551 - one row of the read-only Settings "Auto-update" table.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUpdateOverviewRow {
    pub key: String,
    pub label: String,
    pub command: String,
    pub color: String,
    pub update_commands: Vec<String>,
    pub install: InstallState,
}

/// Pure plan builder. Binding rules:
/// 0. Only commands in `registered_commands` (the command strings of
///    `settings.agents[]`, i.e. agents the user actually registered) are
///    considered: a catalog-only command is never prompted nor updated.
///    Zero registered commands -> empty plan.
/// 1. The FIRST catalog entry for a command is the effective entry (#1967):
///    distinct commands keep catalog order, and `label`/`commands` come from
///    that first entry. A later duplicate never contributes anything.
/// 2. An empty effective sequence means no prompt and no update: the command is
///    skipped entirely. Nothing is borrowed from a later duplicate, so a
///    non-empty duplicate can never re-enable an empty winner.
/// 3. `answers[command]` absent -> prompt (default No on timeout);
///    `Some(true)` -> update now; `Some(false)` -> nothing.
///
/// Prompts and updates are disjoint by construction.
pub fn build_update_plan(
    catalog: &[crate::config::coding_agents_catalog::CodingAgentDefinition],
    registered_commands: &HashSet<String>,
    answers: &BTreeMap<String, bool>,
    default_cwd: PathBuf,
) -> AgentUpdatePlan {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut prompts: Vec<UpdateTarget> = Vec::new();
    let mut updates: Vec<UpdateTarget> = Vec::new();

    for entry in catalog {
        if !registered_commands.contains(&entry.command) {
            continue; // catalog-only command: never prompted, never updated
        }
        // First occurrence wins - including an empty sequence, which suppresses
        // the command for this pass (a later duplicate cannot re-enable it).
        if !seen.insert(entry.command.as_str()) {
            continue;
        }
        if entry.update_commands.is_empty() {
            continue; // effective sequence empty: no prompt, no update
        }
        let target = UpdateTarget {
            command: entry.command.clone(),
            label: entry.label.clone(),
            commands: entry.update_commands.clone(),
            cwd: default_cwd.clone(),
        };
        match answers.get(&entry.command) {
            None => prompts.push(target),
            Some(true) => updates.push(target),
            Some(false) => {}
        }
    }

    AgentUpdatePlan { prompts, updates }
}

/// #1551/#1967 - one row per DISTINCT effective command with a non-empty
/// sequence, in catalog order. The first entry of a command wins: its
/// key/label/color and its exact sequence are used, a later duplicate adds no
/// row, and an empty first sequence suppresses the command for good (a later
/// non-empty duplicate cannot bring it back). Commands without an install entry
/// are `checking` (seq 0).
pub fn build_update_overview_rows(
    catalog: &[CodingAgentDefinition],
    install_by_command: &HashMap<String, InstallState>,
) -> Vec<AgentUpdateOverviewRow> {
    let mut seen: HashSet<&str> = HashSet::new();
    catalog
        .iter()
        .filter(|entry| {
            if !seen.insert(entry.command.as_str()) {
                return false; // first effective entry wins
            }
            !entry.update_commands.is_empty()
        })
        .map(|entry| AgentUpdateOverviewRow {
            key: entry.key.clone(),
            label: entry.label.clone(),
            command: entry.command.clone(),
            color: entry.color.clone(),
            update_commands: entry.update_commands.clone(),
            install: install_by_command
                .get(&entry.command)
                .cloned()
                .unwrap_or_else(InstallState::checking),
        })
        .collect()
}

/// #1551 - resolve + probe policy for one catalog command. Executes a process ONLY for a
/// bare token whose stem has a built-in probe (plan 5.2): a project-authored catalog can
/// name `claude` and get the user's own PATH `claude`, but can never point the probe at a
/// bundled binary. Explicit paths and unknown stems report presence only (`unprobed`).
fn resolve_command_install_probe(
    command: &str,
) -> Result<(PathBuf, &'static [&'static str]), InstallState> {
    resolve_command_install_probe_with(command, resolve_program)
}

/// #2589 - `resolve_command_install_probe` against an explicit search path.
fn resolve_command_install_probe_in(
    command: &str,
    search_path: &OsStr,
) -> Result<(PathBuf, &'static [&'static str]), InstallState> {
    resolve_command_install_probe_with(command, |token| resolve_program_in(token, search_path))
}

fn resolve_command_install_probe_with(
    command: &str,
    resolve: impl FnOnce(&str) -> Option<PathBuf>,
) -> Result<(PathBuf, &'static [&'static str]), InstallState> {
    let Ok(normalized) = normalize_legacy_agent_command(command) else {
        return Err(InstallState::missing("empty command".to_string()));
    };
    let token = normalized.shell;
    let bare = is_bare_program_token(&token);
    let Some(path) = resolve(&token) else {
        return Err(InstallState::missing(if bare {
            format!("'{token}' was not found on PATH")
        } else {
            format!("'{token}' is not a file")
        }));
    };
    if !bare {
        return Err(InstallState::unprobed(
            &path,
            "explicit path: version not probed".to_string(),
        ));
    }
    let stem = Path::new(&token)
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let Some(args) = version_probe_args(&stem) else {
        return Err(InstallState::unprobed(
            &path,
            format!("no built-in version probe for '{stem}'"),
        ));
    };
    Ok((path, args))
}

pub async fn probe_command_install_state(command: &str) -> InstallState {
    warm_login_shell_path().await;
    probe_command_install_state_in(command, &effective_search_path()).await
}

/// #2589 - resolve AND execute the probe with `search_path`, so both the agent
/// and its `#!/usr/bin/env` interpreter are found.
async fn probe_command_install_state_in(command: &str, search_path: &OsStr) -> InstallState {
    let (path, args) = match resolve_command_install_probe_in(command, search_path) {
        Ok(probe) => probe,
        Err(state) => return state,
    };
    match probe_version_in(&path, args, PROBE_TIMEOUT, search_path).await {
        ProbeOutcome::Version(version) => InstallState::installed(version, &path),
        ProbeOutcome::Failed(detail) => InstallState::probe_failed(&path, detail),
    }
}

enum TargetProbeOutcome {
    Completed(InstallState),
    Cancelled,
    CleanupFailed(String),
}

async fn probe_command_install_state_cancellable<I, O>(
    command: &str,
    cancel: &mut tokio::sync::watch::Receiver<bool>,
    owner_slot: &mut RetainedProbeOwner,
    owner_installed: I,
    cancellation_observed: O,
) -> TargetProbeOutcome
where
    I: FnOnce(),
    O: FnOnce(),
{
    if *cancel.borrow() {
        return TargetProbeOutcome::Cancelled;
    }
    let (path, args) = match resolve_command_install_probe(command) {
        Ok(probe) => probe,
        Err(state) => return TargetProbeOutcome::Completed(state),
    };
    match probe_version_cancellable_retained(
        &path,
        args,
        PROBE_TIMEOUT,
        &effective_search_path(),
        cancel,
        owner_slot,
        owner_installed,
        cancellation_observed,
    )
    .await
    {
        CancellableProbeOutcome::Completed(ProbeOutcome::Version(version)) => {
            TargetProbeOutcome::Completed(InstallState::installed(version, &path))
        }
        CancellableProbeOutcome::Completed(ProbeOutcome::Failed(detail)) => {
            TargetProbeOutcome::Completed(InstallState::probe_failed(&path, detail))
        }
        CancellableProbeOutcome::Cancelled => TargetProbeOutcome::Cancelled,
        CancellableProbeOutcome::CleanupFailed(detail) => {
            log::warn!("[agent-update] cancellable probe cleanup defective: {detail}");
            TargetProbeOutcome::CleanupFailed(detail)
        }
    }
}

/// #1551 - the probe a scheduled task runs; production = `probe_command_install_state`.
pub type ProbeFn =
    Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = InstallState> + Send>> + Send + Sync>;

pub fn production_probe() -> ProbeFn {
    Arc::new(|command| Box::pin(async move { probe_command_install_state(&command).await }))
}

/// #1551 - instant: never awaits a probe, never waits on the gate, never holds a lock across
/// an await. Probes are scheduled ONLY once the startup pass is finished, so a version probe
/// can never overlap an update of the same CLI, and ONLY through the single-lock
/// `lookup_or_begin`, so two overlapping calls can never open two tickets.
/// #1967 P4 - `Err` when the persisted catalog is unavailable; nothing is probed or
/// scheduled in that case.
pub async fn update_overview(
    app: &AppHandle,
    settings: &SettingsState,
    gate: &AgentUpdateGate,
    cache: &Arc<AgentInstallCache>,
) -> Result<Vec<AgentUpdateOverviewRow>, CatalogUnavailable> {
    update_overview_with(app, settings, gate, cache, production_probe()).await
}

/// `update_overview` with an injectable probe so the scheduling tests control timing.
pub async fn update_overview_with(
    app: &AppHandle,
    settings: &SettingsState,
    gate: &AgentUpdateGate,
    cache: &Arc<AgentInstallCache>,
    probe: ProbeFn,
) -> Result<Vec<AgentUpdateOverviewRow>, CatalogUnavailable> {
    let settings = settings
        .current_snapshot()
        .await
        .map_err(|error| CatalogUnavailable {
            code: "settings_unavailable".to_string(),
            path: String::new(),
            reason: error.to_string(),
        })?;
    let catalog = load_catalog_for_settings(&settings)?;
    // Read the gate ONCE, before any cache operation: that order is what makes
    // "finished implies the post-pass generation" hold (plan 5.4 step 8).
    let pass_finished = gate.is_finished();
    let now = Instant::now();
    let mut install_by_command: HashMap<String, InstallState> = HashMap::new();
    let mut seen: HashSet<&str> = HashSet::new();

    for entry in &catalog {
        // First effective entry wins: an empty winner is skipped (no row, no
        // probe) and a later duplicate is ignored entirely.
        if !seen.insert(entry.command.as_str()) {
            continue;
        }
        if entry.update_commands.is_empty() {
            continue;
        }
        match cache.lookup_or_begin(&entry.command, now, INSTALL_CACHE_TTL, pass_finished) {
            Scheduling::Fresh(state) => {
                install_by_command.insert(entry.command.clone(), state);
            }
            // Another call's probe is running, or the pass is not finished yet:
            // the row stays `checking` and nothing is scheduled.
            Scheduling::InFlight | Scheduling::Deferred => {}
            Scheduling::Began(ticket) => {
                spawn_probe_task(app, &entry.command, ticket, Arc::clone(&probe));
            }
        }
    }

    Ok(build_update_overview_rows(&catalog, &install_by_command))
}

/// #1551 - the ONE spawn site for probe tasks: the Settings-triggered scheduling of
/// `update_overview_with` and the post-pass scheduling of `schedule_post_update_probes`
/// both come through here. `tauri::async_runtime::spawn`, never `tokio::spawn`, so it
/// runs from any caller context.
fn spawn_probe_task(app: &AppHandle, command: &str, ticket: ProbeTicket, probe: ProbeFn) {
    let app = app.clone();
    let command = command.to_string();
    tauri::async_runtime::spawn(async move {
        probe_commit_announce(&app, &command, ticket, move |c| probe(c)).await;
    });
}

/// #1551 - the probe task body. Announces `agent_install_state_changed` ONLY for a state
/// committed to the current cache generation: a completion rejected by an invalidation is
/// re-probed once on the same (renewed) ticket; a second rejection emits nothing and drops
/// the ticket, which frees the slot so the next overview call schedules fresh work. At most
/// two processes per command per overview call.
async fn probe_commit_announce<F, Fut>(
    app: &AppHandle,
    command: &str,
    ticket: ProbeTicket,
    probe: F,
) where
    F: Fn(String) -> Fut,
    Fut: Future<Output = InstallState>,
{
    let committed = match ticket.complete(probe(command.to_string()).await) {
        Completion::Committed(state) => Some(state),
        Completion::Stale(retry) => match retry.complete(probe(command.to_string()).await) {
            Completion::Committed(state) => Some(state),
            Completion::Stale(abandoned) => {
                drop(abandoned);
                None
            }
        },
    };
    if let Some(install) = committed {
        emit_all(
            app,
            "agent_install_state_changed",
            json!({ "command": command, "install": install }),
        );
    }
}

/// #1551 - one prompt across every surface. One critical section per gate, serialized by
/// `gate.answer_serial` (tokio mutex, FIFO; held across the persist await; the std `state`
/// lock is taken twice, briefly, never across an await): classify -> persist -> claim ->
/// emit the closure -> release the loop. The prompt loop's expiry arm takes the SAME serial
/// section before it removes a pending entry, so the pending entry has exactly one remover
/// and that remover is the only emitter of `agent_update_prompt_closed`.
/// - First answer to persist wins: it records the policy, claims the pending entry, emits the
///   closure to every surface, and only then delivers the answer to the loop (which may then
///   show the next prompt): the closure of A always precedes the prompt of B.
/// - A later answer for a command with a recorded policy returns Ok(false) WITHOUT persisting.
/// - An answer for an expired prompt (timeout or pass ended) persists for future boots, is
///   recorded, and returns Ok(false); this boot is unaffected (accepted late semantics).
/// - A failed persist changes nothing (prompt still pending, nothing recorded) and returns
///   Err; the next answer in the queue proceeds and can win.
///
/// No await sits between claim, emit and deliver: a cancelled call (a WebSocket connection
/// dropped mid-command) either never claimed or completed all three.
pub async fn answer_prompt<P, Fut>(
    app: &AppHandle,
    gate: &AgentUpdateGate,
    command: &str,
    enabled: bool,
    persist: P,
) -> Result<bool, String>
where
    P: FnOnce() -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    let _serial = gate.answer_serial.lock().await;
    match gate.prompt_state(command) {
        PromptState::NotPrompted => {
            return Err(format!(
                "agent_update_answer: '{command}' was not prompted this boot"
            ));
        }
        PromptState::Answered(_) => {
            log::info!(
                "[agent-update] answer for '{command}' (enabled={enabled}): ignored, already answered this boot"
            );
            return Ok(false);
        }
        PromptState::Cancelled => return Ok(false),
        PromptState::Pending | PromptState::Expired => {}
    }
    persist().await?;
    match gate.claim_answer(command, enabled) {
        AnswerClaim::Claimed(claimed) => {
            // (1) the closure is enqueued on every surface (WebSocket queues, Tauri emit) ...
            emit_all(app, "agent_update_prompt_closed", json!(claimed.closed()));
            if let Some(skipped) = claimed.skipped() {
                emit_all(app, "agent_update_command_skipped", json!(skipped));
            }
            // (2) ... and only now is the loop released; `false` iff the receiver is already gone.
            let applied = claimed.deliver(enabled);
            log::info!(
                "[agent-update] answer for '{command}' (enabled={enabled}): {}",
                if applied {
                    "applied this boot"
                } else {
                    "recorded for future boots (prompt already expired)"
                }
            );
            Ok(applied)
        }
        AnswerClaim::Recorded => {
            log::info!(
                "[agent-update] answer for '{command}' (enabled={enabled}): recorded for future boots (prompt already closed)"
            );
            Ok(false)
        }
    }
}

/// Last `OUTPUT_TAIL_BYTES` of a captured pipe, lossy-decoded for logging.
fn output_tail(bytes: &[u8]) -> String {
    let start = bytes.len().saturating_sub(OUTPUT_TAIL_BYTES);
    String::from_utf8_lossy(&bytes[start..]).to_string()
}

enum TargetWait {
    Exited(std::io::Result<std::process::ExitStatus>),
    TimedOut,
    Cancelled,
}

struct TargetSettlement {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    defects: Vec<String>,
}

enum UpdaterStepOutcome {
    Succeeded,
    Failed(String),
    Cancelled,
    CleanupFailed(String),
    /// Windows only: pre-execution containment is a Win32 Job Object primitive, so
    /// `spawn_suspended_contained` is exported only there and this is constructed only on
    /// the `ContainedSpawnError::Containment` arm. On Unix `TargetProcessOwner::spawn` uses
    /// a plain spawn, which can fail but has no containment step to fail, so the variant is
    /// never produced and `dead_code` is correct to say so.
    #[cfg_attr(not(windows), allow(dead_code))]
    ContainmentFailed,
}

struct RecoverableUpdateReader {
    completion: Shared<BoxFuture<'static, Result<Vec<u8>, String>>>,
    abort: tokio::task::AbortHandle,
    abort_requested: bool,
    #[cfg(test)]
    settlement_hook: Option<TargetReaderSettlementHook>,
}

#[cfg(test)]
struct TargetReaderSettlementHook {
    hold_completion: Option<tokio::sync::oneshot::Receiver<()>>,
    before_panic: Option<tokio::sync::oneshot::Sender<()>>,
    release_panic: Option<tokio::sync::oneshot::Receiver<()>>,
    retry_started: Option<tokio::sync::oneshot::Sender<()>>,
}

struct TargetProcessOwner {
    child: Option<tokio::process::Child>,
    stdout: Option<RecoverableUpdateReader>,
    stderr: Option<RecoverableUpdateReader>,
    defects: Vec<String>,
    #[cfg(windows)]
    job: Option<crate::pty::job::JobObject>,
    #[cfg(unix)]
    pgid: i32,
    #[cfg(test)]
    settlement_hook: Option<TargetSettlementHook>,
}

#[cfg(test)]
struct TargetSettlementHook {
    before_native_proof: Option<tokio::sync::oneshot::Sender<()>>,
    release_native_proof: Option<tokio::sync::oneshot::Receiver<()>>,
    settlement_window: Option<Duration>,
    native_observations: std::collections::VecDeque<TargetNativeObservation>,
    panic_after_native_proof_release: bool,
    panic_after_native_query_defect: bool,
    stdout_reader: Option<TargetReaderSettlementHook>,
}

#[cfg(test)]
enum TargetNativeObservation {
    Empty,
    Active,
    Error(&'static str),
}

#[cfg(test)]
tokio::task_local! {
    static TARGET_SETTLEMENT_HOOK: std::cell::RefCell<Option<TargetSettlementHook>>;
}

#[cfg(test)]
async fn with_target_settlement_hook<F>(hook: TargetSettlementHook, future: F) -> F::Output
where
    F: Future,
{
    TARGET_SETTLEMENT_HOOK
        .scope(std::cell::RefCell::new(Some(hook)), future)
        .await
}

impl TargetProcessOwner {
    async fn spawn(command: &mut tokio::process::Command) -> Result<Self, UpdaterStepOutcome> {
        #[cfg(windows)]
        let (mut child, job) = match crate::pty::job::spawn_suspended_contained(command).await {
            Ok(pair) => pair,
            Err(crate::pty::job::ContainedSpawnError::Spawn(error)) => {
                return Err(UpdaterStepOutcome::Failed(error.to_string()))
            }
            Err(crate::pty::job::ContainedSpawnError::Containment(reason)) => {
                log::warn!(
                    "[agent-update] contained updater launch rejected: process containment failed at {reason}"
                );
                return Err(UpdaterStepOutcome::ContainmentFailed);
            }
            Err(crate::pty::job::ContainedSpawnError::Cleanup(detail)) => {
                log::warn!("[agent-update] contained updater launch cleanup defective: {detail}");
                return Err(UpdaterStepOutcome::CleanupFailed(detail));
            }
        };
        #[cfg(not(windows))]
        let mut child = command
            .spawn()
            .map_err(|error| UpdaterStepOutcome::Failed(error.to_string()))?;

        #[cfg(unix)]
        let pgid = child
            .id()
            .and_then(|pid| i32::try_from(pid).ok())
            .unwrap_or(0);
        #[cfg(test)]
        let mut settlement_hook = TARGET_SETTLEMENT_HOOK
            .try_with(|hook| hook.borrow_mut().take())
            .ok()
            .flatten();
        #[cfg(test)]
        let stdout_reader_hook = settlement_hook
            .as_mut()
            .and_then(|hook| hook.stdout_reader.take());
        #[cfg(test)]
        let stdout = child
            .stdout
            .take()
            .map(move |reader| spawn_update_reader(reader, stdout_reader_hook));
        #[cfg(test)]
        let stderr = child
            .stderr
            .take()
            .map(|reader| spawn_update_reader(reader, None));
        #[cfg(not(test))]
        let stdout = child.stdout.take().map(spawn_update_reader);
        #[cfg(not(test))]
        let stderr = child.stderr.take().map(spawn_update_reader);
        Ok(Self {
            child: Some(child),
            stdout,
            stderr,
            defects: Vec::new(),
            #[cfg(windows)]
            job: Some(job),
            #[cfg(unix)]
            pgid,
            #[cfg(test)]
            settlement_hook,
        })
    }

    async fn wait(
        &mut self,
        timeout: Duration,
        cancel: &mut tokio::sync::watch::Receiver<bool>,
    ) -> TargetWait {
        let child = self.child.as_mut().expect("target process owner");
        tokio::select! {
            result = child.wait() => TargetWait::Exited(result),
            _ = tokio::time::sleep(timeout) => TargetWait::TimedOut,
            _ = wait_for_target_cancellation(cancel) => TargetWait::Cancelled,
        }
    }

    async fn settle(&mut self, wait: &TargetWait) -> TargetSettlement {
        #[cfg(test)]
        let settlement_window = self
            .settlement_hook
            .as_ref()
            .and_then(|hook| hook.settlement_window)
            .unwrap_or(PROCESS_SETTLEMENT_WINDOW);
        #[cfg(not(test))]
        let settlement_window = PROCESS_SETTLEMENT_WINDOW;
        let mut attempt = crate::pty::job::SettlementAttempt::new(settlement_window);
        let defects = &mut self.defects;
        let cleanup_required = !matches!(wait, TargetWait::Exited(Ok(_)));

        #[cfg(windows)]
        if cleanup_required {
            if let Some(job) = self.job.as_ref() {
                if let Err(error) = job.terminate_checked() {
                    defects.push(format!("job termination failed: {error}"));
                }
            }
        }
        #[cfg(unix)]
        if cleanup_required && self.pgid > 0 {
            // SAFETY: the group was created for this updater step.
            let result = unsafe { libc::kill(-self.pgid, libc::SIGKILL) };
            if result != 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) {
                defects.push(format!(
                    "process-group termination failed: {}",
                    std::io::Error::last_os_error()
                ));
            }
        }
        if cleanup_required {
            if let Some(child) = self.child.as_mut() {
                if let Err(error) = child.start_kill() {
                    defects.push(format!("direct child termination failed: {error}"));
                }
            }
        }
        if cleanup_required {
            if let Some(child) = self.child.as_mut() {
                let mut wait_defect_recorded = false;
                loop {
                    match child.try_wait() {
                        Ok(Some(_)) => break,
                        Ok(None) => {}
                        Err(error) => {
                            if !wait_defect_recorded {
                                defects.push(format!("direct child wait failed: {error}"));
                                wait_defect_recorded = true;
                            }
                        }
                    }
                    if attempt.expired() {
                        record_update_attempt_expiry(defects, "direct-child settlement");
                        #[cfg(windows)]
                        if let Some(job) = self.job.as_ref() {
                            let _ = job.terminate_checked();
                        }
                        #[cfg(unix)]
                        if self.pgid > 0 {
                            // SAFETY: this remains the retained group identity.
                            unsafe {
                                libc::kill(-self.pgid, libc::SIGKILL);
                            }
                        }
                        let _ = child.start_kill();
                        attempt.restart();
                    }
                    tokio::time::sleep(PROCESS_SETTLEMENT_POLL.min(attempt.remaining())).await;
                }
            }
        }
        drop(self.child.take());

        #[cfg(windows)]
        if !cleanup_required {
            if let Some(job) = self.job.as_ref() {
                job.terminate();
            }
        }
        #[cfg(unix)]
        if !cleanup_required && self.pgid > 0 {
            // SAFETY: the retained positive group belongs to this step.
            unsafe {
                libc::kill(-self.pgid, libc::SIGKILL);
            }
        }

        let stdout = settle_update_reader(&mut self.stdout, defects, &mut attempt).await;
        let stderr = settle_update_reader(&mut self.stderr, defects, &mut attempt).await;

        #[cfg(test)]
        {
            let (ready, mut release) = self
                .settlement_hook
                .as_mut()
                .map(|hook| {
                    (
                        hook.before_native_proof.take(),
                        hook.release_native_proof.take(),
                    )
                })
                .unwrap_or((None, None));
            if let Some(ready) = ready {
                let _ = ready.send(());
            }
            if let Some(release) = release.as_mut() {
                loop {
                    if attempt.expired() {
                        record_update_attempt_expiry(defects, "native-proof barrier");
                        attempt.restart();
                    }
                    match tokio::time::timeout(attempt.remaining(), &mut *release).await {
                        Ok(_) => break,
                        Err(_) => {
                            record_update_attempt_expiry(defects, "native-proof barrier");
                            attempt.restart();
                        }
                    }
                }
            }
            let panic_after_release = self
                .settlement_hook
                .as_mut()
                .is_some_and(|hook| std::mem::take(&mut hook.panic_after_native_proof_release));
            if panic_after_release {
                panic!("injected updater panic during native settlement");
            }
        }

        #[cfg(windows)]
        if let Some(job) = self.job.as_ref() {
            let mut query_defect_recorded = false;
            loop {
                #[cfg(test)]
                let proof = match self
                    .settlement_hook
                    .as_mut()
                    .and_then(|hook| hook.native_observations.pop_front())
                {
                    Some(observation) => match observation {
                        TargetNativeObservation::Empty => Ok(true),
                        TargetNativeObservation::Active => Ok(false),
                        TargetNativeObservation::Error(detail) => Err(detail.to_string()),
                    },
                    None => job
                        .active_processes()
                        .map(|active| active == 0)
                        .map_err(|error| error.to_string()),
                };
                #[cfg(not(test))]
                let proof = job
                    .active_processes()
                    .map(|active| active == 0)
                    .map_err(|error| error.to_string());
                match proof {
                    Ok(true) => break,
                    Ok(false) => {}
                    Err(error) => {
                        if !query_defect_recorded {
                            defects.push(format!("job accounting failed: {error}"));
                            query_defect_recorded = true;
                        }
                        #[cfg(test)]
                        if self.settlement_hook.as_mut().is_some_and(|hook| {
                            std::mem::take(&mut hook.panic_after_native_query_defect)
                        }) {
                            panic!("injected updater panic after native query defect");
                        }
                    }
                }
                if attempt.expired() {
                    record_update_attempt_expiry(defects, "job accounting proof");
                    let _ = job.terminate_checked();
                    attempt.restart();
                }
                tokio::time::sleep(PROCESS_SETTLEMENT_POLL.min(attempt.remaining())).await;
            }
        }

        #[cfg(unix)]
        if self.pgid > 0 {
            let mut errno_defect_recorded = false;
            loop {
                #[cfg(test)]
                let proof = match self
                    .settlement_hook
                    .as_mut()
                    .and_then(|hook| hook.native_observations.pop_front())
                {
                    Some(observation) => match observation {
                        TargetNativeObservation::Empty => Ok(true),
                        TargetNativeObservation::Active => Ok(false),
                        TargetNativeObservation::Error(detail) => Err(detail.to_string()),
                    },
                    None => {
                        // SAFETY: signal zero is a read-only group existence query.
                        let result = unsafe { libc::kill(-self.pgid, 0) };
                        if result == -1 {
                            let error = std::io::Error::last_os_error();
                            if error.raw_os_error() == Some(libc::ESRCH) {
                                Ok(true)
                            } else {
                                Err(error.to_string())
                            }
                        } else {
                            Ok(false)
                        }
                    }
                };
                #[cfg(not(test))]
                let proof = {
                    // SAFETY: signal zero is a read-only group existence query.
                    let result = unsafe { libc::kill(-self.pgid, 0) };
                    if result == -1 {
                        let error = std::io::Error::last_os_error();
                        if error.raw_os_error() == Some(libc::ESRCH) {
                            Ok(true)
                        } else {
                            Err(error.to_string())
                        }
                    } else {
                        Ok(false)
                    }
                };
                match proof {
                    Ok(true) => break,
                    Ok(false) => {}
                    Err(error) => {
                        if !errno_defect_recorded {
                            defects.push(format!("process-group accounting failed: {error}"));
                            errno_defect_recorded = true;
                        }
                        #[cfg(test)]
                        if self.settlement_hook.as_mut().is_some_and(|hook| {
                            std::mem::take(&mut hook.panic_after_native_query_defect)
                        }) {
                            panic!("injected updater panic after native query defect");
                        }
                    }
                }
                if attempt.expired() {
                    record_update_attempt_expiry(defects, "process-group accounting proof");
                    // SAFETY: this remains the retained group identity.
                    unsafe {
                        libc::kill(-self.pgid, libc::SIGKILL);
                    }
                    attempt.restart();
                }
                tokio::time::sleep(PROCESS_SETTLEMENT_POLL.min(attempt.remaining())).await;
            }
        }
        #[cfg(windows)]
        drop(self.job.take());
        TargetSettlement {
            stdout,
            stderr,
            defects: defects.clone(),
        }
    }

    async fn settle_observing_cancellation<F>(
        &mut self,
        wait: &TargetWait,
        cancel: &mut tokio::sync::watch::Receiver<bool>,
        observed: F,
    ) -> TargetSettlement
    where
        F: FnOnce(),
    {
        if matches!(wait, TargetWait::Cancelled) {
            observed();
            return self.settle(wait).await;
        }
        let settlement = self.settle(wait);
        tokio::pin!(settlement);
        tokio::select! {
            biased;
            _ = wait_for_target_cancellation(cancel) => {
                observed();
                settlement.await
            }
            settled = &mut settlement => settled,
        }
    }
}

struct TargetOwnerLease {
    work: TargetWork,
    epoch: u64,
    cancel: tokio::sync::watch::Receiver<bool>,
}

/// One externally retained owner/receiver/handoff lease for a target. The slot
/// lives outside `run_update_target_body`'s caught future, so unwinding during
/// updater or probe settlement cannot discard the only cleanup authority.
#[derive(Default)]
struct TargetOwnerSlot {
    lease: Option<TargetOwnerLease>,
    updater: Option<TargetProcessOwner>,
    probe: RetainedProbeOwner,
}

impl TargetOwnerSlot {
    fn arm(&mut self, work: TargetWork, epoch: u64, cancel: tokio::sync::watch::Receiver<bool>) {
        debug_assert!(self.lease.is_none());
        debug_assert!(self.updater.is_none());
        debug_assert!(!self.probe.is_some());
        self.lease = Some(TargetOwnerLease {
            work,
            epoch,
            cancel,
        });
    }

    fn identity(&self) -> (TargetWork, u64) {
        let lease = self.lease.as_ref().expect("active target owner lease");
        (lease.work, lease.epoch)
    }

    fn complete_without_owner(&mut self, gate: &AgentUpdateGate, command: &str) -> bool {
        let (work, epoch) = self.identity();
        let reserved = gate.completed_without_owner(command, work, epoch);
        self.clear();
        reserved
    }

    fn owner_returned(&mut self, gate: &AgentUpdateGate, command: &str) -> bool {
        let (work, epoch) = self.identity();
        let reserved = gate.owner_returned(command, work, epoch);
        self.clear();
        reserved
    }

    fn clear(&mut self) {
        drop(self.updater.take());
        self.probe.clear();
        self.lease = None;
    }

    async fn settle_after_panic(
        &mut self,
        gate: &AgentUpdateGate,
        command: &str,
    ) -> Option<String> {
        let lease = self.lease.as_mut()?;
        let work = lease.work;
        let epoch = lease.epoch;
        let detail = if let Some(owner) = self.updater.as_mut() {
            let settlement = owner
                .settle_observing_cancellation(&TargetWait::Cancelled, &mut lease.cancel, || {
                    gate.owner_observed_cancellation(command, work, epoch)
                })
                .await;
            (!settlement.defects.is_empty()).then(|| settlement.defects.join("; "))
        } else if self.probe.is_some() {
            self.probe
                .settle_after_panic(&mut lease.cancel, || {
                    gate.owner_observed_cancellation(command, work, epoch)
                })
                .await
        } else {
            gate.completed_without_owner(command, work, epoch);
            self.clear();
            return None;
        };
        gate.owner_returned(command, work, epoch);
        self.clear();
        detail
    }
}

fn spawn_update_reader<R>(
    mut reader: R,
    #[cfg(test)] mut settlement_hook: Option<TargetReaderSettlementHook>,
) -> RecoverableUpdateReader
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    #[cfg(test)]
    let hold_completion = settlement_hook
        .as_mut()
        .and_then(|hook| hook.hold_completion.take());
    let reader = tokio::spawn(async move {
        #[cfg(test)]
        if let Some(hold_completion) = hold_completion {
            let _ = hold_completion.await;
        }
        let mut bytes = Vec::new();
        let _ = tokio::io::AsyncReadExt::read_to_end(&mut reader, &mut bytes).await;
        bytes
    });
    let abort = reader.abort_handle();
    let completion = async move { reader.await.map_err(|error| error.to_string()) }
        .boxed()
        .shared();
    RecoverableUpdateReader {
        completion,
        abort,
        abort_requested: false,
        #[cfg(test)]
        settlement_hook,
    }
}

fn record_update_attempt_expiry(defects: &mut Vec<String>, stage: &str) {
    let detail = format!("settlement attempt deadline exceeded during {stage}");
    if !defects.iter().any(|defect| defect == &detail) {
        defects.push(detail);
    }
}

async fn settle_update_reader(
    reader: &mut Option<RecoverableUpdateReader>,
    defects: &mut Vec<String>,
    attempt: &mut crate::pty::job::SettlementAttempt,
) -> Vec<u8> {
    let Some(reader) = reader.as_mut() else {
        return Vec::new();
    };
    let completion = reader.completion.clone();
    #[cfg(test)]
    if let Some(hook) = reader.settlement_hook.as_mut() {
        if let Some(release_panic) = hook.release_panic.take() {
            if let Some(before_panic) = hook.before_panic.take() {
                let _ = before_panic.send(());
            }
            let _ = release_panic.await;
            panic!("injected updater panic during reader settlement");
        }
        if let Some(retry_started) = hook.retry_started.take() {
            let _ = retry_started.send(());
        }
    }
    if reader.abort_requested {
        reader.abort.abort();
        let _ = completion.await;
        return Vec::new();
    }
    if attempt.expired() {
        record_update_attempt_expiry(defects, "reader settlement");
        attempt.restart();
    }
    match tokio::time::timeout(attempt.remaining(), completion.clone()).await {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(error)) => {
            let detail = format!("reader join failed: {error}");
            if !defects.iter().any(|defect| defect == &detail) {
                defects.push(detail);
            }
            Vec::new()
        }
        Err(_) => {
            record_update_attempt_expiry(defects, "reader settlement");
            reader.abort_requested = true;
            reader.abort.abort();
            let _ = completion.await;
            attempt.restart();
            Vec::new()
        }
    }
}

async fn wait_for_target_cancellation(cancel: &mut tokio::sync::watch::Receiver<bool>) {
    loop {
        if *cancel.borrow() {
            return;
        }
        if cancel.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

/// Execute one command's update sequence IN ORDER, fail-fast: the first failed
/// step stops the chain. Each step is a full shell string run through
/// `cmd.exe /C` (Windows) or `sh -c` (elsewhere), cwd = the target's directory,
/// no stdin, output tail-captured concurrently with the wait (a chatty command
/// must never block on the 64KB pipe buffer). Per-step timeout kills the WHOLE
/// tree (Windows JobObject, Unix process-group kill).
/// #2589 - one updater step's process, with `search_path` as its PATH so a
/// GUI-launched app finds agents installed in the user's bin dirs.
fn build_update_step_command(
    cmd: &str,
    cwd: &Path,
    search_path: &OsStr,
) -> tokio::process::Command {
    let mut command = if cfg!(windows) {
        let mut command = tokio::process::Command::new("cmd.exe");
        command.arg("/C").arg(cmd);
        command
    } else {
        let mut command = tokio::process::Command::new("sh");
        command.arg("-c").arg(cmd);
        command
    };
    command.current_dir(cwd);
    command.env("PATH", search_path);
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    command.kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    command
}

async fn run_update_sequence_cancellable(
    target: &UpdateTarget,
    step_timeout: Duration,
    gate: &AgentUpdateGate,
    owner_slot: &mut TargetOwnerSlot,
) -> UpdaterStepOutcome {
    for (step_index, cmd) in target.commands.iter().enumerate() {
        log::info!(
            "[agent-update] running '{}' for {} ({})",
            cmd,
            target.label,
            target.command
        );
        let work = TargetWork::UpdaterStep(step_index);
        let Some((epoch, _)) = gate.begin_step(&target.command, work) else {
            return UpdaterStepOutcome::Cancelled;
        };
        #[cfg(test)]
        gate.hold_after_step_begin_for_test(&target.command).await;
        let StepAdmission::Committed { epoch, cancel } =
            gate.commit_step(&target.command, work, epoch)
        else {
            return UpdaterStepOutcome::Cancelled;
        };
        #[cfg(test)]
        gate.hold_after_step_commit_for_test(&target.command).await;
        owner_slot.arm(work, epoch, cancel);

        let mut command = build_update_step_command(cmd, &target.cwd, &effective_search_path());

        match TargetProcessOwner::spawn(&mut command).await {
            Ok(owner) => owner_slot.updater = Some(owner),
            Err(outcome) => {
                let reserved = owner_slot.complete_without_owner(gate, &target.command);
                return if reserved && !matches!(outcome, UpdaterStepOutcome::CleanupFailed(_)) {
                    UpdaterStepOutcome::Cancelled
                } else {
                    outcome
                };
            }
        }
        gate.owner_installed(&target.command, work, epoch);
        let waited = {
            let lease = owner_slot.lease.as_mut().expect("installed target lease");
            owner_slot
                .updater
                .as_mut()
                .expect("installed target owner")
                .wait(step_timeout, &mut lease.cancel)
                .await
        };
        let settlement = {
            let lease = owner_slot.lease.as_mut().expect("settled target lease");
            owner_slot
                .updater
                .as_mut()
                .expect("settled target owner")
                .settle_observing_cancellation(&waited, &mut lease.cancel, || {
                    gate.owner_observed_cancellation(&target.command, work, epoch);
                })
                .await
        };
        let cancellation_reserved = owner_slot.owner_returned(gate, &target.command);
        if !settlement.defects.is_empty() {
            log::warn!(
                "[agent-update] process cleanup defective for {} ({}): {}",
                target.label,
                target.command,
                settlement.defects.join("; ")
            );
            return UpdaterStepOutcome::CleanupFailed(settlement.defects.join("; "));
        }
        if cancellation_reserved || matches!(waited, TargetWait::Cancelled) {
            return UpdaterStepOutcome::Cancelled;
        }
        match waited {
            TargetWait::TimedOut => {
                let reason = format!("timed out after {}s (killed)", step_timeout.as_secs());
                log::warn!(
                    "[agent-update] step TIMED OUT for {} ({}): {reason}\n{}",
                    target.label,
                    target.command,
                    output_tail(&settlement.stdout)
                );
                return UpdaterStepOutcome::Failed(reason);
            }
            TargetWait::Exited(Err(error)) => {
                log::warn!(
                    "[agent-update] step wait error for {} ({}): {error}",
                    target.label,
                    target.command
                );
                return UpdaterStepOutcome::Failed(error.to_string());
            }
            TargetWait::Exited(Ok(status)) if status.success() => {
                log::info!(
                    "[agent-update] step ok for {} ({}):\n{}",
                    target.label,
                    target.command,
                    output_tail(&settlement.stdout)
                );
            }
            TargetWait::Exited(Ok(status)) => {
                let reason = format!("exit code {}", status.code().unwrap_or(-1));
                let stderr_tail = output_tail(&settlement.stderr);
                log::warn!(
                    "[agent-update] step FAILED for {} ({}): {}\n{}{}",
                    target.label,
                    target.command,
                    reason,
                    output_tail(&settlement.stdout),
                    if stderr_tail.is_empty() {
                        String::new()
                    } else {
                        format!("\nstderr:\n{stderr_tail}")
                    }
                );
                return UpdaterStepOutcome::Failed(reason);
            }
            TargetWait::Cancelled => return UpdaterStepOutcome::Cancelled,
        }
    }
    UpdaterStepOutcome::Succeeded
}

/// #2736 - resolved install commands with a running install task. Keyed by the
/// command string, so two catalog keys sharing one command never run it twice.
static INSTALLS_IN_FLIGHT: std::sync::LazyLock<Mutex<HashSet<String>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashSet::new()));

/// #2736 - RAII entry in `INSTALLS_IN_FLIGHT`; dropping it releases the command.
struct InstallInFlight(String);

impl InstallInFlight {
    fn acquire(command: &str) -> Result<Self, String> {
        let mut in_flight = INSTALLS_IN_FLIGHT.lock().unwrap_or_else(|e| e.into_inner());
        if !in_flight.insert(command.to_string()) {
            return Err("an install is already running for that command".to_string());
        }
        Ok(Self(command.to_string()))
    }
}

impl Drop for InstallInFlight {
    fn drop(&mut self) {
        INSTALLS_IN_FLIGHT
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}

/// #2736 - run ONE install command for a coding agent with no visible window.
/// Reuses the updater step primitives; deliberately does NOT touch
/// `AgentUpdateGate`, the `agent_update_*` events, or the update overlay, so a
/// first-run install stays silent.
pub(crate) fn start_agent_install(
    app: &AppHandle,
    key: String,
    command: String,
    cwd: PathBuf,
) -> Result<(), String> {
    start_agent_install_with_timeout(app, key, command, cwd, UPDATE_STEP_TIMEOUT)
}

/// #2736 - `start_agent_install` with the step timeout injected. Production
/// always passes `UPDATE_STEP_TIMEOUT`; only tests pass a shorter one.
fn start_agent_install_with_timeout(
    app: &AppHandle,
    key: String,
    command: String,
    cwd: PathBuf,
    timeout: Duration,
) -> Result<(), String> {
    let in_flight = InstallInFlight::acquire(&command)?;
    let app = app.clone();
    let task = async move {
        log::info!("[agent-install] running '{command}' for {key}");
        let run = run_agent_install(&app, &key, &command, &cwd, timeout).await;
        drop(in_flight);
        emit_all(
            &app,
            "coding_agent_install_finished",
            install_finished_payload(&key, &command, &run),
        );
    };
    // Test-only: carry a caller's settlement hook into the detached task, so a
    // test can inject a cleanup defect and still observe the emitted frame.
    #[cfg(test)]
    let task = {
        let hook = TARGET_SETTLEMENT_HOOK
            .try_with(|hook| hook.borrow_mut().take())
            .ok()
            .flatten();
        async move {
            match hook {
                Some(hook) => with_target_settlement_hook(hook, task).await,
                None => task.await,
            }
        }
    };
    tauri::async_runtime::spawn(task);
    Ok(())
}

/// #2736 - the outcome of one install run. `ok` is true only for a successful
/// exit with a clean process-tree settlement.
struct AgentInstallRun {
    ok: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    /// The child exit code when a status was actually observed, else `None`
    /// (timeout, spawn failure, wait error, or a unix signal, where
    /// `ExitStatus::code()` is itself `None`).
    exit_code: Option<i32>,
    /// The single-line outcome reason written to the log: `ok`, `exit code N`,
    /// `timed out after 300s (killed)`, a spawn error, a containment failure,
    /// or the joined cleanup defects.
    detail: String,
}

/// #2736 - the AWAITABLE body. This is the test seam: `start_agent_install`
/// takes the in-flight guard, spawns a detached task and returns; the task
/// awaits this, then releases the guard and emits. Tests await it directly and
/// read the outcome instead of racing a detached task.
async fn run_agent_install(
    _app: &AppHandle,
    key: &str,
    command: &str,
    cwd: &Path,
    timeout: Duration,
) -> AgentInstallRun {
    let mut cmd = build_update_step_command(command, cwd, &effective_search_path());
    let run = match TargetProcessOwner::spawn(&mut cmd).await {
        Ok(mut owner) => {
            // This feature has no cancel verb: the sender is never set.
            let (_cancel_tx, mut cancel) = tokio::sync::watch::channel(false);
            let waited = owner.wait(timeout, &mut cancel).await;
            let settlement = owner
                .settle_observing_cancellation(&waited, &mut cancel, || {})
                .await;
            install_run_from_settlement(waited, settlement, timeout)
        }
        Err(outcome) => install_run_from_spawn_failure(outcome),
    };
    log_agent_install_run(key, command, &run);
    run
}

fn install_run_from_spawn_failure(outcome: UpdaterStepOutcome) -> AgentInstallRun {
    let detail = match outcome {
        UpdaterStepOutcome::Failed(error) => error,
        UpdaterStepOutcome::ContainmentFailed => "containment failed".to_string(),
        UpdaterStepOutcome::CleanupFailed(detail) => format!("launch cleanup defective: {detail}"),
        UpdaterStepOutcome::Succeeded | UpdaterStepOutcome::Cancelled => {
            "unexpected spawn outcome".to_string()
        }
    };
    AgentInstallRun {
        ok: false,
        stdout: Vec::new(),
        stderr: Vec::new(),
        exit_code: None,
        detail,
    }
}

/// #2736 - section 2b of the plan. Defects are checked FIRST, as the updater
/// does, so a zero exit with a defective cleanup is never a success.
fn install_run_from_settlement(
    waited: TargetWait,
    settlement: TargetSettlement,
    timeout: Duration,
) -> AgentInstallRun {
    let TargetSettlement {
        stdout,
        stderr,
        defects,
    } = settlement;
    let (ok, exit_code, detail) = if !defects.is_empty() {
        // A status observed before the defective cleanup is still reported.
        let observed = match &waited {
            TargetWait::Exited(Ok(status)) => status.code(),
            _ => None,
        };
        (
            false,
            observed,
            format!("process cleanup defective: {}", defects.join("; ")),
        )
    } else {
        match waited {
            TargetWait::TimedOut => (
                false,
                None,
                format!("timed out after {}s (killed)", timeout.as_secs()),
            ),
            TargetWait::Exited(Err(error)) => (false, None, format!("wait error: {error}")),
            TargetWait::Exited(Ok(status)) if status.success() => {
                (true, status.code(), "ok".to_string())
            }
            TargetWait::Exited(Ok(status)) => (
                false,
                status.code(),
                format!("exit code {}", status.code().unwrap_or(-1)),
            ),
            TargetWait::Cancelled => (false, None, "unexpected cancellation".to_string()),
        }
    };
    AgentInstallRun {
        ok,
        stdout,
        stderr,
        exit_code,
        detail,
    }
}

/// #2736 - the COMPLETE captured output goes to the log, never `output_tail`.
fn log_agent_install_run(key: &str, command: &str, run: &AgentInstallRun) {
    for (level, line) in agent_install_log_lines(key, command, run) {
        log::log!(level, "{line}");
    }
}

/// #2736 - the exact lines `log_agent_install_run` writes, pure so tests can
/// read them without installing a process-wide logger.
fn agent_install_log_lines(
    key: &str,
    command: &str,
    run: &AgentInstallRun,
) -> Vec<(log::Level, String)> {
    let mut lines = vec![(
        log::Level::Info,
        format!(
            "[agent-install] stdout for {key} ({command}): {}",
            String::from_utf8_lossy(&run.stdout)
        ),
    )];
    if !run.stderr.is_empty() {
        lines.push((
            log::Level::Info,
            format!(
                "[agent-install] stderr for {key} ({command}): {}",
                String::from_utf8_lossy(&run.stderr)
            ),
        ));
    }
    lines.push(if run.ok {
        (
            log::Level::Info,
            format!("[agent-install] {key} ({command}): {}", run.detail),
        )
    } else {
        (
            log::Level::Warn,
            format!("[agent-install] {key} ({command}) FAILED: {}", run.detail),
        )
    });
    lines
}

/// #2736 - one stream for the event: the last `OUTPUT_TAIL_BYTES`, behind one
/// marker line when the stream was cut. The log keeps the full output.
fn event_output(bytes: &[u8]) -> String {
    let tail = output_tail(bytes);
    if bytes.len() > OUTPUT_TAIL_BYTES {
        format!(
            "[... truncated; showing the last {OUTPUT_TAIL_BYTES} bytes. The full output is in the app log.]\n{tail}"
        )
    } else {
        tail
    }
}

/// #2736 - the ONLY builder of the `coding_agent_install_finished` frame. A
/// successful run carries no output.
fn install_finished_payload(key: &str, command: &str, run: &AgentInstallRun) -> Value {
    let (stdout, stderr) = if run.ok {
        (String::new(), String::new())
    } else {
        (event_output(&run.stdout), event_output(&run.stderr))
    };
    json!({
        "key": key,
        "command": command,
        "ok": run.ok,
        "detail": run.detail,
        "exitCode": run.exit_code,
        "stdout": stdout,
        "stderr": stderr,
    })
}

#[cfg(test)]
async fn run_update_sequence(target: &UpdateTarget, step_timeout: Duration) -> AgentUpdateResult {
    let gate = AgentUpdateGate::new();
    gate.mark_started_with_nodes(vec![AgentUpdateNode {
        command: target.command.clone(),
        label: target.label.clone(),
        update_commands: target.commands.clone(),
        install_before: None,
    }]);
    let mut owner = TargetOwnerSlot::default();
    let outcome = run_update_sequence_cancellable(target, step_timeout, &gate, &mut owner).await;
    let (ok, outcome, error) = match outcome {
        UpdaterStepOutcome::Succeeded => (true, AgentUpdateOutcome::Succeeded, None),
        UpdaterStepOutcome::Failed(error) => (false, AgentUpdateOutcome::Failed, Some(error)),
        UpdaterStepOutcome::Cancelled => (false, AgentUpdateOutcome::Cancelled, None),
        UpdaterStepOutcome::CleanupFailed(_) => (
            false,
            AgentUpdateOutcome::Failed,
            Some(CANCELLATION_CLEANUP_ERROR.to_string()),
        ),
        UpdaterStepOutcome::ContainmentFailed => (
            false,
            AgentUpdateOutcome::Failed,
            Some("Updater process-tree containment unavailable; update stopped.".to_string()),
        ),
    };
    AgentUpdateResult {
        command: target.command.clone(),
        label: target.label.clone(),
        ok,
        outcome,
        error,
        install_before: None,
        install_after: None,
        change: AgentUpdateChange::Unknown,
        verification_error: None,
    }
}

fn successful_update_result(
    target: &UpdateTarget,
    install_before: InstallState,
    install_after: InstallState,
) -> AgentUpdateResult {
    let comparable = install_before.status == InstallStatus::Installed
        && install_after.status == InstallStatus::Installed
        && install_before
            .version
            .as_deref()
            .is_some_and(|version| !version.is_empty())
        && install_after
            .version
            .as_deref()
            .is_some_and(|version| !version.is_empty());
    let (change, verification_error) = if comparable {
        (
            if install_before.version == install_after.version {
                AgentUpdateChange::Unchanged
            } else {
                AgentUpdateChange::Changed
            },
            None,
        )
    } else {
        let detail = install_after
            .detail
            .as_deref()
            .filter(|detail| !detail.is_empty())
            .or_else(|| {
                install_before
                    .detail
                    .as_deref()
                    .filter(|detail| !detail.is_empty())
            })
            .unwrap_or("Version comparison unavailable.")
            .to_string();
        (AgentUpdateChange::Unknown, Some(detail))
    };
    AgentUpdateResult {
        command: target.command.clone(),
        label: target.label.clone(),
        ok: true,
        outcome: AgentUpdateOutcome::Succeeded,
        error: None,
        install_before: Some(install_before),
        install_after: Some(install_after),
        change,
        verification_error,
    }
}

fn failed_update_result(
    target: &UpdateTarget,
    install_before: Option<InstallState>,
    install_after: Option<InstallState>,
    error: impl Into<String>,
) -> AgentUpdateResult {
    AgentUpdateResult {
        command: target.command.clone(),
        label: target.label.clone(),
        ok: false,
        outcome: AgentUpdateOutcome::Failed,
        error: Some(error.into()),
        install_before,
        install_after,
        change: AgentUpdateChange::Unknown,
        verification_error: None,
    }
}

fn cancelled_update_result(
    target: &UpdateTarget,
    install_before: Option<InstallState>,
) -> AgentUpdateResult {
    AgentUpdateResult {
        command: target.command.clone(),
        label: target.label.clone(),
        ok: false,
        outcome: AgentUpdateOutcome::Cancelled,
        error: None,
        install_before,
        install_after: None,
        change: AgentUpdateChange::Unknown,
        verification_error: None,
    }
}

struct SettledPostProbe(InstallState);

struct TargetTerminalizer {
    app: AppHandle,
    gate: Arc<AgentUpdateGate>,
    target: UpdateTarget,
    install_before: Option<InstallState>,
    settled_post_probe: Option<SettledPostProbe>,
}

impl TargetTerminalizer {
    fn new(app: AppHandle, gate: Arc<AgentUpdateGate>, target: UpdateTarget) -> Self {
        Self {
            app,
            gate,
            target,
            install_before: None,
            settled_post_probe: None,
        }
    }

    fn store_install_before(&mut self, install: InstallState) {
        self.install_before = Some(install);
    }

    fn store_settled_post_probe(&mut self, install: InstallState) {
        self.settled_post_probe = Some(SettledPostProbe(install));
    }

    async fn publish(self, candidate: AgentUpdateResult) -> AgentUpdateResult {
        let first = self.gate.publish_terminal(candidate.clone(), false);
        let terminal = match first {
            TerminalPublish::Published => candidate,
            TerminalPublish::AlreadyTerminal => self
                .gate
                .snapshot()
                .results
                .into_iter()
                .find(|result| result.command == self.target.command)
                .unwrap_or(candidate),
            TerminalPublish::CancellationReserved => {
                self.gate
                    .acknowledge_unowned_cancellation(&self.target.command);
                self.gate
                    .wait_response_release_if_cancelled(&self.target.command)
                    .await;
                let reserved_result = if candidate.error.as_deref()
                    == Some(CANCELLATION_CLEANUP_ERROR)
                    || candidate.error.as_deref() == Some("Update task failed unexpectedly.")
                {
                    candidate
                } else if self.gate.supervisor_failure_reserved(&self.target.command) {
                    failed_update_result(
                        &self.target,
                        self.install_before.clone(),
                        self.settled_post_probe
                            .as_ref()
                            .map(|settled| settled.0.clone()),
                        "Update supervisor stopped unexpectedly.",
                    )
                } else {
                    cancelled_update_result(&self.target, self.install_before.clone())
                };
                match self.gate.publish_terminal(reserved_result.clone(), true) {
                    TerminalPublish::Published => reserved_result,
                    TerminalPublish::AlreadyTerminal => self
                        .gate
                        .snapshot()
                        .results
                        .into_iter()
                        .find(|result| result.command == self.target.command)
                        .unwrap_or(reserved_result),
                    TerminalPublish::CancellationReserved => unreachable!(
                        "reserved terminal publication explicitly accepts the reservation"
                    ),
                }
            }
        };
        if matches!(
            self.gate
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .controls
                .get(&self.target.command)
                .map(|control| control.terminal_emitted),
            Some(false)
        ) {
            emit_all(&self.app, "agent_update_command_finished", json!(terminal));
            self.gate.command_event_emitted(&self.target.command);
        }
        terminal
    }
}

/// #1551 - every agent-update event reaches Tauri windows AND WebSocket clients.
/// The broadcaster is managed by `lib.rs` before `setup`; the fallback keeps unit
/// tests that build a bare mock app working. The scrape test
/// `agent_update_emits_only_through_emit_all` pins that this is the only
/// `.emit(` call site of the production part of this file.
fn emit_all(app: &AppHandle, event: &str, payload: Value) {
    match app.try_state::<WsBroadcaster>() {
        Some(broadcaster) => broadcast_all(app, &broadcaster, event, &payload),
        None => {
            let _ = app.emit(event, payload);
        }
    }
}

/// Exercise the production event serializer/broadcaster for transport parity.
/// The WebSocket transport test compares the untouched text frames emitted by
/// this same path for both Tauri- and Web-routed cancellation state.
#[cfg(test)]
pub(crate) fn emit_agent_update_transport_contract_for_test(app: &AppHandle) {
    emit_all(
        app,
        "agent_update_command_verifying",
        json!(AgentUpdateCommandRef {
            command: "claude".to_string(),
            label: "Claude".to_string(),
        }),
    );
    emit_all(
        app,
        "agent_update_command_finished",
        json!(AgentUpdateResult {
            command: "claude".to_string(),
            label: "Claude".to_string(),
            ok: false,
            outcome: AgentUpdateOutcome::Cancelled,
            error: None,
            install_before: Some(InstallState {
                status: InstallStatus::Installed,
                version: Some("1.2.3".to_string()),
                path: None,
                detail: None,
                seq: 0,
            }),
            install_after: None,
            change: AgentUpdateChange::Unknown,
            verification_error: None,
        }),
    );
}

/// #1551 - the two effects of the pass end, named fields so the supervisor cannot
/// pass them in the wrong slots.
struct PassEnd<I: FnOnce(), E: FnOnce()> {
    invalidate: I,
    emit: E,
}

/// #1551 - the ONLY production transition to `finished`. Order is load-bearing:
/// invalidate cache -> store the final snapshot while unfinished -> enqueue the
/// pass-finished event -> atomically open terminal fences/mark finished -> notify
/// admission. Post-update probe scheduling happens only after this returns.
fn finish_pass<I: FnOnce(), E: FnOnce()>(
    gate: &AgentUpdateGate,
    results: Vec<AgentUpdateResult>,
    end: PassEnd<I, E>,
) {
    (end.invalidate)();
    gate.store_final_results(results);
    (end.emit)();
    gate.release_finished();
}

async fn run_target_probe(
    app: &AppHandle,
    gate: &AgentUpdateGate,
    target: &UpdateTarget,
    work: TargetWork,
    owner_slot: &mut TargetOwnerSlot,
) -> TargetProbeOutcome {
    let Some((epoch, _)) = gate.begin_step(&target.command, work) else {
        return TargetProbeOutcome::Cancelled;
    };
    #[cfg(test)]
    gate.hold_after_step_begin_for_test(&target.command).await;
    let StepAdmission::Committed { epoch, cancel } = gate.commit_step(&target.command, work, epoch)
    else {
        return TargetProbeOutcome::Cancelled;
    };
    #[cfg(test)]
    gate.hold_after_step_commit_for_test(&target.command).await;
    owner_slot.arm(work, epoch, cancel);
    let outcome = {
        let lease = owner_slot.lease.as_mut().expect("committed probe lease");
        probe_command_install_state_cancellable(
            &target.command,
            &mut lease.cancel,
            &mut owner_slot.probe,
            || {
                gate.owner_installed(&target.command, work, epoch);
                if work == TargetWork::PostProbe {
                    emit_all(
                        app,
                        "agent_update_command_verifying",
                        json!(AgentUpdateCommandRef {
                            command: target.command.clone(),
                            label: target.label.clone(),
                        }),
                    );
                }
            },
            || {
                gate.owner_observed_cancellation(&target.command, work, epoch);
            },
        )
        .await
    };
    let reserved = if owner_slot.probe.is_some() {
        if matches!(outcome, TargetProbeOutcome::Cancelled) {
            gate.owner_observed_cancellation(&target.command, work, epoch);
        }
        owner_slot.owner_returned(gate, &target.command)
    } else {
        owner_slot.complete_without_owner(gate, &target.command)
    };
    if reserved && matches!(outcome, TargetProbeOutcome::Completed(_)) {
        TargetProbeOutcome::Cancelled
    } else {
        outcome
    }
}

async fn run_update_target_body(
    app: &AppHandle,
    gate: &Arc<AgentUpdateGate>,
    target: &UpdateTarget,
    terminalizer: &mut TargetTerminalizer,
    owner_slot: &mut TargetOwnerSlot,
) -> AgentUpdateResult {
    let install_before =
        match run_target_probe(app, gate, target, TargetWork::PreProbe, owner_slot).await {
            TargetProbeOutcome::Completed(install) => install,
            TargetProbeOutcome::Cancelled => return cancelled_update_result(target, None),
            TargetProbeOutcome::CleanupFailed(detail) => {
                log::warn!(
                    "[agent-update] pre-probe cleanup defective for {} ({}): {detail}",
                    target.label,
                    target.command
                );
                return failed_update_result(target, None, None, CANCELLATION_CLEANUP_ERROR);
            }
        };
    terminalizer.store_install_before(install_before.clone());

    if gate.cancellation_reserved(&target.command) {
        gate.acknowledge_unowned_cancellation(&target.command);
        return cancelled_update_result(target, Some(install_before));
    }

    let node =
        gate.mark_command_started(&target.command, &target.label, Some(install_before.clone()));
    emit_all(app, "agent_update_command_started", json!(node));
    match run_update_sequence_cancellable(target, UPDATE_STEP_TIMEOUT, gate, owner_slot).await {
        UpdaterStepOutcome::Succeeded => {}
        UpdaterStepOutcome::Cancelled => {
            return cancelled_update_result(target, Some(install_before))
        }
        UpdaterStepOutcome::CleanupFailed(detail) => {
            log::warn!(
                "[agent-update] updater cleanup defective for {} ({}): {detail}",
                target.label,
                target.command
            );
            return failed_update_result(
                target,
                Some(install_before),
                None,
                CANCELLATION_CLEANUP_ERROR,
            );
        }
        UpdaterStepOutcome::ContainmentFailed => {
            return failed_update_result(
                target,
                Some(install_before),
                None,
                "Updater process-tree containment unavailable; update stopped.",
            )
        }
        UpdaterStepOutcome::Failed(error) => {
            return failed_update_result(target, Some(install_before), None, error)
        }
    }

    let install_after =
        match run_target_probe(app, gate, target, TargetWork::PostProbe, owner_slot).await {
            TargetProbeOutcome::Completed(install) => install,
            TargetProbeOutcome::Cancelled => {
                return cancelled_update_result(target, Some(install_before))
            }
            TargetProbeOutcome::CleanupFailed(detail) => {
                log::warn!(
                    "[agent-update] post-probe cleanup defective for {} ({}): {detail}",
                    target.label,
                    target.command
                );
                return failed_update_result(
                    target,
                    Some(install_before),
                    None,
                    CANCELLATION_CLEANUP_ERROR,
                );
            }
        };
    terminalizer.store_settled_post_probe(install_after.clone());
    #[cfg(test)]
    gate.hold_after_post_probe_settled_for_test(&target.command)
        .await;
    successful_update_result(target, install_before, install_after)
}

/// One retained target has one outer terminalizer and an owner slot outside the
/// caught body. A panic therefore triggers asynchronous owner settlement before
/// any failed result can be published.
async fn run_update_target(
    app: AppHandle,
    gate: Arc<AgentUpdateGate>,
    target: UpdateTarget,
) -> AgentUpdateResult {
    let mut terminalizer = TargetTerminalizer::new(app.clone(), Arc::clone(&gate), target.clone());
    let mut owner_slot = TargetOwnerSlot::default();
    let body = std::panic::AssertUnwindSafe(run_update_target_body(
        &app,
        &gate,
        &target,
        &mut terminalizer,
        &mut owner_slot,
    ))
    .catch_unwind()
    .await;
    let candidate = match body {
        Ok(result) => result,
        Err(_) => {
            let cleanup_failure = owner_slot.settle_after_panic(&gate, &target.command).await;
            if let Some(detail) = cleanup_failure.as_deref() {
                log::warn!(
                    "[agent-update] process cleanup after target panic defective for {} ({}): {detail}",
                    target.label,
                    target.command
                );
            }
            failed_update_result(
                &target,
                terminalizer.install_before.clone(),
                terminalizer
                    .settled_post_probe
                    .as_ref()
                    .map(|settled| settled.0.clone()),
                if cleanup_failure.is_some() {
                    CANCELLATION_CLEANUP_ERROR
                } else {
                    "Update task failed unexpectedly."
                },
            )
        }
    };
    terminalizer.publish(candidate).await
}

/// #1551 - a task that panicked never ran `mark_command_finished`: settle it here so the
/// row leaves `running` and every surface receives its `command_finished`.
#[cfg(test)]
fn settle_joined_update(
    app: &AppHandle,
    gate: &AgentUpdateGate,
    target: &UpdateTarget,
    joined: Result<AgentUpdateResult, tokio::task::JoinError>,
) -> AgentUpdateResult {
    match joined {
        Ok(result) => result,
        Err(_) => {
            let result = AgentUpdateResult {
                command: target.command.clone(),
                label: target.label.clone(),
                ok: false,
                outcome: AgentUpdateOutcome::Failed,
                error: Some("Update task failed unexpectedly.".to_string()),
                install_before: None,
                install_after: None,
                change: AgentUpdateChange::Unknown,
                verification_error: None,
            };
            gate.mark_command_finished(result.clone());
            emit_all(app, "agent_update_command_finished", json!(result));
            result
        }
    }
}

/// #1551/#1967 - the agents of this boot's pass in catalog order: every target of
/// `plan.updates` (decided `true`) and `plan.prompts` (to be asked), one node per
/// command (first catalog occurrence, like `build_update_plan`). A command whose
/// first effective entry has an empty sequence gets no node at all: empty
/// winners produce no prompt, update, row or probe (#1967). Pure; the order is
/// the timeline order on every surface.
pub fn pass_nodes(
    catalog: &[CodingAgentDefinition],
    plan: &AgentUpdatePlan,
) -> Vec<AgentUpdateNode> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut nodes = Vec::new();
    for entry in catalog {
        if !seen.insert(entry.command.as_str()) {
            continue;
        }
        if entry.update_commands.is_empty() {
            continue;
        }
        if let Some(target) = plan
            .updates
            .iter()
            .chain(plan.prompts.iter())
            .find(|t| t.command == entry.command)
        {
            nodes.push(AgentUpdateNode {
                command: target.command.clone(),
                label: target.label.clone(),
                update_commands: target.commands.clone(),
                install_before: None,
            });
        }
    }
    nodes
}

/// #1551 - a prompted target leaves the pass (answered No, or expired): prune its node and tell
/// every surface, exactly once. Called by the prompt loop only, after that prompt's closure.
fn skip_prompted_target(app: &AppHandle, gate: &AgentUpdateGate, command: &str) {
    if let Some(skipped) = gate.mark_command_skipped(command) {
        emit_all(app, "agent_update_command_skipped", json!(skipped));
    }
}

/// #1551 round 5 - after the pass is finished, re-read the install state of every UPDATED
/// command in the background through the cache (single-flight with any Settings-triggered
/// probe), so the timeline's finished nodes can show `<before> -> <after>` and Settings sees
/// the post-update state without asking. Runs only after `finish_pass` returned (program
/// order in the pass task), so every ticket it opens carries the post-pass generation and
/// no probe can overlap an update (every update ended before `finished`).
fn schedule_post_update_probes(app: &AppHandle, updated: &[UpdateTarget]) {
    let Some(cache) = app.try_state::<Arc<AgentInstallCache>>() else {
        return;
    };
    let now = Instant::now();
    for target in updated {
        if let Scheduling::Began(ticket) =
            cache.lookup_or_begin(&target.command, now, INSTALL_CACHE_TTL, true)
        {
            spawn_probe_task(app, &target.command, ticket, production_probe());
        }
    }
}

struct SupervisedTargetTask {
    target: UpdateTarget,
    handle: Option<tokio::task::JoinHandle<AgentUpdateResult>>,
}

#[derive(Default)]
struct PassRuntime {
    started: bool,
    known_targets: Vec<UpdateTarget>,
    updated_targets: Vec<UpdateTarget>,
    tasks: Vec<SupervisedTargetTask>,
}

struct PassSupervisor {
    app: AppHandle,
    gate: Arc<AgentUpdateGate>,
}

impl PassSupervisor {
    fn spawn(app: AppHandle, gate: Arc<AgentUpdateGate>) -> tokio::sync::oneshot::Receiver<()> {
        let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
        tauri::async_runtime::spawn(async move {
            Self { app, gate }.run().await;
            let _ = finished_tx.send(());
        });
        finished_rx
    }

    async fn terminalize_pending_cancel(&self, target: &UpdateTarget) {
        self.gate.acknowledge_unowned_cancellation(&target.command);
        TargetTerminalizer::new(self.app.clone(), Arc::clone(&self.gate), target.clone())
            .publish(cancelled_update_result(target, None))
            .await;
    }

    async fn run_body(&self, runtime: &mut PassRuntime) {
        let settings = match self.app.state::<SettingsState>().current_snapshot().await {
            Ok(settings) => settings,
            Err(error) => {
                log::debug!("[agent-update] startup pass skipped: {error}");
                return;
            }
        };
        let catalog = match load_catalog_for_settings(&settings) {
            Ok(catalog) => catalog,
            Err(unavailable) => {
                // #1967 P4 - never substitute embedded commands and never run a
                // vendor command or install probe for an unavailable catalog.
                // Return through the normal supervisor completion/cleanup path;
                // `finalize` still releases the gate and invalidates the cache.
                log::warn!("[agent-update] startup pass skipped: {unavailable}");
                return;
            }
        };
        let default_cwd = primary_project_root(&settings)
            .or_else(crate::config::config_dir)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let registered_commands: HashSet<String> = settings
            .agents
            .iter()
            .map(|agent| agent.command.clone())
            .collect();
        let plan = build_update_plan(
            &catalog,
            &registered_commands,
            &settings.agent_auto_update_by_command,
            default_cwd,
        );
        if plan.prompts.is_empty() && plan.updates.is_empty() {
            log::debug!("[agent-update] nothing to prompt or update; skipping");
            return;
        }

        runtime.known_targets = plan
            .updates
            .iter()
            .chain(plan.prompts.iter())
            .cloned()
            .collect();
        let nodes = pass_nodes(&catalog, &plan);
        self.gate.mark_started_with_nodes(nodes.clone());
        emit_all(
            &self.app,
            "agent_updates_started",
            json!({ "nodes": nodes }),
        );
        runtime.started = true;

        if !plan.prompts.is_empty() {
            log::info!(
                "[agent-update] prompt phase started: {} command(s) await SI/NO ({}s each, default No): [{}]",
                plan.prompts.len(),
                PROMPT_TIMEOUT.as_secs(),
                plan.prompts
                    .iter()
                    .map(|prompt| prompt.command.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }

        let mut updates = plan.updates;
        for pending in &plan.prompts {
            if self.gate.cancellation_reserved(&pending.command) {
                self.terminalize_pending_cancel(pending).await;
                continue;
            }
            let rx = self.gate.register_prompt(&pending.command, &pending.label);
            emit_all(
                &self.app,
                "agent_update_prompt",
                json!(AgentUpdatePrompt {
                    command: pending.command.clone(),
                    label: pending.label.clone(),
                }),
            );
            match tokio::time::timeout(PROMPT_TIMEOUT, rx).await {
                Ok(Ok(true)) => updates.push(pending.clone()),
                Ok(Ok(false)) => skip_prompted_target(&self.app, &self.gate, &pending.command),
                Ok(Err(_)) | Err(_) => {
                    let serial = self.gate.answer_serial.lock().await;
                    if self.gate.cancellation_reserved(&pending.command) {
                        self.gate.acknowledge_unowned_cancellation(&pending.command);
                        drop(serial);
                        self.terminalize_pending_cancel(pending).await;
                    } else {
                        if let Some(closed) = self.gate.drop_pending(&pending.command) {
                            emit_all(&self.app, "agent_update_prompt_closed", json!(closed));
                        }
                        skip_prompted_target(&self.app, &self.gate, &pending.command);
                    }
                }
            }
        }

        runtime.updated_targets = updates.clone();
        for target in updates {
            if self.gate.cancellation_reserved(&target.command) {
                self.terminalize_pending_cancel(&target).await;
                continue;
            }
            let app = self.app.clone();
            let gate = Arc::clone(&self.gate);
            let task_target = target.clone();
            runtime.tasks.push(SupervisedTargetTask {
                target,
                handle: Some(tokio::spawn(async move {
                    run_update_target(app, gate, task_target).await
                })),
            });
        }

        for task in &mut runtime.tasks {
            let joined = task
                .handle
                .as_mut()
                .expect("supervisor target handle")
                .await;
            task.handle = None;
            if joined.is_err() {
                TargetTerminalizer::new(
                    self.app.clone(),
                    Arc::clone(&self.gate),
                    task.target.clone(),
                )
                .publish(failed_update_result(
                    &task.target,
                    None,
                    None,
                    "Update task failed unexpectedly.",
                ))
                .await;
            }
        }
    }

    async fn settle_pass_panic(&self, runtime: &mut PassRuntime) {
        if let Some(closed) = self.gate.reserve_supervisor_failure() {
            emit_all(&self.app, "agent_update_prompt_closed", json!(closed));
        }
        for task in &mut runtime.tasks {
            if let Some(handle) = task.handle.as_mut() {
                let _ = handle.await;
                task.handle = None;
            }
        }
        for node in self.gate.unfinished_nodes() {
            let target = runtime
                .known_targets
                .iter()
                .find(|target| target.command == node.command)
                .cloned()
                .unwrap_or(UpdateTarget {
                    command: node.command.clone(),
                    label: node.label.clone(),
                    commands: node.update_commands.clone(),
                    cwd: PathBuf::from("."),
                });
            let mut terminalizer =
                TargetTerminalizer::new(self.app.clone(), Arc::clone(&self.gate), target.clone());
            terminalizer.install_before = node.install_before;
            let install_before = terminalizer.install_before.clone();
            terminalizer
                .publish(failed_update_result(
                    &target,
                    install_before,
                    None,
                    "Update supervisor stopped unexpectedly.",
                ))
                .await;
        }
    }

    async fn finalize(&self, runtime: PassRuntime) {
        self.gate.wait_all_terminal_publications().await;
        let results = self.gate.snapshot().results;
        let announced = results.clone();
        let cache = self.app.try_state::<Arc<AgentInstallCache>>();
        finish_pass(
            &self.gate,
            results,
            PassEnd {
                invalidate: || {
                    if let Some(cache) = cache.as_ref() {
                        cache.invalidate_all();
                    }
                },
                emit: || {
                    if runtime.started {
                        emit_all(
                            &self.app,
                            "agent_updates_finished",
                            json!({ "results": announced }),
                        );
                    }
                },
            },
        );
        schedule_post_update_probes(&self.app, &runtime.updated_targets);
    }

    async fn run(self) {
        let mut runtime = PassRuntime::default();
        let outcome = std::panic::AssertUnwindSafe(self.run_body(&mut runtime))
            .catch_unwind()
            .await;
        if outcome.is_err() {
            self.settle_pass_panic(&mut runtime).await;
        }
        self.finalize(runtime).await;
    }
}

/// The public startup await owns only a completion receiver. The private
/// supervisor task owns every target and finalization fence, so aborting or
/// dropping this outer future cannot abort the pass.
pub async fn run_startup_updates(app: AppHandle, gate: Arc<AgentUpdateGate>) {
    warm_login_shell_path().await;
    let completion = PassSupervisor::spawn(app, gate);
    let _ = completion.await;
}

#[cfg(test)]
mod tests;
