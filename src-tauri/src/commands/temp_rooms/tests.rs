use super::*;
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};

fn manager() -> Arc<tokio::sync::RwLock<SessionManager>> {
    Arc::new(tokio::sync::RwLock::new(SessionManager::new()))
}

fn listed(base: &Path) -> Vec<String> {
    list_temp_rooms_in(base)
        .expect("list")
        .into_iter()
        .map(|room| room.name)
        .collect()
}

fn create(base: &Path) -> String {
    create_next_with(base, create_dir_exclusive)
        .expect("create")
        .name
}

#[test]
fn lowest_free_number_skips_taken_and_reuses_gaps() {
    assert_eq!(
        lowest_free_temp_room_number(["temp-room-tr01", "temp-room-tr03"]),
        2
    );
    assert_eq!(lowest_free_temp_room_number([]), 1);
    assert_eq!(
        lowest_free_temp_room_number(["room-1-x", "wg-1-x", "temp-room-tr1"]),
        1
    );
}

#[test]
fn create_starts_at_one_and_reuses_gaps() {
    let tmp = tempfile::tempdir().expect("tempdir");
    assert_eq!(create(tmp.path()), "temp-room-tr01");
    std::fs::create_dir(tmp.path().join("temp-room-tr03")).expect("tr03");
    assert_eq!(create(tmp.path()), "temp-room-tr02");
    assert_eq!(create(tmp.path()), "temp-room-tr04");
    std::fs::remove_dir(tmp.path().join("temp-room-tr02")).expect("free tr02");
    assert_eq!(create(tmp.path()), "temp-room-tr02");
}

#[test]
fn create_ignores_names_that_do_not_parse() {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(tmp.path().join("room-1-x")).expect("room");
    std::fs::create_dir(tmp.path().join("temp-room-tr1")).expect("short");
    let room = create_next_with(tmp.path(), create_dir_exclusive).expect("create");
    assert_eq!(room.name, "temp-room-tr01");
    assert!(tmp.path().join("temp-room-tr01").is_dir());
    assert_eq!(
        room.path,
        crate::path_utils::path_to_string_without_windows_verbatim_prefix(
            &tmp.path().join("temp-room-tr01")
        )
    );
}

#[test]
fn stray_files_and_links_take_a_number_but_are_not_listed() {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(tmp.path().join("temp-room-tr01")).expect("dir");
    std::fs::write(tmp.path().join("temp-room-tr02"), b"x").expect("file");
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        tmp.path().join("temp-room-tr01"),
        tmp.path().join("temp-room-tr03"),
    )
    .expect("link");
    #[cfg(not(unix))]
    std::fs::write(tmp.path().join("temp-room-tr03"), b"x").expect("file");
    assert_eq!(create(tmp.path()), "temp-room-tr04");
    assert_eq!(listed(tmp.path()), vec!["temp-room-tr01", "temp-room-tr04"]);
}

#[test]
fn create_retries_after_a_lost_race() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut first = true;
    let room = create_next_with(tmp.path(), |path| {
        if std::mem::take(&mut first) {
            create_dir_exclusive(path)?;
            return Err(io::Error::from(io::ErrorKind::AlreadyExists));
        }
        create_dir_exclusive(path)
    })
    .expect("create");
    assert_eq!(room.name, "temp-room-tr02");
}

#[test]
fn create_skips_a_number_the_fs_reports_taken_without_a_listed_name() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let room = create_next_with(tmp.path(), |path| {
        if path.ends_with("temp-room-tr01") {
            return Err(io::Error::from(io::ErrorKind::AlreadyExists));
        }
        create_dir_exclusive(path)
    })
    .expect("create");
    assert_eq!(room.name, "temp-room-tr02");
}

#[test]
fn create_returns_other_errors() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let err = create_next_with(tmp.path(), |_| Err(io::Error::other("boom"))).unwrap_err();
    assert!(err.contains("boom"), "{err}");
}

#[test]
fn list_sorts_by_number() {
    let tmp = tempfile::tempdir().expect("tempdir");
    for name in ["temp-room-tr10", "temp-room-tr02", "ac-root-agent"] {
        std::fs::create_dir(tmp.path().join(name)).expect("dir");
    }
    assert_eq!(listed(tmp.path()), vec!["temp-room-tr02", "temp-room-tr10"]);
}

