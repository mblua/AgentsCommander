use super::*;

#[test]
fn settings_snapshot_exposes_overlay_owns_agents_false_and_not_in_base_settings() {
    let settings = settings_with_agents(&["a", "b"]);

    let snapshot = settings_snapshot_from(&settings, None);
    assert!(!snapshot.overlay_owns_agents);
    let wire = serde_json::to_value(&snapshot).unwrap();
    assert_eq!(wire["overlayOwnsAgents"], json!(false));
    // Read-only disclosure: the base settings object never carries the key.
    let base = serde_json::to_value(&settings).unwrap();
    assert!(base.get("overlayOwnsAgents").is_none());
}

#[test]
fn settings_snapshot_reports_overlay_owned_agents_with_overlay_precedence() {
    let temp = tempfile::tempdir().unwrap();
    let path = seed_overlay_agents_fixture(
        temp.path(),
        &json!([
            { "id": "ov-a", "label": "A", "command": "claude", "color": "#111111", "order": 2 },
            { "id": "ov-b", "label": "B", "command": "claude", "color": "#222222" },
            { "id": "ov-c", "label": "C", "command": "claude", "color": "#333333", "order": -1 },
            { "id": "ov-d", "label": "D", "command": "claude", "color": "#444444", "order": "two" },
            { "id": "ov-e", "label": "E", "command": "claude", "color": "#555555", "order": 1 }
        ]),
    );
    let settings = crate::config::settings::load_settings_from_path(&path);
    // Capture AFTER the loader: the first load of this minimal fixture may
    // run its one-time repair write. Snapshot building must add no write.
    let base_before = std::fs::read(&path).unwrap();
    let local_path = temp.path().join("settings.50.personal.no-git.json");
    let local_before = std::fs::read(&local_path).unwrap();
    assert!(settings
        .local_overlay_state
        .owns_top_level(crate::config::settings::OVERLAY_KEY_AGENTS));

    let snapshot = settings_snapshot_from(&settings, None);
    assert!(snapshot.overlay_owns_agents);
    let wire = serde_json::to_value(&snapshot).unwrap();
    assert_eq!(wire["overlayOwnsAgents"], json!(true));
    // P1 effective order: valid ordinals first (1 -> ov-e, 2 -> ov-a), then
    // absent/invalid records by their original overlay index.
    let wire_ids: Vec<&str> = wire["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|agent| agent["id"].as_str().unwrap())
        .collect();
    assert_eq!(wire_ids, ["ov-b", "ov-e", "ov-a", "ov-c", "ov-d"]);
    // Building the snapshot must not touch either file.
    assert_eq!(std::fs::read(&path).unwrap(), base_before);
    assert_eq!(std::fs::read(&local_path).unwrap(), local_before);
}

#[tokio::test]
async fn settings_update_does_not_clobber_in_memory_project_lists() {
    let temp = tempfile::tempdir().unwrap();
    let settings_path = temp.path().join("settings.json");
    let disk_project = real_ac_project(temp.path(), "disk-project-a");
    write_settings_file(
        temp.path(),
        &AppSettings {
            project_paths: vec![disk_project.clone()],
            project_path: Some(disk_project.clone()),
            ..AppSettings::default()
        },
    );

    let mut current = settings_with_single_agent();
    current.project_paths = vec!["C:/stale/current".to_string()];
    current.project_path = Some("C:/stale/current".to_string());
    current.root_token = Some("current-root-token".to_string());

    let protected_state = state_for(current.clone());
    let mut stale = current.clone();
    stale.project_paths.clear();
    stale.project_path = None;
    stale.root_token = Some("frontend-root-token".to_string());

    let (saved, _) =
        persist_protected_settings_update_with_saver(&protected_state, stale, |candidate| {
            crate::config::settings::save_settings_to_path_preserving_project_paths(
                candidate,
                &settings_path,
            )
        })
        .await
        .unwrap();

    assert_single_project(&saved, &disk_project);
    assert_eq!(saved.root_token.as_deref(), Some("current-root-token"));
    {
        let live = protected_state.read().await;
        assert_single_project(&live, &disk_project);
        assert_eq!(live.root_token.as_deref(), Some("current-root-token"));
    }

    let draft_state = state_for(current.clone());
    let mut draft = current;
    draft.project_paths.clear();
    draft.project_path = None;

    let (saved, _events) =
        persist_settings_draft_update_with_saver(&draft_state, draft, |candidate| {
            crate::config::settings::save_settings_to_path_preserving_project_paths(
                candidate,
                &settings_path,
            )
        })
        .await
        .unwrap();

    assert_single_project(&saved, &disk_project);
    {
        let live = draft_state.read().await;
        assert_single_project(&live, &disk_project);
    }
}

#[tokio::test]
async fn update_settings_adopts_canonical_archive_without_resurrecting_absent_archive() {
    for canonical in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let settings_path = temp.path().join("settings.json");
        let live_project = real_ac_project(temp.path(), "live-a");
        let archived_project = real_ac_project(temp.path(), "archived-b");
        write_settings_file_without_keys(
            temp.path(),
            &AppSettings {
                project_paths: vec![live_project.clone()],
                project_path: Some(live_project.clone()),
                ..AppSettings::default()
            },
            &["archivedProjectPaths"],
        );
        let sidecar = temp.path().join("project-paths.json");
        if canonical {
            let mut managed: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
            for key in [
                "projectPath",
                "projectPathRelativeToInstance",
                "projectPaths",
                "projectPathsRelativeToInstance",
                "archivedProjectPaths",
                "archivedProjectPathsRelativeToInstance",
            ] {
                managed.as_object_mut().unwrap().remove(key);
            }
            std::fs::write(&settings_path, serde_json::to_vec(&managed).unwrap()).unwrap();
            std::fs::write(
                &sidecar,
                serde_json::to_vec(&serde_json::json!({
                    "projectPath":live_project, "projectPathRelativeToInstance":null,
                    "projectPaths":[live_project], "projectPathsRelativeToInstance":[null],
                    "archivedProjectPaths":[archived_project],
                    "archivedProjectPathsRelativeToInstance":[null]
                }))
                .unwrap(),
            )
            .unwrap();
        }
        let canonical_bytes = canonical.then(|| std::fs::read(&sidecar).unwrap());
        let mut current = settings_with_single_agent();
        current.project_paths = vec![live_project.clone()];
        current.project_path = Some(live_project.clone());
        current.archived_project_paths = vec![archived_project.clone()];
        let state = state_for(current.clone());
        let payload = settings_payload_without_keys(&current, &["archivedProjectPaths"]);
        let (saved, _) =
            persist_protected_settings_update_with_saver(&state, payload, |candidate| {
                crate::config::settings::save_settings_to_path_preserving_project_paths(
                    candidate,
                    &settings_path,
                )
            })
            .await
            .unwrap();
        let disk = crate::config::settings::load_settings_from_path(&settings_path);
        let live = state.read().await;
        for adopted in [&saved, &*live, &disk] {
            assert_single_project(adopted, &live_project);
            if canonical {
                assert_eq!(
                    adopted.archived_project_paths,
                    vec![archived_project.clone()]
                );
            } else {
                assert!(adopted.archived_project_paths.is_empty());
            }
            assert_eq!(
                session_retention_project_paths(adopted)
                    .unwrap()
                    .contains(&archived_project),
                canonical
            );
        }
        let managed: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
        for key in [
            "projectPath",
            "projectPathRelativeToInstance",
            "projectPaths",
            "projectPathsRelativeToInstance",
            "archivedProjectPaths",
            "archivedProjectPathsRelativeToInstance",
        ] {
            assert!(
                managed.get(key).is_none(),
                "managed settings resurrected {key}"
            );
        }
        if let Some(bytes) = canonical_bytes {
            assert_eq!(std::fs::read(&sidecar).unwrap(), bytes);
        } else {
            let raw: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&sidecar).unwrap()).unwrap();
            for key in &[
                "archivedProjectPaths",
                "archivedProjectPathsRelativeToInstance",
            ][..]
            {
                assert!(raw.get(*key).is_none(), "raw authority resurrected {key}");
            }
        }
    }
}

