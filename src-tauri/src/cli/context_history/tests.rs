use super::*;

use clap::Parser;
use uuid::Uuid;

use crate::context_history::{
    InitialKind, MatrixType, MetadataSource, RecordKind, TeamAttribution,
};

const NOW: i64 = 1_800_000_000_000;
const FROM: &str = "2026-11-01T00:00:00Z";
const TO: &str = "2026-11-08T00:00:00Z";

#[derive(Parser)]
struct TestCli {
    #[command(flatten)]
    args: ContextHistoryArgs,
}

fn parse(arguments: &[&str]) -> Result<ContextHistoryArgs, clap::Error> {
    let all = std::iter::once("ac").chain(arguments.iter().copied());
    TestCli::try_parse_from(all).map(|cli| cli.args)
}

fn args(project: &str, from: &str, to: &str) -> ContextHistoryArgs {
    parse(&["--project", project, "--from", from, "--to", to]).expect("valid arguments")
}

fn project() -> (tempfile::TempDir, String) {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(tmp.path().join(".ac")).expect("create .ac");
    let text = tmp.path().to_str().expect("utf-8 temp path").to_string();
    (tmp, text)
}

fn run_to_string(args: &ContextHistoryArgs) -> (i32, String) {
    let mut out = Vec::new();
    let code = exit_code(run(args, NOW, &mut out));
    (code, String::from_utf8(out).expect("utf-8 output"))
}

fn record(index: u128, project_path: &str, started: i64) -> HistoryRecord {
    HistoryRecord {
        schema_version: 2,
        record_id: Uuid::from_u128(index),
        app_run_id: Uuid::from_u128(1),
        runtime_session_id: Uuid::from_u128(2),
        logical_session_id: Uuid::from_u128(index),
        sequence: 1,
        ordinal: 1,
        observed_at_utc_ms: started,
        logical_started_at_utc_ms: Some(started),
        partial_start: false,
        partial_end: false,
        initial_record_id: Some(Uuid::from_u128(index)),
        initial_kind: InitialKind::FirstObserved,
        continuity_known: true,
        project_path: project_path.to_string(),
        project_name: "demo".to_string(),
        agent_key: format!("agent-{index:03}"),
        instance_fqn: format!("demo:room-1/agent-{index:03}"),
        room: Some("room-1".to_string()),
        matrix_type: MatrixType::Replica,
        team_attribution: TeamAttribution::Known,
        team_ids: vec!["team-a".to_string()],
        coding_agent_id: None,
        model: None,
        effort: None,
        metadata_source: MetadataSource::Unavailable,
        context_window: None,
        kind: RecordKind::Change,
        percent: Some(10),
        boundary_reason: None,
    }
}

#[test]
fn missing_database_prints_an_empty_schema_2_report() {
    let (_tmp, project) = project();
    let (code, output) = run_to_string(&args(&project, FROM, TO));
    assert_eq!(code, 0);
    let json: serde_json::Value = serde_json::from_str(&output).expect("pure json");
    assert_eq!(json["schemaVersion"], 2);
    assert_eq!(json["totalAgents"], 0);
    assert_eq!(json["agents"], serde_json::json!([]));
    assert_eq!(json["requestedPeriod"]["selection"], "session_started");
    assert_eq!(json["coverage"]["retentionDays"], 30);
}

#[test]
fn invalid_periods_fail_with_no_stdout() {
    let (_tmp, project) = project();
    let cases = [
        ("2026-11-01T00:00:00", TO),
        (FROM, "2026-11-08"),
        (TO, FROM),
        (FROM, FROM),
        (FROM, "2026-12-03T00:00:00Z"),
    ];
    for (from, to) in cases {
        let (code, output) = run_to_string(&args(&project, from, to));
        assert_eq!((code, output.as_str()), (1, ""), "{from} {to}");
    }
}

#[test]
fn project_must_be_absolute_and_an_ac_project() {
    let (code, output) = run_to_string(&args("relative/project", FROM, TO));
    assert_eq!((code, output.as_str()), (1, ""));
    let tmp = tempfile::tempdir().expect("tempdir");
    let bare = tmp.path().to_str().expect("utf-8 temp path");
    let (code, output) = run_to_string(&args(bare, FROM, TO));
    assert_eq!((code, output.as_str()), (1, ""));
}

#[test]
fn exact_and_unknown_flags_are_mutually_exclusive() {
    let base = ["--project", "/p", "--from", FROM, "--to", TO];
    for pair in [
        ["--coding-agent", "claude", "--unknown-coding-agent"],
        ["--model", "opus", "--unknown-model"],
        ["--effort", "high", "--unknown-effort"],
    ] {
        let all: Vec<&str> = base.iter().chain(pair.iter()).copied().collect();
        assert!(parse(&all).is_err(), "{pair:?}");
    }
    let unknown: Vec<&str> = base.iter().copied().chain(["--bogus"]).collect();
    assert!(parse(&unknown).is_err());
    let missing = parse(&["--project", "/p", "--from", FROM]);
    assert!(missing.is_err());
}

