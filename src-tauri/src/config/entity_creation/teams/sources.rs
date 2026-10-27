//! Team source orchestration; publication belongs to the team store.
use super::store::{
    create_new_team_config_on_disk_guarded, normalized_team_config_bytes, publish_owned_team,
    TeamConfigMutationGuard,
};
use crate::config::entity_creation::copies::{
    copy_agent_matrix_strict_with_hooks, preflight_team_sources, require_absent,
};
use crate::config::entity_creation::names::{
    selected_ac_root, validate_existing_name, TeamAgentSource, TeamConfigResult,
};
use crate::filesystem::projects::entities::copies::{
    cleanup_owned_copy_artifacts, MatrixCopyHooks, MatrixCopyStage, OwnedCopyArtifacts,
};
use std::path::{Path, PathBuf};

pub(crate) fn create_team_from_sources(
    project: &Path,
    name: &str,
    mut config: TeamConfigResult,
    sources: Option<Vec<TeamAgentSource>>,
    hooks: &mut MatrixCopyHooks<'_>,
) -> Result<PathBuf, String> {
    validate_existing_name(name, "Team")?;
    let ac = selected_ac_root(project)?;
    let guard = TeamConfigMutationGuard::acquire(&ac)?;
    let Some(sources) = sources else {
        return create_new_team_config_on_disk_guarded(&ac, name, &config, &guard);
    };
    let ac = crate::config::projects::canonical_source_project(&ac).map_err(|e| e.to_string())?;
    let mut team = OwnedCopyArtifacts {
        root: ac.join(format!("_team_{name}")),
        entries: Vec::new(),
    };
    require_absent(&team.root)?;
    let foreign = preflight_team_sources(project, &ac, &mut config, sources)?;
    let mut copies = Vec::new();
    let result = (|| -> Result<(), String> {
        for (source_project, agent) in foreign {
            copies.push(copy_agent_matrix_strict_with_hooks(
                &source_project,
                &agent,
                &ac,
                &agent,
                hooks,
            )?);
        }
        let bytes = normalized_team_config_bytes(&ac, &config)?;
        publish_owned_team(&mut team, &bytes, &copies, hooks)
    })();
    if let Err(primary) = result {
        let mut errors = vec![primary];
        if let Err(error) = hooks.run(MatrixCopyStage::BeforeCleanup, &team.root) {
            errors.push(error);
        }
        for owned in std::iter::once(&team).chain(copies.iter().rev()) {
            if let Err(error) = cleanup_owned_copy_artifacts(owned) {
                errors.push(error);
            }
        }
        return Err(errors.join("; "));
    }
    Ok(team.root)
}

#[cfg(test)]
mod tests;
