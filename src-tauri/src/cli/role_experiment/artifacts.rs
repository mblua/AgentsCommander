use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CliEnvelope {
    pub(crate) ok: bool,
    pub(crate) command: &'static str,
    pub(crate) data: Option<serde_json::Value>,
    pub(crate) warnings: Vec<CliWarning>,
    pub(crate) errors: Vec<CliError>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CliWarning {
    pub(crate) code: String,
    pub(crate) message: String,
    pub(crate) variant: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CliError {
    pub(crate) code: String,
    pub(crate) message: String,
    pub(crate) variant: Option<String>,
    pub(crate) line: Option<usize>,
    pub(crate) field: Option<String>,
    pub(crate) path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExperimentMetadata {
    pub(crate) schema_version: u32,
    pub(crate) name: String,
    pub(crate) project: String,
    pub(crate) source_agent: String,
    pub(crate) source_matrix_path: String,
    pub(crate) source_role_path: String,
    pub(crate) source_role_sha256: String,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
    pub(crate) variants: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VariantMetadata {
    pub(crate) schema_version: u32,
    pub(crate) name: String,
    pub(crate) source_agent: String,
    pub(crate) agent_name: String,
    pub(crate) matrix_path: String,
    pub(crate) role_path: String,
    pub(crate) role_sha256: String,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PromptCase {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) prompt: String,
    pub(crate) tags: Vec<String>,
    pub(crate) expected_behaviors: Vec<String>,
    pub(crate) line: usize,
}

#[derive(Clone)]
pub(crate) struct ParsedPromptSuite {
    pub(crate) path: PathBuf,
    pub(crate) sha256: String,
    pub(crate) prompts: Vec<PromptCase>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunArtifact {
    pub(crate) schema_version: u32,
    pub(crate) run_id: String,
    pub(crate) project: String,
    pub(crate) experiment: String,
    pub(crate) status: String,
    pub(crate) dry_run: bool,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
    pub(crate) suite: RunSuiteArtifact,
    pub(crate) seed: u64,
    pub(crate) seed_provided: bool,
    pub(crate) replicates: u32,
    pub(crate) variants: Vec<RunVariantArtifact>,
    pub(crate) attempt_count: usize,
    pub(crate) artifacts: RunArtifactPaths,
    pub(crate) execution: Option<RunExecutionArtifact>,
    pub(crate) workgroup: Option<RunWorkgroupArtifact>,
    pub(crate) warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunSuiteArtifact {
    pub(crate) path: String,
    pub(crate) sha256: String,
    pub(crate) prompt_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunVariantArtifact {
    pub(crate) name: String,
    pub(crate) agent_name: String,
    pub(crate) role_sha256: String,
    pub(crate) role_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunArtifactPaths {
    pub(crate) attempts: String,
    pub(crate) report_json: String,
    pub(crate) report_markdown: String,
    pub(crate) prompts: String,
    pub(crate) outputs: String,
    pub(crate) transcripts: String,
    pub(crate) attempt_state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunExecutionArtifact {
    pub(crate) provider: String,
    pub(crate) attempt_timeout_secs: u64,
    pub(crate) max_parallel: u32,
    pub(crate) started_at: Option<String>,
    pub(crate) completed_at: Option<String>,
    pub(crate) cancellation_requested: bool,
    pub(crate) executor: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunWorkgroupArtifact {
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) retained: bool,
    pub(crate) cleanup_supported: bool,
    pub(crate) run_id: String,
    pub(crate) experiment: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AttemptArtifact {
    pub(crate) schema_version: u32,
    pub(crate) attempt_id: String,
    pub(crate) run_id: String,
    pub(crate) prompt_id: String,
    pub(crate) prompt_title: String,
    pub(crate) prompt_line: usize,
    pub(crate) variant: String,
    pub(crate) agent_name: String,
    pub(crate) replicate: u32,
    pub(crate) status: String,
    pub(crate) tags: Vec<String>,
    pub(crate) expected_behaviors: Vec<String>,
    pub(crate) duration_ms: Option<u64>,
    pub(crate) messages: Option<usize>,
    pub(crate) flags: Vec<String>,
    pub(crate) transcript_path: Option<String>,
    pub(crate) session_id: Option<String>,
    pub(crate) workgroup: Option<String>,
    pub(crate) replica_root: Option<String>,
    pub(crate) prompt_path: Option<String>,
    pub(crate) output_path: Option<String>,
    pub(crate) started_at: Option<String>,
    pub(crate) completed_at: Option<String>,
    pub(crate) exit_code: Option<i32>,
    pub(crate) provider: Option<String>,
    pub(crate) provider_transcript_path: Option<String>,
    pub(crate) attempt_generation: u32,
    pub(crate) failure_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AttemptOwnershipArtifact {
    pub(crate) schema_version: u32,
    pub(crate) run_id: String,
    pub(crate) attempt_id: String,
    pub(crate) attempt_generation: u32,
    pub(crate) session_id: Option<String>,
    pub(crate) workgroup_path: String,
    pub(crate) replica_root: String,
    pub(crate) prompt_path: String,
    pub(crate) output_path: String,
    pub(crate) started_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReportArtifact {
    pub(crate) schema_version: u32,
    pub(crate) run_id: String,
    pub(crate) experiment: String,
    pub(crate) format: String,
    pub(crate) status: String,
    pub(crate) dry_run: bool,
    pub(crate) generated_at: String,
    pub(crate) summary: ReportSummaryArtifact,
    pub(crate) variants: Vec<ReportVariantArtifact>,
    pub(crate) prompts: Vec<ReportPromptArtifact>,
    pub(crate) artifact_paths: ReportArtifactPaths,
    pub(crate) notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReportSummaryArtifact {
    pub(crate) prompt_count: usize,
    pub(crate) variant_count: usize,
    pub(crate) replicates: u32,
    pub(crate) attempt_count: usize,
    pub(crate) planned_attempt_count: usize,
    pub(crate) running_attempt_count: usize,
    pub(crate) completed_attempt_count: usize,
    pub(crate) failed_attempt_count: usize,
    pub(crate) timed_out_attempt_count: usize,
    pub(crate) inconclusive_attempt_count: usize,
    pub(crate) cancelled_attempt_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReportVariantArtifact {
    pub(crate) name: String,
    pub(crate) agent_name: String,
    pub(crate) planned_attempt_count: usize,
    pub(crate) running_attempt_count: usize,
    pub(crate) completed_attempt_count: usize,
    pub(crate) failed_attempt_count: usize,
    pub(crate) timed_out_attempt_count: usize,
    pub(crate) inconclusive_attempt_count: usize,
    pub(crate) cancelled_attempt_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReportPromptArtifact {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) planned_attempt_count: usize,
    pub(crate) running_attempt_count: usize,
    pub(crate) completed_attempt_count: usize,
    pub(crate) failed_attempt_count: usize,
    pub(crate) timed_out_attempt_count: usize,
    pub(crate) inconclusive_attempt_count: usize,
    pub(crate) cancelled_attempt_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReportArtifactPaths {
    pub(crate) run: String,
    pub(crate) attempts: String,
    pub(crate) report_json: String,
    pub(crate) report_markdown: String,
}

pub(crate) struct CommandOutput {
    pub(crate) data: serde_json::Value,
    pub(crate) warnings: Vec<CliWarning>,
    pub(crate) errors: Vec<CliError>,
}

pub(crate) struct LoadedExperiment {
    pub(crate) ac_root: PathBuf,
    pub(crate) experiment_dir: PathBuf,
    pub(crate) experiment: ExperimentMetadata,
}

pub(crate) struct ExecutionPlan {
    pub(crate) run_id: String,
    pub(crate) run_dir: PathBuf,
    pub(crate) suite: ParsedPromptSuite,
    pub(crate) attempts: Vec<AttemptArtifact>,
    pub(crate) variants: Vec<VariantMetadata>,
    pub(crate) workgroup: RunWorkgroupArtifact,
    pub(crate) resume_run_artifact: Option<RunArtifact>,
}

pub(crate) struct AttemptExecutionResult {
    pub(crate) attempt_id: String,
    pub(crate) status: String,
    pub(crate) duration_ms: Option<u64>,
    pub(crate) transcript_path: Option<String>,
    pub(crate) provider_transcript_path: Option<String>,
    pub(crate) output_path: Option<String>,
    pub(crate) failure_reason: Option<String>,
    pub(crate) exit_code: Option<i32>,
    pub(crate) flags: Vec<String>,
}

pub(crate) struct ExperimentValidation {
    pub(crate) warnings: Vec<CliWarning>,
    pub(crate) errors: Vec<CliError>,
    pub(crate) variant_metas: Vec<VariantMetadata>,
}

#[derive(Debug, Clone)]
pub(crate) struct DiffStats {
    pub(crate) added_lines: usize,
    pub(crate) removed_lines: usize,
    pub(crate) changed_lines: usize,
    pub(crate) identical: bool,
}
