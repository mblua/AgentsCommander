use super::*;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use futures::future::BoxFuture;
use serde_json::{json, Value};
use tauri::Manager;

use crate::config::settings::{new_settings_state, AppSettings};
use crate::context_history::config::{ConfigStatus, HistoryConfig, LoadedHistoryConfig};
use crate::context_history::observation::TeamMembers;
use crate::context_history::store::HistoryWriter;
use crate::context_history::{InitialKind, MatrixType, MetadataSource, RecordKind};
use crate::history_runtime::SessionLaunchSource;
use crate::history_runtime::{HistoryAppend, ProjectEnvironment, SessionLaunch};

const HOUR_MS: i64 = 60 * 60 * 1000;
const NAMES: [&str; 4] = [
    "get_context_history_report",
    "get_context_history_agent_detail",
    "get_context_history_changes",
    "get_context_history_sidebar_means",
];

// ---- Fixtures ----------------------------------------------------------------------------

/// Sessions the manager shows, by id.
#[derive(Default)]
struct Sessions(Mutex<HashMap<Uuid, SessionLaunch>>);

impl Sessions {
    fn add(&self, working_directory: &str, root: bool, exited: bool) -> Uuid {
        let session_id = Uuid::new_v4();
        let launch = SessionLaunch {
            session_id,
            working_directory: working_directory.to_string(),
            shell: "claude".to_string(),
            effective_shell_args: None,
            is_root_agent: root,
            exited,
        };
        self.0.lock().unwrap().insert(session_id, launch);
        session_id
    }
}

impl SessionLaunchSource for Sessions {
    fn launches<'a>(
        &'a self,
        ids: &'a [Uuid],
    ) -> BoxFuture<'a, Option<Vec<Option<SessionLaunch>>>> {
        Box::pin(async move {
            let present = self.0.lock().unwrap();
            Some(ids.iter().map(|id| present.get(id).cloned()).collect())
        })
    }

    fn live_ids<'a>(&'a self, ids: &'a [Uuid]) -> BoxFuture<'a, Option<HashSet<Uuid>>> {
        Box::pin(async move {
            let present = self.0.lock().unwrap();
            let live = |id: &&Uuid| present.get(id).is_some_and(|launch| !launch.exited);
            Some(ids.iter().filter(live).copied().collect())
        })
    }
}

/// Teams of the project by origin matrix; capture stays disabled.
struct Env {
    teams: Vec<TeamMembers>,
    discoveries: AtomicUsize,
}

impl ProjectEnvironment for Env {
    fn load_config(&self, _ac_root: &Path) -> LoadedHistoryConfig {
        LoadedHistoryConfig {
            config: HistoryConfig::DISABLED,
            status: ConfigStatus::Loaded,
        }
    }
    fn authoritative_ac_root(&self, _ac_root: &Path) -> bool {
        true
    }
    fn teams(&self, _project_path: &Path) -> Vec<TeamMembers> {
        self.discoveries.fetch_add(1, Ordering::SeqCst);
        self.teams.clone()
    }
    fn open_writer(
        &self,
        _ac_root: &Path,
        _config: &HistoryConfig,
    ) -> Result<Box<dyn HistoryAppend>, StoreError> {
        Err(StoreError::Disabled)
    }
}

/// A temporary AC project and the text spelling it is registered with.
struct Project {
    dir: tempfile::TempDir,
    text: String,
}

