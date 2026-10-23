use super::*;

// #2434 - restart and drift read one matched selection.

fn route_agent(id: &str, label: &str, command: &str) -> AgentConfig {
    AgentConfig {
        preflight: None,
        preflight_timeout_seconds: None,
        id: id.to_string(),
        id_hash: None,
        id_hash_debug: None,
        label: label.to_string(),
        command: command.to_string(),
        color: "#10b981".to_string(),
        order: None,
        envs: Vec::new(),
        isolated_home: false,
        instructions_filename: None,
        config_seed: None,
        context_regex: None,
        blocking_menus: None,
        backend: Default::default(),
    }
}

/// One local `codex` agent with enabled cells `A` (`--a`) and `B` (`--b`).
fn route_settings() -> AppSettings {
    let mut settings = AppSettings {
        agents: vec![route_agent("codex", "Codex", "codex")],
        ..AppSettings::default()
    };
    for (letter, command) in [("A", "--a"), ("B", "--b")] {
        settings
            .coding_agent_profiles
            .profiles_by_agent
            .entry("codex".to_string())
            .or_default()
            .insert(
                letter.to_string(),
                ProfileCellConfig {
                    id_hash: None,
                    id_hash_debug: None,
                    preflight: None,
                    preflight_timeout_seconds: None,
                    enabled: true,
                    command: command.to_string(),
                    env: BTreeMap::new(),
                    notes: String::new(),
                },
            );
    }
    settings
}

fn route_digest(settings: &AppSettings, letter: &str) -> String {
    crate::config::agent_command::cell_identity(
        &settings.agents[0],
        &settings.coding_agent_profiles.profiles_by_agent["codex"][letter],
    )
}

/// Merge `tooling` into `dir/config.json`, keeping every other key.
fn merge_tooling(dir: &std::path::Path, tooling: serde_json::Value) {
    let path = dir.join("config.json");
    let mut value: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let entry = value
        .as_object_mut()
        .unwrap()
        .entry("tooling")
        .or_insert_with(|| serde_json::json!({}));
    for (key, v) in tooling.as_object().unwrap() {
        entry
            .as_object_mut()
            .unwrap()
            .insert(key.clone(), v.clone());
    }
    std::fs::write(&path, value.to_string()).unwrap();
}

struct RouteHarness {
    app: tauri::App<tauri::test::MockRuntime>,
    manager: Arc<tokio::sync::RwLock<SessionManager>>,
    pty: Arc<Mutex<crate::pty::manager::PtyManager>>,
    backend: Arc<ScriptedSpawnBackend>,
}

impl RouteHarness {
    fn new(settings: AppSettings) -> Self {
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let backend = Arc::new(ScriptedSpawnBackend::default());
        let pty = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
            backend.clone(),
        )));
        let app = session_test_app(settings, Arc::clone(&manager), Arc::clone(&pty));
        Self {
            app,
            manager,
            pty,
            backend,
        }
    }

    /// A live session recorded with `agent_id` and the saved `shell`/`args`.
    async fn live(
        &self,
        cwd: &std::path::Path,
        agent_id: Option<&str>,
        shell: &str,
        args: &[&str],
    ) -> Uuid {
        let info = super::super::create_session_inner(
            self.app.handle(),
            &self.manager,
            &self.pty,
            shell.to_string(),
            args.iter().map(|a| a.to_string()).collect(),
            cwd.to_string_lossy().into_owned(),
            Some("route fixture".to_string()),
            agent_id.map(str::to_string),
            None,
            true,
            Vec::new(),
            true,
            None,
            None,
            None,
            CreateSelectionIntent::User,
        )
        .await
        .expect("create route fixture session");
        Uuid::parse_str(&info.id).unwrap()
    }

    async fn restart(
        &self,
        id: Uuid,
        requested_profile: Option<&str>,
    ) -> Result<SessionInfo, String> {
        let settings = self.app.state::<crate::config::settings::SettingsState>();
        super::super::restart_session_inner_with_intent(
            self.app.handle(),
            &self.manager,
            &self.pty,
            settings.inner(),
            id,
            None,
            requested_profile.map(str::to_string),
            Some(true),
            true,
            crate::session::selection::TrustedRestartIntent::User,
            None,
            crate::config::sessions_persistence::default_creation_gate_enforcement(),
        )
        .await
    }

    async fn close(self) {
        close_test_coordinator(&self.app).await;
    }
}

