use crate::config::agent_config;
use crate::config::settings::AppSettings;
use crate::session::session::SessionCommunication;
use crate::session::session::SessionCommunicationKind;

/// Extract the basename (without extension) from a path or command token.
pub(crate) fn executable_basename(s: &str) -> String {
    std::path::Path::new(s)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(s)
        .to_lowercase()
}

pub(crate) type ResolvedRootAgentCommand = (String, Vec<String>, Option<String>, Option<String>);

#[cfg(test)]
pub(crate) fn resolve_agent_command(
    agent_id: &str,
    settings: &AppSettings,
) -> Result<(String, Vec<String>, Option<String>), String> {
    if let Some(agent) = settings.agents.iter().find(|a| a.id == agent_id) {
        log::info!(
            "[session] Agent resolved: id={:?}, label={:?}, command={:?}",
            agent.id,
            agent.label,
            agent.command
        );
        let source = format!("selected agent '{}'", agent_id);
        let (shell, shell_args) = normalize_agent_command_for_source(agent, &source)?;
        Ok((shell, shell_args, Some(agent.label.clone())))
    } else {
        log::warn!(
            "[session] Agent NOT found for aid={:?}. Falling back to default shell.",
            agent_id
        );
        Ok((
            settings.default_shell.clone(),
            settings.default_shell_args.clone(),
            None,
        ))
    }
}

pub(crate) fn normalize_agent_command_for_source(
    agent: &crate::config::settings::AgentConfig,
    source: &str,
) -> Result<(String, Vec<String>), String> {
    crate::config::agent_command::normalize_legacy_agent_command(&agent.command)
        .map(|cmd| (cmd.shell, cmd.shell_args))
        .map_err(|e| {
            format!(
                "Invalid agent command from {} (agent id '{}', label '{}'): {}. command={:?}",
                source, agent.id, agent.label, e, agent.command
            )
        })
}

pub(crate) fn resolve_root_agent_command(
    settings: &AppSettings,
    requested_agent_id: Option<&str>,
    last_coding_agent: Option<&str>,
) -> Result<ResolvedRootAgentCommand, String> {
    let resolve_configured =
        |agent_id: &str| settings.agents.iter().find(|agent| agent.id == agent_id);

    if let Some(agent_id) = requested_agent_id {
        if let Some(agent) = resolve_configured(agent_id) {
            let source = format!("requested root agent '{}'", agent_id);
            let (shell, shell_args) = normalize_agent_command_for_source(agent, &source)?;
            return Ok((
                shell,
                shell_args,
                Some(agent.id.clone()),
                Some(agent.label.clone()),
            ));
        } else {
            log::warn!(
                "[root-agent] Requested coding agent '{}' no longer exists; falling back",
                agent_id
            );
        }
    }

    if let Some(agent_id) = last_coding_agent {
        if let Some(agent) = resolve_configured(agent_id) {
            let source = format!("root lastCodingAgent '{}'", agent_id);
            let (shell, shell_args) = normalize_agent_command_for_source(agent, &source)?;
            return Ok((
                shell,
                shell_args,
                Some(agent.id.clone()),
                Some(agent.label.clone()),
            ));
        } else {
            log::warn!(
                "[root-agent] lastCodingAgent '{}' no longer exists; falling back",
                agent_id
            );
        }
    }

    if let Some(agent) = settings.agents.first() {
        let source = format!("first configured root agent '{}'", agent.id);
        let (shell, shell_args) = normalize_agent_command_for_source(agent, &source)?;
        return Ok((
            shell,
            shell_args,
            Some(agent.id.clone()),
            Some(agent.label.clone()),
        ));
    }

    // #2716 (B3, option C): an unreadable agents layer is not "none configured".
    if let Some(path) = settings.agents_layer.unreadable_path() {
        return Err(crate::config::settings::agents_unreadable_session_error(
            path,
        ));
    }
    Err(
        "No resolvable coding agent is configured for the Root Agent. \
         Configure a coding agent before launching the Root Agent."
            .to_string(),
    )
}