impl Project {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join(".ac")).expect("create .ac");
        let text = dir.path().to_str().expect("utf-8 temp path").to_string();
        Self { dir, text }
    }

    fn ac(&self, rest: &str) -> String {
        let path = self.dir.path().join(".ac").join(rest);
        path.to_str().expect("utf-8").to_string()
    }

    fn replica(&self, agent: &str) -> String {
        self.ac(&format!("room-1-x/__agent_{agent}"))
    }

    fn team(&self, team_id: &str, agents: &[&str]) -> TeamMembers {
        TeamMembers {
            team_id: team_id.to_string(),
            member_paths: agents
                .iter()
                .map(|agent| self.ac(&format!("_agent_{agent}")))
                .collect(),
        }
    }

    fn seed(&self, records: &[HistoryRecord]) {
        let config = HistoryConfig {
            enabled: true,
            retention_days: 30,
        };
        let mut writer = HistoryWriter::open(&self.dir.path().join(".ac"), &config).expect("open");
        writer.append(records, now_utc_ms()).expect("append");
    }
}

fn record(project: &str, agent: &str, team: &str, index: u128, started: i64) -> HistoryRecord {
    HistoryRecord {
        schema_version: 2,
        record_id: Uuid::from_u128(index),
        app_run_id: Uuid::from_u128(1),
        runtime_session_id: Uuid::from_u128(1000 + index),
        logical_session_id: Uuid::from_u128(index),
        sequence: u64::try_from(index).expect("sequence"),
        ordinal: 1,
        observed_at_utc_ms: started,
        logical_started_at_utc_ms: Some(started),
        partial_start: false,
        partial_end: false,
        initial_record_id: Some(Uuid::from_u128(index)),
        initial_kind: InitialKind::FirstObserved,
        continuity_known: true,
        project_path: project.to_string(),
        project_name: "demo".to_string(),
        agent_key: agent.to_string(),
        instance_fqn: format!("demo:room-1-x/{agent}"),
        room: Some("room-1-x".to_string()),
        matrix_type: MatrixType::Replica,
        team_attribution: TeamAttribution::Known,
        team_ids: vec![team.to_string()],
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

/// A mock app routing through [`dispatch`]; the fallback counts its calls.
struct Harness {
    _app: tauri::App<tauri::test::MockRuntime>,
    webview: tauri::WebviewWindow<tauri::test::MockRuntime>,
    fallbacks: Arc<AtomicUsize>,
    sessions: Arc<Sessions>,
    env: Arc<Env>,
}

fn harness(registered: &[&str], teams: Vec<TeamMembers>, fallback_result: bool) -> Harness {
    let fallbacks = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&fallbacks);
    let fallback = move |_invoke: tauri::ipc::Invoke<tauri::test::MockRuntime>| {
        counted.fetch_add(1, Ordering::SeqCst);
        fallback_result
    };
    let app = tauri::test::mock_builder()
        .invoke_handler(move |invoke| dispatch(invoke, &fallback))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("mock app");
    let settings = AppSettings {
        project_paths: registered.iter().map(|path| path.to_string()).collect(),
        ..AppSettings::default()
    };
    app.manage(new_settings_state(settings));
    let sessions = Arc::new(Sessions::default());
    let env = Arc::new(Env {
        teams,
        discoveries: AtomicUsize::new(0),
    });
    app.manage(Arc::new(HistoryScopeState::new(
        sessions.clone(),
        env.clone(),
    )));
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("webview");
    Harness {
        _app: app,
        webview,
        fallbacks,
        sessions,
        env,
    }
}

impl Harness {
    fn invoke(&self, command: &str, request: Value) -> Result<Value, Value> {
        let response = tauri::test::get_ipc_response(
            &self.webview,
            tauri::webview::InvokeRequest {
                cmd: command.into(),
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: "http://tauri.localhost".parse().unwrap(),
                body: tauri::ipc::InvokeBody::Json(json!({ "request": request })),
                headers: Default::default(),
                invoke_key: tauri::test::INVOKE_KEY.to_string(),
            },
        );
        response.map(|body| body.deserialize::<Value>().expect("json body"))
    }

    fn error_code(&self, command: &str, request: Value) -> String {
        let error = self.invoke(command, request).expect_err("refused");
        error["code"].as_str().expect("coded error").to_string()
    }

    fn fallbacks(&self) -> usize {
        self.fallbacks.load(Ordering::SeqCst)
    }
}

fn any() -> Value {
    json!({ "kind": "any" })
}

fn query(project: &str, agent: Option<&str>) -> Value {
    let now = now_utc_ms();
    json!({
        "projectPath": project,
        "fromUtcMs": now - 7 * 24 * HOUR_MS,
        "toUtcMs": now + HOUR_MS,
        "agentKey": agent,
        "teamId": null,
        "codingAgentId": any(),
        "model": any(),
        "effort": any(),
        "cursor": null,
    })
}

fn sidebar(project: &str, ids: &[String]) -> Value {
    json!({ "projectPath": project, "runtimeSessionIds": ids })
}

// ---- Dispatch ----------------------------------------------------------------------------

#[test]
fn the_four_history_names_answer_without_the_fallback() {
    let project = Project::new();
    let h = harness(&[&project.text], Vec::new(), false);
    for name in NAMES {
        assert_eq!(
            h.error_code(name, json!({ "bogus": 1 })),
            "InvalidQuery",
            "{name}"
        );
    }
    let report = h
        .invoke(NAMES[0], query(&project.text, None))
        .expect("report");
    assert_eq!(report["schemaVersion"], 2);
    assert_eq!(
        h.fallbacks(),
        0,
        "recognised names never reach the fallback"
    );
}

#[test]
fn other_and_look_alike_names_delegate_once_and_keep_the_result() {
    let h = harness(&[], Vec::new(), false);
    let names = [
        "list_sessions",
        "get_context_history",
        "get_context_history_reports",
        "Get_context_history_report",
    ];
    for (calls, name) in names.into_iter().enumerate() {
        assert!(h.invoke(name, json!({})).is_err(), "{name} stays unhandled");
        assert_eq!(h.fallbacks(), calls + 1, "{name} delegated exactly once");
    }
}

#[test]
fn dispatch_returns_the_fallback_bool_for_unknown_commands() {
    for result in [false, true] {
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&calls);
        let fallback = move |invoke: tauri::ipc::Invoke<tauri::test::MockRuntime>| {
            counted.fetch_add(1, Ordering::SeqCst);
            if result {
                invoke.resolver.resolve("fallback");
            }
            result
        };
        let app = tauri::test::mock_builder()
            .invoke_handler(move |invoke| dispatch(invoke, &fallback))
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("mock app");
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .expect("webview");
        let response = tauri::test::get_ipc_response(
            &webview,
            tauri::webview::InvokeRequest {
                cmd: "get_context_history_reports".into(),
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: "http://tauri.localhost".parse().unwrap(),
                body: tauri::ipc::InvokeBody::Json(json!({})),
                headers: Default::default(),
                invoke_key: tauri::test::INVOKE_KEY.to_string(),
            },
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        match result {
            true => assert_eq!(
                response.unwrap().deserialize::<String>().unwrap(),
                "fallback"
            ),
            false => assert!(response.is_err(), "an unhandled command stays unhandled"),
        }
    }
}

#[test]
fn lib_routes_history_through_dispatch_after_one_observation() {
    let lib = include_str!("../../lib.rs");
    let start = lib.find("fn observe_invokes").expect("observer");
    let observer = &lib[start..start + lib[start..].find("\n    }\n").expect("end")];
    let note = observer.find("self.note_invoke(").expect("one note");
    let call = "commands::context_history::dispatch(invoke, &generated)";
    let routed = observer.find(call).expect("dispatch in the fallback");
    assert!(note < routed, "observed before dispatch");
    assert_eq!(observer.matches("note_invoke(").count(), 1);
    assert_eq!(
        lib.matches("commands::context_history::dispatch(").count(),
        1
    );
    assert!(
        observer.contains("list_targets(invoke)"),
        "list_targets route kept"
    );
    let handler = &lib[lib.find("fn invoke_handler(").expect("handler")..];
    let handler = &handler[..handler
        .find(
            "
}
",
        )
        .expect("handler end")];
    assert!(
        handler.contains("generate_handler!["),
        "the big macro is still there"
    );
    assert!(
        !handler.contains("context_history"),
        "no history entry in invoke_handler"
    );
}

