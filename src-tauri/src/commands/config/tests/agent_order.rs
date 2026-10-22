use super::*;

// ── #2306 P2 - ordered move + whole-settings order protection ────────

fn order_ids(agents: &[AgentConfig]) -> Vec<String> {
    agents.iter().map(|agent| agent.id.clone()).collect()
}

fn order_positions(agents: &[AgentConfig]) -> Vec<Option<u32>> {
    agents.iter().map(|agent| agent.order).collect()
}

fn disk_ids_and_orders(path: &Path) -> (Vec<String>, Vec<u64>) {
    // #2716 (B3): `agents` lives in the agents file beside settings.json.
    let raw = std::fs::read_to_string(path.with_file_name("agents.30.instance.no-git.json"))
        .expect("read agents file");
    let disk: Value = serde_json::from_str(&raw).expect("parse agents file");
    let agents = disk["agents"].as_array().expect("agents array");
    (
        agents
            .iter()
            .map(|agent| agent["id"].as_str().unwrap().to_string())
            .collect(),
        agents
            .iter()
            .map(|agent| agent["order"].as_u64().unwrap())
            .collect(),
    )
}

#[tokio::test]
async fn agent_order_update_keeps_live_survivors_and_appends_incoming_only() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let live = settings_with_agents(&["a", "b", "d"]);
    let state = state_for(live.clone());
    // The incoming snapshot carries a forged/stale ordinal order and the
    // incoming membership: `d` is gone, `c` is new, `b` was edited.
    let mut incoming = AppSettings {
        agents: vec![
            agent_with_order("c", Some(9)),
            agent_with_order("a", Some(50)),
            agent_with_order("b", Some(0)),
        ],
        ..live.clone()
    };
    incoming.agents[2].label = "b-edited".to_string();

    let (written, _) =
        persist_protected_settings_update_with_saver(&state, incoming, |candidate| {
            crate::config::settings::save_settings_to_path_preserving_project_paths(
                candidate, &path,
            )
        })
        .await
        .unwrap();

    assert_eq!(order_ids(&written.agents), ["a", "b", "c"]);
    assert_eq!(
        order_positions(&written.agents),
        [Some(0), Some(1), Some(2)]
    );
    assert_eq!(written.agents[1].label, "b-edited");
    let (disk_ids, disk_orders) = disk_ids_and_orders(&path);
    assert_eq!(disk_ids, ["a", "b", "c"]);
    assert_eq!(disk_orders, [0, 1, 2]);
    let reloaded = crate::config::settings::load_settings_from_path(&path);
    assert_eq!(order_ids(&reloaded.agents), ["a", "b", "c"]);
    let live_now = state.read().await;
    assert_eq!(order_ids(&live_now.agents), ["a", "b", "c"]);
}

#[tokio::test]
async fn agent_order_stale_draft_cannot_undo_a_move_and_keeps_incoming_records() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    // Live state after moving `b` above `a`.
    let state = state_for(settings_with_agents(&["b", "a", "c"]));
    // Stale draft taken before the move.
    let mut stale = settings_with_agents(&["a", "b", "c"]);
    stale.agents[1].label = "b-edited-in-draft".to_string();

    let (written, _events) = persist_settings_draft_update_with_saver(&state, stale, |candidate| {
        crate::config::settings::save_settings_to_path_preserving_project_paths(candidate, &path)
    })
    .await
    .unwrap();

    assert_eq!(order_ids(&written.agents), ["b", "a", "c"]);
    assert_eq!(written.agents[0].label, "b-edited-in-draft");
    assert_eq!(
        order_positions(&written.agents),
        [Some(0), Some(1), Some(2)]
    );
    let (disk_ids, disk_orders) = disk_ids_and_orders(&path);
    assert_eq!(disk_ids, ["b", "a", "c"]);
    assert_eq!(disk_orders, [0, 1, 2]);
}

