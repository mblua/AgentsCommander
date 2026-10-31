use super::*;
use tauri::Manager;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn remote_menus_fixture() -> Vec<u8> {
    serde_json::json!({
        "schemaVersion": 1,
        "byCommand": {
            "grok": [{
                "pattern": r"^\s*Grok test menu\?",
                "notification": "grok is waiting for you to answer the test menu",
                "enabled": true
            }]
        },
        "byAgent": {}
    })
    .to_string()
    .into_bytes()
}

fn remote_menus_seed() -> Vec<u8> {
    serde_json::to_vec_pretty(&serde_json::json!({
        "schemaVersion": 1,
        "byCommand": {
            "grok": [{
                "pattern": r"^\s*Grok test menu\?",
                "notification": "seed G1",
                "enabled": true
            }]
        },
        "byAgent": {}
    }))
    .expect("the seed serializes")
}

fn seeded_grok() -> crate::config::settings::BlockingMenuEntry {
    crate::config::settings::BlockingMenuEntry::Valid(crate::config::settings::BlockingMenuConfig {
        pattern: r"^\s*Grok test menu\?".to_string(),
        notification: "seed G1".to_string(),
        enabled: true,
        captured_against: None,
    })
}

fn test_app(
    settings: crate::config::settings::AppSettings,
) -> tauri::App<tauri::test::MockRuntime> {
    let state: crate::config::settings::SettingsState =
        crate::config::settings::new_settings_state(settings);
    tauri::test::mock_builder()
        .manage(state)
        .manage(crate::network::OutboundNetwork::new_for_tests(4))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build a remote blocking-menus test app")
}

/// One HTTP/1.1 response, then close. Every accept/read/write/shutdown error is
/// ignored and the task never panics, so a client that stops reading at the cap
/// (and resets the socket) cannot fail or flake the test.
pub(super) async fn serve_once(status: u16, body: Vec<u8>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a loopback listener");
    let port = listener.local_addr().expect("listener address").port();
    tokio::spawn(async move {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            let Ok(read) = stream.read(&mut buffer).await else {
                return;
            };
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        let head = format!(
            "HTTP/1.1 {status} Status\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(head.as_bytes()).await;
        let _ = stream.write_all(&body).await;
        let _ = stream.shutdown().await;
    });
    format!(
        "http://127.0.0.1:{port}/remote-resources/blocking-menus/v1/\
         settings-blocking-menus.json"
    )
}

pub(super) async fn offline_url() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a loopback listener");
    let port = listener.local_addr().expect("listener address").port();
    drop(listener);
    format!(
        "http://127.0.0.1:{port}/remote-resources/blocking-menus/v1/\
         settings-blocking-menus.json"
    )
}

#[tokio::test]
async fn disabled_makes_no_request_and_writes_nothing() {
    let temp = tempfile::TempDir::new().unwrap();
    let settings_path = temp.path().join("settings.json");
    let network = crate::network::OutboundNetwork::new_for_tests(4);
    let now = "2026-09-12T12:00:00Z".parse::<DateTime<Utc>>().unwrap();

    let outcome = run_remote_blocking_menus_check(
        &network,
        &settings_path,
        false,
        crate::config::settings::REMOTE_BLOCKING_MENUS_URL,
        now,
    )
    .await;

    assert_eq!(outcome, RemoteMenusCheck::Disabled);
    assert!(network.acquired_labels_for_tests().is_empty());
    assert!(!crate::config::settings::blocking_menus_remote_check_path(&settings_path).exists());
    assert!(!crate::config::settings::blocking_menus_remote_path(&settings_path).exists());
}