// ---- Report family -----------------------------------------------------------------------

#[test]
fn report_detail_and_changes_share_one_snapshot_contract() {
    let project = Project::new();
    let started = now_utc_ms() - HOUR_MS;
    project.seed(&[
        record(&project.text, "dev", "team-a", 1, started),
        record(&project.text, "dev", "team-a", 2, started + 1),
        record(&project.text, "qa", "team-a", 3, started + 2),
    ]);
    let h = harness(&[&project.text], Vec::new(), false);
    let report = h
        .invoke(NAMES[0], query(&project.text, None))
        .expect("report");
    assert_eq!(report["totalAgents"], 2);
    assert_eq!(report["agentsPage"][0]["agentKey"], "dev");
    assert_eq!(report["agentsPage"][0]["recordsCount"], 2);
    assert_eq!(report["agentsPage"][0]["positiveSum"], 20);
    assert_eq!(report["nextAgentCursor"], Value::Null);

    let detail = h
        .invoke(NAMES[1], query(&project.text, Some("dev")))
        .expect("detail");
    assert_eq!(detail["agentKey"], "dev");
    assert_eq!(detail["sessionsPage"].as_array().unwrap().len(), 2);
    let logical = detail["sessionsPage"][0]["logicalSessionId"].clone();
    let runtime = detail["sessionsPage"][0]["runtimeSessionId"].clone();

    let changes =
        json!({ "query": query(&project.text, Some("dev")), "logicalSessionId": logical });
    let page = h.invoke(NAMES[2], changes).expect("changes");
    assert_eq!(page["recordsPage"].as_array().unwrap().len(), 1);
    let by_runtime =
        json!({ "query": query(&project.text, Some("dev")), "logicalSessionId": runtime });
    assert_eq!(
        h.error_code(NAMES[2], by_runtime),
        "InvalidQuery",
        "runtime id is not logical"
    );
    assert_eq!(h.fallbacks(), 0);
}

