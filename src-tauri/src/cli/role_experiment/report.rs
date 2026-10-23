use super::support::{err, err_at_path};
use super::validation::reject_link_or_reparse;
use super::AttemptArtifact;
use super::CliError;
use super::LoadedExperiment;
use super::PromptCase;
use super::ReportArtifact;
use super::ReportArtifactPaths;
use super::ReportPromptArtifact;
use super::ReportSummaryArtifact;
use super::ReportVariantArtifact;
use super::RunArtifact;
use super::RunArtifactPaths;
use super::RunWorkgroupArtifact;
use super::VariantMetadata;
use std::path::Path;
use std::path::PathBuf;

pub(super) fn experiments_dir(ac_root: &Path) -> PathBuf {
    ac_root.join("experiments")
}

pub(super) fn runs_dir(loaded: &LoadedExperiment) -> PathBuf {
    loaded.experiment_dir.join("runs")
}

pub(super) fn run_dir(loaded: &LoadedExperiment, run_id: &str) -> PathBuf {
    runs_dir(loaded).join(run_id)
}

pub(super) fn create_generated_run_dir(
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

pub(super) fn create_explicit_run_dir(
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

pub(super) fn validate_run_id(run_id: &str) -> Result<(), Vec<CliError>> {
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

pub(super) fn validate_replicates(replicates: u32) -> Result<(), Vec<CliError>> {
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

pub(super) fn generated_seed_from_uuid(uuid: uuid::Uuid) -> u64 {
    let bytes = uuid.as_bytes();
    u64::from_le_bytes(bytes[0..8].try_into().expect("uuid slice is 8 bytes"))
}

pub(super) fn default_run_artifact_paths() -> RunArtifactPaths {
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

pub(super) fn report_artifact_paths(run_dir: &Path) -> ReportArtifactPaths {
    ReportArtifactPaths {
        run: run_dir.join("run.json").to_string_lossy().to_string(),
        attempts: run_dir.join("attempts.jsonl").to_string_lossy().to_string(),
        report_json: run_dir.join("report.json").to_string_lossy().to_string(),
        report_markdown: run_dir.join("report.md").to_string_lossy().to_string(),
    }
}

pub(super) fn build_attempts(
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
pub(super) fn build_report_artifact(
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

pub(super) fn summarize_run_status(attempts: &[AttemptArtifact], dry_run: bool) -> String {
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
        notes.push(format!("Run room retained at {}.", workgroup.path));
    }
    notes.push("Transcript capture is best-effort.".to_string());
    notes.push("No scoring or winner selection performed.".to_string());
    notes
}

pub(super) fn render_report_markdown(report: &ReportArtifact) -> String {
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
    markdown.push_str(
        "| Variant | Agent | Planned | Running | Completed | Failed | Timed out | \
         Inconclusive | Cancelled |\n|---|---|---:|---:|---:|---:|---:|---:|---:|\n",
    );
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
    markdown.push_str(
        "| Prompt | Title | Planned | Running | Completed | Failed | Timed out | \
         Inconclusive | Cancelled |\n|---|---|---:|---:|---:|---:|---:|---:|---:|\n",
    );
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

pub(super) fn validate_report_artifacts(
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
            "Real run artifacts must include room metadata",
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
