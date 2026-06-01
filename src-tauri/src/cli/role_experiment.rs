use std::collections::{BTreeSet, HashSet};
use std::path::{Component, Path, PathBuf};

use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::cli::session_safety::{find_live_sessions_under, LiveSessionBlocker};
use crate::cli::workgroup;
use crate::commands::entity_creation::{
    agent_ref_bare_name, apply_agent_matrix_settings_files, create_agent_matrix_from_role,
    resolve_agent_ref, sanitize_name, validate_existing_name, AgentMatrixSettingsFlags,
    CreateAgentMatrixFromRoleArgs,
};

#[derive(Args)]
pub struct RoleExperimentArgs {
    #[command(subcommand)]
    command: RoleExperimentCommand,
}

#[derive(Subcommand)]
enum RoleExperimentCommand {
    Init(InitArgs),
    List(ProjectArgs),
    Show(ExperimentArgs),
    Variant(VariantArgs),
    Validate(ValidateArgs),
}

#[derive(Args)]
struct ProjectArgs {
    #[arg(long)]
    project: String,
}

#[derive(Args)]
struct ExperimentArgs {
    #[arg(long)]
    project: String,
    #[arg(long)]
    experiment: String,
}

#[derive(Args)]
struct InitArgs {
    #[arg(long)]
    project: String,
    #[arg(long)]
    name: String,
    #[arg(long = "source-agent")]
    source_agent: String,
    #[arg(long, value_delimiter = ',')]
    variants: Vec<String>,
}

#[derive(Args)]
struct ValidateArgs {
    #[arg(long)]
    project: String,
    #[arg(long)]
    experiment: String,
    #[arg(long = "prompt-suite")]
    prompt_suite: Option<String>,
}

#[derive(Args)]
struct VariantArgs {
    #[command(subcommand)]
    command: VariantCommand,
}

#[derive(Subcommand)]
enum VariantCommand {
    Set(VariantSetArgs),
    Diff(VariantDiffArgs),
}

#[derive(Args)]
struct VariantSetArgs {
    #[arg(long)]
    project: String,
    #[arg(long)]
    experiment: String,
    #[arg(long)]
    variant: String,
    #[arg(long = "role-file")]
    role_file: String,
}