#[tokio::test]
async fn save_settings_draft_adopts_canonical_archive_without_resurrecting_absent_archive() {
    for canonical in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let settings_path = temp.path().join("settings.json");
        let live_project = real_ac_project(temp.path(), "live-a");
        let archived_project = real_ac_project(temp.path(), "archived-b");
        write_settings_file_without_keys(
            temp.path(),
            &AppSettings {
                project_paths: vec![live_project.clone()],
                project_path: Some(live_project.clone()),
                ..AppSettings::default()
            },
            &["archivedProjectPaths"],
        );
        let sidecar = temp.path().join("project-paths.json");
        if canonical {
            let mut managed: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
            for key in [
                "projectPath",
                "projectPathRelativeToInstance",
                "projectPaths",
                "projectPathsRelativeToInstance",
                "archivedProjectPaths",
                "archivedProjectPathsRelativeToInstance",
            ] {
                managed.as_object_mut().unwrap().remove(key);
            }
            std::fs::write(&settings_path, serde_json::to_vec(&managed).unwrap()).unwrap();
            std::fs::write(
                &sidecar,
                serde_json::to_vec(&serde_json::json!({
                    "projectPath":live_project, "projectPathRelativeToInstance":null,
                    "projectPaths":[live_project], "projectPathsRelativeToInstance":[null],
                    "archivedProjectPaths":[archived_project],
                    "archivedProjectPathsRelativeToInstance":[null]
                }))
                .unwrap(),
            )
            .unwrap();
        }
        let canonical_bytes = canonical.then(|| std::fs::read(&sidecar).unwrap());
        let mut current = settings_with_single_agent();
        current.project_paths = vec![live_project.clone()];
        current.project_path = Some(live_project.clone());
        current.archived_project_paths = vec![archived_project.clone()];
        let state = state_for(current.clone());
        let payload = settings_payload_without_keys(&current, &["archivedProjectPaths"]);
        let (saved, _events) =
            persist_settings_draft_update_with_saver(&state, payload, |candidate| {
                crate::config::settings::save_settings_to_path_preserving_project_paths(
                    candidate,
                    &settings_path,
                )
            })
            .await
            .unwrap();
        let disk = crate::config::settings::load_settings_from_path(&settings_path);
        let live = state.read().await;
        for adopted in [&saved, &*live, &disk] {
            assert_single_project(adopted, &live_project);
            if canonical {
                assert_eq!(
                    adopted.archived_project_paths,
                    vec![archived_project.clone()]
                );
            } else {
                assert!(adopted.archived_project_paths.is_empty());
            }
            assert_eq!(
                session_retention_project_paths(adopted)
                    .unwrap()
                    .contains(&archived_project),
                canonical
            );
        }
        let managed: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
        for key in [
            "projectPath",
            "projectPathRelativeToInstance",
            "projectPaths",
            "projectPathsRelativeToInstance",
            "archivedProjectPaths",
            "archivedProjectPathsRelativeToInstance",
        ] {
            assert!(
                managed.get(key).is_none(),
                "managed settings resurrected {key}"
            );
        }
        if let Some(bytes) = canonical_bytes {
            assert_eq!(std::fs::read(&sidecar).unwrap(), bytes);
        } else {
            let raw: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&sidecar).unwrap()).unwrap();
            for key in &[
                "archivedProjectPaths",
                "archivedProjectPathsRelativeToInstance",
            ][..]
            {
                assert!(raw.get(*key).is_none(), "raw authority resurrected {key}");
            }
        }
    }
}

