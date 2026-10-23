use crate::config::agent_config;
use crate::config::settings::AppSettings;

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
