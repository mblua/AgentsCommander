//! Re-export seam for team-config reading, writing and the mutation guard.
//! Exposes `TeamConfigMutationGuard`, the guarded writers and name/context-alert helpers to
//! the other entity-creation commands; some items are test-only.
//! No logic lives here: it is in `crate::config::entity_creation::teams` and `names`.

pub(crate) use crate::config::entity_creation::names::{
    normalize_context_alert_percentages, resolve_agent_ref,
};
#[cfg(test)]
pub(crate) use crate::config::entity_creation::teams::normalized_team_config_bytes;
pub(crate) use crate::config::entity_creation::teams::{
    acquire_team_config_mutation_guard_async, create_new_team_config_on_disk,
    create_new_team_config_on_disk_guarded, normalize_team_config_for_project, read_team_config,
    read_team_config_classified, resolve_context_alert_argument,
    write_existing_team_config_guarded, write_team_config_guarded, ContextAlertWriteMode,
    TeamConfigMutationGuard, TeamConfigReadError,
};
#[cfg(test)]
pub(crate) use crate::config::entity_creation::teams::{
    write_team_config_guarded_with_publisher, TeamConfigLockTiming, TEAM_CONFIG_MUTATION_LOCK_NAME,
};
