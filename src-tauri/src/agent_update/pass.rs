use super::gate::AgentUpdateGate;
use super::plan::{
    build_update_plan, probe_command_install_state_cancellable, production_probe, spawn_probe_task,
    AgentUpdatePlan, TargetProbeOutcome, UpdateTarget,
};
use super::target::{StepAdmission, TargetWork, TerminalPublish};
use super::updater::{
    cancelled_update_result, failed_update_result, run_update_sequence_cancellable,
    successful_update_result, TargetOwnerSlot, UpdaterStepOutcome,
};
use super::{
    emit_all, AgentUpdateCommandRef, AgentUpdateNode, AgentUpdatePrompt, AgentUpdateResult,
    CANCELLATION_CLEANUP_ERROR, PROMPT_TIMEOUT, UPDATE_STEP_TIMEOUT,
};
#[cfg(test)]
use super::{AgentUpdateChange, AgentUpdateOutcome};
use super::{AgentUpdateState, AgentUpdateTasks, ManagedBroadcastEvents};
use crate::agent_version::{InstallState, Scheduling, INSTALL_CACHE_TTL};
use crate::config::coding_agents_catalog::{
    load_catalog_for_settings, primary_project_root, CodingAgentDefinition,
};
use futures::FutureExt;
use serde_json::json;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

pub(crate) struct SettledPostProbe(pub(crate) InstallState);

pub(crate) struct TargetTerminalizer<E> {
    pub(crate) app: E,
    pub(crate) gate: Arc<AgentUpdateGate>,
    pub(crate) target: UpdateTarget,
    pub(crate) install_before: Option<InstallState>,
    pub(crate) settled_post_probe: Option<SettledPostProbe>,
}

impl<E: ManagedBroadcastEvents> TargetTerminalizer<E> {
    pub(crate) fn new(app: E, gate: Arc<AgentUpdateGate>, target: UpdateTarget) -> Self {
        Self {
            app,
            gate,
            target,
            install_before: None,
            settled_post_probe: None,
        }
    }

    pub(crate) fn store_install_before(&mut self, install: InstallState) {
        self.install_before = Some(install);
    }

    pub(crate) fn store_settled_post_probe(&mut self, install: InstallState) {
        self.settled_post_probe = Some(SettledPostProbe(install));
    }

    pub(crate) async fn publish(self, candidate: AgentUpdateResult) -> AgentUpdateResult {
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

/// #1551 - the two effects of the pass end, named fields so the supervisor cannot
/// pass them in the wrong slots.
pub(crate) struct PassEnd<I: FnOnce(), E: FnOnce()> {
    pub(crate) invalidate: I,
    pub(crate) emit: E,
}

/// #1551 - the ONLY production transition to `finished`. Order is load-bearing:
/// invalidate cache -> store the final snapshot while unfinished -> enqueue the
/// pass-finished event -> atomically open terminal fences/mark finished -> notify
/// admission. Post-update probe scheduling happens only after this returns.
pub(crate) fn finish_pass<I: FnOnce(), E: FnOnce()>(
    gate: &AgentUpdateGate,
    results: Vec<AgentUpdateResult>,
    end: PassEnd<I, E>,
) {
    (end.invalidate)();
    gate.store_final_results(results);
    (end.emit)();
    gate.release_finished();
}

pub(crate) async fn run_target_probe(
    app: &impl ManagedBroadcastEvents,
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

pub(crate) async fn run_update_target_body<E: ManagedBroadcastEvents>(
    app: &E,
    gate: &Arc<AgentUpdateGate>,
    target: &UpdateTarget,
    terminalizer: &mut TargetTerminalizer<E>,
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
pub(crate) async fn run_update_target<E: ManagedBroadcastEvents + Clone>(
    app: E,
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
                    "[agent-update] process cleanup after target panic defective for {} ({}): \
                    {detail}",
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
pub(crate) fn settle_joined_update(
    app: &impl ManagedBroadcastEvents,
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
pub(crate) fn skip_prompted_target(
    app: &impl ManagedBroadcastEvents,
    gate: &AgentUpdateGate,
    command: &str,
) {
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
pub(crate) fn schedule_post_update_probes<E>(app: &E, updated: &[UpdateTarget])
where
    E: ManagedBroadcastEvents + AgentUpdateTasks + AgentUpdateState + Clone + 'static,
{
    let Some(cache) = app.install_cache() else {
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

pub(crate) struct SupervisedTargetTask {
    pub(crate) target: UpdateTarget,
    pub(crate) handle: Option<tokio::task::JoinHandle<AgentUpdateResult>>,
}

#[derive(Default)]
pub(crate) struct PassRuntime {
    pub(crate) started: bool,
    pub(crate) known_targets: Vec<UpdateTarget>,
    pub(crate) updated_targets: Vec<UpdateTarget>,
    pub(crate) tasks: Vec<SupervisedTargetTask>,
}

pub(crate) struct PassSupervisor<E> {
    pub(crate) app: E,
    pub(crate) gate: Arc<AgentUpdateGate>,
}

impl<E> PassSupervisor<E>
where
    E: ManagedBroadcastEvents + AgentUpdateTasks + AgentUpdateState + Clone + 'static,
{
    pub(crate) fn spawn(app: E, gate: Arc<AgentUpdateGate>) -> tokio::sync::oneshot::Receiver<()> {
        let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
        let spawner = app.clone();
        spawner.spawn_update_task(Box::pin(async move {
            Self { app, gate }.run().await;
            let _ = finished_tx.send(());
        }));
        finished_rx
    }

    pub(crate) async fn terminalize_pending_cancel(&self, target: &UpdateTarget) {
        self.gate.acknowledge_unowned_cancellation(&target.command);
        TargetTerminalizer::new(self.app.clone(), Arc::clone(&self.gate), target.clone())
            .publish(cancelled_update_result(target, None))
            .await;
    }

    pub(crate) async fn run_body(&self, runtime: &mut PassRuntime) {
        let settings = match self.app.update_settings().current_snapshot().await {
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
                "[agent-update] prompt phase started: {} command(s) await SI/NO ({}s each, \
                default No): [{}]",
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

    pub(crate) async fn settle_pass_panic(&self, runtime: &mut PassRuntime) {
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

    pub(crate) async fn finalize(&self, runtime: PassRuntime) {
        self.gate.wait_all_terminal_publications().await;
        let results = self.gate.snapshot().results;
        let announced = results.clone();
        let cache = self.app.install_cache();
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

    pub(crate) async fn run(self) {
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
