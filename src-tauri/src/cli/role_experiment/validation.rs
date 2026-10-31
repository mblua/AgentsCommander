use super::support::err;
use super::support::line_diff;
use super::support::source_matrix_path;
use super::support::variant_agent_name;
use super::support::variant_matrix_path;
use super::support::warn;
use super::CliError;
use super::CliWarning;
use super::LoadedExperiment;
use super::VariantMetadata;
use crate::cli::session_safety::find_live_sessions_under;
use crate::cli::session_safety::LiveSessionBlocker;
use crate::commands::entity_creation::sanitize_name;
use crate::commands::entity_creation::validate_existing_name;
use crate::filesystem::projects::entities::roles;
use crate::filesystem::{directories, documents};
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

fn validate_experiment_metadata_paths(loaded: &LoadedExperiment) -> Vec<CliError> {
    let parent = &loaded.experiment_dir;
    let source = &loaded.experiment.source_agent;
    let mut errors = Vec::new();
    push_path_mismatch(
        &mut errors,
        parent,
        &loaded.experiment.source_matrix_path,
        &source_matrix_path(&loaded.ac_root, source),
        "sourceMatrixPath",
        None,
    );
    push_path_mismatch(
        &mut errors,
        parent,
        &loaded.experiment.source_role_path,
        &roles::role_path(&source_matrix_path(&loaded.ac_root, source)),
        "sourceRolePath",
        None,
    );
    errors
}

pub(super) fn validate_loaded_experiment_source_metadata(
    loaded: &LoadedExperiment,
) -> Vec<CliError> {
    let mut errors = Vec::new();
    if let Err(mut e) = validate_slug_identity(
        &loaded.experiment.source_agent,
        "Source agent",
        "metadata_path_mismatch",
    ) {
        errors.append(&mut e);
        return errors;
    }
    errors.extend(validate_experiment_metadata_paths(loaded));
    let source_matrix = source_matrix_path(&loaded.ac_root, &loaded.experiment.source_agent);
    if let Err(mut e) = reject_link_or_reparse(&source_matrix, "source_role_link_or_reparse", None)
    {
        errors.append(&mut e);
    }
    if let Err(mut e) = roles::inspect_role_path(&source_matrix)
        .map_err(|failure| map_path_failure(failure, "source_role_link_or_reparse", None))
    {
        errors.append(&mut e);
    }
    errors
}

pub(super) fn validate_variant_name_request(
    loaded: &LoadedExperiment,
    variant: &str,
) -> Result<(), Vec<CliError>> {
    validate_slug_identity(variant, "Variant", "variant_name_invalid")?;
    if !loaded
        .experiment
        .variants
        .iter()
        .any(|name| name == variant)
    {
        return Err(vec![err(
            "variant_metadata_missing",
            format!("Variant '{}' is not in experiment metadata", variant),
            Some(variant),
        )]);
    }
    Ok(())
}

pub(super) fn validate_variant_metadata_paths(
    loaded: &LoadedExperiment,
    requested_variant: &str,
    variant: &VariantMetadata,
) -> Vec<CliError> {
    let parent = loaded.experiment_dir.join("variants");
    let mut errors = Vec::new();
    if variant.name != requested_variant {
        errors.push(err(
            "metadata_path_mismatch",
            format!(
                "Variant metadata name '{}' does not match requested variant '{}'",
                variant.name, requested_variant
            ),
            Some(requested_variant),
        ));
        return errors;
    }
    let expected_agent = variant_agent_name(&loaded.experiment.source_agent, requested_variant);
    let expected_matrix = variant_matrix_path(
        &loaded.ac_root,
        &loaded.experiment.source_agent,
        requested_variant,
    );
    let expected_role = roles::role_path(&expected_matrix);
    if variant.source_agent != loaded.experiment.source_agent
        || variant.agent_name != expected_agent
    {
        errors.push(err(
            "metadata_path_mismatch",
            format!(
                "Variant '{}' metadata does not match expected source or agent name",
                variant.name
            ),
            Some(requested_variant),
        ));
    }
    push_path_mismatch(
        &mut errors,
        &parent,
        &variant.matrix_path,
        &expected_matrix,
        "matrixPath",
        Some(requested_variant),
    );
    push_path_mismatch(
        &mut errors,
        &parent,
        &variant.role_path,
        &expected_role,
        "rolePath",
        Some(requested_variant),
    );
    errors
}

