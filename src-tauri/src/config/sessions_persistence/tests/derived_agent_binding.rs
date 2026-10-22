use super::super::{
    load_sessions_from_dir, load_sessions_from_path_with_settings, load_sessions_raw_from_path,
    CodingAgentBindingSeed,
};
use super::*;
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

// No process is spawned by this carrier fixture; every host-effect method
// panics so the dormant snapshot control cannot silently become a spawn test.
struct SourceCarrierBackend;

impl crate::pty::backend::PtyBackend for SourceCarrierBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn spawn(
        &self,
        _: crate::pty::backend::BackendSpawnSpec,
    ) -> futures::future::BoxFuture<'_, Result<(), crate::errors::AppError>> {
        panic!("unexpected spawn")
    }
    fn write(
        &self,
        _: &crate::pty::manager::BackendWriteAuthority,
        _: Uuid,
        _: &[u8],
    ) -> Result<(), crate::errors::AppError> {
        panic!("unexpected write")
    }
    fn resize(&self, _: Uuid, _: u16, _: u16) -> Result<(), crate::errors::AppError> {
        panic!("unexpected resize")
    }
    fn kill(&self, _: Uuid) -> Result<(), crate::errors::AppError> {
        panic!("unexpected kill")
    }
    fn has_session(&self, _: Uuid) -> bool {
        false
    }
    fn get_screen_snapshot(&self, _: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
        None
    }
    fn get_pty_size(&self, _: Uuid) -> Option<(u16, u16)> {
        None
    }
    fn get_screen_rows(&self, _: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
        crate::pty::context_scrape::ScreenRowsRead::SessionOver
    }
    fn register_response_watcher(&self, _: Uuid, _: String, _: PathBuf) {
        panic!("unexpected watcher")
    }
    fn terminate_job_for_session(&self, _: Uuid) -> bool {
        panic!("unexpected termination")
    }
    fn kill_all_jobs(&self) -> (usize, usize) {
        (0, 0)
    }
}

