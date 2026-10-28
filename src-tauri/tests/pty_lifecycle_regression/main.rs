#![cfg(target_os = "windows")]

mod fixture;
mod process;

use std::sync::{Arc, Mutex};

use agentscommander_lib::commands::pty::{get_screen_snapshot, PtyScreenSnapshotPayload};
use agentscommander_lib::commands::session::{
    create_session_inner, destroy_session_inner, restart_session_inner, CreateSelectionIntent,
};
use agentscommander_lib::config::sessions_persistence::load_sessions;
use agentscommander_lib::config::settings::SettingsState;
use agentscommander_lib::pty::manager::PtyManager;
use agentscommander_lib::session::manager::SessionManager;
use tauri::Manager;
use uuid::Uuid;

use fixture::{
    assert_active_session, assert_has_pty, assert_no_pty, assert_persisted_contains,
    assert_persisted_missing, assert_session_missing, last_strictly_increasing_sequence,
    make_lifecycle_fixture, make_test_app, parse_session_id, persist_sessions,
    wait_for_snapshot_contains,
};
use process::{
    path_to_string, process_exists, script_args, wait_for_pid_change, wait_for_pid_file,
    wait_for_process_exit, write_child_script, EXIT_TIMEOUT, PID_TIMEOUT,
};

#[test]
fn pty_screen_snapshot_payload_serializes_camel_case_bytes_and_optional_size() {
    let value = serde_json::to_value(PtyScreenSnapshotPayload {
        session_id: "session-1".to_string(),
        data: vec![27, 91, 72],
        rows: Some(30),
        cols: Some(120),
        sequence: 7,
    })
    .expect("serialize snapshot payload");

    assert_eq!(
        value,
        serde_json::json!({
            "sessionId": "session-1",
            "data": [27, 91, 72],
            "rows": 30,
            "cols": 120,
            "sequence": 7,
        })
    );
    assert!(value.get("session_id").is_none());
}

