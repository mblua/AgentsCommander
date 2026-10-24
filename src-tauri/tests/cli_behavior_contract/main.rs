use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard};

const VALID_TOKEN: &str = "00000000-0000-0000-0000-000000000487";
const LEAK_PROBE_TOKEN: &str = "zzzz-leakprobe-487-not-a-token";

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
    command.env_remove("AC_MACHINE_OUTPUT");
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

fn copy_binary_as(tmp: &Path, name: &str) -> PathBuf {
    let src = Path::new(env!("CARGO_BIN_EXE_agentscommander"));
    let dst = tmp.join(name);
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

fn run(bin: &Path, args: &[&str]) -> (Option<i32>, String, String) {
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
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn assert_no_stdout_on_error(stdout: &str) {
    assert!(
        stdout.trim().is_empty(),
        "error contract should not write stdout, got:\n{}",
        stdout
    );
}

fn assert_success_json_array(stdout: &str) -> Value {
    let value: Value = serde_json::from_str(stdout.trim()).expect("stdout JSON array");
    assert!(value.is_array(), "expected JSON array, got {}", value);
    value
}

fn assert_success_json_object(stdout: &str) -> Value {
    let value: Value = serde_json::from_str(stdout.trim()).expect("stdout JSON object");
    assert!(value.is_object(), "expected JSON object, got {}", value);
    value
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

fn project_with_agents(tmp: &Path, agents: &[&str]) -> PathBuf {
    let project = tmp.join("ProjectAlpha");
    let ac_root = project.join(".ac");
    std::fs::create_dir_all(&ac_root).expect("create .ac");
    for agent in agents {
        let dir = ac_root.join(format!("_agent_{}", agent));
        std::fs::create_dir_all(dir.join("memory")).expect("agent memory");
        std::fs::write(dir.join("Role.md"), format!("# {}\n", agent)).expect("role");
    }
    project
}

fn run_success_json(bin: &Path, args: &[&str]) -> Value {
    let (code, stdout, stderr) = run(bin, args);
    assert_eq!(code, Some(0), "stdout: {stdout}\nstderr: {stderr}");
    serde_json::from_str(stdout.trim()).expect("stdout json")
}

fn create_send_fixture(tmp: &Path, bin: &Path, config_dir: &Path) -> (PathBuf, PathBuf) {
    write_settings(config_dir, tmp);
    let project = project_with_agents(tmp, &["architect", "dev-rust"]);
    let _team = run_success_json(
        bin,
        &[
            "team",
            "create",
            "--project",
            "ProjectAlpha",
            "--team",
            "Dev Team",
            "--coordinator",
            "architect",
            "--agent",
            "dev-rust",
        ],
    );
    let _wg = run_success_json(
        bin,
        &[
            "workgroup",
            "add",
            "--project",
            "ProjectAlpha",
            "--team",
            "Dev Team",
            "--title",
            "Build",
        ],
    );
    // #1614 requirement (A): creation produces `room-<N>-<team>`. This fixture
    // names the directory the product CREATES, so it moves with the creation
    // prefix; the legacy `wg-*` fixtures that prove dual-prefix ACCEPTANCE are
    // untouched (Rule P2).
    let sender = project
        .join(".ac")
        .join("room-1-dev-team")
        .join("__agent_architect");
    assert!(sender.is_dir(), "sender replica root should exist");
    (project, sender)
}

// ── #738: task-set-title user-owned title output contract ───────────────────

fn seed_master_token(config_dir: &Path, token: &str) {
    std::fs::create_dir_all(config_dir).expect("create config dir");
    std::fs::write(config_dir.join("master-token.txt"), token).expect("write master-token.txt");
}

fn make_wg_agent_root(tmp: &Path) -> PathBuf {
    let root = tmp
        .join("proj")
        .join(".ac")
        .join("wg-1-dev-team")
        .join("__agent_architect");
    std::fs::create_dir_all(&root).expect("create agent root");
    root
}

/// Run `task-set-title` with `RUST_LOG=agentscommander=info` pinned, so the
/// clean-output assertions hold even under verbose logging: the AC_MACHINE_OUTPUT
/// allowlist must suppress stderr log lines and the startup log-path line.
fn run_task_title(
    bin: &Path,
    agent_root: &Path,
    master: &str,
    title: &str,
) -> (Option<i32>, String, String) {
    let mut command = command_for_binary(bin);
    command
        .args([
            "task-set-title",
            "--token",
            master,
            "--root",
            &agent_root.to_string_lossy(),
            "--title",
            title,
        ])
        .env("RUST_LOG", "agentscommander=info")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = {
        let _guard = spawn_lock();
        command.spawn().expect("spawn task-set-title")
    };
    let out = child.wait_with_output().expect("collect output");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn bak_files(wg_root: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(wg_root)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("TASK.") && n.ends_with(".bak.md"))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn assert_no_leak(stdout: &str, stderr: &str) {
    for stream in [stdout, stderr] {
        for needle in [
            "backup",
            "pid=",
            "[task]",
            "app.log",
            "INFO",
            "agentscommander",
            "module",
        ] {
            assert!(
                !stream.contains(needle),
                "normal output leaked {needle:?}:\nstdout={stdout:?}\nstderr={stderr:?}"
            );
        }
    }
}

#[test]
fn task_set_title_success_stdout_is_exact_updated() {
    let tmp = Tmp::new("task-title-ok");
    let bin = copy_binary_into(tmp.path());
    let master = "test-master-738-ok";
    seed_master_token(&config_dir_for_bin(&bin), master);
    let agent_root = make_wg_agent_root(tmp.path());
    let wg_root = agent_root.parent().unwrap().to_path_buf();

    let (code, stdout, stderr) = run_task_title(&bin, &agent_root, master, "Auto");
    assert_eq!(code, Some(0), "stdout: {stdout}\nstderr: {stderr}");
    assert_eq!(stdout, "Updated\n");
    assert!(stderr.trim().is_empty(), "stderr must be empty: {stderr}");

    let task = std::fs::read_to_string(wg_root.join("TASK.md")).expect("read TASK.md");
    assert!(
        task.contains("title: 'Auto'") && !task.contains("USER:"),
        "coordinator title must be plain, not USER:-marked:\n{task}"
    );
}

#[test]
fn task_set_title_rejects_user_owned_title_with_exact_output() {
    let tmp = Tmp::new("task-title-reject");
    let bin = copy_binary_into(tmp.path());
    let master = "test-master-738-reject";
    seed_master_token(&config_dir_for_bin(&bin), master);
    let agent_root = make_wg_agent_root(tmp.path());
    let wg_root = agent_root.parent().unwrap().to_path_buf();
    let task_path = wg_root.join("TASK.md");
    std::fs::write(&task_path, "---\ntitle: 'USER: Manual'\n---\n").unwrap();
    let before = std::fs::read(&task_path).unwrap();

    let (code, stdout, stderr) = run_task_title(&bin, &agent_root, master, "Auto");
    assert_eq!(code, Some(0), "stdout: {stdout}\nstderr: {stderr}");
    assert_eq!(stdout, "Rejected: title set by user\n");
    assert!(stderr.trim().is_empty(), "stderr must be empty: {stderr}");
    assert_eq!(
        std::fs::read(&task_path).unwrap(),
        before,
        "TASK.md must be byte-unchanged on reject"
    );
    assert!(bak_files(&wg_root).is_empty(), "no backup on reject");
}

#[test]
fn task_set_title_reserved_user_prefix_input_is_invalid() {
    let tmp = Tmp::new("task-title-reserved");
    let bin = copy_binary_into(tmp.path());
    let master = "test-master-738-reserved";
    seed_master_token(&config_dir_for_bin(&bin), master);
    let agent_root = make_wg_agent_root(tmp.path());
    let wg_root = agent_root.parent().unwrap().to_path_buf();
    let task_path = wg_root.join("TASK.md");
    std::fs::write(&task_path, "---\ntitle: 'Auto'\n---\n").unwrap();
    let before = std::fs::read(&task_path).unwrap();

    let (code, stdout, stderr) = run_task_title(&bin, &agent_root, master, "USER: forged");
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert!(stdout.is_empty(), "no stdout on invalid input: {stdout}");
    assert_eq!(
        stderr,
        "Error: --title cannot start with reserved USER: prefix\n"
    );
    assert_eq!(
        std::fs::read(&task_path).unwrap(),
        before,
        "TASK.md must be byte-unchanged on invalid input"
    );
    assert!(bak_files(&wg_root).is_empty(), "no backup on invalid input");
}

#[test]
fn task_set_title_normal_output_does_not_leak_logs_or_paths() {
    let tmp = Tmp::new("task-title-leak");
    let bin = copy_binary_into(tmp.path());
    let master = "test-master-738-leak";
    seed_master_token(&config_dir_for_bin(&bin), master);
    let agent_root = make_wg_agent_root(tmp.path());
    let wg_root = agent_root.parent().unwrap().to_path_buf();

    // Accepted path (fresh file -> Wrote): exact stdout, no log/path leak.
    let (code, stdout, stderr) = run_task_title(&bin, &agent_root, master, "First");
    assert_eq!(code, Some(0), "stderr: {stderr}");
    assert_eq!(stdout, "Updated\n");
    assert_no_leak(&stdout, &stderr);

    // Rejected path (seed a USER: title so the next coordinator write is rejected).
    std::fs::write(wg_root.join("TASK.md"), "---\ntitle: 'USER: Manual'\n---\n").unwrap();
    let (code, stdout, stderr) = run_task_title(&bin, &agent_root, master, "Second");
    assert_eq!(code, Some(0), "stderr: {stderr}");
    assert_eq!(stdout, "Rejected: title set by user\n");
    assert_no_leak(&stdout, &stderr);
}

// Copied-binary P2 acceptance exercises the actual pre-logger selector.
struct TaskStatusFixture {
    tmp: Tmp,
    bin: PathBuf,
    root: PathBuf,
    room: PathBuf,
    token: String,
}

impl TaskStatusFixture {
    fn new(prefix: &str) -> Self {
        let tmp = Tmp::new("task-status");
        let bin = copy_binary_into(tmp.path());
        let token = format!("master-status-{}", uuid::Uuid::new_v4());
        seed_master_token(&config_dir_for_bin(&bin), &token);
        let root = tmp
            .path()
            .join("proj/.ac")
            .join(prefix)
            .join("__agent_architect");
        std::fs::create_dir_all(&root).unwrap();
        let room = root.parent().unwrap().to_path_buf();
        assert!(!config_dir_for_bin(&bin).join("app.log").exists());
        Self {
            tmp,
            bin,
            root,
            room,
            token,
        }
    }

    fn invoke(&self, args: &[&str]) -> (Option<i32>, String, String) {
        let mut command = command_for_binary(&self.bin);
        command
            .args(args)
            .env_remove("AC_MACHINE_OUTPUT")
            .env("RUST_LOG", "agentscommander=info")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let child = {
            let _guard = spawn_lock();
            command.spawn().expect("spawn task contract")
        };
        let out = child.wait_with_output().expect("collect task contract");
        (
            out.status.code(),
            String::from_utf8(out.stdout).unwrap(),
            String::from_utf8(out.stderr).unwrap(),
        )
    }

    fn get(&self) -> (Option<i32>, String, String) {
        self.invoke(&[
            "task-get",
            "--token",
            &self.token,
            "--root",
            &self.root.to_string_lossy(),
        ])
    }

    fn set(&self, base: &str, id: &str, text: &str) -> (Option<i32>, String, String) {
        self.invoke(&[
            "task-status-set",
            "--token",
            &self.token,
            "--root",
            &self.root.to_string_lossy(),
            "--expected-revision",
            base,
            "--request-id",
            id,
            "--text",
            text,
        ])
    }
}

const TASK_SNAPSHOT_KEYS: &[&str] = &[
    "workgroupRoot",
    "task",
    "taskTitle",
    "description",
    "status",
    "revision",
    "statusRecord",
    "tailIncomplete",
];
const TASK_RECEIPT_KEYS: &[&str] = &[
    "workgroupRoot",
    "revision",
    "requestId",
    "recordedAt",
    "status",
    "replayed",
];

fn task_json_line(stream: &str, keys: &[&str]) -> Value {
    assert!(stream.ends_with('\n') && !stream.ends_with("\r\n"));
    assert_eq!(stream.bytes().filter(|b| *b == b'\n').count(), 1);
    let value: Value = serde_json::from_str(stream).expect("one JSON object");
    let mut actual: Vec<_> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let mut expected = keys.to_vec();
    actual.sort_unstable();
    expected.sort_unstable();
    assert_eq!(actual, expected);
    value
}

fn task_success(out: (Option<i32>, String, String), keys: &[&str]) -> Value {
    assert_eq!(out.0, Some(0), "stdout={} stderr={}", out.1, out.2);
    assert_eq!(out.2, "");
    task_json_line(&out.1, keys)
}

fn task_failure(out: (Option<i32>, String, String), code: i32, error: &str) -> Value {
    assert_eq!(out.0, Some(code), "stdout={} stderr={}", out.1, out.2);
    assert_eq!(out.1, "");
    let keys = if code == 2 {
        vec!["error", "message", "currentRevision"]
    } else {
        vec!["error", "message"]
    };
    let value = task_json_line(&out.2, &keys);
    assert_eq!(value["error"], error);
    value
}

#[test]
fn task_get_missing_zero_byte_and_full_legacy_unicode_snapshot() {
    let f = TaskStatusFixture::new("room-1-dev-team");
    let missing = task_success(f.get(), TASK_SNAPSHOT_KEYS);
    assert!(
        missing["task"].is_null()
            && missing["status"].is_null()
            && missing["statusRecord"].is_null()
    );
    assert_eq!(missing["revision"], "legacy:0");
    assert_eq!(missing["tailIncomplete"], false);
    let task = "---\ntitle: 'USER: Manual 🦀'\n---\n\nLegacy body\r\ncontinuación\t完整\n";
    std::fs::write(f.room.join("TASK.md"), task).unwrap();
    std::fs::write(f.room.join("TASK-status.jsonl"), "").unwrap();
    let snapshot = task_success(f.get(), TASK_SNAPSHOT_KEYS);
    assert_eq!(snapshot["task"], task);
    assert_eq!(snapshot["taskTitle"], "USER: Manual 🦀");
    assert!(snapshot["description"]
        .as_str()
        .unwrap()
        .contains("Legacy body"));
    assert!(snapshot["status"].is_null());
    assert_eq!(
        std::fs::read(f.room.join("TASK-status.jsonl")).unwrap(),
        b""
    );
}

#[test]
fn task_status_write_retry_conflict_and_overtaken_preserve_user_task() {
    let f = TaskStatusFixture::new("wg-1-dev-team");
    let task = "---\ntitle: 'USER: Manual'\n---\nLegacy body\n";
    std::fs::write(f.room.join("TASK.md"), task).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let text = "P2-status-body-sentinel 🦀\nRemaining #2840\r\tFUP continuation";
    let receipt = task_success(f.set("legacy:0", &id, text), TASK_RECEIPT_KEYS);
    assert_eq!(receipt["status"], text);
    assert_eq!(receipt["requestId"], id);
    assert_eq!(receipt["replayed"], false);
    assert!(chrono::DateTime::parse_from_rfc3339(receipt["recordedAt"].as_str().unwrap()).is_ok());
    let path = f.room.join("TASK-status.jsonl");
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(bytes.iter().filter(|b| **b == b'\n').count(), 1);
    let retry = task_success(f.set("legacy:0", &id, text), TASK_RECEIPT_KEYS);
    for key in [
        "revision",
        "requestId",
        "recordedAt",
        "status",
        "workgroupRoot",
    ] {
        assert_eq!(receipt[key], retry[key]);
    }
    assert_eq!(retry["replayed"], true);
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    task_failure(f.set("legacy:0", &id, "changed"), 1, "request_id_conflict");
    let stale = task_failure(
        f.set("legacy:0", &uuid::Uuid::new_v4().to_string(), "stale"),
        2,
        "revision_conflict",
    );
    assert_eq!(stale["currentRevision"], receipt["revision"]);
    let snapshot = task_success(f.get(), TASK_SNAPSHOT_KEYS);
    assert_eq!(snapshot["status"], text);
    assert_eq!(
        snapshot["statusRecord"]["author"],
        "proj:wg-1-dev-team/architect"
    );
    let next = task_success(
        f.set(
            receipt["revision"].as_str().unwrap(),
            &uuid::Uuid::new_v4().to_string(),
            "next",
        ),
        TASK_RECEIPT_KEYS,
    );
    let overtaken = task_failure(f.set("legacy:0", &id, text), 2, "revision_conflict");
    assert_eq!(overtaken["currentRevision"], next["revision"]);
    assert_eq!(
        std::fs::read(&path)
            .unwrap()
            .iter()
            .filter(|b| **b == b'\n')
            .count(),
        2
    );
    assert_eq!(
        std::fs::read_to_string(f.room.join("TASK.md")).unwrap(),
        task
    );
    assert!(bak_files(&f.room).is_empty());
    let log = std::fs::read_to_string(config_dir_for_bin(&f.bin).join("app.log")).unwrap();
    assert!(
        log.contains("[task] get:")
            && log.contains("replayed=false")
            && log.contains("replayed=true")
    );
    assert!(!log.contains("P2-status-body-sentinel") && !log.contains(&f.token));
}

#[test]
fn task_get_partial_corrupt_and_post_clean_seed_revision() {
    let f = TaskStatusFixture::new("room-1-dev-team");
    let log = f.room.join("TASK-status.jsonl");
    std::fs::write(&log, b"{partial").unwrap();
    let partial = task_success(f.get(), TASK_SNAPSHOT_KEYS);
    assert_eq!(partial["revision"], "legacy:0");
    assert_eq!(partial["tailIncomplete"], true);
    assert!(partial["status"].is_null());
    task_success(
        f.set("legacy:0", &uuid::Uuid::new_v4().to_string(), "recovered"),
        TASK_RECEIPT_KEYS,
    );
    assert!(std::fs::read_dir(&f.room).unwrap().flatten().any(|e| e
        .file_name()
        .to_string_lossy()
        .starts_with("TASK-status.partial.")));
    std::fs::write(&log, b"{bad}\n").unwrap();
    task_failure(f.get(), 1, "status_corrupt");
    task_failure(
        f.set("legacy:0", &uuid::Uuid::new_v4().to_string(), "no mutation"),
        1,
        "status_corrupt",
    );
    assert_eq!(std::fs::read(&log).unwrap(), b"{bad}\n");
    let topic = uuid::Uuid::new_v4();
    let seed = serde_json::json!({
        "schemaVersion": 1, "kind": "topic_started", "topicId": topic, "sequence": 0,
        "requestId": null, "baseRevision": null, "recordedAt": "2026-10-03T00:00:00Z",
        "author": null, "status": null
    });
    std::fs::write(&log, format!("{seed}\n")).unwrap();
    let snapshot = task_success(f.get(), TASK_SNAPSHOT_KEYS);
    assert_eq!(snapshot["revision"], format!("{topic}:0"));
    assert!(snapshot["status"].is_null());
    task_failure(
        f.set(
            "legacy:0",
            &uuid::Uuid::new_v4().to_string(),
            "pre-Clean revision",
        ),
        2,
        "revision_conflict",
    );
    task_success(
        f.set(
            snapshot["revision"].as_str().unwrap(),
            &uuid::Uuid::new_v4().to_string(),
            "new topic",
        ),
        TASK_RECEIPT_KEYS,
    );
}

#[test]
fn task_status_invalid_inputs_and_encoded_size_do_not_mutate() {
    let f = TaskStatusFixture::new("room-1-dev-team");
    let id = uuid::Uuid::new_v4().to_string();
    for text in [
        "".to_owned(),
        " \t\n".to_owned(),
        "bad\u{7}".to_owned(),
        "界".repeat(16 * 1024 + 1),
        format!("{}{}", "\t".repeat(25000), "界".repeat(5200)),
    ] {
        task_failure(f.set("legacy:0", &id, &text), 1, "invalid_input");
        assert!(!f.room.join("TASK-status.jsonl").exists());
    }
    task_failure(f.set("bad:0", &id, "text"), 1, "invalid_input");
    task_failure(f.set("legacy:0", "bad-id", "text"), 1, "invalid_input");
}

#[test]
fn task_admission_invalid_token_empty_teams_nonroom_and_nested_room() {
    let f = TaskStatusFixture::new("room-1-dev-team");
    for token in [LEAK_PROBE_TOKEN, VALID_TOKEN] {
        let out = f.invoke(&[
            "task-get",
            "--token",
            token,
            "--root",
            &f.root.to_string_lossy(),
        ]);
        assert!(!out.2.contains(token));
        task_failure(out, 1, "authorization_denied");
    }
    for root in [
        f.tmp.path().to_path_buf(),
        f.tmp.path().join("nested/room-9-invalid/__agent_architect"),
    ] {
        std::fs::create_dir_all(&root).unwrap();
        task_failure(
            f.invoke(&[
                "task-get",
                "--token",
                &f.token,
                "--root",
                &root.to_string_lossy(),
            ]),
            1,
            "authorization_denied",
        );
    }
    assert!(!f.room.join("TASK-status.jsonl").exists());
}

#[test]
fn task_admission_honest_coordinator_uuid_noncoordinator_and_root_token() {
    let f = TaskStatusFixture::new("room-1-dev-team");
    let cfg = config_dir_for_bin(&f.bin);
    let (_project, sender) = create_send_fixture(f.tmp.path(), &f.bin, &cfg);
    task_success(
        f.invoke(&[
            "task-get",
            "--token",
            VALID_TOKEN,
            "--root",
            &sender.to_string_lossy(),
        ]),
        TASK_SNAPSHOT_KEYS,
    );
    let peer = sender.parent().unwrap().join("__agent_dev-rust");
    task_failure(
        f.invoke(&[
            "task-get",
            "--token",
            VALID_TOKEN,
            "--root",
            &peer.to_string_lossy(),
        ]),
        1,
        "authorization_denied",
    );
    let settings = cfg.join("settings.30.instance.no-git.json");
    let mut value: Value = serde_json::from_slice(&std::fs::read(&settings).unwrap()).unwrap();
    value["rootToken"] = "fixture-root-token".into();
    std::fs::write(&settings, serde_json::to_vec(&value).unwrap()).unwrap();
    task_success(
        f.invoke(&[
            "task-get",
            "--token",
            "fixture-root-token",
            "--root",
            &f.root.to_string_lossy(),
        ]),
        TASK_SNAPSHOT_KEYS,
    );
}

#[test]
fn task_syntax_help_version_preserve_clap_contract() {
    let f = TaskStatusFixture::new("room-1-dev-team");
    for args in [
        vec!["task-get"],
        vec!["task-get", "--unknown"],
        vec!["task-status-set", "--token", "fixture", "--root", "fixture"],
    ] {
        let out = f.invoke(&args);
        assert_eq!(out.0, Some(1));
        assert_eq!(out.1, "");
        assert!(out.2.contains("error:") && serde_json::from_str::<Value>(&out.2).is_err());
    }
    for args in [
        vec!["task-get", "--help"],
        vec!["task-status-set", "--help"],
        vec!["--version"],
    ] {
        let out = f.invoke(&args);
        assert_eq!(out.0, Some(0));
        assert_eq!(out.2, "");
        assert!(!out.1.is_empty());
    }
    assert!(!config_dir_for_bin(&f.bin).join("app.log").exists());
}

#[test]
fn task_inherited_startup_failure_precedes_handler_json() {
    let f = TaskStatusFixture::new("room-1-dev-team");
    let cfg = config_dir_for_bin(&f.bin);
    std::fs::remove_dir_all(&cfg).unwrap();
    std::fs::write(&cfg, "blocked config directory").unwrap();
    let out = f.get();
    assert_eq!(out.0, Some(1));
    assert!(serde_json::from_str::<Value>(&out.2).is_err());
    assert!(!out.2.is_empty());
    assert!(!out.2.contains("authorization_denied"));
}

#[test]
fn telegram_send_message_local_rejections_and_missing_config() {
    let tmp = Tmp::new("cli-telegram-message");
    let bin = copy_binary_into(tmp.path());
    // The existing runner pins the public config override for this unsuffixed
    // copy; it takes precedence over inherited debug config and selects no real
    // settings or credentials. ROOT/TOKEN/LOCAL_DIR do not select Telegram bots.
    assert!(!bin.file_stem().unwrap().to_string_lossy().contains('_'));
    let config = config_dir_for_bin(&bin);
    assert!(!config.exists());
    let cases = [
        (" \n\t".to_string(), "--text must not be empty"),
        (
            "a".repeat(4097),
            "--text must not exceed 4096 UTF-16 code units",
        ),
        (
            "😀".repeat(2049),
            "--text must not exceed 4096 UTF-16 code units",
        ),
        (
            "BODY_SENTINEL".to_string(),
            "No Telegram bots are configured",
        ),
        ("-value".to_string(), "No Telegram bots are configured"),
    ];
    for (text, expected) in cases {
        let text_arg = format!("--text={text}");
        let (code, stdout, stderr) = run(&bin, &["telegram-send-message", &text_arg]);
        assert_eq!(code, Some(1));
        assert_no_stdout_on_error(&stdout);
        assert!(stderr.contains(expected), "{stderr}");
        assert!(!stderr.contains("BODY_SENTINEL"));
        assert!(!stderr.contains("Sent:"));
        let log = config.join("app.log");
        if log.exists() {
            assert!(!std::fs::read_to_string(log)
                .unwrap()
                .contains("BODY_SENTINEL"));
        }
    }
}

mod public_commands;