pub(crate) fn resolve_agent_label(agent_id: &str, settings: &AppSettings) -> Option<String> {
    settings
        .agents
        .iter()
        .find(|a| a.id == agent_id)
        .map(|a| a.label.clone())
}

/// #2433 / #2450 seam - the enabled profile cells of `agent_id`. The single
/// accessor for reading an agent's cells here; #2450 re-keys the map and changes
/// only this body.
pub(crate) fn enabled_profile_cells<'a>(
    settings: &'a AppSettings,
    agent_id: &str,
) -> impl Iterator<Item = (&'a String, &'a crate::config::settings::ProfileCellConfig)> + 'a {
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .get(agent_id)
        .into_iter()
        .flat_map(|cells| cells.iter())
        .filter(|(_, cell)| cell.enabled)
}

/// #2433 - `(canonical command, enabled letter -> cell identity)` for `agent_id`,
/// written beside its `codingAgents` entry. `None` when the agent is absent from
/// settings: nothing is known, so no descriptor is written (an empty identity
/// would read as a real agent with zero enabled cells).
pub(crate) fn coding_agent_descriptor(
    agent_id: &str,
    settings: &AppSettings,
) -> Option<(String, std::collections::BTreeMap<String, String>)> {
    let agent = settings.agents.iter().find(|a| a.id == agent_id)?;
    let identity = enabled_profile_cells(settings, agent_id)
        .map(|(letter, cell)| {
            (
                letter.clone(),
                crate::config::agent_command::cell_identity(agent, cell),
            )
        })
        .collect();
    Some((
        crate::config::agent_command::canonical_command_text(&agent.command),
        identity,
    ))
}

/// #2433 - everything the spawn-time `codingAgents` write needs, resolved from
/// settings for the launched agent only.
pub(crate) struct LastCodingAgentWrite {
    agent_id: String,
    label: String,
    descriptor: Option<(String, std::collections::BTreeMap<String, String>)>,
}

/// Resolve the label (the provided one, else settings, else "Unknown") and the
/// descriptor for `agent_id`. Without the label fallback, callers that pass an
/// agent id but no label (session-requests, web remote) would write
/// app: "Unknown" into the per-instance config.json.
pub(crate) fn prepare_last_coding_agent_write(
    agent_id: &str,
    agent_label: Option<&str>,
    settings: &AppSettings,
) -> LastCodingAgentWrite {
    let label = match agent_label {
        Some(l) => l.to_string(),
        None => resolve_agent_label(agent_id, settings).unwrap_or_else(|| {
            log::warn!(
                "Could not resolve label for agent_id='{}' — defaulting to 'Unknown'",
                agent_id
            );
            "Unknown".to_string()
        }),
    };
    LastCodingAgentWrite {
        agent_id: agent_id.to_string(),
        label,
        descriptor: coding_agent_descriptor(agent_id, settings),
    }
}

/// The spawn-time write of `lastCodingAgent` + `codingAgents[<id>]`.
pub(crate) fn save_last_coding_agent(
    cwd: &str,
    session_id: &str,
    write: &LastCodingAgentWrite,
) -> Result<(), String> {
    agent_config::set_last_coding_agent(
        cwd,
        &write.agent_id,
        &write.label,
        Some(session_id),
        write
            .descriptor
            .as_ref()
            .map(|(command, identity)| (command.as_str(), identity)),
    )
}