#[tokio::test]
async fn a_recent_stamp_makes_no_request() {
    let temp = tempfile::TempDir::new().unwrap();
    let settings_path = temp.path().join("settings.json");
    let network = crate::network::OutboundNetwork::new_for_tests(4);
    let now = "2026-09-12T12:00:00Z".parse::<DateTime<Utc>>().unwrap();

    crate::config::settings::write_remote_blocking_menus_check_stamp(
        &settings_path,
        now - chrono::Duration::hours(1),
    )
    .unwrap();
    let outcome = run_remote_blocking_menus_check(
        &network,
        &settings_path,
        true,
        crate::config::settings::REMOTE_BLOCKING_MENUS_URL,
        now,
    )
    .await;
    assert_eq!(outcome, RemoteMenusCheck::NotDue);
    assert!(network.acquired_labels_for_tests().is_empty());

    // A stamp in the future (clock moved backwards) is due again.
    crate::config::settings::write_remote_blocking_menus_check_stamp(
        &settings_path,
        now + chrono::Duration::hours(1),
    )
    .unwrap();
    let url = offline_url().await;
    let outcome = run_remote_blocking_menus_check(&network, &settings_path, true, &url, now).await;
    assert!(
        matches!(outcome, RemoteMenusCheck::Unreachable(_)),
        "got {outcome:?}"
    );
    assert_eq!(
        network.acquired_labels_for_tests(),
        vec!["update_check.remote_blocking_menus"]
    );
    assert_eq!(
        crate::config::settings::read_remote_blocking_menus_check_stamp(&settings_path),
        Some(now)
    );
}

#[tokio::test]
async fn an_accepted_download_is_cached_and_applies_at_the_next_start() {
    let temp = tempfile::TempDir::new().unwrap();
    let settings_path = temp.path().join("settings.json");
    let network = crate::network::OutboundNetwork::new_for_tests(4);
    let now = "2026-09-12T12:00:00Z".parse::<DateTime<Utc>>().unwrap();

    let store_before =
        crate::config::settings::BlockingMenusStore::load_from_settings_path(&settings_path);

    let url = serve_once(200, remote_menus_fixture()).await;
    let outcome = run_remote_blocking_menus_check(&network, &settings_path, true, &url, now).await;

    assert_eq!(outcome, RemoteMenusCheck::Accepted);
    assert_eq!(
        network.acquired_labels_for_tests(),
        vec!["update_check.remote_blocking_menus"]
    );

    let cache_bytes = std::fs::read(crate::config::settings::blocking_menus_remote_path(
        &settings_path,
    ))
    .expect("the accepted download writes the cache");
    let note = serde_json::from_slice::<serde_json::Value>(&cache_bytes).unwrap()["note"]
        .as_str()
        .expect("the written note is a string")
        .to_string();
    assert!(
        note.contains(&url),
        "note should carry the runtime URL: {note}"
    );
    assert!(
        note.contains("(source ref main)"),
        "note should carry the source ref: {note}"
    );
    assert!(
        note.contains("2026-09-12T12:00:00Z"),
        "note should carry the fetched time: {note}"
    );
    assert_eq!(
        crate::config::settings::read_remote_blocking_menus_check_stamp(&settings_path),
        Some(now)
    );

    // The store built before the download never sees the new file...
    assert_eq!(store_before.resolve("grok-1", "grok"), Vec::new());
    // ...and a store built afterwards does, at the next start.
    let store_after =
        crate::config::settings::BlockingMenusStore::load_from_settings_path(&settings_path);
    let expected = crate::config::settings::BlockingMenuEntry::Valid(
        crate::config::settings::BlockingMenuConfig {
            pattern: r"^\s*Grok test menu\?".to_string(),
            notification: "grok is waiting for you to answer the test menu".to_string(),
            enabled: true,
            captured_against: None,
        },
    );
    assert_eq!(store_after.resolve("grok-1", "grok"), vec![expected]);
}

enum RemoteMenusReply {
    Offline,
    Http(u16, Vec<u8>),
}

