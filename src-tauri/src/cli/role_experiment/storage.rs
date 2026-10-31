use super::report::experiments_dir;
use super::support::err;
use super::support::err_at_line;
use super::support::err_at_path;
use super::support::variant_matrix_path;
use super::validation::reject_link_or_reparse;
use super::validation::validate_slug_identity;
use super::AttemptArtifact;
use super::CliError;
use super::ExperimentMetadata;
use super::LoadedExperiment;
use super::PromptCase;
use super::RunWorkgroupArtifact;
use super::VariantMetadata;
use crate::cli::workgroup;
use crate::commands::entity_creation::sanitize_name;
use crate::filesystem::projects::entities::roles;
use crate::filesystem::{directories, documents};
use serde::Deserialize;
use serde::Serialize;
use std::path::Path;
use std::path::PathBuf;

pub(super) fn resolve_project(project: &str) -> Result<PathBuf, Vec<CliError>> {
    workgroup::resolve_cli_project(project).map_err(|e| vec![err("project_not_found", e, None)])
}

pub(super) fn resolve_ac_root(project_path: &Path) -> Result<PathBuf, Vec<CliError>> {
    let ac_root = workgroup::resolve_cli_ac_root(project_path)
        .map_err(|e| vec![err("workspace_not_found", e, None)])?;
    reject_link_or_reparse(&ac_root, "workspace_link_or_reparse", None)?;
    documents::canonical_experiment_role(&ac_root).map_err(|e| {
        vec![err(
            "workspace_not_found",
            format!("Failed to canonicalize Project AC Root: {}", e),
            None,
        )]
    })
}

pub(super) fn load_experiment(
    project: &str,
    experiment: &str,
) -> Result<LoadedExperiment, Vec<CliError>> {
    validate_slug_identity(experiment, "Experiment", "experiment_name_invalid")?;
    let project_path = resolve_project(project)?;
    let ac_root = resolve_ac_root(&project_path)?;
    let experiment_dir = experiments_dir(&ac_root).join(experiment);
    let metadata_path = experiment_dir.join("experiment.json");
    reject_link_or_reparse(&experiment_dir, "experiment_metadata_link_or_reparse", None)?;
    reject_link_or_reparse(&metadata_path, "experiment_metadata_link_or_reparse", None)?;
    let experiment = read_json::<ExperimentMetadata>(&metadata_path)
        .map_err(|e| vec![err("experiment_metadata_missing", e, None)])?;
    Ok(LoadedExperiment {
        ac_root,
        experiment_dir,
        experiment,
    })
}

pub(super) fn load_variant_metadata(
    loaded: &LoadedExperiment,
    variant: &str,
) -> Result<VariantMetadata, String> {
    let path = loaded
        .experiment_dir
        .join("variants")
        .join(format!("{}.json", variant));
    reject_link_or_reparse(&path, "variant_metadata_link_or_reparse", Some(variant)).map_err(
        |errors| {
            errors
                .into_iter()
                .map(|e| e.message)
                .collect::<Vec<_>>()
                .join("; ")
        },
    )?;
    read_json(&path)
}

pub(super) fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    let content = documents::read_experiment_document(path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
    serde_json::from_str(&content).map_err(|e| format!("Failed to parse {}: {}", path.display(), e))
}

pub(super) fn write_json<T: Serialize>(
    path: &Path,
    value: &T,
    code: &'static str,
    variant: Option<&str>,
) -> Result<(), Vec<CliError>> {
    let mut json = serde_json::to_string_pretty(value).map_err(|e| {
        vec![err(
            code,
            format!("Failed to serialize JSON for {}: {}", path.display(), e),
            variant,
        )]
    })?;
    json.push('\n');
    documents::write_experiment_document(path, json.as_bytes()).map_err(|e| {
        vec![err(
            code,
            format!("Failed to write {}: {}", path.display(), e),
            variant,
        )]
    })
}

pub(super) fn write_json_new<T: Serialize>(path: &Path, value: &T) -> Result<(), Vec<CliError>> {
    let mut json = serde_json::to_string_pretty(value).map_err(|e| {
        vec![err_at_path(
            "run_artifact_write_failed",
            format!("Failed to serialize JSON for {}: {}", path.display(), e),
            path,
        )]
    })?;
    json.push('\n');
    write_new_file(path, json.as_bytes())
}

const ATTEMPTS_FILE_NAME: &str = "attempts.jsonl";

pub(super) fn attempts_name() -> String {
    ATTEMPTS_FILE_NAME.to_string()
}

pub(super) fn attempts_path(run_dir: &Path) -> PathBuf {
    run_dir.join(ATTEMPTS_FILE_NAME)
}

