use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::config::settings::{
    empty_profile_cell, normalize_profile_letter, AppSettings, ProfileCellConfig,
};
use crate::config::workspace::{
    ensure_authoritative_workspace_dir, is_workspace_dir_name, CANONICAL_WORKSPACE_DIR,
};

#[derive(Debug, Clone)]
pub struct ProfileResolutionRequest<'a> {
    pub coding_agent_id: &'a str,
    pub launch_path: Option<&'a Path>,
    pub agent_matrix_name: Option<&'a str>,
    pub requested_profile: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileResolution {
    pub requested_profile: String,
    pub effective_profile: String,
    pub fallback_chain: Vec<String>,
    pub fallback_applied: bool,
    pub cell: ProfileCellConfig,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedProfileAgentPath {
    pub launch_path: PathBuf,
    pub origin_matrix_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileSelectionResolution {
    pub requested_profile_input: Option<String>,
    pub instance_profile_override: Option<String>,
    pub origin_default_profile: Option<String>,
    pub agent_default_profile: Option<String>,
    pub resolution: ProfileResolution,
}

fn read_json_object(path: &Path) -> Option<Value> {
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<Value>(&content).ok()
}

fn read_tooling_string(agent_dir: &Path, key: &str) -> Option<String> {
    read_json_object(&agent_dir.join("config.json"))?
        .get("tooling")?
        .get(key)?
        .as_str()
        .map(str::to_string)
}

fn write_tooling_string(agent_dir: &Path, key: &str, value: Option<&str>) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(agent_dir).map_err(|e| {
        format!(
            "Agent config dir '{}' is not readable: {}",
            agent_dir.display(),
            e
        )
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!(
            "Agent config dir '{}' is not a real directory",
            agent_dir.display()
        ));
    }
    let config_path = agent_dir.join("config.json");
    crate::config::local_config_io::update_config_json_object(&config_path, true, |obj| {
        let tooling = obj
            .entry("tooling".to_string())
            .or_insert_with(|| serde_json::json!({}));
        if !tooling.is_object() {
            *tooling = serde_json::json!({});
        }
        let tooling = tooling.as_object_mut().expect("tooling set to object");
        match value {
            Some(value) => {
                tooling.insert(key.to_string(), Value::String(value.to_string()));
            }
            None => {
                tooling.remove(key);
            }
        }
        Ok(())
    })?;
    Ok(())
}

fn strip_extended_prefix(path: PathBuf) -> PathBuf {
    let s = path.to_string_lossy();
    s.strip_prefix(r"\\?\").map(PathBuf::from).unwrap_or(path)
}

fn canonical_existing_dir(path: &Path, label: &str) -> Result<PathBuf, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|e| format!("{} '{}' is not readable: {}", label, path.display(), e))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!(
            "{} '{}' is not a real directory",
            label,
            path.display()
        ));
    }
    std::fs::canonicalize(path)
        .map(strip_extended_prefix)
        .map_err(|e| {
            format!(
                "Failed to canonicalize {} '{}': {}",
                label,
                path.display(),
                e
            )
        })
}

#[cfg(windows)]
fn same_canonical_path(left: &Path, right: &Path) -> bool {
    left.to_string_lossy()
        .eq_ignore_ascii_case(&right.to_string_lossy())
}

#[cfg(not(windows))]
fn same_canonical_path(left: &Path, right: &Path) -> bool {
    left == right
}

fn collect_workspace_candidate(out: &mut Vec<PathBuf>, candidate: &Path) {
    if !candidate.is_dir() || ensure_authoritative_workspace_dir(candidate).is_err() {
        return;
    }
    let Ok(canonical) = canonical_existing_dir(candidate, "Project AC Root") else {
        return;
    };
    if !out
        .iter()
        .any(|existing| same_canonical_path(existing, &canonical))
    {
        out.push(canonical);
    }
}