#[tokio::test]
async fn update_settings_adopts_canonical_projects_without_resurrecting_absent_authority() {
    for canonical in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let settings_path = temp.path().join("settings.json");
        let live_project = real_ac_project(temp.path(), "live-a");
        let sidecar = temp.path().join("project-paths.json");
        if canonical {
            std::fs::write(
                &sidecar,
                serde_json::to_vec(&serde_json::json!({
                    "projectPath":live_project, "projectPathRelativeToInstance":null,
                    "projectPaths":[live_project], "projectPathsRelativeToInstance":[null],
                    "archivedProjectPaths":[], "archivedProjectPathsRelativeToInstance":[]
                }))
                .unwrap(),
            )
            .unwrap();
        }
        let canonical_bytes = canonical.then(|| std::fs::read(&sidecar).unwrap());
        let mut current = settings_with_single_agent();
        current.project_paths = vec![live_project.clone()];
        current.project_path = Some(live_project.clone());
        let state = state_for(current.clone());
        let payload = settings_payload_without_keys(&current, &["projectPaths", "projectPath"]);
        let (saved, _) =
            persist_protected_settings_update_with_saver(&state, payload, |candidate| {
                crate::config::settings::save_settings_to_path_preserving_project_paths(
                    candidate,
                    &settings_path,
                )
            })
            .await
            .unwrap();
        let disk = crate::config::settings::load_settings_from_path(&settings_path);
        let live = state.read().await;
        for adopted in [&saved, &*live, &disk] {
            if canonical {
                assert_single_project(adopted, &live_project);
            } else {
                assert!(adopted.project_paths.is_empty());
                assert!(adopted.project_path.is_none());
            }
            assert_eq!(
                session_retention_project_paths(adopted)
                    .unwrap()
                    .contains(&live_project),
                canonical
            );
        }
        let managed: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
        for key in [
            "projectPath",
            "projectPathRelativeToInstance",
            "projectPaths",
            "projectPathsRelativeToInstance",
            "archivedProjectPaths",
            "archivedProjectPathsRelativeToInstance",
        ] {
            assert!(
                managed.get(key).is_none(),
                "managed settings resurrected {key}"
            );
        }
        if let Some(bytes) = canonical_bytes {
            assert_eq!(std::fs::read(&sidecar).unwrap(), bytes);
        } else {
            let raw: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&sidecar).unwrap()).unwrap();
            for key in &[
                "projectPath",
                "projectPathRelativeToInstance",
                "projectPaths",
                "projectPathsRelativeToInstance",
            ][..]
            {
                assert!(raw.get(*key).is_none(), "raw authority resurrected {key}");
            }
        }
    }
}