pub(crate) fn resolve_actual_agent(
    shell: &str,
    shell_args: &[String],
    requested_agent_id: Option<&str>,
    requested_agent_label: Option<&str>,
    settings: &AppSettings,
) -> (Option<String>, Option<String>) {
    if let Some(agent_id) = requested_agent_id {
        if let Some(agent) = settings.agents.iter().find(|a| a.id == agent_id) {
            match crate::config::agent_command::normalize_legacy_agent_command(&agent.command) {
                Ok(normalized)
                    if normalized.shell == shell && normalized.shell_args == shell_args =>
                {
                    return (
                        Some(agent.id.clone()),
                        requested_agent_label
                            .map(ToString::to_string)
                            .or_else(|| Some(agent.label.clone())),
                    );
                }
                Ok(_) => {}
                Err(e) => {
                    log::debug!(
                        "[session] Requested agent_id='{}' command did not normalize during \
                         metadata resolution: {}",
                        agent_id,
                        e
                    );
                }
            }
        }
    }

    let detected = resolve_agent_from_shell(shell, shell_args, settings);

    if let Some(agent_id) = requested_agent_id {
        match detected.0.as_deref() {
            Some(detected_id) if detected_id == agent_id => {
                return (
                    detected.0,
                    requested_agent_label
                        .map(ToString::to_string)
                        .or(detected.1),
                )
            }
            Some(detected_id) => {
                log::warn!(
                    "[session] Requested agent_id='{}' did not match final shell-resolved agent \
                     '{}'; storing resolved agent instead",
                    agent_id,
                    detected_id
                );
                return detected;
            }
            None => {
                log::warn!(
                    "[session] Requested agent_id='{}' did not validate against final launched \
                     shell; clearing actual agent metadata",
                    agent_id
                );
                return (None, None);
            }
        }
    }

    detected
}

/// Try to match the shell command against configured agents in settings.
/// Returns (Some(agent_id), Some(label)) if a match is found, (None, None) otherwise.
pub(crate) fn resolve_agent_from_shell(
    shell: &str,
    shell_args: &[String],
    settings: &AppSettings,
) -> (Option<String>, Option<String>) {
    // Compare against already-normalized launch tokens without re-splitting shell.
    let mut cmd_basenames: Vec<String> = Vec::with_capacity(shell_args.len() + 1);
    cmd_basenames.push(executable_basename(shell));
    cmd_basenames.extend(shell_args.iter().map(|arg| executable_basename(arg)));

    for agent in &settings.agents {
        let normalized =
            match crate::config::agent_command::normalize_legacy_agent_command(&agent.command) {
                Ok(cmd) => cmd,
                Err(e) => {
                    log::debug!(
                    "[session] Skipping agent '{}' during shell auto-detection because its command \
                     is invalid: {}",
                    agent.id,
                    e
                );
                    continue;
                }
            };
        let agent_basename = executable_basename(&normalized.shell);
        if !agent_basename.is_empty() && cmd_basenames.contains(&agent_basename) {
            log::info!(
                "Auto-detected agent '{}' ({}) from shell command",
                agent.id,
                agent.label
            );
            return (Some(agent.id.clone()), Some(agent.label.clone()));
        }
    }
    (None, None)
}

/// Resolves the effective `skip_auto_resume` flag for `restart_session`.
/// Defaults to `true` (fresh conversation) to preserve existing restart-button semantics.
/// `Some(false)` is used by the deferred-wake path (ProjectPanel.handleReplicaClick)
/// to allow provider auto-resume and continue the prior conversation.
pub(crate) fn effective_restart_skip_auto_resume(requested: Option<bool>) -> bool {
    requested.unwrap_or(true)
}

/// (#630/#631) Compose the restart path's effective fresh intent: fresh on an
/// explicit "Restart Session" (`requested` defaults to `true`), OR if the session
/// still carries an unconsumed durable fresh intent (`stored_start_fresh`). A
/// deferred member reopened via Branch A passes `Some(false)`, but its persisted
/// intent wins, so it stays fresh; a normal member (`false || false`) resumes,
/// unchanged. Named so the composition is unit-tested; the call site is guarded
/// against a silent revert by the `dead_code` lint under `-D warnings` (CI runs
/// `cargo clippy --all-targets -- -D warnings`), which fails the build if
/// reverting to the inline expression leaves this function unused. Mirrors
/// `lib::skip_auto_resume_for_restore`.
pub(crate) fn restart_skip_auto_resume_with_intent(
    stored_start_fresh: bool,
    requested: Option<bool>,
) -> bool {
    stored_start_fresh || effective_restart_skip_auto_resume(requested)
}

