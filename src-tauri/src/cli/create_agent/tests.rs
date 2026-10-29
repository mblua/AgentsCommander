use super::*;
use crate::config::settings::{AgentConfig, AppSettings};

fn agent(id: &str, label: &str, command: &str) -> AgentConfig {
    AgentConfig {
        preflight: None,
        preflight_timeout_seconds: None,
        id: id.to_string(),
        id_hash: None,
        id_hash_debug: None,
        label: label.to_string(),
        command: command.to_string(),
        color: "#000000".to_string(),
        order: None,
        envs: Vec::new(),
        isolated_home: false,
        instructions_filename: None,
        config_seed: None,
        context_regex: None,
        blocking_menus: None,
        backend: Default::default(),
    }
}

#[test]
fn find_launch_agent_matches_id_label_substring_and_command_prefix() {
    let settings = AppSettings {
        agents: vec![
            agent("codex", "OpenAI Codex", "codex"),
            agent(
                "claude",
                "Claude Desktop",
                "claude --dangerously-skip-permissions",
            ),
            agent("pwsh", "PowerShell", "powershell.exe -NoLogo"),
        ],
        ..AppSettings::default()
    };

    assert_eq!(
        find_launch_agent(&settings, "CODEX").map(|a| a.id.as_str()),
        Some("codex")
    );
    assert_eq!(
        find_launch_agent(&settings, "Claude Desktop").map(|a| a.id.as_str()),
        Some("claude")
    );
    assert_eq!(
        find_launch_agent(&settings, "desktop").map(|a| a.id.as_str()),
        Some("claude")
    );
    assert_eq!(
        find_launch_agent(&settings, "powershell").map(|a| a.id.as_str()),
        Some("pwsh")
    );
    assert_eq!(
        find_launch_agent(&settings, "  CODEX  ").map(|a| a.id.as_str()),
        Some("codex")
    );
}

#[test]
fn find_launch_agent_rejects_empty_and_whitespace_requests() {
    let settings = AppSettings {
        agents: vec![
            agent("codex", "OpenAI Codex", "codex"),
            agent("claude", "Claude Desktop", "claude"),
        ],
        ..AppSettings::default()
    };

    assert!(find_launch_agent(&settings, "").is_none());
    assert!(find_launch_agent(&settings, "   \t  ").is_none());
}

#[test]
fn build_session_request_preserves_agent_args_from_command() {
    let launch_agent = agent("codex-yolo", "Codex Yolo", "codex --yolo");
    let request = build_session_request(
        "C:/repo/.ac/_agent_architect".to_string(),
        "repo/architect".to_string(),
        &launch_agent,
    )
    .unwrap();

    assert_eq!(request.shell, "codex");
    assert_eq!(request.shell_args, vec!["--yolo"]);
}

#[test]
fn build_session_request_supports_quoted_executable_path() {
    let launch_agent = agent(
        "codex-local",
        "Codex Local",
        "\"C:\\Program Files\\Codex\\codex.exe\" --yolo",
    );
    let request = build_session_request(
        "C:/repo/.ac/_agent_architect".to_string(),
        "repo/architect".to_string(),
        &launch_agent,
    )
    .unwrap();

    assert_eq!(request.shell, "C:\\Program Files\\Codex\\codex.exe");
    assert_eq!(request.shell_args, vec!["--yolo"]);
}

#[test]
fn build_session_request_rejects_invalid_quoted_command() {
    let launch_agent = agent("codex", "Codex", "codex \"unterminated");
    let err = build_session_request(
        "C:/repo/.ac/_agent_architect".to_string(),
        "repo/architect".to_string(),
        &launch_agent,
    )
    .unwrap_err();

    assert!(err.contains("codex"));
    assert!(err.contains("unclosed double quote"));
}

