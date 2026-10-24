use super::storage::{attempts_name, attempts_path, attempts_path_display};
use super::*;

fn loaded_fixture(source_agent: &str) -> (tempfile::TempDir, LoadedExperiment) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ac_root = tmp.path().join(".ac");
    let experiment_dir = ac_root.join("experiments").join("exp");
    std::fs::create_dir_all(experiment_dir.join("variants")).expect("experiment dirs");

    let source_matrix = ac_root.join(format!("_agent_{}", source_agent));
    std::fs::create_dir_all(&source_matrix).expect("source matrix");
    std::fs::write(source_matrix.join("Role.md"), "source role\n").expect("source role");

    for variant in ["control", "test"] {
        let agent_name = variant_agent_name(source_agent, variant);
        let matrix = ac_root.join(format!("_agent_{}", agent_name));
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
        ac_root,
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
    let workgroup_dir = loaded.ac_root.join("wg-1-role-exp-exp");
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
        report::report_artifact_paths(&run_dir, attempts_path_display(&run_dir)),
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
        artifacts: default_run_artifact_paths(attempts_name()),
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

    let errors =
        load_and_validate_resume_artifacts(&args, &loaded, &suite, &run_id, &run_dir, &variants)
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
    let ac_root = project.join(".ac");
    let source_matrix = ac_root.join("_agent_alpha");
    std::fs::create_dir_all(&source_matrix).expect("source matrix");
    std::fs::write(source_matrix.join("Role.md"), "source role\n").expect("source role");

    let outside = tmp.path().join("outside-experiments");
    std::fs::create_dir_all(&outside).expect("outside experiments");
    let experiments = ac_root.join("experiments");

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

    let errors = reject_link_or_reparse(&link.join("child").join("Role.md"), "link_error", None)
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

#[test]
fn role_exp_room_dir_name_is_padded() {
    assert_eq!(
        role_exp_room_dir_name(1, "team", 3),
        "room-001-role-exp-team"
    );
    assert_eq!(role_exp_room_dir_name(1, "team", 1), "room-1-role-exp-team");
    assert_eq!(
        role_exp_room_dir_name(100, "team", 2),
        "room-100-role-exp-team"
    );
}

#[test]
fn attempts_reader_preserves_empty_and_invalid_line_behavior() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = attempts_path(tmp.path());
    std::fs::write(&path, "").expect("empty fixture");
    assert!(storage::load_attempts(tmp.path())
        .expect("empty attempts")
        .is_empty());

    let (_tmp, _loaded, _suite, _variants, _args, run_dir, _run_id, _report) = resume_fixture();
    let fixture = std::fs::read_to_string(attempts_path(&run_dir)).expect("fixture");
    let first = fixture.lines().next().expect("first attempt");
    std::fs::write(&path, format!("{}\ninvalid\n", first)).expect("invalid fixture");
    let errors = storage::load_attempts(tmp.path()).expect_err("invalid line");
    assert_eq!(errors[0].code, "run_artifact_unparseable");
    assert_eq!(errors[0].line, Some(2));
    assert!(errors[0]
        .message
        .starts_with("Attempt line 2 is invalid JSON: "));

    std::fs::write(&path, format!("{}\n\n", first)).expect("blank line fixture");
    let errors = storage::load_attempts(tmp.path()).expect_err("blank line");
    assert_eq!(errors[0].line, Some(2));
}

#[test]
fn attempts_storage_preserves_bytes_order_and_create_new() {
    let (_tmp, _loaded, _suite, _variants, _args, run_dir, _run_id, _report) = resume_fixture();
    let fixture = std::fs::read_to_string(attempts_path(&run_dir)).expect("fixture");
    let attempts: Vec<AttemptArtifact> = fixture
        .lines()
        .map(|line| serde_json::from_str(line).expect("fixture attempt"))
        .collect();
    let target = tempfile::tempdir().expect("target");
    storage::create_attempts(target.path(), &attempts).expect("create");
    let expected = format!("{}\n", fixture);
    assert_eq!(
        std::fs::read_to_string(attempts_path(target.path())).unwrap(),
        expected
    );
    let errors = storage::create_attempts(target.path(), &[]).expect_err("already exists");
    assert_eq!(errors[0].code, "run_artifact_write_failed");
    assert_eq!(
        std::fs::read_to_string(attempts_path(target.path())).unwrap(),
        expected
    );
    let loaded = storage::load_attempts(target.path()).expect("read");
    assert_eq!(
        serde_json::to_value(loaded).unwrap(),
        serde_json::to_value(&attempts).unwrap()
    );
    storage::replace_attempts(target.path(), &attempts[..1]).expect("replace");
    assert_eq!(
        std::fs::read_to_string(attempts_path(target.path())).unwrap(),
        format!("{}\n", fixture.lines().next().unwrap())
    );
}

#[test]
fn resume_checks_run_presence_before_invalid_attempts() {
    let (_tmp, loaded, suite, variants, args, run_dir, run_id, _report) = resume_fixture();
    std::fs::remove_file(run_dir.join("run.json")).expect("remove run");
    std::fs::write(attempts_path(&run_dir), "invalid").expect("invalid attempts");
    let errors = resume::load_and_validate_resume_artifacts(
        &args, &loaded, &suite, &run_id, &run_dir, &variants,
    )
    .expect_err("missing run precedes parsing attempts");
    assert_eq!(errors[0].code, "run_artifact_missing");
    assert_eq!(
        errors[0].path,
        Some(run_dir.join("run.json").to_string_lossy().to_string())
    );
}
