use std::path::Path;
use std::process::Stdio;

use super::{
    assert_same_path, assert_single_project_refresh_request, command_for_binary,
    config_dir_for_bin, copy_binary_into, project_with_workspace, session_request_paths,
    spawn_lock, write_settings, Tmp,
};

/// #1163: watch `config_dir/session-requests/` for the CLI's request file and
/// answer it with a `<id>.result.json` sidecar exactly like the running app
/// would. Runs in a background thread; returns once the result is written.
/// Result sidecars (`*.result.json`) are never treated as requests.
fn watch_and_answer_session_request(
    config_dir: &Path,
    status: &str,
    session_id: Option<&str>,
    error: Option<&str>,
) {
    let requests_dir = config_dir.join("session-requests");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if std::time::Instant::now() > deadline {
            panic!("watcher timed out waiting for a session request");
        }
        let request_path = std::fs::read_dir(&requests_dir).ok().and_then(|rd| {
            rd.filter_map(|e| e.ok()).map(|e| e.path()).find(|p| {
                p.extension().and_then(|s| s.to_str()) == Some("json")
                    && !p
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .is_some_and(|s| s.ends_with(".result"))
            })
        });
        let Some(request_path) = request_path else {
            std::thread::sleep(std::time::Duration::from_millis(50));
            continue;
        };
        let id = request_path
            .file_stem()
            .and_then(|s| s.to_str())
            .expect("request id")
            .to_string();
        let result = serde_json::json!({
            "id": id,
            "status": status,
            "sessionId": session_id,
            "error": error,
        });
        std::fs::write(
            requests_dir.join(format!("{id}.result.json")),
            serde_json::to_string_pretty(&result).expect("result json"),
        )
        .expect("write result sidecar");
        return;
    }
}

#[test]
fn create_agent_matrix_launch_reports_launched_when_app_confirms() {
    let tmp = Tmp::new("cli-create-agent-matrix-launch");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let project = project_with_workspace(tmp.path());
    write_settings(
        &config_dir,
        serde_json::json!({
            "defaultShell": "powershell.exe",
            "defaultShellArgs": [],
            "agents": [{
                "id": "codex",
                "label": "Codex",
                "command": "codex --ask-for-approval never",
                "color": "#000000"
            }],
            "projectPaths": [tmp.path().to_string_lossy().to_string()]
        }),
    );

    // #1163: a watcher stands in for the running app and confirms the launch.
    let watcher_config_dir = config_dir.clone();
    let watcher = std::thread::spawn(move || {
        watch_and_answer_session_request(&watcher_config_dir, "created", Some("sess-1"), None);
    });

    let mut command = command_for_binary(&bin);
    command
        .args([
            "create-agent-matrix",
            "--project",
            "ProjectAlpha",
            "--name",
            "Architect",
            "--description",
            "Build plans",
            "--launch",
            "codex",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = {
        let _guard = spawn_lock();
        command.spawn().expect("spawn binary")
    };
    let out = child.wait_with_output().expect("collect output");
    watcher.join().expect("watcher answered");

    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout should be JSON");
    assert_eq!(json["launched"], true);
    assert_eq!(json["launchStatus"], "launched");
    assert_eq!(json["launchAgent"], "codex");
    assert!(json["launchError"].is_null());

    // The request is still on disk: the CLI consumes only the result sidecar
    // (which it deletes), never the request.
    let requests = session_request_paths(&config_dir);
    assert_eq!(requests.len(), 1, "expected exactly one session request");
    assert!(
        !config_dir
            .join("session-requests")
            .join("sess-1.result.json")
            .exists()
            && !requests.iter().any(|p| {
                p.file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.ends_with(".result.json"))
            }),
        "the CLI must consume (delete) the result sidecar"
    );

    let request: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&requests[0]).expect("read request"))
            .expect("request json");
    let agent_dir = project.join(".ac").join("_agent_architect");
    assert_same_path(request["cwd"].as_str().expect("cwd"), &agent_dir);
    assert_eq!(request["sessionName"], "ProjectAlpha/architect");
    assert_eq!(request["agentId"], "codex");

    assert_single_project_refresh_request(&config_dir, &project);
}