#[test]
fn real_pty_session_lifecycle_create_io_resize_restart_persist_restore_cleanup() {
    let mut fixture = make_lifecycle_fixture();
    assert!(get_screen_snapshot(
        fixture.app.state::<Arc<Mutex<PtyManager>>>(),
        "not-a-uuid".to_string(),
    )
    .is_err());

    let missing = get_screen_snapshot(
        fixture.app.state::<Arc<Mutex<PtyManager>>>(),
        Uuid::nil().to_string(),
    )
    .expect("missing session lookup should not fail");
    assert!(missing.is_none());

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");
    runtime.block_on(async {
        let pid_a = fixture.pid_path("a");
        let session_a = fixture
            .create_session("PTY lifecycle A", &pid_a)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "Phase A create failed: {e}\n{}",
                    fixture.dump("Phase A", &[], std::slice::from_ref(&pid_a))
                )
            });
        let id_a = parse_session_id(&session_a);
        fixture.cleanup.record_session(&fixture.pty_mgr, id_a);
        assert_has_pty(&fixture.pty_mgr, id_a, "Phase A", &fixture);
        wait_for_snapshot_contains(&fixture, &fixture.app, &id_a, "AC_READY", "Phase A")
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "{e}\n{}",
                    fixture.dump("Phase A", &[id_a], std::slice::from_ref(&pid_a))
                )
            });
        {
            let last_event_sequence =
                last_strictly_increasing_sequence(&fixture, &session_a.id, "Phase A");
            let snapshot = get_screen_snapshot(
                fixture.app.state::<Arc<Mutex<PtyManager>>>(),
                session_a.id.clone(),
            )
            .expect("snapshot command succeeds")
            .expect("screen snapshot exists after PTY output");
            let snapshot_text = String::from_utf8_lossy(&snapshot.data);
            assert_eq!(snapshot.session_id, session_a.id);
            assert!(snapshot.rows.is_some(), "Phase A snapshot rows missing");
            assert!(snapshot.cols.is_some(), "Phase A snapshot cols missing");
            assert!(
                snapshot.sequence >= last_event_sequence,
                "Phase A snapshot sequence {} should cover last event sequence {}",
                snapshot.sequence,
                last_event_sequence
            );
            assert!(
                snapshot_text.contains("AC_READY"),
                "Phase A snapshot missing AC_READY; snapshot={snapshot_text:?}\n{}",
                fixture.dump("Phase A snapshot", &[id_a], std::slice::from_ref(&pid_a))
            );
        }
        let pid_a_value = wait_for_pid_file(&pid_a, PID_TIMEOUT)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "{e}\n{}",
                    fixture.dump("Phase A", &[id_a], std::slice::from_ref(&pid_a))
                )
            });
        fixture.cleanup.record_pid(pid_a_value);
        assert!(
            process_exists(pid_a_value),
            "Phase A process not alive\n{}",
            fixture.dump("Phase A", &[id_a], std::slice::from_ref(&pid_a))
        );

        {
            let permit = PtyManager::acquire_input_writer(&fixture.pty_mgr, id_a)
                .await
                .expect("acquire pty writer");
            PtyManager::write_with_permit(&permit, b"AC_ECHO hello-from-pty-lifecycle\r\n")
                .expect("write to pty");
        }
        wait_for_snapshot_contains(
            &fixture,
            &fixture.app,
            &id_a,
            "AC_ECHOED hello-from-pty-lifecycle",
            "Phase B",
        )
        .await
        .unwrap_or_else(|e| {
            panic!(
                "{e}\n{}",
                fixture.dump("Phase B", &[id_a], std::slice::from_ref(&pid_a))
            )
        });
        assert_has_pty(&fixture.pty_mgr, id_a, "Phase B", &fixture);
        assert!(process_exists(pid_a_value));

        {
            fixture
                .pty_mgr
                .lock()
                .unwrap()
                .resize(id_a, 80, 24)
                .expect("resize pty");
        }
        let size = { fixture.pty_mgr.lock().unwrap().get_pty_size(id_a) };
        assert_eq!(
            size,
            Some((24, 80)),
            "Phase C resize mismatch\n{}",
            fixture.dump("Phase C", &[id_a], std::slice::from_ref(&pid_a))
        );
        assert_has_pty(&fixture.pty_mgr, id_a, "Phase C", &fixture);
        assert!(process_exists(pid_a_value));

        persist_sessions(&fixture.session_mgr).await;
        assert_persisted_contains(&fixture, "PTY lifecycle A", "Phase D");

        destroy_session_inner(fixture.app.handle(), id_a)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "Phase E destroy failed: {e}\n{}",
                    fixture.dump("Phase E", &[id_a], std::slice::from_ref(&pid_a))
                )
            });
        wait_for_process_exit(pid_a_value, EXIT_TIMEOUT)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "{e}\n{}",
                    fixture.dump("Phase E", &[id_a], std::slice::from_ref(&pid_a))
                )
            });
        assert_no_pty(&fixture.pty_mgr, id_a, "Phase E", &fixture);
        assert_session_missing(&fixture.session_mgr, id_a).await;
        assert_persisted_missing(&fixture, "PTY lifecycle A", "Phase E");

        let pid_restart = fixture.pid_path("restart");
        let restart_session = fixture
            .create_session("PTY lifecycle restart", &pid_restart)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "Phase F create failed: {e}\n{}",
                    fixture.dump("Phase F", &[], std::slice::from_ref(&pid_restart))
                )
            });
        let restart_old_id = parse_session_id(&restart_session);
        fixture
            .cleanup
            .record_session(&fixture.pty_mgr, restart_old_id);
        wait_for_snapshot_contains(
            &fixture,
            &fixture.app,
            &restart_old_id,
            "AC_READY",
            "Phase F",
        )
        .await
        .unwrap_or_else(|e| {
            panic!(
                "{e}\n{}",
                fixture.dump(
                    "Phase F",
                    &[restart_old_id],
                    std::slice::from_ref(&pid_restart)
                )
            )
        });
        let restart_old_pid = wait_for_pid_file(&pid_restart, PID_TIMEOUT)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "{e}\n{}",
                    fixture.dump(
                        "Phase F",
                        &[restart_old_id],
                        std::slice::from_ref(&pid_restart)
                    )
                )
            });
        fixture.cleanup.record_pid(restart_old_pid);

        let restarted = restart_session_inner(
            fixture.app.handle(),
            &fixture.session_mgr,
            &fixture.pty_mgr,
            fixture.app.state::<SettingsState>().inner(),
            restart_old_id,
            None,
            None,
            Some(true),
        )
        .await
        .unwrap_or_else(|e| {
            panic!(
                "Phase F restart failed: {e}\n{}",
                fixture.dump(
                    "Phase F",
                    &[restart_old_id],
                    std::slice::from_ref(&pid_restart)
                )
            )
        });
        let restart_new_id = parse_session_id(&restarted);
        fixture
            .cleanup
            .record_session(&fixture.pty_mgr, restart_new_id);
        assert_ne!(restart_old_id, restart_new_id);
        wait_for_process_exit(restart_old_pid, EXIT_TIMEOUT)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "{e}\n{}",
                    fixture.dump(
                        "Phase F",
                        &[restart_old_id, restart_new_id],
                        std::slice::from_ref(&pid_restart)
                    )
                )
            });
        let restart_new_pid = wait_for_pid_change(&pid_restart, restart_old_pid, PID_TIMEOUT)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "{e}\n{}",
                    fixture.dump(
                        "Phase F",
                        &[restart_old_id, restart_new_id],
                        std::slice::from_ref(&pid_restart)
                    )
                )
            });
        fixture.cleanup.record_pid(restart_new_pid);
        assert!(process_exists(restart_new_pid));
        wait_for_snapshot_contains(
            &fixture,
            &fixture.app,
            &restart_new_id,
            "AC_READY",
            "Phase F",
        )
        .await
        .unwrap_or_else(|e| {
            panic!(
                "{e}\n{}",
                fixture.dump(
                    "Phase F",
                    &[restart_old_id, restart_new_id],
                    std::slice::from_ref(&pid_restart)
                )
            )
        });
        {
            let last_event_sequence =
                last_strictly_increasing_sequence(&fixture, &restarted.id, "Phase F");
            let snapshot = get_screen_snapshot(
                fixture.app.state::<Arc<Mutex<PtyManager>>>(),
                restarted.id.clone(),
            )
            .expect("restarted snapshot command succeeds")
            .expect("restarted screen snapshot exists after PTY output");
            let snapshot_text = String::from_utf8_lossy(&snapshot.data);
            assert_eq!(snapshot.session_id, restarted.id);
            assert!(snapshot.rows.is_some(), "Phase F snapshot rows missing");
            assert!(snapshot.cols.is_some(), "Phase F snapshot cols missing");
            assert!(
                snapshot.sequence >= last_event_sequence,
                "Phase F snapshot sequence {} should cover last event sequence {}",
                snapshot.sequence,
                last_event_sequence
            );
            assert!(
                snapshot_text.contains("AC_READY"),
                "Phase F snapshot missing AC_READY; snapshot={snapshot_text:?}\n{}",
                fixture.dump(
                    "Phase F snapshot",
                    &[restart_new_id],
                    std::slice::from_ref(&pid_restart)
                )
            );
        }
        assert_no_pty(&fixture.pty_mgr, restart_old_id, "Phase F", &fixture);
        assert_has_pty(&fixture.pty_mgr, restart_new_id, "Phase F", &fixture);
        assert_active_session(&fixture.session_mgr, restart_new_id).await;

        destroy_session_inner(fixture.app.handle(), restart_new_id)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "Phase F destroy failed: {e}\n{}",
                    fixture.dump(
                        "Phase F",
                        &[restart_new_id],
                        std::slice::from_ref(&pid_restart)
                    )
                )
            });
        wait_for_process_exit(restart_new_pid, EXIT_TIMEOUT)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "{e}\n{}",
                    fixture.dump(
                        "Phase F",
                        &[restart_new_id],
                        std::slice::from_ref(&pid_restart)
                    )
                )
            });

        let pid_persisted = fixture.pid_path("persisted");
        let persisted_session = fixture
            .create_session("PTY lifecycle persisted", &pid_persisted)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "Phase G create failed: {e}\n{}",
                    fixture.dump("Phase G", &[], std::slice::from_ref(&pid_persisted))
                )
            });
        let persisted_id = parse_session_id(&persisted_session);
        fixture
            .cleanup
            .record_session(&fixture.pty_mgr, persisted_id);
        wait_for_snapshot_contains(&fixture, &fixture.app, &persisted_id, "AC_READY", "Phase G")
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "{e}\n{}",
                    fixture.dump(
                        "Phase G",
                        &[persisted_id],
                        std::slice::from_ref(&pid_persisted)
                    )
                )
            });
        let persisted_old_pid = wait_for_pid_file(&pid_persisted, PID_TIMEOUT)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "{e}\n{}",
                    fixture.dump(
                        "Phase G",
                        &[persisted_id],
                        std::slice::from_ref(&pid_persisted)
                    )
                )
            });
        fixture.cleanup.record_pid(persisted_old_pid);
        persist_sessions(&fixture.session_mgr).await;
        assert_persisted_contains(&fixture, "PTY lifecycle persisted", "Phase G");

        {
            fixture
                .pty_mgr
                .lock()
                .unwrap()
                .kill(persisted_id)
                .expect("kill persisted pty");
        }
        wait_for_process_exit(persisted_old_pid, EXIT_TIMEOUT)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "{e}\n{}",
                    fixture.dump(
                        "Phase G",
                        &[persisted_id],
                        std::slice::from_ref(&pid_persisted)
                    )
                )
            });
        assert_persisted_contains(&fixture, "PTY lifecycle persisted", "Phase G");

        let persisted_row = load_sessions()
            .into_iter()
            .find(|row| row.name == "PTY lifecycle persisted")
            .expect("persisted recipe row");
        let second_owners = agentscommander_lib::AppOwners::new();
        let second_session_mgr = Arc::new(tokio::sync::RwLock::new(
            SessionManager::with_activity_coalescer(second_owners.activity.clone()),
        ));
        let second_captured = Arc::clone(&fixture.captured_output);
        let second_sequences = Arc::clone(&fixture.captured_sequences);
        let second_deliveries = Arc::clone(&fixture.captured_deliveries);
        let second_errors = Arc::clone(&fixture.listener_errors);
        let (second_app, second_mgr, second_pty_mgr) = make_test_app(
            &fixture.repo_root,
            &second_captured,
            &second_sequences,
            &second_deliveries,
            &second_errors,
            &fixture.cleanup,
            Some(second_session_mgr),
            second_owners,
        );

        let restored = create_session_inner(
            second_app.handle(),
            &second_mgr,
            &second_pty_mgr,
            persisted_row.shell,
            persisted_row.shell_args,
            persisted_row.working_directory,
            Some(persisted_row.name),
            persisted_row.agent_id,
            persisted_row.agent_label,
            true,
            persisted_row.git_repos,
            false,
            None,
            None,
            None, // #973 - no view in this test: 120x30
            CreateSelectionIntent::User,
        )
        .await
        .unwrap_or_else(|e| {
            panic!(
                "Phase G restore failed: {e}\n{}",
                fixture.dump(
                    "Phase G",
                    &[persisted_id],
                    std::slice::from_ref(&pid_persisted)
                )
            )
        });
        let restored_id = parse_session_id(&restored);
        fixture.cleanup.record_session(&second_pty_mgr, restored_id);
        wait_for_snapshot_contains(&fixture, &second_app, &restored_id, "AC_READY", "Phase G")
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "{e}\n{}",
                    fixture.dump(
                        "Phase G",
                        &[persisted_id, restored_id],
                        std::slice::from_ref(&pid_persisted)
                    )
                )
            });
        let restored_pid = wait_for_pid_change(&pid_persisted, persisted_old_pid, PID_TIMEOUT)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "{e}\n{}",
                    fixture.dump(
                        "Phase G",
                        &[persisted_id, restored_id],
                        std::slice::from_ref(&pid_persisted)
                    )
                )
            });
        fixture.cleanup.record_pid(restored_pid);
        assert!(process_exists(restored_pid));
        assert_has_pty(&second_pty_mgr, restored_id, "Phase G", &fixture);

        destroy_session_inner(second_app.handle(), restored_id)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "Phase G restored destroy failed: {e}\n{}",
                    fixture.dump(
                        "Phase G",
                        &[restored_id],
                        std::slice::from_ref(&pid_persisted)
                    )
                )
            });
        wait_for_process_exit(restored_pid, EXIT_TIMEOUT)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "{e}\n{}",
                    fixture.dump(
                        "Phase G",
                        &[restored_id],
                        std::slice::from_ref(&pid_persisted)
                    )
                )
            });
    });
}

