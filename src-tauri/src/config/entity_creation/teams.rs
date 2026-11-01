mod conventions;
mod raw;
mod sources;
mod store;

pub(crate) use raw::{collect_agent_team_mutations_raw, AgentTeamMutation};
pub(crate) use sources::create_team_from_sources;
#[cfg(test)]
pub(crate) use store::normalized_team_config_bytes;

pub(crate) use store::{
    acquire_team_config_mutation_guard_async, create_new_team_config_on_disk,
    create_new_team_config_on_disk_guarded, inspect_team_skill_source,
    normalize_team_config_for_project, read_discovery_team_config, read_team_config,
    read_team_config_bytes, read_team_config_classified, resolve_context_alert_argument,
    write_existing_team_config_guarded, write_team_config_guarded, write_team_config_json_atomic,
    ContextAlertWriteMode, TeamConfigMutationGuard, TeamConfigReadError, TeamSkillSourceInspection,
};
#[cfg(test)]
pub(crate) use store::{
    write_team_config_guarded_with_publisher, TeamConfigLockTiming, TEAM_CONFIG_MUTATION_LOCK_NAME,
};