/// (#747) Decide whether a raised hand carries across the restart
/// destroy+create boundary. Carry ONLY a visible `RaiseHand` and ONLY when the
/// conversation resumes (`restart_start_fresh == false`, i.e. the Branch-A
/// reopen of a dormant session). A fresh restart starts a new conversation;
/// the raise belonged to the old one and is dropped, consistent with
/// "destroyed sessions leave no stale raised-hand state" (#747 AC 4).
///
/// Seam map: the explicit "Restart Session" command and the bulk
/// profile-assignment restart (commands/config.rs:627-637) pass fresh and drop
/// (user-initiated resets); the sidebar dormant reopen passes `Some(false)`
/// and carries; the startup wake arm reuses this helper with the persisted
/// intent (#747 plan change 3); the agent-initiated self-clear-and-handoff
/// restart carries via its own capture/re-apply (#747 plan change 10) because
/// it is not user input. `pub(crate)` so lib.rs's wake arm shares the exact
/// same decision.
pub(crate) fn carry_communication_for_restart(
    stored: Option<SessionCommunication>,
    restart_start_fresh: bool,
) -> Option<SessionCommunication> {
    if restart_start_fresh {
        return None;
    }
    stored.filter(|c| c.kind == SessionCommunicationKind::RaiseHand && c.visible)
}

/// (#599) Resolves the effective `skip_auto_resume` for the `create_session`
/// command. Defaults to `true` (fresh conversation) so every existing
/// create-in-place / new-agent / open-agent / CLI / web call site keeps its
/// intentional "no --continue on a fresh create" semantics. The
/// reopen-of-a-closed-coordinator path (ProjectPanel.handleReplicaClick) passes
/// `Some(false)` to resume the prior conversation. Mirror of
/// `effective_restart_skip_auto_resume` so both create and restart routes share
/// the same default.
pub(crate) fn effective_create_skip_auto_resume(requested: Option<bool>) -> bool {
    requested.unwrap_or(true)
}

/// (#756) Create-path mirror consume: force a fresh spawn when the caller
/// requested resume but the cwd's coordinator carries a pending fresh boundary.
/// Named + unit-tested per the #630 pattern (the call site is guarded against a
/// silent inline revert by the `dead_code` lint under CI's `-D warnings`).
pub(crate) fn mirror_forces_fresh(
    requested_skip_auto_resume: bool,
    is_coordinator: bool,
    mirror_pending: bool,
) -> bool {
    !requested_skip_auto_resume && is_coordinator && mirror_pending
}

pub(crate) fn effective_restart_requested_profile(
    requested: Option<String>,
    stored: Option<String>,
) -> Option<String> {
    requested.or(stored)
}

/// #2434 - one matched selection, read by both the restart and the drift badge
/// so a Reload launches exactly what the badge compared against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MatchedSelection {
    /// No coding-agent reference at all: restart the saved shell. Today's path.
    NoAgent,
    /// A reference that resolved.
    Agent {
        agent_id: String,
        /// The letter to launch, straight from `AgentMatch.profile_letter`;
        /// `None` whenever no letter was chosen. An absent request stays absent.
        profile_letter: Option<String>,
        /// True only when `tier.is_some()` and `profile_letter` is `Some`: a
        /// matched letter outranks the replica pin for this spawn.
        authoritative: bool,
        tier: Option<crate::config::agent_command::MatchTier>,
        original_letter: Option<String>,
    },
    /// A reference that exists and resolves to nothing.
    Unresolved { reference: String },
}