#[tokio::test]
async fn save_settings_draft_adopts_canonical_projects_without_resurrecting_absent_authority() {
    for canonical in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let settings_path = temp.path().join("settings.json");
        let live_project = real_ac_project(temp.path(), "live-a");
        let sidecar = temp.path().join("project-paths.json");
        if canonical {
            std::fs::write(
                &sidecar,
                serde_json::to_vec(&serde_json::json!({
                    "projectPath":live_project, "projectPathRelativeToInstance":null,
                    "projectPaths":[live_project], "projectPathsRelativeToInstance":[null],
                    "archivedProjectPaths":[], "archivedProjectPathsRelativeToInstance":[]
                }))
                .unwrap(),
            )
            .unwrap();
        }
        let canonical_bytes = canonical.then(|| std::fs::read(&sidecar).unwrap());
        let mut current = settings_with_single_agent();
        current.project_paths = vec![live_project.clone()];
        current.project_path = Some(live_project.clone());
        let state = state_for(current.clone());
        let payload = settings_payload_without_keys(&current, &["projectPaths", "projectPath"]);
        let (saved, _events) =
            persist_settings_draft_update_with_saver(&state, payload, |candidate| {
                crate::config::settings::save_settings_to_path_preserving_project_paths(
                    candidate,
                    &settings_path,
                )
            })
            .await
            .unwrap();
        let disk = crate::config::settings::load_settings_from_path(&settings_path);
        let live = state.read().await;
        for adopted in [&saved, &*live, &disk] {
            if canonical {
                assert_single_project(adopted, &live_project);
            } else {
                assert!(adopted.project_paths.is_empty());
                assert!(adopted.project_path.is_none());
            }
            assert_eq!(
                session_retention_project_paths(adopted)
                    .unwrap()
                    .contains(&live_project),
                canonical
            );
        }
        let managed: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
        for key in [
            "projectPath",
            "projectPathRelativeToInstance",
            "projectPaths",
            "projectPathsRelativeToInstance",
            "archivedProjectPaths",
            "archivedProjectPathsRelativeToInstance",
        ] {
            assert!(
                managed.get(key).is_none(),
                "managed settings resurrected {key}"
            );
        }
        if let Some(bytes) = canonical_bytes {
            assert_eq!(std::fs::read(&sidecar).unwrap(), bytes);
        } else {
            let raw: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&sidecar).unwrap()).unwrap();
            for key in &[
                "projectPath",
                "projectPathRelativeToInstance",
                "projectPaths",
                "projectPathsRelativeToInstance",
            ][..]
            {
                assert!(raw.get(*key).is_none(), "raw authority resurrected {key}");
            }
        }
    }
}