#[test]
fn flags_build_tagged_launch_filters() {
    let parsed = parse(&[
        "--project",
        "/p",
        "--from",
        FROM,
        "--to",
        TO,
        "--agent",
        "dev",
        "--team",
        "team-a",
        "--model",
        "opus",
        "--unknown-effort",
    ])
    .expect("valid arguments");
    let mut absolute = parsed.clone();
    absolute.project = std::env::temp_dir();
    let query = query_from_args(&absolute).expect("query");
    assert_eq!(query.agent_key.as_deref(), Some("dev"));
    assert_eq!(query.team_id.as_deref(), Some("team-a"));
    assert_eq!(query.coding_agent_id, LaunchFilter::Any);
    assert_eq!(query.model, LaunchFilter::Exact("opus".to_string()));
    assert_eq!(query.effort, LaunchFilter::Unknown);
    assert_eq!(query.to_utc_ms - query.from_utc_ms, 7 * 24 * 60 * 60 * 1000);
}

#[test]
fn every_agent_page_is_streamed_into_one_document() {
    let project = "C:/projects/demo";
    let query = ReportQuery {
        project_path: project.to_string(),
        from_utc_ms: NOW - 1000,
        to_utc_ms: NOW,
        agent_key: None,
        team_id: None,
        coding_agent_id: LaunchFilter::Any,
        model: LaunchFilter::Any,
        effort: LaunchFilter::Any,
        cursor: None,
    };
    let records: Vec<HistoryRecord> = (1..=230)
        .map(|index| record(index, project, NOW - 10))
        .collect();
    let context = ReportContext {
        retention_days: 30,
        generated_at_utc_ms: NOW,
    };
    let mut out = Vec::new();
    write_report(&records, &query, context, &mut out).expect("written");
    let json: serde_json::Value = serde_json::from_slice(&out).expect("pure json");
    assert_eq!(json["totalAgents"], 230);
    let agents = json["agents"].as_array().expect("agents");
    assert_eq!(agents.len(), 230);
    assert_eq!(agents[229]["agentKey"], "agent-230");
    assert!(json.get("agentsPage").is_none() && json.get("nextAgentCursor").is_none());
}

struct FailingWriter(io::ErrorKind);

impl Write for FailingWriter {
    fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
        Err(io::Error::from(self.0))
    }

    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::from(self.0))
    }
}

#[test]
fn broken_pipe_exits_successfully_and_other_write_errors_fail() {
    let (_tmp, project) = project();
    let arguments = args(&project, FROM, TO);
    let mut pipe = FailingWriter(io::ErrorKind::BrokenPipe);
    assert_eq!(exit_code(run(&arguments, NOW, &mut pipe)), 0);
    let mut denied = FailingWriter(io::ErrorKind::PermissionDenied);
    assert_eq!(exit_code(run(&arguments, NOW, &mut denied)), 1);
}

#[cfg(unix)]
#[test]
fn non_utf8_agent_is_rejected_by_the_parser() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let invalid = OsString::from_vec(vec![0x66, 0xff]);
    let all: Vec<OsString> = [
        "ac",
        "--project",
        "/p",
        "--from",
        FROM,
        "--to",
        TO,
        "--agent",
    ]
    .iter()
    .map(OsString::from)
    .chain([invalid])
    .collect();
    assert!(TestCli::try_parse_from(all).is_err());
}

#[test]
fn a_corrupt_database_fails_with_no_stdout_and_keeps_its_bytes() {
    use crate::context_history::store::{history_database_path, history_directory};
    let (tmp, project) = project();
    let ac_root = tmp.path().join(".ac");
    std::fs::create_dir(history_directory(&ac_root)).expect("mkdir");
    let junk = vec![0x5a; 4096];
    std::fs::write(history_database_path(&ac_root), &junk).expect("write");
    let (code, output) = run_to_string(&args(&project, FROM, TO));
    assert_eq!((code, output.as_str()), (1, ""));
    let after = std::fs::read(history_database_path(&ac_root)).expect("read");
    assert_eq!(after, junk, "bytes must be preserved");
}

#[test]
fn every_store_failure_exits_1_and_is_never_an_empty_success() {
    let failures = [
        StoreError::Corrupt("not_a_sqlite_database".to_string()),
        StoreError::UnsupportedVersion(3),
        StoreError::RetryableBusy,
        StoreError::DeadlineExceeded,
        StoreError::TooManyRows(100_001),
        StoreError::RecoveryRequired {
            action: "retry".to_string(),
        },
    ];
    for failure in failures {
        let shown = format!("{failure:?}");
        assert_eq!(exit_code(Err(CliError::Store(failure))), 1, "{shown}");
    }
}