pub(super) fn validate_variant_files(
    loaded: &LoadedExperiment,
    variant: &VariantMetadata,
    errors: &mut Vec<CliError>,
) {
    let matrix = variant_matrix_path(
        &loaded.ac_root,
        &loaded.experiment.source_agent,
        &variant.name,
    );
    let role = roles::role_path(&matrix);
    if let Err(mut e) = reject_link_or_reparse(
        &matrix,
        "variant_role_link_or_reparse",
        Some(variant.name.as_str()),
    ) {
        errors.append(&mut e);
    }
    if let Err(mut e) = roles::inspect_role_path(&matrix).map_err(|failure| {
        map_path_failure(
            failure,
            "variant_role_link_or_reparse",
            Some(variant.name.as_str()),
        )
    }) {
        errors.append(&mut e);
    }
    if !directories::experiment_path_is_dir(&matrix) {
        errors.push(err(
            "variant_matrix_missing",
            format!("Variant matrix is missing: {}", matrix.display()),
            Some(variant.name.as_str()),
        ));
    } else if path_key(&matrix)
        == path_key(&source_matrix_path(
            &loaded.ac_root,
            &loaded.experiment.source_agent,
        ))
    {
        errors.push(err(
            "variant_matrix_not_separate",
            "Variant matrix resolves to source matrix",
            Some(variant.name.as_str()),
        ));
    }
    if !roles::role_is_file(&matrix) {
        errors.push(err(
            "variant_role_missing",
            format!("Variant Role.md is missing: {}", role.display()),
            Some(variant.name.as_str()),
        ));
    }
}

fn push_path_mismatch(
    errors: &mut Vec<CliError>,
    metadata_parent: &Path,
    stored: &str,
    expected: &Path,
    field: &str,
    variant: Option<&str>,
) {
    let resolved = metadata_parent.join(stored);
    if reject_link_or_reparse(&resolved, "metadata_path_mismatch", variant).is_err() {
        errors.push(err(
            "metadata_path_mismatch",
            format!(
                "{} resolves through a symlink or reparse point: {}",
                field,
                resolved.display()
            ),
            variant,
        ));
        return;
    }
    if !paths_equivalent(&resolved, expected) {
        errors.push(err(
            "metadata_path_mismatch",
            format!(
                "{} resolves to {}, expected {}",
                field,
                resolved.display(),
                expected.display()
            ),
            variant,
        ));
    }
}

fn paths_equivalent(a: &Path, b: &Path) -> bool {
    path_key(&canonical_or_self(a)) == path_key(&canonical_or_self(b))
}

fn canonical_or_self(path: &Path) -> PathBuf {
    documents::canonical_experiment_role(path).unwrap_or_else(|_| normalize_lexical(path))
}

fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn path_key(path: &Path) -> String {
    let s = path.to_string_lossy().replace('/', "\\");
    if cfg!(windows) {
        s.trim_end_matches('\\').to_ascii_lowercase()
    } else {
        s.trim_end_matches('\\').to_string()
    }
}

pub(super) fn validate_slug_identity(
    value: &str,
    label: &str,
    code: &'static str,
) -> Result<(), Vec<CliError>> {
    if value.is_empty() {
        return Err(vec![err(
            code,
            format!("{} name cannot be empty", label),
            None,
        )]);
    }
    let sanitized = sanitize_name(value).map_err(|e| vec![err(code, e, None)])?;
    if sanitized != value {
        return Err(vec![err(
            code,
            format!("{} '{}' must already be a lowercase slug", label, value),
            None,
        )]);
    }
    validate_existing_name(value, label).map_err(|e| vec![err(code, e, None)])
}

pub(super) fn copy_source_skills(
    source_matrix: &Path,
    target_matrix: &Path,
    variant: &str,
    warnings: &mut Vec<CliWarning>,
) {
    let src = source_matrix.join("skills");
    if !directories::experiment_path_exists(&src) {
        return;
    }
    for link in directories::collect_experiment_link_paths(&src) {
        warnings.push(warn(
            "source_skills_copy_warning",
            format!(
                "Skipped link or reparse point under source skills: {}",
                link.display()
            ),
            Some(variant.to_string()),
        ));
    }
    let failures =
        crate::commands::role_templates::copy_dir_recursive(&src, &target_matrix.join("skills"));
    for failure in failures {
        warnings.push(warn(
            "source_skills_copy_warning",
            failure,
            Some(variant.to_string()),
        ));
    }
}