#[tokio::test]
async fn e2432_source_seed_roundtrip_decoy() {
    use crate::config::settings::{
        configuration_identity, SourceFamily, SourceKind, SourceRef, SourceRole, SourceScope,
    };
    use crate::session::selection::{DormantRestoreRequest, SelectionCoordinator};
    use sha2::{Digest, Sha256};
    use tauri::Manager;
    let dir = tempfile::tempdir().unwrap();
    let f_path = dir.path().join("F.json");
    let g_path = dir.path().join("G.json");
    let row =
        serde_json::json!({"id":"original", "label":"Same ID", "command":"codex", "color":"red"});
    let base = configuration_identity("codex", &[]);
    let file = |command: &str| {
        serde_json::to_vec(&serde_json::json!({
            "agents": [row.clone()],
            "profilesByBaseHash": {(base.clone()): {"A": {
                "command": command, "env": {"SOURCE": command}, "enabled": true
            }}}
        }))
        .unwrap()
    };
    let f_bytes = file("F-cell");
    std::fs::write(&f_path, &f_bytes).unwrap();
    std::fs::write(&g_path, file("G-cell")).unwrap();
    let mut f_cell = crate::config::settings::empty_profile_cell();
    f_cell.command = "F-cell".into();
    f_cell.env.insert("SOURCE".into(), "F-cell".into());
    let seed = CodingAgentBindingSeed {
        identity_revision: format!("s1:{:x}:{base}", Sha256::digest(&f_bytes)),
        original_agent_id: "original".into(),
        configuration_ref: Some(crate::config::settings::ConfigurationRef {
            version: 1,
            source: SourceRef {
                family: SourceFamily::Agents,
                role: SourceRole::Catalog,
                scope: SourceScope::Project,
                kind: SourceKind::CatalogProject,
                context_root: f_path.to_string_lossy().into_owned(),
            },
            calculated_id: base.clone(),
            requested_profile: None,
            profile_calculated_id: Some(crate::config::settings::pair_identity(
                "codex",
                &[],
                &f_cell,
            )),
            legacy_entry_hint: Some("original".into()),
        }),
    };
    let mut original = recipe("F recipe", dir.path().to_str().unwrap());
    original.coding_agent_binding_id = Some(Uuid::new_v4().to_string());
    original.coding_agent_binding_seed = Some(seed.clone());
    let path = dir.path().join("sessions.json");
    let raw = serde_json::to_vec(&vec![original.clone()]).unwrap();
    std::fs::write(&path, &raw).unwrap();
    // An empty global configuration cannot bootstrap the same stable ID.
    let settings = crate::config::settings::AppSettings::default();
    assert!(crate::config::settings::bootstrap_binding_revision(&settings, "original").is_err());
    let mut loaded = load_sessions_from_path_with_settings(&path, Some(&settings)).remove(0);
    let origin = loaded.coding_agent_binding_origin.clone();
    assert_eq!(
        origin.as_ref().unwrap()["rawFileSHA256"],
        format!("{:x}", Sha256::digest(&raw))
    );
    assert_eq!(
        origin.as_ref().unwrap()["sessionsPath"],
        path.to_string_lossy().as_ref()
    );
    assert_eq!(origin.as_ref().unwrap()["originalOrdinal"], 0);
    std::fs::write(&g_path, file("G-edited-cell")).unwrap();
    loaded.capture_binding_seed(&settings).unwrap();
    assert_eq!(loaded.coding_agent_binding_seed.as_ref(), Some(&seed));
    assert_eq!(
        loaded.coding_agent_binding_id,
        original.coding_agent_binding_id
    );
    assert_eq!(loaded.coding_agent_binding_origin, origin);
    assert_eq!(std::fs::read(&f_path).unwrap(), f_bytes);

    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let coordinator =
        SelectionCoordinator::new(manager.clone(), tokio_util::sync::CancellationToken::new());
    let app = tauri::test::mock_builder()
        .manage(manager.clone())
        .manage(crate::DetachedSessionsState::default())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let pty = Arc::new(std::sync::Mutex::new(
        crate::pty::manager::PtyManager::new_for_test(Arc::new(SourceCarrierBackend)),
    ));
    assert!(app.manage(pty));
    coordinator.start(app.handle().clone()).unwrap();
    let guard = coordinator.submit_restore_first().await.unwrap();
    let transaction = guard.transaction(app.handle().clone());
    let info = transaction
        .restore_dormant_inline(DormantRestoreRequest {
            persisted: loaded.clone(),
            working_directory: loaded.working_directory.clone(),
            is_coordinator: false,
            is_root_agent: false,
        })
        .await
        .unwrap();
    let handle = manager.read().await.clone();
    let live_snapshot = snapshot_sessions(&handle).await;
    let live = live_snapshot
        .iter()
        .find(|row| row.id.as_deref() == Some(info.id.as_str()))
        .unwrap();
    assert_eq!(live.coding_agent_binding_seed.as_ref(), Some(&seed));
    assert_eq!(
        live.coding_agent_binding_id,
        original.coding_agent_binding_id
    );
    assert_eq!(live.coding_agent_binding_origin, origin);
    let failed = sanitize_failed_recoverable(live);
    assert_eq!(failed.coding_agent_binding_seed.as_ref(), Some(&seed));
    let failed_manager = SessionManager::new();
    failed_manager
        .enroll_failed_binding_recipe_for_test(
            failed.coding_agent_binding_id.clone().unwrap(),
            serde_json::to_value(&failed).unwrap(),
        )
        .await;
    // Recipe JSON omits origin; retain its RAM witness through the same
    // binding metadata API used by production failed-recipe refresh.
    failed_manager
        .refresh_failed_binding_recipes(vec![(
            failed.binding_metadata().unwrap().unwrap(),
            serde_json::to_value(&failed).unwrap(),
        )])
        .await;
    let failed_snapshot = snapshot_sessions(&failed_manager).await;
    assert_eq!(failed_snapshot.len(), 1);
    assert_eq!(
        failed_snapshot[0].coding_agent_binding_seed.as_ref(),
        Some(&seed)
    );
    assert_eq!(failed_snapshot[0].coding_agent_binding_origin, origin);
    save_sessions_to_dir(dir.path(), &failed_snapshot).unwrap();
    let restored = load_sessions_from_path_with_settings(&path, Some(&settings)).remove(0);
    assert_eq!(restored.coding_agent_binding_seed.as_ref(), Some(&seed));
    assert_eq!(
        restored.coding_agent_binding_id,
        original.coding_agent_binding_id
    );
    // A new durable read captures a new byte witness; the old RAM witness
    // was preserved across snapshot/failure, and is never serialized.
    assert_ne!(restored.coding_agent_binding_origin, origin);
    assert!(serde_json::to_value(&failed)
        .unwrap()
        .get("codingAgentBindingOrigin")
        .is_none());
    drop(transaction);
    drop(guard);
    coordinator.close_and_join().await;
}

