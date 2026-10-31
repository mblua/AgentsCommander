use crate::commands::session::agent_command::{matched_selection, MatchedSelection};
use crate::commands::session::create::launch::resolve_configured_agent_spawn_for_cwd;
use crate::config::agent_command::AgentSpawnCommand;
use crate::config::settings::AppSettings;
use crate::pty::backend::ResolvedAgentHostShell;

pub(crate) struct RestartSelection {
    pub(crate) selected_agent_id: Option<String>,
    pub(crate) resolved_spawn: Option<AgentSpawnCommand>,
    pub(crate) resolved_agent_host_shell: Option<ResolvedAgentHostShell>,
}

pub(crate) fn resolve_restart_selection(
    cfg: &AppSettings,
    cwd: &str,
    requested_agent_id: Option<&str>,
    stored_agent_id: Option<&str>,
    selected_requested_profile: Option<&str>,
) -> Result<RestartSelection, String> {
    let selection = matched_selection(
        cfg,
        cwd,
        requested_agent_id,
        stored_agent_id,
        selected_requested_profile,
    );
    let (selected_agent_id, resolved_spawn) = match selection {
        MatchedSelection::NoAgent => (None, None),
        MatchedSelection::Unresolved { reference } => {
            return Err(format!(
                "unresolved_coding_agent_reference: '{}' matches no configured coding agent",
                reference
            ));
        }
        MatchedSelection::Agent {
            agent_id,
            profile_letter,
            authoritative,
            tier,
            original_letter,
        } => {
            let spawn = resolve_configured_agent_spawn_for_cwd(
                cfg,
                &agent_id,
                cwd,
                profile_letter.as_deref(),
                authoritative,
            )?
            .map(|mut spawn| {
                // A matched restart records the ORIGINAL request; the matched
                // letter lives in `effective_profile`, its provenance in the
                // #2451 carriers.
                if authoritative {
                    if let Some(original) = selected_requested_profile {
                        spawn.profile_resolution.requested_profile = original.to_string();
                    }
                }
                spawn.match_tier = tier.map(|tier| tier.as_wire_str().to_string());
                spawn.original_profile_letter = original_letter;
                spawn
            });
            (Some(agent_id), spawn)
        }
    };
    let resolved_agent_host_shell = if resolved_spawn.is_some() {
        Some(ResolvedAgentHostShell {
            program: cfg.default_shell.clone(),
            args: cfg.default_shell_args.clone(),
        })
    } else {
        None
    };
    Ok(RestartSelection {
        selected_agent_id,
        resolved_spawn,
        resolved_agent_host_shell,
    })
}