#[test]
fn ipc_and_cli_build_the_same_report_from_one_fixture() {
    let project = Project::new();
    let started = now_utc_ms() - HOUR_MS;
    project.seed(&[record(&project.text, "dev", "team-a", 1, started)]);
    let h = harness(&[&project.text], Vec::new(), false);
    let request = query(&project.text, None);
    let ipc = h.invoke(NAMES[0], request.clone()).expect("report");
    let (canonical, retention, records) = read_project(&project.text).expect("read");
    let mut core_query: ReportQuery = serde_json::from_value(request).expect("query");
    core_query.project_path = canonical;
    let generated = ipc["generatedAtUtcMs"].as_i64().expect("now");
    let core = build_report(&records, &core_query, context(retention, generated)).expect("core");
    assert_eq!(ipc, serde_json::to_value(core).expect("json"));
}

#[test]
fn missing_history_is_empty_and_creates_nothing() {
    let project = Project::new();
    let h = harness(&[&project.text], Vec::new(), false);
    let report = h
        .invoke(NAMES[0], query(&project.text, None))
        .expect("report");
    assert_eq!(report["totalAgents"], 0);
    assert_eq!(report["agentsPage"], json!([]));
    assert!(!project.dir.path().join(".ac/context-history").exists());
}

#[test]
fn unregistered_relative_or_unverified_projects_are_refused() {
    let project = Project::new();
    let other = Project::new();
    let h = harness(&[&project.text], Vec::new(), false);
    assert_eq!(
        h.error_code(NAMES[0], query(&other.text, None)),
        "InvalidProject"
    );
    assert_eq!(
        h.error_code(NAMES[0], query("relative/demo", None)),
        "InvalidProject"
    );
    let gone = Project::new();
    let gone_text = gone.text.clone();
    drop(gone);
    let h = harness(&[&gone_text], Vec::new(), false);
    let error = h
        .invoke(NAMES[0], query(&gone_text, None))
        .expect_err("refused");
    assert_eq!(error["code"], "InvalidProject");
    assert!(
        !error["message"].as_str().unwrap().contains(&gone_text),
        "no private path"
    );
}