#[test]
fn create_agent_matrix_launch_rejection_is_reported() {
    let tmp = Tmp::new("cli-create-agent-matrix-launch-rejected");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let project = project_with_workspace(tmp.path());
    write_settings(
        &config_dir,
        serde_json::json!({
            "defaultShell": "powershell.exe",
            "defaultShellArgs": [],
            "agents": [{
                "id": "codex",
                "label": "Codex",
                "command": "codex --ask-for-approval never",
                "color": "#000000"
            }],
            "projectPaths": [tmp.path().to_string_lossy().to_string()]
        }),
    );

    // #1163: the app answers with a rejection (e.g. the sessionRace gate).
    let watcher_config_dir = config_dir.clone();
    let watcher = std::thread::spawn(move || {
        watch_and_answer_session_request(
            &watcher_config_dir,
            "rejected",
            None,
            Some("sessionRace"),
        );
    });

    let mut command = command_for_binary(&bin);
    command
        .args([
            "create-agent-matrix",
            "--project",
            "ProjectAlpha",
            "--name",
            "Architect",
            "--description",
            "Build plans",
            "--launch",
            "codex",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = {
        let _guard = spawn_lock();
        command.spawn().expect("spawn binary")
    };
    let out = child.wait_with_output().expect("collect output");
    watcher.join().expect("watcher answered");

    // Matrix creation stays independent of the launch result: exit 0 and all
    // matrix fields present.
    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout should be JSON");
    let agent_dir = project.join(".ac").join("_agent_architect");
    assert_same_path(json["agentPath"].as_str().expect("agentPath"), &agent_dir);
    assert_eq!(json["agentName"], "ProjectAlpha/architect");
    assert_same_path(
        json["rolePath"].as_str().expect("rolePath"),
        &agent_dir.join("Role.md"),
    );
    assert_eq!(json["launched"], false);
    assert_eq!(json["launchStatus"], "rejected");
    assert_eq!(json["launchAgent"], "codex");
    let launch_error = json["launchError"].as_str().expect("launchError");
    assert!(launch_error.contains("sessionRace"), "{launch_error}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("session launch rejected"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn create_agent_matrix_whitespace_launch_command_warns_without_request() {
    let tmp = Tmp::new("cli-create-agent-matrix-blank-launch");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let _project = project_with_workspace(tmp.path());
    write_settings(
        &config_dir,
        serde_json::json!({
            "defaultShell": "powershell.exe",
            "defaultShellArgs": [],
            "agents": [{
                "id": "codex",
                "label": "Codex",
                "command": "   \t  ",
                "color": "#000000"
            }],
            "projectPaths": [tmp.path().to_string_lossy().to_string()]
        }),
    );

    let mut command = command_for_binary(&bin);
    command
        .args([
            "create-agent-matrix",
            "--project",
            "ProjectAlpha",
            "--name",
            "Architect",
            "--description",
            "Build plans",
            "--launch",
            "codex",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = {
        let _guard = spawn_lock();
        command.spawn().expect("spawn binary")
    };
    let out = child.wait_with_output().expect("collect output");

    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout should be JSON");
    assert_eq!(json["launched"], false);
    assert!(json["launchAgent"].is_null());
    assert!(String::from_utf8_lossy(&out.stderr).contains("empty command"));
    assert!(
        session_request_paths(&config_dir).is_empty(),
        "blank command must not write a launch request"
    );
}

#[test]
fn create_agent_matrix_empty_launch_request_warns_without_request() {
    let tmp = Tmp::new("cli-create-agent-matrix-empty-launch-request");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let _project = project_with_workspace(tmp.path());
    write_settings(
        &config_dir,
        serde_json::json!({
            "defaultShell": "powershell.exe",
            "defaultShellArgs": [],
            "agents": [{
                "id": "codex",
                "label": "Codex",
                "command": "codex --ask-for-approval never",
                "color": "#000000"
            }],
            "projectPaths": [tmp.path().to_string_lossy().to_string()]
        }),
    );

    let mut command = command_for_binary(&bin);
    command
        .args([
            "create-agent-matrix",
            "--project",
            "ProjectAlpha",
            "--name",
            "Architect",
            "--description",
            "Build plans",
            "--launch",
            "",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = {
        let _guard = spawn_lock();
        command.spawn().expect("spawn binary")
    };
    let out = child.wait_with_output().expect("collect output");

    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout should be JSON");
    assert_eq!(json["launched"], false);
    assert!(json["launchAgent"].is_null());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not found in settings"));
    assert!(
        session_request_paths(&config_dir).is_empty(),
        "empty launch request must not write a launch request"
    );
}

#[test]
fn create_agent_project_mode_blank_launch_command_warns_without_request() {
    let tmp = Tmp::new("cli-create-agent-project-blank-launch");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let project = project_with_workspace(tmp.path());
    write_settings(
        &config_dir,
        serde_json::json!({
            "defaultShell": "powershell.exe",
            "defaultShellArgs": [],
            "agents": [{
                "id": "codex",
                "label": "Codex",
                "command": "",
                "color": "#000000"
            }],
            "projectPaths": [tmp.path().to_string_lossy().to_string()]
        }),
    );

    let mut command = command_for_binary(&bin);
    command
        .args([
            "create-agent",
            "--project",
            "ProjectAlpha",
            "--name",
            "Architect",
            "--description",
            "Build plans",
            "--launch",
            "codex",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = {
        let _guard = spawn_lock();
        command.spawn().expect("spawn binary")
    };
    let out = child.wait_with_output().expect("collect output");

    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout should be JSON");
    assert_eq!(json["launched"], false);
    assert!(json["launchAgent"].is_null());
    assert_eq!(json["agentName"], "ProjectAlpha/architect");
    assert!(String::from_utf8_lossy(&out.stderr).contains("empty command"));
    assert!(project.join(".ac").join("_agent_architect").is_dir());
    assert!(
        session_request_paths(&config_dir).is_empty(),
        "blank command must not write a launch request"
    );
}
