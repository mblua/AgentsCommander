use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard};

const FAKE_EXECUTOR_TEST_ENV: &str = "AGENTSCOMMANDER_ROLE_EXPERIMENT_TEST_FAKE_EXECUTOR";

/// Excludes one test's open write descriptor on a freshly copied binary from
/// overlapping another test's fork/exec.
///
/// These tests run in parallel and each copies the binary into its own temp dir
/// before exec'ing it. `Command::spawn` forks, and the child inherits the write
/// descriptor another thread still holds on *its* copy; exec'ing a binary that
/// any process holds open for writing fails with `ETXTBSY`. Covering both the
/// copy and the spawn closes that window. The lock is released before output is
/// collected, so the binary runs themselves still overlap.
static SPAWN_LOCK: Mutex<()> = Mutex::new(());

fn spawn_lock() -> MutexGuard<'static, ()> {
    // A test that panics elsewhere must not disable the guard for the rest.
    SPAWN_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn command_for_binary(bin: &Path) -> Command {
    let mut command = Command::new(bin);
    let stem = bin.file_stem().expect("bin stem").to_string_lossy();
    if !stem.contains('_') {
        command.env("AGENTSCOMMANDER_CONFIG_DIR", config_dir_for_bin(bin));
    }
    command
}

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
    let src = Path::new(env!("CARGO_BIN_EXE_agentscommander"));
    let dst = tmp.join(src.file_name().expect("binary file name"));
    {
        let _guard = spawn_lock();
        std::fs::copy(src, &dst).expect("copy binary");
    }
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
    let mut command = command_for_binary(bin);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = {
        let _guard = spawn_lock();
        command.spawn().expect("spawn")
    };
    let out = child.wait_with_output().expect("collect output");
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

fn run_fake(bin: &Path, args: &[&str]) -> (i32, serde_json::Value, String) {
    let mut command = command_for_binary(bin);
    command
        .env(FAKE_EXECUTOR_TEST_ENV, "1")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = {
        let _guard = spawn_lock();
        command.spawn().expect("spawn")
    };
    let out = child.wait_with_output().expect("collect output");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("stdout json: {}\n{}", e, stdout));
    (
        out.status.code().unwrap_or(-1),
        json,
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn run_fake_ok(bin: &Path, args: &[&str]) -> serde_json::Value {
    let (code, json, stderr) = run_fake(bin, args);
    assert_eq!(code, 0, "stdout: {}\nstderr: {}", json, stderr);
    assert_eq!(json["ok"], true, "{}", json);
    json
}

fn ac_snapshot(root: &Path) -> BTreeMap<String, Option<String>> {
    let mut out = BTreeMap::new();
    snapshot_into(root, root, &mut out);
    out
}

fn team_snapshot(root: &Path) -> BTreeMap<String, Option<String>> {
    ac_snapshot(root)
        .into_iter()
        .filter(|(path, _)| path.starts_with("_team_"))
        .collect()
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
            concat!(
                "{\"id\":\"coord-001\",\"title\":\"Bug in CI\",\"prompt\":\"Fix it",
                "\",\"tags\":[\"ci\"],\"expectedBehaviors\":[\"delegate\"]}\n",
            ),
            "{\"id\":\"coord-002\",\"title\":\"Triage\",\"prompt\":\"Triage it\"}\n",
        ),
    )
    .unwrap();
}

struct FakeRunFixture {
    _tmp: Tmp,
    bin: PathBuf,
    suite: PathBuf,
    run_dir: PathBuf,
}

fn completed_fake_run(prefix: &str, extra_run_args: &[&str]) -> FakeRunFixture {
    let tmp = Tmp::new(prefix);
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, tmp.path());
    let project = project_with_source(tmp.path());
    init_experiment(&bin);
    let suite = tmp.path().join("prompts.jsonl");
    write_prompt_suite(&suite);
    let mut args = vec![
        "role-experiment",
        "run",
        "--project",
        "ProjectAlpha",
        "--experiment",
        "techlead-test",
        "--prompt-suite",
        suite.to_str().expect("suite path"),
        "--run-id",
        "20260601-181500",
        "--execute",
        "--fake-executor",
    ];
    args.extend_from_slice(extra_run_args);
    run_fake_ok(&bin, &args);
    let run_dir = project
        .join(".ac")
        .join("experiments")
        .join("techlead-test")
        .join("runs")
        .join("20260601-181500");
    FakeRunFixture {
        _tmp: tmp,
        bin,
        suite,
        run_dir,
    }
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

mod execution;
mod reports;
mod variants;