#[test]
fn claude_launch_materializes_context_without_prompt_file_arg() {
    let mut fixture = make_lifecycle_fixture();
    let agent_cwd = fixture.repo_root.join(".ac").join("_agent_claude");
    std::fs::create_dir_all(&agent_cwd).expect("create claude agent cwd");
    let script_path = agent_cwd.join("claude.ps1");
    write_child_script(&script_path);
    let pid_file = fixture.pid_path("claude_no_prompt_arg");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    runtime.block_on(async {
        let info = create_session_inner(
            fixture.app.handle(),
            &fixture.session_mgr,
            &fixture.pty_mgr,
            "powershell.exe".to_string(),
            script_args(&script_path, &pid_file),
            path_to_string(&agent_cwd),
            Some("Claude context materialization".to_string()),
            None,
            None,
            false,
            Vec::new(),
            true,
            None,
            None,
            None, // #973 - no view in this test: 120x30
            CreateSelectionIntent::User,
        )
        .await
        .unwrap_or_else(|e| panic!("Claude launch failed: {e}"));

        let session_id = parse_session_id(&info);
        fixture.cleanup.record_session(&fixture.pty_mgr, session_id);
        wait_for_snapshot_contains(
            &fixture,
            &fixture.app,
            &session_id,
            "AC_READY",
            "Claude materialization",
        )
        .await
        .unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(
            info.agent_kind,
            Some(agentscommander_lib::session::profile::CodingAgentKind::Claude)
        );
        assert!(
            agent_cwd.join("CLAUDE.md").is_file(),
            "Claude managed instructions file should still be materialized"
        );
        let effective_args = info
            .effective_shell_args
            .as_ref()
            .expect("effective args are captured before spawn");
        assert!(
            !effective_args
                .iter()
                .any(|arg| arg.to_lowercase().contains("--append-system-prompt-file")),
            "Claude launch args must not include --append-system-prompt-file: {effective_args:?}"
        );

        let pid = wait_for_pid_file(&pid_file, PID_TIMEOUT)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        fixture.cleanup.record_pid(pid);
        destroy_session_inner(fixture.app.handle(), session_id)
            .await
            .unwrap_or_else(|e| panic!("destroy Claude materialization session failed: {e}"));
        wait_for_process_exit(pid, EXIT_TIMEOUT)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
    });
}
