use crate::commands::session::agent_command::effective_restart_requested_profile;
use crate::commands::session::agent_command::matched_selection;
use crate::commands::session::agent_command::MatchedSelection;
use crate::config::agent_command::AgentSpawnCommand;
use crate::config::coding_agents_catalog::command_executable_basename;
use crate::config::coding_agents_catalog::CodingAgentDefinition;
use crate::config::settings::AppSettings;
use crate::session::profile::idle_tuning_for_launch;
use crate::session::profile::CodingAgentKind;
use crate::session::profile::IdleTuning;
use crate::session::session::SessionInfo;

pub(crate) fn resolve_launch_idle_tuning(
    settings: &AppSettings,
    agent_id: Option<&str>,
    agent_kind: Option<CodingAgentKind>,
    catalog: &[CodingAgentDefinition],
) -> IdleTuning {
    let Some(command) = agent_id
        .and_then(|id| settings.agents.iter().find(|agent| agent.id == id))
        .map(|agent| agent.command.as_str())
    else {
        return idle_tuning_for_launch(agent_kind, None);
    };
    let matched = catalog
        .iter()
        .find(|definition| definition.command == command)
        .or_else(|| {
            let basename = command_executable_basename(command);
            basename.as_deref().and_then(|basename| {
                catalog.iter().find(|definition| {
                    command_executable_basename(&definition.command).as_deref() == Some(basename)
                })
            })
        });
    idle_tuning_for_launch(
        agent_kind,
        matched
            .and_then(|definition| definition.idle_burst.as_ref())
            .map(|burst| (burst.max_bytes, burst.max_secs, burst.prior_silence_secs)),
    )
}

pub(crate) fn resolve_launch_auto_self_clear(
    settings: &AppSettings,
    shell: &str,
    cwd: &str,
    is_coordinator: bool,
) -> bool {
    if !crate::pty::inject::supports_auto_self_maintenance(shell) {
        return false;
    }
    let class_default_on = is_coordinator || crate::config::root_agent::is_root_agent_dir_name(cwd);
    let agent_name =
        crate::config::coding_agent_profiles::agent_name_from_dir(std::path::Path::new(cwd))
            .unwrap_or_default();
    crate::config::settings::resolve_auto_self_clear(settings, &agent_name, class_default_on)
}

pub(crate) fn resolve_configured_agent_spawn_for_cwd(
    settings: &AppSettings,
    agent_id: &str,
    cwd: &str,
    requested_profile: Option<&str>,
    requested_profile_authoritative: bool,
) -> Result<Option<AgentSpawnCommand>, String> {
    // #2716 (B3, option C): the agents layer could not be read, so the list is not
    // the user's; refuse rather than fall through to a plain terminal.
    if let Some(path) = settings.agents_layer.unreadable_path() {
        return Err(crate::config::settings::agents_unreadable_session_error(
            path,
        ));
    }
    if !settings.agents.iter().any(|agent| agent.id == agent_id) {
        return Ok(None);
    }
    let cwd = crate::path_utils::normalize_windows_verbatim_path(cwd);
    crate::config::agent_command::resolve_agent_spawn_command(
        settings,
        agent_id,
        Some(std::path::Path::new(&cwd)),
        requested_profile,
        requested_profile_authoritative,
    )
    .map(Some)
}

pub(crate) fn build_configured_agent_spawn_for_cwd(
    settings: &AppSettings,
    agent_id: &str,
    cwd: &str,
    requested_profile: Option<&str>,
) -> Result<Option<AgentSpawnCommand>, String> {
    // #2716 (B3, option C): the same refusal as the resolver above; each builder
    // carries its own, so no entry point can reach a spawn past either one.
    if let Some(path) = settings.agents_layer.unreadable_path() {
        return Err(crate::config::settings::agents_unreadable_session_error(
            path,
        ));
    }
    if !settings.agents.iter().any(|agent| agent.id == agent_id) {
        return Ok(None);
    }
    let cwd = crate::path_utils::normalize_windows_verbatim_path(cwd);
    crate::config::agent_command::build_agent_spawn_command(
        settings,
        agent_id,
        Some(std::path::Path::new(&cwd)),
        requested_profile,
    )
    .map(Some)
}

/// #592 - true when a session's loaded profile cell no longer matches what a
/// Reload would load right now. Mirrors the restart-time resolution so the
/// "configured" side equals `restart_session`'s effective cell. Hashes RAW
/// cells (cell-only); swallows all "cannot determine" cases to false (never
/// false-alarms). Plain-shell sessions (no agent) never drift.
pub(crate) fn compute_profile_outdated(settings: &AppSettings, info: &SessionInfo) -> bool {
    if info.agent_id.is_none() {
        return false;
    }
    let cwd = info.working_directory.as_str();
    // #2434 - the SAME selection a Reload would launch: the matched agent, its
    // letter and its authority. `NoAgent` and `Unresolved` raise no badge (a
    // badge whose click restarts a session that cannot start is a trap).
    let requested = effective_restart_requested_profile(None, info.requested_profile.clone());
    let MatchedSelection::Agent {
        agent_id,
        profile_letter,
        authoritative,
        ..
    } = matched_selection(
        settings,
        cwd,
        None,
        info.agent_id.as_deref(),
        requested.as_deref(),
    )
    else {
        return false;
    };
    let resolution = crate::config::coding_agent_profiles::resolve_profile(
        settings,
        crate::config::coding_agent_profiles::ProfileResolutionRequest {
            coding_agent_id: &agent_id,
            launch_path: Some(std::path::Path::new(cwd)),
            agent_matrix_name: None,
            requested_profile: profile_letter.as_deref(),
            requested_profile_authoritative: authoritative,
        },
    );
    // If the agent vanished from settings, do not false-flag.
    let Some(agent) = settings.agents.iter().find(|a| a.id == agent_id) else {
        return false;
    };
    // #2431 - the one definition of a cell digest, as the spawn stamps it.
    let configured = crate::config::agent_command::cell_identity(agent, &resolution.cell);
    // Loaded hash: in-memory stamp, else the persisted replica copy (survives an
    // AC restart that cleared the in-memory stamp).
    let loaded = info.profile_content_hash.clone().or_else(|| {
        crate::config::coding_agent_profiles::read_replica_profile_content_hash(
            std::path::Path::new(cwd),
        )
    });
    match loaded {
        Some(loaded) => loaded != configured,
        None => false,
    }
}

/// §1295 S5 — resolve the create-command working directory. The bare
/// `invoke("create_session", {})` default (cwd: None) falls back to the home
/// dir. In production the creation gate then REJECTS that default when home is
/// not a registered project root (an intentional, documented UX change, §5.8);
/// every shipped frontend call site passes an explicit registered cwd.
pub(crate) fn resolve_create_session_cwd(cwd: Option<String>) -> String {
    let cwd = cwd.unwrap_or_else(|| {
        dirs::home_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "C:\\".to_string())
    });
    crate::path_utils::normalize_windows_verbatim_path(&cwd)
}
