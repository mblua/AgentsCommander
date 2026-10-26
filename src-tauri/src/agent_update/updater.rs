use super::gate::AgentUpdateGate;
use super::plan::UpdateTarget;
use super::target::{StepAdmission, TargetWork};
use super::{AgentUpdateChange, AgentUpdateOutcome, AgentUpdateResult};
#[cfg(test)]
use super::{AgentUpdateNode, CANCELLATION_CLEANUP_ERROR};
use crate::agent_version::{InstallState, InstallStatus, RetainedProbeOwner};
use crate::config::agent_path::effective_search_path;
use std::time::Duration;

#[cfg(test)]
pub(crate) use crate::process::agent::update::{
    build_update_step_command_for_test as build_update_step_command, with_target_settlement_hook,
    TargetNativeObservation, TargetReaderSettlementHook, TargetSettlementHook,
    TARGET_SETTLEMENT_HOOK,
};
pub(crate) use crate::process::agent::update::{
    output_tail, TargetProcessOwner, TargetSettlement, TargetWait, UpdaterStepOutcome,
};

pub(crate) struct TargetOwnerLease {
    pub(crate) work: TargetWork,
    pub(crate) epoch: u64,
    pub(crate) cancel: tokio::sync::watch::Receiver<bool>,
}

/// One externally retained owner/receiver/handoff lease for a target. The slot
/// lives outside `run_update_target_body`'s caught future, so unwinding during
/// updater or probe settlement cannot discard the only cleanup authority.
#[derive(Default)]
pub(crate) struct TargetOwnerSlot {
    pub(crate) lease: Option<TargetOwnerLease>,
    pub(crate) updater: Option<TargetProcessOwner>,
    pub(crate) probe: RetainedProbeOwner,
}

impl TargetOwnerSlot {
    pub(crate) fn arm(
        &mut self,
        work: TargetWork,
        epoch: u64,
        cancel: tokio::sync::watch::Receiver<bool>,
    ) {
        debug_assert!(self.lease.is_none());
        debug_assert!(self.updater.is_none());
        debug_assert!(!self.probe.is_some());
        self.lease = Some(TargetOwnerLease {
            work,
            epoch,
            cancel,
        });
    }

    pub(crate) fn identity(&self) -> (TargetWork, u64) {
        let lease = self.lease.as_ref().expect("active target owner lease");
        (lease.work, lease.epoch)
    }

    pub(crate) fn complete_without_owner(&mut self, gate: &AgentUpdateGate, command: &str) -> bool {
        let (work, epoch) = self.identity();
        let reserved = gate.completed_without_owner(command, work, epoch);
        self.clear();
        reserved
    }

    pub(crate) fn owner_returned(&mut self, gate: &AgentUpdateGate, command: &str) -> bool {
        let (work, epoch) = self.identity();
        let reserved = gate.owner_returned(command, work, epoch);
        self.clear();
        reserved
    }

    pub(crate) fn clear(&mut self) {
        drop(self.updater.take());
        self.probe.clear();
        self.lease = None;
    }

    pub(crate) async fn settle_after_panic(
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

pub(crate) async fn run_update_sequence_cancellable(
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

        match TargetProcessOwner::spawn_step(cmd, &target.cwd, &effective_search_path()).await {
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

#[cfg(test)]
pub(crate) async fn run_update_sequence(
    target: &UpdateTarget,
    step_timeout: Duration,
) -> AgentUpdateResult {
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

pub(crate) fn successful_update_result(
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

pub(crate) fn failed_update_result(
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

pub(crate) fn cancelled_update_result(
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
