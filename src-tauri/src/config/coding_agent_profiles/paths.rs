use super::{read_json_object, ValidatedProfileAgentPath};
use crate::config::ac_root::{
    ensure_authoritative_ac_root, is_ac_root_name, CANONICAL_AC_ROOT_DIR,
};
use crate::config::settings::{normalize_profile_letter, AppSettings};
use std::path::{Path, PathBuf};

pub(super) fn strip_extended_prefix(path: PathBuf) -> PathBuf {
    crate::path_utils::normalize_windows_verbatim_path_buf(&path)
}

pub(super) fn canonical_existing_dir(path: &Path, label: &str) -> Result<PathBuf, String> {
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
pub(super) fn same_canonical_path(left: &Path, right: &Path) -> bool {
    left.to_string_lossy()
        .eq_ignore_ascii_case(&right.to_string_lossy())
}

#[cfg(not(windows))]
pub(super) fn same_canonical_path(left: &Path, right: &Path) -> bool {
    left == right
}

pub(super) fn collect_ac_root_candidate(out: &mut Vec<PathBuf>, candidate: &Path) {
    if !candidate.is_dir() || ensure_authoritative_ac_root(candidate).is_err() {
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

pub(crate) fn configured_ac_roots(settings: &AppSettings) -> Vec<PathBuf> {
    let mut ac_roots = Vec::new();
    for project_path in &settings.project_paths {
        let base = Path::new(project_path);
        if base
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(is_ac_root_name)
        {
            collect_ac_root_candidate(&mut ac_roots, base);
        }

        collect_ac_root_candidate(&mut ac_roots, &base.join(CANONICAL_AC_ROOT_DIR));

        let Ok(entries) = std::fs::read_dir(base) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                collect_ac_root_candidate(&mut ac_roots, &entry.path().join(CANONICAL_AC_ROOT_DIR));
            }
        }
    }
    ac_roots
}

pub(super) fn ensure_ac_root_is_configured(
    settings: &AppSettings,
    ac_root: &Path,
) -> Result<(), String> {
    let ac_root = canonical_existing_dir(ac_root, "Project AC Root")?;
    let configured = configured_ac_roots(settings);
    if configured
        .iter()
        .any(|candidate| same_canonical_path(candidate, &ac_root))
    {
        return Ok(());
    }

    Err(format!(
        "Agent path is outside configured AC project roots: {}",
        ac_root.display()
    ))
}

pub(super) fn validate_authoritative_matrix_dir(matrix_dir: &Path) -> Result<PathBuf, String> {
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
    let ac_root = matrix_dir.parent().ok_or_else(|| {
        format!(
            "Agent Matrix '{}' has no parent Project AC Root",
            matrix_dir.display()
        )
    })?;
    ensure_authoritative_ac_root(ac_root)?;
    Ok(matrix_dir)
}

pub(super) fn validated_replica_origin(replica_dir: &Path) -> Result<(PathBuf, PathBuf), String> {
    let replica_dir = canonical_existing_dir(replica_dir, "Room replica")?;
    let dir_name = replica_dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("Room replica '{}' has no valid name", replica_dir.display()))?;
    if !dir_name.starts_with("__agent_") {
        return Err(format!(
            "Room replica '{}' must be named '__agent_<name>'",
            replica_dir.display()
        ));
    }
    let wg_dir = replica_dir.parent().ok_or_else(|| {
        format!(
            "Room replica '{}' has no parent room",
            replica_dir.display()
        )
    })?;
    let wg_name = wg_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if !crate::config::entity_prefix::has_entity_prefix(wg_name) {
        return Err(format!(
            "Room replica '{}' is not inside a `room-*` or legacy `wg-*` Room directory",
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
        let ac_root = launch_path.parent().ok_or_else(|| {
            format!(
                "Agent Matrix '{}' has no parent Project AC Root",
                launch_path.display()
            )
        })?;
        ensure_ac_root_is_configured(settings, ac_root)?;
        return Ok(ValidatedProfileAgentPath {
            origin_matrix_dir: launch_path.clone(),
            launch_path,
        });
    }

    if dir_name.starts_with("__agent_") {
        let (launch_path, origin_matrix_dir) = validated_replica_origin(launch_path)?;
        let ac_root = origin_matrix_dir.parent().ok_or_else(|| {
            format!(
                "Agent Matrix '{}' has no parent Project AC Root",
                origin_matrix_dir.display()
            )
        })?;
        ensure_ac_root_is_configured(settings, ac_root)?;
        return Ok(ValidatedProfileAgentPath {
            launch_path,
            origin_matrix_dir,
        });
    }

    Err(format!(
        "Agent path '{}' is not an Agent Matrix or Room replica",
        launch_path.display()
    ))
}

pub(crate) fn agent_name_from_dir(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    name.strip_prefix("_agent_")
        .or_else(|| name.strip_prefix("__agent_"))
        .map(str::to_string)
}

pub(super) fn origin_matrix_dir_for_launch_path(
    launch_path: &Path,
) -> Result<Option<PathBuf>, String> {
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

pub(super) fn normalize_profile_from_source(
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
