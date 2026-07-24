use std::path::{Path, PathBuf};
use std::process::Command;

mod support;

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
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
                .expect("make tmp dir private");
        }
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

fn copy_binary_into(tmp: &Path) -> PathBuf {
    let src = Path::new(env!("CARGO_BIN_EXE_agentscommander-new"));
    let dst = tmp.join(src.file_name().expect("binary file name"));
    support::copy_executable(src, &dst);
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

fn project_with_verified_coordinator(tmp: &Path) -> PathBuf {
    let project = tmp.join("ProjectAlpha");
    let workspace = project.join(".ac");
    let team_dir = workspace.join("_team_dev-team");
    let matrix = workspace.join("_agent_tech-lead");
    let wg = workspace.join("wg-1-dev-team");
    let replica = wg.join("__agent_tech-lead");
    for dir in [&team_dir, &matrix, &replica] {
        std::fs::create_dir_all(dir).expect("create project fixture");
    }
    std::fs::write(matrix.join("Role.md"), "# Tech Lead\n").expect("role");
    std::fs::write(
        team_dir.join("config.json"),
        r#"{"agents":["_agent_tech-lead"],"coordinator":"_agent_tech-lead","repos":[]}"#,
    )
    .expect("team config");
    std::fs::write(
        replica.join("config.json"),
        r#"{"identity":"../../_agent_tech-lead"}"#,
    )
    .expect("replica config");
    project
}

fn run_json(bin: &Path, args: &[&str]) -> serde_json::Value {
    let out = Command::new(bin).args(args).output().expect("spawn");
    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("stdout json")
}

fn run_stdout(bin: &Path, args: &[&str]) -> String {
    let out = Command::new(bin).args(args).output().expect("spawn");
    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn run_fail(bin: &Path, args: &[&str]) -> String {
    let out = Command::new(bin).args(args).output().expect("spawn");
    assert!(
        !out.status.success(),
        "expected failure\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stderr).to_string()
}

fn refresh_requests(config_dir: &Path) -> Vec<serde_json::Value> {
    let requests_dir = config_dir.join("project-refresh-requests");
    let mut paths = std::fs::read_dir(&requests_dir)
        .expect("read refresh dir")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| path.extension().and_then(|s| s.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    paths.sort();
    paths
        .iter()
        .map(|path| {
            serde_json::from_str(&std::fs::read_to_string(path).expect("read refresh"))
                .expect("refresh json")
        })
        .collect()
}

#[test]
fn loop_create_help_describes_busy_flags() {
    let tmp = Tmp::new("cli-loop-help");
    let bin = copy_binary_into(tmp.path());

    let help = run_stdout(&bin, &["loop", "create", "--help"]);
    assert!(help.contains("--busy-coordinator"));
    assert!(help.contains("--force-inject-when-busy"));
}

#[test]
fn loop_create_writes_policy_variants_and_list_summaries() {
    let tmp = Tmp::new("cli-loop-create");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_verified_coordinator(tmp.path());

    let created = run_json(
        &bin,
        &[
            "loop",
            "create",
            "--project",
            "ProjectAlpha",
            "--name",
            "Daily Standup",
            "--cron",
            "0 9 * * 1-5",
            "--workgroup",
            "wg-1-dev-team",
            "--prompt",
            "Summarize current status",
        ],
    );
    assert_eq!(created["summary"]["id"], "daily-standup");
    assert_eq!(
        created["summary"]["promptPreview"],
        "Summarize current status"
    );
    assert!(created["promptBody"].as_str().is_some());

    let default_config = std::fs::read_to_string(
        project
            .join(".ac")
            .join("_loop_daily-standup")
            .join("config.toml"),
    )
    .expect("default config");
    assert!(default_config.contains("busyCoordinator = \"waitUntilIdle\""));

    run_json(
        &bin,
        &[
            "loop",
            "create",
            "--project",
            "ProjectAlpha",
            "--name",
            "Force Flag",
            "--cron",
            "0 10 * * *",
            "--workgroup",
            "wg-1-dev-team",
            "--prompt",
            "Force prompt",
            "--force-inject-when-busy",
        ],
    );
    let force_config = std::fs::read_to_string(
        project
            .join(".ac")
            .join("_loop_force-flag")
            .join("config.toml"),
    )
    .expect("force config");
    assert!(force_config.contains("busyCoordinator = \"forceInject\""));

    run_json(
        &bin,
        &[
            "loop",
            "create",
            "--project",
            "ProjectAlpha",
            "--name",
            "Skip Busy",
            "--cron",
            "30 10 * * *",
            "--workgroup",
            "wg-1-dev-team",
            "--prompt",
            "Skip prompt",
            "--busy-coordinator",
            "skip",
        ],
    );
    let list = run_json(&bin, &["loop", "list", "--project", "ProjectAlpha"]);
    assert_eq!(list["loops"][0]["id"], "daily-standup");
    assert!(list["loops"].as_array().expect("loops").len() >= 3);
    assert_eq!(list["loops"][0]["expr"], "0 9 * * 1-5");
    assert_eq!(list["loops"][0]["workgroup"], "wg-1-dev-team");
    assert_eq!(list["loops"][0]["busyCoordinator"], "waitUntilIdle");

    let requests = refresh_requests(&config_dir);
    assert!(requests
        .iter()
        .any(|request| request["reason"] == "loopCreated"));
}

#[test]
fn loop_create_validates_cron_prompt_sources_and_busy_conflict() {
    let tmp = Tmp::new("cli-loop-validation");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    project_with_verified_coordinator(tmp.path());

    let conflict = run_fail(
        &bin,
        &[
            "loop",
            "create",
            "--project",
            "ProjectAlpha",
            "--name",
            "Bad Busy",
            "--cron",
            "0 9 * * *",
            "--workgroup",
            "wg-1-dev-team",
            "--prompt",
            "Prompt",
            "--force-inject-when-busy",
            "--busy-coordinator",
            "skip",
        ],
    );
    assert!(conflict.contains("conflicts"));

    let six_field = run_fail(
        &bin,
        &[
            "loop",
            "create",
            "--project",
            "ProjectAlpha",
            "--name",
            "Bad Cron",
            "--cron",
            "0 0 9 * * *",
            "--workgroup",
            "wg-1-dev-team",
            "--prompt",
            "Prompt",
        ],
    );
    assert!(six_field.contains("exactly 5 fields"));

    let both_prompt_sources = run_fail(
        &bin,
        &[
            "loop",
            "create",
            "--project",
            "ProjectAlpha",
            "--name",
            "Both Prompt",
            "--cron",
            "0 9 * * *",
            "--workgroup",
            "wg-1-dev-team",
            "--prompt",
            "Prompt",
            "--prompt-file",
            "missing.txt",
        ],
    );
    assert!(both_prompt_sources.contains("not both"));

    let no_prompt = run_fail(
        &bin,
        &[
            "loop",
            "create",
            "--project",
            "ProjectAlpha",
            "--name",
            "No Prompt",
            "--cron",
            "0 9 * * *",
            "--workgroup",
            "wg-1-dev-team",
        ],
    );
    assert!(no_prompt.contains("required"));
}

#[test]
fn loop_prompt_file_errors_are_clear() {
    let tmp = Tmp::new("cli-loop-prompt-file");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    project_with_verified_coordinator(tmp.path());
    let invalid_utf8 = tmp.path().join("invalid.bin");
    std::fs::write(&invalid_utf8, [0xff, 0xfe, 0xfd]).expect("invalid utf8");
    let too_large = tmp.path().join("too-large.txt");
    std::fs::write(&too_large, vec![b'a'; 128 * 1024 + 1]).expect("large file");

    let invalid = run_fail(
        &bin,
        &[
            "loop",
            "create",
            "--project",
            "ProjectAlpha",
            "--name",
            "Invalid Utf8",
            "--cron",
            "0 9 * * *",
            "--workgroup",
            "wg-1-dev-team",
            "--prompt-file",
            invalid_utf8.to_str().expect("path"),
        ],
    );
    assert!(invalid.contains("not valid UTF-8"));

    let large = run_fail(
        &bin,
        &[
            "loop",
            "create",
            "--project",
            "ProjectAlpha",
            "--name",
            "Large Prompt",
            "--cron",
            "0 9 * * *",
            "--workgroup",
            "wg-1-dev-team",
            "--prompt-file",
            too_large.to_str().expect("path"),
        ],
    );
    assert!(large.contains("larger than"));
}

#[test]
fn loop_existing_id_commands_reject_transformed_ids() {
    let tmp = Tmp::new("cli-loop-ids");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    project_with_verified_coordinator(tmp.path());

    for (cmd, flag) in [
        ("update", "--loop"),
        ("remove", "--loop"),
        ("enable", "--loop"),
        ("disable", "--loop"),
    ] {
        for bad in ["../daily", "daily!", "daily_loop"] {
            let mut args = vec!["loop", cmd, "--project", "ProjectAlpha", flag, bad];
            if cmd == "update" {
                args.extend(["--name", "New Name"]);
            }
            let stderr = run_fail(&bin, &args);
            assert!(
                stderr.contains("Invalid Loop id"),
                "cmd={cmd} bad={bad} stderr={stderr}"
            );
        }
    }
}
