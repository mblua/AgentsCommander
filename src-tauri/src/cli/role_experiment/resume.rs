use super::arguments::RunArgs;
use super::artifacts::AttemptArtifact;
use super::artifacts::AttemptOwnershipArtifact;
use super::artifacts::CliError;
use super::artifacts::LoadedExperiment;
use super::artifacts::ParsedPromptSuite;
use super::artifacts::ReportArtifact;
use super::artifacts::RunArtifact;
use super::artifacts::RunSuiteArtifact;
use super::artifacts::RunVariantArtifact;
use super::artifacts::RunWorkgroupArtifact;
use super::artifacts::VariantMetadata;
use super::report::build_attempts;
use super::report::default_run_artifact_paths;
use super::report::validate_report_artifacts;
use super::storage::attempt_done_path;
use super::storage::attempt_running_path;
use super::storage::load_attempts;
use super::storage::read_json;
use super::storage::replace_attempts;
use super::storage::validate_required_artifact_file;
use super::storage::validate_workgroup_under_ac_root;
use super::storage::{attempts_name, attempts_path, attempts_path_display};
use super::storage::{load_report, load_run, validate_report_presence, validate_run_presence};
use super::support::err;
use super::support::err_at_path;
use super::validation::reject_link_or_reparse;
use crate::filesystem::{directories, documents};
use std::collections::BTreeMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

