use super::*;

use crate::config::loops::{
    write_loop_config, BusyCoordinatorPolicy, LoopDef, LoopPolicy, LoopPrompt, LoopTarget,
    LoopTargetKind, LoopTrigger, LoopTriggerKind, LOOP_STATE_FILE, LOOP_TIMEZONE_LOCAL,
};

fn sample_config() -> LoopConfigToml {
    LoopConfigToml {
        loop_def: LoopDef {
            id: "daily-sync".to_string(),
            name: "Daily sync".to_string(),
            enabled: true,
        },
        trigger: LoopTrigger {
            kind: LoopTriggerKind::Cron,
            expr: "0 9 * * *".to_string(),
            timezone: LOOP_TIMEZONE_LOCAL.to_string(),
        },
        target: LoopTarget {
            kind: LoopTargetKind::WorkgroupCoordinator,
            workgroup: "wg-1-dev-team".to_string(),
        },
        prompt: LoopPrompt {
            body: "Send status".to_string(),
        },
        policy: LoopPolicy {
            busy_coordinator: BusyCoordinatorPolicy::WaitUntilIdle,
            ..LoopPolicy::default()
        },
    }
}

fn project_with_loop(config: &LoopConfigToml) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ac_root = tmp.path().join(".ac");
    std::fs::create_dir_all(&ac_root).expect("create ac root");
    write_loop_config(&ac_root, config).expect("write config");
    tmp
}

/// AC-17 fixture - the only app these writers need. They reach
/// `append_loop_audit_once`, `write_loop_state_atomic` and
/// `emit_transition`, and `emit_loop_change` only calls `app.emit`, which
/// is infallible here, so no managed state is required.
fn audit_writer_app() -> tauri::App {
    crate::test_support::test_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build audit writer app")
}

fn delivered_report() -> LoopDeliveryReport {
    LoopDeliveryReport {
        kind: LoopAuditKind::Delivered,
        message: "gated delivery".to_string(),
        target: None,
        session_id: None,
        error: None,
        prompt_snapshot: None,
        completed_at: Some(Utc::now()),
    }
}

/// #2695 scheduler-side fixture: a project with one Loop whose pending
/// run makes the next scan deliver, then apply the report (S2).
fn pending_loop_fixture() -> (tempfile::TempDir, PathBuf) {
    let config = sample_config();
    let tmp = project_with_loop(&config);
    let dir = loop_dir(&tmp.path().join(".ac"), &config.loop_def.id);
    let state = LoopState {
        last_checked_at: Some(Utc::now() - chrono::Duration::minutes(5)),
        pending_due_at: Some(Utc::now() - chrono::Duration::minutes(5)),
        pending_run_id: Some(Uuid::new_v4()),
        ..LoopState::default()
    };
    crate::config::loops::write_loop_state_atomic(&dir, &state).expect("write state");
    (tmp, dir)
}

/// A project whose `wg-1-dev-team` target resolves, so a real update
/// section passes validation, with one Loop and a baseline state.
fn t2b_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ac_root = tmp.path().join(".ac");
    let team_dir = ac_root.join("_team_dev-team");
    let matrix = ac_root.join("_agent_tech-lead");
    let replica = ac_root.join("wg-1-dev-team").join("__agent_tech-lead");
    for dir in [&team_dir, &matrix, &replica] {
        std::fs::create_dir_all(dir).expect("create fixture dir");
    }
    std::fs::write(matrix.join("Role.md"), "# Tech Lead\n").expect("role");
    std::fs::write(
        team_dir.join("config.json"),
        r#"{"agents":["_agent_tech-lead"],"coordinator":"_agent_tech-lead","repos":[]}"#,
    )
    .expect("team config");
    std::fs::write(
        replica.join("config.json"),
        r#"{"identity":"../../_agent_tech-lead"}"#,
    )
    .expect("replica config");
    let config = sample_config();
    let dir = write_loop_config(&ac_root, &config).expect("write config");
    let baseline =
        baseline_loop_state(&config, Utc::now() - chrono::Duration::hours(1)).expect("baseline");
    write_loop_state_atomic(&dir, &baseline).expect("write state");
    (tmp, ac_root, dir)
}

fn audit_rows(dir: &Path) -> Vec<LoopAuditEntry> {
    let path = dir.join(crate::config::loops::LOOP_AUDIT_FILE);
    if !path.is_file() {
        return Vec::new();
    }
    std::fs::read_to_string(path)
        .expect("audit read")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("audit row"))
        .collect()
}

fn has_record(scheduler: &LoopScheduler, dir: &Path) -> bool {
    scheduler
        .unrecorded_deliveries
        .lock()
        .expect("records")
        .contains_key(&generation_key(dir))
}

fn recorded_entry(scheduler: &LoopScheduler, dir: &Path) -> LoopAuditEntry {
    scheduler.unrecorded_deliveries.lock().expect("records")[&generation_key(dir)]
        .entry
        .clone()
}

fn gate_is_untaken(scheduler: &LoopScheduler) -> bool {
    scheduler.delivery_gate.lock().expect("gate").is_some()
}

fn install_report_gate(scheduler: &LoopScheduler, report: LoopDeliveryReport) {
    scheduler.install_delivery_gate(LoopDeliveryGate {
        entered: None,
        release: None,
        report: Some(report),
    });
}

mod issue_2682;
mod issue_2698;
mod issue_2733;
mod scan;