#[tokio::test]
async fn delete_rejects_non_temp_and_unsafe_targets() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mgr = manager();
    for name in ["ac-root-agent", "room-1-dev"] {
        std::fs::create_dir(tmp.path().join(name)).expect("dir");
    }
    std::fs::write(tmp.path().join("temp-room-tr06"), b"x").expect("file");
    for name in ["../x", "ac-root-agent", "room-1-dev", "temp-room-tr1"] {
        let err = delete_temp_room_in(tmp.path(), name, &mgr)
            .await
            .unwrap_err();
        assert_eq!(err, "Invalid temp room name", "{name}");
    }
    for name in ["temp-room-tr05", "temp-room-tr06"] {
        let err = delete_temp_room_in(tmp.path(), name, &mgr)
            .await
            .unwrap_err();
        assert_eq!(err, "delete_root_not_directory", "{name}");
    }
    assert!(tmp.path().join("ac-root-agent").is_dir());
    assert!(tmp.path().join("room-1-dev").is_dir());
    assert!(tmp.path().join("temp-room-tr06").is_file());
}

#[cfg(unix)]
#[tokio::test]
async fn delete_rejects_a_symlink() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let target = tmp.path().join("target");
    std::fs::create_dir(&target).expect("dir");
    std::os::unix::fs::symlink(&target, tmp.path().join("temp-room-tr07")).expect("link");
    let err = delete_temp_room_in(tmp.path(), "temp-room-tr07", &manager())
        .await
        .unwrap_err();
    assert_eq!(err, "delete_root_is_symlink");
    assert!(target.is_dir());
}

#[tokio::test]
async fn delete_refuses_while_a_live_session_runs_inside() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path().join("temp-room-tr01");
    std::fs::create_dir(&dir).expect("dir");
    let mgr = manager();
    mgr.read()
        .await
        .create_session(
            "shell".to_string(),
            Vec::new(),
            dir.to_string_lossy().to_string(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create live session");
    let err = delete_temp_room_in(tmp.path(), "temp-room-tr01", &mgr)
        .await
        .unwrap_err();
    assert!(
        err.contains("Cannot delete while live sessions exist"),
        "{err}"
    );
    assert!(dir.is_dir());
}

#[tokio::test]
async fn delete_removes_an_idle_temp_room() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path().join("temp-room-tr01");
    std::fs::create_dir(&dir).expect("dir");
    std::fs::write(dir.join("notes.txt"), b"x").expect("file");
    delete_temp_room_in(tmp.path(), "temp-room-tr01", &manager())
        .await
        .expect("delete");
    assert!(!dir.exists());
}

#[test]
fn only_a_deleted_outcome_is_success() {
    assert!(delete_outcome_result(WgDeleteOutcome::Deleted).is_ok());
    let partial = delete_outcome_result(WgDeleteOutcome::Partial {
        orphan_path: PathBuf::from("orphan-dir"),
        error: io::Error::other("stuck"),
    })
    .unwrap_err();
    assert!(
        partial.contains("orphan-dir") && partial.contains("stuck"),
        "{partial}"
    );
    let blocked = delete_outcome_result(WgDeleteOutcome::Blocked(io::Error::other("busy")));
    assert_eq!(blocked.unwrap_err(), "Temp room folder is in use: busy");
    let other = delete_outcome_result(WgDeleteOutcome::Other(io::Error::other("denied")));
    assert!(other.unwrap_err().contains("denied"));
}

#[test]
fn dispatch_matches_exactly_the_three_temp_room_names() {
    assert_eq!(
        TEMP_ROOM_COMMANDS,
        ["list_temp_rooms", "create_temp_room", "delete_temp_room"]
    );
}

type Mock = tauri::test::MockRuntime;

fn invoke_with_counter(cmd: &str, body: serde_json::Value) -> (Result<(), String>, usize) {
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&calls);
    let fallback = move |_invoke: tauri::ipc::Invoke<Mock>| {
        counted.fetch_add(1, Ordering::SeqCst);
        false
    };
    let app = tauri::test::mock_builder()
        .manage(manager())
        .invoke_handler(move |invoke| dispatch(invoke, &fallback))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("mock app");
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("webview");
    let response = tauri::test::get_ipc_response(
        &webview,
        tauri::webview::InvokeRequest {
            cmd: cmd.into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: crate::test_support::LOCAL_IPC_ORIGIN.parse().unwrap(),
            body: tauri::ipc::InvokeBody::Json(body),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.to_string(),
        },
    );
    let result = response.map(|_| ()).map_err(|e| e.to_string());
    (result, calls.load(Ordering::SeqCst))
}

#[test]
fn delete_temp_room_is_handled_without_the_fallback() {
    let (result, calls) = invoke_with_counter("delete_temp_room", json!({ "name": "../x" }));
    assert!(result.unwrap_err().contains("Invalid temp room name"));
    assert_eq!(calls, 0);
}

#[test]
fn other_and_look_alike_names_reach_the_fallback_once() {
    for name in [
        "list_sessions",
        "list_temp_room",
        "create_temp_rooms",
        "Delete_temp_room",
    ] {
        let (result, calls) = invoke_with_counter(name, json!({}));
        assert!(result.is_err(), "{name} stays unhandled");
        assert_eq!(calls, 1, "{name} delegated exactly once");
    }
}
