#[cfg(test)]
use std::path::PathBuf;

mod arguments;
mod artifacts;
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

pub use arguments::RoleExperimentArgs;
pub(crate) use arguments::{
    ExperimentArgs, InitArgs, ProjectArgs, ReportArgs, RunArgs, ValidateArgs, VariantDiffArgs,
    VariantSetArgs,
};
use arguments::{RoleExperimentCommand, VariantCommand};
pub(crate) use artifacts::{
    AttemptArtifact, AttemptExecutionResult, AttemptOwnershipArtifact, CliEnvelope, CliError,
    CliWarning, CommandOutput, DiffStats, ExecutionPlan, ExperimentMetadata, ExperimentValidation,
    LoadedExperiment, ParsedPromptSuite, PromptCase, ReportArtifact, ReportArtifactPaths,
    ReportPromptArtifact, ReportSummaryArtifact, ReportVariantArtifact, RunArtifact,
    RunArtifactPaths, RunExecutionArtifact, RunSuiteArtifact, RunVariantArtifact,
    RunWorkgroupArtifact, VariantMetadata,
};

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

    let code = i32::from(!envelope.ok);
    match serde_json::to_string_pretty(&envelope) {
        Ok(json) => crate::cli_println!("{}", json),
        Err(e) => eprintln!("Error: failed to serialize role-experiment output: {e}"),
    }
    code
}

#[cfg(test)]
mod tests;