/// #2434 - pure, read-only: pick the reference a restart of `cwd` would launch
/// (explicit request, then `tooling.currentCodingAgent`, then the session's own
/// agent) and resolve it through `resolve_portable_reference`. Never falls back
/// to another agent and never writes the reference back.
pub(crate) fn matched_selection(
    settings: &AppSettings,
    cwd: &str,
    requested_agent_id: Option<&str>,
    stored_agent_id: Option<&str>,
    source_letter: Option<&str>,
) -> MatchedSelection {
    let dir = std::path::Path::new(cwd);
    // #2786 (C1) - ONE load through the shared loader serves both
    // `currentCodingAgent` and the descriptor, so they share one snapshot.
    let config = match crate::config::agent_config::read_agent_local_config_json(dir) {
        Ok(value) => value,
        Err(e) => {
            log::warn!(
                "[agent-match] malformed '{}', no descriptor: {}",
                dir.join("config.json").display(),
                e
            );
            None
        }
    };
    let current = config
        .as_ref()
        .and_then(|value| value.get("tooling")?.get("currentCodingAgent")?.as_str())
        // An empty or whitespace-only id names nothing: it is absent, always.
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string);
    // Only a stale `currentCodingAgent` on a session with no agent of its own
    // may degrade to the saved shell; an explicit request never does.
    let plain_shell_selection =
        requested_agent_id.is_none() && stored_agent_id.is_none() && current.is_some();
    let Some(id) = requested_agent_id
        .map(str::to_string)
        .or(current)
        .or_else(|| stored_agent_id.map(str::to_string))
    else {
        return MatchedSelection::NoAgent;
    };
    let mut reference =
        crate::config::agent_command::StoredReference::from_config(config.as_ref(), &id);
    reference.source_letter = source_letter.map(str::to_string);
    match crate::config::agent_command::resolve_portable_reference(settings, &reference) {
        crate::config::agent_command::MatchOutcome::Matched(found) => MatchedSelection::Agent {
            authoritative: found.tier.is_some() && found.profile_letter.is_some(),
            agent_id: found.agent_id,
            profile_letter: found.profile_letter,
            tier: found.tier,
            original_letter: found.original_letter,
        },
        crate::config::agent_command::MatchOutcome::NoMatch { reference } => {
            if plain_shell_selection {
                // Nothing else to launch: the session already is a plain shell.
                log::warn!(
                    "[agent-match] ignoring unresolvable currentCodingAgent '{}' in '{}'; \
                     restarting the saved shell",
                    reference,
                    cwd
                );
                MatchedSelection::NoAgent
            } else {
                MatchedSelection::Unresolved { reference }
            }
        }
    }
}

/// #537 read-side: pick which coding agent a restart should launch.
///
/// Ranks an explicit request first, then the replica's Selection-UI assignment
/// (`tooling.currentCodingAgent` in `cwd/config.json`, validated against the
/// configured agents), then the agent the live session was launched with. This
/// mirrors the wake path (#534): a fresh user selection must override stale
/// launch history. The `currentCodingAgent` is validated so an unconfigured id
/// is never passed downstream (which would make `build_configured_agent_spawn_for_cwd`
/// return `Ok(None)` and silently keep the old recipe). Root agents and plain
/// terminals have no `currentCodingAgent`, so they fall straight through to
/// `stored_agent_id` and their behavior is unchanged.
pub(crate) fn resolve_restart_selected_agent_id(
    settings: &AppSettings,
    cwd: &str,
    requested_agent_id: Option<&str>,
    stored_agent_id: Option<&str>,
) -> Option<String> {
    if let Some(requested) = requested_agent_id {
        return Some(requested.to_string());
    }
    let current_selection =
        crate::config::coding_agent_profiles::read_replica_current_coding_agent(
            std::path::Path::new(cwd),
        )
        .filter(|id| settings.agents.iter().any(|agent| agent.id == *id));
    current_selection.or_else(|| stored_agent_id.map(str::to_string))
}
