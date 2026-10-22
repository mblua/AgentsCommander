use super::*;

// ---------------------------------------------------------------------
// #1551 - events and the emitter
// ---------------------------------------------------------------------

#[tokio::test]
async fn run_update_target_emits_started_then_finished_and_updates_gate() {
    let (app, mut frames_rx) = app_with_broadcaster();
    let handle = app.handle().clone();
    let gate = Arc::new(AgentUpdateGate::new());
    let dir = tempfile::tempdir().expect("tempdir");
    let target = UpdateTarget {
        command: "x-1551-missing".to_string(),
        label: "X".to_string(),
        commands: vec!["exit 0".to_string()],
        cwd: dir.path().to_path_buf(),
    };
    gate.mark_started_with_nodes(vec![AgentUpdateNode {
        command: target.command.clone(),
        label: target.label.clone(),
        update_commands: target.commands.clone(),
        install_before: None,
    }]);

    let result = run_update_target(handle.clone(), Arc::clone(&gate), target).await;
    assert!(result.ok, "unexpected: {result:?}");

    let frames = drain_frames(&mut frames_rx);
    assert_eq!(frames.len(), 2, "unexpected frames: {frames:?}");
    assert_eq!(frames[0]["event"], "agent_update_command_started");
    assert_eq!(frames[0]["payload"]["command"], "x-1551-missing");
    assert_eq!(frames[0]["payload"]["updateCommands"], json!(["exit 0"]));
    assert_eq!(frames[0]["payload"]["installBefore"]["status"], "missing");
    assert_eq!(frames[1]["event"], "agent_update_command_finished");
    assert_eq!(frames[1]["payload"]["ok"], true);
    assert_eq!(frames[1]["payload"]["outcome"], "succeeded");
    assert!(frames[1]["payload"]["installAfter"].is_object());

    let snapshot = gate.snapshot();
    assert!(snapshot.running.is_empty());
    assert_eq!(snapshot.results.len(), 1);
    assert_eq!(
        snapshot.nodes[0]
            .install_before
            .as_ref()
            .map(|state| state.status),
        Some(InstallStatus::Missing)
    );
}

#[tokio::test]
async fn skip_prompted_target_emits_once_and_prunes_the_node() {
    let (app, mut frames_rx) = app_with_broadcaster();
    let handle = app.handle().clone();
    let gate = AgentUpdateGate::new();
    gate.mark_started_with_nodes(vec![
        node("claude", "Claude", vec!["claude --update"]),
        node("codex", "Codex", vec!["codex update"]),
    ]);
    skip_prompted_target(&handle, &gate, "codex");
    let frames = drain_frames(&mut frames_rx);
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0]["event"], "agent_update_command_skipped");
    assert_eq!(
        frames[0]["payload"],
        json!({ "command": "codex", "label": "Codex" })
    );
    assert_eq!(
        gate.snapshot().nodes,
        vec![node("claude", "Claude", vec!["claude --update"])]
    );

    skip_prompted_target(&handle, &gate, "codex");
    assert_no_frame(&mut frames_rx).await;
    assert_eq!(gate.snapshot().nodes.len(), 1);
}

#[tokio::test]
async fn settle_joined_update_turns_a_panic_into_command_finished() {
    let (app, mut frames_rx) = app_with_broadcaster();
    let handle = app.handle().clone();
    let gate = AgentUpdateGate::new();
    gate.mark_command_started("x", "X", None);
    let target = UpdateTarget {
        command: "x".to_string(),
        label: "X".to_string(),
        commands: vec!["exit 0".to_string()],
        cwd: cwd(),
    };
    let joined: Result<AgentUpdateResult, tokio::task::JoinError> =
        tokio::spawn(async { panic!("boom") }).await;
    let settled = settle_joined_update(&handle, &gate, &target, joined);
    assert!(!settled.ok);
    assert_eq!(
        settled.error.as_deref(),
        Some("Update task failed unexpectedly.")
    );
    let snapshot = gate.snapshot();
    assert!(snapshot.running.is_empty());
    assert_eq!(snapshot.results.len(), 1);
    assert_eq!(
        snapshot.results[0].error.as_deref(),
        Some("Update task failed unexpectedly.")
    );
    let frames = drain_frames(&mut frames_rx);
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0]["event"], "agent_update_command_finished");
    assert_eq!(
        frames[0]["payload"]["error"],
        "Update task failed unexpectedly."
    );

    // The `Ok` branch returns the result unchanged and emits nothing.
    let passthrough = settle_joined_update(&handle, &gate, &target, Ok(ok_result("y", "Y")));
    assert!(passthrough.ok);
    assert_no_frame(&mut frames_rx).await;
}

#[tokio::test]
async fn emit_all_reaches_websocket_subscribers_and_survives_no_broadcaster() {
    let (app, mut frames_rx) = app_with_broadcaster();
    emit_all(
        app.handle(),
        "agent_updates_started",
        json!({ "nodes": [] }),
    );
    let frames = drain_frames(&mut frames_rx);
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0]["event"], "agent_updates_started");

    let bare = build_mock_app(crate::test_support::test_builder());
    emit_all(
        bare.handle(),
        "agent_updates_started",
        json!({ "nodes": [] }),
    );
}