#[tokio::test]
async fn update_settings_still_takes_disk_archived_list_when_key_present() {
    let temp = tempfile::tempdir().unwrap();
    let settings_path = temp.path().join("settings.json");
    let live_project = real_ac_project(temp.path(), "live-a");
    let disk_archived = real_ac_project(temp.path(), "archived-a");
    let live_archived = "C:/archived/b"; // distractor: overridden by disk
    let payload_archived = "C:/archived/c"; // distractor: overridden by disk
    write_settings_file(
        temp.path(),
        &AppSettings {
            project_paths: vec![live_project.clone()],
            project_path: Some(live_project.clone()),
            archived_project_paths: vec![disk_archived.clone()],
            ..AppSettings::default()
        },
    );

    let mut current = settings_with_single_agent();
    current.project_paths = vec![live_project.to_string()];
    current.project_path = Some(live_project.to_string());
    current.archived_project_paths = vec![live_archived.to_string()];
    let state = state_for(current.clone());
    let mut payload = settings_payload_without_keys(&current, &[]);
    payload.archived_project_paths = vec![payload_archived.to_string()];

    let (saved, _) = persist_protected_settings_update_with_saver(&state, payload, |candidate| {
        crate::config::settings::save_settings_to_path_preserving_project_paths(
            candidate,
            &settings_path,
        )
    })
    .await
    .unwrap();

    assert_eq!(
        saved.archived_project_paths,
        vec![disk_archived.to_string()]
    );
    {
        let live = state.read().await;
        assert_eq!(live.archived_project_paths, vec![disk_archived.to_string()]);
    }
    let disk: AppSettings =
        crate::config::settings::load_settings_from_path(std::path::Path::new(&settings_path));
    assert_eq!(disk.archived_project_paths, vec![disk_archived.to_string()]);
}

#[tokio::test]
async fn set_rail_collapse_persists_both_fields_and_publishes_to_live_state() {
    let mut original = settings_with_single_agent();
    original.rail_collapsed_projects = vec!["c:/stale".to_string()];
    original.rail_favorites_collapsed = false;
    let state = state_for(original);

    // Drives the REAL `set_rail_collapse_inner` mutation through its saver seam, so
    // dropping or swapping a field assignment inside that fn fails this test. The fake
    // saver keeps it off disk: the real `save_settings` resolves `config::config_dir()`,
    // a `OnceLock` no test redirects, which would overwrite the developer's settings.json.
    let captured: Mutex<Option<AppSettings>> = Mutex::new(None);
    set_rail_collapse_inner_with_saver(
        &state,
        vec!["c:/foo".to_string(), "d:/bar".to_string()],
        true,
        |candidate| {
            *captured.lock().expect("capture lock") = Some(candidate.clone());
            Ok(candidate.clone())
        },
    )
    .await
    .expect("persist rail collapse");

    let written = captured
        .lock()
        .expect("capture lock")
        .clone()
        .expect("saver ran");
    assert_eq!(
        written.rail_collapsed_projects,
        vec!["c:/foo".to_string(), "d:/bar".to_string()]
    );
    assert!(written.rail_favorites_collapsed);

    let live = state.read().await;
    assert_eq!(
        live.rail_collapsed_projects,
        vec!["c:/foo".to_string(), "d:/bar".to_string()]
    );
    assert!(live.rail_favorites_collapsed);
}

