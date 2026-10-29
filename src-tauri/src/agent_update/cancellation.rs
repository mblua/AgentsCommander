use super::gate::{AgentUpdateGate, GateState};
use super::target::{ResponseReleaseGuard, TargetClaim, TargetPhase};
use super::ManagedBroadcastEvents;
use super::{
    emit_all, AgentUpdateCancelAllResponse, AgentUpdateCancelDisposition,
    AgentUpdateCancelResponse, AgentUpdateCancellationChanged, AgentUpdateCommandRef,
};
use serde_json::json;
use std::sync::Arc;

pub(crate) fn cancellation_snapshot(state: &GateState) -> AgentUpdateCancellationChanged {
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
    app: &impl ManagedBroadcastEvents,
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
    app: &impl ManagedBroadcastEvents,
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