#[test]
fn e2432_source_seed_malformed_refuses_without_downgrade_and_old_bytes_omit_ref() {
    let old = serde_json::json!({"identityRevision":"r1:old", "originalAgentId":"original"});
    let legacy: CodingAgentBindingSeed = serde_json::from_value(old.clone()).unwrap();
    assert_eq!(serde_json::to_value(legacy).unwrap(), old);
    let mut ps = recipe("invalid", "C:/invalid");
    ps.coding_agent_binding_id = Some(Uuid::new_v4().to_string());
    ps.coding_agent_binding_seed = Some(CodingAgentBindingSeed {
        identity_revision: format!("s1:{}:cid1:{}", "a".repeat(64), "b".repeat(64)),
        original_agent_id: "original".into(),
        configuration_ref: None,
    });
    let before = serde_json::to_value(&ps).unwrap();
    assert!(ps
        .capture_binding_seed(&crate::config::settings::AppSettings::default())
        .is_err());
    assert!(ps.binding_metadata().is_err());
    assert_eq!(serde_json::to_value(&ps).unwrap(), before);
    let reference = serde_json::json!({
        "version": 1,
        "source": {
            "family":"agents", "role":"catalog", "scope":"project",
            "kind":"catalogProject", "contextRoot":"F"
        },
        "calculatedId": format!("cid1:{}", "b".repeat(64)),
        "requestedProfile": null,
        "profileCalculatedId": format!("cid1:{}", "c".repeat(64)),
        "legacyEntryHint": "original"
    });
    for field in [
        "version",
        "calculatedId",
        "requestedProfile",
        "profileCalculatedId",
        "legacyEntryHint",
    ] {
        let mut invalid = reference.clone();
        invalid[field] = match field {
            "version" => serde_json::json!(2),
            "calculatedId" => format!("cid1:{}", "d".repeat(64)).into(),
            "requestedProfile" => "invalid".into(),
            "profileCalculatedId" => format!("pid1:{}", "c".repeat(64)).into(),
            _ => "decoy".into(),
        };
        ps.coding_agent_binding_seed
            .as_mut()
            .unwrap()
            .configuration_ref = Some(serde_json::from_value(invalid).unwrap());
        let before = serde_json::to_value(&ps).unwrap();
        assert!(ps
            .capture_binding_seed(&crate::config::settings::AppSettings::default())
            .is_err());
        assert!(ps.binding_metadata().is_err());
        assert_eq!(serde_json::to_value(&ps).unwrap(), before);
        assert_eq!(
            sanitize_failed_recoverable(&ps).coding_agent_binding_seed,
            ps.coding_agent_binding_seed
        );
    }
}

fn recipe(name: &str, cwd: &str) -> PersistedSession {
    PersistedSession {
        name: name.into(),
        shell: "shell".into(),
        working_directory: cwd.into(),
        agent_id: Some("original".into()),
        ..Default::default()
    }
}

#[test]
fn derived_agent_id_binding_legacy_loader_captures_raw_origin_before_transform() {
    use sha2::{Digest, Sha256};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.json");
    let mut rows = vec![
        recipe("[temp] discard", "C:/discard"),
        recipe("kept", "C:/kept"),
    ];
    rows[1].git_branch_source = Some("C:/source".into());
    rows[1].git_branch_prefix = Some("source".into());
    let bytes = serde_json::to_vec_pretty(&rows).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("codingAgentBindingId"));
    std::fs::write(&path, &bytes).unwrap();
    let loaded = load_sessions_from_path(&path);
    assert_eq!(loaded.len(), 1);
    let retained = &loaded[0];
    assert!(Uuid::parse_str(retained.coding_agent_binding_id.as_ref().unwrap()).is_ok());
    let origin = retained.coding_agent_binding_origin.as_ref().unwrap();
    assert_eq!(origin["originalOrdinal"], 1);
    assert_eq!(
        origin["rawFileSHA256"],
        format!("{:x}", Sha256::digest(&bytes))
    );
    assert_eq!(origin["sessionsPath"], path.to_string_lossy().as_ref());
    assert_eq!(retained.git_repos.len(), 1);
    let value = serde_json::to_value(retained).unwrap();
    assert!(value.get("codingAgentBindingOrigin").is_none());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[tokio::test]