#[test]
fn agent_update_emits_only_through_emit_all() {
    let src = include_str!("../../agent_update.rs");
    let lines: Vec<&str> = src.lines().collect();
    let boundary = lines
        .iter()
        .enumerate()
        .find(|(index, line)| {
            **line == "mod tests;"
                && lines[..*index]
                    .iter()
                    .rev()
                    .find(|previous| !previous.trim().is_empty())
                    == Some(&"#[cfg(test)]")
        })
        .map(|(index, _)| index)
        .expect("the tests module boundary");

    let production: Vec<&str> = lines[..boundary]
        .iter()
        .copied()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect();
    assert_eq!(
        production.join("\n").matches(".emit(").count(),
        1,
        "every agent-update emit must go through emit_all"
    );
    let children = [
        include_str!("../cancellation.rs"),
        include_str!("../gate.rs"),
        include_str!("../install.rs"),
        include_str!("../pass.rs"),
        include_str!("../plan.rs"),
        include_str!("../target.rs"),
        include_str!("../updater.rs"),
    ];
    for child in children {
        let emits = child
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .filter(|line| line.contains(".emit("))
            .count();
        assert_eq!(emits, 0, "every agent-update emit must go through emit_all");
    }
    let emit_all_line = production
        .iter()
        .position(|line| line.starts_with("fn emit_all("))
        .expect("fn emit_all");
    let emit_line = production
        .iter()
        .position(|line| line.contains(".emit("))
        .expect("the single .emit( call site");
    let next_item = production[emit_all_line + 1..]
        .iter()
        .position(|line| {
            line.starts_with("fn ") || line.starts_with("struct ") || line.starts_with("impl ")
        })
        .map(|offset| offset + emit_all_line + 1)
        .unwrap_or(production.len());
    assert!(
        emit_all_line < emit_line && emit_line < next_item,
        "the only .emit( must live inside emit_all"
    );
}

// ---------------------------------------------------------------------
// #1551 - rows and the probe policy
// ---------------------------------------------------------------------

#[test]
fn overview_rows_use_the_first_effective_entry_and_skip_empty_winners() {
    let catalog = vec![
        entry("claude", "Claude", vec!["claude --update"]),
        entry("codex", "Codex", vec!["codex update"]),
        entry("hermes", "Hermes", vec!["hermes update --yes"]),
        entry("agent", "Cursor", vec![]),
        entry("pi", "Pi", vec!["pi update"]),
        entry("opencode", "OpenCode", vec!["opencode upgrade"]),
        entry("agy", "Antigravity", vec!["agy update"]),
        CodingAgentDefinition {
            key: "pi-alt".to_string(),
            ..entry("pi", "Pi (alt)", vec!["pi --custom"])
        },
    ];
    let mut installed = InstallState::installed("1.0".to_string(), Path::new("/bin/pi"));
    installed.seq = 7;
    let install_by_command = HashMap::from([("pi".to_string(), installed.clone())]);
    let rows = build_update_overview_rows(&catalog, &install_by_command);
    assert_eq!(
        rows.iter().map(|row| row.key.as_str()).collect::<Vec<_>>(),
        vec!["claude", "codex", "hermes", "pi", "opencode", "agy"]
    );
    assert!(
        !rows.iter().any(|row| row.command == "agent"),
        "cursor ships no update command"
    );
    let pi = rows.iter().find(|row| row.command == "pi").expect("pi row");
    assert_eq!(pi.key, "pi");
    assert_eq!(pi.label, "Pi");
    assert_eq!(pi.update_commands, vec!["pi update"]);
    assert_eq!(pi.install, installed);
    assert_eq!(rows[0].install, InstallState::checking());
    assert_eq!(rows[0].install.seq, 0);

    // An empty FIRST entry suppresses the command entirely: no row at all,
    // even when a later duplicate carries a sequence.
    let suppressed = vec![
        entry("pi", "Pi (empty)", vec![]),
        CodingAgentDefinition {
            key: "pi-alt".to_string(),
            ..entry("pi", "Pi (alt)", vec!["pi --custom"])
        },
    ];
    assert!(build_update_overview_rows(&suppressed, &HashMap::new()).is_empty());
}

#[tokio::test]
async fn probe_command_install_state_unresolvable_is_missing() {
    let state = probe_command_install_state("definitely-missing-cli-1551").await;
    assert_eq!(state.status, InstallStatus::Missing);
    assert!(
        state
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("definitely-missing-cli-1551")),
        "unexpected: {state:?}"
    );
}

#[tokio::test]
async fn probe_command_install_state_explicit_path_is_unprobed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("tool.exe");
    std::fs::write(&file, b"x").expect("write");
    let state = probe_command_install_state(&file.to_string_lossy()).await;
    assert_eq!(state.status, InstallStatus::Unprobed);
    assert!(state.path.is_some());
    assert_eq!(
        state.detail.as_deref(),
        Some("explicit path: version not probed")
    );
}

#[tokio::test]
async fn probe_command_install_state_unknown_stem_is_unprobed() {
    let token = if cfg!(windows) { "cmd" } else { "sh" };
    let state = probe_command_install_state(token).await;
    assert_eq!(state.status, InstallStatus::Unprobed);
    assert!(state.path.is_some());
    assert!(
        state
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("no built-in version probe")),
        "unexpected: {state:?}"
    );
}

#[tokio::test]
async fn probe_command_install_state_empty_command_is_missing() {
    let state = probe_command_install_state("").await;
    assert_eq!(state.status, InstallStatus::Missing);
    assert_eq!(state.detail.as_deref(), Some("empty command"));
}