#[test]
fn build_session_request_rejects_blank_and_whitespace_commands() {
    for command in ["", "   \t  "] {
        let launch_agent = agent("codex", "Codex", command);
        let err = build_session_request(
            "C:/repo/.ac/_agent_architect".to_string(),
            "repo/architect".to_string(),
            &launch_agent,
        )
        .expect_err("blank command");

        assert!(err.contains("empty command"));
        assert!(err.contains("codex"));
    }
}

#[test]
fn write_session_request_is_still_json_camel_case() {
    let _guard = SESSION_REQUESTS_TEST_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap();
    let request = SessionRequest {
        id: format!("test-{}", uuid::Uuid::new_v4().simple()),
        cwd: "C:/repo/.ac/_agent_architect".to_string(),
        session_name: "repo/architect".to_string(),
        agent_id: "codex".to_string(),
        shell: "codex".to_string(),
        shell_args: vec!["--ask-for-approval".to_string(), "never".to_string()],
        requested_profile: None,
        timestamp: "2026-05-28T00:00:00Z".to_string(),
    };

    write_session_request(&request).expect("write request");

    let path = crate::config::config_dir()
        .expect("config dir")
        .join(SESSION_REQUESTS_DIR_NAME)
        .join(format!("{}.json", request.id));
    let json = std::fs::read_to_string(&path).expect("read request");
    let _ = std::fs::remove_file(&path);

    assert!(json.contains("\"sessionName\""));
    assert!(json.contains("\"agentId\""));
    assert!(!json.contains("session_name"));
    assert!(!json.contains("agent_id"));
}

// #1163: the result sidecar must stay camelCase JSON so the CLI/app
// contract is stable.
#[test]
fn write_session_request_result_is_still_json_camel_case() {
    let _guard = SESSION_REQUESTS_TEST_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap();
    let result = SessionRequestResult {
        id: format!("test-{}", uuid::Uuid::new_v4().simple()),
        status: SessionRequestResultStatus::Created,
        session_id: Some("sess-1".to_string()),
        error: None,
    };

    write_session_request_result(&result).expect("write result");

    let path = crate::config::config_dir()
        .expect("config dir")
        .join(SESSION_REQUESTS_DIR_NAME)
        .join(format!("{}.result.json", result.id));
    let json = std::fs::read_to_string(&path).expect("read result");
    let _ = std::fs::remove_file(&path);

    assert!(json.contains("\"status\""));
    assert!(json.contains("\"sessionId\""));
    assert!(!json.contains("session_id"));
    assert!(!json.contains("created_at"));
}

#[test]
fn read_session_request_result_consumes_and_deletes() {
    let _guard = SESSION_REQUESTS_TEST_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap();
    let result = SessionRequestResult {
        id: format!("test-{}", uuid::Uuid::new_v4().simple()),
        status: SessionRequestResultStatus::Created,
        session_id: Some("sess-1".to_string()),
        error: None,
    };
    write_session_request_result(&result).expect("write result");
    let path = crate::config::config_dir()
        .expect("config dir")
        .join(SESSION_REQUESTS_DIR_NAME)
        .join(format!("{}.result.json", result.id));

    let read = read_session_request_result(&result.id).expect("read result");
    assert_eq!(read.status, SessionRequestResultStatus::Created);
    assert_eq!(read.session_id.as_deref(), Some("sess-1"));
    assert!(!path.exists(), "consumed result must be deleted");
}

#[test]
fn wait_for_session_request_result_returns_launched() {
    let _guard = SESSION_REQUESTS_TEST_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap();
    let result = SessionRequestResult {
        id: format!("test-{}", uuid::Uuid::new_v4().simple()),
        status: SessionRequestResultStatus::Created,
        session_id: Some("sess-1".to_string()),
        error: None,
    };
    write_session_request_result(&result).expect("write result");

    let outcome = wait_for_session_request_result(&result.id, std::time::Duration::from_secs(1));
    assert_eq!(
        outcome,
        LaunchOutcome::Launched {
            session_id: "sess-1".to_string()
        }
    );
}