pub(super) fn attempts_path_display(run_dir: &Path) -> String {
    attempts_path(run_dir).to_string_lossy().to_string()
}

pub(super) fn load_attempts(run_dir: &Path) -> Result<Vec<AttemptArtifact>, Vec<CliError>> {
    read_attempts_jsonl(&attempts_path(run_dir))
}

fn read_attempts_jsonl(path: &Path) -> Result<Vec<AttemptArtifact>, Vec<CliError>> {
    let content = documents::read_experiment_document(path).map_err(|e| {
        vec![err_at_path(
            "run_artifact_unparseable",
            format!("Failed to read {}: {}", path.display(), e),
            path,
        )]
    })?;
    let mut attempts = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        attempts.push(serde_json::from_str(line).map_err(|e| {
            vec![err_at_line(
                "run_artifact_unparseable",
                format!("Attempt line {} is invalid JSON: {}", idx + 1, e),
                idx + 1,
                None,
            )]
        })?);
    }
    Ok(attempts)
}

pub(super) fn create_attempts(
    run_dir: &Path,
    attempts: &[AttemptArtifact],
) -> Result<(), Vec<CliError>> {
    let path = &attempts_path(run_dir);
    let mut content = String::new();
    for attempt in attempts {
        let line = serde_json::to_string(attempt).map_err(|e| {
            vec![err_at_path(
                "run_artifact_write_failed",
                format!(
                    "Failed to serialize attempt JSON for {}: {}",
                    path.display(),
                    e
                ),
                path,
            )]
        })?;
        content.push_str(&line);
        content.push('\n');
    }
    write_new_file(path, content.as_bytes())
}

pub(super) fn replace_attempts(
    run_dir: &Path,
    attempts: &[AttemptArtifact],
) -> Result<(), Vec<CliError>> {
    let path = &attempts_path(run_dir);
    let mut content = String::new();
    for attempt in attempts {
        let line = serde_json::to_string(attempt).map_err(|e| {
            vec![err_at_path(
                "run_artifact_write_failed",
                format!(
                    "Failed to serialize attempt JSON for {}: {}",
                    path.display(),
                    e
                ),
                path,
            )]
        })?;
        content.push_str(&line);
        content.push('\n');
    }
    atomic_write_bytes(path, content.as_bytes())
}

pub(super) fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), Vec<CliError>> {
    documents::create_experiment_artifact(path, bytes).map_err(map_artifact_failure)
}

pub(super) fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), Vec<CliError>> {
    let mut json = serde_json::to_string_pretty(value).map_err(|e| {
        vec![err_at_path(
            "run_artifact_write_failed",
            format!("Failed to serialize JSON for {}: {}", path.display(), e),
            path,
        )]
    })?;
    json.push('\n');
    atomic_write_bytes(path, json.as_bytes())
}

pub(super) fn atomic_write_bytes(path: &Path, bytes: &[u8]) -> Result<(), Vec<CliError>> {
    documents::replace_experiment_artifact(path, bytes).map_err(map_artifact_failure)
}

pub(crate) fn map_artifact_failure(failure: documents::ExperimentArtifactFailure) -> Vec<CliError> {
    use documents::ExperimentArtifactFailure as Failure;
    let (action, path, error) = match failure {
        Failure::PathInspection(failure) => {
            return super::validation::map_path_failure(
                failure,
                "run_artifact_link_or_reparse",
                None,
            );
        }
        Failure::NoParent { path } => {
            return vec![err_at_path(
                "run_artifact_write_failed",
                "Target path has no parent",
                &path,
            )]
        }
        Failure::CreateDirectory { path, error } | Failure::Create { path, error } => {
            ("create", path, error)
        }
        Failure::Write { path, error } => ("write", path, error),
        Failure::Flush { path, error } => ("flush", path, error),
        Failure::Replace { path, error } => ("replace", path, error),
    };
    vec![err_at_path(
        "run_artifact_write_failed",
        format!("Failed to {} {}: {}", action, path.display(), error),
        &path,
    )]
}