pub(crate) fn configured_workspace_dirs(settings: &AppSettings) -> Vec<PathBuf> {
    let mut workspaces = Vec::new();
    for project_path in &settings.project_paths {
        let base = Path::new(project_path);
        if base
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(is_workspace_dir_name)
        {
            collect_workspace_candidate(&mut workspaces, base);
        }

        collect_workspace_candidate(&mut workspaces, &base.join(CANONICAL_WORKSPACE_DIR));

        let Ok(entries) = std::fs::read_dir(base) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                collect_workspace_candidate(
                    &mut workspaces,
                    &entry.path().join(CANONICAL_WORKSPACE_DIR),
                );
            }
        }
    }
    workspaces
}

fn ensure_workspace_is_configured(
    settings: &AppSettings,
    workspace_dir: &Path,
) -> Result<(), String> {
    let workspace = canonical_existing_dir(workspace_dir, "Project AC Root")?;
    let configured = configured_workspace_dirs(settings);
    if configured
        .iter()
        .any(|candidate| same_canonical_path(candidate, &workspace))
    {
        return Ok(());
    }

    Err(format!(
        "Agent path is outside configured AC project roots: {}",
        workspace.display()
    ))
}

fn validate_authoritative_matrix_dir(matrix_dir: &Path) -> Result<PathBuf, String> {
    let matrix_dir = canonical_existing_dir(matrix_dir, "Agent Matrix")?;
    let dir_name = matrix_dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("Agent Matrix '{}' has no valid name", matrix_dir.display()))?;
    if !dir_name.starts_with("_agent_") {
        return Err(format!(
            "Agent Matrix '{}' must be named '_agent_<name>'",
            matrix_dir.display()
        ));
    }
    let workspace_dir = matrix_dir.parent().ok_or_else(|| {
        format!(
            "Agent Matrix '{}' has no parent Project AC Root",
            matrix_dir.display()
        )
    })?;
    ensure_authoritative_workspace_dir(workspace_dir)?;
    Ok(matrix_dir)
}

fn validated_replica_origin(replica_dir: &Path) -> Result<(PathBuf, PathBuf), String> {
    let replica_dir = canonical_existing_dir(replica_dir, "WG replica")?;
    let dir_name = replica_dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("WG replica '{}' has no valid name", replica_dir.display()))?;
    if !dir_name.starts_with("__agent_") {
        return Err(format!(
            "WG replica '{}' must be named '__agent_<name>'",
            replica_dir.display()
        ));
    }
    let wg_dir = replica_dir.parent().ok_or_else(|| {
        format!(
            "WG replica '{}' has no parent workgroup",
            replica_dir.display()
        )
    })?;
    let wg_name = wg_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if !wg_name.starts_with("wg-") {
        return Err(format!(
            "WG replica '{}' is not inside a wg-* workgroup",
            replica_dir.display()
        ));
    }

    let persisted_identity =
        read_json_object(&replica_dir.join("config.json")).and_then(|config| {
            config
                .get("identity")
                .and_then(|value| value.as_str())
                .map(str::to_string)
        });
    let identity = crate::config::replica_identity::validate_or_repair_wg_replica_identity(
        &replica_dir,
        persisted_identity.as_deref(),
    )?;
    let origin = validate_authoritative_matrix_dir(&identity.matrix_dir)?;
    Ok((replica_dir, origin))
}

pub fn validate_profile_selection_agent_path(
    settings: &AppSettings,
    launch_path: &Path,
) -> Result<ValidatedProfileAgentPath, String> {
    let dir_name = launch_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("Agent path '{}' has no valid name", launch_path.display()))?;

    if dir_name.starts_with("_agent_") {
        let launch_path = validate_authoritative_matrix_dir(launch_path)?;
        let workspace_dir = launch_path.parent().ok_or_else(|| {
            format!(
                "Agent Matrix '{}' has no parent Project AC Root",
                launch_path.display()
            )
        })?;
        ensure_workspace_is_configured(settings, workspace_dir)?;
        return Ok(ValidatedProfileAgentPath {
            origin_matrix_dir: launch_path.clone(),
            launch_path,
        });
    }

    if dir_name.starts_with("__agent_") {
        let (launch_path, origin_matrix_dir) = validated_replica_origin(launch_path)?;
        let workspace_dir = origin_matrix_dir.parent().ok_or_else(|| {
            format!(
                "Agent Matrix '{}' has no parent Project AC Root",
                origin_matrix_dir.display()
            )
        })?;
        ensure_workspace_is_configured(settings, workspace_dir)?;
        return Ok(ValidatedProfileAgentPath {
            launch_path,
            origin_matrix_dir,
        });
    }

    Err(format!(
        "Agent path '{}' is not an Agent Matrix or WG replica",
        launch_path.display()
    ))
}