async fn derived_agent_id_binding_legacy_seed_precedes_purge_publication() {
    let dir = tempfile::tempdir().unwrap();
    let settings_path = dir
        .path()
        .join(crate::config::instance_artifacts::SETTINGS_FILE_NAME);
    std::fs::write(
        &settings_path,
        serde_json::to_vec(&serde_json::json!({
            "defaultShell": "test-shell",
            "defaultShellArgs": [],
            "agents": [{"id":"original","label":"Original","command":"claude","color":"#112233"}],
            "rootToken": "fixture-token",
        }))
        .unwrap(),
    )
    .unwrap();
    let settings = super::super::super::settings::load_settings_from_path(&settings_path);
    let agent_id = settings.agents[0].id.clone();
    let retained_cwd = dir.path().join("retained");
    std::fs::create_dir(&retained_cwd).unwrap();
    let mut retained = recipe("retained", &retained_cwd.to_string_lossy());
    retained.agent_id = Some(agent_id.clone());
    let rows = vec![recipe("removed", "Z:/outside-project"), retained];
    std::fs::write(
        dir.path().join("sessions.json"),
        serde_json::to_vec(&rows).unwrap(),
    )
    .unwrap();
    let loaded = load_sessions_purging_outside_project_paths_in_dir(
        dir.path(),
        &[retained_cwd.to_string_lossy().to_string()],
        Some(&settings),
    )
    .await;
    assert_eq!(loaded.len(), 1);
    let loaded = &loaded[0];
    assert_eq!(
        loaded.coding_agent_binding_origin.as_ref().unwrap()["originalOrdinal"],
        1
    );
    assert_eq!(
        loaded
            .coding_agent_binding_seed
            .as_ref()
            .unwrap()
            .original_agent_id,
        agent_id
    );
    let durable = load_sessions_raw_from_path(&dir.path().join("sessions.json"));
    assert_eq!(
        durable[0].coding_agent_binding_id,
        loaded.coding_agent_binding_id
    );
    assert_eq!(
        durable[0].coding_agent_binding_seed,
        loaded.coding_agent_binding_seed
    );
}

#[test]
fn derived_agent_id_binding_explicit_id_seed_survive_loader_and_failed_sanitize() {
    let dir = tempfile::tempdir().unwrap();
    let mut ps = recipe("failed", "C:/failed");
    ps.coding_agent_binding_id = Some(Uuid::new_v4().to_string());
    ps.coding_agent_binding_seed = Some(CodingAgentBindingSeed {
        identity_revision: format!("r1:0:{}", "a".repeat(64)),
        original_agent_id: "original".into(),
        configuration_ref: None,
    });
    ps.id = Some(Uuid::new_v4().to_string());
    ps.created_at = Some("old-runtime-date".into());
    ps.status = Some(SessionStatus::Running);
    ps.waiting_for_input = Some(true);
    std::fs::write(
        dir.path().join("sessions.json"),
        serde_json::to_vec(&vec![ps.clone()]).unwrap(),
    )
    .unwrap();
    let loaded = load_sessions_from_dir(dir.path());
    assert_eq!(
        loaded[0].coding_agent_binding_id,
        ps.coding_agent_binding_id
    );
    assert_eq!(
        loaded[0].coding_agent_binding_seed,
        ps.coding_agent_binding_seed
    );
    let failed = sanitize_failed_recoverable(&loaded[0]);
    assert!(
        failed.id.is_none()
            && failed.status.is_none()
            && failed.created_at.is_none()
            && failed.waiting_for_input.is_none()
    );
    assert_eq!(failed.coding_agent_binding_id, ps.coding_agent_binding_id);
    assert_eq!(
        failed.coding_agent_binding_seed,
        ps.coding_agent_binding_seed
    );
    assert_eq!(
        failed.binding_metadata().unwrap().unwrap().origin,
        loaded[0].coding_agent_binding_origin
    );
}

