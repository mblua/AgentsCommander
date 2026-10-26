use super::*;
use crate::commands::entity_creation::{
    create_workgroup_on_disk, RepoAssignment, WorkgroupDiskCreateArgs,
};
use serde_json::Value;
use std::collections::HashMap;
use std::process::Command;
use std::sync::Mutex;
use std::task::{Context, Poll, Waker};

const CHILD: &str = "commands::entity_creation::creation_trace::tests::creation_trace_child";
const MARKER: &str = "AC_ROOM_CREATION_TRACE_TEST_CHILD";
const SECRET: &str = "sensitive-title-url-error-sentinel";
static RECORDS: Mutex<Vec<(log::Level, String)>> = Mutex::new(Vec::new());
struct Capture;
static CAPTURE: Capture = Capture;

impl log::Log for Capture {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.target() == TARGET
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            RECORDS
                .lock()
                .unwrap()
                .push((record.level(), record.args().to_string()));
        }
    }

    fn flush(&self) {}
}

#[test]
fn creation_trace_real_creation_is_observable_in_isolated_process() {
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", CHILD, "--nocapture"])
        .env(MARKER, "1")
        .output()
        .expect("launch isolated logger child");
    assert!(
        output.status.success(),
        "trace child failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("trace scenarios completed"));
}

#[test]
fn creation_trace_child() {
    if std::env::var(MARKER).as_deref() != Ok("1") {
        return;
    }
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<CreationTrace>();
    assert_send_sync::<StageGuard<'_>>();
    log::set_logger(&CAPTURE).unwrap();
    log::set_max_level(log::LevelFilter::Info);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(real_creation_scenarios());
    interrupted_and_interleaved();
    bounded_vocabulary_and_results();
    println!("trace scenarios completed");
}

fn take_records() -> Vec<Value> {
    let lines = std::mem::take(&mut *RECORDS.lock().unwrap());
    lines
        .into_iter()
        .map(|(level, line)| {
            assert!(line.len() <= 1024, "oversized diagnostic: {}", line.len());
            assert!(
                !line.contains(SECRET),
                "new diagnostic leaked sensitive input"
            );
            let record: Value = serde_json::from_str(line.strip_prefix("room_creation ").unwrap())
                .expect("compact diagnostic must be JSON");
            let warning = matches!(
                record["outcome"].as_str(),
                Some("error" | "timeout" | "interrupted")
            );
            assert_eq!(
                level,
                if warning {
                    log::Level::Warn
                } else {
                    log::Level::Info
                }
            );
            assert_eq!(record["v"], 1);
            Uuid::parse_str(record["operation_id"].as_str().unwrap()).unwrap();
            assert!(record["elapsed_ms"].is_u64());
            assert!(record["operation_ms"].is_u64());
            record
        })
        .collect()
}

fn event<'a>(records: &'a [Value], stage: &str, kind: &str) -> &'a Value {
    records
        .iter()
        .find(|r| r["stage"] == stage && r["event"] == kind)
        .unwrap_or_else(|| panic!("missing {stage}/{kind}: {records:#?}"))
}

fn check_pairs(records: &[Value], expected: &str) {
    let first = records.first().unwrap();
    assert_eq!(first["stage"], "operation");
    assert_eq!(first["event"], "start");
    let last = records.last().unwrap();
    assert_eq!(last["stage"], "operation");
    assert_eq!(last["event"], "end");
    assert_eq!(last["outcome"], expected, "operation outcome");
    let mut starts: HashMap<u64, &Value> = HashMap::new();
    let mut ends: HashMap<u64, &Value> = HashMap::new();
    for record in records {
        assert_eq!(record["operation_id"], first["operation_id"]);
        assert_eq!(record["origin"], "on_disk");
        assert!(record["operation_ms"].as_u64().unwrap() <= last["operation_ms"].as_u64().unwrap());
        assert!(record["elapsed_ms"].as_u64().unwrap() <= last["elapsed_ms"].as_u64().unwrap());
        let id = record["stage_id"].as_u64().unwrap();
        if let Some(parent) = record["parent_stage_id"].as_u64() {
            assert!(
                starts.contains_key(&parent),
                "parent must start before child"
            );
            assert!(
                !ends.contains_key(&parent),
                "parent must remain open for child"
            );
        }
        if record["event"] == "start" {
            assert_eq!(record["elapsed_ms"], 0);
            assert!(starts.insert(id, record).is_none(), "duplicate start");
        } else {
            assert_eq!(record["event"], "end");
            assert!(ends.insert(id, record).is_none(), "duplicate end");
            if record["outcome"] == "skip" {
                assert!(!starts.contains_key(&id));
                assert_eq!(record["elapsed_ms"], 0);
            } else {
                let start = starts.get(&id).expect("end without start");
                assert_eq!(record["stage"], start["stage"]);
                assert_eq!(record["parent_stage_id"], start["parent_stage_id"]);
                assert_eq!(record["agent_index"], start["agent_index"]);
                assert_eq!(record["repo_index"], start["repo_index"]);
            }
        }
    }
    for id in starts.keys() {
        assert!(ends.contains_key(id), "stage left open on normal return");
    }
    for record in records.iter().filter(|r| r["event"] == "end") {
        if let Some(parent) = record["parent_stage_id"].as_u64() {
            assert!(
                record["elapsed_ms"].as_u64().unwrap()
                    <= ends[&parent]["elapsed_ms"].as_u64().unwrap()
            );
        }
    }
}

