use super::*;

#[test]
fn role_experiment_report_reads_text_and_json_artifacts() {
    let tmp = Tmp::new("role-exp-report");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_source(tmp.path());
    init_experiment(&bin);
    let suite = tmp.path().join("prompts.jsonl");
    write_prompt_suite(&suite);
    run_ok(
        &bin,
        &[
            "role-experiment",
            "run",
            "--project",
            "ProjectAlpha",
            "--experiment",
            "techlead-test",
            "--prompt-suite",
            &suite.to_string_lossy(),
            "--run-id",
            "20260601-181500",
            "--dry-run",
        ],
    );
    let run_dir = project
        .join(".ac")
        .join("experiments")
        .join("techlead-test")
        .join("runs")
        .join("20260601-181500");
    let mut run_artifact: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(run_dir.join("run.json")).unwrap()).unwrap();
    run_artifact["artifacts"]["reportMarkdown"] = serde_json::json!("..\\outside.md");
    std::fs::write(
        run_dir.join("run.json"),
        serde_json::to_string_pretty(&run_artifact).unwrap(),
    )
    .unwrap();
    let expected_run_path = std::fs::canonicalize(run_dir.join("run.json"))
        .unwrap()
        .to_string_lossy()
        .to_string();
    let expected_attempts_path = std::fs::canonicalize(run_dir.join("attempts.jsonl"))
        .unwrap()
        .to_string_lossy()
        .to_string();
    let expected_report_json_path = std::fs::canonicalize(run_dir.join("report.json"))
        .unwrap()
        .to_string_lossy()
        .to_string();
    let expected_report_markdown_path = std::fs::canonicalize(run_dir.join("report.md"))
        .unwrap()
        .to_string_lossy()
        .to_string();

    let text = run_ok(
        &bin,
        &[
            "role-experiment",
            "report",
            "--project",
            "ProjectAlpha",
            "--experiment",
            "techlead-test",
            "--run-id",
            "20260601-181500",
        ],
    );
    assert_eq!(text["data"]["experiment"], "techlead-test");
    assert_eq!(text["data"]["runId"], "20260601-181500");
    assert_eq!(text["data"]["format"], "text");
    assert_eq!(text["data"]["status"], "dry_run");
    assert_eq!(text["data"]["artifactPaths"]["run"], expected_run_path);
    assert_eq!(
        text["data"]["artifactPaths"]["attempts"],
        expected_attempts_path
    );
    assert_eq!(
        text["data"]["artifactPaths"]["reportJson"],
        expected_report_json_path
    );
    assert_eq!(
        text["data"]["artifactPaths"]["reportMarkdown"],
        expected_report_markdown_path
    );
    assert!(text["data"]["reportMarkdown"]
        .as_str()
        .unwrap()
        .contains("Status: dry_run"));
    let json = run_ok(
        &bin,
        &[
            "role-experiment",
            "report",
            "--project",
            "ProjectAlpha",
            "--experiment",
            "techlead-test",
            "--run-id",
            "20260601-181500",
            "--format",
            "json",
        ],
    );
    assert_eq!(json["data"]["experiment"], "techlead-test");
    assert_eq!(json["data"]["runId"], "20260601-181500");
    assert_eq!(json["data"]["format"], "json");
    assert_eq!(json["data"]["status"], "dry_run");
    assert_eq!(json["data"]["artifactPaths"]["run"], expected_run_path);
    assert_eq!(
        json["data"]["artifactPaths"]["attempts"],
        expected_attempts_path
    );
    assert_eq!(
        json["data"]["artifactPaths"]["reportJson"],
        expected_report_json_path
    );
    assert_eq!(
        json["data"]["artifactPaths"]["reportMarkdown"],
        expected_report_markdown_path
    );
    let persisted_report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(run_dir.join("report.json")).unwrap())
            .unwrap();
    assert_eq!(persisted_report["format"], "json");
    assert_eq!(
        persisted_report["artifactPaths"],
        json["data"]["artifactPaths"]
    );
    assert_eq!(persisted_report, json["data"]["report"]);
    assert_eq!(json["data"]["report"]["summary"]["attemptCount"], 4);

    let (code, invalid, _stderr) = run(
        &bin,
        &[
            "role-experiment",
            "report",
            "--project",
            "ProjectAlpha",
            "--experiment",
            "techlead-test",
            "--run-id",
            "20260601-181500",
            "--format",
            "xml",
        ],
    );
    assert_eq!(code, 1);
    assert_eq!(invalid["errors"][0]["code"], "report_format_invalid");
}