pub(super) fn ensure_run_artifact_dirs(run_dir: &Path) -> Result<(), Vec<CliError>> {
    reject_link_or_reparse(run_dir, "run_artifact_link_or_reparse", None)?;
    for name in ["prompts", "outputs", "transcripts", "attempt-state"] {
        let dir = run_dir.join(name);
        reject_link_or_reparse(&dir, "run_artifact_link_or_reparse", None)?;
        directories::ensure_experiment_directory(&dir).map_err(|e| {
            vec![err_at_path(
                "run_artifact_write_failed",
                format!("Failed to create {}: {}", dir.display(), e),
                &dir,
            )]
        })?;
        let canonical_run = documents::canonical_experiment_role(run_dir).map_err(|e| {
            vec![err_at_path(
                "run_artifact_write_failed",
                format!("Failed to canonicalize {}: {}", run_dir.display(), e),
                run_dir,
            )]
        })?;
        let canonical_dir = documents::canonical_experiment_role(&dir).map_err(|e| {
            vec![err_at_path(
                "run_artifact_write_failed",
                format!("Failed to canonicalize {}: {}", dir.display(), e),
                &dir,
            )]
        })?;
        if !canonical_dir.starts_with(&canonical_run) {
            return Err(vec![err_at_path(
                "run_artifact_mismatch",
                format!("Artifact directory is outside run dir: {}", dir.display()),
                &dir,
            )]);
        }
    }
    Ok(())
}

pub(super) fn write_attempt_prompt_file(
    run_dir: &Path,
    attempt: &AttemptArtifact,
    prompt: &PromptCase,
) -> Result<PathBuf, Vec<CliError>> {
    ensure_run_artifact_dirs(run_dir)?;
    let path = run_dir
        .join("prompts")
        .join(format!("{}.md", attempt.attempt_id));
    validate_child_path(run_dir, &path)?;
    let content = format!(
        "# Role Experiment Attempt\n\nRun: {}\nAttempt: {}\nVariant: {}\nPrompt: {}\n\
         Replicate: {}\n\n## Runner Instructions\n\nRespond to the prompt below. \
         Do not modify files outside paths allowed by your session instructions.\n\n\
         ## Prompt\n\n{}\n",
        attempt.run_id,
        attempt.attempt_id,
        attempt.variant,
        attempt.prompt_id,
        attempt.replicate,
        prompt.prompt
    );
    atomic_write_bytes(&path, content.as_bytes())?;
    Ok(path)
}

pub(super) fn next_attempt_output_path(
    run_dir: &Path,
    attempt: &AttemptArtifact,
) -> Result<PathBuf, Vec<CliError>> {
    let base = run_dir
        .join("outputs")
        .join(format!("{}.txt", attempt.attempt_id));
    if !directories::experiment_path_exists(&base) {
        validate_child_path(run_dir, &base)?;
        return Ok(base);
    }
    for idx in 1..100 {
        let retry = run_dir
            .join("outputs")
            .join(format!("{}.retry-{idx:02}.txt", attempt.attempt_id));
        if !directories::experiment_path_exists(&retry) {
            validate_child_path(run_dir, &retry)?;
            return Ok(retry);
        }
    }
    Err(vec![err(
        "run_artifact_write_failed",
        "Too many retry output files for attempt",
        None,
    )])
}

fn validate_child_path(root: &Path, child: &Path) -> Result<(), Vec<CliError>> {
    reject_link_or_reparse(child, "run_artifact_link_or_reparse", None)?;
    let parent = child.parent().unwrap_or(root);
    let canonical_root = documents::canonical_experiment_role(root).map_err(|e| {
        vec![err_at_path(
            "run_artifact_write_failed",
            format!("Failed to canonicalize {}: {}", root.display(), e),
            root,
        )]
    })?;
    let canonical_parent = documents::canonical_experiment_role(parent).map_err(|e| {
        vec![err_at_path(
            "run_artifact_write_failed",
            format!("Failed to canonicalize {}: {}", parent.display(), e),
            parent,
        )]
    })?;
    if canonical_parent.starts_with(&canonical_root) {
        Ok(())
    } else {
        Err(vec![err_at_path(
            "run_artifact_mismatch",
            format!("Path is outside run dir: {}", child.display()),
            child,
        )])
    }
}

