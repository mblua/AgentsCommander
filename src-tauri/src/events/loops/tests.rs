use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tauri::Listener;

use crate::config::loops::{
    AcLoopSummary, BusyCoordinatorPolicy, LoopSessionStart, LoopTargetKind,
};
use crate::loops::events::emit_loop_change;

fn summary() -> AcLoopSummary {
    AcLoopSummary {
        id: "daily".into(),
        name: "Daily".into(),
        enabled: true,
        expr: "0 9 * * *".into(),
        timezone: "local".into(),
        target_kind: LoopTargetKind::WorkgroupCoordinator,
        workgroup: "room-1-team".into(),
        prompt_preview: "Report".into(),
        busy_coordinator: BusyCoordinatorPolicy::WaitUntilIdle,
        session_start: LoopSessionStart::Fresh,
        path: "project/.ac/loops/daily".into(),
        config_path: "project/.ac/loops/daily/loop.toml".into(),
        last_checked_at: None,
        last_due_at: None,
        last_delivered_at: None,
        last_result: None,
        pending_due_at: None,
        last_missed_closed_at: None,
        next_due_at: None,
    }
}

fn assert_delivery(
    project: &Path,
    changed: &Path,
    kind: &str,
    reason: &str,
    summary: Option<AcLoopSummary>,
    message: Option<String>,
) {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build loop event app without managed state");
    let events = Arc::new(Mutex::new(Vec::new()));
    for name in ["loop_event", "ac_project_refresh_requested"] {
        let events = Arc::clone(&events);
        app.listen_any(name, move |event| {
            events.lock().unwrap().push((
                name,
                serde_json::from_str::<Value>(event.payload()).expect("event JSON"),
            ));
        });
    }
    let project_string = std::fs::canonicalize(project)
        .unwrap_or_else(|_| project.into())
        .to_string_lossy()
        .to_string();
    let changed_string = std::fs::canonicalize(changed)
        .unwrap_or_else(|_| changed.into())
        .to_string_lossy()
        .to_string();
    let expected_summary = serde_json::to_value(&summary).unwrap();
    emit_loop_change(
        app.handle(),
        project,
        changed,
        "daily",
        kind,
        summary,
        message.clone(),
    );
    let events = events.lock().unwrap();
    assert_eq!(
        events.len(),
        2,
        "both events arrive synchronously exactly once"
    );
    assert_eq!(events[0].0, "loop_event");
    assert_eq!(events[1].0, "ac_project_refresh_requested");
    assert_eq!(
        events[0].1,
        json!({
            "kind": kind, "projectPath": project_string, "loopId": "daily",
            "summary": expected_summary, "message": message,
        })
    );
    let id = events[1].1["id"].as_str().expect("refresh id");
    let id = uuid::Uuid::parse_str(id).expect("valid UUID");
    assert_eq!(id.get_version(), Some(uuid::Version::Random));
    assert_eq!(id.get_variant(), uuid::Variant::RFC4122);
    assert_eq!(
        events[1].1,
        json!({
            "id": id.to_string(), "projectPath": project_string, "changedPath": changed_string,
            "changedName": "daily", "reason": reason,
        })
    );
}

#[test]
fn canonical_existing_project_and_missing_changed_path_preserve_order_and_payload() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join(".");
    assert_ne!(
        project.to_string_lossy(),
        std::fs::canonicalize(&project).unwrap().to_string_lossy()
    );
    assert_delivery(
        &project,
        &tmp.path().join("missing"),
        "updated",
        "loopUpdated",
        Some(summary()),
        Some("Saved".into()),
    );
}

#[test]
fn missing_project_and_existing_changed_path_fall_back_independently() {
    let tmp = tempfile::tempdir().unwrap();
    assert_delivery(
        &tmp.path().join("missing"),
        &tmp.path().join("."),
        "",
        "loopChanged",
        None,
        None,
    );
}

#[test]
fn both_missing_paths_preserve_spelling_and_unicode_reason() {
    let tmp = tempfile::tempdir().unwrap();
    assert_delivery(
        &tmp.path().join("missing-project"),
        &tmp.path().join("missing-loop"),
        "ßaved",
        "loopSSaved",
        None,
        None,
    );
}

#[test]
fn issue_3260_delivery_prompt_event_payload_and_best_effort_delivery() {
    use crate::loops::delivery::LoopPromptEvents;
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&events);
    let listener = app.listen_any("last_prompt", move |event| {
        captured
            .lock()
            .unwrap()
            .push(serde_json::from_str::<Value>(event.payload()).unwrap());
    });
    let id = uuid::Uuid::new_v4();
    app.handle().last_prompt(id, "status\nñ");
    assert_eq!(
        *events.lock().unwrap(),
        [json!({"sessionId": id.to_string(), "text": "status\nñ"})]
    );
    app.unlisten(listener);
    // No managed broadcaster, no window and no listener are required; the port returns unit.
    app.handle().last_prompt(id, "unobserved");
    assert_eq!(events.lock().unwrap().len(), 1);
}

#[test]
fn issue_3260_delivery_prompt_event_ignores_transport_error() {
    let id = uuid::Uuid::new_v4();
    let mut calls = 0;
    super::emit_last_prompt(id, "report", |name, payload| {
        calls += 1;
        assert_eq!(name, "last_prompt");
        assert_eq!(
            payload,
            json!({"sessionId": id.to_string(), "text": "report"})
        );
        Err(tauri::Error::AssetNotFound(
            "recording transport failure".into(),
        ))
    });
    assert_eq!(calls, 1);
}