fn seeded_project(count: u128) -> (tempfile::TempDir, String, PathBuf) {
    use crate::context_history::config::HistoryConfig;
    use crate::context_history::store::HistoryWriter;
    let (tmp, project) = project();
    let ac_root = tmp.path().join(".ac");
    let config = HistoryConfig {
        enabled: true,
        retention_days: 30,
    };
    let mut writer = HistoryWriter::open(&ac_root, &config).expect("open");
    let batch: Vec<_> = (1..=count)
        .map(|i| {
            let mut record = record(i, &project, NOW);
            record.sequence = u64::try_from(i).expect("sequence");
            record
        })
        .collect();
    writer.append(&batch, NOW).expect("append");
    (tmp, project, ac_root)
}

#[test]
fn a_snapshot_over_the_row_cap_fails_instead_of_returning_part() {
    use crate::context_history::store::{read_history_with_limits, StoreLimits};
    let (_tmp, _project, ac_root) = seeded_project(2);
    let limits = StoreLimits {
        max_rows: 1,
        ..StoreLimits::PRODUCTION
    };
    for attempt in [ReadAttempt::ReadOnly, ReadAttempt::AllowRecovery] {
        let result = read_history_with_limits(&ac_root, attempt, limits);
        assert_eq!(result, Err(StoreError::TooManyRows(2)));
    }
}

#[test]
fn a_busy_database_fails_with_no_stdout() {
    use crate::context_history::store::history_database_path;
    let (_tmp, project, ac_root) = seeded_project(1);
    let holder = rusqlite::Connection::open(history_database_path(&ac_root)).expect("holder");
    holder
        .execute_batch("BEGIN EXCLUSIVE")
        .expect("exclusive lock");
    assert_eq!(
        read_history(&ac_root, ReadAttempt::ReadOnly),
        Err(StoreError::RetryableBusy)
    );
    let (code, output) = run_to_string(&args(&project, FROM, TO));
    assert_eq!((code, output.as_str()), (1, ""));
    holder.execute_batch("ROLLBACK").expect("release");
    let (code, _) = run_to_string(&args(&project, FROM, TO));
    assert_eq!(code, 0, "the same read succeeds once the lock is gone");
}

fn agents_of(project: &str) -> serde_json::Value {
    let (code, output) = run_to_string(&args(project, FROM, TO));
    assert_eq!(code, 0, "{project}");
    let json: serde_json::Value = serde_json::from_str(&output).expect("pure json");
    json["agents"].clone()
}

fn started_in_period() -> i64 {
    parse_time("from", FROM).expect("from") + 1000
}

#[test]
fn other_spellings_of_the_project_report_the_same_history() {
    use crate::context_history::store::history_database_path;
    let (tmp, project) = project();
    let ac_root = tmp.path().join(".ac");
    let started = started_in_period();
    let mut writer = crate::context_history::store::HistoryWriter::open(
        &ac_root,
        &crate::context_history::config::HistoryConfig {
            enabled: true,
            retention_days: 30,
        },
    )
    .expect("open");
    let mut foreign = record(2, "C:/elsewhere/other", started);
    foreign.sequence = 2;
    writer
        .append(&[record(1, &project, started), foreign], started)
        .expect("append");
    drop(writer);
    let bytes = std::fs::read(history_database_path(&ac_root)).expect("read");
    let exact = agents_of(&project);
    assert_eq!(exact.as_array().map(Vec::len), Some(1), "only this project");
    assert_eq!(exact[0]["recordsCount"], 1);
    let mut spellings = vec![format!("{project}/.")];
    if cfg!(windows) {
        spellings.push(project.to_uppercase());
        spellings.push(project.replace('\\', "/"));
    }
    for spelling in spellings {
        assert_eq!(agents_of(&spelling), exact, "{spelling}");
    }
    let after = std::fs::read(history_database_path(&ac_root)).expect("read");
    assert_eq!(after, bytes, "the database is never rewritten");
}

#[test]
fn an_unverifiable_project_fails_instead_of_reporting_empty() {
    let (tmp, project) = project();
    let missing = tmp.path().join("missing");
    let missing = missing.to_str().expect("utf-8");
    let (code, output) = run_to_string(&args(missing, FROM, TO));
    assert_eq!((code, output.as_str()), (1, ""));
    let bare = tempfile::tempdir().expect("tempdir");
    let bare = bare.path().to_str().expect("utf-8");
    let (code, output) = run_to_string(&args(bare, FROM, TO));
    assert_eq!((code, output.as_str()), (1, ""));
    assert_eq!(
        agents_of(&project),
        serde_json::json!([]),
        "missing DB stays empty"
    );
}