/// A foreign `currentCodingAgent` whose descriptor names local cell `B` as `A`.
fn moved_letter_reference(dir: &std::path::Path, settings: &AppSettings) {
    merge_tooling(
        dir,
        serde_json::json!({
            "profile": "A",
            "currentCodingAgent": "their-codex",
            "codingAgents": {"their-codex": {
                "app": "Theirs",
                "identity": {"A": route_digest(settings, "B")}
            }}
        }),
    );
}

#[tokio::test]
async fn restart_spawns_the_matched_letter_over_a_pinned_replica_letter() {
    let fixture = super::super::co_managed_tests::room_fixture();
    let cwd = fixture.coordinator_path();
    let settings = route_settings();
    moved_letter_reference(cwd, &settings);
    let h = RouteHarness::new(settings);
    let old = h.live(cwd, Some("codex"), "codex", &[]).await;

    let restarted = h.restart(old, Some("A")).await.expect("restart");
    assert_eq!(restarted.agent_id.as_deref(), Some("codex"));
    assert!(
        restarted.shell_args.iter().any(|a| a == "--b"),
        "{:?}",
        restarted.shell_args
    );
    assert!(
        !restarted.shell_args.iter().any(|a| a == "--a"),
        "{:?}",
        restarted.shell_args
    );
    assert_eq!(restarted.effective_profile.as_deref(), Some("B"));
    h.close().await;
}

#[tokio::test]
async fn restart_populates_the_match_carrier_fields() {
    let fixture = super::super::co_managed_tests::room_fixture();
    let cwd = fixture.coordinator_path();
    let settings = route_settings();
    moved_letter_reference(cwd, &settings);
    let h = RouteHarness::new(settings);
    let old = h.live(cwd, Some("codex"), "codex", &[]).await;

    let restarted = h.restart(old, Some("A")).await.expect("restart");
    assert_eq!(restarted.match_tier.as_deref(), Some("hash"));
    assert_eq!(restarted.original_profile_letter.as_deref(), Some("A"));
    let wire = serde_json::to_value(&restarted).unwrap();
    assert_eq!(wire["matchTier"], "hash");
    assert_eq!(wire["originalProfileLetter"], "A");
    h.close().await;
}

#[tokio::test]
async fn a_matched_restart_records_the_original_request() {
    let fixture = super::super::co_managed_tests::room_fixture();
    let cwd = fixture.coordinator_path();
    let settings = route_settings();
    moved_letter_reference(cwd, &settings);
    let h = RouteHarness::new(settings);
    let old = h.live(cwd, Some("codex"), "codex", &[]).await;

    let restarted = h.restart(old, Some("A")).await.expect("restart");
    assert_eq!(restarted.requested_profile.as_deref(), Some("A"));
    assert_eq!(restarted.effective_profile.as_deref(), Some("B"));
    assert_eq!(restarted.original_profile_letter.as_deref(), Some("A"));
    h.close().await;
}

#[tokio::test]
async fn drift_mirrors_a_moved_letter() {
    let fixture = super::super::co_managed_tests::room_fixture();
    let cwd = fixture.coordinator_path();
    let settings = route_settings();
    moved_letter_reference(cwd, &settings);
    let h = RouteHarness::new(settings.clone());
    let old = h.live(cwd, Some("codex"), "codex", &[]).await;

    let restarted = h.restart(old, Some("A")).await.expect("restart");
    assert_eq!(
        restarted.profile_content_hash.as_deref(),
        Some(route_digest(&settings, "B").as_str())
    );
    assert!(
        !compute_profile_outdated(&settings, &restarted),
        "a matched B must not raise a drift badge against the pinned A"
    );
    h.close().await;
}

