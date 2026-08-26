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
//! No version detection/comparison by decision: the update command itself
//! decides whether something is new.
//!
//! `AgentUpdateGate` is the process-local blocker every session open waits on
//! (`create_session_inner_impl`, the single chokepoint). The gate is released by
//! a `FinishGuard` on EVERY exit path of the startup task (including panics), so
//! sessions never wedge and the splash never sticks.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::agent_version::{
    probe_version, version_probe_args, AgentInstallCache, Completion, InstallState, ProbeOutcome,
    ProbeTicket, Scheduling, INSTALL_CACHE_TTL, PROBE_TIMEOUT,
};
use crate::config::agent_command::{
    is_bare_program_token, normalize_legacy_agent_command, resolve_program,
};
use crate::config::coding_agents_catalog::{
    load_catalog_for_settings, primary_project_root, CodingAgentDefinition,
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
/// Bounded wait for the pipe readers to finish draining.
const READER_JOIN_TIMEOUT: Duration = Duration::from_secs(5);

/// One command's update outcome, shown in the sidebar as a red toast on failure.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUpdateResult {
    pub command: String,
    pub label: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Snapshot of the whole startup run, served to a late-mounting sidebar.
#[derive(Debug, Clone, serde::Serialize)]
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
}

/// The pending SI/NO question for one command.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUpdatePrompt {
    pub command: String,
    pub label: String,
}

/// #1551 - the identity of one command of this boot's pass, carried by the
/// closure, skip, and running-set payloads.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUpdateCommandRef {
    pub command: String,
    pub label: String,
}

/// #1551 - one agent of this boot's pass in pass order; install_before is the
/// pre-update probe result once it ran (never cached, seq 0).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUpdateNode {
    pub command: String,
    pub label: String,
    pub update_commands: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_before: Option<InstallState>,
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
}

/// #1551 - where one prompted command stands, as `answer_prompt` classifies it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptState {
    NotPrompted,
    Pending,
    Expired,
    Answered(bool),
}

/// #1551 - a pending prompt taken out of the gate by an answer. Its sender is
/// handed back UNSENT so the caller can emit the closure BEFORE releasing the
/// prompt loop.
pub struct ClaimedPrompt {
    closed: AgentUpdateCommandRef,
    tx: tokio::sync::oneshot::Sender<bool>,
}

impl ClaimedPrompt {
    pub fn closed(&self) -> &AgentUpdateCommandRef {
        &self.closed
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
            }),
            release: tokio::sync::Notify::new(),
            answer_serial: tokio::sync::Mutex::new(()),
        }
    }

    pub fn mark_started(&self) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .started = true;
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
        debug_assert!(!state.pending.contains_key(command), "duplicate prompt for {command}");
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
            Some(pending) => AnswerClaim::Claimed(ClaimedPrompt {
                closed: AgentUpdateCommandRef {
                    command: command.to_string(),
                    label: pending.label,
                },
                tx: pending.tx,
            }),
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
        state
            .running
            .retain(|entry| entry.command != result.command);
        if let Some(existing) = state
            .results
            .iter_mut()
            .find(|existing| existing.command == result.command)
        {
            *existing = result;
        } else {
            state.results.push(result);
        }
    }

    /// #1551 - round 5: the pass starts with its node set in ONE lock take, so no
    /// snapshot can observe started without nodes.
    pub fn mark_started_with_nodes(&self, nodes: Vec<AgentUpdateNode>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.started = true;
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

    /// Release the gate. IDEMPOTENT: once `finished` is set it stays set and a
    /// repeated call only overwrites `results`. FIRST clears the pending prompt
    /// and all pending oneshot senders (in the same lock section), so a
    /// post-panic snapshot never resurrects a dead prompt that no event is left
    /// to clear; THEN sets finished + results and wakes the waiters.
    pub fn mark_finished(&self, results: Vec<AgentUpdateResult>) {
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.pending.clear();
            state.pending_prompt = None;
            // #1551 - a panic or a lost event can never leave a stuck `Updating...`.
            state.running.clear();
            state.finished = true;
            state.results = results;
        }
        self.release.notify_waiters();
    }

    pub fn snapshot(&self) -> AgentUpdateStatus {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        AgentUpdateStatus {
            in_progress: state.started && !state.finished,
            prompt: state.pending_prompt.clone(),
            results: state.results.clone(),
            running: state.running.clone(),
            answered: state.answered.clone(),
            nodes: state.nodes.clone(),
        }
    }

    /// Await gate release. Race-free for late waiters: the notification is
    /// registered BEFORE the finished check, so a `mark_finished` between the
    /// two cannot be missed.
    pub async fn wait_until_done(&self) {
        loop {
            let notified = self.release.notified();
            if self.state.lock().unwrap_or_else(|e| e.into_inner()).finished {
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
/// 1. Distinct commands in catalog order (first occurrence wins). `label` and
///    `commands` come from the FIRST catalog entry (in order) whose
///    `update_commands` is non-empty. A command with NO non-empty sequence is
///    skipped entirely: never prompted, never updated.
/// 2. `answers[command]` absent -> prompt (default No on timeout).
/// 3. `Some(true)` -> update now.
/// 4. `Some(false)` -> nothing.
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
        if !seen.insert(entry.command.as_str()) {
            continue; // first occurrence wins
        }
        let Some(sequence) = catalog
            .iter()
            .find(|e| e.command == entry.command && !e.update_commands.is_empty())
            .map(|e| e.update_commands.clone())
        else {
            continue; // no entry for this command carries an update sequence
        };
        let target = UpdateTarget {
            command: entry.command.clone(),
            label: entry.label.clone(),
            commands: sequence,
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

/// #1551 - one row per catalog entry with a non-empty (backfilled) update sequence, in
/// catalog order, NO dedup (duplicate-command entries show identical command-keyed
/// install state). Cursor drops out because it ships no update command. Commands
/// without an install entry are `checking` (seq 0).
pub fn build_update_overview_rows(
    catalog: &[CodingAgentDefinition],
    install_by_command: &HashMap<String, InstallState>,
) -> Vec<AgentUpdateOverviewRow> {
    catalog
        .iter()
        .filter(|entry| !entry.update_commands.is_empty())
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
pub async fn probe_command_install_state(command: &str) -> InstallState {
    let Ok(normalized) = normalize_legacy_agent_command(command) else {
        return InstallState::missing("empty command".to_string());
    };
    let token = normalized.shell;
    let bare = is_bare_program_token(&token);
    let Some(path) = resolve_program(&token) else {
        return InstallState::missing(if bare {
            format!("'{token}' was not found on PATH")
        } else {
            format!("'{token}' is not a file")
        });
    };
    if !bare {
        return InstallState::unprobed(&path, "explicit path: version not probed".to_string());
    }
    let stem = Path::new(&token)
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let Some(args) = version_probe_args(&stem) else {
        return InstallState::unprobed(&path, format!("no built-in version probe for '{stem}'"));
    };
    match probe_version(&path, args, PROBE_TIMEOUT).await {
        ProbeOutcome::Version(version) => InstallState::installed(version, &path),
        ProbeOutcome::Failed(detail) => InstallState::probe_failed(&path, detail),
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
pub async fn update_overview(
    app: &AppHandle,
    settings: &SettingsState,
    gate: &AgentUpdateGate,
    cache: &Arc<AgentInstallCache>,
) -> Vec<AgentUpdateOverviewRow> {
    update_overview_with(app, settings, gate, cache, production_probe()).await
}

/// `update_overview` with an injectable probe so the scheduling tests control timing.
pub async fn update_overview_with(
    app: &AppHandle,
    settings: &SettingsState,
    gate: &AgentUpdateGate,
    cache: &Arc<AgentInstallCache>,
    probe: ProbeFn,
) -> Vec<AgentUpdateOverviewRow> {
    let settings = settings.read().await.clone();
    let catalog = load_catalog_for_settings(&settings);
    // Read the gate ONCE, before any cache operation: that order is what makes
    // "finished implies the post-pass generation" hold (plan 5.4 step 8).
    let pass_finished = gate.is_finished();
    let now = Instant::now();
    let mut install_by_command: HashMap<String, InstallState> = HashMap::new();
    let mut seen: HashSet<&str> = HashSet::new();

    for entry in &catalog {
        if entry.update_commands.is_empty() {
            continue;
        }
        if !seen.insert(entry.command.as_str()) {
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

    build_update_overview_rows(&catalog, &install_by_command)
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
        PromptState::Pending | PromptState::Expired => {}
    }
    persist().await?;
    match gate.claim_answer(command, enabled) {
        AnswerClaim::Claimed(claimed) => {
            // (1) the closure is enqueued on every surface (WebSocket queues, Tauri emit) ...
            emit_all(app, "agent_update_prompt_closed", json!(claimed.closed()));
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

/// Execute one command's update sequence IN ORDER, fail-fast: the first failed
/// step stops the chain. Each step is a full shell string run through
/// `cmd.exe /C` (Windows) or `sh -c` (elsewhere), cwd = the target's directory,
/// no stdin, output tail-captured concurrently with the wait (a chatty command
/// must never block on the 64KB pipe buffer). Per-step timeout kills the WHOLE
/// tree (Windows JobObject, Unix process-group kill).
async fn run_update_sequence(target: &UpdateTarget, step_timeout: Duration) -> AgentUpdateResult {
    let mut first_error: Option<String> = None;

    for cmd in &target.commands {
        log::info!(
            "[agent-update] running '{}' for {} ({})",
            cmd,
            target.label,
            target.command
        );

        let mut command = {
            let mut c = if cfg!(windows) {
                let mut c = tokio::process::Command::new("cmd.exe");
                c.arg("/C").arg(cmd);
                c
            } else {
                let mut c = tokio::process::Command::new("sh");
                c.arg("-c").arg(cmd);
                c
            };
            c.current_dir(&target.cwd);
            c.stdin(Stdio::null());
            c.stdout(Stdio::piped());
            c.stderr(Stdio::piped());
            c.kill_on_drop(true); // safety net: a dropped child never lingers
            c
        };
        // The GUI binary is `windows_subsystem = "windows"` and never owns a
        // console, so UNCONDITIONALLY suppress the child's console window
        // (without the flag every update step pops one over the splash).
        #[cfg(windows)]
        {
            // CREATE_NO_WINDOW. `tokio::process::Command` exposes this as an
            // inherent method on Windows (no trait import needed).
            command.creation_flags(0x0800_0000);
        }
        // Unix tree-kill support: descendants inherit the group (`sh -c` creates
        // no new group); on timeout the whole tree dies with one group kill.
        #[cfg(unix)]
        {
            // `tokio::process::Command` exposes this as an inherent method on
            // Unix, mirroring `creation_flags` above (no trait import needed).
            command.process_group(0);
        }

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(e) => {
                first_error = Some(e.to_string());
                break;
            }
        };
        // F2 (round 3) pin: job dropped at step end on EVERY path - no
        // survivors from an update step on Windows (deliberate, plan 5.2/18).
        // The timeout arm keeps `terminate()` and the R1 truncation path keeps
        // `job.take()`; on the plain-success path KILL_ON_JOB_CLOSE reaps any
        // fully detached descendant the update command left behind. On Unix
        // there is no job: detached descendants survive the step (group-kill
        // runs only on timeout/truncation).
        let mut job: Option<crate::pty::job::JobObject> = {
            #[cfg(windows)]
            {
                child.id().and_then(crate::pty::job::JobObject::for_child)
            }
            #[cfg(not(windows))]
            {
                None
            }
        };

        // Drain the pipes CONCURRENTLY with the wait: each reader task finishes
        // on EOF and hands its buffer over a oneshot.
        let mut stdout_rx = child.stdout.take().map(|mut out| {
            let (tx, rx) = tokio::sync::oneshot::channel();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let _ = tokio::io::AsyncReadExt::read_to_end(&mut out, &mut buf).await;
                let _ = tx.send(buf);
            });
            rx
        });
        let mut stderr_rx = child.stderr.take().map(|mut err| {
            let (tx, rx) = tokio::sync::oneshot::channel();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let _ = tokio::io::AsyncReadExt::read_to_end(&mut err, &mut buf).await;
                let _ = tx.send(buf);
            });
            rx
        });

        async fn collect(
            stdout_rx: &mut Option<tokio::sync::oneshot::Receiver<Vec<u8>>>,
            stderr_rx: &mut Option<tokio::sync::oneshot::Receiver<Vec<u8>>>,
        ) -> (Vec<u8>, Vec<u8>) {
            let stdout = match stdout_rx {
                Some(rx) => rx.await.unwrap_or_default(),
                None => Vec::new(),
            };
            let stderr = match stderr_rx {
                Some(rx) => rx.await.unwrap_or_default(),
                None => Vec::new(),
            };
            (stdout, stderr)
        }

        #[cfg(unix)]
        let pid = child.id().unwrap_or(0);
        let waited = tokio::time::timeout(step_timeout, child.wait()).await;
        let (step_ok, step_error): (bool, Option<String>) = match waited {
            Ok(Ok(status)) => {
                // The parent exited. Join the readers bounded; a descendant
                // (npm -> node) can hold the pipe open past the parent exit.
                let joined =
                    tokio::time::timeout(READER_JOIN_TIMEOUT, collect(&mut stdout_rx, &mut stderr_rx))
                        .await;
                let (stdout, stderr) = match joined {
                    Ok((out, err)) => (out, err),
                    Err(_) => {
                        log::warn!(
                            "[agent-update] output truncated: descendant holds the pipe"
                        );
                        // Kill the tree BEFORE joining again so the readers EOF:
                        // Windows KILL_ON_JOB_CLOSE reaps the lingering tree
                        // member, Unix the process-group kill. The detached
                        // readers' buffers are unreachable, so "take what
                        // arrived" is not an option; a second bounded join
                        // recovers the FULL tail once the tree is dead.
                        if let Some(job) = job.take() {
                            // Windows: dropping the handle closes the job, and
                            // KILL_ON_JOB_CLOSE reaps the lingering tree member.
                            // Off Windows `JobObject::for_child` always returns
                            // `None` (`pty::job::stub_impl`), so this arm is
                            // unreachable and the stub has no `Drop` to call.
                            #[cfg(windows)]
                            drop(job);
                            #[cfg(not(windows))]
                            let _ = job;
                        }
                        #[cfg(unix)]
                        // SAFETY: `-pid` is the process group created by
                        // `process_group(0)` at spawn; it contains only this
                        // update step's own tree.
                        unsafe {
                            libc::kill(-(pid as i32), libc::SIGKILL);
                        }
                        match tokio::time::timeout(
                            READER_JOIN_TIMEOUT,
                            collect(&mut stdout_rx, &mut stderr_rx),
                        )
                        .await
                        {
                            Ok((out, err)) => (out, err),
                            Err(_) => {
                                log::warn!(
                                    "[agent-update] output still truncated after tree kill"
                                );
                                (Vec::new(), Vec::new())
                            }
                        }
                    }
                };
                if status.success() {
                    log::info!(
                        "[agent-update] step ok for {} ({}):\n{}",
                        target.label,
                        target.command,
                        output_tail(&stdout)
                    );
                    (true, None)
                } else {
                    let reason = format!("exit code {}", status.code().unwrap_or(-1));
                    let stderr_tail = output_tail(&stderr);
                    log::warn!(
                        "[agent-update] step FAILED for {} ({}): {}\n{}{}",
                        target.label,
                        target.command,
                        reason,
                        output_tail(&stdout),
                        if stderr_tail.is_empty() {
                            String::new()
                        } else {
                            format!("\nstderr:\n{stderr_tail}")
                        }
                    );
                    (false, Some(reason))
                }
            }
            Ok(Err(e)) => {
                log::warn!(
                    "[agent-update] step spawn/wait error for {} ({}): {e}",
                    target.label,
                    target.command
                );
                (false, Some(e.to_string()))
            }
            Err(_) => {
                // Step timeout: kill the whole tree, bounded, best-effort.
                if let Some(job) = &job {
                    job.terminate();
                }
                let _ = child.kill().await;
                #[cfg(unix)]
                // SAFETY: `-pid` is the process group created by
                // `process_group(0)` at spawn; it contains only this update
                // step's own tree.
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGKILL);
                }
                let _ =
                    tokio::time::timeout(READER_JOIN_TIMEOUT, child.wait()).await;
                let (stdout, stderr) =
                    tokio::time::timeout(READER_JOIN_TIMEOUT, collect(&mut stdout_rx, &mut stderr_rx))
                        .await
                        .unwrap_or_default();
                let reason = format!("timed out after {}s (killed)", step_timeout.as_secs());
                log::warn!(
                    "[agent-update] step TIMED OUT for {} ({}): {reason}\n{}",
                    target.label,
                    target.command,
                    output_tail(&stdout)
                );
                if !stderr.is_empty() {
                    log::warn!(
                        "[agent-update] stderr tail for {} ({}):\n{}",
                        target.label,
                        target.command,
                        output_tail(&stderr)
                    );
                }
                (false, Some(reason))
            }
        };

        if !step_ok {
            first_error = step_error;
            break; // fail-fast: later steps of this command do not run
        }
    }

    AgentUpdateResult {
        command: target.command.clone(),
        label: target.label.clone(),
        ok: first_error.is_none(),
        error: first_error,
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

/// #1551 - the two effects of the pass end, named fields so `FinishGuard::drop` cannot
/// pass them in the wrong slots.
struct PassEnd<I: FnOnce(), E: FnOnce()> {
    invalidate: I,
    emit: E,
}

/// #1551 - the ONLY transition to `finished`. Order is load-bearing (happens-before):
/// (1) invalidate the install cache while the gate is still un-finished, so an overview
///     that later observes `is_finished() == true` (gate lock acquired after `mark_finished`
///     released it) necessarily observes generation G+1 on its cache lock take (the cache
///     lock was released by `invalidate_all` before the gate lock was taken by `mark_finished`);
/// (2) mark the gate finished (results replaced, `running`/`pending` cleared, waiters released);
/// (3) announce. A client refreshing on the event therefore sees `finished` AND G+1, and no
///     ticket opened after `finished` became observable can carry the pre-pass generation.
fn finish_pass<I: FnOnce(), E: FnOnce()>(
    gate: &AgentUpdateGate,
    results: Vec<AgentUpdateResult>,
    end: PassEnd<I, E>,
) {
    (end.invalidate)();
    gate.mark_finished(results);
    (end.emit)();
}

/// Releases the gate on EVERY exit path (including panics), and emits the
/// finished event exactly once per started run. Created as the FIRST statement
/// of the startup task body so even a settings/catalog panic unblocks sessions.
struct FinishGuard {
    gate: Arc<AgentUpdateGate>,
    app: AppHandle,
    /// True once `agent_updates_started` was emitted; only then is the finished
    /// event (and the frontend splash teardown) owed.
    emit_finished: bool,
    /// Filled by the explicit completion path; a panic leaves it None -> empty
    /// results (no phantom failures).
    results: Option<Vec<AgentUpdateResult>>,
}

impl FinishGuard {
    fn complete(&mut self, results: Vec<AgentUpdateResult>) {
        self.results = Some(results);
    }
}

impl Drop for FinishGuard {
    fn drop(&mut self) {
        let results = self.results.take().unwrap_or_default();
        let cache = self.app.try_state::<Arc<AgentInstallCache>>();
        let app = self.app.clone();
        let emit_finished = self.emit_finished;
        let announced = results.clone();
        // #1551 - invalidate, THEN finish, THEN announce (plan 5.4 step 4). The
        // invalidation runs on EVERY guard drop (a quiet boot's cache is empty,
        // so the bump is free and the transition has one shape); the emit stays
        // conditional on `emit_finished`, exactly as before. Emitting is
        // synchronous in tauri v2; no await is legal or needed in Drop.
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
                    if emit_finished {
                        emit_all(
                            &app,
                            "agent_updates_finished",
                            json!({ "results": announced }),
                        );
                    }
                },
            },
        );
    }
}

/// #1551 - one update task: probes the pre-update install state (round 5), marks the gate,
/// emits the per-command events around the unchanged `run_update_sequence`, and returns its
/// result for `join_all`.
async fn run_update_target(
    app: AppHandle,
    gate: Arc<AgentUpdateGate>,
    target: UpdateTarget,
) -> AgentUpdateResult {
    // #1551 round 5 - read the installed version BEFORE the update, directly (no cache ticket,
    // no generation): the node stays `Pendiente` meanwhile, and this target's update starts
    // only after its own probe ended, so a probe never overlaps an update of the same CLI.
    let install_before = probe_command_install_state(&target.command).await;
    let node = gate.mark_command_started(&target.command, &target.label, Some(install_before));
    emit_all(&app, "agent_update_command_started", json!(node));
    let result = run_update_sequence(&target, UPDATE_STEP_TIMEOUT).await;
    gate.mark_command_finished(result.clone());
    emit_all(&app, "agent_update_command_finished", json!(result));
    result
}

/// #1551 - a task that panicked never ran `mark_command_finished`: settle it here so the
/// row leaves `running` and every surface receives its `command_finished`.
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
                error: Some("update task panicked".to_string()),
            };
            gate.mark_command_finished(result.clone());
            emit_all(app, "agent_update_command_finished", json!(result));
            result
        }
    }
}