fn git(dir: &Path, args: &[&str]) {
    let mut command = Command::new("git");
    command
        .current_dir(dir)
        .args([
            "-c",
            "user.name=AC-Test",
            "-c",
            "user.email=ac-test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "git fixture: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn args(project: &Path, repos: Vec<RepoAssignment>) -> WorkgroupDiskCreateArgs {
    WorkgroupDiskCreateArgs {
        project_path: project.to_path_buf(),
        team_name: "dev-team".to_string(),
        task_title: SECRET.to_string(),
        coordinator: Some("_agent_coordinator".to_string()),
        agents: vec!["_agent_coordinator".to_string()],
        repos,
    }
}

fn assignment(url: &str) -> RepoAssignment {
    RepoAssignment {
        url: url.to_string(),
        agents: vec!["_agent_coordinator".to_string()],
    }
}

fn project_fixture(root: &Path, name: &str) -> std::path::PathBuf {
    let project = root.join(name);
    std::fs::create_dir_all(project.join(".ac/_agent_coordinator")).unwrap();
    project
}

async fn real_creation_scenarios() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source").join(SECRET);
    std::fs::create_dir_all(&source).unwrap();
    git(&source, &["init"]);
    // Version the checkout policy so inherited autocrlf cannot change fixture bytes.
    std::fs::write(source.join(".gitattributes"), "fixture.txt text eol=lf\n").unwrap();
    std::fs::write(source.join("fixture.txt"), "local repository\n").unwrap();
    git(&source, &["add", ".gitattributes", "fixture.txt"]);
    git(&source, &["commit", "-m", "fixture"]);
    let url = source.to_string_lossy().replace(char::from(92), "/");
    // Different URL, identical basename: existing CLI target must be skipped.
    let alias = tmp
        .path()
        .join("missing-alias")
        .join(SECRET)
        .to_string_lossy()
        .replace(char::from(92), "/");
    let project = project_fixture(tmp.path(), "success");
    let created = create_workgroup_on_disk(args(
        &project,
        vec![assignment(&url), assignment(&url), assignment(&alias)],
    ))
    .await
    .expect("real creation with local clone");
    assert!(created.clone_errors.is_empty());
    let room = Path::new(&created.path);
    assert!(room.join("room-shared").is_dir());
    assert!(room.join("messaging").is_dir());
    assert!(room.join("__agent_coordinator/config.json").is_file());
    assert_eq!(
        std::fs::read_to_string(room.join("TASK.md")).unwrap(),
        format!("---\ntitle: 'USER: {SECRET}'\n---\n")
    );
    assert_eq!(
        std::fs::read_to_string(room.join(format!("repo-{SECRET}/fixture.txt"))).unwrap(),
        "local repository\n"
    );
    let records = take_records();
    check_pairs(&records, "ok");
    for stage in [
        "physical",
        "prepare",
        "team_guard",
        "allocate",
        "replicas",
        "repos",
    ] {
        assert_eq!(event(&records, stage, "end")["outcome"], "ok");
    }
    assert_eq!(
        event(&records, "replica", "start")["agent_index"],
        1,
        "real replica must be traced"
    );
    assert_eq!(event(&records, "replica", "end")["agent_index"], 1);
    assert_eq!(
        event(&records, "clone_process", "start")["repo_index"],
        1,
        "real clone_process must be traced for repo 1"
    );
    assert_eq!(event(&records, "clone_process", "end")["outcome"], "ok");
    for (index, reason) in [(2, "duplicate_url"), (3, "target_exists")] {
        assert!(records.iter().any(|r| r["stage"] == "repo"
            && r["repo_index"] == index
            && r["outcome"] == "skip"
            && r["reason"] == reason));
    }
    let physical = event(&records, "physical", "end");
    assert_eq!(physical["room_number"], 1);
    assert_eq!(physical["clone_error_count"], 0);
    assert_eq!(physical["project_key"].as_str().unwrap().len(), 64);
    assert_eq!(
        event(&records, "operation", "end")["project_key"],
        physical["project_key"]
    );

    let missing = tmp
        .path()
        .join(format!("missing-{SECRET}"))
        .to_string_lossy()
        .replace(char::from(92), "/");
    let project = project_fixture(tmp.path(), "partial");
    let created = create_workgroup_on_disk(args(&project, vec![assignment(&missing)]))
        .await
        .expect("clone failure must preserve Ok result");
    assert_eq!(created.clone_errors.len(), 1);
    assert_eq!(created.clone_errors[0].url, missing);
    assert!(created.clone_errors[0].error.contains("git clone failed:"));
    assert!(Path::new(&created.path).join("TASK.md").is_file());
    let records = take_records();
    check_pairs(&records, "partial");
    assert_eq!(event(&records, "physical", "end")["outcome"], "partial");
    assert_eq!(event(&records, "operation", "end")["clone_error_count"], 1);
    assert_eq!(event(&records, "clone_process", "end")["outcome"], "error");
    assert_eq!(event(&records, "clone_process", "end")["reason"], "failed");

    let project = project_fixture(tmp.path(), "invalid");
    let mut invalid = args(&project, Vec::new());
    invalid.coordinator = None;
    invalid.agents.clear();
    invalid.team_name = SECRET.to_string();
    let error = create_workgroup_on_disk(invalid).await.unwrap_err();
    assert!(
        error.contains(SECRET),
        "fixture must exercise sensitive Err text"
    );
    let records = take_records();
    check_pairs(&records, "error");
    assert_eq!(event(&records, "prepare", "end")["outcome"], "error");
    assert_eq!(event(&records, "physical", "end")["outcome"], "error");
    assert!(!records.iter().any(|r| r["stage"] == "allocate"));
}

fn interrupted_and_interleaved() {
    let trace = CreationTrace::new(Origin::OnDisk);
    let mut operation = trace.operation();
    let guard = operation.child(Stage::Physical);
    let mut future = Box::pin(guard.run_async(std::future::pending::<Result<(), String>>()));
    let mut context = Context::from_waker(Waker::noop());
    assert!(matches!(future.as_mut().poll(&mut context), Poll::Pending));
    drop(future);
    operation.finish(Outcome::Interrupted, Some(Reason::FutureDropped));
    let records = take_records();
    assert_eq!(event(&records, "physical", "end")["outcome"], "interrupted");
    assert_eq!(
        event(&records, "physical", "end")["reason"],
        "future_dropped"
    );

    let first = CreationTrace::new(Origin::Cli);
    let second = CreationTrace::new(Origin::Gui);
    let mut a = first.operation();
    let mut b = second.operation();
    let mut repo = a.indexed_child(Stage::Repo, 7);
    let mut replica = b.indexed_child(Stage::Replica, 3);
    repo.finish(Outcome::Ok, None);
    replica.finish(Outcome::Ok, None);
    b.finish(Outcome::Ok, None);
    a.finish(Outcome::Ok, None);
    let records = take_records();
    assert_ne!(first.id, second.id);
    for record in records {
        if record["operation_id"] == first.id.to_string() {
            assert_eq!(record["origin"], "cli");
            assert!(record["agent_index"].is_null());
            if record["stage"] == "repo" {
                assert_eq!(record["repo_index"], 7);
            }
        } else {
            assert_eq!(record["operation_id"], second.id.to_string());
            assert_eq!(record["origin"], "gui");
            assert!(record["repo_index"].is_null());
            if record["stage"] == "replica" {
                assert_eq!(record["agent_index"], 3);
            }
        }
    }
    let result = std::panic::catch_unwind(|| {
        let trace = CreationTrace::new(Origin::OnDisk);
        let _operation = trace.operation();
        panic!("synthetic unwind");
    });
    assert!(result.is_err());
    let records = take_records();
    assert_eq!(event(&records, "operation", "end")["reason"], "unwinding");
}

fn bounded_vocabulary_and_results() {
    let trace = CreationTrace::new(Origin::OnDisk);
    let mut operation = trace.operation();
    operation.project(Path::new(SECRET));
    operation.room(u32::MAX);
    operation.count(usize::MAX);
    operation.clone_errors(usize::MAX);
    operation.refresh_request(Uuid::new_v4());
    for (stage, outcome, reason) in [
        (Stage::CloneProcess, Outcome::Timeout, Reason::CloneTimeout),
        (Stage::ResetProcess, Outcome::Timeout, Reason::ResetTimeout),
        (Stage::ResetProcess, Outcome::Error, Reason::ResetFailed),
    ] {
        let mut repo = operation.indexed_child(Stage::Repo, usize::MAX);
        let mut child = repo.child(stage);
        child.finish(outcome, Some(reason));
        repo.finish(Outcome::Ok, None);
    }
    let sensitive_error = format!("Err {SECRET}");
    let returned = operation
        .child(Stage::Prepare)
        .run(|| Err::<(), _>(sensitive_error.clone()));
    assert_eq!(returned.unwrap_err(), sensitive_error);
    operation.partial();
    operation.result(Ok::<_, String>(())).unwrap();
    let records = take_records();
    assert_eq!(
        event(&records, "clone_process", "end")["reason"],
        "clone_timeout"
    );
    assert!(records.iter().any(|r| r["reason"] == "reset_timeout"));
    assert!(records.iter().any(|r| r["reason"] == "reset_failed"));
    assert_eq!(event(&records, "operation", "end")["outcome"], "partial");
}