#[tokio::test]
async fn agent_order_draft_add_appends_and_remove_compacts() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let live = settings_with_agents(&["a", "b", "c"]);
    let state = state_for(live.clone());

    // Remove `b`: survivors keep the live order and ordinals compact.
    let mut removed = live.clone();
    removed.agents.retain(|agent| agent.id != "b");
    let (written, _) = persist_settings_draft_update_with_saver(&state, removed, |candidate| {
        crate::config::settings::save_settings_to_path_preserving_project_paths(candidate, &path)
    })
    .await
    .unwrap();
    assert_eq!(order_ids(&written.agents), ["a", "c"]);
    assert_eq!(order_positions(&written.agents), [Some(0), Some(1)]);

    // Add `d` at the end of the effective list, as the modal does.
    let mut added = written.clone();
    added.agents.push(agent_with_order("d", None));
    let (written, _) = persist_settings_draft_update_with_saver(&state, added, |candidate| {
        crate::config::settings::save_settings_to_path_preserving_project_paths(candidate, &path)
    })
    .await
    .unwrap();
    assert_eq!(order_ids(&written.agents), ["a", "c", "d"]);
    assert_eq!(
        order_positions(&written.agents),
        [Some(0), Some(1), Some(2)]
    );
    let (disk_ids, disk_orders) = disk_ids_and_orders(&path);
    assert_eq!(disk_ids, ["a", "c", "d"]);
    assert_eq!(disk_orders, [0, 1, 2]);
}

#[tokio::test]
async fn agent_order_case_distinct_ids_stay_distinct() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let state = state_for(settings_with_agents(&["a"]));
    let mut incoming = settings_with_agents(&["a", "A"]);
    // Case-distinct stable IDs are legal; inputs must not introduce a
    // duplicate configuration under the case-folded physical delta gate.
    incoming.agents[1].command = "uppercase-id-tool".to_string();

    let (written, _) =
        persist_protected_settings_update_with_saver(&state, incoming, |candidate| {
            crate::config::settings::save_settings_to_path_preserving_project_paths(
                candidate, &path,
            )
        })
        .await
        .unwrap();

    assert_eq!(order_ids(&written.agents), ["a", "A"]);
    assert_ne!(written.agents[0].id_hash, written.agents[1].id_hash);
}

#[tokio::test]
async fn agent_order_draft_save_failure_leaves_live_settings_unchanged() {
    let state = state_for(settings_with_agents(&["a", "b", "c"]));
    let stale = settings_with_agents(&["c", "b", "a"]);

    let err = persist_settings_draft_update_with_saver(&state, stale, |_| {
        Err("simulated order save failure".to_string())
    })
    .await
    .unwrap_err();

    assert_eq!(err, "simulated order save failure");
    let live = state.read().await;
    assert_eq!(order_ids(&live.agents), ["a", "b", "c"]);
    assert_eq!(order_positions(&live.agents), [Some(0), Some(1), Some(2)]);
}

#[tokio::test]
async fn move_coding_agent_with_saver_returns_written_order_and_persists_ordinals() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let state = state_for(settings_with_agents(&["a", "b", "c"]));

    let ids =
        move_coding_agent_inner_with_saver(&state, "b", "a", AgentMoveDirection::Up, |candidate| {
            crate::config::settings::save_settings_to_path_preserving_project_paths(
                candidate, &path,
            )
        })
        .await
        .unwrap();

    assert_eq!(ids, ["b", "a", "c"]);
    let (disk_ids, disk_orders) = disk_ids_and_orders(&path);
    assert_eq!(disk_ids, ["b", "a", "c"]);
    assert_eq!(disk_orders, [0, 1, 2]);
    let reloaded = crate::config::settings::load_settings_from_path(&path);
    assert_eq!(order_ids(&reloaded.agents), ["b", "a", "c"]);
    let live = state.read().await;
    assert_eq!(order_ids(&live.agents), ["b", "a", "c"]);
    assert_eq!(order_positions(&live.agents), [Some(0), Some(1), Some(2)]);
}

#[tokio::test]
async fn move_coding_agent_with_saver_rejection_never_calls_saver() {
    let state = state_for(settings_with_agents(&["a", "b", "c"]));

    // `c` sits below `b`, so it is not `b`'s up-neighbor in the latest order.
    let err = move_coding_agent_inner_with_saver(
        &state,
        "b",
        "c",
        AgentMoveDirection::Up,
        |_| -> Result<AppSettings, String> { panic!("saver must not run on a rejected move") },
    )
    .await
    .unwrap_err();

    assert!(err.contains("not adjacent"), "{err}");
    let live = state.read().await;
    assert_eq!(order_ids(&live.agents), ["a", "b", "c"]);
}

