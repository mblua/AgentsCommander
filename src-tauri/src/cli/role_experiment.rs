use std::collections::{BTreeMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use clap::builder::BoolishValueParser;
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
    Run(RunArgs),
    Report(ReportArgs),
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
struct RunArgs {
    #[arg(long)]
    project: String,
    #[arg(long)]
    experiment: String,
    #[arg(long = "prompt-suite")]
    prompt_suite: String,
    #[arg(long, default_value_t = 1)]
    replicates: u32,
    #[arg(long)]
    seed: Option<u64>,
    #[arg(long = "run-id")]
    run_id: Option<String>,
    #[arg(long = "dry-run")]
    dry_run: bool,
    #[arg(long = "execute")]
    execute: bool,
    #[arg(long = "provider", default_value = "auto")]
    provider: String,
    #[arg(long = "attempt-timeout-secs", default_value_t = 900)]
    attempt_timeout_secs: u64,
    #[arg(long = "max-parallel", default_value_t = 1)]
    max_parallel: u32,
    #[arg(long = "retain-workgroup", value_parser = BoolishValueParser::new())]
    retain_workgroup: Option<bool>,
    #[arg(long = "resume-run")]
    resume_run: bool,
    #[arg(long = "retry-failed")]
    retry_failed: bool,
    #[arg(long = "fake-executor", hide = true)]
    fake_executor: bool,
}

#[derive(Args)]
struct ReportArgs {
    #[arg(long)]
    project: String,
    #[arg(long)]
    experiment: String,
    #[arg(long = "run-id")]
    run_id: String,
    #[arg(long, default_value = "text")]
    format: String,
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
    line: Option<usize>,
    field: Option<String>,
    path: Option<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PromptCase {
    id: String,
    title: String,
    prompt: String,
    tags: Vec<String>,
    expected_behaviors: Vec<String>,
    line: usize,
}

#[derive(Clone)]
struct ParsedPromptSuite {
    path: PathBuf,
    sha256: String,
    prompts: Vec<PromptCase>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunArtifact {
    schema_version: u32,
    run_id: String,
    project: String,
    experiment: String,
    status: String,
    dry_run: bool,
    created_at: String,
    updated_at: String,
    suite: RunSuiteArtifact,
    seed: u64,
    seed_provided: bool,
    replicates: u32,
    variants: Vec<RunVariantArtifact>,
    attempt_count: usize,
    artifacts: RunArtifactPaths,
    execution: Option<RunExecutionArtifact>,
    workgroup: Option<RunWorkgroupArtifact>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunSuiteArtifact {
    path: String,
    sha256: String,
    prompt_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunVariantArtifact {
    name: String,
    agent_name: String,
    role_sha256: String,
    role_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunArtifactPaths {
    attempts: String,
    report_json: String,
    report_markdown: String,
    prompts: String,
    outputs: String,
    transcripts: String,
    attempt_state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunExecutionArtifact {
    provider: String,
    attempt_timeout_secs: u64,
    max_parallel: u32,
    started_at: Option<String>,
    completed_at: Option<String>,
    cancellation_requested: bool,
    executor: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunWorkgroupArtifact {
    name: String,
    path: String,
    retained: bool,
    cleanup_supported: bool,
    run_id: String,
    experiment: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttemptArtifact {
    schema_version: u32,
    attempt_id: String,
    run_id: String,
    prompt_id: String,
    prompt_title: String,
    prompt_line: usize,
    variant: String,
    agent_name: String,
    replicate: u32,
    status: String,
    tags: Vec<String>,
    expected_behaviors: Vec<String>,
    duration_ms: Option<u64>,
    messages: Option<usize>,
    flags: Vec<String>,
    transcript_path: Option<String>,
    session_id: Option<String>,
    workgroup: Option<String>,
    replica_root: Option<String>,
    prompt_path: Option<String>,
    output_path: Option<String>,
    started_at: Option<String>,
    completed_at: Option<String>,
    exit_code: Option<i32>,
    provider: Option<String>,
    provider_transcript_path: Option<String>,
    attempt_generation: u32,
    failure_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttemptOwnershipArtifact {
    schema_version: u32,
    run_id: String,
    attempt_id: String,
    attempt_generation: u32,
    session_id: Option<String>,
    workgroup_path: String,
    replica_root: String,
    prompt_path: String,
    output_path: String,
    started_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReportArtifact {
    schema_version: u32,
    run_id: String,
    experiment: String,
    format: String,
    status: String,
    dry_run: bool,
    generated_at: String,
    summary: ReportSummaryArtifact,
    variants: Vec<ReportVariantArtifact>,
    prompts: Vec<ReportPromptArtifact>,
    artifact_paths: ReportArtifactPaths,
    notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReportSummaryArtifact {
    prompt_count: usize,
    variant_count: usize,
    replicates: u32,
    attempt_count: usize,
    planned_attempt_count: usize,
    running_attempt_count: usize,
    completed_attempt_count: usize,
    failed_attempt_count: usize,
    timed_out_attempt_count: usize,
    inconclusive_attempt_count: usize,
    cancelled_attempt_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReportVariantArtifact {
    name: String,
    agent_name: String,
    planned_attempt_count: usize,
    running_attempt_count: usize,
    completed_attempt_count: usize,
    failed_attempt_count: usize,
    timed_out_attempt_count: usize,
    inconclusive_attempt_count: usize,
    cancelled_attempt_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReportPromptArtifact {
    id: String,
    title: String,
    planned_attempt_count: usize,
    running_attempt_count: usize,
    completed_attempt_count: usize,
    failed_attempt_count: usize,
    timed_out_attempt_count: usize,
    inconclusive_attempt_count: usize,
    cancelled_attempt_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReportArtifactPaths {
    run: String,
    attempts: String,
    report_json: String,
    report_markdown: String,
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

struct ExecutionPlan {
    run_id: String,
    run_dir: PathBuf,
    suite: ParsedPromptSuite,
    attempts: Vec<AttemptArtifact>,
    variants: Vec<VariantMetadata>,
    workgroup: RunWorkgroupArtifact,
    resume_run_artifact: Option<RunArtifact>,
}

struct AttemptExecutionResult {
    attempt_id: String,
    status: String,
    duration_ms: Option<u64>,
    transcript_path: Option<String>,
    provider_transcript_path: Option<String>,
    output_path: Option<String>,
    failure_reason: Option<String>,
    exit_code: Option<i32>,
    flags: Vec<String>,
}

struct ExperimentValidation {
    warnings: Vec<CliWarning>,
    errors: Vec<CliError>,
    variant_metas: Vec<VariantMetadata>,
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
        RoleExperimentCommand::Run(args) => ("role-experiment run", run(args)),
        RoleExperimentCommand::Report(args) => ("role-experiment report", report(args)),
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
    let project_path = resolve_project(&args.project)?;
    init_for_project_path(args, &project_path)
}

fn init_for_project_path(
    args: InitArgs,
    project_path: &Path,
) -> Result<CommandOutput, Vec<CliError>> {
    validate_slug_identity(&args.name, "Experiment", "experiment_name_invalid")?;
    if args.variants.len() < 2 {
        return Err(vec![err(
            "variant_count_invalid",
            "--variants must contain at least two entries",
            None,
        )]);
    }

    let workspace_dir = resolve_workspace(project_path)?;
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
    reject_link_or_reparse(&experiment_dir, "experiment_metadata_link_or_reparse", None)?;
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
            project_path,
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
    let validation = collect_experiment_validation(&loaded);
    let mut errors = validation.errors;
    let mut warnings = validation.warnings;
    let prompt_count = if let Some(path) = args.prompt_suite.as_ref() {
        match parse_prompt_suite(Path::new(path)) {
            Ok(suite) => Some(suite.prompts.len()),
            Err(mut e) => {
                errors.append(&mut e);
                None
            }
        }
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

fn validate_run_mode(args: &RunArgs) -> Result<(), Vec<CliError>> {
    let mut errors = Vec::new();
    match (args.dry_run, args.execute) {
        (false, false) => errors.push(err(
            "run_mode_required",
            "Specify exactly one of --dry-run or --execute",
            None,
        )),
        (true, true) => errors.push(err(
            "run_mode_conflict",
            "Specify only one of --dry-run or --execute",
            None,
        )),
        _ => {}
    }
    if args.fake_executor && !fake_executor_available() {
        errors.push(err(
            "fake_executor_not_available",
            "--fake-executor is only available to test builds",
            None,
        ));
    }
    if args.max_parallel != 1 {
        errors.push(err(
            "parallel_execution_not_supported",
            "--max-parallel must be 1 in Phase 3A",
            None,
        ));
    }
    if !(30..=86_400).contains(&args.attempt_timeout_secs) {
        errors.push(err(
            "attempt_timeout_invalid",
            "--attempt-timeout-secs must be between 30 and 86400",
            None,
        ));
    }
    if matches!(args.retain_workgroup, Some(false)) {
        errors.push(err(
            "cleanup_not_supported",
            "--retain-workgroup=false is not supported in Phase 3A",
            None,
        ));
    }
    if args.resume_run {
        if !args.execute {
            errors.push(err(
                "resume_requires_execute",
                "--resume-run requires --execute",
                None,
            ));
        }
        if args.run_id.is_none() {
            errors.push(err(
                "resume_requires_run_id",
                "--resume-run requires --run-id",
                None,
            ));
        }
    }
    if args.retry_failed && !args.resume_run {
        errors.push(err(
            "retry_failed_requires_resume",
            "--retry-failed requires --resume-run",
            None,
        ));
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn fake_executor_available() -> bool {
    cfg!(test)
        || std::env::var("AGENTSCOMMANDER_ROLE_EXPERIMENT_TEST_FAKE_EXECUTOR")
            .is_ok_and(|value| value == "1")
}

fn execute_requires_runtime_guard(args: &RunArgs) -> Result<(), Vec<CliError>> {
    if args.execute && !args.fake_executor {
        Err(vec![err(
            "execution_requires_running_app",
            "Real role-experiment execution requires a running AgentsCommander app or daemon; Phase 3A CLI-only execution has no side effects",
            None,
        )])
    } else {
        Ok(())
    }
}

fn run(args: RunArgs) -> Result<CommandOutput, Vec<CliError>> {
    validate_run_mode(&args)?;
    execute_requires_runtime_guard(&args)?;
    validate_replicates(args.replicates)?;
    if args.execute {
        return run_fake_execution(args);
    }
    let loaded = load_experiment(&args.project, &args.experiment)?;
    let validation = collect_experiment_validation(&loaded);
    if !validation.errors.is_empty() {
        return Ok(CommandOutput {
            data: serde_json::Value::Null,
            warnings: validation.warnings,
            errors: validation.errors,
        });
    }
    let suite = parse_prompt_suite(Path::new(&args.prompt_suite))?;
    let (run_id, run_dir) = match args.run_id.as_ref() {
        Some(run_id) => (run_id.clone(), create_explicit_run_dir(&loaded, run_id)?),
        None => create_generated_run_dir(&loaded, chrono::Utc::now())?,
    };
    let timestamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let (seed, seed_provided) = match args.seed {
        Some(seed) => (seed, true),
        None => (generated_seed_from_uuid(uuid::Uuid::new_v4()), false),
    };
    let attempts = build_attempts(
        &run_id,
        &suite.prompts,
        &validation.variant_metas,
        args.replicates,
    );
    let artifacts = default_run_artifact_paths();
    let run_artifact = RunArtifact {
        schema_version: 1,
        run_id: run_id.clone(),
        project: args.project.clone(),
        experiment: loaded.experiment.name.clone(),
        status: "dry_run".to_string(),
        dry_run: true,
        created_at: timestamp.clone(),
        updated_at: timestamp.clone(),
        suite: RunSuiteArtifact {
            path: suite.path.to_string_lossy().to_string(),
            sha256: suite.sha256.clone(),
            prompt_count: suite.prompts.len(),
        },
        seed,
        seed_provided,
        replicates: args.replicates,
        variants: validation
            .variant_metas
            .iter()
            .map(|variant| RunVariantArtifact {
                name: variant.name.clone(),
                agent_name: variant.agent_name.clone(),
                role_sha256: variant.role_sha256.clone(),
                role_path: variant_matrix_path(
                    &loaded.workspace_dir,
                    &loaded.experiment.source_agent,
                    &variant.name,
                )
                .join("Role.md")
                .to_string_lossy()
                .to_string(),
            })
            .collect(),
        attempt_count: attempts.len(),
        artifacts: artifacts.clone(),
        execution: None,
        workgroup: None,
        warnings: Vec::new(),
    };
    let report_artifact = build_report_artifact(
        &run_id,
        &loaded.experiment.name,
        &run_dir,
        &timestamp,
        &suite.prompts,
        &validation.variant_metas,
        args.replicates,
        &attempts,
        true,
        None,
    );
    let report_markdown = render_report_markdown(&report_artifact);

    write_json_new(&run_dir.join("run.json"), &run_artifact)?;
    write_attempts_jsonl_new(&run_dir.join("attempts.jsonl"), &attempts)?;
    write_json_new(&run_dir.join("report.json"), &report_artifact)?;
    write_new_file(&run_dir.join("report.md"), report_markdown.as_bytes())?;

    let mut warnings = validation.warnings;
    if attempts.len() > 50 {
        warnings.push(warn(
            "large_run_size",
            "Prompt count multiplied by variant count and replicates exceeds 50",
            None,
        ));
    }

    Ok(CommandOutput {
        data: serde_json::json!({
            "experiment": loaded.experiment.name,
            "runId": run_id,
            "status": "dry_run",
            "dryRun": true,
            "runDir": run_dir.to_string_lossy(),
            "promptCount": suite.prompts.len(),
            "variantCount": validation.variant_metas.len(),
            "replicates": args.replicates,
            "attemptCount": attempts.len(),
            "seed": seed,
            "seedProvided": seed_provided,
            "artifacts": {
                "run": "run.json",
                "attempts": artifacts.attempts,
                "reportJson": artifacts.report_json,
                "reportMarkdown": artifacts.report_markdown,
            }
        }),
        warnings,
        errors: Vec::new(),
    })
}

fn report(args: ReportArgs) -> Result<CommandOutput, Vec<CliError>> {
    if args.format != "text" && args.format != "json" {
        return Err(vec![err(
            "report_format_invalid",
            "--format must be text or json",
            None,
        )]);
    }
    validate_run_id(&args.run_id)?;
    let loaded = load_experiment(&args.project, &args.experiment)?;
    let validation = collect_experiment_validation(&loaded);
    if !validation.errors.is_empty() {
        return Ok(CommandOutput {
            data: serde_json::Value::Null,
            warnings: validation.warnings,
            errors: validation.errors,
        });
    }
    let dir = run_dir(&loaded, &args.run_id);
    reject_link_or_reparse(&dir, "run_artifact_link_or_reparse", None)?;
    if !dir.is_dir() {
        return Err(vec![err_at_path(
            "run_artifact_missing",
            format!("Run directory not found: {}", dir.display()),
            &dir,
        )]);
    }
    let run_path = dir.join("run.json");
    let attempts_path = dir.join("attempts.jsonl");
    let report_path = dir.join("report.json");
    let markdown_path = dir.join("report.md");
    for path in [&run_path, &attempts_path, &report_path, &markdown_path] {
        reject_link_or_reparse(path, "run_artifact_link_or_reparse", None)?;
        if !path.is_file() {
            return Err(vec![err_at_path(
                "run_artifact_missing",
                format!("Run artifact not found: {}", path.display()),
                path,
            )]);
        }
    }
    let run_artifact: RunArtifact = read_json(&run_path)
        .map_err(|e| vec![err_at_path("run_artifact_unparseable", e, &run_path)])?;
    let report_artifact: ReportArtifact = read_json(&report_path)
        .map_err(|e| vec![err_at_path("run_artifact_unparseable", e, &report_path)])?;
    validate_report_artifacts(&loaded, &args.run_id, &run_artifact, &report_artifact)?;
    let expected_artifact_paths = report_artifact_paths(&dir);
    let artifact_paths =
        serde_json::to_value(&expected_artifact_paths).unwrap_or(serde_json::Value::Null);
    if args.format == "json" {
        Ok(CommandOutput {
            data: serde_json::json!({
                "experiment": loaded.experiment.name,
                "runId": args.run_id,
                "format": args.format,
                "status": report_artifact.status,
                "artifactPaths": artifact_paths,
                "report": report_artifact,
            }),
            warnings: validation.warnings,
            errors: Vec::new(),
        })
    } else {
        let markdown = std::fs::read_to_string(&markdown_path).map_err(|e| {
            vec![err_at_path(
                "run_artifact_read_failed",
                format!("Failed to read {}: {}", markdown_path.display(), e),
                &markdown_path,
            )]
        })?;
        Ok(CommandOutput {
            data: serde_json::json!({
                "experiment": loaded.experiment.name,
                "runId": args.run_id,
                "format": args.format,
                "status": report_artifact.status,
                "artifactPaths": artifact_paths,
                "reportMarkdown": markdown,
            }),
            warnings: validation.warnings,
            errors: Vec::new(),
        })
    }
}

fn run_fake_execution(args: RunArgs) -> Result<CommandOutput, Vec<CliError>> {
    let (loaded, mut plan, mut warnings) = prepare_execution_plan(&args)?;
    let timestamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let mut run_artifact = if let Some(mut run_artifact) = plan.resume_run_artifact.take() {
        run_artifact.status = "running".to_string();
        run_artifact.updated_at = timestamp.clone();
        run_artifact.workgroup = Some(plan.workgroup.clone());
        if let Some(execution) = run_artifact.execution.as_mut() {
            execution.provider = args.provider.clone();
            execution.attempt_timeout_secs = args.attempt_timeout_secs;
            execution.max_parallel = args.max_parallel;
            execution.started_at = Some(timestamp.clone());
            execution.completed_at = None;
            execution.cancellation_requested = false;
            execution.executor = "fake".to_string();
        }
        run_artifact
    } else {
        let (seed, seed_provided) = match args.seed {
            Some(seed) => (seed, true),
            None => (generated_seed_from_uuid(uuid::Uuid::new_v4()), false),
        };
        RunArtifact {
            schema_version: 1,
            run_id: plan.run_id.clone(),
            project: args.project.clone(),
            experiment: loaded.experiment.name.clone(),
            status: "running".to_string(),
            dry_run: false,
            created_at: timestamp.clone(),
            updated_at: timestamp.clone(),
            suite: RunSuiteArtifact {
                path: plan.suite.path.to_string_lossy().to_string(),
                sha256: plan.suite.sha256.clone(),
                prompt_count: plan.suite.prompts.len(),
            },
            seed,
            seed_provided,
            replicates: args.replicates,
            variants: plan
                .variants
                .iter()
                .map(|variant| RunVariantArtifact {
                    name: variant.name.clone(),
                    agent_name: variant.agent_name.clone(),
                    role_sha256: variant.role_sha256.clone(),
                    role_path: variant.role_path.clone(),
                })
                .collect(),
            attempt_count: plan.attempts.len(),
            artifacts: default_run_artifact_paths(),
            execution: Some(RunExecutionArtifact {
                provider: args.provider.clone(),
                attempt_timeout_secs: args.attempt_timeout_secs,
                max_parallel: args.max_parallel,
                started_at: Some(timestamp.clone()),
                completed_at: None,
                cancellation_requested: false,
                executor: "fake".to_string(),
            }),
            workgroup: Some(plan.workgroup.clone()),
            warnings: vec![
                "run_workgroup_retained".to_string(),
                "transcript_capture_best_effort".to_string(),
                "no_scoring".to_string(),
                "real_execution_deferred".to_string(),
            ],
        }
    };

    let result = execute_attempts_with_fake_executor(&args, &loaded, &mut plan, &mut run_artifact)?;
    let completed_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    if let Some(execution) = run_artifact.execution.as_mut() {
        execution.completed_at = Some(completed_at.clone());
    }
    run_artifact.updated_at = completed_at.clone();
    run_artifact.status = summarize_run_status(&plan.attempts, false);
    atomic_write_json(&plan.run_dir.join("run.json"), &run_artifact)?;
    write_attempts_jsonl_replace(&plan.run_dir.join("attempts.jsonl"), &plan.attempts)?;
    let report_artifact = build_report_artifact(
        &plan.run_id,
        &loaded.experiment.name,
        &plan.run_dir,
        &completed_at,
        &plan.suite.prompts,
        &plan.variants,
        args.replicates,
        &plan.attempts,
        false,
        Some(&plan.workgroup),
    );
    atomic_write_json(&plan.run_dir.join("report.json"), &report_artifact)?;
    atomic_write_bytes(
        &plan.run_dir.join("report.md"),
        render_report_markdown(&report_artifact).as_bytes(),
    )?;

    warnings.extend([
        warn(
            "run_workgroup_retained",
            "Run workgroup was retained for inspection; cleanup is not implemented in Phase 3A.",
            None,
        ),
        warn(
            "transcript_capture_best_effort",
            "Provider-native transcript paths may be unavailable; use run metadata and retained workgroup for inspection.",
            None,
        ),
        warn(
            "no_scoring",
            "Phase 3A does not score attempts or select winners.",
            None,
        ),
        warn(
            "real_execution_deferred",
            "Real PTY execution is deferred until an app or daemon runtime bridge is available.",
            None,
        ),
    ]);

    Ok(CommandOutput {
        data: serde_json::json!({
            "experiment": loaded.experiment.name,
            "runId": plan.run_id,
            "status": run_artifact.status,
            "dryRun": false,
            "runDir": plan.run_dir.to_string_lossy(),
            "promptCount": plan.suite.prompts.len(),
            "variantCount": plan.variants.len(),
            "replicates": args.replicates,
            "attemptCount": plan.attempts.len(),
            "resultCount": result.len(),
            "seed": run_artifact.seed,
            "seedProvided": run_artifact.seed_provided,
            "artifacts": {
                "run": "run.json",
                "attempts": "attempts.jsonl",
                "reportJson": "report.json",
                "reportMarkdown": "report.md",
                "prompts": "prompts",
                "outputs": "outputs",
                "transcripts": "transcripts",
                "attemptState": "attempt-state",
            },
            "workgroup": run_artifact.workgroup,
            "execution": run_artifact.execution,
        }),
        warnings,
        errors: Vec::new(),
    })
}

fn prepare_execution_plan(
    args: &RunArgs,
) -> Result<(LoadedExperiment, ExecutionPlan, Vec<CliWarning>), Vec<CliError>> {
    let loaded = load_experiment(&args.project, &args.experiment)?;
    let validation = collect_experiment_validation(&loaded);
    if !validation.errors.is_empty() {
        return Err(validation.errors);
    }
    let suite = parse_prompt_suite(Path::new(&args.prompt_suite))?;
    let (run_id, run_dir) = if args.resume_run {
        let run_id = args.run_id.clone().expect("validated run id");
        validate_run_id(&run_id)?;
        let dir = run_dir(&loaded, &run_id);
        reject_link_or_reparse(&dir, "run_artifact_link_or_reparse", None)?;
        if !dir.is_dir() {
            return Err(vec![err_at_path(
                "run_artifact_missing",
                format!("Run directory not found: {}", dir.display()),
                &dir,
            )]);
        }
        (run_id, dir)
    } else {
        match args.run_id.as_ref() {
            Some(run_id) => (run_id.clone(), create_explicit_run_dir(&loaded, run_id)?),
            None => create_generated_run_dir(&loaded, chrono::Utc::now())?,
        }
    };
    let resume_contract = if args.resume_run {
        Some(load_and_validate_resume_artifacts(
            args,
            &loaded,
            &suite,
            &run_id,
            &run_dir,
            &validation.variant_metas,
        )?)
    } else {
        ensure_run_artifact_dirs(&run_dir)?;
        None
    };
    let (workgroup, resume_run_artifact, resume_attempts) =
        if let Some((run_artifact, attempts, _report_artifact, workgroup)) = resume_contract {
            (workgroup, Some(run_artifact), Some(attempts))
        } else {
            (
                create_run_workgroup_dirs(
                    &loaded.workspace_dir,
                    &loaded.experiment.name,
                    &run_id,
                    &validation.variant_metas,
                )?,
                None,
                None,
            )
        };
    let attempts = if args.resume_run {
        let _validated_attempts = resume_attempts;
        reconcile_mutable_artifacts(&run_dir, &workgroup, args.retry_failed)?
    } else {
        build_attempts(
            &run_id,
            &suite.prompts,
            &validation.variant_metas,
            args.replicates,
        )
    };
    Ok((
        loaded,
        ExecutionPlan {
            run_id,
            run_dir,
            suite,
            attempts,
            variants: validation.variant_metas,
            workgroup,
            resume_run_artifact,
        },
        validation.warnings,
    ))
}

fn execute_attempts_with_fake_executor(
    args: &RunArgs,
    loaded: &LoadedExperiment,
    plan: &mut ExecutionPlan,
    run_artifact: &mut RunArtifact,
) -> Result<Vec<AttemptExecutionResult>, Vec<CliError>> {
    let mut results = Vec::new();
    for idx in 0..plan.attempts.len() {
        let should_run = if args.retry_failed {
            matches!(
                plan.attempts[idx].status.as_str(),
                "failed" | "timed_out" | "inconclusive"
            )
        } else {
            plan.attempts[idx].status == "planned"
        };
        if !should_run {
            continue;
        }
        if args.retry_failed {
            plan.attempts[idx].attempt_generation += 1;
            plan.attempts[idx].flags.push("retry_failed".to_string());
        }
        let started_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let prompt = plan
            .suite
            .prompts
            .iter()
            .find(|prompt| prompt.id == plan.attempts[idx].prompt_id)
            .ok_or_else(|| {
                vec![err(
                    "run_artifact_mismatch",
                    "Attempt prompt id is not present in the prompt suite",
                    None,
                )]
            })?;
        let prompt_path = write_attempt_prompt_file(&plan.run_dir, &plan.attempts[idx], prompt)?;
        let output_path = next_attempt_output_path(&plan.run_dir, &plan.attempts[idx])?;
        let replica_root = replica_root_for_attempt(&plan.workgroup, &plan.attempts[idx])?;
        let ownership = AttemptOwnershipArtifact {
            schema_version: 1,
            run_id: plan.run_id.clone(),
            attempt_id: plan.attempts[idx].attempt_id.clone(),
            attempt_generation: plan.attempts[idx].attempt_generation,
            session_id: None,
            workgroup_path: plan.workgroup.path.clone(),
            replica_root: replica_root.to_string_lossy().to_string(),
            prompt_path: prompt_path.to_string_lossy().to_string(),
            output_path: output_path.to_string_lossy().to_string(),
            started_at: started_at.clone(),
        };
        atomic_write_json(
            &plan
                .run_dir
                .join("attempt-state")
                .join(format!("{}.running.json", plan.attempts[idx].attempt_id)),
            &ownership,
        )?;
        plan.attempts[idx].status = "running".to_string();
        plan.attempts[idx].started_at = Some(started_at.clone());
        plan.attempts[idx].prompt_path = Some(prompt_path.to_string_lossy().to_string());
        plan.attempts[idx].output_path = Some(output_path.to_string_lossy().to_string());
        plan.attempts[idx].replica_root = Some(replica_root.to_string_lossy().to_string());
        plan.attempts[idx].workgroup = Some(plan.workgroup.name.clone());
        plan.attempts[idx].provider = Some(args.provider.clone());
        write_attempts_jsonl_replace(&plan.run_dir.join("attempts.jsonl"), &plan.attempts)?;
        run_artifact.status = "running".to_string();
        run_artifact.updated_at = started_at.clone();
        atomic_write_json(&plan.run_dir.join("run.json"), run_artifact)?;

        let result = fake_attempt_result(args, &plan.attempts[idx], &output_path);
        let completed_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        atomic_write_bytes(
            &output_path,
            fake_output_text(&result, &started_at, &completed_at).as_bytes(),
        )?;
        atomic_write_json(
            &plan
                .run_dir
                .join("attempt-state")
                .join(format!("{}.done.json", plan.attempts[idx].attempt_id)),
            &ownership,
        )?;
        plan.attempts[idx].status = result.status.clone();
        plan.attempts[idx].duration_ms = result.duration_ms;
        plan.attempts[idx].transcript_path = result.transcript_path.clone();
        plan.attempts[idx].provider_transcript_path = result.provider_transcript_path.clone();
        plan.attempts[idx].output_path = result.output_path.clone();
        plan.attempts[idx].failure_reason = result.failure_reason.clone();
        plan.attempts[idx].exit_code = result.exit_code;
        plan.attempts[idx].flags.extend(result.flags.clone());
        plan.attempts[idx].completed_at = Some(completed_at);
        plan.attempts[idx].session_id = None;
        results.push(result);

        let _ = loaded;
    }
    Ok(results)
}

fn fake_attempt_result(
    args: &RunArgs,
    attempt: &AttemptArtifact,
    output_path: &Path,
) -> AttemptExecutionResult {
    let (status, failure_reason, flags) = match args.provider.as_str() {
        "fake-failed" => (
            "failed".to_string(),
            Some("fake_executor_failure".to_string()),
            vec!["fake_executor_failure".to_string()],
        ),
        "fake-timed-out" => (
            "timed_out".to_string(),
            Some("fake_executor_timeout".to_string()),
            vec!["fake_executor_timeout".to_string()],
        ),
        "fake-no-activity" => (
            "inconclusive".to_string(),
            Some("no_provider_activity_after_injection".to_string()),
            vec!["no_provider_activity_after_injection".to_string()],
        ),
        "fake-unexpected-shell" => (
            "inconclusive".to_string(),
            Some("unexpected_shell".to_string()),
            vec!["unexpected_shell".to_string()],
        ),
        _ => ("completed".to_string(), None, Vec::new()),
    };
    AttemptExecutionResult {
        attempt_id: attempt.attempt_id.clone(),
        status,
        duration_ms: Some(0),
        transcript_path: None,
        provider_transcript_path: None,
        output_path: Some(output_path.to_string_lossy().to_string()),
        failure_reason,
        exit_code: None,
        flags,
    }
}

fn fake_output_text(
    result: &AttemptExecutionResult,
    started_at: &str,
    completed_at: &str,
) -> String {
    format!(
        "attemptId: {}\nstatus: {}\nstartedAt: {}\ncompletedAt: {}\nflags: {}\nfailureReason: {}\n",
        result.attempt_id,
        result.status,
        started_at,
        completed_at,
        result.flags.join(","),
        result.failure_reason.clone().unwrap_or_default()
    )
}

fn collect_experiment_validation(loaded: &LoadedExperiment) -> ExperimentValidation {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    if let Err(mut e) = validate_slug_identity(
        &loaded.experiment.name,
        "Experiment",
        "experiment_name_invalid",
    ) {
        errors.append(&mut e);
    }
    let source_metadata_errors = validate_loaded_experiment_source_metadata(loaded);
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
        match load_variant_metadata(loaded, variant) {
            Ok(meta) => {
                errors.extend(validate_variant_metadata_paths(loaded, variant, &meta));
                validate_variant_files(loaded, &meta, &mut errors);
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

    add_diff_warnings(loaded, &variant_metas, &mut warnings);
    ExperimentValidation {
        warnings,
        errors,
        variant_metas,
    }
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
            format!("Failed to canonicalize Project AC Root: {}", e),
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

fn write_json_new<T: Serialize>(path: &Path, value: &T) -> Result<(), Vec<CliError>> {
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

fn write_attempts_jsonl_new(
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

fn write_attempts_jsonl_replace(
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

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), Vec<CliError>> {
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

fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), Vec<CliError>> {
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

fn atomic_write_bytes(path: &Path, bytes: &[u8]) -> Result<(), Vec<CliError>> {
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

fn ensure_run_artifact_dirs(run_dir: &Path) -> Result<(), Vec<CliError>> {
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

fn write_attempt_prompt_file(
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
        "# Role Experiment Attempt\n\nRun: {}\nAttempt: {}\nVariant: {}\nPrompt: {}\nReplicate: {}\n\n## Runner Instructions\n\nRespond to the prompt below. Do not modify files outside paths allowed by your session instructions.\n\n## Prompt\n\n{}\n",
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

#[allow(dead_code)]
fn build_attempt_injection(prompt_path: &Path) -> String {
    format!(
        "Role experiment attempt.\n\nRead the prompt file at the exact path below. The path is delimited and quoted for Windows shells:\n\n```role-experiment-path\n\"{}\"\n```\n\nWhen complete, include a concise final answer in this session.\n",
        prompt_path.to_string_lossy()
    )
}

fn next_attempt_output_path(
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

fn create_run_workgroup_dirs(
    workspace_dir: &Path,
    experiment: &str,
    run_id: &str,
    variants: &[VariantMetadata],
) -> Result<RunWorkgroupArtifact, Vec<CliError>> {
    reject_link_or_reparse(workspace_dir, "workspace_link_or_reparse", None)?;
    let sanitized_experiment = sanitize_name(experiment).map_err(|e| {
        vec![err(
            "experiment_name_invalid",
            format!("Experiment name cannot be used for workgroup: {}", e),
            None,
        )]
    })?;
    let next = next_workgroup_number(workspace_dir)?;
    let name = format!("wg-{}-role-exp-{}", next, sanitized_experiment);
    let path = workspace_dir.join(&name);
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
        let role_path = variant_matrix_path(workspace_dir, &variant.source_agent, &variant.name)
            .join("Role.md");
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

fn next_workgroup_number(workspace_dir: &Path) -> Result<u32, Vec<CliError>> {
    let mut max = 0;
    for entry in fs::read_dir(workspace_dir).map_err(|e| {
        vec![err_at_path(
            "workspace_not_found",
            format!("Failed to read {}: {}", workspace_dir.display(), e),
            workspace_dir,
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
        if let Some(rest) = name.strip_prefix("wg-") {
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(n) = digits.parse::<u32>() {
                max = max.max(n);
            }
        }
    }
    Ok(max + 1)
}

fn validate_workgroup_under_workspace(
    workspace_dir: &Path,
    workgroup: &RunWorkgroupArtifact,
) -> Result<(), Vec<CliError>> {
    let workspace = fs::canonicalize(workspace_dir).map_err(|e| {
        vec![err_at_path(
            "workspace_not_found",
            format!("Failed to canonicalize Project AC Root: {}", e),
            workspace_dir,
        )]
    })?;
    let path = PathBuf::from(&workgroup.path);
    reject_link_or_reparse(&path, "run_artifact_link_or_reparse", None)?;
    let canonical = fs::canonicalize(&path).map_err(|e| {
        vec![err_at_path(
            "run_artifact_mismatch",
            format!("Failed to canonicalize workgroup: {}", e),
            &path,
        )]
    })?;
    if canonical.starts_with(workspace) && workgroup.name.starts_with("wg-") {
        Ok(())
    } else {
        Err(vec![err(
            "run_artifact_mismatch",
            "Run workgroup is not under the current Project AC Root",
            None,
        )])
    }
}

fn load_and_validate_resume_artifacts(
    args: &RunArgs,
    loaded: &LoadedExperiment,
    suite: &ParsedPromptSuite,
    run_id: &str,
    run_dir: &Path,
    variants: &[VariantMetadata],
) -> Result<
    (
        RunArtifact,
        Vec<AttemptArtifact>,
        ReportArtifact,
        RunWorkgroupArtifact,
    ),
    Vec<CliError>,
> {
    let run_path = run_dir.join("run.json");
    let attempts_path = run_dir.join("attempts.jsonl");
    let report_path = run_dir.join("report.json");
    let markdown_path = run_dir.join("report.md");
    let state_dir = run_dir.join("attempt-state");
    for path in [&run_path, &attempts_path, &report_path, &markdown_path] {
        reject_link_or_reparse(path, "run_artifact_link_or_reparse", None)?;
        if !path.is_file() {
            return Err(vec![err_at_path(
                "run_artifact_missing",
                format!("Run artifact not found: {}", path.display()),
                path,
            )]);
        }
    }
    reject_link_or_reparse(&state_dir, "run_artifact_link_or_reparse", None)?;
    if !state_dir.is_dir() {
        return Err(vec![err_at_path(
            "run_artifact_missing",
            format!("Run artifact directory not found: {}", state_dir.display()),
            &state_dir,
        )]);
    }

    let run_artifact: RunArtifact = read_json(&run_path)
        .map_err(|e| vec![err_at_path("run_artifact_unparseable", e, &run_path)])?;
    let attempts = read_attempts_jsonl(&attempts_path)?;
    let report_artifact: ReportArtifact = read_json(&report_path)
        .map_err(|e| vec![err_at_path("run_artifact_unparseable", e, &report_path)])?;

    let expected_attempts = build_attempts(run_id, &suite.prompts, variants, args.replicates);
    let expected_variants: Vec<RunVariantArtifact> = variants
        .iter()
        .map(|variant| RunVariantArtifact {
            name: variant.name.clone(),
            agent_name: variant.agent_name.clone(),
            role_sha256: variant.role_sha256.clone(),
            role_path: variant.role_path.clone(),
        })
        .collect();
    let expected_suite = RunSuiteArtifact {
        path: suite.path.to_string_lossy().to_string(),
        sha256: suite.sha256.clone(),
        prompt_count: suite.prompts.len(),
    };

    let mut errors = Vec::new();
    if let Err(mut report_errors) =
        validate_report_artifacts(loaded, run_id, &run_artifact, &report_artifact)
    {
        errors.append(&mut report_errors);
    }
    if run_artifact.schema_version != 1 || run_artifact.project != args.project {
        errors.push(err(
            "run_artifact_mismatch",
            "Run artifact schema or project does not match resume arguments",
            None,
        ));
    }
    if run_artifact.run_id != run_id
        || run_artifact.experiment != loaded.experiment.name
        || run_artifact.dry_run
        || run_artifact.execution.is_none()
    {
        errors.push(err(
            "run_artifact_mismatch",
            "Run artifact identity does not match resume arguments",
            None,
        ));
    }
    if run_artifact.suite != expected_suite
        || run_artifact.replicates != args.replicates
        || run_artifact.variants != expected_variants
        || run_artifact.attempt_count != expected_attempts.len()
        || run_artifact.artifacts != default_run_artifact_paths()
    {
        errors.push(err(
            "run_artifact_mismatch",
            "Run artifact contract does not match resume arguments",
            None,
        ));
    }
    if let Some(seed) = args.seed {
        if run_artifact.seed != seed || !run_artifact.seed_provided {
            errors.push(err(
                "run_artifact_mismatch",
                "Run artifact seed does not match resume arguments",
                None,
            ));
        }
    }
    if attempts.len() != expected_attempts.len() {
        errors.push(err(
            "run_artifact_mismatch",
            "Attempt artifact count does not match resume arguments",
            None,
        ));
    }
    for (attempt, expected) in attempts.iter().zip(expected_attempts.iter()) {
        if attempt.schema_version != 1
            || attempt.attempt_id != expected.attempt_id
            || attempt.run_id != expected.run_id
            || attempt.prompt_id != expected.prompt_id
            || attempt.prompt_title != expected.prompt_title
            || attempt.prompt_line != expected.prompt_line
            || attempt.variant != expected.variant
            || attempt.agent_name != expected.agent_name
            || attempt.replicate != expected.replicate
            || attempt.tags != expected.tags
            || attempt.expected_behaviors != expected.expected_behaviors
        {
            errors.push(err(
                "run_artifact_mismatch",
                "Attempt artifact identity does not match resume arguments",
                Some(&attempt.variant),
            ));
        }
    }
    if report_artifact.variants.len() != expected_variants.len()
        || report_artifact.prompts.len() != suite.prompts.len()
    {
        errors.push(err(
            "run_artifact_mismatch",
            "Report artifact shape does not match resume arguments",
            None,
        ));
    }
    for (report_variant, expected_variant) in report_artifact
        .variants
        .iter()
        .zip(expected_variants.iter())
    {
        if report_variant.name != expected_variant.name
            || report_variant.agent_name != expected_variant.agent_name
        {
            errors.push(err(
                "run_artifact_mismatch",
                "Report variant identity does not match resume arguments",
                Some(&report_variant.name),
            ));
        }
    }
    for (report_prompt, expected_prompt) in report_artifact.prompts.iter().zip(suite.prompts.iter())
    {
        if report_prompt.id != expected_prompt.id || report_prompt.title != expected_prompt.title {
            errors.push(err(
                "run_artifact_mismatch",
                "Report prompt identity does not match resume arguments",
                None,
            ));
        }
    }

    let Some(workgroup) = run_artifact.workgroup.clone() else {
        errors.push(err(
            "run_artifact_mismatch",
            "Run artifact is missing workgroup metadata",
            None,
        ));
        return Err(errors);
    };
    if workgroup.run_id != run_id
        || workgroup.experiment != loaded.experiment.name
        || !workgroup.retained
        || workgroup.cleanup_supported
    {
        errors.push(err(
            "run_artifact_mismatch",
            "Run workgroup metadata does not match resume arguments",
            None,
        ));
    }
    if let Err(mut workgroup_errors) =
        validate_workgroup_under_workspace(&loaded.workspace_dir, &workgroup)
    {
        errors.append(&mut workgroup_errors);
    }
    if let Err(mut state_errors) =
        validate_attempt_state_contract(&state_dir, &attempts, &workgroup)
    {
        errors.append(&mut state_errors);
    }

    if errors.is_empty() {
        Ok((run_artifact, attempts, report_artifact, workgroup))
    } else {
        Err(errors)
    }
}

fn validate_attempt_state_contract(
    state_dir: &Path,
    attempts: &[AttemptArtifact],
    workgroup: &RunWorkgroupArtifact,
) -> Result<(), Vec<CliError>> {
    let mut errors = Vec::new();
    let attempt_ids: HashSet<&str> = attempts
        .iter()
        .map(|attempt| attempt.attempt_id.as_str())
        .collect();
    let attempts_by_id: BTreeMap<&str, &AttemptArtifact> = attempts
        .iter()
        .map(|attempt| (attempt.attempt_id.as_str(), attempt))
        .collect();
    let entries = fs::read_dir(state_dir).map_err(|e| {
        vec![err_at_path(
            "run_artifact_unparseable",
            format!("Failed to read {}: {}", state_dir.display(), e),
            state_dir,
        )]
    })?;
    for entry in entries {
        let entry = entry.map_err(|e| {
            vec![err_at_path(
                "run_artifact_unparseable",
                format!("Failed to read attempt-state entry: {}", e),
                state_dir,
            )]
        })?;
        let path = entry.path();
        reject_link_or_reparse(&path, "run_artifact_link_or_reparse", None)?;
        if !path.is_file() {
            errors.push(err_at_path(
                "run_artifact_mismatch",
                format!("Attempt-state entry is not a file: {}", path.display()),
                &path,
            ));
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            errors.push(err_at_path(
                "run_artifact_mismatch",
                format!("Attempt-state entry has invalid name: {}", path.display()),
                &path,
            ));
            continue;
        };
        let attempt_id = file_name
            .strip_suffix(".running.json")
            .or_else(|| file_name.strip_suffix(".done.json"));
        let Some(attempt_id) = attempt_id else {
            errors.push(err_at_path(
                "run_artifact_mismatch",
                format!(
                    "Attempt-state entry has unsupported name: {}",
                    path.display()
                ),
                &path,
            ));
            continue;
        };
        if !attempt_ids.contains(attempt_id) {
            errors.push(err_at_path(
                "run_artifact_mismatch",
                format!(
                    "Attempt-state entry does not match a run attempt: {}",
                    path.display()
                ),
                &path,
            ));
            continue;
        }
        let ownership: AttemptOwnershipArtifact = match read_json(&path) {
            Ok(ownership) => ownership,
            Err(e) => {
                errors.push(err_at_path("run_artifact_unparseable", e, &path));
                continue;
            }
        };
        if ownership.schema_version != 1 {
            errors.push(err_at_path(
                "run_artifact_mismatch",
                "Attempt ownership schemaVersion must be 1",
                &path,
            ));
            continue;
        }
        if let Some(attempt) = attempts_by_id.get(attempt_id) {
            if let Err(mut ownership_errors) =
                validate_ownership_matches_attempt(&ownership, attempt, workgroup)
            {
                errors.append(&mut ownership_errors);
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn replica_root_for_attempt(
    workgroup: &RunWorkgroupArtifact,
    attempt: &AttemptArtifact,
) -> Result<PathBuf, Vec<CliError>> {
    let workgroup_path = PathBuf::from(&workgroup.path);
    let replica = workgroup_path.join(format!("__agent_{}", attempt.agent_name));
    reject_link_or_reparse(
        &replica,
        "run_artifact_link_or_reparse",
        Some(&attempt.variant),
    )?;
    let canonical_workgroup = fs::canonicalize(&workgroup_path).map_err(|e| {
        vec![err_at_path(
            "run_artifact_mismatch",
            format!("Failed to canonicalize workgroup: {}", e),
            &workgroup_path,
        )]
    })?;
    let canonical_replica = fs::canonicalize(&replica).map_err(|e| {
        vec![err_at_path(
            "run_artifact_mismatch",
            format!("Failed to canonicalize replica: {}", e),
            &replica,
        )]
    })?;
    if canonical_replica.starts_with(canonical_workgroup) {
        Ok(canonical_replica)
    } else {
        Err(vec![err(
            "run_artifact_mismatch",
            "Replica root is outside run workgroup",
            Some(&attempt.variant),
        )])
    }
}

fn read_attempts_jsonl(path: &Path) -> Result<Vec<AttemptArtifact>, Vec<CliError>> {
    let content = fs::read_to_string(path).map_err(|e| {
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

fn reconcile_mutable_artifacts(
    run_dir: &Path,
    workgroup: &RunWorkgroupArtifact,
    retry_failed: bool,
) -> Result<Vec<AttemptArtifact>, Vec<CliError>> {
    let mut attempts = read_attempts_jsonl(&run_dir.join("attempts.jsonl"))?;
    let state_dir = run_dir.join("attempt-state");
    reject_link_or_reparse(&state_dir, "run_artifact_link_or_reparse", None)?;
    for attempt in &mut attempts {
        let done_path = state_dir.join(format!("{}.done.json", attempt.attempt_id));
        let running_path = state_dir.join(format!("{}.running.json", attempt.attempt_id));
        let done = if done_path.is_file() {
            Some(
                read_json::<AttemptOwnershipArtifact>(&done_path)
                    .map_err(|e| vec![err_at_path("run_artifact_unparseable", e, &done_path)])?,
            )
        } else {
            None
        };
        let running = if running_path.is_file() {
            Some(
                read_json::<AttemptOwnershipArtifact>(&running_path)
                    .map_err(|e| vec![err_at_path("run_artifact_unparseable", e, &running_path)])?,
            )
        } else {
            None
        };
        if let (Some(done), Some(running)) = (&done, &running) {
            validate_ownership_pair(done, running)?;
        }
        if let Some(ownership) = done.as_ref().or(running.as_ref()) {
            validate_ownership_matches_attempt(ownership, attempt, workgroup)?;
        }
        if done.is_some() {
            continue;
        }
        if running.is_some() && !retry_failed {
            attempt.status = "inconclusive".to_string();
            attempt.failure_reason = Some("stale_running_attempt".to_string());
            if !attempt
                .flags
                .iter()
                .any(|flag| flag == "stale_running_attempt")
            {
                attempt.flags.push("stale_running_attempt".to_string());
            }
        }
    }
    write_attempts_jsonl_replace(&run_dir.join("attempts.jsonl"), &attempts)?;
    Ok(attempts)
}

fn validate_ownership_pair(
    done: &AttemptOwnershipArtifact,
    running: &AttemptOwnershipArtifact,
) -> Result<(), Vec<CliError>> {
    if done.run_id == running.run_id
        && done.attempt_id == running.attempt_id
        && done.attempt_generation == running.attempt_generation
        && done.workgroup_path == running.workgroup_path
        && done.replica_root == running.replica_root
    {
        Ok(())
    } else {
        Err(vec![err(
            "run_artifact_mismatch",
            "Attempt ownership records disagree",
            None,
        )])
    }
}

fn validate_ownership_matches_attempt(
    ownership: &AttemptOwnershipArtifact,
    attempt: &AttemptArtifact,
    workgroup: &RunWorkgroupArtifact,
) -> Result<(), Vec<CliError>> {
    let expected_replica = replica_root_for_attempt(workgroup, attempt)?;
    if ownership.run_id == attempt.run_id
        && ownership.attempt_id == attempt.attempt_id
        && ownership.attempt_generation == attempt.attempt_generation
        && ownership.workgroup_path == workgroup.path
        && Path::new(&ownership.replica_root) == expected_replica.as_path()
    {
        Ok(())
    } else {
        Err(vec![err(
            "run_artifact_mismatch",
            "Attempt ownership does not match attempt metadata",
            Some(&attempt.variant),
        )])
    }
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
            if name.starts_with("wg-") && !name.contains("-role-exp-") {
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

fn parse_prompt_suite(path: &Path) -> Result<ParsedPromptSuite, Vec<CliError>> {
    reject_link_or_reparse(path, "prompt_suite_missing", None)?;
    if !path.is_file() {
        return Err(vec![err_at_path(
            "prompt_suite_missing",
            format!("Prompt suite not found: {}", path.display()),
            path,
        )]);
    }
    let bytes = std::fs::read(path).map_err(|e| {
        vec![err_at_path(
            "prompt_suite_missing",
            format!("Failed to read prompt suite: {}", e),
            path,
        )]
    })?;
    let sha256 = sha256_hex(&bytes);
    let content = String::from_utf8(bytes).map_err(|e| {
        vec![err_at_path(
            "prompt_suite_unparseable",
            format!("Prompt suite is not valid UTF-8: {}", e),
            path,
        )]
    })?;
    let mut errors = Vec::new();
    let mut prompts = Vec::new();
    let mut ids = BTreeMap::new();
    for (idx, line) in content.lines().enumerate() {
        let physical_line = idx + 1;
        if line.trim().is_empty() {
            continue;
        }
        let parsed: serde_json::Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(e) => {
                errors.push(err_at_line(
                    "prompt_suite_unparseable",
                    format!("Prompt suite line {} is invalid JSON: {}", physical_line, e),
                    physical_line,
                    None,
                ));
                continue;
            }
        };
        let Some(object) = parsed.as_object() else {
            errors.push(err_at_line(
                "prompt_suite_unparseable",
                format!("Prompt suite line {} must be a JSON object", physical_line),
                physical_line,
                None,
            ));
            continue;
        };
        let Some(id) = required_prompt_string(object, "id", physical_line, &mut errors) else {
            continue;
        };
        if !is_safe_prompt_id(&id) {
            errors.push(err_at_line(
                "prompt_suite_unparseable",
                format!(
                    "Prompt suite line {} has invalid id '{}'; use lowercase ASCII letters, digits, hyphen, or underscore",
                    physical_line, id
                ),
                physical_line,
                Some("id"),
            ));
            continue;
        }
        let Some(title) = required_prompt_string(object, "title", physical_line, &mut errors)
        else {
            continue;
        };
        let Some(prompt) = required_prompt_string(object, "prompt", physical_line, &mut errors)
        else {
            continue;
        };
        let Some(tags) = optional_string_array(object, "tags", physical_line, &mut errors) else {
            continue;
        };
        let Some(expected_behaviors) =
            optional_string_array(object, "expectedBehaviors", physical_line, &mut errors)
        else {
            continue;
        };
        if let Some(first_line) = ids.insert(id.clone(), physical_line) {
            errors.push(err_at_line(
                "prompt_suite_duplicate_id",
                format!(
                    "Prompt suite duplicate id '{}' at line {}; first seen at line {}",
                    id, physical_line, first_line
                ),
                physical_line,
                Some("id"),
            ));
            continue;
        }
        prompts.push(PromptCase {
            id,
            title,
            prompt,
            tags,
            expected_behaviors,
            line: physical_line,
        });
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    if prompts.is_empty() {
        return Err(vec![err_at_path(
            "prompt_suite_empty",
            "Prompt suite contains no prompt cases",
            path,
        )]);
    }
    Ok(ParsedPromptSuite {
        path: std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()),
        sha256,
        prompts,
    })
}

fn required_prompt_string(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &'static str,
    line: usize,
    errors: &mut Vec<CliError>,
) -> Option<String> {
    let value = object.get(field).and_then(|v| v.as_str()).map(str::trim);
    match value {
        Some(value) if !value.is_empty() => Some(value.to_string()),
        _ => {
            errors.push(err_at_line(
                "prompt_suite_unparseable",
                format!(
                    "Prompt suite line {} is missing required string field {}",
                    line, field
                ),
                line,
                Some(field),
            ));
            None
        }
    }
}

fn optional_string_array(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &'static str,
    line: usize,
    errors: &mut Vec<CliError>,
) -> Option<Vec<String>> {
    let Some(value) = object.get(field) else {
        return Some(Vec::new());
    };
    let Some(items) = value.as_array() else {
        errors.push(err_at_line(
            "prompt_suite_unparseable",
            format!(
                "Prompt suite line {} field {} must be an array of strings",
                line, field
            ),
            line,
            Some(field),
        ));
        return None;
    };
    let mut out = Vec::new();
    for item in items {
        let Some(item) = item.as_str() else {
            errors.push(err_at_line(
                "prompt_suite_unparseable",
                format!(
                    "Prompt suite line {} field {} must contain only strings",
                    line, field
                ),
                line,
                Some(field),
            ));
            return None;
        };
        out.push(item.to_string());
    }
    Some(out)
}

fn is_safe_prompt_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
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

fn runs_dir(loaded: &LoadedExperiment) -> PathBuf {
    loaded.experiment_dir.join("runs")
}

fn run_dir(loaded: &LoadedExperiment, run_id: &str) -> PathBuf {
    runs_dir(loaded).join(run_id)
}

fn create_generated_run_dir(
    loaded: &LoadedExperiment,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(String, PathBuf), Vec<CliError>> {
    ensure_runs_dir(loaded)?;
    let base = generated_run_id_base(now);
    for idx in 1..=99 {
        let run_id = if idx == 1 {
            base.clone()
        } else {
            format!("{}-{:02}", base, idx)
        };
        let dir = run_dir(loaded, &run_id);
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok((run_id, dir)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(vec![err_at_path(
                    "run_artifact_write_failed",
                    format!("Failed to create run directory {}: {}", dir.display(), e),
                    &dir,
                )]);
            }
        }
    }
    Err(vec![err(
        "run_id_collision_exhausted",
        "Generated run id collided through suffix -99",
        None,
    )])
}

fn create_explicit_run_dir(
    loaded: &LoadedExperiment,
    run_id: &str,
) -> Result<PathBuf, Vec<CliError>> {
    validate_run_id(run_id)?;
    ensure_runs_dir(loaded)?;
    let dir = run_dir(loaded, run_id);
    match std::fs::create_dir(&dir) {
        Ok(()) => Ok(dir),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(vec![err_at_path(
            "run_id_exists",
            format!("Run id already exists: {}", run_id),
            &dir,
        )]),
        Err(e) => Err(vec![err_at_path(
            "run_artifact_write_failed",
            format!("Failed to create run directory {}: {}", dir.display(), e),
            &dir,
        )]),
    }
}

fn ensure_runs_dir(loaded: &LoadedExperiment) -> Result<(), Vec<CliError>> {
    let dir = runs_dir(loaded);
    if dir.exists() {
        reject_link_or_reparse(&dir, "run_artifact_link_or_reparse", None)?;
    } else {
        reject_link_or_reparse(&loaded.experiment_dir, "run_artifact_link_or_reparse", None)?;
        std::fs::create_dir_all(&dir).map_err(|e| {
            vec![err_at_path(
                "run_artifact_write_failed",
                format!("Failed to create runs directory {}: {}", dir.display(), e),
                &dir,
            )]
        })?;
    }
    Ok(())
}

fn generated_run_id_base(now: chrono::DateTime<chrono::Utc>) -> String {
    now.format("%Y%m%d-%H%M%S").to_string()
}

fn validate_run_id(run_id: &str) -> Result<(), Vec<CliError>> {
    let bytes = run_id.as_bytes();
    let valid_base = bytes.len() >= 15
        && bytes[8] == b'-'
        && bytes[..8].iter().all(u8::is_ascii_digit)
        && bytes[9..15].iter().all(u8::is_ascii_digit);
    let valid = if bytes.len() == 15 {
        valid_base
    } else if bytes.len() == 18 {
        valid_base
            && bytes[15] == b'-'
            && bytes[16..18].iter().all(u8::is_ascii_digit)
            && bytes[16..18] != *b"00"
    } else {
        false
    };
    if valid {
        Ok(())
    } else {
        Err(vec![err(
            "run_id_invalid",
            "Run id must match YYYYMMDD-HHMMSS or YYYYMMDD-HHMMSS-NN",
            None,
        )])
    }
}

fn validate_replicates(replicates: u32) -> Result<(), Vec<CliError>> {
    if (1..=100).contains(&replicates) {
        Ok(())
    } else {
        Err(vec![err(
            "replicates_invalid",
            "--replicates must be between 1 and 100",
            None,
        )])
    }
}

fn generated_seed_from_uuid(uuid: uuid::Uuid) -> u64 {
    let bytes = uuid.as_bytes();
    u64::from_le_bytes(bytes[0..8].try_into().expect("uuid slice is 8 bytes"))
}

fn default_run_artifact_paths() -> RunArtifactPaths {
    RunArtifactPaths {
        attempts: "attempts.jsonl".to_string(),
        report_json: "report.json".to_string(),
        report_markdown: "report.md".to_string(),
        prompts: "prompts".to_string(),
        outputs: "outputs".to_string(),
        transcripts: "transcripts".to_string(),
        attempt_state: "attempt-state".to_string(),
    }
}

fn report_artifact_paths(run_dir: &Path) -> ReportArtifactPaths {
    ReportArtifactPaths {
        run: run_dir.join("run.json").to_string_lossy().to_string(),
        attempts: run_dir.join("attempts.jsonl").to_string_lossy().to_string(),
        report_json: run_dir.join("report.json").to_string_lossy().to_string(),
        report_markdown: run_dir.join("report.md").to_string_lossy().to_string(),
    }
}

fn build_attempts(
    run_id: &str,
    prompts: &[PromptCase],
    variants: &[VariantMetadata],
    replicates: u32,
) -> Vec<AttemptArtifact> {
    let mut attempts = Vec::new();
    for prompt in prompts {
        for variant in variants {
            for replicate in 1..=replicates {
                attempts.push(AttemptArtifact {
                    schema_version: 1,
                    attempt_id: format!("{}__{}__r{}", prompt.id, variant.name, replicate),
                    run_id: run_id.to_string(),
                    prompt_id: prompt.id.clone(),
                    prompt_title: prompt.title.clone(),
                    prompt_line: prompt.line,
                    variant: variant.name.clone(),
                    agent_name: variant.agent_name.clone(),
                    replicate,
                    status: "planned".to_string(),
                    tags: prompt.tags.clone(),
                    expected_behaviors: prompt.expected_behaviors.clone(),
                    duration_ms: None,
                    messages: None,
                    flags: Vec::new(),
                    transcript_path: None,
                    session_id: None,
                    workgroup: None,
                    replica_root: None,
                    prompt_path: None,
                    output_path: None,
                    started_at: None,
                    completed_at: None,
                    exit_code: None,
                    provider: None,
                    provider_transcript_path: None,
                    attempt_generation: 1,
                    failure_reason: None,
                });
            }
        }
    }
    attempts
}

#[allow(clippy::too_many_arguments)]
fn build_report_artifact(
    run_id: &str,
    experiment: &str,
    run_dir: &Path,
    timestamp: &str,
    prompts: &[PromptCase],
    variants: &[VariantMetadata],
    replicates: u32,
    attempts: &[AttemptArtifact],
    dry_run: bool,
    workgroup: Option<&RunWorkgroupArtifact>,
) -> ReportArtifact {
    let status = summarize_run_status(attempts, dry_run);
    ReportArtifact {
        schema_version: 1,
        run_id: run_id.to_string(),
        experiment: experiment.to_string(),
        format: "json".to_string(),
        status,
        dry_run,
        generated_at: timestamp.to_string(),
        summary: count_attempt_statuses(prompts.len(), variants.len(), replicates, attempts),
        variants: variants
            .iter()
            .map(|variant| {
                let variant_attempts: Vec<AttemptArtifact> = attempts
                    .iter()
                    .filter(|attempt| attempt.variant == variant.name)
                    .cloned()
                    .collect();
                let mut counts =
                    count_attempt_statuses(prompts.len(), 1, replicates, &variant_attempts);
                counts.variant_count = 1;
                ReportVariantArtifact {
                    name: variant.name.clone(),
                    agent_name: variant.agent_name.clone(),
                    planned_attempt_count: counts.planned_attempt_count,
                    running_attempt_count: counts.running_attempt_count,
                    completed_attempt_count: counts.completed_attempt_count,
                    failed_attempt_count: counts.failed_attempt_count,
                    timed_out_attempt_count: counts.timed_out_attempt_count,
                    inconclusive_attempt_count: counts.inconclusive_attempt_count,
                    cancelled_attempt_count: counts.cancelled_attempt_count,
                }
            })
            .collect(),
        prompts: prompts
            .iter()
            .map(|prompt| {
                let prompt_attempts: Vec<AttemptArtifact> = attempts
                    .iter()
                    .filter(|attempt| attempt.prompt_id == prompt.id)
                    .cloned()
                    .collect();
                let mut counts =
                    count_attempt_statuses(1, variants.len(), replicates, &prompt_attempts);
                counts.prompt_count = 1;
                ReportPromptArtifact {
                    id: prompt.id.clone(),
                    title: prompt.title.clone(),
                    planned_attempt_count: counts.planned_attempt_count,
                    running_attempt_count: counts.running_attempt_count,
                    completed_attempt_count: counts.completed_attempt_count,
                    failed_attempt_count: counts.failed_attempt_count,
                    timed_out_attempt_count: counts.timed_out_attempt_count,
                    inconclusive_attempt_count: counts.inconclusive_attempt_count,
                    cancelled_attempt_count: counts.cancelled_attempt_count,
                }
            })
            .collect(),
        artifact_paths: report_artifact_paths(run_dir),
        notes: report_notes(dry_run, workgroup),
    }
}

fn count_attempt_statuses(
    prompt_count: usize,
    variant_count: usize,
    replicates: u32,
    attempts: &[AttemptArtifact],
) -> ReportSummaryArtifact {
    let mut summary = ReportSummaryArtifact {
        prompt_count,
        variant_count,
        replicates,
        attempt_count: attempts.len(),
        planned_attempt_count: 0,
        running_attempt_count: 0,
        completed_attempt_count: 0,
        failed_attempt_count: 0,
        timed_out_attempt_count: 0,
        inconclusive_attempt_count: 0,
        cancelled_attempt_count: 0,
    };
    for attempt in attempts {
        match attempt.status.as_str() {
            "planned" => summary.planned_attempt_count += 1,
            "running" => summary.running_attempt_count += 1,
            "completed" => summary.completed_attempt_count += 1,
            "failed" => summary.failed_attempt_count += 1,
            "timed_out" => summary.timed_out_attempt_count += 1,
            "inconclusive" => summary.inconclusive_attempt_count += 1,
            "cancelled" => summary.cancelled_attempt_count += 1,
            _ => {}
        }
    }
    summary
}

fn summarize_run_status(attempts: &[AttemptArtifact], dry_run: bool) -> String {
    if dry_run {
        return "dry_run".to_string();
    }
    for status in [
        "running",
        "failed",
        "timed_out",
        "inconclusive",
        "cancelled",
        "completed",
    ] {
        if attempts.iter().any(|attempt| attempt.status == status) {
            return status.to_string();
        }
    }
    "completed".to_string()
}

fn report_notes(dry_run: bool, workgroup: Option<&RunWorkgroupArtifact>) -> Vec<String> {
    if dry_run {
        return vec![
            "Dry-run report contains planned attempts only.".to_string(),
            "No sessions, messages, transcripts, scoring, or winner selection were produced."
                .to_string(),
        ];
    }
    let mut notes = Vec::new();
    if let Some(workgroup) = workgroup {
        notes.push(format!("Run workgroup retained at {}.", workgroup.path));
    }
    notes.push("Transcript capture is best-effort.".to_string());
    notes.push("No scoring or winner selection performed.".to_string());
    notes
}

fn render_report_markdown(report: &ReportArtifact) -> String {
    let mut markdown = String::new();
    if report.dry_run {
        markdown.push_str("# Role Experiment Dry Run Report\n\n");
    } else {
        markdown.push_str("# Role Experiment Run Report\n\n");
    }
    markdown.push_str(&format!("Experiment: {}\n", report.experiment));
    markdown.push_str(&format!("Run ID: {}\n", report.run_id));
    markdown.push_str(&format!("Status: {}\n\n", report.status));
    markdown.push_str("## Summary\n\n");
    markdown.push_str(&format!("- Prompts: {}\n", report.summary.prompt_count));
    markdown.push_str(&format!("- Variants: {}\n", report.summary.variant_count));
    markdown.push_str(&format!("- Replicates: {}\n", report.summary.replicates));
    markdown.push_str(&format!(
        "- Planned attempts: {}\n\n",
        report.summary.planned_attempt_count
    ));
    markdown.push_str("## Status Counts\n\n");
    markdown.push_str("| Status | Attempts |\n|---|---:|\n");
    for (status, count) in [
        ("planned", report.summary.planned_attempt_count),
        ("running", report.summary.running_attempt_count),
        ("completed", report.summary.completed_attempt_count),
        ("failed", report.summary.failed_attempt_count),
        ("timed_out", report.summary.timed_out_attempt_count),
        ("inconclusive", report.summary.inconclusive_attempt_count),
        ("cancelled", report.summary.cancelled_attempt_count),
    ] {
        markdown.push_str(&format!("| {} | {} |\n", status, count));
    }
    markdown.push('\n');
    markdown.push_str("## Variants\n\n");
    markdown.push_str("| Variant | Agent | Planned | Running | Completed | Failed | Timed out | Inconclusive | Cancelled |\n|---|---|---:|---:|---:|---:|---:|---:|---:|\n");
    for variant in &report.variants {
        markdown.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            markdown_table_cell(&variant.name),
            markdown_table_cell(&variant.agent_name),
            variant.planned_attempt_count,
            variant.running_attempt_count,
            variant.completed_attempt_count,
            variant.failed_attempt_count,
            variant.timed_out_attempt_count,
            variant.inconclusive_attempt_count,
            variant.cancelled_attempt_count
        ));
    }
    markdown.push_str("\n## Prompts\n\n");
    markdown.push_str("| Prompt | Title | Planned | Running | Completed | Failed | Timed out | Inconclusive | Cancelled |\n|---|---|---:|---:|---:|---:|---:|---:|---:|\n");
    for prompt in &report.prompts {
        markdown.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            markdown_table_cell(&prompt.id),
            markdown_table_cell(&prompt.title),
            prompt.planned_attempt_count,
            prompt.running_attempt_count,
            prompt.completed_attempt_count,
            prompt.failed_attempt_count,
            prompt.timed_out_attempt_count,
            prompt.inconclusive_attempt_count,
            prompt.cancelled_attempt_count
        ));
    }
    markdown.push_str("\n## Notes\n\n");
    for note in &report.notes {
        markdown.push_str(&format!("- {}\n", note));
    }
    markdown
}

fn markdown_table_cell(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace(['\r', '\n'], " ")
}

fn validate_report_artifacts(
    loaded: &LoadedExperiment,
    run_id: &str,
    run: &RunArtifact,
    report: &ReportArtifact,
) -> Result<(), Vec<CliError>> {
    let mut errors = Vec::new();
    if run.schema_version != 1 || report.schema_version != 1 {
        errors.push(err(
            "run_artifact_mismatch",
            "Run artifacts must use schemaVersion 1",
            None,
        ));
    }
    if run.run_id != run_id || report.run_id != run_id || run.run_id != report.run_id {
        errors.push(err(
            "run_artifact_mismatch",
            "Run artifact runId values do not match",
            None,
        ));
    }
    if run.experiment != loaded.experiment.name || report.experiment != loaded.experiment.name {
        errors.push(err(
            "run_artifact_mismatch",
            "Run artifact experiment values do not match",
            None,
        ));
    }
    if !valid_run_status(&run.status) {
        errors.push(err(
            "report_run_status_unsupported",
            "Run artifact status is not supported",
            None,
        ));
    }
    if !valid_run_status(&report.status)
        || report.status != run.status
        || report.dry_run != run.dry_run
    {
        errors.push(err(
            "run_artifact_mismatch",
            "Report artifact status does not match run artifact",
            None,
        ));
    }
    if !run.dry_run && run.execution.is_none() {
        errors.push(err(
            "run_artifact_mismatch",
            "Real run artifacts must include execution metadata",
            None,
        ));
    }
    if !run.dry_run && run.workgroup.is_none() {
        errors.push(err(
            "run_artifact_mismatch",
            "Real run artifacts must include workgroup metadata",
            None,
        ));
    }
    if report.format != "json" {
        errors.push(err(
            "run_artifact_mismatch",
            "Report artifact format must be json",
            None,
        ));
    }
    let expected_artifact_paths = report_artifact_paths(&run_dir(loaded, run_id));
    if report.artifact_paths != expected_artifact_paths {
        errors.push(err(
            "run_artifact_mismatch",
            "Report artifactPaths do not match the run artifact paths",
            None,
        ));
    }
    if report.summary.prompt_count != run.suite.prompt_count
        || report.summary.variant_count != run.variants.len()
        || report.summary.replicates != run.replicates
        || report.summary.attempt_count != run.attempt_count
        || report_status_total(&report.summary) != run.attempt_count
    {
        errors.push(err(
            "run_artifact_mismatch",
            "Report summary does not match run artifact",
            None,
        ));
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn report_status_total(summary: &ReportSummaryArtifact) -> usize {
    summary.planned_attempt_count
        + summary.running_attempt_count
        + summary.completed_attempt_count
        + summary.failed_attempt_count
        + summary.timed_out_attempt_count
        + summary.inconclusive_attempt_count
        + summary.cancelled_attempt_count
}

fn valid_run_status(status: &str) -> bool {
    matches!(
        status,
        "dry_run" | "running" | "completed" | "failed" | "timed_out" | "inconclusive" | "cancelled"
    )
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
        line: None,
        field: None,
        path: None,
    }
}

fn err_at_line(
    code: &'static str,
    message: impl Into<String>,
    line: usize,
    field: Option<&str>,
) -> CliError {
    CliError {
        code: code.to_string(),
        message: message.into(),
        variant: None,
        line: Some(line),
        field: field.map(str::to_string),
        path: None,
    }
}

fn err_at_path(code: &'static str, message: impl Into<String>, path: &Path) -> CliError {
    CliError {
        code: code.to_string(),
        message: message.into(),
        variant: None,
        line: None,
        field: None,
        path: Some(path.to_string_lossy().to_string()),
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

    fn resume_fixture() -> (
        tempfile::TempDir,
        LoadedExperiment,
        ParsedPromptSuite,
        Vec<VariantMetadata>,
        RunArgs,
        PathBuf,
        String,
        ReportArtifact,
    ) {
        let (tmp, loaded) = loaded_fixture("alpha");
        let run_id = "20260601-181500".to_string();
        let run_dir = runs_dir(&loaded).join(&run_id);
        std::fs::create_dir_all(run_dir.join("attempt-state")).expect("run dirs");
        let workgroup_dir = loaded.workspace_dir.join("wg-1-role-exp-exp");
        std::fs::create_dir_all(&workgroup_dir).expect("workgroup dir");

        let suite = ParsedPromptSuite {
            path: loaded.experiment_dir.join("suite.md"),
            sha256: sha256_hex(b"prompt suite"),
            prompts: vec![
                PromptCase {
                    id: "prompt-a".to_string(),
                    title: "Prompt A".to_string(),
                    prompt: "Do A".to_string(),
                    tags: vec!["one".to_string()],
                    expected_behaviors: vec!["A happens".to_string()],
                    line: 1,
                },
                PromptCase {
                    id: "prompt-b".to_string(),
                    title: "Prompt B".to_string(),
                    prompt: "Do B".to_string(),
                    tags: vec!["two".to_string()],
                    expected_behaviors: vec!["B happens".to_string()],
                    line: 7,
                },
            ],
        };
        let variants = loaded
            .experiment
            .variants
            .iter()
            .map(|name| load_variant_metadata(&loaded, name).expect("variant metadata"))
            .collect::<Vec<_>>();
        let attempts = build_attempts(&run_id, &suite.prompts, &variants, 1);
        let workgroup = RunWorkgroupArtifact {
            name: "wg-1-role-exp-exp".to_string(),
            path: std::fs::canonicalize(&workgroup_dir)
                .expect("canonical workgroup")
                .to_string_lossy()
                .to_string(),
            retained: true,
            cleanup_supported: false,
            run_id: run_id.clone(),
            experiment: loaded.experiment.name.clone(),
        };
        let report = build_report_artifact(
            &run_id,
            &loaded.experiment.name,
            &run_dir,
            "2026-06-01T18:15:00Z",
            &suite.prompts,
            &variants,
            1,
            &attempts,
            false,
            Some(&workgroup),
        );
        let run = RunArtifact {
            schema_version: 1,
            run_id: run_id.clone(),
            project: loaded.experiment.project.clone(),
            experiment: loaded.experiment.name.clone(),
            status: report.status.clone(),
            dry_run: false,
            created_at: "2026-06-01T18:15:00Z".to_string(),
            updated_at: "2026-06-01T18:15:00Z".to_string(),
            suite: RunSuiteArtifact {
                path: suite.path.to_string_lossy().to_string(),
                sha256: suite.sha256.clone(),
                prompt_count: suite.prompts.len(),
            },
            seed: 123,
            seed_provided: false,
            replicates: 1,
            variants: variants
                .iter()
                .map(|variant| RunVariantArtifact {
                    name: variant.name.clone(),
                    agent_name: variant.agent_name.clone(),
                    role_sha256: variant.role_sha256.clone(),
                    role_path: variant.role_path.clone(),
                })
                .collect(),
            attempt_count: attempts.len(),
            artifacts: default_run_artifact_paths(),
            execution: Some(RunExecutionArtifact {
                provider: "auto".to_string(),
                attempt_timeout_secs: 900,
                max_parallel: 1,
                started_at: Some("2026-06-01T18:15:00Z".to_string()),
                completed_at: None,
                cancellation_requested: false,
                executor: "fake".to_string(),
            }),
            workgroup: Some(workgroup),
            warnings: Vec::new(),
        };

        std::fs::write(
            run_dir.join("run.json"),
            serde_json::to_string_pretty(&run).expect("run json"),
        )
        .expect("write run");
        std::fs::write(
            run_dir.join("attempts.jsonl"),
            attempts
                .iter()
                .map(|attempt| serde_json::to_string(attempt).expect("attempt json"))
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .expect("write attempts");
        std::fs::write(
            run_dir.join("report.json"),
            serde_json::to_string_pretty(&report).expect("report json"),
        )
        .expect("write report");
        std::fs::write(run_dir.join("report.md"), render_report_markdown(&report))
            .expect("write report markdown");

        let args = RunArgs {
            project: loaded.experiment.project.clone(),
            experiment: loaded.experiment.name.clone(),
            prompt_suite: suite.path.to_string_lossy().to_string(),
            replicates: 1,
            seed: None,
            run_id: Some(run_id.clone()),
            dry_run: false,
            execute: true,
            provider: "auto".to_string(),
            attempt_timeout_secs: 900,
            max_parallel: 1,
            retain_workgroup: Some(true),
            resume_run: true,
            retry_failed: true,
            fake_executor: true,
        };

        (tmp, loaded, suite, variants, args, run_dir, run_id, report)
    }

    fn assert_resume_report_tamper_rejected_without_mutation(
        mut tamper: impl FnMut(&mut ReportArtifact),
    ) {
        let (_tmp, loaded, suite, variants, args, run_dir, run_id, mut report) = resume_fixture();
        tamper(&mut report);
        let report_path = run_dir.join("report.json");
        std::fs::write(
            &report_path,
            serde_json::to_string_pretty(&report).expect("tampered report json"),
        )
        .expect("write tampered report");
        let before = std::fs::read_to_string(&report_path).expect("read tampered report");

        let errors = load_and_validate_resume_artifacts(
            &args, &loaded, &suite, &run_id, &run_dir, &variants,
        )
        .expect_err("tampered report should be rejected");

        assert!(errors
            .iter()
            .any(|error| error.code == "run_artifact_mismatch"));
        assert_eq!(
            before,
            std::fs::read_to_string(&report_path).expect("read report after validation")
        );
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
    fn init_rejects_linked_experiments_parent_before_metadata_write() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project = tmp.path().join("project");
        let workspace_dir = project.join(".ac");
        let source_matrix = workspace_dir.join("_agent_alpha");
        std::fs::create_dir_all(&source_matrix).expect("source matrix");
        std::fs::write(source_matrix.join("Role.md"), "source role\n").expect("source role");

        let outside = tmp.path().join("outside-experiments");
        std::fs::create_dir_all(&outside).expect("outside experiments");
        let experiments = workspace_dir.join("experiments");

        #[cfg(windows)]
        let link_result = std::os::windows::fs::symlink_dir(&outside, &experiments);
        #[cfg(unix)]
        let link_result = std::os::unix::fs::symlink(&outside, &experiments);

        if link_result.is_err() {
            return;
        }

        let errors = match init_for_project_path(
            InitArgs {
                project: "project".to_string(),
                name: "exp".to_string(),
                source_agent: "alpha".to_string(),
                variants: vec!["control".to_string(), "test".to_string()],
            },
            &project,
        ) {
            Ok(_) => panic!("linked experiments parent should be rejected"),
            Err(errors) => errors,
        };

        assert!(errors
            .iter()
            .any(|e| e.code == "experiment_metadata_link_or_reparse"));
        assert!(
            !outside.join("exp").exists(),
            "init must not write experiment metadata through linked experiments parent"
        );
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

    #[test]
    fn resume_rejects_report_variant_identity_tamper_without_mutation() {
        assert_resume_report_tamper_rejected_without_mutation(|report| {
            report.variants[0].name = "renamed-control".to_string();
        });
    }

    #[test]
    fn resume_rejects_report_prompt_identity_tamper_without_mutation() {
        assert_resume_report_tamper_rejected_without_mutation(|report| {
            report.prompts[0].id = "renamed-prompt".to_string();
            report.prompts[0].title = "Renamed Prompt".to_string();
        });
    }

    #[test]
    fn run_id_validation_accepts_only_phase2_shape() {
        assert!(validate_run_id("20260601-181500").is_ok());
        assert!(validate_run_id("20260601-181500-01").is_ok());
        assert!(validate_run_id("20260601-181500-99").is_ok());
        assert!(validate_run_id("2026060-181500").is_err());
        assert!(validate_run_id("20260601181500").is_err());
        assert!(validate_run_id("20260601_181500").is_err());
        assert!(validate_run_id("20260601-18150x").is_err());
        assert!(validate_run_id("20260601-181500-00").is_err());
        assert!(validate_run_id("20260601-181500-100").is_err());
    }

    #[test]
    fn generated_run_id_suffixes_existing_directories() {
        let (_tmp, loaded) = loaded_fixture("alpha");
        std::fs::create_dir_all(runs_dir(&loaded).join("20260601-181500")).unwrap();
        std::fs::create_dir_all(runs_dir(&loaded).join("20260601-181500-02")).unwrap();
        let now = chrono::DateTime::parse_from_rfc3339("2026-06-01T18:15:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);

        let (run_id, run_dir) = create_generated_run_dir(&loaded, now).unwrap();

        assert_eq!(run_id, "20260601-181500-03");
        assert!(run_dir.is_dir());
    }
}
