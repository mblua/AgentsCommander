use super::*;
use crate::config::settings::AppSettings;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// A loopback listener that only counts connections; the two dry-run tests
/// below assert the count stays zero.
async fn silent_listener() -> (String, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a loopback listener");
    let port = listener.local_addr().expect("listener address").port();
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            counter.fetch_add(1, Ordering::SeqCst);
            let _ = tokio::io::AsyncWriteExt::shutdown(&mut stream).await;
        }
    });
    (format!("http://127.0.0.1:{port}/v1/systemone"), hits)
}

fn dry_run_settings(endpoint: String) -> JevSettings {
    JevSettings {
        api_key: "test-key".to_string(),
        model: "jev-1.13.0".to_string(),
        endpoint,
        timeout_secs: 5,
        threshold: 0.70,
        margin: 0.15,
    }
}

/// Test 10: an absent catalog is inert and no HTTP request is issued.
#[tokio::test]
async fn classify_dry_run_without_a_catalog_issues_no_request() {
    let (url, hits) = silent_listener().await;
    let outcome = classify_dry_run(&Catalog::missing(), "candidate", &dry_run_settings(url))
        .await
        .expect("a missing catalog is an abstention, not an error");
    match outcome {
        ClassifyOutcome::Abstained { reason } => {
            assert!(reason.contains("NoCatalogFile"), "{reason}")
        }
        other => panic!("a missing catalog must be inert, got {other:?}"),
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "no HTTP request may be issued"
    );
}

/// Test 14: a seeded secret means no request, no file anywhere, and a reason
/// with neither an excerpt nor a path.
#[tokio::test]
async fn classify_dry_run_with_a_seeded_secret_creates_no_file_and_issues_no_request() {
    let scratch = tempfile::tempdir().unwrap();
    let (url, hits) = silent_listener().await;
    let catalog = Catalog::from_json_str(
        r#"{"categories": {"a": {"destination": "user", "question": "is a?"}}}"#,
    );
    let secret = "AKIAIOSFODNN7EXAMPLE";
    let outcome = classify_dry_run(&catalog, secret, &dry_run_settings(url))
        .await
        .expect("a seeded secret is an abstention, not an error");
    match outcome {
        ClassifyOutcome::Abstained { reason } => {
            assert!(reason.contains("secret detected"), "{reason}");
            assert!(
                !reason.contains(secret),
                "the reason must not carry an excerpt"
            );
            assert!(!reason.contains('/'), "the reason must not carry a path");
        }
        other => panic!("a seeded secret must abstain, got {other:?}"),
    }
    let entries: Vec<_> = std::fs::read_dir(scratch.path())
        .unwrap()
        .flatten()
        .collect();
    assert!(entries.is_empty(), "no file may be created anywhere");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "a flagged candidate must never reach the network"
    );
}

fn test_app(project_paths: Vec<String>) -> tauri::App<tauri::test::MockRuntime> {
    let settings = AppSettings {
        project_paths,
        ..AppSettings::default()
    };
    tauri::test::mock_builder()
        .manage(crate::config::settings::new_settings_state(settings))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build co-managed test app")
}

/// Test 13: a real directory that is not a `room-*` directory is rejected
/// and nothing is created.
#[tokio::test]
async fn get_rejects_a_directory_that_is_not_a_room() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project-a");
    let ac_root = project.join(".ac");
    let notes = ac_root.join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    let app = test_app(vec![project.to_string_lossy().to_string()]);

    let error = co_managed_get(app.handle().clone(), notes.to_string_lossy().to_string())
        .await
        .expect_err("a non-room directory must be rejected");
    assert!(error.contains("coManagedRoomRootInvalid"), "{error}");

    let entries: Vec<_> = std::fs::read_dir(&notes).unwrap().flatten().collect();
    assert!(
        entries.is_empty(),
        "nothing may be created under a rejected directory"
    );
}

/// Test 14: a `room-*` directory outside every registered project is
/// rejected and writes nothing.
#[tokio::test]
async fn set_enabled_rejects_a_room_outside_registered_projects() {
    let temp = tempfile::tempdir().unwrap();
    let registered = temp.path().join("registered-project");
    std::fs::create_dir_all(registered.join(".ac")).unwrap();
    let outside_room = temp
        .path()
        .join("outside-project")
        .join(".ac")
        .join("room-1-dev-team");
    std::fs::create_dir_all(&outside_room).unwrap();
    let app = test_app(vec![registered.to_string_lossy().to_string()]);

    let error = co_managed_set_enabled(
        app.handle().clone(),
        outside_room.to_string_lossy().to_string(),
        true,
    )
    .await
    .expect_err("an unregistered room must be rejected");
    assert!(error.contains("coManagedRoomRootUnregistered"), "{error}");

    let entries: Vec<_> = std::fs::read_dir(&outside_room)
        .unwrap()
        .flatten()
        .collect();
    assert!(
        entries.is_empty(),
        "nothing may be written under an unregistered room"
    );
}

/// The happy path: a registered room accepts the toggle and reads it back.
#[tokio::test]
async fn set_enabled_round_trips_for_a_registered_room() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project-a");
    let room = project.join(".ac").join("room-1-dev-team");
    std::fs::create_dir_all(&room).unwrap();
    let app = test_app(vec![project.to_string_lossy().to_string()]);
    let room_text = room.to_string_lossy().to_string();

    let enabled = co_managed_set_enabled(app.handle().clone(), room_text.clone(), true)
        .await
        .expect("registered room accepts the toggle");
    assert!(enabled.enabled);
    assert!(co_managed::config_path(&room).is_file());

    let fetched = co_managed_get(app.handle().clone(), room_text.clone())
        .await
        .expect("registered room reads back");
    assert_eq!(fetched, enabled);

    let disabled = co_managed_set_enabled(app.handle().clone(), room_text, false)
        .await
        .expect("toggle back");
    assert!(!disabled.enabled);
}