#[tokio::test]
async fn move_coding_agent_with_saver_write_failure_leaves_live_state_unchanged() {
    let state = state_for(settings_with_agents(&["a", "b", "c"]));

    let err = move_coding_agent_inner_with_saver(&state, "b", "a", AgentMoveDirection::Up, |_| {
        Err("simulated move save failure".to_string())
    })
    .await
    .unwrap_err();

    assert_eq!(err, "simulated move save failure");
    let live = state.read().await;
    assert_eq!(order_ids(&live.agents), ["a", "b", "c"]);
    assert_eq!(order_positions(&live.agents), [Some(0), Some(1), Some(2)]);
}

#[tokio::test]
async fn move_coding_agent_checks_adjacency_against_the_latest_order() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let state = state_for(settings_with_agents(&["a", "b", "c"]));

    move_coding_agent_inner_with_saver(&state, "c", "b", AgentMoveDirection::Up, |candidate| {
        crate::config::settings::save_settings_to_path_preserving_project_paths(candidate, &path)
    })
    .await
    .unwrap();

    // Latest order is a, c, b: the stale neighbor is rejected without a save.
    let err = move_coding_agent_inner_with_saver(
        &state,
        "c",
        "b",
        AgentMoveDirection::Up,
        |_| -> Result<AppSettings, String> {
            panic!("saver must not run on a stale-neighbor move")
        },
    )
    .await
    .unwrap_err();

    assert!(err.contains("not adjacent"), "{err}");
    let live = state.read().await;
    assert_eq!(order_ids(&live.agents), ["a", "c", "b"]);
}

#[tokio::test]
async fn move_coding_agent_overlay_owned_agents_rejects_before_saver_without_event() {
    use tauri::Listener;

    let temp = tempfile::tempdir().unwrap();
    let path = seed_overlay_agents_fixture(
        temp.path(),
        &json!([
            { "id": "ov-a", "label": "A", "command": "claude", "color": "#111111", "order": 1 },
            { "id": "ov-b", "label": "B", "command": "claude", "color": "#222222" }
        ]),
    );
    let settings = crate::config::settings::load_settings_from_path(&path);
    assert!(settings
        .local_overlay_state
        .owns_top_level(crate::config::settings::OVERLAY_KEY_AGENTS));
    let overlay_order = order_ids(&settings.agents);
    let state = state_for(settings);

    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build overlay rejection app");
    let (tx, rx) = std::sync::mpsc::channel();
    app.listen_any("coding_agent_settings_updated", move |event| {
        let _ = tx.send(event.payload().to_string());
    });

    let err = move_coding_agent_command_with_saver(
        app.handle(),
        &state,
        "ov-a".to_string(),
        "ov-b".to_string(),
        AgentMoveDirection::Up,
        |_| -> Result<AppSettings, String> {
            panic!("saver must not run while the overlay owns `agents`")
        },
    )
    .await
    .unwrap_err();

    assert_eq!(err, AGENT_ORDER_OVERLAY_PINNED);
    let live = state.read().await;
    assert_eq!(order_ids(&live.agents), overlay_order);
    assert!(
        rx.recv_timeout(Duration::from_millis(200)).is_err(),
        "a rejected move must not emit an event"
    );
}

fn abc_ids_2542() -> Vec<String> {
    ["a", "b", "c"].iter().map(|id| id.to_string()).collect()
}

fn mock_app_with_settings_listener_2542() -> (
    tauri::App<tauri::test::MockRuntime>,
    std::sync::mpsc::Receiver<String>,
) {
    use tauri::Listener;

    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build reorder rejection app");
    let (tx, rx) = std::sync::mpsc::channel();
    app.listen_any("coding_agent_settings_updated", move |event| {
        let _ = tx.send(event.payload().to_string());
    });
    (app, rx)
}