#[test]
fn role_experiment_report_rejects_artifact_mismatch() {
    let tmp = Tmp::new("role-exp-report-mismatch");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_source(tmp.path());
    init_experiment(&bin);
    let suite = tmp.path().join("prompts.jsonl");
    write_prompt_suite(&suite);
    run_ok(
        &bin,
        &[
            "role-experiment",
            "run",
            "--project",
            "ProjectAlpha",
            "--experiment",
            "techlead-test",
            "--prompt-suite",
            &suite.to_string_lossy(),
            "--run-id",
            "20260601-181500",
            "--dry-run",
        ],
    );
    let report_path = project
        .join(".ac")
        .join("experiments")
        .join("techlead-test")
        .join("runs")
        .join("20260601-181500")
        .join("report.json");
    let mut report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report_path).unwrap()).unwrap();
    report["summary"]["plannedAttemptCount"] = serde_json::json!(999);
    std::fs::write(&report_path, serde_json::to_string_pretty(&report).unwrap()).unwrap();

    let (code, json, _stderr) = run(
        &bin,
        &[
            "role-experiment",
            "report",
            "--project",
            "ProjectAlpha",
            "--experiment",
            "techlead-test",
            "--run-id",
            "20260601-181500",
            "--format",
            "json",
        ],
    );
    assert_eq!(code, 1);
    assert!(json["errors"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["code"] == "run_artifact_mismatch"));
}

#[test]
fn role_experiment_report_accepts_completed_real_run() {
    let tmp = Tmp::new("role-exp-real-report");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    project_with_source(tmp.path());
    init_experiment(&bin);
    let suite = tmp.path().join("prompts.jsonl");
    write_prompt_suite(&suite);
    run_fake_ok(
        &bin,
        &[
            "role-experiment",
            "run",
            "--project",
            "ProjectAlpha",
            "--experiment",
            "techlead-test",
            "--prompt-suite",
            &suite.to_string_lossy(),
            "--run-id",
            "20260601-181500",
            "--execute",
            "--fake-executor",
        ],
    );
    let json = run_ok(
        &bin,
        &[
            "role-experiment",
            "report",
            "--project",
            "ProjectAlpha",
            "--experiment",
            "techlead-test",
            "--run-id",
            "20260601-181500",
            "--format",
            "json",
        ],
    );
    assert_eq!(json["data"]["status"], "completed");
    assert_eq!(json["data"]["report"]["dryRun"], false);
    assert_eq!(
        json["data"]["report"]["summary"]["completedAttemptCount"],
        4
    );
    let text = run_ok(
        &bin,
        &[
            "role-experiment",
            "report",
            "--project",
            "ProjectAlpha",
            "--experiment",
            "techlead-test",
            "--run-id",
            "20260601-181500",
        ],
    );
    assert!(text["data"]["reportMarkdown"]
        .as_str()
        .unwrap()
        .contains("# Role Experiment Run Report"));
}

#[test]
fn role_experiment_retry_failed_preserves_previous_outputs() {
    let tmp = Tmp::new("role-exp-retry");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_source(tmp.path());
    init_experiment(&bin);
    let suite = tmp.path().join("prompts.jsonl");
    write_prompt_suite(&suite);
    run_fake_ok(
        &bin,
        &[
            "role-experiment",
            "run",
            "--project",
            "ProjectAlpha",
            "--experiment",
            "techlead-test",
            "--prompt-suite",
            &suite.to_string_lossy(),
            "--run-id",
            "20260601-181500",
            "--execute",
            "--fake-executor",
            "--provider",
            "fake-failed",
        ],
    );
    let run_dir = project
        .join(".ac")
        .join("experiments")
        .join("techlead-test")
        .join("runs")
        .join("20260601-181500");
    let first_output = run_dir.join("outputs").join("coord-001__control__r1.txt");
    assert!(first_output.is_file());
    run_fake_ok(
        &bin,
        &[
            "role-experiment",
            "run",
            "--project",
            "ProjectAlpha",
            "--experiment",
            "techlead-test",
            "--prompt-suite",
            &suite.to_string_lossy(),
            "--run-id",
            "20260601-181500",
            "--execute",
            "--fake-executor",
            "--resume-run",
            "--retry-failed",
        ],
    );
    assert!(first_output.is_file());
    assert!(run_dir
        .join("outputs")
        .join("coord-001__control__r1.retry-01.txt")
        .is_file());
}