#[test]
fn invalid_queries_and_cursors_are_refused_before_disk() {
    let project = Project::new();
    let h = harness(&[&project.text], Vec::new(), false);
    let mut inverted = query(&project.text, None);
    inverted["toUtcMs"] = inverted["fromUtcMs"].clone();
    assert_eq!(h.error_code(NAMES[0], inverted), "InvalidQuery");
    let mut cursor = query(&project.text, None);
    cursor["cursor"] = json!("not-a-cursor");
    assert_eq!(h.error_code(NAMES[0], cursor), "InvalidCursor");
    assert_eq!(
        h.error_code(NAMES[1], query(&project.text, None)),
        "InvalidQuery"
    );
    let malformed = json!({ "query": query(&project.text, Some("dev")), "logicalSessionId": "x" });
    assert_eq!(h.error_code(NAMES[2], malformed), "InvalidQuery");
    let mut unknown_kind = query(&project.text, None);
    unknown_kind["model"] = json!({ "kind": "maybe" });
    assert_eq!(h.error_code(NAMES[0], unknown_kind), "InvalidQuery");
}

#[test]
fn store_failures_map_to_stable_codes_without_paths() {
    let cases = [
        (
            StoreError::UnsupportedVersion(3),
            HistoryErrorCode::UnsupportedVersion,
        ),
        (
            StoreError::StorageLimitMismatch("p".into()),
            HistoryErrorCode::StorageLimitMismatch,
        ),
        (StoreError::RetryableBusy, HistoryErrorCode::Busy),
        (StoreError::DeadlineExceeded, HistoryErrorCode::Timeout),
        (
            StoreError::Corrupt("C:/secret".into()),
            HistoryErrorCode::Corrupt,
        ),
        (
            StoreError::Io("C:/secret".into()),
            HistoryErrorCode::ReadFailed,
        ),
        (StoreError::TooManyRows(9), HistoryErrorCode::ReadFailed),
        (
            StoreError::RecoveryRequired {
                action: "C:/secret".into(),
            },
            HistoryErrorCode::RecoveryRequired,
        ),
    ];
    for (failure, code) in cases {
        let error = store_error(failure);
        assert_eq!(error.code, code);
        assert!(!error.message.contains("secret"), "{}", error.message);
    }
}

#[test]
fn a_corrupt_database_is_an_error_not_an_empty_report() {
    let project = Project::new();
    let directory = project.dir.path().join(".ac/context-history");
    std::fs::create_dir_all(&directory).expect("dir");
    std::fs::write(directory.join("history.sqlite3"), b"not a database at all").expect("write");
    let h = harness(&[&project.text], Vec::new(), false);
    let error = h
        .invoke(NAMES[0], query(&project.text, None))
        .expect_err("refused");
    assert_ne!(error["code"], "InvalidQuery");
    assert_ne!(error["code"], "InvalidProject");
}

// ---- Sidebar means -----------------------------------------------------------------------

