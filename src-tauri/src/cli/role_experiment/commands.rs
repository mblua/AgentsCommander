use super::fake_run::collect_experiment_validation;
use super::fake_run::run_fake_execution;
use super::prompt_suite::parse_prompt_suite;
use super::report::build_attempts;
use super::report::build_report_artifact;
use super::report::create_explicit_run_dir;
use super::report::create_generated_run_dir;
use super::report::default_run_artifact_paths;
use super::report::experiments_dir;
use super::report::generated_seed_from_uuid;
use super::report::render_report_markdown;
use super::report::report_artifact_paths;
use super::report::run_dir;
use super::report::validate_replicates;
use super::report::validate_report_artifacts;
use super::report::validate_run_id;
use super::storage::create_attempts;
use super::storage::load_experiment;
use super::storage::load_variant_metadata;
use super::storage::read_json;
use super::storage::resolve_ac_root;
use super::storage::resolve_project;
use super::storage::write_json;
use super::storage::write_json_new;
use super::storage::write_new_file;
use super::storage::{attempts_name, attempts_path, attempts_path_display};
use super::support::err;
use super::support::err_at_path;
use super::support::line_diff;
use super::support::map_err;
use super::support::sha256_hex;
use super::support::source_matrix_path;
use super::support::variant_agent_name;
use super::support::variant_matrix_path;
use super::support::warn;
use super::validation::copy_source_skills;
use super::validation::find_live_sessions_for_variant;
use super::validation::is_link_path;
use super::validation::is_replica_role_file;
use super::validation::reject_link_or_reparse;
use super::validation::validate_loaded_experiment_source_metadata;
use super::validation::validate_slug_identity;
use super::validation::validate_variant_metadata_paths;
use super::validation::validate_variant_name_request;
use super::CliError;
use super::CommandOutput;
use super::ExperimentArgs;
use super::ExperimentMetadata;
use super::InitArgs;
use super::LoadedExperiment;
use super::ProjectArgs;
use super::ReportArgs;
use super::ReportArtifact;
use super::RunArgs;
use super::RunArtifact;
use super::RunSuiteArtifact;
use super::RunVariantArtifact;
use super::ValidateArgs;
use super::VariantDiffArgs;
use super::VariantMetadata;
use super::VariantSetArgs;
use crate::cli::workgroup;
use crate::commands::entity_creation::agent_ref_bare_name;
use crate::commands::entity_creation::create_agent_matrix_from_role;
use crate::commands::entity_creation::resolve_agent_ref;
use crate::commands::entity_creation::validate_existing_name;
use crate::commands::entity_creation::CreateAgentMatrixFromRoleArgs;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

pub(super) fn init(args: InitArgs) -> Result<CommandOutput, Vec<CliError>> {
    let project_path = resolve_project(&args.project)?;
    init_for_project_path(args, &project_path)
}

pub(super) fn init_for_project_path(
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

    let ac_root = resolve_ac_root(project_path)?;
    reject_link_or_reparse(&ac_root, "workspace_link_or_reparse", None)?;

    let source_ref = map_err(
        resolve_agent_ref(&ac_root, &args.source_agent),
        "source_agent_missing",
        None,
    )?;
    let source_agent = agent_ref_bare_name(&source_ref);
    validate_slug_identity(&source_agent, "Source agent", "source_agent_invalid")?;
    let source_matrix = source_matrix_path(&ac_root, &source_agent);
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
        let target = variant_matrix_path(&ac_root, &source_agent, variant);
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

    let experiment_dir = experiments_dir(&ac_root).join(&args.name);
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
            ac_root: &ac_root,
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

pub(super) fn list(args: ProjectArgs) -> Result<CommandOutput, Vec<CliError>> {
    let project_path = resolve_project(&args.project)?;
    let ac_root = resolve_ac_root(&project_path)?;
    let root = experiments_dir(&ac_root);
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

pub(super) fn show(args: ExperimentArgs) -> Result<CommandOutput, Vec<CliError>> {
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

pub(super) fn variant_set(args: VariantSetArgs) -> Result<CommandOutput, Vec<CliError>> {
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

    let blockers = find_live_sessions_for_variant(&loaded.ac_root, &variant.agent_name);
    if !blockers.is_empty() {
        return Err(vec![err(
            "active_variant_session",
            format!("Variant '{}' has active sessions", args.variant),
            Some(args.variant.as_str()),
        )]);
    }

    let expected_matrix = variant_matrix_path(
        &loaded.ac_root,
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

pub(super) fn variant_diff(args: VariantDiffArgs) -> Result<CommandOutput, Vec<CliError>> {
    let loaded = load_experiment(&args.project, &args.experiment)?;
    variant_diff_loaded(&loaded, &args)
}

pub(super) fn variant_diff_loaded(
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
        &loaded.ac_root,
        &loaded.experiment.source_agent,
        &args.against,
    )
    .join("Role.md");
    let right_role = variant_matrix_path(
        &loaded.ac_root,
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

pub(super) fn validate(args: ValidateArgs) -> Result<CommandOutput, Vec<CliError>> {
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
            "--retain-room=false is not supported in Phase 3A",
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
            "Real role-experiment execution requires a running AgentsCommander app or daemon; \
             Phase 3A CLI-only execution has no side effects",
            None,
        )])
    } else {
        Ok(())
    }
}

pub(super) fn run(args: RunArgs) -> Result<CommandOutput, Vec<CliError>> {
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
    let artifacts = default_run_artifact_paths(attempts_name());
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
                    &loaded.ac_root,
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
        report_artifact_paths(&run_dir, attempts_path_display(&run_dir)),
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
    create_attempts(&run_dir, &attempts)?;
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

pub(super) fn report(args: ReportArgs) -> Result<CommandOutput, Vec<CliError>> {
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
    let attempts_path = attempts_path(&dir);
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
    validate_report_artifacts(
        &loaded,
        &args.run_id,
        &run_artifact,
        &report_artifact,
        attempts_path_display(&dir),
    )?;
    let expected_artifact_paths = report_artifact_paths(&dir, attempts_path_display(&dir));
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