#[test]
fn derived_agent_id_binding_duplicate_durable_rejects_before_dedup_use() {
    let dir = tempfile::tempdir().unwrap();
    let binding = Uuid::new_v4().to_string();
    let mut first = recipe("first", "C:/first");
    first.coding_agent_binding_id = Some(binding.clone());
    let mut second = recipe("second", "C:/second");
    second.coding_agent_binding_id = Some(binding);
    std::fs::write(
        dir.path().join("sessions.json"),
        serde_json::to_vec(&vec![first, second]).unwrap(),
    )
    .unwrap();
    let loaded = load_sessions_from_dir(dir.path());
    assert_eq!(loaded.len(), 2);
    for ps in loaded {
        assert_eq!(
            ps.binding_metadata().unwrap_err(),
            "duplicate_coding_agent_binding"
        );
    }
}

#[test]
fn derived_agent_id_binding_unavailable_capture_keeps_raw_origin_and_recipe() {
    use sha2::{Digest, Sha256};
    for case in ["removed", "ambiguous", "unreadable"] {
        let dir = tempfile::tempdir().unwrap();
        let settings_path = dir
            .path()
            .join(crate::config::instance_artifacts::SETTINGS_FILE_NAME);
        let row = serde_json::json!(
            {"id":"original","label":"Original","command":"claude","color":"red"}
        );
        let agents = if case == "ambiguous" {
            serde_json::json!([row.clone(), row])
        } else {
            serde_json::json!([])
        };
        std::fs::write(
            &settings_path,
            serde_json::to_vec(&serde_json::json!({
                "defaultShell":"test-shell",
                "defaultShellArgs":[],
                "agents":agents,
                "rootToken":"fixture-token"
            }))
            .unwrap(),
        )
        .unwrap();
        if case == "unreadable" {
            std::fs::write(
                dir.path()
                    .join(crate::config::instance_artifacts::AGENTS_INSTANCE_FILE_NAME),
                b"invalid agents JSON",
            )
            .unwrap();
        }
        let settings = super::super::super::settings::load_settings_from_path(&settings_path);
        let expected_error = match case {
            "removed" => "coding_agent_binding_entry_missing_or_ambiguous",
            "ambiguous" => "coding_agent_identity_duplicate",
            _ => "coding_agent_identity_physical_unreadable",
        };
        assert_eq!(
            super::super::super::settings::bootstrap_binding_revision(&settings, "original")
                .unwrap_err(),
            expected_error
        );
        let path = dir.path().join("sessions.json");
        let original = recipe(case, &dir.path().to_string_lossy());
        let bytes = serde_json::to_vec(&vec![original.clone()]).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let loaded = load_sessions_from_path_with_settings(&path, Some(&settings));
        assert_eq!(loaded.len(), 1);
        let ps = &loaded[0];
        assert!(!ps.coding_agent_binding_conflict);
        assert!(ps.coding_agent_binding_seed.is_none());
        let binding = ps.binding_metadata().unwrap().unwrap();
        assert!(Uuid::parse_str(&binding.id).is_ok());
        assert!(binding.seed.is_none());
        assert_eq!(binding.origin, ps.coding_agent_binding_origin);
        let origin = binding.origin.as_ref().unwrap();
        assert_eq!(origin["sessionsPath"], path.to_string_lossy().as_ref());
        assert_eq!(
            origin["rawFileSHA256"],
            format!("{:x}", Sha256::digest(&bytes))
        );
        assert_eq!(origin["originalOrdinal"], 0);
        assert_eq!(ps.shell, original.shell);
        assert_eq!(ps.agent_id, original.agent_id);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        let failed = sanitize_failed_recoverable(ps);
        assert_eq!(failed.coding_agent_binding_id, ps.coding_agent_binding_id);
        assert_eq!(
            failed.coding_agent_binding_origin,
            ps.coding_agent_binding_origin
        );
        assert!(failed.coding_agent_binding_seed.is_none());
        assert_eq!(failed.shell, original.shell);
        let mut invalid = ps.clone();
        invalid.coding_agent_binding_seed = Some(CodingAgentBindingSeed {
            identity_revision: String::new(),
            original_agent_id: "original".into(),
            configuration_ref: None,
        });
        invalid.capture_binding_seed(&settings).unwrap();
        assert_eq!(
            invalid.binding_metadata().unwrap_err(),
            "invalid_coding_agent_binding_seed"
        );
        assert!(invalid
            .coding_agent_binding_seed
            .as_ref()
            .unwrap()
            .identity_revision
            .is_empty());
    }
}

