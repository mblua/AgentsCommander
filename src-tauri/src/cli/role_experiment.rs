use std::path::PathBuf;

use clap::builder::BoolishValueParser;
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};

mod commands;
mod fake_run;
mod prompt_suite;
mod report;
mod resume;
mod storage;
mod support;
mod validation;

use commands::init;
use commands::list;
use commands::report;
use commands::run;
use commands::show;
use commands::validate;
use commands::variant_diff;
use commands::variant_set;
#[cfg(test)]
use commands::{init_for_project_path, variant_diff_loaded};
#[cfg(test)]
use report::{
    build_attempts, build_report_artifact, create_generated_run_dir, default_run_artifact_paths,
    render_report_markdown, runs_dir, validate_run_id,
};
#[cfg(test)]
use resume::load_and_validate_resume_artifacts;
#[cfg(test)]
use storage::{load_variant_metadata, role_exp_room_dir_name, write_json};
#[cfg(test)]
use support::{sha256_hex, variant_agent_name};
#[cfg(test)]
use validation::{
    reject_link_or_reparse, validate_loaded_experiment_source_metadata,
    validate_variant_metadata_paths,
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
    #[arg(
        long = "retain-room",
        alias = "retain-workgroup",
        value_name = "RETAIN_ROOM",
        value_parser = BoolishValueParser::new()
    )]
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
    ac_root: PathBuf,
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

#[cfg(test)]
mod tests;