/// #1551 - the agents of this boot's pass in catalog order: every target of `plan.updates`
/// (decided `true`) and `plan.prompts` (to be asked), one node per command (first catalog
/// occurrence, like `build_update_plan`). Pure; the order is the timeline order on every surface.
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

/// Detached startup task (spawned in `lib.rs` setup before `submit_restore_first`).
/// Plans from the SAME catalog source `get_coding_agent_catalog` serves, prompts
/// sequentially (60s each, default No), runs updates in parallel across commands
/// (300s per step), then releases the gate. Never returns an error: every failure
/// path is a red-toast result, not a startup failure.
pub async fn run_startup_updates(app: AppHandle, gate: Arc<AgentUpdateGate>) {
    // 1. Guard FIRST: a panic anywhere still releases the gate.
    let mut guard = FinishGuard {
        gate: Arc::clone(&gate),
        app: app.clone(),
        emit_finished: false,
        results: None,
    };

    let settings = app.state::<SettingsState>().read().await.clone();
    let catalog = load_catalog_for_settings(&settings);
    let default_cwd = primary_project_root(&settings)
        .or_else(crate::config::config_dir)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    // 2. Pure plan. An empty plan releases the gate instantly (guard Drop) and
    // emits nothing: no splash flash on a quiet boot.
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

    // #1551 - the pass's node set is built BEFORE `plan.updates` is moved below and
    // is installed in the gate before the event, so a client that reacts to the
    // event with a snapshot already sees the nodes.
    let nodes = pass_nodes(&catalog, &plan);
    gate.mark_started_with_nodes(nodes.clone());
    emit_all(&app, "agent_updates_started", json!({ "nodes": nodes }));
    guard.emit_finished = true;

    // #1341 - the prompt phase was log-silent, which hid the startup freeze
    // (the prompt expired unseen at PROMPT_TIMEOUT while the setup thread was
    // blocked inside the restore block_on). Info-level visibility from here.
    if !plan.prompts.is_empty() {
        log::info!(
            "[agent-update] prompt phase started: {} command(s) await SI/NO ({}s each, default No): [{}]",
            plan.prompts.len(),
            PROMPT_TIMEOUT.as_secs(),
            plan.prompts
                .iter()
                .map(|p| p.command.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    // 3. Prompt phase: SEQUENTIAL, catalog order, one modal at a time.
    let mut updates: Vec<UpdateTarget> = plan.updates;
    for pending in &plan.prompts {
        let rx = gate.register_prompt(&pending.command, &pending.label);
        emit_all(
            &app,
            "agent_update_prompt",
            json!(AgentUpdatePrompt {
                command: pending.command.clone(),
                label: pending.label.clone(),
            }),
        );
        log::info!(
            "[agent-update] prompting for '{}' ({}) - awaiting SI/NO ({}s, default No)",
            pending.command,
            pending.label,
            PROMPT_TIMEOUT.as_secs()
        );
        match tokio::time::timeout(PROMPT_TIMEOUT, rx).await {
            Ok(Ok(true)) => updates.push(pending.clone()), // answer command already persisted true
            // Persisted false; never asked again. The answer that released the
            // loop already emitted the closure; the loop, the single owner of
            // this boot's decision, emits the skip.
            Ok(Ok(false)) => skip_prompted_target(&app, &gate, &pending.command),
            Ok(Err(_)) | Err(_) => {
                // Timeout / channel dropped: nothing persisted (asked again next
                // boot), nothing runs this boot. was_prompted stays true so a
                // late answer still persists and returns Ok(false).
                //
                // #1551 - the expiry competes with answer claims for the single
                // pending entry inside the SAME serial section as `answer_prompt`;
                // it emits the closure only when it removed the entry, with no
                // await between the removal and the emit.
                {
                    let _serial = gate.answer_serial.lock().await;
                    if let Some(closed) = gate.drop_pending(&pending.command) {
                        emit_all(&app, "agent_update_prompt_closed", json!(closed));
                    }
                }
                skip_prompted_target(&app, &gate, &pending.command);
            }
        }
    }

    // 4. Update phase: PARALLEL across commands (one task per command keeps a
    // panicking command from aborting the others), each sequence strictly
    // ordered inside `run_update_sequence`.
    let handles: Vec<_> = updates
        .iter()
        .map(|t| {
            let target = t.clone();
            let app = app.clone();
            let gate = Arc::clone(&gate);
            tokio::spawn(async move { run_update_target(app, gate, target).await })
        })
        .collect();
    let joined = futures::future::join_all(handles).await;
    let results: Vec<AgentUpdateResult> = joined
        .into_iter()
        .zip(updates.iter())
        .map(|(r, t)| settle_joined_update(&app, &gate, t, r))
        .collect();

    // 5. Exactly one mark, exactly one emit; the explicit drop runs `finish_pass`
    // (invalidate -> finished -> announce) BEFORE the post-pass probes are
    // scheduled, so every ticket they open carries the post-pass generation.
    guard.complete(results);
    drop(guard);
    schedule_post_update_probes(&app, &updates);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::coding_agents_catalog::CodingAgentDefinition;

    fn entry(command: &str, label: &str, update_commands: Vec<&str>) -> CodingAgentDefinition {
        CodingAgentDefinition {
            key: command.to_string(),
            label: label.to_string(),
            description: String::new(),
            color: String::new(),
            command: command.to_string(),
            instructions_filename: None,
            envs: Vec::new(),
            isolated_home: false,
            config_seed: None,
            removable: true,
            update_commands: update_commands.into_iter().map(str::to_string).collect(),
            auto_update: false,
        }
    }

    fn cwd() -> PathBuf {
        PathBuf::from("/tmp/proj")
    }

    #[test]
    fn build_plan_dedupes_by_command_first_entry_wins() {
        let catalog = vec![
            entry("claude", "Claude (primary)", vec!["claude --update"]),
            entry("claude", "Claude (secondary)", vec!["claude --update --beta"]),
            entry("codex", "Codex", vec!["codex update"]),
        ];
        let answers = BTreeMap::new();
        let registered = HashSet::from(["claude".to_string(), "codex".to_string()]);
        let plan = build_update_plan(&catalog, &registered, &answers, cwd());
        assert_eq!(plan.prompts.len(), 2);
        assert!(plan.updates.is_empty());
        let claude = plan
            .prompts
            .iter()
            .find(|t| t.command == "claude")
            .expect("claude prompt");
        assert_eq!(claude.label, "Claude (primary)");
        assert_eq!(claude.commands, vec!["claude --update"]);
    }

    #[test]
    fn build_plan_first_nonempty_sequence_wins() {
        let catalog = vec![
            entry("claude", "Claude A", vec![]),
            entry("claude", "Claude B", vec!["claude --update"]),
        ];
        let answers = BTreeMap::from([("claude".to_string(), true)]);
        let registered = HashSet::from(["claude".to_string()]);
        let plan = build_update_plan(&catalog, &registered, &answers, cwd());
        assert!(plan.prompts.is_empty());
        assert_eq!(plan.updates.len(), 1);
        let claude = &plan.updates[0];
        assert_eq!(claude.label, "Claude A");
        assert_eq!(claude.commands, vec!["claude --update"]);
    }

    #[test]
    fn build_plan_skips_commands_without_sequence() {
        let catalog = vec![
            entry("claude", "Claude", vec!["claude --update"]),
            entry("hermes", "Hermes", vec![]),
        ];
        let answers = BTreeMap::new();
        let registered = HashSet::from(["claude".to_string(), "hermes".to_string()]);
        let plan = build_update_plan(&catalog, &registered, &answers, cwd());
        assert_eq!(plan.prompts.len(), 1);
        assert_eq!(plan.prompts[0].command, "claude");
        assert!(!plan.prompts.iter().any(|t| t.command == "hermes"));
        assert!(plan.updates.is_empty());
    }

    #[test]
    fn build_plan_answer_kinds_and_disjointness() {
        let catalog = vec![
            entry("claude", "Claude", vec!["claude --update"]),
            entry("codex", "Codex", vec!["codex update"]),
            entry("pi", "Pi", vec!["pi update"]),
        ];
        let answers = BTreeMap::from([("claude".to_string(), true)]);
        let registered =
            HashSet::from(["claude".to_string(), "codex".to_string(), "pi".to_string()]);
        let plan = build_update_plan(&catalog, &registered, &answers, cwd());
        assert_eq!(plan.prompts.len(), 2);
        assert_eq!(plan.updates.len(), 1);
        assert_eq!(plan.updates[0].command, "claude");
        // catalog order preserved for prompts
        let prompt_commands: Vec<&str> = plan.prompts.iter().map(|t| t.command.as_str()).collect();
        assert_eq!(prompt_commands, vec!["codex", "pi"]);
        // disjoint by construction
        for p in &plan.prompts {
            assert!(!plan.updates.iter().any(|u| u.command == p.command));
        }
    }

    #[test]
    fn build_plan_filters_to_registered_commands() {
        let catalog = vec![
            entry("claude", "Claude", vec!["claude --update"]),
            entry("codex", "Codex", vec!["codex update"]),
            entry("pi", "Pi", vec!["pi update"]),
        ];
        let registered = HashSet::from(["claude".to_string(), "pi".to_string()]);
        let answers = BTreeMap::new();
        let plan = build_update_plan(&catalog, &registered, &answers, cwd());
        assert_eq!(plan.prompts.len(), 2);
        assert!(plan.updates.is_empty());
        // catalog order preserved for the registered subset
        let prompt_commands: Vec<&str> = plan.prompts.iter().map(|t| t.command.as_str()).collect();
        assert_eq!(prompt_commands, vec!["claude", "pi"]);
        // catalog-only command skipped: never prompted, never updated
        assert!(!plan.prompts.iter().any(|t| t.command == "codex"));
        assert!(!plan.updates.iter().any(|t| t.command == "codex"));
    }

    #[test]
    fn build_plan_zero_registered_commands_returns_empty_plan() {
        let catalog = vec![
            entry("claude", "Claude", vec!["claude --update"]),
            entry("codex", "Codex", vec!["codex update"]),
        ];
        let registered = HashSet::new();
        let answers = BTreeMap::new();
        let plan = build_update_plan(&catalog, &registered, &answers, cwd());
        assert!(plan.prompts.is_empty());
        assert!(plan.updates.is_empty());
    }

    #[test]
    fn build_plan_registered_filter_preserves_first_entry_semantics() {
        let catalog = vec![
            entry("claude", "Claude (primary)", vec![]),
            entry("claude", "Claude (secondary)", vec!["claude --update"]),
            entry("codex", "Codex", vec!["codex update"]),
        ];
        let registered = HashSet::from(["claude".to_string()]);
        let answers = BTreeMap::from([("claude".to_string(), true)]);
        let plan = build_update_plan(&catalog, &registered, &answers, cwd());
        assert!(plan.prompts.is_empty());
        assert_eq!(plan.updates.len(), 1);
        let claude = &plan.updates[0];
        assert_eq!(claude.label, "Claude (primary)"); // first-entry label wins
        assert_eq!(claude.commands, vec!["claude --update"]); // first non-empty sequence wins
    }

    #[tokio::test]
    async fn gate_wait_returns_after_finish_and_releases_all_waiters() {
        let gate = Arc::new(AgentUpdateGate::new());
        let gate2 = Arc::clone(&gate);
        let waiter = tokio::spawn(async move { gate2.wait_until_done().await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        gate.mark_finished(vec![]);
        tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .expect("waiter released")
            .expect("waiter ok");

        // late waiter returns immediately
        let gate3 = Arc::clone(&gate);
        let late = tokio::spawn(async move { gate3.wait_until_done().await });
        tokio::time::timeout(Duration::from_secs(2), late)
            .await
            .expect("late waiter released")
            .expect("late waiter ok");
    }

    #[tokio::test]
    async fn gate_prompt_registry_and_snapshot() {
        let gate = AgentUpdateGate::new();
        gate.mark_started();
        let rx = gate.register_prompt("claude", "Claude");
        assert!(gate.was_prompted("claude"));
        let snap = gate.snapshot();
        assert!(snap.in_progress);
        assert_eq!(snap.prompt.as_ref().map(|p| p.command.as_str()), Some("claude"));

        assert!(gate.resolve_answer("claude", true));
        assert!(!gate.resolve_answer("claude", true), "second resolve must fail");
        assert!(rx.await.expect("answer delivered"));
        assert!(gate.snapshot().prompt.is_none(), "resolved prompt cleared");

        // dropped-receiver path (round-3 F1): the prompt timeout drops the
        // answer channel; a late resolve must return false without panicking
        let gate2 = AgentUpdateGate::new();
        let rx_dropped = gate2.register_prompt("codex", "Codex");
        drop(rx_dropped);
        assert!(!gate2.resolve_answer("codex", true));
        assert!(
            gate2.snapshot().prompt.is_none(),
            "a dead prompt must not be resurrected by the snapshot"
        );

        // drop_pending path: no answer, snapshot must not resurrect the prompt
        let rx2 = gate.register_prompt("codex", "Codex");
        gate.drop_pending("codex");
        assert!(!gate.resolve_answer("codex", false));
        assert!(rx2.await.is_err(), "sender dropped");
        assert!(gate.snapshot().prompt.is_none());

        // mark_finished clears pending prompt and pending senders
        let rx3 = gate.register_prompt("pi", "Pi");
        gate.mark_finished(vec![AgentUpdateResult {
            command: "pi".to_string(),
            label: "Pi".to_string(),
            ok: true,
            error: None,
        }]);
        assert!(rx3.await.is_err(), "sender dropped by mark_finished");
        let snap = gate.snapshot();
        assert!(!snap.in_progress);
        assert!(snap.prompt.is_none());
        assert_eq!(snap.results.len(), 1);
        assert!(snap.results[0].ok);

        // idempotent: repeated mark_finished keeps finished and replaces results
        gate.mark_finished(vec![]);
        assert!(!gate.snapshot().in_progress);
        assert!(gate.snapshot().results.is_empty());
    }

    /// Platform-shell-aware runner tests. Every command string below is a full
    /// shell string valid under BOTH cmd.exe /C and sh -c.
    async fn run_in_tmpdir(commands: Vec<&str>, step_timeout: Duration) -> AgentUpdateResult {
        let dir = tempfile::tempdir().expect("tempdir");
        run_update_sequence(
            &UpdateTarget {
                command: "test".to_string(),
                label: "Test".to_string(),
                commands: commands.into_iter().map(str::to_string).collect(),
                cwd: dir.path().to_path_buf(),
            },
            step_timeout,
        )
        .await
    }

    #[tokio::test]
    async fn runner_exit_zero_ok() {
        let result = run_in_tmpdir(vec!["exit 0"], UPDATE_STEP_TIMEOUT).await;
        assert!(result.ok, "unexpected: {result:?}");
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn runner_exit_nonzero_fails_with_code() {
        let result = run_in_tmpdir(vec!["exit 3"], UPDATE_STEP_TIMEOUT).await;
        assert!(!result.ok);
        assert_eq!(result.error.as_deref(), Some("exit code 3"));
    }

    #[tokio::test]
    async fn runner_fail_fast_stops_sequence() {
        let dir = tempfile::tempdir().expect("tempdir");
        let marker = dir.path().join("marker.txt");
        let result = run_update_sequence(
            &UpdateTarget {
                command: "test".to_string(),
                label: "Test".to_string(),
                commands: vec![
                    "exit 3".to_string(),
                    format!(
                        "echo x > {}",
                        marker.to_string_lossy().replace('\\', "/")
                    ),
                ],
                cwd: dir.path().to_path_buf(),
            },
            UPDATE_STEP_TIMEOUT,
        )
        .await;
        assert!(!result.ok);
        assert!(!marker.exists(), "later step must not run after a failure");
    }

    #[tokio::test]
    async fn runner_timeout_kills_tree() {
        // Windows: ping ~30s (cmd has no sleep); Unix: sleep 30. The step
        // timeout (200ms) must kill the whole tree; the test itself is bounded.
        let step = if cfg!(windows) {
            "ping -n 30 127.0.0.1 >NUL"
        } else {
            "sleep 30"
        };
        let started = std::time::Instant::now();
        let result = run_in_tmpdir(vec![step], Duration::from_millis(200)).await;
        assert!(!result.ok);
        assert!(
            result
                .error
                .as_deref()
                .is_some_and(|e| e.contains("timed out")),
            "unexpected: {result:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "tree kill must be prompt, took {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn runner_success_with_descendant_holding_pipe() {
        // The parent exits 0 quickly while a descendant keeps the pipes open:
        // the reader-join timeout arm runs (warn + tree-kill FIRST + second
        // bounded join) and the step is reported SUCCESS with the full tail.
        let step = if cfg!(windows) {
            "cmd /C start /B ping -n 30 127.0.0.1 >NUL"
        } else {
            "sh -c 'sleep 30 &'"
        };
        let started = std::time::Instant::now();
        let result = run_in_tmpdir(vec![step], UPDATE_STEP_TIMEOUT).await;
        assert!(result.ok, "unexpected: {result:?}");
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "descendant must be reaped promptly, took {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn runner_success_with_detached_survivor() {
        // round-3 F2 pin (deliberate platform divergence): a fully detached
        // (non-pipe-holding) descendant of the update step is reaped on
        // Windows by the per-step JobObject drop (KILL_ON_JOB_CLOSE: no
        // survivors from an update step on Windows) but SURVIVES on Unix (no
        // job; the group-kill runs only on timeout/truncation). The step
        // itself reports success on both platforms, via the plain-success
        // path, NOT the R1 reader-truncation arm: the survivor's handles are
        // redirected away from the step's pipes, so the readers EOF when the
        // parent exits. The survivor writes its marker only after a delay that
        // exceeds the step.
        let dir = tempfile::tempdir().expect("tempdir");
        let marker = dir.path().join("marker.txt");
        let marker_str = marker.to_string_lossy().replace('\\', "/");
        let step = if cfg!(windows) {
            // The plan's pinned `cmd /C start /B "" cmd /C "<cmd>"` shape was
            // VERIFIED to hang layer-2 cmd at `start` when spawned through the
            // runner's `cmd.exe /C` chain (start mangles the nested quotes
            // into a recursive `start "" "..."` command, observed in the
            // process tree), so the implementer pinned a quoting-free
            // equivalent with the SAME semantics (plan 9.1: implementer
            // verifies both arms and pins the exact quoting): `start` an exe
            // target - powershell, whose -EncodedCommand carries the whole
            // script with no quotes/spaces - with the handles redirected to
            // NUL on the start line. PowerShell exits right after
            // Start-Process returns (no wait); the detached survivor (the
            // Start-Process child, in the step's job, NOT inheriting the
            // step's pipes) pings ~2s and only then writes the marker - after
            // the step end, so the per-step job drop must reap it before the
            // write.
            use base64::Engine as _;
            let script = format!(
                "Start-Process cmd -ArgumentList '/c','ping -n 3 127.0.0.1 >NUL & echo x > {marker_str}' -WindowStyle Hidden"
            );
            let mut utf16 = Vec::new();
            for u in script.encode_utf16() {
                utf16.extend_from_slice(&u.to_le_bytes());
            }
            let encoded = base64::engine::general_purpose::STANDARD.encode(&utf16);
            format!("start /B powershell -NoProfile -EncodedCommand {encoded} >NUL 2>NUL")
        } else {
            format!("sh -c '(sleep 1; echo x > {marker_str}) >/dev/null 2>&1 &'")
        };
        let started = std::time::Instant::now();
        let result = run_update_sequence(
            &UpdateTarget {
                command: "test".to_string(),
                label: "Test".to_string(),
                commands: vec![step],
                cwd: dir.path().to_path_buf(),
            },
            UPDATE_STEP_TIMEOUT,
        )
        .await;
        assert!(result.ok, "unexpected: {result:?}");
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "completion bounded, took {:?}",
            started.elapsed()
        );
        if cfg!(windows) {
            // The per-step job drop reaped the survivor before its delayed
            // marker write.
            tokio::time::sleep(Duration::from_secs(3)).await;
            assert!(!marker.exists(), "Windows survivor must have been reaped");
        } else {
            // No job on Unix: the detached descendant survives the step and
            // writes the marker within ~3s.
            for _ in 0..30 {
                if marker.exists() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            assert!(marker.exists(), "Unix survivor must write the marker");
        }
    }

    #[tokio::test]
    async fn pending_prompt_release_requires_only_concurrent_runtime_work() {
        // #1341: pins the cooperative gate-release invariant the spawned
        // restore task relies on (the answerer task is the `agent_update_answer`
        // command; the waiter is the restore wake; both must progress on the
        // same runtime without the main thread doing anything). It does NOT
        // reproduce the #1341 freeze itself (the gate was cooperative pre-fix;
        // the bug was the lib.rs setup `block_on`, unreachable from a unit
        // test) - the freeze regression net is AC2 + AC8 + AC9 + AC11. If the
        // gate wait were ever made thread-blocking, this test would hang on the
        // single-threaded tokio test runtime and the timeout would fail.
        let gate = Arc::new(AgentUpdateGate::new());
        gate.mark_started();
        let _rx = gate.register_prompt("claude", "Claude");
        let waiter = tokio::spawn({
            let gate = Arc::clone(&gate);
            async move { gate.wait_until_done().await }
        });
        let answerer = tokio::spawn({
            let gate = Arc::clone(&gate);
            async move {
                tokio::time::sleep(Duration::from_millis(25)).await;
                assert!(gate.resolve_answer("claude", false), "prompt must still be pending");
                gate.mark_finished(vec![]);
            }
        });
        tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .expect("gate waiter must be released by concurrent runtime work")
            .expect("waiter ok");
        answerer.await.expect("answerer ok");
    }

    // ---------------------------------------------------------------------
    // #1551 - shared helpers for the new groups below.
    // ---------------------------------------------------------------------

    use crate::agent_version::{CacheLookup, InstallStatus};
    use crate::config::settings::{AgentConfig, AppSettings};
    use crate::web::broadcast::WsOutMsg;
    use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
    use std::task::{Context, Wake, Waker};

    const POLL_STEP: Duration = Duration::from_millis(50);
    const POLL_CAP: Duration = Duration::from_secs(5);
    const QUIET_WINDOW: Duration = Duration::from_millis(300);

    fn command_ref(command: &str, label: &str) -> AgentUpdateCommandRef {
        AgentUpdateCommandRef {
            command: command.to_string(),
            label: label.to_string(),
        }
    }

    fn node(command: &str, label: &str, update_commands: Vec<&str>) -> AgentUpdateNode {
        AgentUpdateNode {
            command: command.to_string(),
            label: label.to_string(),
            update_commands: update_commands.into_iter().map(str::to_string).collect(),
            install_before: None,
        }
    }

    fn ok_result(command: &str, label: &str) -> AgentUpdateResult {
        AgentUpdateResult {
            command: command.to_string(),
            label: label.to_string(),
            ok: true,
            error: None,
        }
    }

    fn build_mock_app(builder: tauri::Builder<tauri::Wry>) -> tauri::App {
        builder
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build test app")
    }

    fn app_with_broadcaster() -> (tauri::App, tokio::sync::mpsc::Receiver<WsOutMsg>) {
        let broadcaster = WsBroadcaster::new();
        let rx = broadcaster.subscribe();
        let app = build_mock_app(crate::test_support::test_builder().manage(broadcaster));
        (app, rx)
    }

    fn app_with_cache() -> (
        tauri::App,
        Arc<AgentInstallCache>,
        tokio::sync::mpsc::Receiver<WsOutMsg>,
    ) {
        let broadcaster = WsBroadcaster::new();
        let rx = broadcaster.subscribe();
        let cache = Arc::new(AgentInstallCache::new());
        let app = build_mock_app(
            crate::test_support::test_builder()
                .manage(broadcaster)
                .manage(Arc::clone(&cache)),
        );
        (app, cache, rx)
    }

    fn decode_frame(frame: WsOutMsg) -> Value {
        match frame {
            WsOutMsg::Text(text) => serde_json::from_str(&text).expect("json frame"),
            WsOutMsg::Binary(_) => panic!("unexpected binary frame"),
        }
    }

    fn drain_frames(rx: &mut tokio::sync::mpsc::Receiver<WsOutMsg>) -> Vec<Value> {
        let mut frames = Vec::new();
        while let Ok(frame) = rx.try_recv() {
            frames.push(decode_frame(frame));
        }
        frames
    }

    /// Bounded real-time poll: `tauri::async_runtime::spawn` runs on Tauri's own
    /// runtime, so paused-time helpers cannot drive it.
    async fn next_frame(rx: &mut tokio::sync::mpsc::Receiver<WsOutMsg>) -> Value {
        let started = Instant::now();
        loop {
            if let Ok(frame) = rx.try_recv() {
                return decode_frame(frame);
            }
            assert!(
                started.elapsed() < POLL_CAP,
                "no frame arrived within the bound"
            );
            tokio::time::sleep(POLL_STEP).await;
        }
    }

    async fn assert_no_frame(rx: &mut tokio::sync::mpsc::Receiver<WsOutMsg>) {
        tokio::time::sleep(QUIET_WINDOW).await;
        let leftover = drain_frames(rx);
        assert!(leftover.is_empty(), "unexpected frames: {leftover:?}");
    }

    /// The exact statement the prompt loop's expiry arm runs (plan 5.4 step 3).
    async fn run_expiry_arm(
        app: &AppHandle,
        gate: &AgentUpdateGate,
        command: &str,
    ) -> Option<AgentUpdateCommandRef> {
        let _serial = gate.answer_serial.lock().await;
        let closed = gate.drop_pending(command);
        if let Some(closed) = closed.as_ref() {
            emit_all(app, "agent_update_prompt_closed", json!(closed));
        }
        closed
    }

    fn counting_probe(calls: Arc<AtomicUsize>, park: Option<Arc<tokio::sync::Notify>>) -> ProbeFn {
        Arc::new(move |_command| {
            let calls = Arc::clone(&calls);
            let park = park.clone();
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                if let Some(park) = park {
                    park.notified().await;
                }
                InstallState::missing("stub".to_string())
            })
        })
    }

    /// `bob` (a bare token that resolves nowhere, with an update sequence) and
    /// `nob` (no sequence), so no embedded vendor CLI is ever probed.
    fn bob_catalog_dir(root: &Path) {
        let catalog_dir = root.join(".ac").join("coding-agents");
        std::fs::create_dir_all(&catalog_dir).expect("catalog dir");
        let manifest = json!({
            "schemaVersion": 1,
            "agents": [
                {
                    "key": "bob",
                    "label": "Bob",
                    "description": "",
                    "color": "#000000",
                    "command": "bob-1551-missing",
                    "updateCommands": ["bob up"]
                },
                {
                    "key": "nob",
                    "label": "Nob",
                    "description": "",
                    "color": "#000000",
                    "command": "nob-1551-missing",
                    "updateCommands": []
                }
            ]
        });
        std::fs::write(
            catalog_dir.join("agents.json"),
            serde_json::to_vec_pretty(&manifest).expect("manifest json"),
        )
        .expect("write catalog");
    }

    fn bob_settings(root: &Path) -> SettingsState {
        Arc::new(tokio::sync::RwLock::new(AppSettings {
            project_paths: vec![root.to_string_lossy().to_string()],
            ..AppSettings::default()
        }))
    }

    // ---------------------------------------------------------------------
    // #1551 - gate
    // ---------------------------------------------------------------------

    #[test]
    fn gate_running_tracks_commands_and_clears_on_finish() {
        let gate = AgentUpdateGate::new();
        gate.mark_started();
        gate.mark_command_started("claude", "Claude", None);
        gate.mark_command_started("claude", "Claude", None);
        assert_eq!(
            gate.snapshot().running,
            vec![command_ref("claude", "Claude")]
        );
        gate.mark_command_finished(ok_result("claude", "Claude"));
        let snapshot = gate.snapshot();
        assert!(snapshot.running.is_empty());
        assert_eq!(snapshot.results.len(), 1);
        gate.mark_command_started("codex", "Codex", None);
        gate.mark_finished(vec![]);
        let snapshot = gate.snapshot();
        assert!(snapshot.running.is_empty());
        assert!(snapshot.results.is_empty());
        assert!(!snapshot.in_progress);
    }

    #[test]
    fn gate_command_finished_upserts_by_command() {
        let gate = AgentUpdateGate::new();
        gate.mark_command_finished(ok_result("claude", "Claude"));
        gate.mark_command_finished(AgentUpdateResult {
            command: "claude".to_string(),
            label: "Claude".to_string(),
            ok: false,
            error: Some("exit code 1".to_string()),
        });
        let results = gate.snapshot().results;
        assert_eq!(results.len(), 1);
        assert!(!results[0].ok);
        assert_eq!(results[0].error.as_deref(), Some("exit code 1"));
    }

    #[test]
    fn gate_is_finished_follows_mark_finished() {
        let gate = AgentUpdateGate::new();
        assert!(!gate.is_finished());
        gate.mark_started();
        assert!(!gate.is_finished());
        gate.mark_finished(vec![]);
        assert!(gate.is_finished());
        gate.mark_started();
        assert!(gate.is_finished());
    }

    #[tokio::test]
    async fn gate_prompt_state_and_claim() {
        let gate = AgentUpdateGate::new();
        assert_eq!(gate.prompt_state("claude"), PromptState::NotPrompted);
        let mut rx = gate.register_prompt("claude", "Claude");
        assert_eq!(gate.prompt_state("claude"), PromptState::Pending);

        let AnswerClaim::Claimed(claimed) = gate.claim_answer("claude", true) else {
            panic!("a pending prompt must be claimed");
        };
        assert_eq!(claimed.closed(), &command_ref("claude", "Claude"));
        assert_eq!(gate.prompt_state("claude"), PromptState::Answered(true));
        let snapshot = gate.snapshot();
        assert!(snapshot.prompt.is_none());
        assert_eq!(
            snapshot.answered,
            BTreeMap::from([("claude".to_string(), true)])
        );
        assert!(
            matches!(
                rx.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ),
            "the claim must not release the loop"
        );
        assert!(claimed.deliver(true));
        assert!(rx.await.expect("the loop is released"));

        // `claim_answer` records unconditionally: only `answer_prompt` may call
        // it in production, after classifying under the serial lock.
        assert!(matches!(
            gate.claim_answer("claude", false),
            AnswerClaim::Recorded
        ));
        assert_eq!(gate.prompt_state("claude"), PromptState::Answered(false));

        let codex_rx = gate.register_prompt("codex", "Codex");
        drop(codex_rx);
        let AnswerClaim::Claimed(claimed) = gate.claim_answer("codex", true) else {
            panic!("the entry was present, so the claim must succeed");
        };
        assert!(!claimed.deliver(true), "the receiver is gone");
        assert_eq!(gate.snapshot().answered.get("codex"), Some(&true));

        let _pi_rx = gate.register_prompt("pi", "Pi");
        assert_eq!(gate.drop_pending("pi"), Some(command_ref("pi", "Pi")));
        assert_eq!(gate.drop_pending("pi"), None);
        assert_eq!(gate.prompt_state("pi"), PromptState::Expired);

        let _opencode_rx = gate.register_prompt("opencode", "OpenCode");
        gate.mark_finished(vec![]);
        assert_eq!(gate.prompt_state("opencode"), PromptState::Expired);
        assert_eq!(gate.drop_pending("opencode"), None);
        let answered = gate.snapshot().answered;
        assert_eq!(answered.get("claude"), Some(&false));
        assert_eq!(answered.get("codex"), Some(&true));
    }

    #[test]
    fn gate_mark_started_with_nodes_replaces_nodes_and_snapshot_carries_them() {
        let gate = AgentUpdateGate::new();
        let nodes = vec![
            node("claude", "Claude", vec!["claude --update"]),
            node("codex", "Codex", vec!["codex update"]),
        ];
        gate.mark_started_with_nodes(nodes.clone());
        let snapshot = gate.snapshot();
        assert_eq!(snapshot.nodes, nodes);
        assert!(snapshot.in_progress);
        gate.mark_finished(vec![]);
        assert_eq!(gate.snapshot().nodes, nodes, "the summary keeps the order");
        let replacement = vec![node("pi", "Pi", vec!["pi update"])];
        gate.mark_started_with_nodes(replacement.clone());
        assert_eq!(gate.snapshot().nodes, replacement);
    }

    #[test]
    fn gate_mark_command_skipped_prunes_once_and_returns_the_ref() {
        let gate = AgentUpdateGate::new();
        gate.mark_started_with_nodes(vec![
            node("claude", "Claude", vec!["claude --update"]),
            node("codex", "Codex", vec!["codex update"]),
        ]);
        gate.mark_command_started("claude", "Claude", None);
        gate.mark_command_finished(ok_result("claude", "Claude"));
        assert_eq!(
            gate.mark_command_skipped("codex"),
            Some(command_ref("codex", "Codex"))
        );
        let snapshot = gate.snapshot();
        assert_eq!(
            snapshot.nodes,
            vec![node("claude", "Claude", vec!["claude --update"])]
        );
        assert!(snapshot.running.is_empty());
        assert_eq!(snapshot.results.len(), 1);
        assert_eq!(gate.mark_command_skipped("codex"), None);
        assert_eq!(gate.snapshot().nodes.len(), 1);
    }

    #[test]
    fn gate_mark_command_started_records_install_before_on_its_node() {
        let gate = AgentUpdateGate::new();
        gate.mark_started_with_nodes(vec![node("claude", "Claude", vec!["claude --update"])]);
        let started = gate.mark_command_started(
            "claude",
            "Claude",
            Some(InstallState::missing("x".to_string())),
        );
        assert_eq!(
            started.install_before.as_ref().map(|state| state.status),
            Some(InstallStatus::Missing)
        );
        let snapshot = gate.snapshot();
        assert_eq!(
            snapshot.nodes[0]
                .install_before
                .as_ref()
                .map(|state| state.status),
            Some(InstallStatus::Missing)
        );
        let synthesized = gate.mark_command_started("nope", "Nope", None);
        assert!(synthesized.update_commands.is_empty());
        assert_eq!(synthesized.command, "nope");
        assert_eq!(gate.snapshot().nodes.len(), 1, "nothing was inserted");
    }

    #[test]
    fn pass_nodes_follow_catalog_order_across_updates_and_prompts() {
        let catalog = vec![
            entry("claude", "Claude", vec!["claude --update"]),
            entry("codex", "Codex", vec!["codex update"]),
            entry("pi", "Pi", vec!["pi update"]),
            entry("opencode", "OpenCode", vec!["opencode upgrade"]),
            CodingAgentDefinition {
                key: "pi-alt".to_string(),
                ..entry("pi", "Pi (alt)", vec!["pi update"])
            },
        ];
        let plan = AgentUpdatePlan {
            prompts: vec![UpdateTarget {
                command: "pi".to_string(),
                label: "Pi".to_string(),
                commands: vec!["pi update".to_string()],
                cwd: cwd(),
            }],
            updates: vec![
                UpdateTarget {
                    command: "opencode".to_string(),
                    label: "OpenCode".to_string(),
                    commands: vec!["opencode upgrade".to_string()],
                    cwd: cwd(),
                },
                UpdateTarget {
                    command: "claude".to_string(),
                    label: "Claude".to_string(),
                    commands: vec!["claude --update".to_string()],
                    cwd: cwd(),
                },
            ],
        };
        let nodes = pass_nodes(&catalog, &plan);
        assert_eq!(
            nodes,
            vec![
                node("claude", "Claude", vec!["claude --update"]),
                node("pi", "Pi", vec!["pi update"]),
                node("opencode", "OpenCode", vec!["opencode upgrade"]),
            ]
        );
    }

    // ---------------------------------------------------------------------
    // #1551 - the answer flow
    // ---------------------------------------------------------------------

    /// #1551 - waker of the prompt's `oneshot::Receiver` in
    /// `answer_prompt_enqueues_the_closure_before_releasing_the_loop`. tokio's
    /// `oneshot::Sender::send` runs it INLINE (`Inner::complete` -> `wake_by_ref`), i.e. inside
    /// `ClaimedPrompt::deliver`, before `answer_prompt` executes anything else, so it sees the
    /// WebSocket queue at the instant the loop is released. `seen`: 0 = never woken, 1 = woken
    /// while no closure frame was enqueued (the superseded send-before-emit order), 2 = woken
    /// with `agent_update_prompt_closed {claude}` already enqueued (the plan's order).
    struct ReleaseProbe {
        probe_rx: std::sync::Mutex<tokio::sync::mpsc::Receiver<WsOutMsg>>,
        seen: AtomicU8,
    }

    impl Wake for ReleaseProbe {
        fn wake(self: Arc<Self>) {
            self.wake_by_ref();
        }

        fn wake_by_ref(self: &Arc<Self>) {
            let frame = self.probe_rx.lock().unwrap().try_recv().ok();
            let closed = frame.is_some_and(|frame| match frame {
                WsOutMsg::Text(text) => {
                    serde_json::from_str::<Value>(&text)
                        .ok()
                        .is_some_and(|frame| {
                            frame["event"] == "agent_update_prompt_closed"
                                && frame["payload"]["command"] == "claude"
                        })
                }
                WsOutMsg::Binary(_) => false,
            });
            self.seen
                .store(if closed { 2 } else { 1 }, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn answer_prompt_first_answer_resolves_persists_and_closes_every_surface() {
        let (app, mut frames_rx) = app_with_broadcaster();
        let handle = app.handle().clone();
        let gate = AgentUpdateGate::new();
        let rx = gate.register_prompt("claude", "Claude");
        let calls = Arc::new(AtomicUsize::new(0));
        let persist = {
            let calls = Arc::clone(&calls);
            move || {
                calls.fetch_add(1, Ordering::SeqCst);
                async { Ok::<(), String>(()) }
            }
        };
        assert!(answer_prompt(&handle, &gate, "claude", true, persist)
            .await
            .expect("answer"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(rx.await.expect("the loop is released"));
        let frames = drain_frames(&mut frames_rx);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["event"], "agent_update_prompt_closed");
        assert_eq!(
            frames[0]["payload"],
            json!({ "command": "claude", "label": "Claude" })
        );
        assert_eq!(gate.prompt_state("claude"), PromptState::Answered(true));
        assert!(gate.snapshot().prompt.is_none());
    }

    #[tokio::test]
    async fn answer_prompt_enqueues_the_closure_before_releasing_the_loop() {
        let broadcaster = WsBroadcaster::new();
        let probe_rx = broadcaster.subscribe();
        let mut order_rx = broadcaster.subscribe();
        let app = build_mock_app(crate::test_support::test_builder().manage(broadcaster));
        let handle = app.handle().clone();
        let gate = AgentUpdateGate::new();
        let mut rx = gate.register_prompt("claude", "Claude");

        let probe = Arc::new(ReleaseProbe {
            probe_rx: std::sync::Mutex::new(probe_rx),
            seen: AtomicU8::new(0),
        });
        let waker = Waker::from(Arc::clone(&probe));
        let mut cx = Context::from_waker(&waker);
        assert!(
            Pin::new(&mut rx).poll(&mut cx).is_pending(),
            "the probe must become the receiver's waker"
        );

        assert!(answer_prompt(&handle, &gate, "claude", true, || async {
            Ok::<(), String>(())
        })
        .await
        .expect("answer"));
        assert_eq!(
            probe.seen.load(Ordering::SeqCst),
            2,
            "the closure must already be enqueued when the loop is released"
        );
        assert!(rx.await.expect("the loop is released"));

        // Mirror the loop's advance to the next prompt.
        let _codex_rx = gate.register_prompt("codex", "Codex");
        emit_all(
            &handle,
            "agent_update_prompt",
            json!(AgentUpdatePrompt {
                command: "codex".to_string(),
                label: "Codex".to_string(),
            }),
        );
        let frames = drain_frames(&mut order_rx);
        assert_eq!(frames.len(), 2, "unexpected frames: {frames:?}");
        assert_eq!(frames[0]["event"], "agent_update_prompt_closed");
        assert_eq!(
            frames[0]["payload"],
            json!({ "command": "claude", "label": "Claude" })
        );
        assert_eq!(frames[1]["event"], "agent_update_prompt");
        assert_eq!(
            frames[1]["payload"],
            json!({ "command": "codex", "label": "Codex" })
        );
    }

    #[tokio::test]
    async fn answer_prompt_superseded_answer_returns_false_without_persisting() {
        let (app, mut frames_rx) = app_with_broadcaster();
        let handle = app.handle().clone();
        let gate = AgentUpdateGate::new();
        let rx = gate.register_prompt("claude", "Claude");
        assert!(answer_prompt(&handle, &gate, "claude", true, || async {
            Ok::<(), String>(())
        })
        .await
        .expect("first answer"));
        assert!(rx.await.expect("the loop is released"));
        assert_eq!(drain_frames(&mut frames_rx).len(), 1);

        let calls = Arc::new(AtomicUsize::new(0));
        let persist = {
            let calls = Arc::clone(&calls);
            move || {
                calls.fetch_add(1, Ordering::SeqCst);
                async { Ok::<(), String>(()) }
            }
        };
        assert!(!answer_prompt(&handle, &gate, "claude", false, persist)
            .await
            .expect("second answer"));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(gate.snapshot().answered.get("claude"), Some(&true));
        assert_no_frame(&mut frames_rx).await;
    }

    #[tokio::test]
    async fn answer_prompt_two_simultaneous_answers_are_serialized_first_wins() {
        let (app, mut frames_rx) = app_with_broadcaster();
        let handle = app.handle().clone();
        let gate = Arc::new(AgentUpdateGate::new());
        let rx = gate.register_prompt("claude", "Claude");
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());

        let task_a = tokio::spawn({
            let gate = Arc::clone(&gate);
            let handle = handle.clone();
            let started = Arc::clone(&started);
            let release = Arc::clone(&release);
            async move {
                answer_prompt(&handle, &gate, "claude", true, move || async move {
                    started.notify_one();
                    release.notified().await;
                    Ok::<(), String>(())
                })
                .await
            }
        });
        started.notified().await;

        let calls_b = Arc::new(AtomicUsize::new(0));
        let mut task_b = tokio::spawn({
            let gate = Arc::clone(&gate);
            let handle = handle.clone();
            let calls = Arc::clone(&calls_b);
            async move {
                answer_prompt(&handle, &gate, "claude", false, move || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    async { Ok::<(), String>(()) }
                })
                .await
            }
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut task_b)
                .await
                .is_err(),
            "B must be queued behind the serial lock"
        );

        release.notify_one();
        assert!(task_a.await.expect("join a").expect("answer a"));
        assert!(!task_b.await.expect("join b").expect("answer b"));
        assert_eq!(calls_b.load(Ordering::SeqCst), 0);
        assert!(rx.await.expect("the loop is released"));
        let frames = drain_frames(&mut frames_rx);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["event"], "agent_update_prompt_closed");
        assert_eq!(
            gate.snapshot().answered,
            BTreeMap::from([("claude".to_string(), true)])
        );
    }

    #[tokio::test]
    async fn answer_prompt_failed_persist_keeps_the_prompt_pending_and_the_next_answer_wins() {
        let (app, mut frames_rx) = app_with_broadcaster();
        let handle = app.handle().clone();
        let gate = AgentUpdateGate::new();
        let rx = gate.register_prompt("claude", "Claude");

        let failed = answer_prompt(&handle, &gate, "claude", true, || async {
            Err::<(), String>("disk full".to_string())
        })
        .await;
        assert_eq!(failed, Err("disk full".to_string()));
        assert_eq!(gate.prompt_state("claude"), PromptState::Pending);
        assert!(gate.snapshot().answered.is_empty());
        assert!(drain_frames(&mut frames_rx).is_empty());

        assert!(answer_prompt(&handle, &gate, "claude", false, || async {
            Ok::<(), String>(())
        })
        .await
        .expect("second answer"));
        assert!(!rx.await.expect("the loop is released"));
        assert_eq!(drain_frames(&mut frames_rx).len(), 1);
        assert_eq!(
            gate.snapshot().answered,
            BTreeMap::from([("claude".to_string(), false)])
        );
    }

    #[tokio::test]
    async fn answer_prompt_failed_persist_while_queued_lets_the_waiter_win() {
        let (app, _frames_rx) = app_with_broadcaster();
        let handle = app.handle().clone();
        let gate = Arc::new(AgentUpdateGate::new());
        let _rx = gate.register_prompt("claude", "Claude");
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());

        let task_a = tokio::spawn({
            let gate = Arc::clone(&gate);
            let handle = handle.clone();
            let started = Arc::clone(&started);
            let release = Arc::clone(&release);
            async move {
                answer_prompt(&handle, &gate, "claude", true, move || async move {
                    started.notify_one();
                    release.notified().await;
                    Err::<(), String>("disk full".to_string())
                })
                .await
            }
        });
        started.notified().await;
        let task_b = tokio::spawn({
            let gate = Arc::clone(&gate);
            let handle = handle.clone();
            async move {
                answer_prompt(&handle, &gate, "claude", false, || async {
                    Ok::<(), String>(())
                })
                .await
            }
        });
        release.notify_one();
        assert_eq!(
            task_a.await.expect("join a"),
            Err("disk full".to_string()),
            "a failed persist changes nothing"
        );
        assert!(task_b.await.expect("join b").expect("answer b"));
        assert_eq!(gate.snapshot().answered.get("claude"), Some(&false));
    }

    #[tokio::test]
    async fn answer_prompt_late_answer_after_timeout_persists_and_returns_false() {
        let (app, mut frames_rx) = app_with_broadcaster();
        let handle = app.handle().clone();
        let gate = AgentUpdateGate::new();
        let rx = gate.register_prompt("claude", "Claude");
        drop(rx);
        assert!(run_expiry_arm(&handle, &gate, "claude").await.is_some());
        assert_eq!(drain_frames(&mut frames_rx).len(), 1);

        let calls = Arc::new(AtomicUsize::new(0));
        let persist = {
            let calls = Arc::clone(&calls);
            move || {
                calls.fetch_add(1, Ordering::SeqCst);
                async { Ok::<(), String>(()) }
            }
        };
        assert!(!answer_prompt(&handle, &gate, "claude", true, persist)
            .await
            .expect("late answer"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            gate.snapshot().answered,
            BTreeMap::from([("claude".to_string(), true)])
        );
        assert_no_frame(&mut frames_rx).await;
    }

    #[tokio::test]
    async fn answer_prompt_and_expiry_race_emit_exactly_one_closure_with_either_winner() {
        // (i) the answer claims first: the expiry then finds nothing.
        {
            let (app, mut frames_rx) = app_with_broadcaster();
            let handle = app.handle().clone();
            let gate = AgentUpdateGate::new();
            let rx = gate.register_prompt("claude", "Claude");
            assert!(answer_prompt(&handle, &gate, "claude", true, || async {
                Ok::<(), String>(())
            })
            .await
            .expect("answer"));
            assert!(rx.await.expect("the loop is released"));
            assert!(run_expiry_arm(&handle, &gate, "claude").await.is_none());
            assert_eq!(drain_frames(&mut frames_rx).len(), 1);
        }

        // (ii) the expiry wins: the late answer persists and emits nothing.
        {
            let (app, mut frames_rx) = app_with_broadcaster();
            let handle = app.handle().clone();
            let gate = AgentUpdateGate::new();
            let rx = gate.register_prompt("claude", "Claude");
            drop(rx);
            assert!(run_expiry_arm(&handle, &gate, "claude").await.is_some());
            let calls = Arc::new(AtomicUsize::new(0));
            let persist = {
                let calls = Arc::clone(&calls);
                move || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    async { Ok::<(), String>(()) }
                }
            };
            assert!(!answer_prompt(&handle, &gate, "claude", true, persist)
                .await
                .expect("late answer"));
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert_eq!(drain_frames(&mut frames_rx).len(), 1);
        }

        // (iii) the expiry is queued behind a claiming answer.
        {
            let (app, mut frames_rx) = app_with_broadcaster();
            let handle = app.handle().clone();
            let gate = Arc::new(AgentUpdateGate::new());
            let rx = gate.register_prompt("claude", "Claude");
            drop(rx); // the timeout elapsed
            let started = Arc::new(tokio::sync::Notify::new());
            let release = Arc::new(tokio::sync::Notify::new());
            let calls = Arc::new(AtomicUsize::new(0));

            let answer = tokio::spawn({
                let gate = Arc::clone(&gate);
                let handle = handle.clone();
                let started = Arc::clone(&started);
                let release = Arc::clone(&release);
                let calls = Arc::clone(&calls);
                async move {
                    answer_prompt(&handle, &gate, "claude", true, move || async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        started.notify_one();
                        release.notified().await;
                        Ok::<(), String>(())
                    })
                    .await
                }
            });
            started.notified().await;
            let mut expiry = tokio::spawn({
                let gate = Arc::clone(&gate);
                let handle = handle.clone();
                async move { run_expiry_arm(&handle, &gate, "claude").await }
            });
            assert!(
                tokio::time::timeout(Duration::from_millis(100), &mut expiry)
                    .await
                    .is_err(),
                "the expiry arm must block on the serial lock"
            );
            release.notify_one();
            assert!(
                !answer.await.expect("join answer").expect("answer"),
                "the receiver is gone, so the answer is late"
            );
            assert!(
                expiry.await.expect("join expiry").is_none(),
                "the expiry found no entry and emitted nothing"
            );
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert_eq!(drain_frames(&mut frames_rx).len(), 1);
            assert_eq!(
                gate.snapshot().answered,
                BTreeMap::from([("claude".to_string(), true)])
            );
        }
    }

    #[tokio::test]
    async fn answer_prompt_rejects_unprompted_command_without_persisting() {
        let (app, mut frames_rx) = app_with_broadcaster();
        let handle = app.handle().clone();
        let gate = AgentUpdateGate::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let persist = {
            let calls = Arc::clone(&calls);
            move || {
                calls.fetch_add(1, Ordering::SeqCst);
                async { Ok::<(), String>(()) }
            }
        };
        let error = answer_prompt(&handle, &gate, "claude", true, persist)
            .await
            .expect_err("an unprompted command must be rejected");
        assert!(error.contains("was not prompted"), "unexpected: {error}");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_no_frame(&mut frames_rx).await;
    }

    // ---------------------------------------------------------------------
    // #1551 - events and the emitter
    // ---------------------------------------------------------------------

    #[tokio::test]
    async fn run_update_target_emits_started_then_finished_and_updates_gate() {
        let (app, mut frames_rx) = app_with_broadcaster();
        let handle = app.handle().clone();
        let gate = Arc::new(AgentUpdateGate::new());
        let dir = tempfile::tempdir().expect("tempdir");
        let target = UpdateTarget {
            command: "x-1551-missing".to_string(),
            label: "X".to_string(),
            commands: vec!["exit 0".to_string()],
            cwd: dir.path().to_path_buf(),
        };
        gate.mark_started_with_nodes(vec![AgentUpdateNode {
            command: target.command.clone(),
            label: target.label.clone(),
            update_commands: target.commands.clone(),
            install_before: None,
        }]);

        let result = run_update_target(handle.clone(), Arc::clone(&gate), target).await;
        assert!(result.ok, "unexpected: {result:?}");

        let frames = drain_frames(&mut frames_rx);
        assert_eq!(frames.len(), 2, "unexpected frames: {frames:?}");
        assert_eq!(frames[0]["event"], "agent_update_command_started");
        assert_eq!(frames[0]["payload"]["command"], "x-1551-missing");
        assert_eq!(frames[0]["payload"]["updateCommands"], json!(["exit 0"]));
        assert_eq!(frames[0]["payload"]["installBefore"]["status"], "missing");
        assert_eq!(frames[1]["event"], "agent_update_command_finished");
        assert_eq!(frames[1]["payload"]["ok"], true);

        let snapshot = gate.snapshot();
        assert!(snapshot.running.is_empty());
        assert_eq!(snapshot.results.len(), 1);
        assert_eq!(
            snapshot.nodes[0]
                .install_before
                .as_ref()
                .map(|state| state.status),
            Some(InstallStatus::Missing)
        );
    }

    #[tokio::test]
    async fn skip_prompted_target_emits_once_and_prunes_the_node() {
        let (app, mut frames_rx) = app_with_broadcaster();
        let handle = app.handle().clone();
        let gate = AgentUpdateGate::new();
        gate.mark_started_with_nodes(vec![
            node("claude", "Claude", vec!["claude --update"]),
            node("codex", "Codex", vec!["codex update"]),
        ]);
        skip_prompted_target(&handle, &gate, "codex");
        let frames = drain_frames(&mut frames_rx);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["event"], "agent_update_command_skipped");
        assert_eq!(
            frames[0]["payload"],
            json!({ "command": "codex", "label": "Codex" })
        );
        assert_eq!(
            gate.snapshot().nodes,
            vec![node("claude", "Claude", vec!["claude --update"])]
        );

        skip_prompted_target(&handle, &gate, "codex");
        assert_no_frame(&mut frames_rx).await;
        assert_eq!(gate.snapshot().nodes.len(), 1);
    }

    #[tokio::test]
    async fn settle_joined_update_turns_a_panic_into_command_finished() {
        let (app, mut frames_rx) = app_with_broadcaster();
        let handle = app.handle().clone();
        let gate = AgentUpdateGate::new();
        gate.mark_command_started("x", "X", None);
        let target = UpdateTarget {
            command: "x".to_string(),
            label: "X".to_string(),
            commands: vec!["exit 0".to_string()],
            cwd: cwd(),
        };
        let joined: Result<AgentUpdateResult, tokio::task::JoinError> =
            tokio::spawn(async { panic!("boom") }).await;
        let settled = settle_joined_update(&handle, &gate, &target, joined);
        assert!(!settled.ok);
        assert_eq!(settled.error.as_deref(), Some("update task panicked"));
        let snapshot = gate.snapshot();
        assert!(snapshot.running.is_empty());
        assert_eq!(snapshot.results.len(), 1);
        assert_eq!(
            snapshot.results[0].error.as_deref(),
            Some("update task panicked")
        );
        let frames = drain_frames(&mut frames_rx);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["event"], "agent_update_command_finished");
        assert_eq!(frames[0]["payload"]["error"], "update task panicked");

        // The `Ok` branch returns the result unchanged and emits nothing.
        let passthrough = settle_joined_update(&handle, &gate, &target, Ok(ok_result("y", "Y")));
        assert!(passthrough.ok);
        assert_no_frame(&mut frames_rx).await;
    }

    #[tokio::test]
    async fn emit_all_reaches_websocket_subscribers_and_survives_no_broadcaster() {
        let (app, mut frames_rx) = app_with_broadcaster();
        emit_all(
            app.handle(),
            "agent_updates_started",
            json!({ "nodes": [] }),
        );
        let frames = drain_frames(&mut frames_rx);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["event"], "agent_updates_started");

        let bare = build_mock_app(crate::test_support::test_builder());
        emit_all(
            bare.handle(),
            "agent_updates_started",
            json!({ "nodes": [] }),
        );
    }

    #[test]
    fn agent_update_emits_only_through_emit_all() {
        let src = include_str!("agent_update.rs");
        let lines: Vec<&str> = src.lines().collect();
        let boundary = lines
            .iter()
            .enumerate()
            .find(|(index, line)| {
                **line == "mod tests {"
                    && lines[..*index]
                        .iter()
                        .rev()
                        .find(|previous| !previous.trim().is_empty())
                        == Some(&"#[cfg(test)]")
            })
            .map(|(index, _)| index)
            .expect("the tests module boundary");

        let production: Vec<&str> = lines[..boundary]
            .iter()
            .copied()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect();
        assert_eq!(
            production.join("\n").matches(".emit(").count(),
            1,
            "every agent-update emit must go through emit_all"
        );
        let emit_all_line = production
            .iter()
            .position(|line| line.starts_with("fn emit_all("))
            .expect("fn emit_all");
        let emit_line = production
            .iter()
            .position(|line| line.contains(".emit("))
            .expect("the single .emit( call site");
        let next_item = production[emit_all_line + 1..]
            .iter()
            .position(|line| {
                line.starts_with("fn ") || line.starts_with("struct ") || line.starts_with("impl ")
            })
            .map(|offset| offset + emit_all_line + 1)
            .unwrap_or(production.len());
        assert!(
            emit_all_line < emit_line && emit_line < next_item,
            "the only .emit( must live inside emit_all"
        );
    }

    // ---------------------------------------------------------------------
    // #1551 - rows and the probe policy
    // ---------------------------------------------------------------------

    #[test]
    fn overview_rows_only_update_capable_entries_in_catalog_order_no_dedup() {
        let catalog = vec![
            entry("claude", "Claude", vec!["claude --update"]),
            entry("codex", "Codex", vec!["codex update"]),
            entry("hermes", "Hermes", vec!["hermes update --yes"]),
            entry("agent", "Cursor", vec![]),
            entry("pi", "Pi", vec!["pi update"]),
            entry("opencode", "OpenCode", vec!["opencode upgrade"]),
            entry("agy", "Antigravity", vec!["agy update"]),
            CodingAgentDefinition {
                key: "pi-alt".to_string(),
                ..entry("pi", "Pi (alt)", vec!["pi update"])
            },
        ];
        let mut installed = InstallState::installed("1.0".to_string(), Path::new("/bin/pi"));
        installed.seq = 7;
        let install_by_command = HashMap::from([("pi".to_string(), installed.clone())]);
        let rows = build_update_overview_rows(&catalog, &install_by_command);
        assert_eq!(
            rows.iter().map(|row| row.key.as_str()).collect::<Vec<_>>(),
            vec!["claude", "codex", "hermes", "pi", "opencode", "agy", "pi-alt"]
        );
        assert!(
            !rows.iter().any(|row| row.command == "agent"),
            "cursor ships no update command"
        );
        for row in rows.iter().filter(|row| row.command == "pi") {
            assert_eq!(row.install, installed);
        }
        assert_eq!(rows[0].install, InstallState::checking());
        assert_eq!(rows[0].install.seq, 0);
    }

    #[tokio::test]
    async fn probe_command_install_state_unresolvable_is_missing() {
        let state = probe_command_install_state("definitely-missing-cli-1551").await;
        assert_eq!(state.status, InstallStatus::Missing);
        assert!(
            state
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("definitely-missing-cli-1551")),
            "unexpected: {state:?}"
        );
    }

    #[tokio::test]
    async fn probe_command_install_state_explicit_path_is_unprobed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("tool.exe");
        std::fs::write(&file, b"x").expect("write");
        let state = probe_command_install_state(&file.to_string_lossy()).await;
        assert_eq!(state.status, InstallStatus::Unprobed);
        assert!(state.path.is_some());
        assert_eq!(
            state.detail.as_deref(),
            Some("explicit path: version not probed")
        );
    }

    #[tokio::test]
    async fn probe_command_install_state_unknown_stem_is_unprobed() {
        let token = if cfg!(windows) { "cmd" } else { "sh" };
        let state = probe_command_install_state(token).await;
        assert_eq!(state.status, InstallStatus::Unprobed);
        assert!(state.path.is_some());
        assert!(
            state
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("no built-in version probe")),
            "unexpected: {state:?}"
        );
    }

    #[tokio::test]
    async fn probe_command_install_state_empty_command_is_missing() {
        let state = probe_command_install_state("").await;
        assert_eq!(state.status, InstallStatus::Missing);
        assert_eq!(state.detail.as_deref(), Some("empty command"));
    }

    // ---------------------------------------------------------------------
    // #1551 - probe scheduling
    // ---------------------------------------------------------------------

    #[tokio::test]
    async fn update_overview_first_call_returns_checking_then_cached_state() {
        let (app, cache, mut frames_rx) = app_with_cache();
        let handle = app.handle().clone();
        let dir = tempfile::tempdir().expect("tempdir");
        bob_catalog_dir(dir.path());
        let settings = bob_settings(dir.path());
        let gate = AgentUpdateGate::new();
        gate.mark_finished(vec![]);

        let rows =
            update_overview_with(&handle, &settings, &gate, &cache, production_probe()).await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].command, "bob-1551-missing");
        assert_eq!(rows[0].install.status, InstallStatus::Checking);
        assert_eq!(rows[0].install.seq, 0);

        let started = Instant::now();
        let committed = loop {
            let rows =
                update_overview_with(&handle, &settings, &gate, &cache, production_probe()).await;
            if rows[0].install.status == InstallStatus::Missing {
                break rows[0].install.clone();
            }
            assert!(
                started.elapsed() < POLL_CAP,
                "the probe never committed a state"
            );
            tokio::time::sleep(POLL_STEP).await;
        };
        assert_eq!(committed.seq, 1);

        let frame = next_frame(&mut frames_rx).await;
        assert_eq!(frame["event"], "agent_install_state_changed");
        assert_eq!(frame["payload"]["command"], "bob-1551-missing");
        assert_eq!(frame["payload"]["install"]["seq"], 1);
        assert_no_frame(&mut frames_rx).await;
        assert_eq!(cache.in_flight_len(), 0);
    }

    #[tokio::test]
    async fn update_overview_schedules_nothing_until_the_pass_is_finished() {
        let (app, cache, mut frames_rx) = app_with_cache();
        let handle = app.handle().clone();
        let dir = tempfile::tempdir().expect("tempdir");
        bob_catalog_dir(dir.path());
        let settings = bob_settings(dir.path());
        let gate = AgentUpdateGate::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let probe = counting_probe(Arc::clone(&calls), None);

        let rows =
            update_overview_with(&handle, &settings, &gate, &cache, Arc::clone(&probe)).await;
        assert_eq!(rows[0].install.status, InstallStatus::Checking);
        assert_no_frame(&mut frames_rx).await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(cache.in_flight_len(), 0);

        gate.mark_started();
        let rows =
            update_overview_with(&handle, &settings, &gate, &cache, Arc::clone(&probe)).await;
        assert_eq!(rows[0].install.status, InstallStatus::Checking);
        assert_no_frame(&mut frames_rx).await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(cache.in_flight_len(), 0);

        gate.mark_finished(vec![]);
        update_overview_with(&handle, &settings, &gate, &cache, Arc::clone(&probe)).await;
        let started = Instant::now();
        while calls.load(Ordering::SeqCst) == 0 {
            assert!(started.elapsed() < POLL_CAP, "the probe never ran");
            tokio::time::sleep(POLL_STEP).await;
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let frame = next_frame(&mut frames_rx).await;
        assert_eq!(frame["event"], "agent_install_state_changed");
    }

    #[tokio::test]
    async fn update_overview_serves_fresh_entries_without_probing_during_the_pass() {
        let (app, cache, mut frames_rx) = app_with_cache();
        let handle = app.handle().clone();
        let dir = tempfile::tempdir().expect("tempdir");
        bob_catalog_dir(dir.path());
        let settings = bob_settings(dir.path());
        let ticket = cache.try_begin("bob-1551-missing").expect("ticket");
        assert!(matches!(
            ticket.complete(InstallState::installed(
                "1.0".to_string(),
                Path::new("/bin/bob")
            )),
            Completion::Committed(_)
        ));
        let gate = AgentUpdateGate::new();
        gate.mark_started();
        let calls = Arc::new(AtomicUsize::new(0));

        let rows = update_overview_with(
            &handle,
            &settings,
            &gate,
            &cache,
            counting_probe(Arc::clone(&calls), None),
        )
        .await;
        assert_eq!(rows[0].install.status, InstallStatus::Installed);
        assert_eq!(rows[0].install.version.as_deref(), Some("1.0"));
        assert_eq!(rows[0].install.seq, 1);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_no_frame(&mut frames_rx).await;
        assert_eq!(cache.in_flight_len(), 0);
    }

    #[tokio::test]
    async fn update_overview_two_racing_calls_schedule_one_probe() {
        let (app, cache, mut frames_rx) = app_with_cache();
        let handle = app.handle().clone();
        let dir = tempfile::tempdir().expect("tempdir");
        bob_catalog_dir(dir.path());
        let settings = bob_settings(dir.path());
        let gate = AgentUpdateGate::new();
        gate.mark_finished(vec![]);
        let calls = Arc::new(AtomicUsize::new(0));
        let park = Arc::new(tokio::sync::Notify::new());
        let probe = counting_probe(Arc::clone(&calls), Some(Arc::clone(&park)));

        let (rows_a, rows_b) = tokio::join!(
            update_overview_with(&handle, &settings, &gate, &cache, Arc::clone(&probe)),
            update_overview_with(&handle, &settings, &gate, &cache, Arc::clone(&probe)),
        );
        assert_eq!(rows_a[0].install.status, InstallStatus::Checking);
        assert_eq!(rows_b[0].install.status, InstallStatus::Checking);

        let started = Instant::now();
        while calls.load(Ordering::SeqCst) == 0 {
            assert!(started.elapsed() < POLL_CAP, "the probe never ran");
            tokio::time::sleep(POLL_STEP).await;
        }
        tokio::time::sleep(QUIET_WINDOW).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1, "single-flight per command");
        assert!(matches!(
            cache.lookup("bob-1551-missing", Instant::now(), INSTALL_CACHE_TTL),
            CacheLookup::InFlight
        ));
        assert_eq!(cache.in_flight_len(), 1);

        park.notify_one();
        let started = Instant::now();
        loop {
            if let CacheLookup::Fresh(state) =
                cache.lookup("bob-1551-missing", Instant::now(), INSTALL_CACHE_TTL)
            {
                assert_eq!(state.seq, 1);
                break;
            }
            assert!(started.elapsed() < POLL_CAP, "the probe never committed");
            tokio::time::sleep(POLL_STEP).await;
        }
        let rows =
            update_overview_with(&handle, &settings, &gate, &cache, Arc::clone(&probe)).await;
        assert_eq!(rows[0].install.seq, 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let frame = next_frame(&mut frames_rx).await;
        assert_eq!(frame["event"], "agent_install_state_changed");
        assert_no_frame(&mut frames_rx).await;
    }

    #[tokio::test]
    async fn update_overview_serves_a_commit_that_lands_between_two_calls() {
        let (app, cache, _frames_rx) = app_with_cache();
        let handle = app.handle().clone();
        let dir = tempfile::tempdir().expect("tempdir");
        bob_catalog_dir(dir.path());
        let settings = bob_settings(dir.path());
        let gate = AgentUpdateGate::new();
        gate.mark_finished(vec![]);
        let calls = Arc::new(AtomicUsize::new(0));
        let park = Arc::new(tokio::sync::Notify::new());
        let probe = counting_probe(Arc::clone(&calls), Some(Arc::clone(&park)));

        let rows =
            update_overview_with(&handle, &settings, &gate, &cache, Arc::clone(&probe)).await;
        assert_eq!(rows[0].install.status, InstallStatus::Checking);
        let started = Instant::now();
        while calls.load(Ordering::SeqCst) == 0 {
            assert!(started.elapsed() < POLL_CAP, "the probe never ran");
            tokio::time::sleep(POLL_STEP).await;
        }

        park.notify_one();
        let started = Instant::now();
        while !matches!(
            cache.lookup("bob-1551-missing", Instant::now(), INSTALL_CACHE_TTL),
            CacheLookup::Fresh(_)
        ) {
            assert!(started.elapsed() < POLL_CAP, "the probe never committed");
            tokio::time::sleep(POLL_STEP).await;
        }

        let rows =
            update_overview_with(&handle, &settings, &gate, &cache, Arc::clone(&probe)).await;
        assert_eq!(rows[0].install.seq, 1);
        assert_eq!(rows[0].install.status, InstallStatus::Missing);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "no second ticket, no second process"
        );
        assert_eq!(cache.in_flight_len(), 0);
    }

    #[tokio::test]
    async fn overview_at_the_pass_boundary_never_opens_a_pre_pass_ticket() {
        let (app, cache, mut frames_rx) = app_with_cache();
        let handle = app.handle().clone();
        let dir = tempfile::tempdir().expect("tempdir");
        bob_catalog_dir(dir.path());
        let settings = bob_settings(dir.path());
        let calls = Arc::new(AtomicUsize::new(0));
        let probe = counting_probe(Arc::clone(&calls), None);

        let ticket = cache.try_begin("bob-1551-missing").expect("ticket");
        assert!(matches!(
            ticket.complete(InstallState::installed(
                "1.0".to_string(),
                Path::new("/bin/bob")
            )),
            Completion::Committed(_)
        ));
        let gate = AgentUpdateGate::new();
        gate.mark_started();

        // (a) gate read before mark_finished, cache read before invalidate_all.
        let rows =
            update_overview_with(&handle, &settings, &gate, &cache, Arc::clone(&probe)).await;
        assert_eq!(rows[0].install.status, InstallStatus::Installed);
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        cache.invalidate_all();

        // (b) gate read before mark_finished, cache read after invalidate_all.
        let rows =
            update_overview_with(&handle, &settings, &gate, &cache, Arc::clone(&probe)).await;
        assert_eq!(rows[0].install.status, InstallStatus::Checking);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(cache.in_flight_len(), 0, "Deferred opens no ticket");

        gate.mark_finished(vec![]);

        // (c) gate read after mark_finished: any ticket carries the new generation.
        let rows =
            update_overview_with(&handle, &settings, &gate, &cache, Arc::clone(&probe)).await;
        assert_eq!(rows[0].install.status, InstallStatus::Checking);
        let started = Instant::now();
        while calls.load(Ordering::SeqCst) == 0 {
            assert!(started.elapsed() < POLL_CAP, "the probe never ran");
            tokio::time::sleep(POLL_STEP).await;
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let frame = next_frame(&mut frames_rx).await;
        assert_eq!(frame["payload"]["install"]["seq"], 2);
        match cache.lookup("bob-1551-missing", Instant::now(), INSTALL_CACHE_TTL) {
            CacheLookup::Fresh(state) => assert_eq!(state.seq, 2),
            other => panic!("unexpected: {other:?}"),
        }
        assert_eq!(cache.generation(), 1);
    }

    #[tokio::test]
    async fn schedule_post_update_probes_opens_one_ticket_per_updated_command_after_finished() {
        let (app, cache, mut frames_rx) = app_with_cache();
        let handle = app.handle().clone();
        let gate = AgentUpdateGate::new();
        gate.mark_finished(vec![]);
        let target = UpdateTarget {
            command: "bob-1551-missing".to_string(),
            label: "Bob".to_string(),
            commands: vec!["bob up".to_string()],
            cwd: cwd(),
        };

        schedule_post_update_probes(&handle, std::slice::from_ref(&target));
        let frame = next_frame(&mut frames_rx).await;
        assert_eq!(frame["event"], "agent_install_state_changed");
        assert_eq!(frame["payload"]["command"], "bob-1551-missing");
        assert_eq!(frame["payload"]["install"]["seq"], 1);
        assert_eq!(cache.in_flight_len(), 0);

        // A second call is served from the cache: nothing is scheduled.
        schedule_post_update_probes(&handle, std::slice::from_ref(&target));
        assert_no_frame(&mut frames_rx).await;
        schedule_post_update_probes(&handle, &[]);
        assert_no_frame(&mut frames_rx).await;

        // Single-flight with a Settings-triggered probe that is still running.
        let (app, cache, mut frames_rx) = app_with_cache();
        let handle = app.handle().clone();
        let dir = tempfile::tempdir().expect("tempdir");
        bob_catalog_dir(dir.path());
        let settings = bob_settings(dir.path());
        let calls = Arc::new(AtomicUsize::new(0));
        let park = Arc::new(tokio::sync::Notify::new());
        update_overview_with(
            &handle,
            &settings,
            &gate,
            &cache,
            counting_probe(Arc::clone(&calls), Some(Arc::clone(&park))),
        )
        .await;
        let started = Instant::now();
        while calls.load(Ordering::SeqCst) == 0 {
            assert!(started.elapsed() < POLL_CAP, "the probe never ran");
            tokio::time::sleep(POLL_STEP).await;
        }
        schedule_post_update_probes(&handle, std::slice::from_ref(&target));
        tokio::time::sleep(QUIET_WINDOW).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(cache.in_flight_len(), 1, "the scheduler got InFlight");
        park.notify_one();
        let frame = next_frame(&mut frames_rx).await;
        assert_eq!(frame["event"], "agent_install_state_changed");
        drop(app);
    }

    // ---------------------------------------------------------------------
    // #1551 - the pass end
    // ---------------------------------------------------------------------

    #[test]
    fn finish_pass_invalidates_before_the_gate_is_finished_and_emits_after() {
        let cache = Arc::new(AgentInstallCache::new());
        let ticket = cache.try_begin("bob").expect("ticket");
        assert!(matches!(
            ticket.complete(InstallState::missing("seed".to_string())),
            Completion::Committed(_)
        ));
        let gate = AgentUpdateGate::new();
        let invalidate_saw_finished = std::cell::Cell::new(true);
        let emit_saw = std::cell::Cell::new((false, 0_u64, false));
        let results = vec![ok_result("claude", "Claude")];

        finish_pass(
            &gate,
            results,
            PassEnd {
                invalidate: || {
                    invalidate_saw_finished.set(gate.is_finished());
                    cache.invalidate_all();
                },
                emit: || {
                    emit_saw.set((
                        gate.is_finished(),
                        cache.generation(),
                        matches!(
                            cache.lookup("bob", Instant::now(), INSTALL_CACHE_TTL),
                            CacheLookup::Absent
                        ),
                    ));
                },
            },
        );

        assert!(
            !invalidate_saw_finished.get(),
            "the cache is invalidated while the gate is still un-finished"
        );
        assert_eq!(emit_saw.get(), (true, 1, true));
        assert_eq!(gate.snapshot().results.len(), 1);
    }

    #[tokio::test]
    async fn finish_guard_drop_invalidates_marks_finished_and_emits_only_on_a_real_pass() {
        let (app, cache, mut frames_rx) = app_with_cache();
        let handle = app.handle().clone();
        let ticket = cache.try_begin("bob").expect("ticket");
        assert!(matches!(
            ticket.complete(InstallState::missing("seed".to_string())),
            Completion::Committed(_)
        ));
        let gate = Arc::new(AgentUpdateGate::new());
        drop(FinishGuard {
            gate: Arc::clone(&gate),
            app: handle.clone(),
            emit_finished: true,
            results: Some(vec![ok_result("claude", "Claude")]),
        });
        let frames = drain_frames(&mut frames_rx);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["event"], "agent_updates_finished");
        assert_eq!(
            frames[0]["payload"]["results"]
                .as_array()
                .expect("results")
                .len(),
            1
        );
        assert!(gate.is_finished());
        assert_eq!(cache.generation(), 1);
        assert!(matches!(
            cache.lookup("bob", Instant::now(), INSTALL_CACHE_TTL),
            CacheLookup::Absent
        ));

        // A quiet boot: the invalidation still runs, the emit does not.
        let (quiet_app, quiet_cache, mut quiet_rx) = app_with_cache();
        let quiet_gate = Arc::new(AgentUpdateGate::new());
        drop(FinishGuard {
            gate: Arc::clone(&quiet_gate),
            app: quiet_app.handle().clone(),
            emit_finished: false,
            results: None,
        });
        assert_no_frame(&mut quiet_rx).await;
        assert!(quiet_gate.is_finished());
        assert_eq!(quiet_cache.generation(), 1);
    }

    // ---------------------------------------------------------------------
    // #1551 - end to end
    // ---------------------------------------------------------------------

    #[tokio::test]
    async fn run_startup_updates_emits_started_with_nodes_then_command_events_then_finished_then_post_probe(
    ) {
        let dir = tempfile::tempdir().expect("tempdir");
        bob_catalog_dir(dir.path());
        let settings = AppSettings {
            project_paths: vec![dir.path().to_string_lossy().to_string()],
            agents: vec![AgentConfig {
                id: "agent-0".to_string(),
                label: "Bob".to_string(),
                command: "bob-1551-missing".to_string(),
                color: "#000000".to_string(),
                envs: Vec::new(),
                isolated_home: false,
                instructions_filename: None,
                config_seed: None,
                context_regex: None,
                backend: Default::default(),
            }],
            agent_auto_update_by_command: BTreeMap::from([("bob-1551-missing".to_string(), true)]),
            ..AppSettings::default()
        };
        let settings_state: SettingsState = Arc::new(tokio::sync::RwLock::new(settings));
        let broadcaster = WsBroadcaster::new();
        let mut frames_rx = broadcaster.subscribe();
        let cache = Arc::new(AgentInstallCache::new());
        let app = build_mock_app(
            crate::test_support::test_builder()
                .manage(settings_state)
                .manage(broadcaster)
                .manage(Arc::clone(&cache)),
        );
        let handle = app.handle().clone();
        let gate = Arc::new(AgentUpdateGate::new());

        run_startup_updates(handle.clone(), Arc::clone(&gate)).await;

        let started = next_frame(&mut frames_rx).await;
        assert_eq!(started["event"], "agent_updates_started");
        assert_eq!(
            started["payload"]["nodes"],
            json!([{
                "command": "bob-1551-missing",
                "label": "Bob",
                "updateCommands": ["bob up"]
            }])
        );

        let command_started = next_frame(&mut frames_rx).await;
        assert_eq!(command_started["event"], "agent_update_command_started");
        assert_eq!(command_started["payload"]["command"], "bob-1551-missing");
        assert_eq!(
            command_started["payload"]["installBefore"]["status"],
            "missing"
        );
        assert_eq!(command_started["payload"]["installBefore"]["seq"], 0);

        let command_finished = next_frame(&mut frames_rx).await;
        assert_eq!(command_finished["event"], "agent_update_command_finished");
        assert_eq!(command_finished["payload"]["command"], "bob-1551-missing");
        assert_eq!(command_finished["payload"]["ok"], false);

        let finished = next_frame(&mut frames_rx).await;
        assert_eq!(finished["event"], "agent_updates_finished");
        let results = finished["payload"]["results"]
            .as_array()
            .expect("results")
            .clone();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["ok"], false);

        let install = next_frame(&mut frames_rx).await;
        assert_eq!(install["event"], "agent_install_state_changed");
        assert_eq!(install["payload"]["command"], "bob-1551-missing");
        assert_eq!(install["payload"]["install"]["status"], "missing");
        assert_eq!(install["payload"]["install"]["seq"], 1);
        assert_no_frame(&mut frames_rx).await;

        assert!(gate.is_finished());
        let snapshot = gate.snapshot();
        assert!(!snapshot.in_progress);
        assert!(snapshot.running.is_empty());
        assert_eq!(snapshot.results.len(), 1);
        assert_eq!(snapshot.nodes.len(), 1);
        assert_eq!(
            snapshot.nodes[0]
                .install_before
                .as_ref()
                .map(|state| state.status),
            Some(InstallStatus::Missing)
        );
        assert_eq!(cache.generation(), 1);
        assert_eq!(cache.in_flight_len(), 0);
        match cache.lookup("bob-1551-missing", Instant::now(), INSTALL_CACHE_TTL) {
            CacheLookup::Fresh(state) => assert_eq!(state.seq, 1),
            other => panic!("unexpected: {other:?}"),
        }
    }

    // ---------------------------------------------------------------------
    // #1551 - commit and announce
    // ---------------------------------------------------------------------

    #[tokio::test]
    async fn probe_commit_announce_emits_the_committed_state_once() {
        let (app, cache, mut frames_rx) = app_with_cache();
        let handle = app.handle().clone();
        let ticket = cache.try_begin("bob").expect("ticket");
        probe_commit_announce(&handle, "bob", ticket, |_command| async {
            InstallState::missing("gone".to_string())
        })
        .await;
        let frames = drain_frames(&mut frames_rx);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["event"], "agent_install_state_changed");
        assert_eq!(frames[0]["payload"]["install"]["seq"], 1);
        assert!(matches!(
            cache.lookup("bob", Instant::now(), INSTALL_CACHE_TTL),
            CacheLookup::Fresh(_)
        ));
    }

    #[tokio::test]
    async fn probe_commit_announce_reprobes_after_invalidation_and_emits_only_the_new_generation() {
        let (app, cache, mut frames_rx) = app_with_cache();
        let handle = app.handle().clone();
        let ticket = cache.try_begin("bob").expect("ticket");
        cache.invalidate_all();
        let calls = Arc::new(AtomicUsize::new(0));
        let probe = {
            let calls = Arc::clone(&calls);
            move |_command: String| {
                calls.fetch_add(1, Ordering::SeqCst);
                async { InstallState::missing("gone".to_string()) }
            }
        };
        probe_commit_announce(&handle, "bob", ticket, probe).await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let frames = drain_frames(&mut frames_rx);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["payload"]["install"]["seq"], 1);
        assert!(matches!(
            cache.lookup("bob", Instant::now(), INSTALL_CACHE_TTL),
            CacheLookup::Fresh(_)
        ));
        assert_eq!(cache.generation(), 1);
    }

    #[tokio::test]
    async fn probe_commit_announce_emits_nothing_when_the_retry_is_rejected() {
        let (app, cache, mut frames_rx) = app_with_cache();
        let handle = app.handle().clone();
        let ticket = cache.try_begin("bob").expect("ticket");
        let calls = Arc::new(AtomicUsize::new(0));
        let probe = {
            let calls = Arc::clone(&calls);
            let cache = Arc::clone(&cache);
            move |_command: String| {
                calls.fetch_add(1, Ordering::SeqCst);
                cache.invalidate_all();
                async { InstallState::missing("gone".to_string()) }
            }
        };
        probe_commit_announce(&handle, "bob", ticket, probe).await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_no_frame(&mut frames_rx).await;
        assert!(matches!(
            cache.lookup("bob", Instant::now(), INSTALL_CACHE_TTL),
            CacheLookup::Absent
        ));
        assert_eq!(cache.in_flight_len(), 0);
    }
}
