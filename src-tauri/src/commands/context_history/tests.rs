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
    fn is_root_directory(&self, _working_directory: &str) -> bool {
        false
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
        self.seed_at(records, now_utc_ms());
    }

    /// Seed with an explicit append clock, so retention keeps old fixtures.
    fn seed_at(&self, records: &[HistoryRecord], now: i64) {
        let config = HistoryConfig {
            enabled: true,
            retention_days: 30,
        };
        let mut writer = HistoryWriter::open(&self.dir.path().join(".ac"), &config).expect("open");
        writer.append(records, now).expect("append");
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
    let settings = AppSettings {
        project_paths: registered.iter().map(ToString::to_string).collect(),
        ..AppSettings::default()
    };
    harness_with(settings, teams, fallback_result)
}

/// #4049: a harness over an explicit Settings snapshot.
fn harness_with(settings: AppSettings, teams: Vec<TeamMembers>, fallback_result: bool) -> Harness {
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
                url: crate::test_support::LOCAL_IPC_ORIGIN.parse().unwrap(),
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
    assert_eq!(report["schemaVersion"], 3);
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
                url: crate::test_support::LOCAL_IPC_ORIGIN.parse().unwrap(),
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

/// The common wire fixture of `changes` (plan v18, AC-3532-017); TS uses the same Q and L.
const WIRE_L: &str = "11111111-1111-4111-8111-111111111111";

fn wire_q(project: &str) -> Value {
    let q = r#"{"projectPath":"__PROJECT__","fromUtcMs":0,"toUtcMs":86400000,"agentKey":"dev",
        "teamId":null,"codingAgentId":{"kind":"any"},"model":{"kind":"any"},
        "effort":{"kind":"any"},"cursor":null}"#;
    let mut q: Value = serde_json::from_str(q).expect("Q");
    q["projectPath"] = json!(project);
    q
}

fn flat(q: &Value, logical: &str) -> Value {
    let mut flat = q.clone();
    flat["logicalSessionId"] = json!(logical);
    flat
}