#[test]
fn wait_for_session_request_result_returns_rejected() {
    let _guard = SESSION_REQUESTS_TEST_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap();
    let result = SessionRequestResult {
        id: format!("test-{}", uuid::Uuid::new_v4().simple()),
        status: SessionRequestResultStatus::Rejected,
        session_id: None,
        error: Some("sessionRace".to_string()),
    };
    write_session_request_result(&result).expect("write result");

    let outcome = wait_for_session_request_result(&result.id, std::time::Duration::from_secs(1));
    assert_eq!(
        outcome,
        LaunchOutcome::Rejected {
            error: "sessionRace".to_string()
        }
    );
}

#[test]
fn wait_for_session_request_result_times_out_as_pending() {
    let id = format!("test-{}", uuid::Uuid::new_v4().simple());
    let outcome = wait_for_session_request_result(&id, std::time::Duration::from_millis(300));
    assert_eq!(outcome, LaunchOutcome::Pending);
}

#[test]
fn session_result_roundtrip_consumes_json_but_preserves_read_failures() {
    let _guard = SESSION_REQUESTS_TEST_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap();
    let result = SessionRequestResult {
        id: format!("test-{}", uuid::Uuid::new_v4().simple()),
        status: SessionRequestResultStatus::Rejected,
        session_id: None,
        error: Some("request rejected".to_string()),
    };
    let config_dir = crate::config::config_dir().expect("config dir");
    let path = session_request::session_request_result_path(&config_dir, &result.id);
    assert!(session_request::read_session_request_result(&result.id).is_none());
    assert!(!path.exists());

    session_request::write_session_request_result(&result).expect("write result");
    let read = session_request::read_session_request_result(&result.id).expect("read result");
    assert_eq!(read.id, result.id);
    assert_eq!(read.status, result.status);
    assert_eq!(read.session_id, result.session_id);
    assert_eq!(read.error, result.error);
    assert!(!path.exists());

    std::fs::write(&path, b"{corrupt json").unwrap();
    assert!(session_request::read_session_request_result(&result.id).is_none());
    assert!(
        !path.exists(),
        "parsed text is consumed even when JSON is invalid"
    );

    std::fs::write(&path, [0xff]).unwrap();
    assert!(session_request::read_session_request_result(&result.id).is_none());
    assert_eq!(std::fs::read(&path).unwrap(), [0xff]);
    std::fs::remove_file(&path).unwrap();
    assert!(session_request::read_session_request_result(&result.id).is_none());
    assert!(!path.exists());
}

#[test]
fn refresh_stage_finalize_moves_bytes_and_cleans_only_stage_on_failure() {
    use crate::filesystem::documents::{finalize_refresh_request, write_refresh_request_stage};
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path().join("request.json.tmp");
    let dst = dir.path().join("request.json");
    let bytes = b"{\"projectPath\":\"example\"}\n";
    write_refresh_request_stage(&tmp, bytes).unwrap();
    finalize_refresh_request(&tmp, &dst).unwrap();
    assert_eq!(std::fs::read(&dst).unwrap(), bytes);
    assert!(!tmp.exists());

    let incompatible = dir.path().join("directory");
    std::fs::create_dir(&incompatible).unwrap();
    let retained = incompatible.join("retained");
    std::fs::write(&retained, b"retain").unwrap();
    write_refresh_request_stage(&tmp, bytes).unwrap();
    let expected = std::fs::rename(&tmp, &incompatible).unwrap_err();
    let error = finalize_refresh_request(&tmp, &incompatible).unwrap_err();
    assert_eq!(error.kind(), expected.kind());
    assert_eq!(error.raw_os_error(), expected.raw_os_error());
    assert!(!tmp.exists());
    assert!(incompatible.is_dir());
    assert_eq!(std::fs::read(&retained).unwrap(), b"retain");
    assert_eq!(std::fs::read(&dst).unwrap(), bytes);
}