#[tokio::test]
async fn restart_with_no_coding_agent_restarts_the_saved_shell() {
    let temp = tempfile::tempdir().unwrap();
    let h = RouteHarness::new(AppSettings::default());
    let old = h.live(temp.path(), None, "test-shell", &["--keep"]).await;

    let restarted = h.restart(old, None).await.expect("plain shell restart");
    assert_eq!(restarted.shell, "test-shell");
    assert_eq!(restarted.shell_args, vec!["--keep".to_string()]);
    assert_eq!(restarted.agent_id, None);
    assert_eq!(restarted.match_tier, None);
    h.close().await;
}

#[tokio::test]
async fn restart_returns_an_error_on_no_match() {
    let temp = tempfile::tempdir().unwrap();
    merge_tooling(
        temp.path(),
        serde_json::json!({
            "currentCodingAgent": "foreign",
            "codingAgents": {"foreign": {"app": "Gemini", "command": "gemini"}}
        }),
    );
    let h = RouteHarness::new(route_settings());
    // C19: actual completed missing-UUID entries release their root capacity.
    // This exceeds the 64-entry admission bound through the real outer core,
    // worker response, sidecar/envelope release and independent publication.
    for _ in 0..65 {
        assert_eq!(
            h.restart(Uuid::new_v4(), None).await.unwrap_err(),
            "Session not found"
        );
    }
    let old = h.live(temp.path(), Some("codex"), "codex", &[]).await;

    // Actual prerequisite errors also release completed C19 entries. If
    // they remained ambiguous, this would exhaust the same 64-entry root.
    for _ in 0..65 {
        let err = h.restart(old, None).await.unwrap_err();
        assert!(err.contains("unresolved_coding_agent_reference"), "{err}");
        assert!(err.contains("'foreign'"), "{err}");
    }
    // No fallback to the stored `codex`, and no teardown.
    let row = h
        .manager
        .read()
        .await
        .get_session(old)
        .await
        .expect("row survives");
    assert!(!matches!(row.status, SessionStatus::Exited(_)));
    assert!(h.backend.has_session(old), "the old PTY is untouched");
    assert_eq!(h.manager.read().await.list_sessions().await.len(), 1);
    h.close().await;
}

#[test]
fn drift_is_false_for_an_unresolved_reference() {
    let temp = tempfile::tempdir().unwrap();
    let settings = route_settings();
    let mut info = make_info(
        &temp.path().to_string_lossy(),
        Some("codex"),
        None,
        Some("stale".to_string()),
    );
    // Control: with no foreign reference, a stale loaded hash IS drift.
    assert!(compute_profile_outdated(&settings, &info));
    merge_tooling(
        temp.path(),
        serde_json::json!({
            "currentCodingAgent": "foreign",
            "codingAgents": {"foreign": {"app": "Gemini"}}
        }),
    );
    assert!(!compute_profile_outdated(&settings, &info));
    // `NoAgent` raises none either.
    info.agent_id = None;
    assert!(!compute_profile_outdated(&settings, &info));
}

#[test]
fn root_agent_chain_is_unchanged() {
    // D5: the root agent keeps its id-only chain; a descriptor-matchable
    // foreign id is NOT routed through the matcher.
    let settings = test_settings();
    let (_, _, agent_id, _) =
        resolve_root_agent_command(&settings, Some("their-codex"), Some("codex")).unwrap();
    assert_eq!(
        agent_id.as_deref(),
        Some("codex"),
        "falls to lastCodingAgent"
    );
    let (_, _, agent_id, _) =
        resolve_root_agent_command(&settings, Some("their-codex"), Some("gone")).unwrap();
    assert_eq!(
        agent_id.as_deref(),
        Some("claude"),
        "then to the first agent"
    );
    let (_, _, agent_id, _) = resolve_root_agent_command(&settings, Some("claude"), None).unwrap();
    assert_eq!(agent_id.as_deref(), Some("claude"), "a valid request first");
}