#[test]
fn changes_wire_fixture_is_nested_and_the_flat_shape_is_refused() {
    let project = Project::new();
    let logical = Uuid::parse_str(WIRE_L).expect("L");
    let mut session = record(&project.text, "dev", "team-a", 7, HOUR_MS);
    session.logical_session_id = logical;
    session.initial_record_id = Some(session.record_id);
    session.runtime_session_id = Uuid::from_u128(0xabc);
    session.sequence = 1;
    project.seed_at(&[session], 2 * HOUR_MS);
    let h = harness(&[&project.text], Vec::new(), false);
    let q = wire_q(&project.text);

    let nested = json!({ "query": q, "logicalSessionId": WIRE_L });
    let page = h.invoke(NAMES[2], nested).expect("nested changes");
    assert_eq!(page["logicalSessionId"], json!(WIRE_L));
    assert_eq!(page["agentKey"], "dev");
    let records = page["recordsPage"].as_array().expect("records");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["logicalSessionId"], json!(WIRE_L));

    assert_eq!(h.error_code(NAMES[2], flat(&q, WIRE_L)), "InvalidQuery");
    let stranger = Project::new();
    let unregistered = flat(&wire_q(&stranger.text), WIRE_L);
    assert_eq!(
        h.error_code(NAMES[2], unregistered),
        "InvalidQuery",
        "shape before project"
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
    let (canonical, records) = read_project(&project.text).expect("read");
    let retention = 30;
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
    for entry in entries {
        let keys: Vec<&str> = entry
            .as_object()
            .expect("entry object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            ["runtimeSessionId", "scope", "sessionWindows"],
            "count-only wire keys, no calendar buckets"
        );
    }
    let buckets = entries[0]["sessionWindows"]
        .as_array()
        .expect("known buckets");
    let requested: Vec<Value> = buckets
        .iter()
        .map(|b| b["requestedSessions"].clone())
        .collect();
    assert_eq!(
        requested,
        [json!(25), json!(100), json!(300)],
        "configured order"
    );
    assert_eq!(buckets[0]["sessionsCount"], 1);
    assert_eq!(buckets[0]["recordsCount"], 1);
    assert_eq!(buckets[0]["recordsPerSession"], 1.0);
    assert_eq!(entries[1]["scope"]["kind"], "ambiguous");
    assert_eq!(entries[1]["scope"]["teamIds"], json!(["team-a", "team-b"]));
    assert_eq!(entries[2]["scope"]["kind"], "unknown");
    for entry in &entries[1..] {
        assert_eq!(
            entry["sessionWindows"],
            Value::Null,
            "never a known-looking zero"
        );
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
    assert_eq!(response["entries"][0]["sessionWindows"], Value::Null);
    assert!(response["entries"][0].get("windows").is_none());
}

// ---- Settings retention (#4049) ----------------------------------------------------------

/// Settings registering `project` with history off and the given retention.
fn disabled_settings(project: &str, retention: u32) -> AppSettings {
    let base = AppSettings {
        project_paths: vec![project.to_string()],
        ..AppSettings::default()
    };
    let mut raw = serde_json::to_value(&base).expect("settings json");
    raw["contextHistoryEnabled"] = json!(false);
    raw["contextHistoryRetentionDays"] = json!(retention);
    let settings: AppSettings = serde_json::from_value(raw).expect("settings");
    assert!(!settings.context_history_enabled);
    assert_eq!(settings.context_history_retention_days.days(), retention);
    settings
}

#[test]
fn settings_retention_overrides_any_heatmap_file_for_every_report() {
    let legacy = [
        None,
        Some(r#"{"version":1,"enabled":true,"retentionDays":5}"#),
        Some(r#"{"version":1,"enabled":false,"retentionDays":365}"#),
        Some("{ not json"),
    ];
    for heatmap in legacy {
        let project = Project::new();
        let started = now_utc_ms() - HOUR_MS;
        project.seed(&[record(&project.text, "dev", "team-a", 1, started)]);
        let heatmap_path = project.dir.path().join(".ac/heatmap.json");
        if let Some(text) = heatmap {
            std::fs::write(&heatmap_path, text).expect("write heatmap");
        }
        let h = harness_with(disabled_settings(&project.text, 120), Vec::new(), false);
        let report = h
            .invoke(NAMES[0], query(&project.text, None))
            .expect("disabled history still reads");
        assert_eq!(report["coverage"]["retentionDays"], 120, "{heatmap:?}");
        assert_eq!(report["totalAgents"], 1, "{heatmap:?}");
        let detail = h
            .invoke(NAMES[1], query(&project.text, Some("dev")))
            .expect("detail");
        assert_eq!(detail["coverage"]["retentionDays"], 120, "{heatmap:?}");
        let logical = detail["sessionsPage"][0]["logicalSessionId"].clone();
        let changes =
            json!({ "query": query(&project.text, Some("dev")), "logicalSessionId": logical });
        let page = h.invoke(NAMES[2], changes).expect("changes");
        assert_eq!(page["coverage"]["retentionDays"], 120, "{heatmap:?}");
        let stranger = Project::new();
        assert_eq!(
            h.error_code(NAMES[0], query(&stranger.text, None)),
            "InvalidProject",
            "registration still precedes the store"
        );
        let after = std::fs::read_to_string(&heatmap_path).ok();
        assert_eq!(
            after.as_deref(),
            heatmap,
            "the legacy file is never written"
        );
        assert_eq!(h.fallbacks(), 0);
    }
}

// ---- Session windows (#4091) -------------------------------------------------------------

/// A count-mode query: the newest `count` sessions before a fixed cutoff.
fn recent(project: &str, agent: Option<&str>, count: u32) -> Value {
    let mut request = query(project, agent);
    request["fromUtcMs"] = json!(0);
    request["sessionCount"] = json!(count);
    request
}

/// Settings registering `project` with the given session windows.
fn windowed_settings(project: &str, windows: &[u32]) -> AppSettings {
    let base = AppSettings {
        project_paths: vec![project.to_string()],
        ..AppSettings::default()
    };
    let mut raw = serde_json::to_value(&base).expect("settings json");
    raw["contextHistorySessionWindows"] = json!(windows);
    let settings: AppSettings = serde_json::from_value(raw).expect("settings");
    assert_eq!(settings.context_history_session_windows.counts(), windows);
    settings
}

#[test]
fn session_count_must_be_configured_and_is_checked_before_disk() {
    let project = Project::new();
    let directory = project.dir.path().join(".ac/context-history");
    std::fs::create_dir_all(&directory).expect("dir");
    std::fs::write(directory.join("history.sqlite3"), b"not a database at all").expect("write");
    let h = harness_with(windowed_settings(&project.text, &[3, 7]), Vec::new(), false);
    for count in [0, 25, 10_001] {
        for name in [NAMES[0], NAMES[1]] {
            let request = recent(&project.text, Some("dev"), count);
            assert_eq!(
                h.error_code(name, request),
                "InvalidQuery",
                "{name} {count}"
            );
        }
        let changes = json!({
            "query": recent(&project.text, Some("dev"), count),
            "logicalSessionId": WIRE_L,
        });
        assert_eq!(h.error_code(NAMES[2], changes), "InvalidQuery", "{count}");
    }
    let configured = h.error_code(NAMES[0], recent(&project.text, None, 7));
    assert_ne!(
        configured, "InvalidQuery",
        "a configured count reaches the store"
    );
    let mut dated = recent(&project.text, None, 7);
    dated["fromUtcMs"] = json!(1);
    assert_eq!(h.error_code(NAMES[0], dated), "InvalidQuery");
}

#[test]
fn count_report_over_ipc_matches_the_core_and_carries_scope_windows() {
    let project = Project::new();
    let started = now_utc_ms() - HOUR_MS;
    project.seed(&[
        record(&project.text, "dev", "team-a", 1, started),
        record(&project.text, "dev", "team-a", 2, started + 1),
        record(&project.text, "dev", "team-b", 3, started + 2),
    ]);
    let h = harness(&[&project.text], Vec::new(), false);
    let request = recent(&project.text, None, 25);
    let ipc = h.invoke(NAMES[0], request.clone()).expect("report");
    assert_eq!(ipc["requestedPeriod"]["selection"], "recent_sessions");
    assert_eq!(ipc["requestedPeriod"]["sessionCount"], 25);
    let agent = &ipc["agentsPage"][0];
    assert_eq!(
        (
            agent["sessionsCount"].clone(),
            agent["recordsPerSession"].clone()
        ),
        (json!(3), json!(1.0))
    );
    let windows = agent["scopeWindows"].as_array().expect("scope windows");
    let available: Vec<Value> = windows
        .iter()
        .map(|w| w["availableSessions"].clone())
        .collect();
    assert_eq!(available, [json!(2), json!(1)]);
    let (canonical, records) = read_project(&project.text).expect("read");
    let mut core_query: ReportQuery = serde_json::from_value(request).expect("query");
    core_query.project_path = canonical;
    let generated = ipc["generatedAtUtcMs"].as_i64().expect("now");
    let core = build_report(&records, &core_query, context(30, generated)).expect("core");
    assert_eq!(ipc, serde_json::to_value(core).expect("json"));
    let detail = h
        .invoke(NAMES[1], recent(&project.text, Some("dev"), 25))
        .expect("detail");
    assert_eq!(detail["recordsPerSession"], 1.0);
    assert_eq!(detail["scopeWindows"].as_array().map(Vec::len), Some(2));
    let date = h
        .invoke(NAMES[0], query(&project.text, None))
        .expect("date");
    assert_eq!(date["requestedPeriod"]["sessionCount"], Value::Null);
    assert_eq!(date["agentsPage"][0]["scopeWindows"], Value::Null);
}

#[test]
fn sidebar_session_windows_follow_the_settings_snapshot_order() {
    let project = Project::new();
    let started = now_utc_ms() - HOUR_MS;
    project.seed(&[
        record(&project.text, "dev", "team-a", 1, started),
        record(&project.text, "dev", "team-a", 2, started + 1),
    ]);
    let settings = windowed_settings(&project.text, &[1, 9]);
    let h = harness_with(settings, vec![project.team("team-a", &["dev"])], false);
    let known = h.sessions.add(&project.replica("dev"), false, false);
    let response = h
        .invoke(NAMES[3], sidebar(&project.text, &[known.to_string()]))
        .expect("means");
    let buckets = response["entries"][0]["sessionWindows"]
        .as_array()
        .expect("buckets");
    let shape: Vec<(Value, Value)> = buckets
        .iter()
        .map(|b| (b["requestedSessions"].clone(), b["sessionsCount"].clone()))
        .collect();
    assert_eq!(shape, [(json!(1), json!(1)), (json!(9), json!(2))]);
    assert_eq!(buckets[1]["recordsPerSession"], 1.0);
}
