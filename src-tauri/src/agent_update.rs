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

mod cancellation;
mod gate;
mod install;
mod pass;
mod plan;
mod target;
mod updater;

use crate::agent_version::InstallState;
#[cfg(test)]
use crate::agent_version::InstallStatus;
#[cfg(test)]
use crate::agent_version::{AgentInstallCache, Completion, INSTALL_CACHE_TTL};
#[cfg(test)]
use crate::config::agent_path::effective_search_path;
use crate::config::agent_path::warm_login_shell_path;
#[cfg(test)]
use crate::config::coding_agents_catalog::load_catalog_for_settings;
#[cfg(test)]
use crate::config::settings::SettingsState;
use crate::process::agent::update::OUTPUT_TAIL_BYTES;
use crate::web::broadcast::WsBroadcaster;
use crate::web::event_broadcast::broadcast_all;
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
#[cfg(test)]
use std::collections::{HashMap, HashSet};
use std::future::Future;
#[cfg(test)]
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::pin::Pin;
#[cfg(test)]
use std::process::Stdio;
#[cfg(test)]
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;
#[cfg(test)]
use std::time::Instant;
use tauri::{AppHandle, Emitter, Manager};

pub use cancellation::{cancel_all_updates, cancel_update};
pub use gate::{AgentUpdateGate, AnswerClaim, ClaimedPrompt, PromptState};
pub(crate) use install::start_agent_install;
pub use install::AgentInstallState;
#[cfg(test)]
use install::{
    agent_install_log_lines, install_finished_payload, run_agent_install,
    start_agent_install_with_timeout, AgentInstallRun,
};
#[cfg(test)]
pub(crate) use install::{contains_install_command, InstallInFlight};
pub use pass::pass_nodes;
use pass::PassSupervisor;
#[cfg(test)]
use pass::{
    finish_pass, run_target_probe, run_update_target, schedule_post_update_probes,
    settle_joined_update, skip_prompted_target, PassEnd, PassRuntime, SupervisedTargetTask,
    TargetTerminalizer,
};
pub use plan::{
    build_update_overview_rows, build_update_plan, probe_command_install_state, production_probe,
    update_overview, update_overview_with, AgentUpdateOverviewRow, AgentUpdatePlan, ProbeFn,
    UpdateTarget,
};
#[cfg(test)]
use plan::{probe_commit_announce, TargetProbeOutcome};
#[cfg(test)]
use target::{
    PostProbeSettledTestHook, ResponseReleaseTestHook, StepAdmission, StepAdmissionTestHook,
    TargetPhase, TargetWork, TerminalPublish,
};
#[cfg(test)]
use updater::{
    build_update_step_command, cancelled_update_result, failed_update_result, output_tail,
    run_update_sequence, run_update_sequence_cancellable, successful_update_result,
    with_target_settlement_hook, TargetNativeObservation, TargetOwnerSlot, TargetProcessOwner,
    TargetReaderSettlementHook, TargetSettlementHook, TargetWait, UpdaterStepOutcome,
};

/// Per-prompt cap. On expiry nothing is persisted (the question is asked again
/// next boot) and the command is skipped this boot.
const PROMPT_TIMEOUT: Duration = Duration::from_secs(60);
/// Per-update-command-step cap. On expiry the whole tree is killed and the step
/// is reported failed; startup continues.
const UPDATE_STEP_TIMEOUT: Duration = Duration::from_secs(300);
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
    /// #1551 - policy recorded by the winning answer per prompted command this boot (resolving or
    /// late).
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
                "[agent-update] answer for '{command}' (enabled={enabled}): ignored, already \
                answered this boot"
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
                "[agent-update] answer for '{command}' (enabled={enabled}): recorded for future \
                boots (prompt already closed)"
            );
            Ok(false)
        }
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