fn isolated_codex_settings(id: &str) -> (AppSettings, PathBuf) {
    let mut agent = route_agent(id, "Codex", "codex");
    agent.isolated_home = true;
    let settings = AppSettings {
        agents: vec![agent],
        ..AppSettings::default()
    };
    let home =
        crate::config::agent_command::resolve_agent_spawn_command(&settings, id, None, None, false)
            .unwrap()
            .effective_codex_home
            .expect("isolated CODEX_HOME");
    (settings, home)
}

#[tokio::test]
async fn restart_prepares_the_spawn_before_teardown() {
    let id = format!("codex-2434-{}", Uuid::new_v4());
    let (settings, home) = isolated_codex_settings(&id);
    let _ = std::fs::remove_dir_all(&home);
    let temp = tempfile::tempdir().unwrap();
    let h = RouteHarness::new(settings);
    let old = h.live(temp.path(), Some(&id), "codex", &[]).await;
    assert!(!home.exists());

    h.restart(old, None).await.expect("restart");
    assert!(
        home.is_dir(),
        "the restart prepared the isolated CODEX_HOME"
    );
    std::fs::remove_dir_all(&home).unwrap();
    h.close().await;
}

#[tokio::test]
async fn a_failed_preparation_keeps_the_old_session() {
    let id = format!("codex-2434-{}", Uuid::new_v4());
    let (settings, home) = isolated_codex_settings(&id);
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(home.parent().unwrap()).unwrap();
    // A regular FILE at the directory path: `create_dir_all` fails for real.
    std::fs::write(&home, "not a directory").unwrap();
    let temp = tempfile::tempdir().unwrap();
    let h = RouteHarness::new(settings);
    let old = h.live(temp.path(), Some(&id), "codex", &[]).await;

    let err = h.restart(old, None).await.unwrap_err();
    assert!(
        err.contains("Failed to create isolated CODEX_HOME"),
        "{err}"
    );
    let row = h
        .manager
        .read()
        .await
        .get_session(old)
        .await
        .expect("row survives");
    assert!(!matches!(row.status, SessionStatus::Exited(_)));
    assert!(h.backend.has_session(old), "never torn down");
    assert_eq!(h.manager.read().await.list_sessions().await.len(), 1);
    std::fs::remove_file(&home).unwrap();
    h.close().await;
}

#[tokio::test]
async fn tier_zero_without_a_request_keeps_the_origin_default() {
    // Origin default `B` (the replica's matrix), then an agent-level default.
    for use_origin in [true, false] {
        let fixture = super::super::co_managed_tests::room_fixture();
        let cwd = fixture.coordinator_path();
        let mut settings = route_settings();
        if use_origin {
            let matrix = cwd
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("_agent_coordinator");
            merge_tooling(&matrix, serde_json::json!({"defaultProfile": "B"}));
        } else {
            settings
                .coding_agent_profiles
                .default_profile_by_agent
                .insert("coordinator".to_string(), "B".to_string());
        }
        assert!(matches!(
            super::super::matched_selection(
                &settings,
                &cwd.to_string_lossy(),
                None,
                Some("codex"),
                None
            ),
            super::super::MatchedSelection::Agent {
                profile_letter: None,
                authoritative: false,
                tier: None,
                ..
            }
        ));
        let h = RouteHarness::new(settings.clone());
        let old = h.live(cwd, Some("codex"), "codex", &[]).await;
        let restarted = h.restart(old, None).await.expect("restart");
        assert_eq!(
            restarted.effective_profile.as_deref(),
            Some("B"),
            "origin={use_origin}"
        );
        assert!(restarted.shell_args.iter().any(|a| a == "--b"));
        assert!(
            !compute_profile_outdated(&settings, &restarted),
            "drift agrees"
        );
        h.close().await;
    }
}