#[tokio::test]
async fn every_failure_keeps_the_previous_cache() {
    let now = "2026-09-12T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
    let invalid_pattern = serde_json::json!({
        "schemaVersion": 1,
        "byCommand": {"grok": [{
            "pattern": ".*",
            "notification": "any",
            "enabled": true
        }]},
        "byAgent": {}
    })
    .to_string()
    .into_bytes();
    let non_empty_by_agent = serde_json::json!({
        "schemaVersion": 1,
        "byCommand": {},
        "byAgent": {"grok-1": []}
    })
    .to_string()
    .into_bytes();
    let wrong_schema = serde_json::json!({
        "schemaVersion": 2,
        "byCommand": {},
        "byAgent": {}
    })
    .to_string()
    .into_bytes();

    let cases: Vec<(&str, RemoteMenusReply, &str)> = vec![
        ("offline", RemoteMenusReply::Offline, "unreachable"),
        (
            "404",
            RemoteMenusReply::Http(404, b"nope".to_vec()),
            "HTTP status 404",
        ),
        (
            "malformed JSON",
            RemoteMenusReply::Http(200, b"{".to_vec()),
            "does not parse",
        ),
        (
            "oversized body",
            RemoteMenusReply::Http(
                200,
                vec![b' '; crate::config::settings::REMOTE_BLOCKING_MENUS_MAX_BYTES + 1],
            ),
            "larger than",
        ),
        (
            "empty-string pattern",
            RemoteMenusReply::Http(200, invalid_pattern),
            "matches the empty string",
        ),
        (
            "non-empty byAgent",
            RemoteMenusReply::Http(200, non_empty_by_agent),
            "byAgent must be empty",
        ),
        (
            "schemaVersion 2",
            RemoteMenusReply::Http(200, wrong_schema),
            "schemaVersion",
        ),
    ];

    for (label, reply, expected) in cases {
        let temp = tempfile::TempDir::new().unwrap();
        let settings_path = temp.path().join("settings.json");
        let network = crate::network::OutboundNetwork::new_for_tests(4);
        let seed = remote_menus_seed();
        std::fs::write(
            crate::config::settings::blocking_menus_remote_path(&settings_path),
            &seed,
        )
        .unwrap();

        let url = match reply {
            RemoteMenusReply::Offline => offline_url().await,
            RemoteMenusReply::Http(status, body) => serve_once(status, body).await,
        };
        let outcome =
            run_remote_blocking_menus_check(&network, &settings_path, true, &url, now).await;

        match (label, outcome) {
            ("offline", RemoteMenusCheck::Unreachable(_)) => {}
            (_, RemoteMenusCheck::Rejected(reason)) => {
                assert!(
                    reason.contains(expected),
                    "{label}: expected {expected:?} in {reason:?}"
                );
            }
            (_, other) => panic!("{label}: expected a failure, got {other:?}"),
        }

        assert_eq!(
            std::fs::read(crate::config::settings::blocking_menus_remote_path(
                &settings_path
            ))
            .unwrap(),
            seed,
            "{label}: the previous cache must stay byte-equal"
        );
        let store =
            crate::config::settings::BlockingMenusStore::load_from_settings_path(&settings_path);
        assert_eq!(
            store.resolve("grok-1", "grok"),
            vec![seeded_grok()],
            "{label}: the previous cache still resolves"
        );
        assert_eq!(
            crate::config::settings::read_remote_blocking_menus_check_stamp(&settings_path),
            Some(now),
            "{label}: the failed attempt still stamps now"
        );
    }
}

#[tokio::test]
async fn the_startup_path_reads_the_remote_flag_not_the_npm_flag() {
    let now = "2026-09-12T12:00:00Z".parse::<DateTime<Utc>>().unwrap();

    // Case A: the remote flag is off even though the npm flag is on.
    let temp = tempfile::TempDir::new().unwrap();
    let settings_path = temp.path().join("settings.json");
    let app = test_app(crate::config::settings::AppSettings {
        remote_blocking_menus_enabled: false,
        npm_update_notifications_enabled: true,
        ..Default::default()
    });
    let url = offline_url().await;
    let outcome = run_remote_blocking_menus_for_app(app.handle(), &settings_path, &url, now).await;
    assert_eq!(outcome, RemoteMenusCheck::Disabled);
    assert!(app
        .state::<crate::network::OutboundNetwork>()
        .acquired_labels_for_tests()
        .is_empty());
    assert!(!crate::config::settings::blocking_menus_remote_check_path(&settings_path).exists());
    assert!(!crate::config::settings::blocking_menus_remote_path(&settings_path).exists());

    // Case B, the control: the remote flag is on even though the npm flag is off.
    let temp = tempfile::TempDir::new().unwrap();
    let settings_path = temp.path().join("settings.json");
    let app = test_app(crate::config::settings::AppSettings {
        remote_blocking_menus_enabled: true,
        npm_update_notifications_enabled: false,
        ..Default::default()
    });
    let url = serve_once(200, remote_menus_fixture()).await;
    let outcome = run_remote_blocking_menus_for_app(app.handle(), &settings_path, &url, now).await;
    assert_eq!(outcome, RemoteMenusCheck::Accepted);
    assert_eq!(
        app.state::<crate::network::OutboundNetwork>()
            .acquired_labels_for_tests(),
        vec!["update_check.remote_blocking_menus"]
    );
}
