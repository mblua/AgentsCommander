use super::prompt_suite::parse_prompt_suite;
use super::report::build_attempts;
use super::report::build_report_artifact;
use super::report::create_explicit_run_dir;
use super::report::create_generated_run_dir;
use super::report::default_run_artifact_paths;
use super::report::generated_seed_from_uuid;
use super::report::render_report_markdown;
use super::report::run_dir;
use super::report::summarize_run_status;
use super::report::validate_run_id;
use super::resume::load_and_validate_resume_artifacts;
use super::resume::reconcile_mutable_artifacts;
use super::resume::replica_root_for_attempt;
use super::storage::atomic_write_bytes;
use super::storage::atomic_write_json;
use super::storage::create_run_workgroup_dirs;
use super::storage::ensure_run_artifact_dirs;
use super::storage::load_experiment;
use super::storage::load_variant_metadata;
use super::storage::next_attempt_output_path;
use super::storage::replace_attempts;
use super::storage::write_attempt_prompt_file;
use super::storage::{attempts_name, attempts_path_display};
use super::support::err;
use super::support::err_at_path;
use super::support::source_matrix_path;
use super::support::warn;
use super::validation::add_diff_warnings;
use super::validation::find_live_sessions_for_variant;
use super::validation::reject_link_or_reparse;
use super::validation::replica_role_overrides;
use super::validation::validate_loaded_experiment_source_metadata;
use super::validation::validate_slug_identity;
use super::validation::validate_variant_files;
use super::validation::validate_variant_metadata_paths;
use super::AttemptArtifact;
use super::AttemptExecutionResult;
use super::AttemptOwnershipArtifact;
use super::CliError;
use super::CliWarning;
use super::CommandOutput;
use super::ExecutionPlan;
use super::ExperimentValidation;
use super::LoadedExperiment;
use super::RunArgs;
use super::RunArtifact;
use super::RunExecutionArtifact;
use super::RunSuiteArtifact;
use super::RunVariantArtifact;
use std::collections::HashSet;
use std::path::Path;

pub(super) fn run_fake_execution(args: RunArgs) -> Result<CommandOutput, Vec<CliError>> {
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
            artifacts: default_run_artifact_paths(attempts_name()),
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
    replace_attempts(&plan.run_dir, &plan.attempts)?;
    let report_artifact = build_report_artifact(
        &plan.run_id,
        &loaded.experiment.name,
        super::report::report_artifact_paths(&plan.run_dir, attempts_path_display(&plan.run_dir)),
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

    warnings.extend(fake_execution_warnings());

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
                "attempts": attempts_name(),
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

fn fake_execution_warnings() -> [CliWarning; 4] {
    [
        warn(
            "run_workgroup_retained",
            "Run room was retained for inspection; cleanup is not implemented in Phase 3A.",
            None,
        ),
        warn(
            "transcript_capture_best_effort",
            "Provider-native transcript paths may be unavailable; \
             use run metadata and retained room for inspection.",
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
    ]
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
                    &loaded.ac_root,
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
        replace_attempts(&plan.run_dir, &plan.attempts)?;
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

pub(super) fn collect_experiment_validation(loaded: &LoadedExperiment) -> ExperimentValidation {
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
        let source_matrix = source_matrix_path(&loaded.ac_root, &loaded.experiment.source_agent);
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
                for blocker in find_live_sessions_for_variant(&loaded.ac_root, &meta.agent_name) {
                    errors.push(err(
                        "active_variant_session",
                        format!(
                            "Active session '{}' at {}",
                            blocker.name, blocker.working_directory
                        ),
                        Some(meta.name.as_str()),
                    ));
                }
                if replica_role_overrides(&loaded.ac_root, &meta.agent_name)
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
