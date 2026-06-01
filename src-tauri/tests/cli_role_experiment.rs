use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

struct Tmp(PathBuf);

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl Tmp {
    fn new(prefix: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("ac-{}-{}", prefix, uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&path).expect("create tmp dir");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

fn copy_binary_into(tmp: &Path) -> PathBuf {
    let src = Path::new(env!("CARGO_BIN_EXE_agentscommander-new"));
    let dst = tmp.join(src.file_name().expect("binary file name"));
    std::fs::copy(src, &dst).expect("copy binary");
    dst
}

fn config_dir_for_bin(bin: &Path) -> PathBuf {
    let stem = bin
        .file_stem()
        .expect("bin stem")
        .to_string_lossy()
        .to_string();
    bin.parent().expect("bin parent").join(format!(".{}", stem))
}

fn write_settings(config_dir: &Path, project_parent: &Path) {
    std::fs::create_dir_all(config_dir).expect("create config dir");
    let settings = serde_json::json!({
        "defaultShell": "powershell.exe",
        "defaultShellArgs": [],
        "agents": [],
        "projectPaths": [project_parent.to_string_lossy().to_string()]
    });
    std::fs::write(
        config_dir.join("settings.json"),
        serde_json::to_string_pretty(&settings).expect("settings json"),
    )
    .expect("write settings");
}

fn project_with_source(tmp: &Path) -> PathBuf {
    let project = tmp.join("ProjectAlpha");
    let source = project.join(".ac").join("_agent_tech-lead");
    std::fs::create_dir_all(source.join("memory")).expect("source memory");
    std::fs::create_dir_all(source.join("plans")).expect("source plans");
    std::fs::create_dir_all(source.join("skills").join("nested")).expect("source skills");
    std::fs::write(
        source.join("Role.md"),
        b"\xEF\xBB\xBF# Tech Lead\r\n\r\nPlan.\r\n",
    )
    .expect("write role");
    std::fs::write(
        source.join("skills").join("nested").join("SKILL.md"),
        "skill\n",
    )
    .expect("write skill");
    project
}

fn run(bin: &Path, args: &[&str]) -> (i32, serde_json::Value, String) {
    let out = Command::new(bin).args(args).output().expect("spawn");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("stdout json: {}\n{}", e, stdout));
    (
        out.status.code().unwrap_or(-1),
        json,
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn run_ok(bin: &Path, args: &[&str]) -> serde_json::Value {
    let (code, json, stderr) = run(bin, args);
    assert_eq!(code, 0, "stdout: {}\nstderr: {}", json, stderr);
    assert_eq!(json["ok"], true, "{}", json);
    json
}

fn ac_snapshot(root: &Path) -> BTreeMap<String, Option<String>> {
    let mut out = BTreeMap::new();
    snapshot_into(root, root, &mut out);
    out
}

fn snapshot_into(root: &Path, path: &Path, out: &mut BTreeMap<String, Option<String>>) {
    if !path.exists() {
        return;
    }
    let relative = path
        .strip_prefix(root)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    if path.is_dir() {
        if !relative.is_empty() {
            out.insert(relative, None);
        }
        let mut entries: Vec<PathBuf> = std::fs::read_dir(path)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        entries.sort();
        for entry in entries {
            snapshot_into(root, &entry, out);
        }
    } else {
        let bytes = std::fs::read(path).unwrap();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        bytes.hash(&mut hasher);
        out.insert(relative, Some(format!("{:x}", hasher.finish())));
    }
}

fn write_prompt_suite(path: &Path) {
    std::fs::write(
        path,
        concat!(
            "{\"id\":\"coord-001\",\"title\":\"Bug in CI\",\"prompt\":\"Fix it\",\"tags\":[\"ci\"],\"expectedBehaviors\":[\"delegate\"]}\n",
            "{\"id\":\"coord-002\",\"title\":\"Triage\",\"prompt\":\"Triage it\"}\n",
        ),
    )
    .unwrap();
}

fn init_experiment(bin: &Path) -> serde_json::Value {
    run_ok(
        bin,
        &[
            "role-experiment",
            "init",
            "--project",
            "ProjectAlpha",
            "--name",
            "techlead-test",
            "--source-agent",
            "tech-lead",
            "--variants",
            "control,strict",
        ],
    )
}

#[test]
fn role_experiment_init_creates_metadata_and_variant_matrices() {
    let tmp = Tmp::new("role-exp-init");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_source(tmp.path());

    let json = init_experiment(&bin);
    assert_eq!(json["command"], "role-experiment init");
    assert_eq!(json["data"]["sourceAgent"], "tech-lead");

    let workspace = project.join(".ac");
    let source_role = std::fs::read(workspace.join("_agent_tech-lead").join("Role.md")).unwrap();
    for variant in ["control", "strict"] {
        let agent = workspace.join(format!("_agent_tech-lead-{}", variant));
        assert_eq!(std::fs::read(agent.join("Role.md")).unwrap(), source_role);
        assert!(agent.join("config.json").is_file());
        for dir in ["memory", "plans", "skills", "inbox", "outbox"] {
            assert!(agent.join(dir).is_dir(), "missing {}", dir);
        }
        assert!(agent
            .join("skills")
            .join("nested")
            .join("SKILL.md")
            .is_file());
        assert!(agent.join("memory").read_dir().unwrap().next().is_none());
        assert!(agent.join("plans").read_dir().unwrap().next().is_none());
    }

    let exp_dir = workspace.join("experiments").join("techlead-test");
    let exp: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(exp_dir.join("experiment.json")).unwrap())
            .unwrap();
    assert_eq!(exp["schemaVersion"], 1);
    assert_eq!(
        std::fs::canonicalize(exp_dir.join(exp["sourceMatrixPath"].as_str().unwrap())).unwrap(),
        std::fs::canonicalize(workspace.join("_agent_tech-lead")).unwrap()
    );
    let strict: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(exp_dir.join("variants").join("strict.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        std::fs::canonicalize(
            exp_dir
                .join("variants")
                .join(strict["rolePath"].as_str().unwrap())
        )
        .unwrap(),
        std::fs::canonicalize(workspace.join("_agent_tech-lead-strict").join("Role.md")).unwrap()
    );

    let list = run_ok(
        &bin,
        &["role-experiment", "list", "--project", "ProjectAlpha"],
    );
    assert_eq!(list["data"]["experiments"][0]["name"], "techlead-test");
    let show = run_ok(
        &bin,
        &[
            "role-experiment",
            "show",
            "--project",
            "ProjectAlpha",
            "--experiment",
            "techlead-test",
        ],
    );
    assert_eq!(show["data"]["variants"].as_array().unwrap().len(), 2);
}

#[test]
fn role_experiment_init_normalizes_prefixed_source_agent() {
    let tmp = Tmp::new("role-exp-prefixed-source");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_source(tmp.path());

    run_ok(
        &bin,
        &[
            "role-experiment",
            "init",
            "--project",
            "ProjectAlpha",
            "--name",
            "techlead-test",
            "--source-agent",
            "_agent_tech-lead",
            "--variants",
            "control,strict",
        ],
    );

    let workspace = project.join(".ac");
    assert!(workspace.join("_agent_tech-lead-control").is_dir());
    assert!(!workspace.join("_agent__agent_tech-lead-control").exists());
    let exp: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            workspace
                .join("experiments")
                .join("techlead-test")
                .join("experiment.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(exp["sourceAgent"], "tech-lead");
}

#[test]
fn role_experiment_init_rejects_bad_variant_names_without_writes() {
    let tmp = Tmp::new("role-exp-bad-variant");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_source(tmp.path());

    let (code, json, _stderr) = run(
        &bin,
        &[
            "role-experiment",
            "init",
            "--project",
            "ProjectAlpha",
            "--name",
            "techlead-test",
            "--source-agent",
            "tech-lead",
            "--variants",
            "control,Strict",
        ],
    );
    assert_eq!(code, 1);
    assert_eq!(json["ok"], false);
    assert_eq!(json["errors"][0]["code"], "variant_name_invalid");
    let workspace = project.join(".ac");
    assert!(!workspace.join("experiments").join("techlead-test").exists());
    assert!(!workspace.join("_agent_tech-lead-control").exists());
}

#[test]
fn role_experiment_variant_set_diff_and_validate() {
    let tmp = Tmp::new("role-exp-set");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_source(tmp.path());
    init_experiment(&bin);

    let new_role = tmp.path().join("StrictRole.md");
    std::fs::write(&new_role, "# Strict\r\n\r\nNew role.\r\n").expect("write new role");
    let set = run_ok(
        &bin,
        &[
            "role-experiment",
            "variant",
            "set",
            "--project",
            "ProjectAlpha",
            "--experiment",
            "techlead-test",
            "--variant",
            "strict",
            "--role-file",
            &new_role.to_string_lossy(),
        ],
    );
    assert_eq!(set["data"]["variant"], "strict");

    let workspace = project.join(".ac");
    assert_eq!(
        std::fs::read_to_string(workspace.join("_agent_tech-lead-strict").join("Role.md")).unwrap(),
        "# Strict\r\n\r\nNew role.\r\n"
    );
    assert_eq!(
        std::fs::read(workspace.join("_agent_tech-lead").join("Role.md")).unwrap(),
        b"\xEF\xBB\xBF# Tech Lead\r\n\r\nPlan.\r\n"
    );
    assert!(!workspace
        .join("wg-1-dev-team")
        .join("__agent_tech-lead-strict")
        .join("Role.md")
        .exists());

    let diff = run_ok(
        &bin,
        &[
            "role-experiment",
            "variant",
            "diff",
            "--project",
            "ProjectAlpha",
            "--experiment",
            "techlead-test",
            "--variant",
            "strict",
        ],
    );
    assert!(diff["data"]["diff"]
        .as_str()
        .unwrap()
        .contains("--- control"));
    assert!(diff["data"]["diff"]
        .as_str()
        .unwrap()
        .contains("+++ strict"));
    assert!(diff["data"]["stats"]["changedLines"].as_u64().unwrap() > 0);

    let validate = run_ok(
        &bin,
        &[
            "role-experiment",
            "validate",
            "--project",
            "ProjectAlpha",
            "--experiment",
            "techlead-test",
        ],
    );
    assert_eq!(validate["data"]["valid"], true);
}

#[test]
fn role_experiment_rejects_replica_role_and_tampered_metadata() {
    let tmp = Tmp::new("role-exp-rejects");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_source(tmp.path());
    init_experiment(&bin);
    let workspace = project.join(".ac");
    let replica_role = workspace
        .join("wg-1-dev-team")
        .join("__agent_tech-lead-strict")
        .join("Role.md");
    std::fs::create_dir_all(replica_role.parent().unwrap()).expect("replica parent");
    std::fs::write(&replica_role, "# Replica\n").expect("replica role");

    let (code, json, _stderr) = run(
        &bin,
        &[
            "role-experiment",
            "variant",
            "set",
            "--project",
            "ProjectAlpha",
            "--experiment",
            "techlead-test",
            "--variant",
            "strict",
            "--role-file",
            &replica_role.to_string_lossy(),
        ],
    );
    assert_eq!(code, 1);
    assert_eq!(json["ok"], false);
    assert_eq!(json["errors"][0]["code"], "replica_role_file_not_allowed");

    let strict_json = workspace
        .join("experiments")
        .join("techlead-test")
        .join("variants")
        .join("strict.json");
    let mut strict: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&strict_json).unwrap()).unwrap();
    strict["rolePath"] = serde_json::json!("../../../_agent_tech-lead/Role.md");
    std::fs::write(&strict_json, serde_json::to_string_pretty(&strict).unwrap()).unwrap();
    let external_role = tmp.path().join("ExternalRole.md");
    std::fs::write(&external_role, "# External\n").unwrap();
    let (code, json, _stderr) = run(
        &bin,
        &[
            "role-experiment",
            "variant",
            "set",
            "--project",
            "ProjectAlpha",
            "--experiment",
            "techlead-test",
            "--variant",
            "strict",
            "--role-file",
            &external_role.to_string_lossy(),
        ],
    );
    assert_eq!(code, 1);
    assert_eq!(json["errors"][0]["code"], "metadata_path_mismatch");
    assert_eq!(
        std::fs::read(workspace.join("_agent_tech-lead").join("Role.md")).unwrap(),
        b"\xEF\xBB\xBF# Tech Lead\r\n\r\nPlan.\r\n"
    );
}

#[test]
fn role_experiment_validate_reports_prompt_and_replica_errors() {
    let tmp = Tmp::new("role-exp-validate");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_source(tmp.path());
    init_experiment(&bin);
    let workspace = project.join(".ac");
    let replica = workspace
        .join("wg-1-dev-team")
        .join("__agent_tech-lead-strict");
    std::fs::create_dir_all(&replica).unwrap();
    std::fs::write(replica.join("Role.md"), "# Override\n").unwrap();
    std::fs::write(
        config_dir.join("sessions.json"),
        serde_json::to_string_pretty(&serde_json::json!([{
            "name": "ProjectAlpha/strict",
            "shell": "powershell.exe",
            "shellArgs": [],
            "workingDirectory": replica.to_string_lossy(),
            "id": "00000000-0000-0000-0000-000000000001",
            "status": "running"
        }]))
        .unwrap(),
    )
    .unwrap();
    let prompt_suite = tmp.path().join("prompts.jsonl");
    std::fs::write(
        &prompt_suite,
        "\n{\"id\":\"a\",\"title\":\"A\",\"prompt\":\"A\"}\n{\"id\":\"a\",\"title\":\"B\",\"prompt\":\"B\"}\n{\"id\":\"bad\"}\n",
    )
    .unwrap();

    let (code, json, _stderr) = run(
        &bin,
        &[
            "role-experiment",
            "validate",
            "--project",
            "ProjectAlpha",
            "--experiment",
            "techlead-test",
            "--prompt-suite",
            &prompt_suite.to_string_lossy(),
        ],
    );
    assert_eq!(code, 1);
    assert_eq!(json["ok"], false);
    let codes: Vec<String> = json["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["code"].as_str().unwrap().to_string())
        .collect();
    assert!(codes.contains(&"replica_role_override_detected".to_string()));
    assert!(codes.contains(&"active_variant_session".to_string()));
    assert!(codes.contains(&"prompt_suite_duplicate_id".to_string()));
    assert!(codes.contains(&"prompt_suite_unparseable".to_string()));
    let prompt_error = json["errors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["code"] == "prompt_suite_unparseable")
        .unwrap();
    assert!(prompt_error["line"].as_u64().unwrap() >= 1);
    assert!(prompt_error["field"].is_string());
}

#[test]
fn role_experiment_run_without_dry_run_has_no_side_effects() {
    let tmp = Tmp::new("role-exp-run-gated");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_source(tmp.path());
    init_experiment(&bin);
    let suite = tmp.path().join("prompts.jsonl");
    write_prompt_suite(&suite);
    let workspace = project.join(".ac");
    let before = ac_snapshot(&workspace);

    let (code, json, _stderr) = run(
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
        ],
    );

    assert_eq!(code, 1);
    assert_eq!(json["ok"], false);
    assert_eq!(json["errors"][0]["code"], "run_execution_not_implemented");
    assert_eq!(before, ac_snapshot(&workspace));
}

#[test]
fn role_experiment_dry_run_writes_planned_artifacts() {
    let tmp = Tmp::new("role-exp-dry-run");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_source(tmp.path());
    init_experiment(&bin);
    let suite = tmp.path().join("prompts.jsonl");
    write_prompt_suite(&suite);
    let workspace = project.join(".ac");
    let before = ac_snapshot(&workspace);

    let json = run_ok(
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
            "--replicates",
            "2",
            "--seed",
            "12345",
            "--run-id",
            "20260601-181500",
            "--dry-run",
        ],
    );

    assert_eq!(json["data"]["status"], "dry_run");
    assert_eq!(json["data"]["dryRun"], true);
    assert_eq!(json["data"]["seed"], 12345);
    assert_eq!(json["data"]["seedProvided"], true);
    assert_eq!(json["data"]["attemptCount"], 8);
    let run_dir = workspace
        .join("experiments")
        .join("techlead-test")
        .join("runs")
        .join("20260601-181500");
    let after = ac_snapshot(&workspace);
    let new_paths: Vec<String> = after
        .keys()
        .filter(|path| !before.contains_key(*path))
        .cloned()
        .collect();
    assert_eq!(
        new_paths,
        vec![
            "experiments/techlead-test/runs/20260601-181500".to_string(),
            "experiments/techlead-test/runs/20260601-181500/attempts.jsonl".to_string(),
            "experiments/techlead-test/runs/20260601-181500/report.json".to_string(),
            "experiments/techlead-test/runs/20260601-181500/report.md".to_string(),
            "experiments/techlead-test/runs/20260601-181500/run.json".to_string(),
        ]
    );
    for (path, hash) in before {
        assert_eq!(after.get(&path), Some(&hash), "changed {}", path);
    }
    assert!(!run_dir.join("transcripts").exists());

    let run_artifact: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(run_dir.join("run.json")).unwrap()).unwrap();
    assert_eq!(run_artifact["status"], "dry_run");
    assert_eq!(run_artifact["suite"]["promptCount"], 2);
    let attempts: Vec<serde_json::Value> = std::fs::read_to_string(run_dir.join("attempts.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let ids: Vec<String> = attempts
        .iter()
        .map(|attempt| attempt["attemptId"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        ids,
        vec![
            "coord-001__control__r1",
            "coord-001__control__r2",
            "coord-001__strict__r1",
            "coord-001__strict__r2",
            "coord-002__control__r1",
            "coord-002__control__r2",
            "coord-002__strict__r1",
            "coord-002__strict__r2",
        ]
    );
    for attempt in attempts {
        assert_eq!(attempt["status"], "planned");
        assert!(attempt["durationMs"].is_null());
        assert!(attempt["messages"].is_null());
        assert!(attempt["transcriptPath"].is_null());
        assert!(attempt["failureReason"].is_null());
    }
}

#[test]
fn role_experiment_dry_run_generates_seed_and_rejects_existing_run_id() {
    let tmp = Tmp::new("role-exp-seed-collision");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    project_with_source(tmp.path());
    init_experiment(&bin);
    let suite = tmp.path().join("prompts.jsonl");
    write_prompt_suite(&suite);

    let first = run_ok(
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
    assert!(first["data"]["seed"].as_u64().is_some());
    assert_eq!(first["data"]["seedProvided"], false);

    let (code, second, _stderr) = run(
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
    assert_eq!(code, 1);
    assert_eq!(second["errors"][0]["code"], "run_id_exists");
}

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