// (#965 F1) The two whole-object writers must not let a stale client payload
// revert the rail snapshot. Without these, `window-geometry.ts` alone (which
// fires a whole-object write on every window move/resize) would silently wipe
// a header click, and nothing re-persists it: the in-memory signal is never
// invalidated, so it does NOT self-heal.
#[tokio::test]
async fn update_settings_keeps_live_rail_collapse_when_payload_is_stale() {
    let mut current = settings_with_single_agent();
    current.rail_collapsed_projects = vec!["c:/foo".to_string()];
    current.rail_favorites_collapsed = true;
    let state = state_for(current);

    let mut stale = settings_with_single_agent();
    stale.rail_collapsed_projects = Vec::new();
    stale.rail_favorites_collapsed = false;

    let (saved, _) = persist_protected_settings_update_with_saver(&state, stale, |c| Ok(c.clone()))
        .await
        .expect("persist");

    assert_eq!(saved.rail_collapsed_projects, vec!["c:/foo".to_string()]);
    assert!(saved.rail_favorites_collapsed);
    let live = state.read().await;
    assert_eq!(live.rail_collapsed_projects, vec!["c:/foo".to_string()]);
    assert!(live.rail_favorites_collapsed);
}

#[tokio::test]
async fn save_settings_draft_keeps_live_rail_collapse_when_draft_is_stale() {
    let mut current = settings_with_single_agent();
    current.rail_collapsed_projects = vec!["c:/foo".to_string()];
    current.rail_favorites_collapsed = true;
    let state = state_for(current);

    let mut draft = settings_with_single_agent();
    draft.rail_collapsed_projects = Vec::new();
    draft.rail_favorites_collapsed = false;

    let (saved, _events) =
        persist_settings_draft_update_with_saver(&state, draft, |c| Ok(c.clone()))
            .await
            .expect("persist");

    assert_eq!(saved.rail_collapsed_projects, vec!["c:/foo".to_string()]);
    assert!(saved.rail_favorites_collapsed);
    let live = state.read().await;
    assert_eq!(live.rail_collapsed_projects, vec!["c:/foo".to_string()]);
    assert!(live.rail_favorites_collapsed);
}

#[tokio::test]
async fn protected_update_settings_transaction_preserves_current_coding_fields() {
    let mut current = settings_with_single_agent();
    current.agents[0].envs = vec![CodingAgentEnv {
        key: "OPENAI_API_BASE".to_string(),
        value: "current".to_string(),
        source: CodingAgentEnvSource::User,
        enabled: true,
    }];
    current.agents[0].isolated_home = true;
    current
        .coding_agent_profiles
        .profiles_by_agent
        .entry("agent-0".to_string())
        .or_default()
        .insert(
            "B".to_string(),
            ProfileCellConfig {
                id_hash: None,
                id_hash_debug: None,
                preflight: None,
                preflight_timeout_seconds: None,
                enabled: true,
                command: "codex --current-profile".to_string(),
                env: BTreeMap::new(),
                notes: String::new(),
            },
        );
    let state = state_for(current.clone());

    let mut stale = current.clone();
    stale.sidebar_style = "command-center".to_string();
    stale.agents[0].envs.clear();
    stale.agents[0].isolated_home = false;
    stale.coding_agent_profiles.profiles_by_agent.clear();

    let (saved, _) = persist_protected_settings_update_with_saver(&state, stale, |candidate| {
        Ok(candidate.clone())
    })
    .await
    .unwrap();

    assert_eq!(saved.sidebar_style, "command-center");
    assert_eq!(saved.agents[0].envs, current.agents[0].envs);
    assert_eq!(
        saved.agents[0].isolated_home,
        current.agents[0].isolated_home
    );
    assert!(saved
        .coding_agent_profiles
        .profiles_by_agent
        .get("agent-0")
        .is_some_and(|cells| cells.contains_key("B")));
    let live = state.read().await;
    assert_eq!(live.sidebar_style, "command-center");
    assert_eq!(live.agents[0].envs, current.agents[0].envs);
    assert_eq!(
        live.agents[0].isolated_home,
        current.agents[0].isolated_home
    );
    assert!(live
        .coding_agent_profiles
        .profiles_by_agent
        .get("agent-0")
        .is_some_and(|cells| cells.contains_key("B")));
}