#[tokio::test]
async fn a_matched_reference_without_a_request_keeps_the_default() {
    let label = serde_json::json!({"app": "CODEX"});
    let command = serde_json::json!({"app": "Other", "command": "codex"});
    for (descriptor, tier) in [
        (
            label,
            crate::config::agent_command::MatchTier::LabelAndLetter,
        ),
        (
            command,
            crate::config::agent_command::MatchTier::CommandAndLetter,
        ),
    ] {
        let fixture = super::super::co_managed_tests::room_fixture();
        let cwd = fixture.coordinator_path();
        let mut settings = route_settings();
        settings
            .coding_agent_profiles
            .default_profile_by_agent
            .insert("coordinator".to_string(), "B".to_string());
        merge_tooling(
            cwd,
            serde_json::json!({
                "currentCodingAgent": "foreign",
                "codingAgents": {"foreign": descriptor}
            }),
        );
        assert_eq!(
            super::super::matched_selection(
                &settings,
                &cwd.to_string_lossy(),
                None,
                Some("codex"),
                None
            ),
            super::super::MatchedSelection::Agent {
                agent_id: "codex".to_string(),
                profile_letter: None,
                authoritative: false,
                tier: Some(tier),
                original_letter: None,
            }
        );
        let h = RouteHarness::new(settings.clone());
        let old = h.live(cwd, Some("codex"), "codex", &[]).await;
        let restarted = h.restart(old, None).await.expect("restart");
        assert_eq!(
            restarted.effective_profile.as_deref(),
            Some("B"),
            "{tier:?}"
        );
        assert_eq!(restarted.match_tier.as_deref(), Some(tier.as_wire_str()));
        assert!(
            !compute_profile_outdated(&settings, &restarted),
            "drift agrees"
        );
        h.close().await;
    }
}

fn selection_for(
    tooling: serde_json::Value,
    requested: Option<&str>,
    stored: Option<&str>,
) -> (tempfile::TempDir, super::super::MatchedSelection) {
    let temp = tempfile::tempdir().unwrap();
    merge_tooling(temp.path(), tooling);
    let selection = super::super::matched_selection(
        &route_settings(),
        &temp.path().to_string_lossy(),
        requested,
        stored,
        None,
    );
    (temp, selection)
}

async fn assert_saved_shell_restart(current: &str) {
    let temp = tempfile::tempdir().unwrap();
    merge_tooling(
        temp.path(),
        serde_json::json!({"currentCodingAgent": current}),
    );
    let h = RouteHarness::new(route_settings());
    let old = h.live(temp.path(), None, "test-shell", &["--keep"]).await;
    let restarted = h.restart(old, None).await.expect("plain shell restart");
    assert_eq!(restarted.shell, "test-shell");
    assert_eq!(restarted.shell_args, vec!["--keep".to_string()]);
    assert_eq!(restarted.agent_id, None);
    h.close().await;
}

#[tokio::test]
async fn an_empty_current_coding_agent_restarts_the_saved_shell() {
    let (_t, selection) = selection_for(serde_json::json!({"currentCodingAgent": ""}), None, None);
    assert_eq!(selection, super::super::MatchedSelection::NoAgent);
    assert_saved_shell_restart("").await;
}

#[tokio::test]
async fn a_whitespace_current_coding_agent_is_absent() {
    let (_t, selection) =
        selection_for(serde_json::json!({"currentCodingAgent": "   "}), None, None);
    assert_eq!(selection, super::super::MatchedSelection::NoAgent);
    // With a stored agent the blank id must not become a reference either.
    let (_t, selection) = selection_for(
        serde_json::json!({"currentCodingAgent": "   "}),
        None,
        Some("codex"),
    );
    assert!(
        matches!(
            selection,
            super::super::MatchedSelection::Agent { ref agent_id, .. } if agent_id == "codex"
        ),
        "{selection:?}"
    );
    assert_saved_shell_restart("   ").await;
}