pub(super) fn create_run_workgroup_dirs(
    ac_root: &Path,
    experiment: &str,
    run_id: &str,
    variants: &[VariantMetadata],
) -> Result<RunWorkgroupArtifact, Vec<CliError>> {
    reject_link_or_reparse(ac_root, "workspace_link_or_reparse", None)?;
    let sanitized_experiment = sanitize_name(experiment).map_err(|e| {
        vec![err(
            "experiment_name_invalid",
            format!("Experiment name cannot be used for room: {}", e),
            None,
        )]
    })?;
    let next = next_workgroup_number(ac_root)?;
    let name = role_exp_room_dir_name(
        next,
        &sanitized_experiment,
        crate::commands::entity_creation::room_number_pad_width(),
    );
    let path = ac_root.join(&name);
    reject_link_or_reparse(&path, "run_artifact_link_or_reparse", None)?;
    directories::create_experiment_directory(&path).map_err(|e| {
        vec![err_at_path(
            "run_artifact_write_failed",
            format!("Failed to create {}: {}", path.display(), e),
            &path,
        )]
    })?;
    directories::create_experiment_directory(&path.join("messaging")).map_err(|e| {
        vec![err_at_path(
            "run_artifact_write_failed",
            format!("Failed to create messaging dir: {}", e),
            &path.join("messaging"),
        )]
    })?;
    atomic_write_bytes(
        &path.join("TASK.md"),
        format!(
            "# Role Experiment Run\n\nExperiment: {}\nRun: {}\n",
            experiment, run_id
        )
        .as_bytes(),
    )?;
    for variant in variants {
        let replica = path.join(format!("__agent_{}", variant.agent_name));
        reject_link_or_reparse(
            &replica,
            "run_artifact_link_or_reparse",
            Some(&variant.name),
        )?;
        directories::create_experiment_directory(&replica).map_err(|e| {
            vec![err_at_path(
                "run_artifact_write_failed",
                format!("Failed to create {}: {}", replica.display(), e),
                &replica,
            )]
        })?;
        for child in ["memory", "plans", "skills"] {
            directories::create_experiment_directory(&replica.join(child)).map_err(|e| {
                vec![err_at_path(
                    "run_artifact_write_failed",
                    format!("Failed to create replica {}: {}", child, e),
                    &replica.join(child),
                )]
            })?;
        }
        let matrix = variant_matrix_path(ac_root, &variant.source_agent, &variant.name);
        let role_path = roles::role_path(&matrix);
        let role_text = roles::read_role(&matrix).map_err(|e| {
            vec![err_at_path(
                "variant_role_missing",
                format!("Failed to read {}: {}", role_path.display(), e),
                &role_path,
            )]
        })?;
        crate::filesystem::experiment_replica_role::write(&replica, role_text.as_bytes())
            .map_err(map_artifact_failure)?;
    }
    Ok(RunWorkgroupArtifact {
        name,
        path: documents::canonical_experiment_role(&path)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string(),
        retained: true,
        cleanup_supported: false,
        run_id: run_id.to_string(),
        experiment: experiment.to_string(),
    })
}

/// #2507 Room directory name for the CLI role-experiment path.
pub(super) fn role_exp_room_dir_name(
    next: u32,
    sanitized_experiment: &str,
    width: usize,
) -> String {
    crate::commands::entity_creation::room_dir_name(
        next,
        &format!("role-exp-{sanitized_experiment}"),
        width,
    )
}

fn next_workgroup_number(ac_root: &Path) -> Result<u32, Vec<CliError>> {
    let mut max = 0;
    let visit = directories::visit_experiment_entries(ac_root, |entry| {
        let (_, name) = match entry {
            Ok(entry) => entry,
            Err(e) => {
                return std::ops::ControlFlow::Break(vec![err(
                    "workspace_not_found",
                    format!("Failed to read Project AC Root entry: {}", e),
                    None,
                )])
            }
        };
        let name = name.to_string_lossy().to_string();
        if let Some(rest) = crate::config::entity_prefix::strip_entity_prefix(&name) {
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(n) = digits.parse::<u32>() {
                max = max.max(n);
            }
        }
        std::ops::ControlFlow::Continue(())
    })
    .map_err(|e| {
        vec![err_at_path(
            "workspace_not_found",
            format!("Failed to read {}: {}", ac_root.display(), e),
            ac_root,
        )]
    })?;
    if let std::ops::ControlFlow::Break(errors) = visit {
        return Err(errors);
    }
    Ok(max + 1)
}

pub(super) fn validate_workgroup_under_ac_root(
    ac_root: &Path,
    workgroup: &RunWorkgroupArtifact,
) -> Result<(), Vec<CliError>> {
    let ac_root = documents::canonical_experiment_role(ac_root).map_err(|e| {
        vec![err_at_path(
            "workspace_not_found",
            format!("Failed to canonicalize Project AC Root: {}", e),
            ac_root,
        )]
    })?;
    let path = PathBuf::from(&workgroup.path);
    reject_link_or_reparse(&path, "run_artifact_link_or_reparse", None)?;
    let canonical = documents::canonical_experiment_role(&path).map_err(|e| {
        vec![err_at_path(
            "run_artifact_mismatch",
            format!("Failed to canonicalize room: {}", e),
            &path,
        )]
    })?;
    if canonical.starts_with(ac_root)
        && crate::config::entity_prefix::has_entity_prefix(&workgroup.name)
    {
        Ok(())
    } else {
        Err(vec![err(
            "run_artifact_mismatch",
            "Run room is not under the current Project AC Root",
            None,
        )])
    }
}
