use super::paths::{
    agent_name_from_dir, normalize_profile_from_source, origin_matrix_dir_for_launch_path,
    validate_profile_selection_agent_path,
};
use super::replica_profile::read_replica_profile_result;
use super::{
    read_tooling_string, ProfileResolution, ProfileResolutionRequest, ProfileSelectionResolution,
};
use crate::config::settings::{
    empty_profile_cell, normalize_profile_letter, AppSettings, ProfileCellConfig,
};
use std::path::Path;

pub fn resolve_profile_selection(
    settings: &AppSettings,
    launch_path: Option<&Path>,
    coding_agent_id: &str,
    requested_profile: Option<&str>,
) -> Result<ProfileSelectionResolution, String> {
    if !settings
        .agents
        .iter()
        .any(|agent| agent.id == coding_agent_id)
    {
        return Err(format!("Agent '{}' is not configured", coding_agent_id));
    }

    let validated = launch_path
        .map(|path| validate_profile_selection_agent_path(settings, path))
        .transpose()?;
    let launch_path = validated
        .as_ref()
        .map(|validated| validated.launch_path.as_path());
    let origin_matrix_dir = validated
        .as_ref()
        .map(|validated| validated.origin_matrix_dir.as_path());
    let agent_name = launch_path.and_then(agent_name_from_dir);
    let instance_read = launch_path.map(read_replica_profile_result);
    let instance_profile_override = instance_read.as_ref().and_then(|read| read.profile.clone());
    let origin_default_profile = origin_matrix_dir
        .and_then(|path| read_tooling_string(path, "defaultProfile"))
        .and_then(|value| normalize_profile_letter(&value));
    let agent_default_profile = agent_name
        .as_ref()
        .and_then(|name| {
            settings
                .coding_agent_profiles
                .default_profile_by_agent
                .get(name)
        })
        .and_then(|value| normalize_profile_letter(value));
    let requested_profile_input = requested_profile.map(str::to_string);
    let mut resolution = resolve_profile(
        settings,
        ProfileResolutionRequest {
            coding_agent_id,
            launch_path,
            agent_matrix_name: None,
            requested_profile,
            requested_profile_authoritative: false,
        },
    );
    if let Some(warning) = instance_read.and_then(|read| read.warning) {
        resolution.warnings.push(warning);
    }

    Ok(ProfileSelectionResolution {
        requested_profile_input,
        instance_profile_override,
        origin_default_profile,
        agent_default_profile,
        resolution,
    })
}

pub(super) fn cell_for_letter(
    settings: &AppSettings,
    coding_agent_id: &str,
    letter: &str,
) -> Option<ProfileCellConfig> {
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .get(coding_agent_id)
        .and_then(|cells| cells.get(letter))
        .filter(|cell| cell.enabled)
        .cloned()
}

pub(super) fn fallback_letters_from(requested: &str) -> Vec<String> {
    let mut letters = Vec::new();
    let start = requested.as_bytes()[0];
    for byte in (b'A'..=start).rev() {
        letters.push((byte as char).to_string());
    }
    letters
}

pub fn resolve_profile(
    settings: &AppSettings,
    request: ProfileResolutionRequest<'_>,
) -> ProfileResolution {
    let mut warnings = Vec::new();

    let launch_path = request.launch_path;
    let agent_name = request
        .agent_matrix_name
        .map(str::to_string)
        .or_else(|| launch_path.and_then(agent_name_from_dir));

    let instance_override = launch_path.and_then(|path| {
        let read = read_replica_profile_result(path);
        if let Some(warning) = read.warning {
            warnings.push(warning);
        }
        normalize_profile_from_source(read.profile, &mut warnings, "instance override")
    });

    let origin_default =
        launch_path.and_then(|path| match origin_matrix_dir_for_launch_path(path) {
            Ok(Some(origin)) => normalize_profile_from_source(
                read_tooling_string(&origin, "defaultProfile"),
                &mut warnings,
                "origin default",
            ),
            Ok(None) => None,
            Err(e) => {
                warnings.push(format!("Ignoring origin default profile: {}", e));
                None
            }
        });

    let explicit = request.requested_profile.and_then(|letter| {
        normalize_profile_from_source(Some(letter.to_string()), &mut warnings, "launch request")
    });

    let agent_default = agent_name
        .as_ref()
        .and_then(|name| {
            settings
                .coding_agent_profiles
                .default_profile_by_agent
                .get(name)
        })
        .and_then(|letter| {
            normalize_profile_from_source(Some(letter.clone()), &mut warnings, "agent default")
        });

    // #1635-2 D2: a wake dispatch request outranks the replica pin for that
    // spawn only; every other caller keeps the historical ranking (instance
    // override wins over the explicit request).
    let requested_profile = if request.requested_profile_authoritative {
        explicit
            .or(instance_override)
            .or(origin_default)
            .or(agent_default)
            .unwrap_or_else(|| "A".to_string())
    } else {
        instance_override
            .or(explicit)
            .or(origin_default)
            .or(agent_default)
            .unwrap_or_else(|| "A".to_string())
    };

    let mut fallback_chain = Vec::new();
    let mut effective_profile = "A".to_string();
    let mut effective_cell = empty_profile_cell();

    for letter in fallback_letters_from(&requested_profile) {
        fallback_chain.push(letter.clone());
        let cell = if letter == "A" {
            cell_for_letter(settings, request.coding_agent_id, &letter)
                .unwrap_or_else(empty_profile_cell)
        } else if let Some(cell) = cell_for_letter(settings, request.coding_agent_id, &letter) {
            cell
        } else {
            continue;
        };
        effective_profile = letter;
        effective_cell = cell;
        break;
    }

    let fallback_applied = effective_profile != requested_profile;
    ProfileResolution {
        requested_profile,
        effective_profile,
        fallback_chain,
        fallback_applied,
        cell: effective_cell,
        warnings,
    }
}