#[test]
fn an_empty_current_coding_agent_is_absent_even_with_a_stored_agent() {
    let (_t, selection) = selection_for(
        serde_json::json!({"currentCodingAgent": ""}),
        None,
        Some("codex"),
    );
    assert!(
        matches!(
            selection,
            super::super::MatchedSelection::Agent { ref agent_id, tier: None, .. } if agent_id == "codex"
        ),
        "{selection:?}"
    );
}

#[tokio::test]
async fn a_plain_shell_ignores_an_unresolvable_current_coding_agent() {
    let (_t, selection) = selection_for(
        serde_json::json!({"currentCodingAgent": "ghost-agent"}),
        None,
        None,
    );
    assert_eq!(selection, super::super::MatchedSelection::NoAgent);
    assert_saved_shell_restart("ghost-agent").await;
}

#[tokio::test]
async fn an_unresolvable_reference_still_fails_a_session_that_has_an_agent() {
    let (_t, selection) = selection_for(
        serde_json::json!({"currentCodingAgent": "ghost-agent"}),
        None,
        Some("codex"),
    );
    assert_eq!(
        selection,
        super::super::MatchedSelection::Unresolved {
            reference: "ghost-agent".to_string()
        }
    );
    let temp = tempfile::tempdir().unwrap();
    merge_tooling(
        temp.path(),
        serde_json::json!({"currentCodingAgent": "ghost-agent"}),
    );
    let h = RouteHarness::new(route_settings());
    let old = h.live(temp.path(), Some("codex"), "codex", &[]).await;
    let err = h.restart(old, None).await.unwrap_err();
    assert!(err.contains("unresolved_coding_agent_reference"), "{err}");
    assert!(h.backend.has_session(old), "no teardown");
    h.close().await;
}

#[tokio::test]
async fn an_explicit_unresolvable_request_fails_without_a_stored_agent() {
    let (_t, selection) = selection_for(serde_json::json!({}), Some("ghost-agent"), None);
    assert_eq!(
        selection,
        super::super::MatchedSelection::Unresolved {
            reference: "ghost-agent".to_string()
        }
    );
    let temp = tempfile::tempdir().unwrap();
    let h = RouteHarness::new(route_settings());
    let old = h.live(temp.path(), None, "test-shell", &[]).await;
    let settings = h.app.state::<crate::config::settings::SettingsState>();
    let err = super::super::restart_session_inner_with_intent(
        h.app.handle(),
        &h.manager,
        &h.pty,
        settings.inner(),
        old,
        Some("ghost-agent".to_string()),
        None,
        Some(true),
        true,
        crate::session::selection::TrustedRestartIntent::User,
        None,
        crate::config::sessions_persistence::default_creation_gate_enforcement(),
    )
    .await
    .unwrap_err();
    assert!(err.contains("unresolved_coding_agent_reference"), "{err}");
    assert!(h.backend.has_session(old), "no teardown");
    h.close().await;
}

#[test]
fn drift_is_false_for_a_plain_shell_with_a_stale_reference() {
    let temp = tempfile::tempdir().unwrap();
    merge_tooling(
        temp.path(),
        serde_json::json!({"currentCodingAgent": "ghost-agent"}),
    );
    let info = make_info(
        &temp.path().to_string_lossy(),
        None,
        None,
        Some("stale".to_string()),
    );
    assert!(!compute_profile_outdated(&route_settings(), &info));
    assert_eq!(
        super::super::matched_selection(
            &route_settings(),
            &temp.path().to_string_lossy(),
            None,
            None,
            None
        ),
        super::super::MatchedSelection::NoAgent
    );
}