fn agent_name_from_dir(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    name.strip_prefix("_agent_")
        .or_else(|| name.strip_prefix("__agent_"))
        .map(str::to_string)
}

fn origin_matrix_dir_for_launch_path(launch_path: &Path) -> Result<Option<PathBuf>, String> {
    let Some(dir_name) = launch_path.file_name().and_then(|name| name.to_str()) else {
        return Ok(None);
    };

    if dir_name.starts_with("_agent_") {
        return validate_authoritative_matrix_dir(launch_path).map(Some);
    }

    if !dir_name.starts_with("__agent_") {
        return Ok(None);
    }

    validated_replica_origin(launch_path).map(|(_, origin)| Some(origin))
}

fn normalize_profile_from_source(
    raw: Option<String>,
    warnings: &mut Vec<String>,
    source: &str,
) -> Option<String> {
    match raw {
        Some(value) => match normalize_profile_letter(&value) {
            Some(letter) => Some(letter),
            None => {
                warnings.push(format!(
                    "Ignoring invalid profile letter '{}' from {}",
                    value, source
                ));
                None
            }
        },
        None => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplicaProfileRead {
    pub profile: Option<String>,
    pub warning: Option<String>,
}

pub fn read_replica_profile_result(launch_path: &Path) -> ReplicaProfileRead {
    let v2 = read_tooling_string(launch_path, "profile");
    let legacy = read_tooling_string(launch_path, "instanceProfileOverride");
    let v2_normalized = v2.as_deref().and_then(normalize_profile_letter);
    let legacy_normalized = legacy.as_deref().and_then(normalize_profile_letter);
    let warning = match (&v2_normalized, &legacy_normalized) {
        (Some(v2), Some(legacy)) if v2 != legacy => Some(format!(
            "tooling.profile ({}) differs from legacy instanceProfileOverride ({}) at {}",
            v2,
            legacy,
            launch_path.display()
        )),
        _ => None,
    };
    ReplicaProfileRead {
        profile: v2_normalized.or(legacy_normalized),
        warning,
    }
}

pub fn read_instance_profile_override(launch_path: &Path) -> Option<String> {
    read_replica_profile_result(launch_path).profile
}

pub fn read_replica_profile(launch_path: &Path) -> Option<String> {
    read_replica_profile_result(launch_path).profile
}

pub fn read_replica_current_coding_agent(launch_path: &Path) -> Option<String> {
    read_tooling_string(launch_path, "currentCodingAgent")
}

/// #592 - read the loaded profile content-hash persisted at last spawn.
pub fn read_replica_profile_content_hash(launch_path: &Path) -> Option<String> {
    read_tooling_string(launch_path, "profileContentHash")
}

/// #592 - persist the loaded profile content-hash. No-op for non-replica /
/// non-matrix / non-root-agent launch roots (a normal repo has no per-agent
/// config.json and must not be littered with one). Best-effort: callers log +
/// ignore errors.
pub fn set_replica_profile_content_hash(launch_path: &Path, hash: &str) -> Result<(), String> {
    let is_agent_dir = launch_path
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.starts_with("__agent_") || name.starts_with("_agent_"))
        .unwrap_or(false);
    // #592 - the Root Agent (`ac-root-agent`) is a legitimate replica that runs a
    // coding agent and must support drift like any other; its dir name does not
    // match the `__agent_`/`_agent_` prefix, so accept it explicitly. Name-only
    // (vs path-validated `is_root_agent_path`) is sufficient here: the sole caller
    // `create_session_inner` already writes this dir's `config.json`
    // unconditionally via `set_last_coding_agent` right before this call, so the
    // gate only governs whether the hash field is added, never whether a stray
    // config.json is created. The read side is ungated, so it round-trips.
    let is_root_agent = launch_path
        .to_str()
        .map(crate::config::root_agent::is_root_agent_dir_name)
        .unwrap_or(false);
    if !is_agent_dir && !is_root_agent {
        return Ok(());
    }
    write_tooling_string(launch_path, "profileContentHash", Some(hash))
}

pub fn read_origin_default_profile(launch_path: &Path) -> Result<Option<String>, String> {
    let Some(origin) = origin_matrix_dir_for_launch_path(launch_path)? else {
        return Ok(None);
    };
    Ok(read_tooling_string(&origin, "defaultProfile")
        .and_then(|value| normalize_profile_letter(&value)))
}

pub fn set_agent_default_profile(
    settings: &AppSettings,
    launch_path: &Path,
    profile: &str,
) -> Result<(), String> {
    let profile = normalize_profile_letter(profile)
        .ok_or_else(|| "Profile must be a single letter A through Z".to_string())?;
    let validated = validate_profile_selection_agent_path(settings, launch_path)?;
    write_tooling_string(
        &validated.origin_matrix_dir,
        "defaultProfile",
        Some(&profile),
    )
}

pub fn set_instance_profile_override(
    settings: &AppSettings,
    launch_path: &Path,
    profile: Option<&str>,
) -> Result<(), String> {
    let normalized = match profile {
        Some(profile) => Some(
            normalize_profile_letter(profile)
                .ok_or_else(|| "Profile must be a single letter A through Z".to_string())?,
        ),
        None => None,
    };
    let validated = validate_profile_selection_agent_path(settings, launch_path)?;
    write_profile_to_launch_path(&validated.launch_path, normalized.as_deref())?;
    Ok(())
}

pub fn set_replica_coding_agent_selection(
    settings: &AppSettings,
    replica_path: &Path,
    coding_agent_id: &str,
    profile: &str,
) -> Result<(), String> {
    if !settings
        .agents
        .iter()
        .any(|agent| agent.id == coding_agent_id)
    {
        return Err(format!("Agent '{}' is not configured", coding_agent_id));
    }
    let profile = normalize_profile_letter(profile)
        .ok_or_else(|| "Profile must be a single letter A through Z".to_string())?;
    let validated = validate_profile_selection_agent_path(settings, replica_path)?;
    let name = validated
        .launch_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if !name.starts_with("__agent_") {
        return Err(format!(
            "Profile assignment target '{}' must be a WG replica",
            validated.launch_path.display()
        ));
    }
    let config_path = validated.launch_path.join("config.json");
    crate::config::local_config_io::update_config_json_object(&config_path, false, |obj| {
        let tooling = obj
            .entry("tooling".to_string())
            .or_insert_with(|| serde_json::json!({}));
        if !tooling.is_object() {
            *tooling = serde_json::json!({});
        }
        let tooling = tooling.as_object_mut().expect("tooling set to object");
        tooling.insert(
            "currentCodingAgent".to_string(),
            Value::String(coding_agent_id.to_string()),
        );
        tooling.insert("profile".to_string(), Value::String(profile.clone()));
        tooling.insert(
            "instanceProfileOverride".to_string(),
            Value::String(profile.clone()),
        );
        tooling.insert(
            "instanceProfileOverrideSource".to_string(),
            Value::String("manual".to_string()),
        );
        Ok(())
    })?;
    Ok(())
}

fn write_profile_to_launch_path(launch_path: &Path, profile: Option<&str>) -> Result<(), String> {
    let config_path = launch_path.join("config.json");
    crate::config::local_config_io::update_config_json_object(&config_path, true, |obj| {
        let tooling = obj
            .entry("tooling".to_string())
            .or_insert_with(|| serde_json::json!({}));
        if !tooling.is_object() {
            *tooling = serde_json::json!({});
        }
        let tooling = tooling.as_object_mut().expect("tooling set to object");
        match profile {
            Some(profile) => {
                tooling.insert("profile".to_string(), Value::String(profile.to_string()));
                tooling.insert(
                    "instanceProfileOverride".to_string(),
                    Value::String(profile.to_string()),
                );
                tooling.insert(
                    "instanceProfileOverrideSource".to_string(),
                    Value::String("manual".to_string()),
                );
            }
            None => {
                tooling.remove("profile");
                tooling.remove("instanceProfileOverride");
                tooling.remove("instanceProfileOverrideSource");
            }
        }
        Ok(())
    })?;
    Ok(())
}

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

fn cell_for_letter(
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

fn fallback_letters_from(requested: &str) -> Vec<String> {
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

    let requested_profile = instance_override
        .or(explicit)
        .or(origin_default)
        .or(agent_default)
        .unwrap_or_else(|| "A".to_string());

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::settings::{AgentConfig, ProfileCellConfig, ProfileSlotConfig};
    use std::collections::BTreeMap;
    use std::path::Path;

    fn settings_with_cells(cells: &[(&str, Vec<&str>)]) -> AppSettings {
        let mut settings = AppSettings::default();
        settings.coding_agent_profiles.profile_slots = BTreeMap::from([
            (
                "A".to_string(),
                ProfileSlotConfig {
                    label: String::new(),
                },
            ),
            (
                "B".to_string(),
                ProfileSlotConfig {
                    label: String::new(),
                },
            ),
            (
                "C".to_string(),
                ProfileSlotConfig {
                    label: String::new(),
                },
            ),
            (
                "D".to_string(),
                ProfileSlotConfig {
                    label: String::new(),
                },
            ),
        ]);
        settings.coding_agent_profiles.profiles_by_agent = cells
            .iter()
            .map(|(agent_id, letters)| {
                (
                    (*agent_id).to_string(),
                    letters
                        .iter()
                        .map(|letter| {
                            (
                                (*letter).to_string(),
                                ProfileCellConfig {
                                    enabled: true,
                                    command: format!("codex --{}", letter.to_ascii_lowercase()),
                                    env: BTreeMap::new(),
                                    notes: String::new(),
                                },
                            )
                        })
                        .collect(),
                )
            })
            .collect();
        settings
    }

    fn settings_with_project(project: &Path) -> AppSettings {
        let mut settings = settings_with_cells(&[("codex", vec!["A", "B", "C"])]);
        settings.project_paths = vec![project.to_string_lossy().to_string()];
        settings.agents = vec![AgentConfig {
            id: "codex".to_string(),
            label: "Codex".to_string(),
            command: "codex".to_string(),
            color: "#000000".to_string(),
            git_pull_before: false,
            exclude_global_claude_md: false,
            envs: Vec::new(),
            isolated_home: false,
            instructions_filename: None,
        }];
        settings
    }

    #[test]
    fn falls_back_to_nearest_lower_available_profile() {
        let settings = settings_with_cells(&[("codex", vec!["A", "C"])]);
        let resolved = resolve_profile(
            &settings,
            ProfileResolutionRequest {
                coding_agent_id: "codex",
                launch_path: None,
                agent_matrix_name: Some("dev-rust"),
                requested_profile: Some("D"),
            },
        );

        assert_eq!(resolved.requested_profile, "D");
        assert_eq!(resolved.effective_profile, "C");
        assert!(resolved.fallback_applied);
        assert_eq!(resolved.fallback_chain, vec!["D", "C"]);
        assert_eq!(resolved.cell.command, "codex --c");
    }

    #[test]
    fn synthesizes_a_cell_when_missing() {
        let settings = settings_with_cells(&[("codex", vec![])]);
        let resolved = resolve_profile(
            &settings,
            ProfileResolutionRequest {
                coding_agent_id: "codex",
                launch_path: None,
                agent_matrix_name: None,
                requested_profile: Some("B"),
            },
        );

        assert_eq!(resolved.effective_profile, "A");
        assert!(resolved.cell.command.is_empty());
    }

    #[test]
    fn instance_override_wins_over_explicit_request() {
        let temp = tempfile::tempdir().unwrap();
        let agent_dir = temp.path().join(".ac").join("_agent_dev-rust");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(
            agent_dir.join("config.json"),
            r#"{"tooling":{"instanceProfileOverride":"C"}}"#,
        )
        .unwrap();

        let settings = settings_with_cells(&[("codex", vec!["A", "B", "C"])]);
        let resolved = resolve_profile(
            &settings,
            ProfileResolutionRequest {
                coding_agent_id: "codex",
                launch_path: Some(&agent_dir),
                agent_matrix_name: None,
                requested_profile: Some("B"),
            },
        );

        assert_eq!(resolved.requested_profile, "C");
        assert_eq!(resolved.effective_profile, "C");
    }

    #[test]
    fn selection_write_dual_writes_profile_and_preserves_last_coding_agent() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let workspace = project.join(".ac");
        let matrix = workspace.join("_agent_dev-rust");
        let replica = workspace.join("wg-7-dev-team").join("__agent_dev-rust");
        std::fs::create_dir_all(&matrix).unwrap();
        std::fs::create_dir_all(&replica).unwrap();
        std::fs::write(
            replica.join("config.json"),
            r#"{"identity":"../../_agent_dev-rust","tooling":{"lastCodingAgent":"claude"}}"#,
        )
        .unwrap();
        let settings = settings_with_project(&project);

        set_replica_coding_agent_selection(&settings, &replica, "codex", "b").unwrap();

        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(replica.join("config.json")).unwrap())
                .unwrap();
        assert_eq!(saved["tooling"]["currentCodingAgent"], "codex");
        assert_eq!(saved["tooling"]["profile"], "B");
        assert_eq!(saved["tooling"]["instanceProfileOverride"], "B");
        assert_eq!(saved["tooling"]["lastCodingAgent"], "claude");
    }

    #[test]
    fn divergent_profile_and_legacy_override_returns_warning_and_prefers_v2() {
        let temp = tempfile::tempdir().unwrap();
        let agent_dir = temp.path().join("__agent_dev-rust");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(
            agent_dir.join("config.json"),
            r#"{"tooling":{"profile":"B","instanceProfileOverride":"C"}}"#,
        )
        .unwrap();

        let read = read_replica_profile_result(&agent_dir);

        assert_eq!(read.profile.as_deref(), Some("B"));
        assert!(read.warning.unwrap().contains("differs"));
    }

    #[test]
    fn replica_origin_default_is_followed_when_identity_is_valid() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("project").join(".ac");
        let matrix = workspace.join("_agent_dev-rust");
        let replica = workspace.join("wg-7-dev-team").join("__agent_dev-rust");
        std::fs::create_dir_all(&matrix).unwrap();
        std::fs::create_dir_all(&replica).unwrap();
        std::fs::write(
            matrix.join("config.json"),
            r#"{"tooling":{"defaultProfile":"B"}}"#,
        )
        .unwrap();
        std::fs::write(
            replica.join("config.json"),
            r#"{"identity":"../../_agent_dev-rust"}"#,
        )
        .unwrap();

        let settings = settings_with_cells(&[("codex", vec!["A", "B"])]);
        let resolved = resolve_profile(
            &settings,
            ProfileResolutionRequest {
                coding_agent_id: "codex",
                launch_path: Some(&replica),
                agent_matrix_name: None,
                requested_profile: None,
            },
        );

        assert_eq!(resolved.requested_profile, "B");
        assert_eq!(resolved.effective_profile, "B");
    }

    #[test]
    fn profile_selection_rejects_arbitrary_agent_dir_outside_ac_roots() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        std::fs::create_dir_all(project.join(".ac")).unwrap();
        let fake = temp.path().join("_agent_fake");
        std::fs::create_dir_all(&fake).unwrap();
        let settings = settings_with_project(&project);

        let err = validate_profile_selection_agent_path(&settings, &fake).unwrap_err();

        assert!(
            err.contains("Project AC Root") || err.contains("configured AC project roots"),
            "{err}"
        );
        assert!(!fake.join("config.json").exists());
    }

    #[test]
    fn profile_selection_accepts_real_matrix_and_replica_paths() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let workspace = project.join(".ac");
        let matrix = workspace.join("_agent_dev-rust");
        let replica = workspace.join("wg-7-dev-team").join("__agent_dev-rust");
        std::fs::create_dir_all(&matrix).unwrap();
        std::fs::create_dir_all(&replica).unwrap();
        std::fs::write(
            replica.join("config.json"),
            r#"{"identity":"../../_agent_dev-rust"}"#,
        )
        .unwrap();
        let settings = settings_with_project(&project);

        let matrix_validated = validate_profile_selection_agent_path(&settings, &matrix).unwrap();
        let replica_validated = validate_profile_selection_agent_path(&settings, &replica).unwrap();
        let matrix_canonical = canonical_existing_dir(&matrix, "Agent Matrix").unwrap();

        assert!(same_canonical_path(
            &matrix_validated.origin_matrix_dir,
            &matrix_canonical
        ));
        assert!(same_canonical_path(
            &replica_validated.origin_matrix_dir,
            &matrix_canonical
        ));
    }

    #[test]
    fn profile_selection_rejects_matrix_outside_configured_project_paths() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let other_project = temp.path().join("other");
        std::fs::create_dir_all(project.join(".ac")).unwrap();
        let fake = other_project.join(".ac").join("_agent_fake");
        std::fs::create_dir_all(&fake).unwrap();
        let settings = settings_with_project(&project);

        let err = validate_profile_selection_agent_path(&settings, &fake).unwrap_err();

        assert!(err.contains("configured AC project roots"), "{err}");
        assert!(!fake.join("config.json").exists());
    }

    // #592 - profile content-hash replica persistence.

    #[test]
    fn profile_content_hash_round_trips_and_preserves_profile() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let workspace = project.join(".ac");
        let matrix = workspace.join("_agent_dev-rust");
        let replica = workspace.join("wg-7-dev-team").join("__agent_dev-rust");
        std::fs::create_dir_all(&matrix).unwrap();
        std::fs::create_dir_all(&replica).unwrap();
        std::fs::write(
            replica.join("config.json"),
            r#"{"identity":"../../_agent_dev-rust","tooling":{"lastCodingAgent":"claude"}}"#,
        )
        .unwrap();
        let settings = settings_with_project(&project);

        // Assign a profile first (writes tooling.profile = "B").
        set_replica_coding_agent_selection(&settings, &replica, "codex", "b").unwrap();
        // Persist a content-hash.
        set_replica_profile_content_hash(&replica, "deadbeefdeadbeef").unwrap();

        assert_eq!(
            read_replica_profile_content_hash(&replica).as_deref(),
            Some("deadbeefdeadbeef")
        );
        // The profile assignment is untouched by the hash write.
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(replica.join("config.json")).unwrap())
                .unwrap();
        assert_eq!(saved["tooling"]["profile"], "B");
        assert_eq!(saved["tooling"]["currentCodingAgent"], "codex");
        assert_eq!(saved["tooling"]["profileContentHash"], "deadbeefdeadbeef");
    }

    #[test]
    fn profile_content_hash_write_is_noop_off_replica() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo-thing");
        std::fs::create_dir_all(&repo).unwrap();

        // A normal repo dir is not a replica/matrix: the write is a silent no-op
        // and must NOT create a config.json.
        assert!(set_replica_profile_content_hash(&repo, "x").is_ok());
        assert!(!repo.join("config.json").exists());
        assert_eq!(read_replica_profile_content_hash(&repo), None);
    }

    #[test]
    fn profile_content_hash_write_persists_for_root_agent_dir() {
        let temp = tempfile::tempdir().unwrap();
        // The Root Agent dir name does NOT start with `__agent_`/`_agent_`, but it
        // is a legitimate replica running a coding agent: the hash must persist
        // and round-trip so drift survives an AC restart (#592 Root Agent fix).
        let root_agent = temp.path().join(crate::config::root_agent::ROOT_AGENT_DIR_NAME);
        std::fs::create_dir_all(&root_agent).unwrap();

        set_replica_profile_content_hash(&root_agent, "cafef00dcafef00d").unwrap();

        assert_eq!(
            read_replica_profile_content_hash(&root_agent).as_deref(),
            Some("cafef00dcafef00d")
        );
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(root_agent.join("config.json")).unwrap())
                .unwrap();
        assert_eq!(saved["tooling"]["profileContentHash"], "cafef00dcafef00d");
    }
}
