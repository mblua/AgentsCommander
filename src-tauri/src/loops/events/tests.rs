use super::*;

use std::cell::RefCell;

use crate::config::loops::{BusyCoordinatorPolicy, LoopSessionStart, LoopTargetKind};

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

struct Recorder<'a> {
    changes: RefCell<Vec<LoopChange<'a>>>,
}

impl LoopEvents for Recorder<'_> {
    fn loop_changed(&self, change: LoopChange<'_>) {
        // The recorder's borrowed paths stay local; compare before the port returns.
        assert_eq!(change.project_path, Path::new("project/./raw"));
        assert_eq!(change.changed_path, Path::new("missing/../loop"));
        assert_eq!(change.loop_id, "daily");
        assert_eq!(change.kind, "updated");
        self.changes.borrow_mut().push(LoopChange {
            project_path: Path::new("project/./raw"),
            changed_path: Path::new("missing/../loop"),
            loop_id: "daily",
            kind: "updated",
            summary: change.summary,
            message: change.message,
        });
    }
}

#[test]
fn forwards_exact_change_once_synchronously_with_optional_payloads() {
    for (summary, message) in [(None, None), (Some(summary()), Some("Saved".into()))] {
        let expected_summary = serde_json::to_value(&summary).unwrap();
        let expected_message = message.clone();
        let recorder = Recorder {
            changes: RefCell::new(Vec::new()),
        };
        let port: &dyn LoopEvents = &recorder;
        emit_loop_change(
            port,
            Path::new("project/./raw"),
            Path::new("missing/../loop"),
            "daily",
            "updated",
            summary,
            message,
        );
        let changes = recorder.changes.borrow();
        assert_eq!(changes.len(), 1);
        assert_eq!(
            serde_json::to_value(&changes[0].summary).unwrap(),
            expected_summary
        );
        assert_eq!(changes[0].message, expected_message);
    }
}

#[test]
fn payload_serialization_preserves_exact_camel_case_keys_and_nulls() {
    for summary in [None, Some(summary())] {
        let expected_summary = serde_json::to_value(&summary).unwrap();
        let payload = LoopEventPayload {
            kind: "updated".into(),
            project_path: "project".into(),
            loop_id: "daily".into(),
            summary,
            message: None,
        };
        assert_eq!(
            serde_json::to_value(payload).unwrap(),
            serde_json::json!({
                "kind": "updated", "projectPath": "project", "loopId": "daily",
                "summary": expected_summary, "message": null,
            })
        );
    }
}
