use super::target::{
    AsyncFence, StepAdmission, TargetClaim, TargetControl, TargetPhase, TargetWork, TerminalPublish,
};
use super::{
    AgentUpdateCommandRef, AgentUpdateNode, AgentUpdatePrompt, AgentUpdateResult, AgentUpdateStatus,
};
use crate::agent_version::InstallState;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};

/// Process-local gate: blocks every session open until the startup update run
/// finishes or times out. Managed as `Arc<AgentUpdateGate>`.
pub struct AgentUpdateGate {
    pub(crate) state: Mutex<GateState>,
    pub(crate) release: tokio::sync::Notify,
    /// #1551 - serializes `answer_prompt` (classify -> persist -> settle) per gate.
    /// A tokio mutex because it is held across the persist await; the std `state`
    /// lock is never held across an await.
    pub(crate) answer_serial: tokio::sync::Mutex<()>,
    #[cfg(test)]
    pub(crate) wait_registered: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

/// #1551 - a registered-but-unanswered prompt: the answer channel plus the label
/// the closure event has to carry.
pub(crate) struct PendingPrompt {
    pub(crate) tx: tokio::sync::oneshot::Sender<bool>,
    pub(crate) label: String,
}

pub(crate) struct GateState {
    pub(crate) started: bool,
    pub(crate) finished: bool,
    pub(crate) results: Vec<AgentUpdateResult>,
    /// Commands prompted this boot (answer validity + late-answer persistence).
    pub(crate) prompted: HashSet<String>,
    /// Registered-but-unanswered prompts, keyed by command.
    pub(crate) pending: HashMap<String, PendingPrompt>,
    /// Currently registered-but-unanswered prompt, for the snapshot.
    pub(crate) pending_prompt: Option<AgentUpdatePrompt>,
    /// #1551 - commands whose update sequence is running, in start order.
    pub(crate) running: Vec<AgentUpdateCommandRef>,
    /// #1551 - the policy the winning answer recorded, per prompted command.
    pub(crate) answered: BTreeMap<String, bool>,
    /// #1551 - this boot's pass, in pass order (catalog order).
    pub(crate) nodes: Vec<AgentUpdateNode>,
    pub(crate) controls: HashMap<String, TargetControl>,
    pub(crate) cancel_all_requested: bool,
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
    pub(crate) closed: AgentUpdateCommandRef,
    pub(crate) tx: tokio::sync::oneshot::Sender<bool>,
    pub(crate) skipped: Option<AgentUpdateCommandRef>,
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
    pub(crate) fn take_pending(state: &mut GateState, command: &str) -> Option<PendingPrompt> {
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
    pub(crate) fn store_final_results(&self, results: Vec<AgentUpdateResult>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.pending.clear();
        state.pending_prompt = None;
        state.running.clear();
        state.results = results;
    }

    /// Atomically open terminal fences and make finished visible, then notify
    /// admission waiters. This is intentionally separate from result storage so
    /// the pass-finished event is enqueued before any waiter can return.
    pub(crate) fn release_finished(&self) {
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

    pub(crate) fn begin_step(
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

    pub(crate) fn commit_step(&self, command: &str, work: TargetWork, epoch: u64) -> StepAdmission {
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
    pub(crate) async fn hold_after_step_begin_for_test(&self, command: &str) {
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
    pub(crate) async fn hold_after_step_commit_for_test(&self, command: &str) {
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
    pub(crate) async fn hold_response_release_for_test(&self, command: &str) {
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
    pub(crate) async fn hold_after_post_probe_settled_for_test(&self, command: &str) {
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

    pub(crate) fn owner_installed(&self, command: &str, work: TargetWork, epoch: u64) {
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

    pub(crate) fn owner_observed_cancellation(&self, command: &str, work: TargetWork, epoch: u64) {
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
    pub(crate) fn completed_without_owner(
        &self,
        command: &str,
        work: TargetWork,
        epoch: u64,
    ) -> bool {
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

    pub(crate) fn owner_returned(&self, command: &str, work: TargetWork, epoch: u64) -> bool {
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

    pub(crate) fn cancellation_reserved(&self, command: &str) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .controls
            .get(command)
            .is_some_and(|control| control.claim == TargetClaim::CancellationReserved)
    }

    pub(crate) fn acknowledge_unowned_cancellation(&self, command: &str) {
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

    pub(crate) async fn wait_response_release_if_cancelled(&self, command: &str) {
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

    pub(crate) fn publish_terminal(
        &self,
        result: AgentUpdateResult,
        allow_reserved: bool,
    ) -> TerminalPublish {
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

    pub(crate) fn command_event_emitted(&self, command: &str) {
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

    pub(crate) async fn wait_all_terminal_publications(&self) {
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

    pub(crate) fn supervisor_failure_reserved(&self, command: &str) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .controls
            .get(command)
            .is_some_and(|control| control.supervisor_failure)
    }

    pub(crate) fn reserve_supervisor_failure(&self) -> Option<AgentUpdateCommandRef> {
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

    pub(crate) fn unfinished_nodes(&self) -> Vec<AgentUpdateNode> {
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
