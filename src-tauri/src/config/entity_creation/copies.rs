//! Matrix copy orchestration and team source validation.
use super::matrices::{write_copy_default_config, AGENT_MATRIX_DIRS};
use super::names::{
    normalize_context_alert_percentages, resolve_agent_ref, selected_ac_root,
    validate_existing_name, TeamAgentSource, TeamConfigResult,
};
use super::provenance::{
    append_copy_provenance, read_copy_provenance, validate_copy_provenance, write_copy_provenance,
};
use crate::filesystem::projects::entities::copies::{
    self as disk, MatrixCopyHooks, MatrixCopyIdentity, MatrixCopyStage, OwnedCopyArtifacts,
};
pub(crate) use crate::filesystem::projects::entities::copies::{
    cleanup_owned_copy_artifacts, require_absent, verify_owned_entry, write_owned_team_file,
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[cfg(test)]
pub(crate) fn copy_agent_matrix_strict(
    source_project: &Path,
    source_ref: &str,
    destination_ac: &Path,
    destination_ref: &str,
) -> Result<OwnedCopyArtifacts, String> {
    copy_agent_matrix_strict_with_hooks(
        source_project,
        source_ref,
        destination_ac,
        destination_ref,
        &mut MatrixCopyHooks::default(),
    )
}

pub(crate) fn copy_agent_matrix_strict_with_hooks(
    source_project: &Path,
    source_ref: &str,
    destination_ac: &Path,
    destination_ref: &str,
    hooks: &mut MatrixCopyHooks<'_>,
) -> Result<OwnedCopyArtifacts, String> {
    let project = crate::config::projects::canonical_source_project(source_project)
        .map_err(|e| e.to_string())?;
    let ac = disk::canonical_source_ac(&selected_ac_root(&project)?)?;
    let source_ref = resolve_agent_ref(&ac, source_ref)?;
    let source = ac.join(&source_ref);
    let source_session = disk::open_source_matrix(
        &source,
        #[cfg(test)]
        hooks.observer.as_ref(),
    )?;
    let canonical_source = disk::canonical_source_matrix(&source)?;
    let parent = canonical_source
        .parent()
        .ok_or("Source Matrix has no parent")?;
    #[cfg(windows)]
    let same_parent = {
        let key = |path: &Path| {
            crate::path_utils::path_to_string_without_windows_verbatim_prefix(path)
                .replace(char::from(92), "/")
                .trim_end_matches('/')
                .to_ascii_lowercase()
        };
        key(parent) == key(&ac)
    };
    #[cfg(not(windows))]
    let same_parent = parent == ac;
    if !same_parent {
        return Err("Source Matrix is not a direct project child".into());
    }
    let source_ref = canonical_source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("Source Matrix has no portable name")?;
    let source_name = source_ref
        .strip_prefix("_agent_")
        .ok_or("Invalid source Matrix name")?;
    validate_existing_name(source_name, "Agent")?;
    let bare = destination_ref
        .strip_prefix("_agent_")
        .ok_or("Invalid destination agent ref")?;
    validate_existing_name(bare, "Agent")?;
    if destination_ref.contains(['/', '\\', ':', '\0']) {
        return Err("Invalid destination agent ref".into());
    }
    let destination_ac = disk::canonical_destination_ac(destination_ac)?;
    let root = destination_ac.join(destination_ref);
    if root.starts_with(&canonical_source) {
        return Err("Copy destination lies inside source Matrix".into());
    }
    let mut owned = OwnedCopyArtifacts {
        root,
        entries: Vec::new(),
    };
    owned.create_directory(Path::new(""))?;
    let result = (|| -> Result<(), String> {
        hooks.run(MatrixCopyStage::Reserved, &owned.root)?;
        let mut first = std::collections::BTreeMap::new();
        OwnedCopyArtifacts::scan(&source, Path::new(""), Some(&mut owned), &mut first, hooks)?;
        let provenance = validate_copy_provenance(None)?;
        hooks.run(MatrixCopyStage::BeforeVerify, &source)?;
        let mut second = std::collections::BTreeMap::new();
        OwnedCopyArtifacts::scan(&source, Path::new(""), None, &mut second, hooks)?;
        let _current = source_session.reopen_and_verify_root(
            &source,
            #[cfg(test)]
            hooks.observer.as_ref(),
        )?;
        if first != second {
            return Err("Source Matrix changed during copy".into());
        }
        for &directory in AGENT_MATRIX_DIRS {
            if directory != "skills" || !first.contains_key(Path::new("skills")) {
                owned.create_directory(Path::new(directory))?;
            }
        }
        write_copy_default_config(|path, bytes| write_owned_team_file(&mut owned, path, bytes))?;
        hooks.run(MatrixCopyStage::ConfigWritten, &owned.root)?;
        let provenance = append_copy_provenance(provenance, &project, source_ref)?;
        write_copy_provenance(&mut owned, &provenance, hooks)?;
        Ok(())
    })();
    if let Err(primary) = result {
        let hook_error = hooks.run(MatrixCopyStage::BeforeCleanup, &owned.root).err();
        let cleanup = cleanup_owned_copy_artifacts(&owned).err();
        return Err(format!(
            "Matrix copy failed: {primary}{}{}",
            hook_error
                .map(|e| format!("; cleanup hook: {e}"))
                .unwrap_or_default(),
            cleanup.map(|e| format!("; {e}")).unwrap_or_default()
        ));
    }
    Ok(owned)
}

pub(crate) fn project_directory_identity(
    project: &Path,
) -> Result<(PathBuf, MatrixCopyIdentity), String> {
    let canonical =
        crate::config::projects::canonical_source_project(project).map_err(|e| e.to_string())?;
    selected_ac_root(&canonical)?;
    let identity = disk::identity_existing_directory(&canonical)?;
    Ok((canonical, identity))
}

pub(crate) fn portable_source_ref(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.contains(['/', '\\', ':', '\0']) {
        return Err("Invalid portable agent reference".into());
    }
    let bare = trimmed.strip_prefix("_agent_").unwrap_or(trimmed);
    validate_existing_name(bare, "Agent")?;
    Ok(format!("_agent_{bare}"))
}

pub(crate) fn preflight_team_sources(
    project: &Path,
    ac: &Path,
    config: &mut TeamConfigResult,
    sources: Vec<TeamAgentSource>,
) -> Result<Vec<(PathBuf, String)>, String> {
    let (_, destination_identity) = project_directory_identity(project)?;
    let mut selected = HashSet::new();
    for agent in &mut config.agents {
        *agent = portable_source_ref(agent)?;
        if !selected.insert(agent.clone()) {
            return Err(format!("Duplicate selected agent: {agent}"));
        }
    }
    if !config.coordinator.trim().is_empty() {
        config.coordinator = portable_source_ref(&config.coordinator)?;
        if !selected.contains(&config.coordinator) {
            return Err("Orchestrator must be one of the selected agents".into());
        }
    }
    for repo in &mut config.repos {
        for agent in &mut repo.agents {
            *agent = portable_source_ref(agent)?;
            if !selected.contains(agent) {
                return Err("Repository agent must be one of the selected agents".into());
            }
        }
    }
    normalize_context_alert_percentages(&config.context_alert_percentages)?;
    let mut seen = HashSet::new();
    let mut foreign = Vec::new();
    for source in sources {
        let agent = portable_source_ref(&source.agent_ref)?;
        if !selected.contains(&agent) || !seen.insert(agent.clone()) {
            return Err(format!("Extra or duplicate agent source: {agent}"));
        }
        let (source_project, identity) =
            project_directory_identity(Path::new(&source.source_project_path))?;
        let source_ac = disk::canonical_source_ac(&selected_ac_root(&source_project)?)?;
        resolve_agent_ref(&source_ac, &agent)?;
        let matrix = source_ac.join(&agent);
        disk::with_source_parent(
            &matrix,
            &source_ac,
            #[cfg(test)]
            None,
            || {
                if identity != destination_identity {
                    require_absent(&ac.join(&agent))?;
                    foreign.push((source_project, agent));
                }
                Ok(())
            },
        )?;
    }
    if seen != selected {
        return Err("Agent sources must cover exactly all selected agents".into());
    }
    Ok(foreign)
}

pub(crate) fn verify_team_copies(copies: &[OwnedCopyArtifacts]) -> Result<(), String> {
    for copy in copies {
        for entry in &copy.entries {
            verify_owned_entry(copy, entry)?;
        }
        read_copy_provenance(&copy.root)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