#[test]
fn derived_agent_id_binding_missing_seed_has_no_label_or_timestamp_fallback() {
    let mut ps = recipe("same label", "C:/same");
    ps.coding_agent_binding_id = Some(Uuid::new_v4().to_string());
    ps.created_at = Some("2026-10-13".into());
    let binding = ps.binding_metadata().unwrap().unwrap();
    assert_eq!(Some(&binding.id), ps.coding_agent_binding_id.as_ref());
    assert!(binding.seed.is_none());
    assert!(binding.origin.is_none());
    ps.coding_agent_binding_id = Some("not-a-uuid".into());
    assert_eq!(
        ps.binding_metadata().unwrap_err(),
        "invalid_coding_agent_binding"
    );
    ps.coding_agent_binding_id = Some(Uuid::new_v4().to_string());
    ps.coding_agent_binding_seed = Some(CodingAgentBindingSeed {
        identity_revision: String::new(),
        original_agent_id: "original".into(),
        configuration_ref: None,
    });
    assert_eq!(
        ps.binding_metadata().unwrap_err(),
        "invalid_coding_agent_binding_seed"
    );
    assert_eq!(
        ps.coding_agent_binding_seed
            .as_ref()
            .unwrap()
            .identity_revision,
        ""
    );
}

#[tokio::test]
async fn derived_agent_id_binding_live_only_snapshot_keeps_only_receipt_affected_failure() {
    let manager = SessionManager::new();
    let mut failed = recipe("affected", "C:/affected");
    let binding = Uuid::new_v4().to_string();
    failed.coding_agent_binding_id = Some(binding.clone());
    failed.coding_agent_binding_seed = Some(CodingAgentBindingSeed {
        identity_revision: format!("r1:0:{}", "a".repeat(64)),
        original_agent_id: "original".into(),
        configuration_ref: None,
    });
    failed.id = Some("stale-runtime".into());
    failed.status = Some(SessionStatus::Running);
    manager
        .enroll_failed_binding_recipe_for_test(
            binding.clone(),
            serde_json::to_value(&failed).unwrap(),
        )
        .await;
    manager
        .refresh_failed_binding_recipes(vec![(
            crate::session::manager::CodingAgentBinding {
                id: Uuid::new_v4().to_string(),
                seed: None,
                origin: None,
            },
            serde_json::to_value(&failed).unwrap(),
        )])
        .await;
    for _ in 0..2 {
        let snapshot = snapshot_sessions(&manager).await;
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].coding_agent_binding_id.as_ref(), Some(&binding));
        assert_eq!(
            snapshot[0].coding_agent_binding_seed,
            failed.coding_agent_binding_seed
        );
        assert!(snapshot[0].id.is_none() && snapshot[0].status.is_none());
        assert!(manager.list_sessions().await.is_empty());
    }
}

#[test]
fn derived_agent_id_binding_plain_legacy_shell_stays_unbound() {
    let dir = tempfile::tempdir().unwrap();
    let mut ps = recipe("plain", "C:/plain");
    ps.agent_id = None;
    std::fs::write(
        dir.path().join("sessions.json"),
        serde_json::to_vec(&vec![ps]).unwrap(),
    )
    .unwrap();
    let loaded = load_sessions_from_dir(dir.path());
    assert!(loaded[0].binding_metadata().unwrap().is_none());
    assert!(loaded[0].coding_agent_binding_origin.is_none());
}
#[tokio::test]
async fn response_close_persist_core_retains_real_leaf_after_waiter_cancellation() {
    for fail in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("output");
        if fail {
            std::fs::write(&output, b"not a directory").unwrap();
        }
        let task_output = output.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
        let waiter = tokio::spawn(async move {
            super::super::run_owned_persist_core(async move {
                started_tx.send(()).unwrap();
                release_rx.await.unwrap();
                let result = super::super::save_sessions_to_dir(&task_output, &[]);
                finished_tx.send(result.clone()).unwrap();
                result
            })
            .await
        });
        started_rx.await.unwrap();
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        release_tx.send(()).unwrap();
        let result = finished_rx.await.unwrap();
        if fail {
            assert!(result
                .unwrap_err()
                .starts_with("Failed to create config directory:"));
            assert_eq!(std::fs::read(&output).unwrap(), b"not a directory");
        } else {
            result.unwrap();
            let rows: Vec<PersistedSession> =
                serde_json::from_slice(&std::fs::read(output.join("sessions.json")).unwrap())
                    .unwrap();
            assert!(rows.is_empty());
        }
    }
}
