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
        .ingest(vec![report("p", true), report("q", true)])
        .await;

    non_stop_report(
        app.handle().clone(),
        app.state::<NonStopWatchdogState>(),
        vec![report("p", false), report("q", false)],
    )
    .await
    .unwrap();

    // "For each recovered path": two armed paths -> exactly two stop payloads.
    let mut paths: Vec<String> = Vec::new();
    for _ in 0..2 {
        let payload: serde_json::Value =
            serde_json::from_str(&rx.recv_timeout(Duration::from_secs(1)).unwrap()).unwrap();
        assert_eq!(payload["action"], "stop");
        assert_eq!(payload["seconds"], 0);
        assert_eq!(payload["groupName"], "");
        let mut keys: Vec<&String> = payload.as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(keys, vec!["action", "groupName", "projectPath", "seconds"]);
        paths.push(payload["projectPath"].as_str().unwrap().to_string());
    }
    paths.sort();
    assert_eq!(paths, vec!["p", "q"]);
    // One stop per recovered path, not a repeat.
    assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
}

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn issue_3260_spawn_from_sync_thread() {
    let app = crate::test_support::test_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build established Wry fixture without managed state");
    let host = app.handle().clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        assert!(tokio::runtime::Handle::try_current().is_err());
        for capability in 0..3 {
            let tx = tx.clone();
            let future = Box::pin(async move { tx.send(capability).unwrap() });
            match capability {
                0 => crate::loops::ipc_observer::IpcObserverHost::spawn_background(&host, future),
                1 => crate::loops::non_stop_watchdog::WatchdogHost::spawn_background(&host, future),
                _ => crate::loops::scheduler::SchedulerHost::spawn_background(&host, future),
            }
        }
        done_tx.send(()).unwrap();
    });
    let completed = done_rx.recv_timeout(Duration::from_secs(5));
    assert!(!matches!(
        completed,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ));
    worker.join().expect("propagate synchronous caller panic");
    completed.expect("synchronous caller completed");
    let mut executed = Vec::new();
    for _ in 0..3 {
        executed.push(
            rx.recv_timeout(Duration::from_secs(5))
                .expect("future executed"),
        );
    }
    executed.sort();
    assert_eq!(executed, [0, 1, 2]);
}
