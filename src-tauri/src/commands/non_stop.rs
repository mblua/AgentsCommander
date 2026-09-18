//! #777 Non-stop watchdog IPC command.
//!
//! The frontend pushes a full snapshot (one entry per project with an ACTIVE
//! Non-stop group) whenever session state, the groups config, or the project
//! list changes, plus a light keepalive. The backend `NonStopWatchdogState`
//! reconciles the snapshot; the `non_stop_watchdog` loop times + actuates.

use crate::loops::non_stop_watchdog::{
    emit_non_stop_alarm, NonStopAlarmPayload, NonStopReport, NonStopWatchdogState,
};
use tauri::State;

#[tauri::command]
pub async fn non_stop_report<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, NonStopWatchdogState>,
    reports: Vec<NonStopReport>,
) -> Result<(), String> {
    for path in state.ingest(reports).await {
        emit_non_stop_alarm(
            &app,
            NonStopAlarmPayload {
                project_path: path,
                group_name: String::new(),
                seconds: 0,
                action: "stop".to_string(),
            },
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loops::non_stop_watchdog::NON_STOP_ALARM_EVENT;
    use std::time::Duration;
    use tauri::{Listener, Manager};

    fn report(project: &str, disparity: bool) -> NonStopReport {
        NonStopReport {
            project_path: project.to_string(),
            group_name: "Alert me!".to_string(),
            disparity,
            working: if disparity { 1 } else { 2 },
            total: 2,
            not_working_workgroups: if disparity {
                vec!["wg-2".to_string()]
            } else {
                vec![]
            },
            tolerance_seconds: 30,
            telegram_enabled: false,
            telegram_bot_id: None,
            sound_enabled: true,
            sound_seconds: 3,
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn non_stop_report_emits_stop_for_each_recovered_path() {
        let app = tauri::test::mock_builder()
            .manage(NonStopWatchdogState::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap_or_else(|_| panic!("build app"));
        let (tx, rx) = std::sync::mpsc::channel();
        app.listen_any(NON_STOP_ALARM_EVENT, move |event| {
            let _ = tx.send(event.payload().to_string());
        });

        app.state::<NonStopWatchdogState>()
            .ingest(vec![report("p", true)])
            .await;

        non_stop_report(
            app.handle().clone(),
            app.state::<NonStopWatchdogState>(),
            vec![report("p", false)],
        )
        .await
        .unwrap();

        let payload: serde_json::Value =
            serde_json::from_str(&rx.recv_timeout(Duration::from_secs(1)).unwrap()).unwrap();
        assert_eq!(payload["action"], "stop");
        assert_eq!(payload["projectPath"], "p");
        assert_eq!(payload["seconds"], 0);
        assert_eq!(payload["groupName"], "");
        let mut keys: Vec<&String> = payload.as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(keys, vec!["action", "groupName", "projectPath", "seconds"]);
        // One stop per recovered path, not a repeat.
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
    }
}