#[derive(Args)]
struct VariantDiffArgs {
    #[arg(long)]
    project: String,
    #[arg(long)]
    experiment: String,
    #[arg(long)]
    variant: String,
    #[arg(long = "against", default_value = "control")]
    against: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CliEnvelope {
    ok: bool,
    command: &'static str,
    data: Option<serde_json::Value>,
    warnings: Vec<CliWarning>,
    errors: Vec<CliError>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CliWarning {
    code: String,
    message: String,
    variant: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CliError {
    code: String,
    message: String,
    variant: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExperimentMetadata {
    schema_version: u32,
    name: String,
    project: String,
    source_agent: String,
    source_matrix_path: String,
    source_role_path: String,
    source_role_sha256: String,
    created_at: String,
    updated_at: String,
    variants: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VariantMetadata {
    schema_version: u32,
    name: String,
    source_agent: String,
    agent_name: String,
    matrix_path: String,
    role_path: String,
    role_sha256: String,
    created_at: String,
    updated_at: String,
}

struct CommandOutput {
    data: serde_json::Value,
    warnings: Vec<CliWarning>,
    errors: Vec<CliError>,
}

struct LoadedExperiment {
    workspace_dir: PathBuf,
    experiment_dir: PathBuf,
    experiment: ExperimentMetadata,
}

#[derive(Debug, Clone)]
struct DiffStats {
    added_lines: usize,
    removed_lines: usize,
    changed_lines: usize,
    identical: bool,
}

pub fn execute(args: RoleExperimentArgs) -> i32 {
    let (command, result) = match args.command {
        RoleExperimentCommand::Init(args) => ("role-experiment init", init(args)),
        RoleExperimentCommand::List(args) => ("role-experiment list", list(args)),
        RoleExperimentCommand::Show(args) => ("role-experiment show", show(args)),
        RoleExperimentCommand::Variant(args) => match args.command {
            VariantCommand::Set(args) => ("role-experiment variant set", variant_set(args)),
            VariantCommand::Diff(args) => ("role-experiment variant diff", variant_diff(args)),
        },
        RoleExperimentCommand::Validate(args) => ("role-experiment validate", validate(args)),
    };

    let envelope = match result {
        Ok(out) => CliEnvelope {
            ok: out.errors.is_empty(),
            command,
            data: if out.errors.is_empty() {
                Some(out.data)
            } else {
                None
            },
            warnings: out.warnings,
            errors: out.errors,
        },
        Err(errors) => CliEnvelope {
            ok: false,
            command,
            data: None,
            warnings: Vec::new(),
            errors,
        },
    };

    let code = if envelope.ok { 0 } else { 1 };
    match serde_json::to_string_pretty(&envelope) {
        Ok(json) => crate::cli_println!("{}", json),
        Err(e) => eprintln!("Error: failed to serialize role-experiment output: {}", e),
    }
    code
}

fn init(args: InitArgs) -> Result<CommandOutput, Vec<CliError>> {
    validate_slug_identity(&args.name, "Experiment", "experiment_name_invalid")?;
    if args.variants.len() < 2 {
        return Err(vec![err(
            "variant_count_invalid",
            "--variants must contain at least two entries",
            None,
        )]);
    }

    let project_path = resolve_project(&args.project)?;
    let workspace_dir = resolve_workspace(&project_path)?;
    reject_link_or_reparse(&workspace_dir, "workspace_link_or_reparse", None)?;

    let source_ref = map_err(
        resolve_agent_ref(&workspace_dir, &args.source_agent),
        "source_agent_missing",
        None,
    )?;
    let source_agent = agent_ref_bare_name(&source_ref);
    validate_slug_identity(&source_agent, "Source agent", "source_agent_invalid")?;
    let source_matrix = source_matrix_path(&workspace_dir, &source_agent);
    let source_role = source_matrix.join("Role.md");
    reject_link_or_reparse(&source_matrix, "source_role_link_or_reparse", None)?;
    reject_link_or_reparse(&source_role, "source_role_link_or_reparse", None)?;
    if !source_role.is_file() {
        return Err(vec![err(
            "source_role_missing",
            format!("Source Role.md not found at {}", source_role.display()),
            None,
        )]);
    }

    let mut seen = HashSet::new();
    let mut variants = Vec::new();
    let mut preflight_errors = Vec::new();
    for variant in &args.variants {
        if variant.is_empty() {
            preflight_errors.push(err(
                "variant_name_empty",
                "Variant name cannot be empty",
                None,
            ));
            continue;
        }
        if let Err(mut e) = validate_slug_identity(variant, "Variant", "variant_name_invalid") {
            preflight_errors.append(&mut e);
            continue;
        }
        if !seen.insert(variant.clone()) {
            preflight_errors.push(err(
                "variant_name_duplicate",
                format!("Variant '{}' is duplicated", variant),
                Some(variant.as_str()),
            ));
            continue;
        }
        let agent_name = variant_agent_name(&source_agent, variant);
        if let Err(e) = validate_existing_name(&agent_name, "Agent") {
            preflight_errors.push(err("variant_name_invalid", e, Some(variant.as_str())));
            continue;
        }
        let target = variant_matrix_path(&workspace_dir, &source_agent, variant);
        if target.exists() {
            if is_link_path(&target) {
                preflight_errors.push(err(
                    "variant_role_link_or_reparse",
                    format!(
                        "Variant matrix path is a link or reparse point: {}",
                        target.display()
                    ),
                    Some(variant.as_str()),
                ));
            } else {
                preflight_errors.push(err(
                    "variant_matrix_exists",
                    format!("Variant matrix already exists: {}", target.display()),
                    Some(variant.as_str()),
                ));
            }
        }
        variants.push(variant.clone());
    }

    let experiment_dir = experiments_dir(&workspace_dir).join(&args.name);
    if experiment_dir.exists() {
        preflight_errors.push(err(
            "experiment_exists",
            format!("Experiment '{}' already exists", args.name),
            None,
        ));
    }
    if !preflight_errors.is_empty() {
        return Err(preflight_errors);
    }

    let role_bytes = map_err(std::fs::read(&source_role), "source_role_read_failed", None)?;
    if std::str::from_utf8(&role_bytes).is_err() {
        return Err(vec![err(
            "source_role_not_utf8",
            "Source Role.md must be valid UTF-8",
            None,
        )]);
    }
    let role_sha = sha256_hex(&role_bytes);
    let now = chrono::Utc::now().to_rfc3339();
    let settings = crate::config::settings::load_settings_for_cli();
    let flags = AgentMatrixSettingsFlags::from_settings(&settings);

    map_err(
        std::fs::create_dir_all(experiment_dir.join("variants")),
        "experiment_metadata_write_failed",
        None,
    )?;
    map_err(
        std::fs::create_dir_all(experiment_dir.join("runs")),
        "experiment_metadata_write_failed",
        None,
    )?;

    let mut result_variants = Vec::new();
    let mut warnings = Vec::new();
    for variant in &variants {
        let agent_name = variant_agent_name(&source_agent, variant);
        let created = create_agent_matrix_from_role(CreateAgentMatrixFromRoleArgs {
            workspace_dir: &workspace_dir,
            safe_name: &agent_name,
            role_bytes: &role_bytes,
        })
        .map_err(|e| {
            vec![err(
                "variant_matrix_create_failed",
                e,
                Some(variant.as_str()),
            )]
        })?;

        copy_source_skills(&source_matrix, &created.agent_dir, variant, &mut warnings);
        for warning in apply_agent_matrix_settings_files(&created.agent_dir, flags) {
            warnings.push(warn(
                "variant_settings_warning",
                warning,
                Some(variant.to_string()),
            ));
        }

        let metadata = VariantMetadata {
            schema_version: 1,
            name: variant.clone(),
            source_agent: source_agent.clone(),
            agent_name: agent_name.clone(),
            matrix_path: format!("../../../_agent_{}", agent_name),
            role_path: format!("../../../_agent_{}/Role.md", agent_name),
            role_sha256: role_sha.clone(),
            created_at: now.clone(),
            updated_at: now.clone(),
        };
        write_json(
            &experiment_dir
                .join("variants")
                .join(format!("{}.json", variant)),
            &metadata,
            "variant_metadata_write_failed",
            Some(variant.as_str()),
        )?;
        workgroup::write_refresh(
            &project_path,
            &created.agent_dir,
            &created.display_name,
            "roleExperimentVariantCreated",
        );
        result_variants.push(serde_json::json!({
            "name": variant,
            "agentName": agent_name,
            "matrixPath": created.agent_dir.to_string_lossy(),
            "roleSha256": role_sha,
        }));
    }

    let experiment = ExperimentMetadata {
        schema_version: 1,
        name: args.name.clone(),
        project: args.project.clone(),
        source_agent: source_agent.clone(),
        source_matrix_path: format!("../../_agent_{}", source_agent),
        source_role_path: format!("../../_agent_{}/Role.md", source_agent),
        source_role_sha256: role_sha.clone(),
        created_at: now.clone(),
        updated_at: now,
        variants,
    };
    write_json(
        &experiment_dir.join("experiment.json"),
        &experiment,
        "experiment_metadata_write_failed",
        None,
    )?;

    Ok(CommandOutput {
        data: serde_json::json!({
            "experiment": args.name,
            "path": experiment_dir.to_string_lossy(),
            "sourceAgent": source_agent,
            "variants": result_variants,
        }),
        warnings,
        errors: Vec::new(),
    })
}

fn list(args: ProjectArgs) -> Result<CommandOutput, Vec<CliError>> {
    let project_path = resolve_project(&args.project)?;
    let workspace_dir = resolve_workspace(&project_path)?;
    let root = experiments_dir(&workspace_dir);
    let mut warnings = Vec::new();
    let mut experiments = Vec::new();
    if root.is_dir() {
        let entries = map_err(std::fs::read_dir(&root), "experiment_list_failed", None)?;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            match read_json::<ExperimentMetadata>(&path.join("experiment.json")) {
                Ok(meta) => experiments.push(serde_json::json!({
                    "name": meta.name,
                    "sourceAgent": meta.source_agent,
                    "variantCount": meta.variants.len(),
                    "path": path.to_string_lossy(),
                })),
                Err(e) => warnings.push(warn(
                    "experiment_metadata_unreadable",
                    e,
                    entry.file_name().to_str().map(str::to_string),
                )),
            }
        }
    }
    Ok(CommandOutput {
        data: serde_json::json!({ "experiments": experiments }),
        warnings,
        errors: Vec::new(),
    })
}

fn show(args: ExperimentArgs) -> Result<CommandOutput, Vec<CliError>> {
    let loaded = load_experiment(&args.project, &args.experiment)?;
    let mut errors = validate_loaded_experiment_source_metadata(&loaded);
    let mut variants = Vec::new();
    if errors.is_empty() {
        for variant in &loaded.experiment.variants {
            if let Err(mut e) = validate_variant_name_request(&loaded, variant) {
                errors.append(&mut e);
                continue;
            }
            match load_variant_metadata(&loaded, variant) {
                Ok(meta) => {
                    errors.extend(validate_variant_metadata_paths(&loaded, variant, &meta));
                    variants.push(serde_json::to_value(meta).unwrap_or(serde_json::Value::Null));
                }
                Err(e) => errors.push(err("variant_metadata_missing", e, Some(variant.as_str()))),
            }
        }
    }
    Ok(CommandOutput {
        data: serde_json::json!({
            "experiment": loaded.experiment,
            "variants": variants,
        }),
        warnings: Vec::new(),
        errors,
    })
}

fn variant_set(args: VariantSetArgs) -> Result<CommandOutput, Vec<CliError>> {
    let loaded = load_experiment(&args.project, &args.experiment)?;
    let errors = validate_loaded_experiment_source_metadata(&loaded);
    if !errors.is_empty() {
        return Err(errors);
    }
    validate_variant_name_request(&loaded, &args.variant)?;
    let mut variant = load_variant_metadata(&loaded, &args.variant).map_err(|e| {
        vec![err(
            "variant_metadata_missing",
            e,
            Some(args.variant.as_str()),
        )]
    })?;
    let errors = validate_variant_metadata_paths(&loaded, &args.variant, &variant);
    if !errors.is_empty() {
        return Err(errors);
    }

    let role_input = PathBuf::from(&args.role_file);
    reject_link_or_reparse(
        &role_input,
        "role_file_link_or_reparse",
        Some(args.variant.as_str()),
    )?;
    let role_input = map_err(
        std::fs::canonicalize(&role_input),
        "role_file_missing",
        Some(args.variant.as_str()),
    )?;
    if is_replica_role_file(&role_input) {
        return Err(vec![err(
            "replica_role_file_not_allowed",
            "Replica-local Role.md files cannot be used as variant source",
            Some(args.variant.as_str()),
        )]);
    }
    let meta = map_err(
        std::fs::metadata(&role_input),
        "role_file_missing",
        Some(args.variant.as_str()),
    )?;
    if !meta.is_file() {
        return Err(vec![err(
            "role_file_missing",
            format!("Role file is not a regular file: {}", role_input.display()),
            Some(args.variant.as_str()),
        )]);
    }

    let blockers = find_live_sessions_for_variant(&loaded.workspace_dir, &variant.agent_name);
    if !blockers.is_empty() {
        return Err(vec![err(
            "active_variant_session",
            format!("Variant '{}' has active sessions", args.variant),
            Some(args.variant.as_str()),
        )]);
    }

    let expected_matrix = variant_matrix_path(
        &loaded.workspace_dir,
        &loaded.experiment.source_agent,
        &args.variant,
    );
    let expected_role = expected_matrix.join("Role.md");
    reject_link_or_reparse(
        &expected_matrix,
        "variant_role_link_or_reparse",
        Some(args.variant.as_str()),
    )?;
    reject_link_or_reparse(
        &expected_role,
        "variant_role_link_or_reparse",
        Some(args.variant.as_str()),
    )?;

    let bytes = map_err(
        std::fs::read(&role_input),
        "role_file_read_failed",
        Some(args.variant.as_str()),
    )?;
    if std::str::from_utf8(&bytes).is_err() {
        return Err(vec![err(
            "role_file_not_utf8",
            "Role file must be valid UTF-8",
            Some(args.variant.as_str()),
        )]);
    }
    map_err(
        std::fs::write(&expected_role, &bytes),
        "variant_role_write_failed",
        Some(args.variant.as_str()),
    )?;

    let now = chrono::Utc::now().to_rfc3339();
    variant.role_sha256 = sha256_hex(&bytes);
    variant.updated_at = now.clone();
    write_json(
        &loaded
            .experiment_dir
            .join("variants")
            .join(format!("{}.json", args.variant)),
        &variant,
        "variant_metadata_write_failed",
        Some(args.variant.as_str()),
    )?;
    let mut experiment = loaded.experiment.clone();
    experiment.updated_at = now;
    write_json(
        &loaded.experiment_dir.join("experiment.json"),
        &experiment,
        "experiment_metadata_write_failed",
        None,
    )?;

    Ok(CommandOutput {
        data: serde_json::json!({
            "experiment": experiment.name,
            "variant": variant.name,
            "agentName": variant.agent_name,
            "rolePath": expected_role.to_string_lossy(),
            "roleSha256": variant.role_sha256,
        }),
        warnings: Vec::new(),
        errors: Vec::new(),
    })
}

fn variant_diff(args: VariantDiffArgs) -> Result<CommandOutput, Vec<CliError>> {
    let loaded = load_experiment(&args.project, &args.experiment)?;
    variant_diff_loaded(&loaded, &args)
}

fn variant_diff_loaded(
    loaded: &LoadedExperiment,
    args: &VariantDiffArgs,
) -> Result<CommandOutput, Vec<CliError>> {
    let errors = validate_loaded_experiment_source_metadata(loaded);
    if !errors.is_empty() {
        return Err(errors);
    }
    validate_variant_name_request(loaded, &args.against)?;
    validate_variant_name_request(loaded, &args.variant)?;
    let left = load_variant_metadata(loaded, &args.against).map_err(|e| {
        vec![err(
            "variant_metadata_missing",
            e,
            Some(args.against.as_str()),
        )]
    })?;
    let right = load_variant_metadata(loaded, &args.variant).map_err(|e| {
        vec![err(
            "variant_metadata_missing",
            e,
            Some(args.variant.as_str()),
        )]
    })?;
    let mut errors = validate_variant_metadata_paths(loaded, &args.against, &left);
    errors.extend(validate_variant_metadata_paths(
        loaded,
        &args.variant,
        &right,
    ));
    if !errors.is_empty() {
        return Err(errors);
    }
    let left_role = variant_matrix_path(
        &loaded.workspace_dir,
        &loaded.experiment.source_agent,
        &args.against,
    )
    .join("Role.md");
    let right_role = variant_matrix_path(
        &loaded.workspace_dir,
        &loaded.experiment.source_agent,
        &args.variant,
    )
    .join("Role.md");
    let left_text = map_err(
        std::fs::read_to_string(&left_role),
        "variant_role_missing",
        Some(args.against.as_str()),
    )?;
    let right_text = map_err(
        std::fs::read_to_string(&right_role),
        "variant_role_missing",
        Some(args.variant.as_str()),
    )?;
    let (diff, stats) = line_diff(&args.against, &args.variant, &left_text, &right_text);
    let mut warnings = Vec::new();
    if stats.identical {
        warnings.push(warn(
            "variant_role_identical",
            "Variant roles are identical",
            Some(args.variant.clone()),
        ));
    }
    Ok(CommandOutput {
        data: serde_json::json!({
            "diff": diff,
            "stats": {
                "addedLines": stats.added_lines,
                "removedLines": stats.removed_lines,
                "changedLines": stats.changed_lines,
                "identical": stats.identical,
            }
        }),
        warnings,
        errors: Vec::new(),
    })
}

fn validate(args: ValidateArgs) -> Result<CommandOutput, Vec<CliError>> {
    let loaded = load_experiment(&args.project, &args.experiment)?;
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    if let Err(mut e) = validate_slug_identity(
        &loaded.experiment.name,
        "Experiment",
        "experiment_name_invalid",
    ) {
        errors.append(&mut e);
    }
    let source_metadata_errors = validate_loaded_experiment_source_metadata(&loaded);
    let source_metadata_valid = source_metadata_errors.is_empty();
    errors.extend(source_metadata_errors);

    if source_metadata_valid {
        let source_matrix =
            source_matrix_path(&loaded.workspace_dir, &loaded.experiment.source_agent);
        let source_role = source_matrix.join("Role.md");
        if !source_matrix.is_dir() {
            errors.push(err(
                "source_agent_missing",
                format!(
                    "Source agent matrix is missing: {}",
                    source_matrix.display()
                ),
                None,
            ));
        }
        if !source_role.is_file() {
            errors.push(err(
                "source_role_missing",
                format!("Source Role.md is missing: {}", source_role.display()),
                None,
            ));
        }
    }

    let mut seen = HashSet::new();
    let mut variant_metas = Vec::new();
    for variant in &loaded.experiment.variants {
        if variant.is_empty() {
            errors.push(err(
                "variant_name_empty",
                "Variant name cannot be empty",
                None,
            ));
            continue;
        }
        if let Err(mut e) = validate_slug_identity(variant, "Variant", "variant_name_invalid") {
            errors.append(&mut e);
        }
        if !seen.insert(variant.clone()) {
            errors.push(err(
                "variant_name_duplicate",
                format!("Duplicate variant '{}'", variant),
                Some(variant.as_str()),
            ));
        }
        if !source_metadata_valid {
            continue;
        }
        match load_variant_metadata(&loaded, variant) {
            Ok(meta) => {
                errors.extend(validate_variant_metadata_paths(&loaded, variant, &meta));
                validate_variant_files(&loaded, &meta, &mut errors);
                for blocker in
                    find_live_sessions_for_variant(&loaded.workspace_dir, &meta.agent_name)
                {
                    errors.push(err(
                        "active_variant_session",
                        format!(
                            "Active session '{}' at {}",
                            blocker.name, blocker.working_directory
                        ),
                        Some(meta.name.as_str()),
                    ));
                }
                if replica_role_overrides(&loaded.workspace_dir, &meta.agent_name)
                    .into_iter()
                    .next()
                    .is_some()
                {
                    errors.push(err(
                        "replica_role_override_detected",
                        format!("Replica Role.md override exists for {}", meta.agent_name),
                        Some(meta.name.as_str()),
                    ));
                }
                variant_metas.push(meta);
            }
            Err(e) => errors.push(err("variant_metadata_missing", e, Some(variant.as_str()))),
        }
    }

    add_diff_warnings(&loaded, &variant_metas, &mut warnings);
    let prompt_count = if let Some(path) = args.prompt_suite.as_ref() {
        validate_prompt_suite(Path::new(path), &mut errors)
    } else {
        None
    };
    if let Some(count) = prompt_count {
        if count * loaded.experiment.variants.len() > 50 {
            warnings.push(warn(
                "large_run_size",
                "Prompt count multiplied by variant count exceeds 50",
                None,
            ));
        }
    }

    Ok(CommandOutput {
        data: serde_json::json!({
            "experiment": loaded.experiment.name,
            "variantCount": loaded.experiment.variants.len(),
            "valid": errors.is_empty(),
        }),
        warnings,
        errors,
    })
}

fn resolve_project(project: &str) -> Result<PathBuf, Vec<CliError>> {
    workgroup::resolve_cli_project(project).map_err(|e| vec![err("project_not_found", e, None)])
}

fn resolve_workspace(project_path: &Path) -> Result<PathBuf, Vec<CliError>> {
    let workspace = workgroup::resolve_cli_workspace(project_path)
        .map_err(|e| vec![err("workspace_not_found", e, None)])?;
    reject_link_or_reparse(&workspace, "workspace_link_or_reparse", None)?;
    std::fs::canonicalize(&workspace).map_err(|e| {
        vec![err(
            "workspace_not_found",
            format!("Failed to canonicalize workspace: {}", e),
            None,
        )]
    })
}

fn load_experiment(project: &str, experiment: &str) -> Result<LoadedExperiment, Vec<CliError>> {
    validate_slug_identity(experiment, "Experiment", "experiment_name_invalid")?;
    let project_path = resolve_project(project)?;
    let workspace_dir = resolve_workspace(&project_path)?;
    let experiment_dir = experiments_dir(&workspace_dir).join(experiment);
    let metadata_path = experiment_dir.join("experiment.json");
    reject_link_or_reparse(&experiment_dir, "experiment_metadata_link_or_reparse", None)?;
    reject_link_or_reparse(&metadata_path, "experiment_metadata_link_or_reparse", None)?;
    let experiment = read_json::<ExperimentMetadata>(&metadata_path)
        .map_err(|e| vec![err("experiment_metadata_missing", e, None)])?;
    Ok(LoadedExperiment {
        workspace_dir,
        experiment_dir,
        experiment,
    })
}

fn load_variant_metadata(
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

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
    serde_json::from_str(&content).map_err(|e| format!("Failed to parse {}: {}", path.display(), e))
}

fn write_json<T: Serialize>(
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

fn validate_experiment_metadata_paths(loaded: &LoadedExperiment) -> Vec<CliError> {
    let parent = &loaded.experiment_dir;
    let source = &loaded.experiment.source_agent;
    let mut errors = Vec::new();
    push_path_mismatch(
        &mut errors,
        parent,
        &loaded.experiment.source_matrix_path,
        &source_matrix_path(&loaded.workspace_dir, source),
        "sourceMatrixPath",
        None,
    );
    push_path_mismatch(
        &mut errors,
        parent,
        &loaded.experiment.source_role_path,
        &source_matrix_path(&loaded.workspace_dir, source).join("Role.md"),
        "sourceRolePath",
        None,
    );
    errors
}

fn validate_loaded_experiment_source_metadata(loaded: &LoadedExperiment) -> Vec<CliError> {
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
    let source_matrix = source_matrix_path(&loaded.workspace_dir, &loaded.experiment.source_agent);
    let source_role = source_matrix.join("Role.md");
    if let Err(mut e) = reject_link_or_reparse(&source_matrix, "source_role_link_or_reparse", None)
    {
        errors.append(&mut e);
    }
    if let Err(mut e) = reject_link_or_reparse(&source_role, "source_role_link_or_reparse", None) {
        errors.append(&mut e);
    }
    errors
}

fn validate_variant_name_request(
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

fn validate_variant_metadata_paths(
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
        &loaded.workspace_dir,
        &loaded.experiment.source_agent,
        requested_variant,
    );
    let expected_role = expected_matrix.join("Role.md");
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

fn validate_variant_files(
    loaded: &LoadedExperiment,
    variant: &VariantMetadata,
    errors: &mut Vec<CliError>,
) {
    let matrix = variant_matrix_path(
        &loaded.workspace_dir,
        &loaded.experiment.source_agent,
        &variant.name,
    );
    let role = matrix.join("Role.md");
    if let Err(mut e) = reject_link_or_reparse(
        &matrix,
        "variant_role_link_or_reparse",
        Some(variant.name.as_str()),
    ) {
        errors.append(&mut e);
    }
    if let Err(mut e) = reject_link_or_reparse(
        &role,
        "variant_role_link_or_reparse",
        Some(variant.name.as_str()),
    ) {
        errors.append(&mut e);
    }
    if !matrix.is_dir() {
        errors.push(err(
            "variant_matrix_missing",
            format!("Variant matrix is missing: {}", matrix.display()),
            Some(variant.name.as_str()),
        ));
    } else if path_key(&matrix)
        == path_key(&source_matrix_path(
            &loaded.workspace_dir,
            &loaded.experiment.source_agent,
        ))
    {
        errors.push(err(
            "variant_matrix_not_separate",
            "Variant matrix resolves to source matrix",
            Some(variant.name.as_str()),
        ));
    }
    if !role.is_file() {
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
    std::fs::canonicalize(path).unwrap_or_else(|_| normalize_lexical(path))
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

fn validate_slug_identity(
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

fn copy_source_skills(
    source_matrix: &Path,
    target_matrix: &Path,
    variant: &str,
    warnings: &mut Vec<CliWarning>,
) {
    let src = source_matrix.join("skills");
    if !src.exists() {
        return;
    }
    for link in collect_link_paths(&src) {
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

fn collect_link_paths(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_link_paths_into(root, &mut out);
    out
}

fn collect_link_paths_into(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(meta) = std::fs::symlink_metadata(root) else {
        return;
    };
    if is_link_or_reparse(&meta) {
        out.push(root.to_path_buf());
        return;
    }
    if !meta.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        collect_link_paths_into(&entry.path(), out);
    }
}

fn reject_link_or_reparse(
    path: &Path,
    code: &'static str,
    variant: Option<&str>,
) -> Result<(), Vec<CliError>> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        if matches!(component, Component::Prefix(_) | Component::RootDir) {
            continue;
        }
        let metadata = match std::fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => {
                return Err(vec![err(
                    code,
                    format!("Failed to inspect {}: {}", current.display(), e),
                    variant,
                )]);
            }
        };
        if is_link_or_reparse(&metadata) {
            return Err(vec![err(
                code,
                format!(
                    "Path component is a symlink or reparse point: {}",
                    current.display()
                ),
                variant,
            )]);
        }
    }
    Ok(())
}

fn is_link_path(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| is_link_or_reparse(&m))
        .unwrap_or(false)
}

fn is_link_or_reparse(meta: &std::fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

fn find_live_sessions_for_variant(
    workspace_dir: &Path,
    variant_agent_name: &str,
) -> Vec<LiveSessionBlocker> {
    let mut out =
        find_live_sessions_under(&workspace_dir.join(format!("_agent_{}", variant_agent_name)));
    if let Ok(entries) = std::fs::read_dir(workspace_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.starts_with("wg-") && path.is_dir() {
                let replica = path.join(format!("__agent_{}", variant_agent_name));
                if replica.is_dir() {
                    out.extend(find_live_sessions_under(&replica));
                }
            }
        }
    }
    out
}

fn replica_role_overrides(workspace_dir: &Path, variant_agent_name: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(workspace_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.starts_with("wg-") {
                let role = path
                    .join(format!("__agent_{}", variant_agent_name))
                    .join("Role.md");
                if role.exists() {
                    out.push(role);
                }
            }
        }
    }
    out
}

fn is_replica_role_file(path: &Path) -> bool {
    let components: Vec<String> = path
        .components()
        .filter_map(|c| c.as_os_str().to_str().map(str::to_string))
        .collect();
    components.windows(4).any(|w| {
        w[0] == ".ac"
            && w[1].starts_with("wg-")
            && w[2].starts_with("__agent_")
            && w[3] == "Role.md"
    })
}

fn add_diff_warnings(
    loaded: &LoadedExperiment,
    variants: &[VariantMetadata],
    warnings: &mut Vec<CliWarning>,
) {
    let Some(control) = variants.iter().find(|v| v.name == "control") else {
        return;
    };
    let control_role = variant_matrix_path(
        &loaded.workspace_dir,
        &loaded.experiment.source_agent,
        &control.name,
    )
    .join("Role.md");
    let Ok(control_text) = std::fs::read_to_string(control_role) else {
        return;
    };
    for variant in variants.iter().filter(|v| v.name != "control") {
        let role = variant_matrix_path(
            &loaded.workspace_dir,
            &loaded.experiment.source_agent,
            &variant.name,
        )
        .join("Role.md");
        let Ok(text) = std::fs::read_to_string(role) else {
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

fn validate_prompt_suite(path: &Path, errors: &mut Vec<CliError>) -> Option<usize> {
    if is_link_path(path) || !path.is_file() {
        errors.push(err(
            "prompt_suite_missing",
            format!("Prompt suite not found: {}", path.display()),
            None,
        ));
        return None;
    }
    let content = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            errors.push(err(
                "prompt_suite_missing",
                format!("Failed to read prompt suite: {}", e),
                None,
            ));
            return None;
        }
    };
    let mut ids = BTreeSet::new();
    let mut count = 0;
    for (idx, line) in content.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let parsed: serde_json::Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(e) => {
                errors.push(err(
                    "prompt_suite_unparseable",
                    format!("Prompt suite line {} is invalid JSON: {}", idx + 1, e),
                    None,
                ));
                continue;
            }
        };
        let id = parsed.get("id").and_then(|v| v.as_str());
        let title = parsed.get("title").and_then(|v| v.as_str());
        let prompt = parsed.get("prompt").and_then(|v| v.as_str());
        let Some(id) = id else {
            errors.push(err(
                "prompt_suite_unparseable",
                format!("Prompt suite line {} is missing string id", idx + 1),
                None,
            ));
            continue;
        };
        if title.is_none() || prompt.is_none() {
            errors.push(err(
                "prompt_suite_unparseable",
                format!("Prompt suite line {} is missing title or prompt", idx + 1),
                None,
            ));
            continue;
        }
        if !ids.insert(id.to_string()) {
            errors.push(err(
                "prompt_suite_duplicate_id",
                format!("Prompt suite duplicate id '{}'", id),
                None,
            ));
        }
        count += 1;
    }
    Some(count)
}

fn line_diff(left_name: &str, right_name: &str, left: &str, right: &str) -> (String, DiffStats) {
    let left_lines: Vec<&str> = left.lines().collect();
    let right_lines: Vec<&str> = right.lines().collect();
    if left == right {
        return (
            format!("--- {}\n+++ {}\n", left_name, right_name),
            DiffStats {
                added_lines: 0,
                removed_lines: 0,
                changed_lines: 0,
                identical: true,
            },
        );
    }
    let max = left_lines.len().max(right_lines.len());
    let mut added = 0;
    let mut removed = 0;
    let mut diff = format!("--- {}\n+++ {}\n", left_name, right_name);
    for i in 0..max {
        match (left_lines.get(i), right_lines.get(i)) {
            (Some(a), Some(b)) if a == b => {
                diff.push(' ');
                diff.push_str(a);
                diff.push('\n');
            }
            (Some(a), Some(b)) => {
                removed += 1;
                added += 1;
                diff.push('-');
                diff.push_str(a);
                diff.push('\n');
                diff.push('+');
                diff.push_str(b);
                diff.push('\n');
            }
            (Some(a), None) => {
                removed += 1;
                diff.push('-');
                diff.push_str(a);
                diff.push('\n');
            }
            (None, Some(b)) => {
                added += 1;
                diff.push('+');
                diff.push_str(b);
                diff.push('\n');
            }
            (None, None) => {}
        }
    }
    (
        diff,
        DiffStats {
            added_lines: added,
            removed_lines: removed,
            changed_lines: added + removed,
            identical: false,
        },
    )
}

fn experiments_dir(workspace_dir: &Path) -> PathBuf {
    workspace_dir.join("experiments")
}

fn source_matrix_path(workspace_dir: &Path, source_agent: &str) -> PathBuf {
    workspace_dir.join(format!("_agent_{}", source_agent))
}

fn variant_agent_name(source_agent: &str, variant: &str) -> String {
    format!("{}-{}", source_agent, variant)
}

fn variant_matrix_path(workspace_dir: &Path, source_agent: &str, variant: &str) -> PathBuf {
    workspace_dir.join(format!(
        "_agent_{}",
        variant_agent_name(source_agent, variant)
    ))
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn map_err<T, E: std::fmt::Display>(
    result: Result<T, E>,
    code: &'static str,
    variant: Option<&str>,
) -> Result<T, Vec<CliError>> {
    result.map_err(|e| vec![err(code, e.to_string(), variant)])
}

fn err(code: &'static str, message: impl Into<String>, variant: Option<&str>) -> CliError {
    CliError {
        code: code.to_string(),
        message: message.into(),
        variant: variant.map(str::to_string),
    }
}

fn warn(code: &'static str, message: impl Into<String>, variant: Option<String>) -> CliWarning {
    CliWarning {
        code: code.to_string(),
        message: message.into(),
        variant,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loaded_fixture(source_agent: &str) -> (tempfile::TempDir, LoadedExperiment) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let workspace_dir = tmp.path().join(".ac");
        let experiment_dir = workspace_dir.join("experiments").join("exp");
        std::fs::create_dir_all(experiment_dir.join("variants")).expect("experiment dirs");

        let source_matrix = workspace_dir.join(format!("_agent_{}", source_agent));
        std::fs::create_dir_all(&source_matrix).expect("source matrix");
        std::fs::write(source_matrix.join("Role.md"), "source role\n").expect("source role");

        for variant in ["control", "test"] {
            let agent_name = variant_agent_name(source_agent, variant);
            let matrix = workspace_dir.join(format!("_agent_{}", agent_name));
            std::fs::create_dir_all(&matrix).expect("variant matrix");
            std::fs::write(matrix.join("Role.md"), format!("{} role\n", variant))
                .expect("variant role");
            let metadata = VariantMetadata {
                schema_version: 1,
                name: variant.to_string(),
                source_agent: source_agent.to_string(),
                agent_name,
                matrix_path: format!("../../../_agent_{}-{}", source_agent, variant),
                role_path: format!("../../../_agent_{}-{}/Role.md", source_agent, variant),
                role_sha256: sha256_hex(format!("{} role\n", variant).as_bytes()),
                created_at: "2026-01-01T00:00:00Z".to_string(),
                updated_at: "2026-01-01T00:00:00Z".to_string(),
            };
            write_json(
                &experiment_dir
                    .join("variants")
                    .join(format!("{}.json", variant)),
                &metadata,
                "variant_metadata_write_failed",
                Some(variant),
            )
            .expect("variant metadata");
        }

        let experiment = ExperimentMetadata {
            schema_version: 1,
            name: "exp".to_string(),
            project: "project".to_string(),
            source_agent: source_agent.to_string(),
            source_matrix_path: format!("../../_agent_{}", source_agent),
            source_role_path: format!("../../_agent_{}/Role.md", source_agent),
            source_role_sha256: sha256_hex(b"source role\n"),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            variants: vec!["control".to_string(), "test".to_string()],
        };
        let loaded = LoadedExperiment {
            workspace_dir,
            experiment_dir,
            experiment,
        };
        (tmp, loaded)
    }

    fn diff_error_envelope(args: VariantDiffArgs, loaded: &LoadedExperiment) -> CliEnvelope {
        match variant_diff_loaded(loaded, &args) {
            Ok(out) => CliEnvelope {
                ok: out.errors.is_empty(),
                command: "role-experiment variant diff",
                data: if out.errors.is_empty() {
                    Some(out.data)
                } else {
                    None
                },
                warnings: out.warnings,
                errors: out.errors,
            },
            Err(errors) => CliEnvelope {
                ok: false,
                command: "role-experiment variant diff",
                data: None,
                warnings: Vec::new(),
                errors,
            },
        }
    }

    #[test]
    fn tampered_source_agent_is_metadata_error_before_path_authority() {
        let (_tmp, mut loaded) = loaded_fixture("alpha");
        loaded.experiment.source_agent = "../evil".to_string();
        loaded.experiment.source_matrix_path = "../../_agent_../evil".to_string();
        loaded.experiment.source_role_path = "../../_agent_../evil/Role.md".to_string();

        let errors = validate_loaded_experiment_source_metadata(&loaded);

        assert!(errors.iter().any(|e| e.code == "metadata_path_mismatch"));
    }

    #[test]
    fn variant_metadata_name_must_match_requested_key() {
        let (_tmp, loaded) = loaded_fixture("alpha");
        let mut metadata = load_variant_metadata(&loaded, "control").expect("variant metadata");
        metadata.name = "test".to_string();

        let errors = validate_variant_metadata_paths(&loaded, "control", &metadata);

        assert!(errors.iter().any(|e| e.code == "metadata_path_mismatch"));
    }

    #[test]
    fn variant_diff_rejects_traversal_variant_before_metadata_load() {
        let (_tmp, loaded) = loaded_fixture("alpha");
        let envelope = diff_error_envelope(
            VariantDiffArgs {
                project: "project".to_string(),
                experiment: "exp".to_string(),
                variant: "../x".to_string(),
                against: "control".to_string(),
            },
            &loaded,
        );

        assert!(!envelope.ok);
        assert!(envelope.data.is_none());
        assert!(envelope
            .errors
            .iter()
            .any(|e| e.code == "variant_name_invalid"));
    }

    #[test]
    fn variant_diff_rejects_unknown_variant_before_metadata_load() {
        let (_tmp, loaded) = loaded_fixture("alpha");
        let envelope = diff_error_envelope(
            VariantDiffArgs {
                project: "project".to_string(),
                experiment: "exp".to_string(),
                variant: "missing".to_string(),
                against: "control".to_string(),
            },
            &loaded,
        );

        assert!(!envelope.ok);
        assert!(envelope.data.is_none());
        assert!(envelope
            .errors
            .iter()
            .any(|e| e.code == "variant_metadata_missing"));
    }

    #[test]
    fn link_or_reparse_check_rejects_existing_ancestor_component() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let real = tmp.path().join("real");
        std::fs::create_dir_all(real.join("child")).expect("real child");
        let link = tmp.path().join("link");

        #[cfg(windows)]
        let link_result = std::os::windows::fs::symlink_dir(&real, &link);
        #[cfg(unix)]
        let link_result = std::os::unix::fs::symlink(&real, &link);

        if link_result.is_err() {
            return;
        }

        let errors =
            reject_link_or_reparse(&link.join("child").join("Role.md"), "link_error", None)
                .expect_err("link ancestor should be rejected");

        assert!(errors.iter().any(|e| e.code == "link_error"));
    }
}