#[test]
fn sidebar_known_scope_has_windows_and_others_stay_null() {
    let project = Project::new();
    let started = now_utc_ms() - HOUR_MS;
    project.seed(&[record(&project.text, "dev", "team-a", 1, started)]);
    let teams = vec![
        project.team("team-a", &["dev", "dual"]),
        project.team("team-b", &["dual"]),
    ];
    let h = harness(&[&project.text], teams, false);
    let known = h.sessions.add(&project.replica("dev"), false, false);
    let ambiguous = h.sessions.add(&project.replica("dual"), false, false);
    let lonely = h.sessions.add(&project.replica("solo"), false, false);
    let root = h.sessions.add(&project.replica("root"), true, false);
    let absent = Uuid::new_v4();
    let ids: Vec<String> = [known, ambiguous, lonely, root, absent]
        .iter()
        .map(Uuid::to_string)
        .collect();
    let response = h
        .invoke(NAMES[3], sidebar(&project.text, &ids))
        .expect("means");
    let entries = response["entries"].as_array().expect("entries");
    assert_eq!(entries.len(), 5);
    assert_eq!(
        entries[0]["scope"],
        json!({ "kind": "known", "agentKey": "dev", "teamIds": ["team-a"] })
    );
    let day = &entries[0]["windows"]["day"];
    assert_eq!(
        (day["days"].clone(), day["recordsCount"].clone()),
        (json!(1), json!(1))
    );
    assert_eq!(entries[0]["windows"]["month"]["days"], 30);
    let generated = response["generatedAtUtcMs"].clone();
    assert_eq!(
        entries[0]["windows"]["week"]["toUtcMs"], generated,
        "one now for every window"
    );
    assert_eq!(entries[1]["scope"]["kind"], "ambiguous");
    assert_eq!(entries[1]["scope"]["teamIds"], json!(["team-a", "team-b"]));
    assert_eq!(entries[2]["scope"]["kind"], "unknown");
    for entry in &entries[1..] {
        assert_eq!(entry["windows"], Value::Null, "never a known-looking zero");
    }
    for entry in &entries[3..] {
        assert_eq!(
            entry["scope"],
            json!({ "kind": "unknown", "agentKey": null, "teamIds": [] })
        );
    }
    assert_eq!(entries[4]["runtimeSessionId"], json!(absent.to_string()));
}

#[test]
fn sidebar_refuses_bad_ids_and_foreign_sessions_before_discovery() {
    let project = Project::new();
    let foreign = Project::new();
    let h = harness(&[&project.text], Vec::new(), false);
    let id = Uuid::new_v4().to_string();
    let too_many: Vec<String> = (0..101).map(|_| Uuid::new_v4().to_string()).collect();
    let bad = [
        vec!["not-a-uuid".to_string()],
        Vec::new(),
        vec![id.clone(), id.clone()],
        too_many,
    ];
    for ids in bad {
        assert_eq!(
            h.error_code(NAMES[3], sidebar(&project.text, &ids)),
            "InvalidQuery"
        );
    }
    let mine = h.sessions.add(&project.replica("dev"), false, false);
    let theirs = h.sessions.add(&foreign.replica("dev"), false, false);
    let exited = h.sessions.add(&foreign.replica("qa"), false, true);
    let other_root = h.sessions.add(&foreign.replica("root"), true, false);
    for stranger in [theirs, exited, other_root] {
        let ids = [mine.to_string(), stranger.to_string()];
        assert_eq!(
            h.error_code(NAMES[3], sidebar(&project.text, &ids)),
            "InvalidProject"
        );
    }
    assert_eq!(
        h.env.discoveries.load(Ordering::SeqCst),
        0,
        "refused before teams"
    );
    let ids = [mine.to_string()];
    assert_eq!(
        h.error_code(NAMES[3], sidebar(&foreign.text, &ids)),
        "InvalidProject"
    );
}

#[test]
fn sidebar_exited_local_session_is_unknown() {
    let project = Project::new();
    let h = harness(
        &[&project.text],
        vec![project.team("team-a", &["dev"])],
        false,
    );
    let exited = h.sessions.add(&project.replica("dev"), false, true);
    let response = h
        .invoke(NAMES[3], sidebar(&project.text, &[exited.to_string()]))
        .expect("means");
    assert_eq!(response["entries"][0]["scope"]["kind"], "unknown");
    assert_eq!(response["entries"][0]["windows"], Value::Null);
}
