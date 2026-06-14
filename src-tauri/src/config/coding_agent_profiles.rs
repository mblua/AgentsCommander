use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::config::settings::{
    empty_profile_cell, normalize_profile_letter, AppSettings, ProfileCellConfig,
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
    std::fs::create_dir_all(agent_dir).map_err(|e| {
        format!(
            "Failed to create agent config dir '{}': {}",
            agent_dir.display(),
            e
        )
    })?;
    let config_path = agent_dir.join("config.json");
    let mut root = read_json_object(&config_path).unwrap_or_else(|| serde_json::json!({}));
    if !root.is_object() {
        root = serde_json::json!({});
    }

    let obj = root.as_object_mut().expect("root set to object");
    let tooling = obj
        .entry("tooling")
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

    let json = serde_json::to_string_pretty(&root)
        .map_err(|e| format!("Failed to serialize '{}': {}", config_path.display(), e))?;
    std::fs::write(&config_path, json)
        .map_err(|e| format!("Failed to write '{}': {}", config_path.display(), e))?;
    Ok(())
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
        return Ok(Some(launch_path.to_path_buf()));
    }

    if !dir_name.starts_with("__agent_") {
        return Ok(None);
    }

    let persisted_identity =
        read_json_object(&launch_path.join("config.json")).and_then(|config| {
            config
                .get("identity")
                .and_then(|value| value.as_str())
                .map(str::to_string)
        });
    let identity = crate::config::replica_identity::validate_or_repair_wg_replica_identity(
        launch_path,
        persisted_identity.as_deref(),
    )?;
    Ok(Some(identity.matrix_dir))
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

pub fn read_instance_profile_override(launch_path: &Path) -> Option<String> {
    read_tooling_string(launch_path, "instanceProfileOverride")
        .and_then(|value| normalize_profile_letter(&value))
}

pub fn read_origin_default_profile(launch_path: &Path) -> Result<Option<String>, String> {
    let Some(origin) = origin_matrix_dir_for_launch_path(launch_path)? else {
        return Ok(None);
    };
    Ok(read_tooling_string(&origin, "defaultProfile")
        .and_then(|value| normalize_profile_letter(&value)))
}

pub fn set_agent_default_profile(launch_path: &Path, profile: &str) -> Result<(), String> {
    let profile = normalize_profile_letter(profile)
        .ok_or_else(|| "Profile must be a single letter A through Z".to_string())?;
    let origin = origin_matrix_dir_for_launch_path(launch_path)?.ok_or_else(|| {
        format!(
            "Cannot resolve origin Matrix config for '{}'",
            launch_path.display()
        )
    })?;
    write_tooling_string(&origin, "defaultProfile", Some(&profile))
}

pub fn set_instance_profile_override(
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
    write_tooling_string(
        launch_path,
        "instanceProfileOverride",
        normalized.as_deref(),
    )?;
    if normalized.is_some() {
        write_tooling_string(launch_path, "instanceProfileOverrideSource", Some("manual"))?;
    } else {
        write_tooling_string(launch_path, "instanceProfileOverrideSource", None)?;
    }
    Ok(())
}

fn cell_for_letter(
    settings: &AppSettings,
    coding_agent_id: &str,
    letter: &str,
) -> Option<ProfileCellConfig> {
    settings
        .coding_agent_profiles
        .matrix
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
        normalize_profile_from_source(
            read_tooling_string(path, "instanceProfileOverride"),
            &mut warnings,
            "instance override",
        )
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
        .and_then(|name| settings.coding_agent_profiles.agent_defaults.get(name))
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
    use crate::config::settings::{ProfileCellConfig, ProfileLetterConfig};
    use std::collections::BTreeMap;

    fn settings_with_cells(cells: &[(&str, Vec<&str>)]) -> AppSettings {
        let mut settings = AppSettings::default();
        settings.coding_agent_profiles.letters = BTreeMap::from([
            (
                "A".to_string(),
                ProfileLetterConfig {
                    name: String::new(),
                },
            ),
            (
                "B".to_string(),
                ProfileLetterConfig {
                    name: String::new(),
                },
            ),
            (
                "C".to_string(),
                ProfileLetterConfig {
                    name: String::new(),
                },
            ),
            (
                "D".to_string(),
                ProfileLetterConfig {
                    name: String::new(),
                },
            ),
        ]);
        settings.coding_agent_profiles.matrix = cells
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
                                    argv: vec![format!("--{}", letter.to_ascii_lowercase())],
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
        assert_eq!(resolved.cell.argv, vec!["--c"]);
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
        assert!(resolved.cell.argv.is_empty());
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
}