pub(super) fn load_and_validate_resume_artifacts(
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
    let state_dir = run_dir.join("attempt-state");
    validate_run_presence(run_dir)?;
    validate_required_artifact_file(&attempts_path(run_dir))?;
    validate_report_presence(run_dir)?;
    validate_required_artifact_file(
        &run_dir.join(default_run_artifact_paths(String::new()).report_markdown),
    )?;
    validate_attempt_state_dir(&state_dir)?;

    let run_artifact = load_run(run_dir)?;
    let attempts = load_attempts(run_dir)?;
    let report_artifact = load_report(run_dir)?;

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
    if let Err(mut report_errors) = validate_report_artifacts(
        loaded,
        run_id,
        &run_artifact,
        &report_artifact,
        attempts_path_display(&super::report::run_dir(loaded, run_id)),
    ) {
        errors.append(&mut report_errors);
    }
    validate_resume_run_contract(
        args,
        loaded,
        run_id,
        &run_artifact,
        &expected_suite,
        &expected_variants,
        expected_attempts.len(),
        &mut errors,
    );
    validate_resume_attempt_and_report_identity(
        &attempts,
        &expected_attempts,
        &report_artifact,
        &expected_variants,
        suite,
        &mut errors,
    );

    let Some(workgroup) = run_artifact.workgroup.clone() else {
        errors.push(err(
            "run_artifact_mismatch",
            "Run artifact is missing room metadata",
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
            "Run room metadata does not match resume arguments",
            None,
        ));
    }
    if let Err(mut workgroup_errors) = validate_workgroup_under_ac_root(&loaded.ac_root, &workgroup)
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

#[allow(clippy::too_many_arguments)]
fn validate_resume_run_contract(
    args: &RunArgs,
    loaded: &LoadedExperiment,
    run_id: &str,
    run_artifact: &RunArtifact,
    expected_suite: &RunSuiteArtifact,
    expected_variants: &[RunVariantArtifact],
    expected_attempt_count: usize,
    errors: &mut Vec<CliError>,
) {
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
    if run_artifact.suite != *expected_suite
        || run_artifact.replicates != args.replicates
        || run_artifact.variants != expected_variants
        || run_artifact.attempt_count != expected_attempt_count
        || run_artifact.artifacts != default_run_artifact_paths(attempts_name())
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
}

fn validate_resume_attempt_and_report_identity(
    attempts: &[AttemptArtifact],
    expected_attempts: &[AttemptArtifact],
    report_artifact: &ReportArtifact,
    expected_variants: &[RunVariantArtifact],
    suite: &ParsedPromptSuite,
    errors: &mut Vec<CliError>,
) {
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
}

fn validate_attempt_state_dir(state_dir: &Path) -> Result<(), Vec<CliError>> {
    reject_link_or_reparse(state_dir, "run_artifact_link_or_reparse", None)?;
    if !directories::experiment_path_is_dir(state_dir) {
        return Err(vec![err_at_path(
            "run_artifact_missing",
            format!("Run artifact directory not found: {}", state_dir.display()),
            state_dir,
        )]);
    }
    Ok(())
}

/// Checks one attempt-state file: name, owning attempt and ownership record.
/// Mismatches go to `errors`; read failures of the entry abort the walk.
fn validate_attempt_state_entry(
    path: &Path,
    attempt_ids: &HashSet<&str>,
    attempts_by_id: &BTreeMap<&str, &AttemptArtifact>,
    workgroup: &RunWorkgroupArtifact,
    errors: &mut Vec<CliError>,
) -> Result<(), Vec<CliError>> {
    reject_link_or_reparse(path, "run_artifact_link_or_reparse", None)?;
    if !directories::experiment_path_is_file(path) {
        errors.push(err_at_path(
            "run_artifact_mismatch",
            format!("Attempt-state entry is not a file: {}", path.display()),
            path,
        ));
        return Ok(());
    }
    let Some(file_name) = path.file_name().and_then(std::ffi::OsStr::to_str) else {
        errors.push(err_at_path(
            "run_artifact_mismatch",
            format!("Attempt-state entry has invalid name: {}", path.display()),
            path,
        ));
        return Ok(());
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
            path,
        ));
        return Ok(());
    };
    if !attempt_ids.contains(attempt_id) {
        errors.push(err_at_path(
            "run_artifact_mismatch",
            format!(
                "Attempt-state entry does not match a run attempt: {}",
                path.display()
            ),
            path,
        ));
        return Ok(());
    }
    let ownership: AttemptOwnershipArtifact = match read_json(path) {
        Ok(ownership) => ownership,
        Err(e) => {
            errors.push(err_at_path("run_artifact_unparseable", e, path));
            return Ok(());
        }
    };
    if ownership.schema_version != 1 {
        errors.push(err_at_path(
            "run_artifact_mismatch",
            "Attempt ownership schemaVersion must be 1",
            path,
        ));
        return Ok(());
    }
    if let Some(attempt) = attempts_by_id.get(attempt_id) {
        if let Err(mut ownership_errors) =
            validate_ownership_matches_attempt(&ownership, attempt, workgroup)
        {
            errors.append(&mut ownership_errors);
        }
    }
    Ok(())
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
    let visit = directories::visit_experiment_entries(state_dir, |entry| {
        let result = (|| -> Result<(), Vec<CliError>> {
            let (path, _) = entry.map_err(|e| {
                vec![err_at_path(
                    "run_artifact_unparseable",
                    format!("Failed to read attempt-state entry: {e}"),
                    state_dir,
                )]
            })?;
            validate_attempt_state_entry(
                &path,
                &attempt_ids,
                &attempts_by_id,
                workgroup,
                &mut errors,
            )
        })();
        match result {
            Ok(()) => std::ops::ControlFlow::Continue(()),
            Err(errors) => std::ops::ControlFlow::Break(errors),
        }
    })
    .map_err(|e| {
        vec![err_at_path(
            "run_artifact_unparseable",
            format!("Failed to read {}: {}", state_dir.display(), e),
            state_dir,
        )]
    })?;
    if let std::ops::ControlFlow::Break(errors) = visit {
        return Err(errors);
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

pub(super) fn replica_root_for_attempt(
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
    let canonical_workgroup =
        documents::canonical_experiment_role(&workgroup_path).map_err(|e| {
            vec![err_at_path(
                "run_artifact_mismatch",
                format!("Failed to canonicalize room: {e}"),
                &workgroup_path,
            )]
        })?;
    let canonical_replica = documents::canonical_experiment_role(&replica).map_err(|e| {
        vec![err_at_path(
            "run_artifact_mismatch",
            format!("Failed to canonicalize replica: {e}"),
            &replica,
        )]
    })?;
    if canonical_replica.starts_with(canonical_workgroup) {
        Ok(canonical_replica)
    } else {
        Err(vec![err(
            "run_artifact_mismatch",
            "Replica root is outside run room",
            Some(&attempt.variant),
        )])
    }
}

pub(super) fn reconcile_mutable_artifacts(
    run_dir: &Path,
    workgroup: &RunWorkgroupArtifact,
    retry_failed: bool,
) -> Result<Vec<AttemptArtifact>, Vec<CliError>> {
    let mut attempts = load_attempts(run_dir)?;
    let state_dir = run_dir.join("attempt-state");
    reject_link_or_reparse(&state_dir, "run_artifact_link_or_reparse", None)?;
    for attempt in &mut attempts {
        let done_path = attempt_done_path(&state_dir, &attempt.attempt_id);
        let running_path = attempt_running_path(&state_dir, &attempt.attempt_id);
        let done = if directories::experiment_path_is_file(&done_path) {
            Some(
                read_json::<AttemptOwnershipArtifact>(&done_path)
                    .map_err(|e| vec![err_at_path("run_artifact_unparseable", e, &done_path)])?,
            )
        } else {
            None
        };
        let running = if directories::experiment_path_is_file(&running_path) {
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
    replace_attempts(run_dir, &attempts)?;
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
