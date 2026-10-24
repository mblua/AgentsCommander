use super::*;
use crate::config::co_managed::OffReason;

/// The collector reads `co_managed_enabled`: a ready room behind a global
/// switch in OFF answers `GlobalSwitchOff`.
#[tokio::test]
async fn global_switch_off_masks_a_ready_room() {
    let fixture = room_fixture();
    configure_room(&fixture.room, true);
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let settings = AppSettings {
        jev_api_key: "test-key".to_string(),
        co_managed_enabled: false,
        ..AppSettings::default()
    };
    let app = test_app(settings, Arc::clone(&manager));
    let projects_dir = fixture._temp.path().join("claude-projects");
    std::fs::create_dir_all(&projects_dir).expect("projects dir");

    let id = add_session(
        &manager,
        &fixture.coordinator,
        SessionBackendKind::LocalProcess,
        CodingAgentKind::Claude,
    )
    .await;
    manager
        .read()
        .await
        .set_resolved_claude_projects_dir(id, Some(projects_dir.clone()))
        .await;

    assert_eq!(
        effective(&app, &fixture.room, id).await,
        CoManagedState::Off {
            reason: OffReason::GlobalSwitchOff
        }
    );
}

#[tokio::test]
async fn orchestrator_answers_ready_and_room_flag_off() {
    let fixture = room_fixture();
    configure_room(&fixture.room, true);
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let app = test_app(settings_with_key(), Arc::clone(&manager));
    let projects_dir = fixture._temp.path().join("claude-projects");
    std::fs::create_dir_all(&projects_dir).expect("projects dir");

    let id = add_session(
        &manager,
        &fixture.coordinator,
        SessionBackendKind::LocalProcess,
        CodingAgentKind::Claude,
    )
    .await;
    manager
        .read()
        .await
        .set_resolved_claude_projects_dir(id, Some(projects_dir.clone()))
        .await;

    assert_eq!(
        effective(&app, &fixture.room, id).await,
        CoManagedState::Ready
    );

    configure_room(&fixture.room, false);
    assert_eq!(
        effective(&app, &fixture.room, id).await,
        CoManagedState::Off {
            reason: OffReason::RoomFlagOff
        }
    );

    // A member session in the same room is never the orchestrator.
    configure_room(&fixture.room, true);
    let member = add_session(
        &manager,
        &fixture.member,
        SessionBackendKind::LocalProcess,
        CodingAgentKind::Claude,
    )
    .await;
    manager
        .read()
        .await
        .set_resolved_claude_projects_dir(member, Some(projects_dir))
        .await;
    assert_eq!(
        effective(&app, &fixture.room, member).await,
        CoManagedState::Off {
            reason: OffReason::NotAnOrchestrator
        }
    );
}

#[tokio::test]
async fn container_sessions_require_their_resolved_homes() {
    let fixture = room_fixture();
    configure_room(&fixture.room, true);
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let app = test_app(settings_with_key(), Arc::clone(&manager));
    let claude_home = fixture._temp.path().join("claude-home");
    let codex_home = fixture._temp.path().join("codex-home");
    std::fs::create_dir_all(&claude_home).expect("claude home");
    std::fs::create_dir_all(&codex_home).expect("codex home");

    let claude = add_session(
        &manager,
        &fixture.coordinator,
        SessionBackendKind::ContainerTransport,
        CodingAgentKind::Claude,
    )
    .await;
    manager
        .read()
        .await
        .set_resolved_claude_projects_dir(claude, Some(claude_home))
        .await;
    assert_eq!(
        effective(&app, &fixture.room, claude).await,
        CoManagedState::Ready
    );

    let claude_bare = add_session(
        &manager,
        &fixture.coordinator,
        SessionBackendKind::ContainerTransport,
        CodingAgentKind::Claude,
    )
    .await;
    manager
        .read()
        .await
        .set_resolved_claude_projects_dir(claude_bare, None)
        .await;
    assert_eq!(
        effective(&app, &fixture.room, claude_bare).await,
        CoManagedState::Off {
            reason: OffReason::UnsupportedProvider {
                agent: "claude".to_string()
            }
        }
    );

    let codex = add_session(
        &manager,
        &fixture.coordinator,
        SessionBackendKind::ContainerTransport,
        CodingAgentKind::Codex,
    )
    .await;
    manager
        .read()
        .await
        .set_profile_metadata(
            codex,
            None,
            None,
            Vec::new(),
            false,
            None,
            None,
            Some(codex_home.to_string_lossy().to_string()),
            None,
        )
        .await;
    assert_eq!(
        effective(&app, &fixture.room, codex).await,
        CoManagedState::Ready
    );

    let codex_bare = add_session(
        &manager,
        &fixture.coordinator,
        SessionBackendKind::ContainerTransport,
        CodingAgentKind::Codex,
    )
    .await;
    assert_eq!(
        effective(&app, &fixture.room, codex_bare).await,
        CoManagedState::Off {
            reason: OffReason::UnsupportedProvider {
                agent: "codex".to_string()
            }
        }
    );
}

#[tokio::test]
async fn missing_session_is_an_error() {
    let fixture = room_fixture();
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let app = test_app(settings_with_key(), Arc::clone(&manager));

    let error = co_managed_effective_state_for_session(
        app.handle(),
        &fixture.room,
        &Uuid::new_v4().to_string(),
    )
    .await
    .expect_err("a missing session must not answer Ready");
    assert!(error.contains("coManagedSessionNotFound"), "{error}");
}
