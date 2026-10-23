use super::report::experiments_dir;
use super::support::err;
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
use serde::Deserialize;
use serde::Serialize;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

pub(super) fn resolve_project(project: &str) -> Result<PathBuf, Vec<CliError>> {
    workgroup::resolve_cli_project(project).map_err(|e| vec![err("project_not_found", e, None)])
}

pub(super) fn resolve_ac_root(project_path: &Path) -> Result<PathBuf, Vec<CliError>> {
    let ac_root = workgroup::resolve_cli_ac_root(project_path)
        .map_err(|e| vec![err("workspace_not_found", e, None)])?;
    reject_link_or_reparse(&ac_root, "workspace_link_or_reparse", None)?;
    std::fs::canonicalize(&ac_root).map_err(|e| {
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
    let content = std::fs::read_to_string(path)
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
    std::fs::write(path, json).map_err(|e| {
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

pub(super) fn write_attempts_jsonl_new(
    path: &Path,
    attempts: &[AttemptArtifact],
) -> Result<(), Vec<CliError>> {
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

pub(super) fn write_attempts_jsonl_replace(
    path: &Path,
    attempts: &[AttemptArtifact],
) -> Result<(), Vec<CliError>> {
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
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| {
            vec![err_at_path(
                "run_artifact_write_failed",
                format!("Failed to create {}: {}", path.display(), e),
                path,
            )]
        })?;
    file.write_all(bytes).map_err(|e| {
        vec![err_at_path(
            "run_artifact_write_failed",
            format!("Failed to write {}: {}", path.display(), e),
            path,
        )]
    })
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
    reject_link_or_reparse(path, "run_artifact_link_or_reparse", None)?;
    let parent = path.parent().ok_or_else(|| {
        vec![err_at_path(
            "run_artifact_write_failed",
            "Target path has no parent",
            path,
        )]
    })?;
    reject_link_or_reparse(parent, "run_artifact_link_or_reparse", None)?;
    fs::create_dir_all(parent).map_err(|e| {
        vec![err_at_path(
            "run_artifact_write_failed",
            format!("Failed to create {}: {}", parent.display(), e),
            parent,
        )]
    })?;
    let tmp = parent.join(format!(
        ".tmp-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(|e| {
                vec![err_at_path(
                    "run_artifact_write_failed",
                    format!("Failed to create {}: {}", tmp.display(), e),
                    &tmp,
                )]
            })?;
        file.write_all(bytes).map_err(|e| {
            vec![err_at_path(
                "run_artifact_write_failed",
                format!("Failed to write {}: {}", tmp.display(), e),
                &tmp,
            )]
        })?;
        file.flush().map_err(|e| {
            vec![err_at_path(
                "run_artifact_write_failed",
                format!("Failed to flush {}: {}", tmp.display(), e),
                &tmp,
            )]
        })?;
    }
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            fs::remove_file(path).map_err(|remove_err| {
                vec![err_at_path(
                    "run_artifact_write_failed",
                    format!("Failed to replace {}: {}", path.display(), remove_err),
                    path,
                )]
            })?;
            fs::rename(&tmp, path).map_err(|rename_err| {
                let _ = fs::remove_file(&tmp);
                vec![err_at_path(
                    "run_artifact_write_failed",
                    format!("Failed to replace {}: {}", path.display(), rename_err),
                    path,
                )]
            })
        }
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(vec![err_at_path(
                "run_artifact_write_failed",
                format!("Failed to replace {}: {}", path.display(), e),
                path,
            )])
        }
    }
}

pub(super) fn ensure_run_artifact_dirs(run_dir: &Path) -> Result<(), Vec<CliError>> {
    reject_link_or_reparse(run_dir, "run_artifact_link_or_reparse", None)?;
    for name in ["prompts", "outputs", "transcripts", "attempt-state"] {
        let dir = run_dir.join(name);
        reject_link_or_reparse(&dir, "run_artifact_link_or_reparse", None)?;
        fs::create_dir_all(&dir).map_err(|e| {
            vec![err_at_path(
                "run_artifact_write_failed",
                format!("Failed to create {}: {}", dir.display(), e),
                &dir,
            )]
        })?;
        let canonical_run = fs::canonicalize(run_dir).map_err(|e| {
            vec![err_at_path(
                "run_artifact_write_failed",
                format!("Failed to canonicalize {}: {}", run_dir.display(), e),
                run_dir,
            )]
        })?;
        let canonical_dir = fs::canonicalize(&dir).map_err(|e| {
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
    if !base.exists() {
        validate_child_path(run_dir, &base)?;
        return Ok(base);
    }
    for idx in 1..100 {
        let retry = run_dir
            .join("outputs")
            .join(format!("{}.retry-{idx:02}.txt", attempt.attempt_id));
        if !retry.exists() {
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
    let canonical_root = fs::canonicalize(root).map_err(|e| {
        vec![err_at_path(
            "run_artifact_write_failed",
            format!("Failed to canonicalize {}: {}", root.display(), e),
            root,
        )]
    })?;
    let canonical_parent = fs::canonicalize(parent).map_err(|e| {
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
    fs::create_dir(&path).map_err(|e| {
        vec![err_at_path(
            "run_artifact_write_failed",
            format!("Failed to create {}: {}", path.display(), e),
            &path,
        )]
    })?;
    fs::create_dir(path.join("messaging")).map_err(|e| {
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
        fs::create_dir(&replica).map_err(|e| {
            vec![err_at_path(
                "run_artifact_write_failed",
                format!("Failed to create {}: {}", replica.display(), e),
                &replica,
            )]
        })?;
        for child in ["memory", "plans", "skills"] {
            fs::create_dir(replica.join(child)).map_err(|e| {
                vec![err_at_path(
                    "run_artifact_write_failed",
                    format!("Failed to create replica {}: {}", child, e),
                    &replica.join(child),
                )]
            })?;
        }
        let role_path =
            variant_matrix_path(ac_root, &variant.source_agent, &variant.name).join("Role.md");
        let role_text = fs::read_to_string(&role_path).map_err(|e| {
            vec![err_at_path(
                "variant_role_missing",
                format!("Failed to read {}: {}", role_path.display(), e),
                &role_path,
            )]
        })?;
        atomic_write_bytes(&replica.join("Role.md"), role_text.as_bytes())?;
    }
    Ok(RunWorkgroupArtifact {
        name,
        path: fs::canonicalize(&path)
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
    for entry in fs::read_dir(ac_root).map_err(|e| {
        vec![err_at_path(
            "workspace_not_found",
            format!("Failed to read {}: {}", ac_root.display(), e),
            ac_root,
        )]
    })? {
        let entry = entry.map_err(|e| {
            vec![err(
                "workspace_not_found",
                format!("Failed to read Project AC Root entry: {}", e),
                None,
            )]
        })?;
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some(rest) = crate::config::entity_prefix::strip_entity_prefix(&name) {
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(n) = digits.parse::<u32>() {
                max = max.max(n);
            }
        }
    }
    Ok(max + 1)
}

pub(super) fn validate_workgroup_under_ac_root(
    ac_root: &Path,
    workgroup: &RunWorkgroupArtifact,
) -> Result<(), Vec<CliError>> {
    let ac_root = fs::canonicalize(ac_root).map_err(|e| {
        vec![err_at_path(
            "workspace_not_found",
            format!("Failed to canonicalize Project AC Root: {}", e),
            ac_root,
        )]
    })?;
    let path = PathBuf::from(&workgroup.path);
    reject_link_or_reparse(&path, "run_artifact_link_or_reparse", None)?;
    let canonical = fs::canonicalize(&path).map_err(|e| {
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