#[tokio::test]
async fn reorder_coding_agent_with_saver_persists_and_returns_written_order() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let state = state_for(settings_with_agents(&["a", "b", "c"]));

    let ids = reorder_coding_agent_inner_with_saver(&state, "a", &abc_ids_2542(), 2, |c| {
        crate::config::settings::save_settings_to_path_preserving_project_paths(c, &path)
    })
    .await
    .unwrap();

    assert_eq!(ids, ["b", "c", "a"]);
    let (disk_ids, disk_orders) = disk_ids_and_orders(&path);
    assert_eq!(disk_ids, ["b", "c", "a"]);
    assert_eq!(disk_orders, [0, 1, 2]);
    let live = state.read().await;
    assert_eq!(order_ids(&live.agents), ["b", "c", "a"]);
}

#[tokio::test]
async fn reorder_coding_agent_with_saver_write_failure_leaves_live_state_unchanged() {
    let state = state_for(settings_with_agents(&["a", "b", "c"]));

    let err = reorder_coding_agent_inner_with_saver(&state, "a", &abc_ids_2542(), 2, |_| {
        Err("simulated reorder save failure".to_string())
    })
    .await
    .unwrap_err();

    assert_eq!(err, "simulated reorder save failure");
    let live = state.read().await;
    assert_eq!(order_ids(&live.agents), ["a", "b", "c"]);
    assert_eq!(order_positions(&live.agents), [Some(0), Some(1), Some(2)]);
}

#[tokio::test]
async fn reorder_coding_agent_overlay_owned_agents_rejects_before_saver_without_event() {
    let temp = tempfile::tempdir().unwrap();
    let path = seed_overlay_agents_fixture(
        temp.path(),
        &json!([
            { "id": "ov-a", "label": "A", "command": "claude", "color": "#111111", "order": 1 },
            { "id": "ov-b", "label": "B", "command": "claude", "color": "#222222" }
        ]),
    );
    let settings = crate::config::settings::load_settings_from_path(&path);
    let overlay_order = order_ids(&settings.agents);
    let state = state_for(settings);
    let (app, rx) = mock_app_with_settings_listener_2542();

    let err = reorder_coding_agent_command_with_saver(
        app.handle(),
        &state,
        "ov-a".to_string(),
        overlay_order.clone(),
        0,
        |_| -> Result<AppSettings, String> {
            panic!("saver must not run while the overlay owns `agents`")
        },
    )
    .await
    .unwrap_err();

    assert_eq!(err, AGENT_ORDER_OVERLAY_PINNED);
    assert_eq!(order_ids(&state.read().await.agents), overlay_order);
    assert!(
        rx.recv_timeout(Duration::from_millis(200)).is_err(),
        "a rejected reorder must not emit an event"
    );
}

#[tokio::test]
async fn reorder_coding_agent_stale_expected_ids_rejects_before_saver_without_event() {
    let state = state_for(settings_with_agents(&["a", "b", "c"]));
    let (app, rx) = mock_app_with_settings_listener_2542();

    let err = reorder_coding_agent_command_with_saver(
        app.handle(),
        &state,
        "a".to_string(),
        vec!["b".to_string(), "a".to_string(), "c".to_string()],
        2,
        |_| -> Result<AppSettings, String> { panic!("saver must not run on a stale reorder") },
    )
    .await
    .unwrap_err();

    assert!(err.contains("changed since"), "{err}");
    let live = state.read().await;
    assert_eq!(order_ids(&live.agents), ["a", "b", "c"]);
    assert_eq!(order_positions(&live.agents), [Some(0), Some(1), Some(2)]);
    assert!(
        rx.recv_timeout(Duration::from_millis(200)).is_err(),
        "a rejected reorder must not emit an event"
    );
}

#[test]
fn reorder_coding_agent_is_registered_in_invoke_handler() {
    let lines: Vec<&str> = include_str!("../../../lib.rs")
        .lines()
        .map(str::trim)
        .collect();
    let hits: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| **line == "commands::config::reorder_coding_agent,")
        .map(|(index, _)| index)
        .collect();
    assert_eq!(hits.len(), 1, "registration must occur exactly once");
    assert!(hits[0] > 0);
    assert_eq!(lines[hits[0] - 1], "commands::config::move_coding_agent,");
}