#[test]
fn role_experiment_resume_rejects_mismatched_run_id_without_mutation() {
    let fixture = completed_fake_run("role-exp-resume-run-id", &["--provider", "fake-failed"]);
    let before = ac_snapshot(&fixture.run_dir);
    let run_path = fixture.run_dir.join("run.json");
    let mut run_artifact: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&run_path).unwrap()).unwrap();
    run_artifact["runId"] = serde_json::json!("20260601-181501");
    std::fs::write(
        &run_path,
        serde_json::to_string_pretty(&run_artifact).unwrap(),
    )
    .unwrap();
    let after_tamper = ac_snapshot(&fixture.run_dir);

    let (code, json, _stderr) = run_fake(
        &fixture.bin,
        &[
            "role-experiment",
            "run",
            "--project",
            "ProjectAlpha",
            "--experiment",
            "techlead-test",
            "--prompt-suite",
            &fixture.suite.to_string_lossy(),
            "--run-id",
            "20260601-181500",
            "--execute",
            "--fake-executor",
            "--resume-run",
            "--retry-failed",
        ],
    );

    assert_ne!(before, after_tamper);
    assert_eq!(code, 1);
    assert!(json["errors"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["code"] == "run_artifact_mismatch"));
    assert_eq!(ac_snapshot(&fixture.run_dir), after_tamper);
}

#[test]
fn role_experiment_resume_rejects_mismatched_seed_without_mutation() {
    let fixture = completed_fake_run(
        "role-exp-resume-seed",
        &["--provider", "fake-failed", "--seed", "12345"],
    );
    let before = ac_snapshot(&fixture.run_dir);

    let (code, json, _stderr) = run_fake(
        &fixture.bin,
        &[
            "role-experiment",
            "run",
            "--project",
            "ProjectAlpha",
            "--experiment",
            "techlead-test",
            "--prompt-suite",
            &fixture.suite.to_string_lossy(),
            "--run-id",
            "20260601-181500",
            "--execute",
            "--fake-executor",
            "--resume-run",
            "--retry-failed",
            "--seed",
            "67890",
        ],
    );

    assert_eq!(code, 1);
    assert!(json["errors"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["code"] == "run_artifact_mismatch"));
    assert_eq!(ac_snapshot(&fixture.run_dir), before);
}

#[test]
fn role_experiment_resume_rejects_mismatched_variant_without_mutation() {
    let fixture = completed_fake_run("role-exp-resume-variant", &["--provider", "fake-failed"]);
    let run_path = fixture.run_dir.join("run.json");
    let mut run_artifact: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&run_path).unwrap()).unwrap();
    run_artifact["variants"][0]["roleSha256"] = serde_json::json!("bad");
    std::fs::write(
        &run_path,
        serde_json::to_string_pretty(&run_artifact).unwrap(),
    )
    .unwrap();
    let after_tamper = ac_snapshot(&fixture.run_dir);

    let (code, json, _stderr) = run_fake(
        &fixture.bin,
        &[
            "role-experiment",
            "run",
            "--project",
            "ProjectAlpha",
            "--experiment",
            "techlead-test",
            "--prompt-suite",
            &fixture.suite.to_string_lossy(),
            "--run-id",
            "20260601-181500",
            "--execute",
            "--fake-executor",
            "--resume-run",
            "--retry-failed",
        ],
    );

    assert_eq!(code, 1);
    assert!(json["errors"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["code"] == "run_artifact_mismatch"));
    assert_eq!(ac_snapshot(&fixture.run_dir), after_tamper);
}

#[test]
fn role_experiment_resume_rejects_mismatched_report_paths_without_mutation() {
    let fixture = completed_fake_run(
        "role-exp-resume-report-paths",
        &["--provider", "fake-failed"],
    );
    let report_path = fixture.run_dir.join("report.json");
    let mut report_artifact: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report_path).unwrap()).unwrap();
    report_artifact["artifactPaths"]["reportJson"] = serde_json::json!("wrong.json");
    std::fs::write(
        &report_path,
        serde_json::to_string_pretty(&report_artifact).unwrap(),
    )
    .unwrap();
    let after_tamper = ac_snapshot(&fixture.run_dir);

    let (code, json, _stderr) = run_fake(
        &fixture.bin,
        &[
            "role-experiment",
            "run",
            "--project",
            "ProjectAlpha",
            "--experiment",
            "techlead-test",
            "--prompt-suite",
            &fixture.suite.to_string_lossy(),
            "--run-id",
            "20260601-181500",
            "--execute",
            "--fake-executor",
            "--resume-run",
            "--retry-failed",
        ],
    );

    assert_eq!(code, 1);
    assert!(json["errors"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["code"] == "run_artifact_mismatch"));
    assert_eq!(ac_snapshot(&fixture.run_dir), after_tamper);
}