pub(super) fn reject_link_or_reparse(
    path: &Path,
    code: &'static str,
    variant: Option<&str>,
) -> Result<(), Vec<CliError>> {
    directories::inspect_experiment_path_chain(path)
        .map_err(|failure| map_path_failure(failure, code, variant))
}

pub(crate) fn map_path_failure(
    failure: directories::ExperimentPathFailure,
    code: &'static str,
    variant: Option<&str>,
) -> Vec<CliError> {
    use directories::ExperimentPathFailure;
    let message = match failure {
        ExperimentPathFailure::Link { path } => format!(
            "Path component is a symlink or reparse point: {}",
            path.display(),
        ),
        ExperimentPathFailure::Inspect { path, error } => {
            format!("Failed to inspect {}: {}", path.display(), error)
        }
    };
    vec![err(code, message, variant)]
}

pub(super) fn is_link_path(path: &Path) -> bool {
    directories::experiment_path_is_link(path)
}

pub(super) fn find_live_sessions_for_variant(
    ac_root: &Path,
    variant_agent_name: &str,
) -> Vec<LiveSessionBlocker> {
    let mut out = find_live_sessions_under(&ac_root.join(format!("_agent_{}", variant_agent_name)));
    let _ = directories::visit_experiment_entries(ac_root, |entry| {
        let Ok((path, _)) = entry else {
            return std::ops::ControlFlow::<()>::Continue(());
        };
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            return std::ops::ControlFlow::Continue(());
        };
        if crate::config::entity_prefix::has_entity_prefix(name)
            && directories::experiment_path_is_dir(&path)
        {
            let replica = path.join(format!("__agent_{}", variant_agent_name));
            if directories::experiment_path_is_dir(&replica) {
                out.extend(find_live_sessions_under(&replica));
            }
        }
        std::ops::ControlFlow::Continue(())
    });
    out
}

pub(super) fn replica_role_overrides(ac_root: &Path, variant_agent_name: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let _ = directories::visit_experiment_entries(ac_root, |entry| {
        let Ok((path, _)) = entry else {
            return std::ops::ControlFlow::<()>::Continue(());
        };
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            return std::ops::ControlFlow::Continue(());
        };
        if crate::config::entity_prefix::has_entity_prefix(name) && !name.contains("-role-exp-") {
            let role = path
                .join(format!("__agent_{}", variant_agent_name))
                .join("Role.md");
            if directories::experiment_path_exists(&role) {
                out.push(role);
            }
        }
        std::ops::ControlFlow::Continue(())
    });
    out
}

pub(super) fn is_replica_role_file(path: &Path) -> bool {
    let components: Vec<String> = path
        .components()
        .filter_map(|c| c.as_os_str().to_str().map(str::to_string))
        .collect();
    components.windows(4).any(|w| {
        w[0] == ".ac"
            && crate::config::entity_prefix::has_entity_prefix(&w[1])
            && w[2].starts_with("__agent_")
            && w[3] == "Role.md"
    })
}

pub(super) fn add_diff_warnings(
    loaded: &LoadedExperiment,
    variants: &[VariantMetadata],
    warnings: &mut Vec<CliWarning>,
) {
    let Some(control) = variants.iter().find(|v| v.name == "control") else {
        return;
    };
    let control_matrix = variant_matrix_path(
        &loaded.ac_root,
        &loaded.experiment.source_agent,
        &control.name,
    );
    let Ok(control_text) = roles::read_role(&control_matrix) else {
        return;
    };
    for variant in variants.iter().filter(|v| v.name != "control") {
        let matrix = variant_matrix_path(
            &loaded.ac_root,
            &loaded.experiment.source_agent,
            &variant.name,
        );
        let Ok(text) = roles::read_role(&matrix) else {
            continue;
        };
        let (_, stats) = line_diff("control", &variant.name, &control_text, &text);
        if stats.identical {
            warnings.push(warn(
                "variant_role_identical_to_control",
                "Variant Role.md is identical to control",
                Some(variant.name.clone()),
            ));
        } else if stats.changed_lines < 3 {
            warnings.push(warn(
                "variant_role_tiny_diff_from_control",
                "Variant differs from control by fewer than 3 changed lines",
                Some(variant.name.clone()),
            ));
        }
    }
}
