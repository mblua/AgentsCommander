use clap::builder::BoolishValueParser;
use clap::{Args, Subcommand};

#[derive(Args)]
pub struct RoleExperimentArgs {
    #[command(subcommand)]
    pub(crate) command: RoleExperimentCommand,
}

#[derive(Subcommand)]
pub(crate) enum RoleExperimentCommand {
    Init(InitArgs),
    List(ProjectArgs),
    Show(ExperimentArgs),
    Variant(VariantArgs),
    Validate(ValidateArgs),
    Run(RunArgs),
    Report(ReportArgs),
}

#[derive(Args)]
pub(crate) struct ProjectArgs {
    #[arg(long)]
    pub(crate) project: String,
}

#[derive(Args)]
pub(crate) struct ExperimentArgs {
    #[arg(long)]
    pub(crate) project: String,
    #[arg(long)]
    pub(crate) experiment: String,
}

#[derive(Args)]
pub(crate) struct InitArgs {
    #[arg(long)]
    pub(crate) project: String,
    #[arg(long)]
    pub(crate) name: String,
    #[arg(long = "source-agent")]
    pub(crate) source_agent: String,
    #[arg(long, value_delimiter = ',')]
    pub(crate) variants: Vec<String>,
}

#[derive(Args)]
pub(crate) struct ValidateArgs {
    #[arg(long)]
    pub(crate) project: String,
    #[arg(long)]
    pub(crate) experiment: String,
    #[arg(long = "prompt-suite")]
    pub(crate) prompt_suite: Option<String>,
}

#[derive(Args)]
pub(crate) struct RunArgs {
    #[arg(long)]
    pub(crate) project: String,
    #[arg(long)]
    pub(crate) experiment: String,
    #[arg(long = "prompt-suite")]
    pub(crate) prompt_suite: String,
    #[arg(long, default_value_t = 1)]
    pub(crate) replicates: u32,
    #[arg(long)]
    pub(crate) seed: Option<u64>,
    #[arg(long = "run-id")]
    pub(crate) run_id: Option<String>,
    #[arg(long = "dry-run")]
    pub(crate) dry_run: bool,
    #[arg(long = "execute")]
    pub(crate) execute: bool,
    #[arg(long = "provider", default_value = "auto")]
    pub(crate) provider: String,
    #[arg(long = "attempt-timeout-secs", default_value_t = 900)]
    pub(crate) attempt_timeout_secs: u64,
    #[arg(long = "max-parallel", default_value_t = 1)]
    pub(crate) max_parallel: u32,
    #[arg(
        long = "retain-room",
        alias = "retain-workgroup",
        value_name = "RETAIN_ROOM",
        value_parser = BoolishValueParser::new()
    )]
    pub(crate) retain_workgroup: Option<bool>,
    #[arg(long = "resume-run")]
    pub(crate) resume_run: bool,
    #[arg(long = "retry-failed")]
    pub(crate) retry_failed: bool,
    #[arg(long = "fake-executor", hide = true)]
    pub(crate) fake_executor: bool,
}

#[derive(Args)]
pub(crate) struct ReportArgs {
    #[arg(long)]
    pub(crate) project: String,
    #[arg(long)]
    pub(crate) experiment: String,
    #[arg(long = "run-id")]
    pub(crate) run_id: String,
    #[arg(long, default_value = "text")]
    pub(crate) format: String,
}

#[derive(Args)]
pub(crate) struct VariantArgs {
    #[command(subcommand)]
    pub(crate) command: VariantCommand,
}

#[derive(Subcommand)]
pub(crate) enum VariantCommand {
    Set(VariantSetArgs),
    Diff(VariantDiffArgs),
}

#[derive(Args)]
pub(crate) struct VariantSetArgs {
    #[arg(long)]
    pub(crate) project: String,
    #[arg(long)]
    pub(crate) experiment: String,
    #[arg(long)]
    pub(crate) variant: String,
    #[arg(long = "role-file")]
    pub(crate) role_file: String,
}

#[derive(Args)]
pub(crate) struct VariantDiffArgs {
    #[arg(long)]
    pub(crate) project: String,
    #[arg(long)]
    pub(crate) experiment: String,
    #[arg(long)]
    pub(crate) variant: String,
    #[arg(long = "against", default_value = "control")]
    pub(crate) against: String,
}
