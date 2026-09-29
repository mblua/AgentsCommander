use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::config::ac_root::existing_ac_root;
use crate::config::loops::{
    create_loop_files, details_from_parts, loop_dir, next_due_after, read_loop_config,
    read_loop_state, remove_loop_files, sanitize_loop_id, update_loop_files, validate_cron_expr,
    validate_loop_id, BusyCoordinatorPolicy, LoopConfigDetails, LoopConfigToml, LoopDef,
    LoopPolicy, LoopPrompt, LoopSessionStart, LoopTarget, LoopTargetKind, LoopTrigger,
    LoopTriggerKind, LoopUpdatePatch, LOOP_TIMEZONE_LOCAL,
};
// #1252: keep private. A `pub use` here would re-expose the emitter and kill the E0603 backstop.
use crate::loops::events::emit_loop_change;
use crate::loops::scheduler::{LoopScheduler, UnresolvedLoopTarget};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopCreateRequest {
    pub project_path: String,
    pub id: Option<String>,
    pub name: String,
    pub expr: String,
    pub workgroup: String,
    pub prompt_body: String,
    #[serde(default)]
    pub busy_coordinator: Option<BusyCoordinatorPolicy>,
    #[serde(default)]
    pub session_start: Option<LoopSessionStart>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopUpdateRequest {
    pub project_path: String,
    pub id: String,
    pub name: Option<String>,
    pub expr: Option<String>,
    pub workgroup: Option<String>,
    pub prompt_body: Option<String>,
    #[serde(default)]
    pub busy_coordinator: Option<BusyCoordinatorPolicy>,
    #[serde(default)]
    pub session_start: Option<LoopSessionStart>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopCronPreview {
    pub next_due_at: Option<chrono::DateTime<Utc>>,
    pub upcoming: Vec<chrono::DateTime<Utc>>,
}

/// The `LoopPolicy` `create_loop` builds from its request. Extracted from the
/// handler's inline literal so the mapping is testable: the handlers take
/// `State<Arc<LoopScheduler>>` and cannot be called from a unit test.
///
/// `None` leaves the field at `LoopPolicy::default()`, which is `Fresh`.
pub(crate) fn policy_from_create_request(request: &LoopCreateRequest) -> LoopPolicy {
    LoopPolicy {
        busy_coordinator: request.busy_coordinator.clone().unwrap_or_default(),
        session_start: request.session_start.unwrap_or_default(),
        ..LoopPolicy::default()
    }
}

/// The `LoopUpdatePatch` `update_loop` builds from its request, extracted for
/// the same reason.
///
/// `None` means "leave unchanged" and must stay `None`: defaulting here would
/// let an update that never mentions the field rewrite it.
pub(crate) fn patch_from_update_request(request: LoopUpdateRequest) -> LoopUpdatePatch {
    LoopUpdatePatch {
        name: request.name,
        expr: request.expr,
        workgroup: request.workgroup,
        prompt_body: request.prompt_body,
        busy_coordinator: request.busy_coordinator,
        session_start: request.session_start,
        enabled: request.enabled,
    }
}

#[tauri::command]
pub async fn create_loop(
    app: AppHandle,
    scheduler: State<'_, Arc<LoopScheduler>>,
    request: LoopCreateRequest,
) -> Result<LoopConfigDetails, String> {
    let project_dir = PathBuf::from(&request.project_path);
    let ac_root = ac_root_for_project(&project_dir)?;
    crate::commands::ac_discovery::ensure_ac_root_gitignore(&ac_root)?;
    let _guard = scheduler.io_guard().await;
    let id = match request.id.as_deref() {
        Some(id) => sanitize_loop_id(id)?,
        None => sanitize_loop_id(&request.name)?,
    };
    let policy = policy_from_create_request(&request);
    let prompt_body = validated_prompt(request.prompt_body)?;
    let config = LoopConfigToml {
        loop_def: LoopDef {
            id: id.clone(),
            name: request.name,
            enabled: request.enabled.unwrap_or(true),
        },
        trigger: LoopTrigger {
            kind: LoopTriggerKind::Cron,
            expr: request.expr,
            timezone: LOOP_TIMEZONE_LOCAL.to_string(),
        },
        target: LoopTarget {
            kind: LoopTargetKind::WorkgroupCoordinator,
            workgroup: request.workgroup,
        },
        prompt: LoopPrompt { body: prompt_body },
        policy,
    };
    let (dir, state) = create_loop_files(&project_dir, &ac_root, &config)?;
    scheduler.bump_loop_generation(&dir);
    let details = details_from_parts(&dir, &config, &state);
    emit_loop_change(
        &app,
        &project_dir,
        &dir,
        &config.loop_def.id,
        "created",
        Some(details.summary.clone()),
        None,
    );
    scheduler.request_scan();
    Ok(details)
}

#[tauri::command]
pub async fn update_loop(
    app: AppHandle,
    scheduler: State<'_, Arc<LoopScheduler>>,
    request: LoopUpdateRequest,
) -> Result<LoopConfigDetails, String> {
    validate_loop_id(&request.id)?;
    let project_dir = PathBuf::from(&request.project_path);
    let ac_root = ac_root_for_project(&project_dir)?;
    crate::commands::ac_discovery::ensure_ac_root_gitignore(&ac_root)?;
    let _guard = scheduler.io_guard().await;
    let loop_id = request.id.clone();
    let (dir, config, state) = update_loop_files(
        &project_dir,
        &ac_root,
        &loop_id,
        patch_from_update_request(request),
    )?;
    scheduler.bump_loop_generation(&dir);
    let details = details_from_parts(&dir, &config, &state);
    emit_loop_change(
        &app,
        &project_dir,
        &dir,
        &config.loop_def.id,
        "updated",
        Some(details.summary.clone()),
        None,
    );
    scheduler.request_scan();
    Ok(details)
}

#[tauri::command]
pub async fn delete_loop(
    app: AppHandle,
    scheduler: State<'_, Arc<LoopScheduler>>,
    project_path: String,
    id: String,
) -> Result<(), String> {
    validate_loop_id(&id)?;
    let project_dir = PathBuf::from(&project_path);
    let ac_root = ac_root_for_project(&project_dir)?;
    let _guard = scheduler.io_guard().await;
    let dir = remove_loop_files(&ac_root, &id)?;
    scheduler.bump_loop_generation(&dir);
    emit_loop_change(&app, &project_dir, &dir, &id, "deleted", None, None);
    scheduler.request_scan();
    Ok(())
}

#[tauri::command]
pub async fn toggle_loop(
    app: AppHandle,
    scheduler: State<'_, Arc<LoopScheduler>>,
    project_path: String,
    id: String,
    enabled: bool,
) -> Result<LoopConfigDetails, String> {
    validate_loop_id(&id)?;
    let project_dir = PathBuf::from(&project_path);
    let ac_root = ac_root_for_project(&project_dir)?;
    let _guard = scheduler.io_guard().await;
    let (dir, config, state) = update_loop_files(
        &project_dir,
        &ac_root,
        &id,
        LoopUpdatePatch {
            enabled: Some(enabled),
            ..LoopUpdatePatch::default()
        },
    )?;
    scheduler.bump_loop_generation(&dir);
    let details = details_from_parts(&dir, &config, &state);
    emit_loop_change(
        &app,
        &project_dir,
        &dir,
        &id,
        if enabled { "enabled" } else { "disabled" },
        Some(details.summary.clone()),
        None,
    );
    scheduler.request_scan();
    Ok(details)
}

#[tauri::command]
pub async fn run_loop_now(
    app: AppHandle,
    scheduler: State<'_, Arc<LoopScheduler>>,
    project_path: String,
    id: String,
) -> Result<LoopConfigDetails, String> {
    validate_loop_id(&id)?;
    let details = scheduler
        .run_loop_now(app.clone(), PathBuf::from(&project_path), id.clone())
        .await?;
    emit_loop_change(
        &app,
        Path::new(&project_path),
        Path::new(&details.summary.path),
        &id,
        "manualRun",
        Some(details.summary.clone()),
        None,
    );
    Ok(details)
}

#[tauri::command]
pub async fn get_loop_config(
    project_path: String,
    id: String,
) -> Result<LoopConfigDetails, String> {
    validate_loop_id(&id)?;
    let project_dir = PathBuf::from(project_path);
    let ac_root = ac_root_for_project(&project_dir)?;
    let dir = loop_dir(&ac_root, &id);
    if !dir.is_dir() {
        return Err(format!("Loop '{}' not found", id));
    }
    let config = read_loop_config(&dir)?;
    let state = read_loop_state(&dir).unwrap_or_default();
    Ok(details_from_parts(&dir, &config, &state))
}

#[tauri::command]
pub async fn preview_loop_cron(expr: String) -> Result<LoopCronPreview, String> {
    validate_cron_expr(&expr)?;
    let mut upcoming = Vec::new();
    let mut cursor = Utc::now();
    for _ in 0..5 {
        let Some(next) = next_due_after(&expr, cursor)? else {
            break;
        };
        upcoming.push(next);
        cursor = next + Duration::seconds(1);
    }
    Ok(LoopCronPreview {
        next_due_at: upcoming.first().copied(),
        upcoming,
    })
}

fn ac_root_for_project(project_dir: &Path) -> Result<PathBuf, String> {
    existing_ac_root(project_dir).ok_or_else(|| {
        format!(
            "Project AC Root not found in {} (.ac)",
            project_dir.display()
        )
    })
}

fn validated_prompt(prompt: String) -> Result<String, String> {
    if prompt.trim().is_empty() {
        Err("Loop prompt cannot be empty".to_string())
    } else {
        Ok(prompt)
    }
}

#[tauri::command]
pub async fn list_unresolved_loop_targets(
    app: AppHandle,
    scheduler: State<'_, Arc<LoopScheduler>>,
) -> Result<Vec<UnresolvedLoopTarget>, String> {
    Ok(scheduler.unresolved_loop_targets(&app).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::loops::{
        apply_loop_update_patch, write_loop_config, write_loop_state_atomic, LoopSessionStart,
        MissedWhileClosedPolicy,
    };
    use tauri::Manager;

    /// A stored Loop whose `sessionStart` is `Accumulate`, so an update that
    /// leaves the field alone is distinguishable from one that defaults it.
    fn accumulate_config() -> LoopConfigToml {
        LoopConfigToml {
            loop_def: LoopDef {
                id: "daily-sync".to_string(),
                name: "Daily sync".to_string(),
                enabled: true,
            },
            trigger: LoopTrigger {
                kind: LoopTriggerKind::Cron,
                expr: "0 9 * * *".to_string(),
                timezone: LOOP_TIMEZONE_LOCAL.to_string(),
            },
            target: LoopTarget {
                kind: LoopTargetKind::WorkgroupCoordinator,
                workgroup: "wg-1-dev-team".to_string(),
            },
            prompt: LoopPrompt {
                body: "Send status".to_string(),
            },
            policy: LoopPolicy {
                session_start: LoopSessionStart::Accumulate,
                ..LoopPolicy::default()
            },
        }
    }

    /// AC-1 - a create payload from a frontend that predates this phase omits
    /// the key entirely and must still deserialize, defaulting to `Fresh`.
    #[test]
    fn create_request_without_session_start_defaults_to_fresh() {
        let payload = r#"{"projectPath":"/tmp/project","name":"Daily sync","expr":"0 9 * * *","workgroup":"wg-1-dev-team","promptBody":"Send status"}"#;
        assert!(
            !payload.contains("sessionStart"),
            "the legacy payload must not carry the key, or this test passes for the wrong reason"
        );

        let request: LoopCreateRequest =
            serde_json::from_str(payload).expect("legacy create payload deserializes");
        assert_eq!(request.session_start, None);
        assert_eq!(
            policy_from_create_request(&request).session_start,
            LoopSessionStart::Fresh
        );
    }

    /// AC-2 - an explicit choice on create is carried into the policy.
    #[test]
    fn create_request_with_accumulate_persists_accumulate() {
        let payload = r#"{"projectPath":"/tmp/project","name":"Daily sync","expr":"0 9 * * *","workgroup":"wg-1-dev-team","promptBody":"Send status","sessionStart":"accumulate"}"#;

        let request: LoopCreateRequest =
            serde_json::from_str(payload).expect("create payload deserializes");
        assert_eq!(request.session_start, Some(LoopSessionStart::Accumulate));
        assert_eq!(
            policy_from_create_request(&request).session_start,
            LoopSessionStart::Accumulate
        );
    }

    /// AC-3 - an update that never mentions the field cannot rewrite it.
    #[test]
    fn update_request_without_session_start_leaves_the_stored_value() {
        let payload = r#"{"projectPath":"/tmp/project","id":"daily-sync","name":"Renamed sync"}"#;
        assert!(!payload.contains("sessionStart"));

        let request: LoopUpdateRequest =
            serde_json::from_str(payload).expect("legacy update payload deserializes");
        let patch = patch_from_update_request(request);
        assert_eq!(patch.session_start, None);

        let mut config = accumulate_config();
        apply_loop_update_patch(&mut config, patch).expect("patch applies");
        assert_eq!(config.policy.session_start, LoopSessionStart::Accumulate);
    }

    /// AC-3b - an explicit `null` is epic 5.1's normal frontend shape and must
    /// behave exactly like an absent key, not like a default.
    #[test]
    fn update_request_with_null_session_start_leaves_the_stored_value() {
        let payload = r#"{"projectPath":"/tmp/project","id":"daily-sync","sessionStart":null}"#;

        let request: LoopUpdateRequest =
            serde_json::from_str(payload).expect("null update payload deserializes");
        let patch = patch_from_update_request(request);
        assert_eq!(patch.session_start, None);

        let mut config = accumulate_config();
        apply_loop_update_patch(&mut config, patch).expect("patch applies");
        assert_eq!(config.policy.session_start, LoopSessionStart::Accumulate);
    }

    /// AC-4 - an explicit choice on update changes the stored value.
    #[test]
    fn update_request_with_fresh_flips_a_stored_accumulate() {
        let payload = r#"{"projectPath":"/tmp/project","id":"daily-sync","sessionStart":"fresh"}"#;

        let request: LoopUpdateRequest =
            serde_json::from_str(payload).expect("update payload deserializes");
        let patch = patch_from_update_request(request);
        assert_eq!(patch.session_start, Some(LoopSessionStart::Fresh));

        let mut config = accumulate_config();
        apply_loop_update_patch(&mut config, patch).expect("patch applies");
        assert_eq!(config.policy.session_start, LoopSessionStart::Fresh);
    }

    /// AC-5 - an unknown value is a serde error surfaced as the existing
    /// command error. No new error type and no new message.
    #[test]
    fn unknown_session_start_value_fails_to_deserialize() {
        let create = r#"{"projectPath":"/tmp/project","name":"Daily sync","expr":"0 9 * * *","workgroup":"wg-1-dev-team","promptBody":"Send status","sessionStart":"resume"}"#;
        assert!(serde_json::from_str::<LoopCreateRequest>(create).is_err());

        let update = r#"{"projectPath":"/tmp/project","id":"daily-sync","sessionStart":"resume"}"#;
        assert!(serde_json::from_str::<LoopUpdateRequest>(update).is_err());
    }

    /// AC-6 (zero-effect) - the extraction is a move. For a request that never
    /// mentions `sessionStart`, both functions reproduce the pre-P2 inline
    /// literals field by field.
    #[test]
    fn extracted_mappings_reproduce_the_pre_phase_literals() {
        let create_payload = r#"{"projectPath":"/tmp/project","name":"Daily sync","expr":"0 9 * * *","workgroup":"wg-1-dev-team","promptBody":"Send status","busyCoordinator":"forceInject","enabled":false}"#;
        let create: LoopCreateRequest =
            serde_json::from_str(create_payload).expect("create payload deserializes");
        let policy = policy_from_create_request(&create);
        assert_eq!(policy.busy_coordinator, BusyCoordinatorPolicy::ForceInject);
        assert_eq!(policy.missed_while_closed, MissedWhileClosedPolicy::Notify);
        // `enabled` is read by the handler, not by the policy mapping.
        assert_eq!(create.enabled, Some(false));

        let update_payload = r#"{"projectPath":"/tmp/project","id":"daily-sync","name":"Renamed sync","expr":"30 9 * * *","workgroup":"wg-2-dev-team","promptBody":"Summarize status","busyCoordinator":"skip","enabled":true}"#;
        let update: LoopUpdateRequest =
            serde_json::from_str(update_payload).expect("update payload deserializes");
        let patch = patch_from_update_request(update);
        assert_eq!(patch.name.as_deref(), Some("Renamed sync"));
        assert_eq!(patch.expr.as_deref(), Some("30 9 * * *"));
        assert_eq!(patch.workgroup.as_deref(), Some("wg-2-dev-team"));
        assert_eq!(patch.prompt_body.as_deref(), Some("Summarize status"));
        assert_eq!(patch.busy_coordinator, Some(BusyCoordinatorPolicy::Skip));
        assert_eq!(patch.enabled, Some(true));
        assert_eq!(patch.session_start, None);
    }

    /// #2695 command fixture: a project whose Room resolves (the shape of
    /// `make_coordinator_fixture`), one Loop with a pending run, a real
    /// `LoopScheduler` managed on a test app.
    struct CommandFixture {
        _tmp: tempfile::TempDir,
        project: PathBuf,
        dir: PathBuf,
        scheduler: Arc<LoopScheduler>,
        app: tauri::App,
    }

    fn command_fixture() -> CommandFixture {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project = tmp.path().join("project");
        let ac_root = project.join(".ac");
        let team_dir = ac_root.join("_team_dev-team");
        let replica = ac_root.join("wg-1-dev-team").join("__agent_tech-lead");
        for dir in [&team_dir, &ac_root.join("_agent_tech-lead"), &replica] {
            std::fs::create_dir_all(dir).expect("fixture dir");
        }
        std::fs::write(
            team_dir.join("config.json"),
            r#"{"agents":["../_agent_tech-lead"],"coordinator":"../_agent_tech-lead"}"#,
        )
        .expect("team config");
        std::fs::write(
            replica.join("config.json"),
            r#"{"identity":"../../_agent_tech-lead"}"#,
        )
        .expect("replica config");
        let dir = write_loop_config(&ac_root, &accumulate_config()).expect("loop config");
        let state = crate::config::loops::LoopState {
            last_checked_at: Some(Utc::now() - Duration::minutes(5)),
            pending_due_at: Some(Utc::now() - Duration::minutes(5)),
            pending_run_id: Some(uuid::Uuid::new_v4()),
            ..Default::default()
        };
        write_loop_state_atomic(&dir, &state).expect("loop state");
        let scheduler = Arc::new(LoopScheduler::new());
        let app = crate::test_support::test_builder()
            .manage(Arc::clone(&scheduler))
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build app");
        CommandFixture {
            _tmp: tmp,
            project,
            dir,
            scheduler,
            app,
        }
    }

    struct GatedScan {
        release: tokio::sync::oneshot::Sender<()>,
        scan: tokio::task::JoinHandle<Result<(), String>>,
        s0_raw: String,
    }

    /// Starts a real scan and returns once it is held inside the delivery.
    async fn gated_command_fixture() -> (CommandFixture, GatedScan) {
        let fixture = command_fixture();
        let s0_raw =
            std::fs::read_to_string(fixture.dir.join(crate::config::loops::LOOP_STATE_FILE))
                .expect("s0 state");
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        fixture
            .scheduler
            .install_delivery_gate(crate::loops::scheduler::LoopDeliveryGate {
                entered: Some(entered_tx),
                release: Some(release_rx),
                report: Some(crate::loops::delivery::LoopDeliveryReport {
                    kind: crate::config::loops::LoopAuditKind::Delivered,
                    message: "gated delivery".to_string(),
                    target: None,
                    session_id: None,
                    error: None,
                    prompt_snapshot: None,
                    completed_at: Some(Utc::now()),
                }),
            });
        let scheduler = Arc::clone(&fixture.scheduler);
        let handle = fixture.app.handle().clone();
        let project = fixture.project.clone();
        let scan = tokio::spawn(async move {
            scheduler
                .scan_project_under_scan_lock(handle, project)
                .await
        });
        entered_rx.await.expect("scan entered the delivery");
        (
            fixture,
            GatedScan {
                release: release_tx,
                scan,
                s0_raw,
            },
        )
    }

    impl CommandFixture {
        fn project_path(&self) -> String {
            self.project.to_string_lossy().to_string()
        }

        fn state_raw(&self) -> String {
            std::fs::read_to_string(self.dir.join(crate::config::loops::LOOP_STATE_FILE))
                .expect("state")
        }

        fn assert_no_scan_audit_row(&self) {
            let audit = self.dir.join(crate::config::loops::LOOP_AUDIT_FILE);
            let rows = std::fs::read_to_string(audit).unwrap_or_default();
            assert!(rows.trim().is_empty(), "no scan audit row: {}", rows);
        }

        fn create_request(&self) -> LoopCreateRequest {
            let config = accumulate_config();
            LoopCreateRequest {
                project_path: self.project_path(),
                id: Some(config.loop_def.id),
                name: config.loop_def.name,
                expr: config.trigger.expr,
                workgroup: config.target.workgroup,
                prompt_body: config.prompt.body,
                busy_coordinator: None,
                session_start: Some(config.policy.session_start),
                enabled: Some(true),
            }
        }

        async fn toggle(&self, enabled: bool) {
            toggle_loop(
                self.app.handle().clone(),
                self.app.state::<Arc<LoopScheduler>>(),
                self.project_path(),
                "daily-sync".to_string(),
                enabled,
            )
            .await
            .expect("toggle_loop");
        }

        async fn delete(&self) {
            delete_loop(
                self.app.handle().clone(),
                self.app.state::<Arc<LoopScheduler>>(),
                self.project_path(),
                "daily-sync".to_string(),
            )
            .await
            .expect("delete_loop");
        }

        async fn create(&self) {
            create_loop(
                self.app.handle().clone(),
                self.app.state::<Arc<LoopScheduler>>(),
                self.create_request(),
            )
            .await
            .expect("create_loop");
        }
    }

    impl GatedScan {
        async fn finish(self) -> Result<(), String> {
            self.release.send(()).expect("release the gate");
            self.scan.await.expect("join scan")
        }
    }

    /// #2695 T2 - a whole `update_loop` completes while a delivery is held.
    #[tokio::test]
    async fn update_loop_completes_while_a_delivery_is_held() {
        let (fixture, gated) = gated_command_fixture().await;
        let request = LoopUpdateRequest {
            project_path: fixture.project_path(),
            id: "daily-sync".to_string(),
            name: None,
            expr: Some("30 9 * * *".to_string()),
            workgroup: None,
            prompt_body: None,
            busy_coordinator: None,
            session_start: None,
            enabled: None,
        };

        let started = std::time::Instant::now();
        let result = update_loop(
            fixture.app.handle().clone(),
            fixture.app.state::<Arc<LoopScheduler>>(),
            request,
        )
        .await;
        let elapsed = started.elapsed();
        eprintln!("T2: update_loop returned in {:?}", elapsed);

        result.expect("update_loop");
        assert!(elapsed < std::time::Duration::from_millis(1000));
        let config = read_loop_config(&fixture.dir).expect("config");
        assert_eq!(config.trigger.expr, "30 9 * * *");
        let baseline = fixture.state_raw();
        let state = read_loop_state(&fixture.dir).expect("state");
        assert!(
            state.pending_run_id.is_none(),
            "the command wrote a baseline"
        );

        gated.finish().await.expect("scan");
        assert_eq!(fixture.state_raw(), baseline);
        fixture.assert_no_scan_audit_row();
    }

    /// #2695 T8 - the value guard alone: an unconditional writer that bumps
    /// no generation and changes no config.
    #[tokio::test]
    async fn scan_does_not_overwrite_a_state_changed_during_delivery() {
        let (fixture, gated) = gated_command_fixture().await;
        let mut state = read_loop_state(&fixture.dir).expect("state");
        state.last_checked_at = Some(Utc::now() + Duration::minutes(1));
        write_loop_state_atomic(&fixture.dir, &state).expect("external write");
        let written = fixture.state_raw();
        assert_ne!(written, gated.s0_raw);

        gated.finish().await.expect("scan");

        assert_eq!(fixture.state_raw(), written);
        fixture.assert_no_scan_audit_row();
    }

    /// #2695 T8b - the realistic ABA: toggle off then on.
    #[tokio::test]
    async fn scan_does_not_overwrite_a_toggle_off_then_on() {
        let (fixture, gated) = gated_command_fixture().await;
        fixture.toggle(false).await;
        fixture.toggle(true).await;
        let baseline = fixture.state_raw();

        gated.finish().await.expect("scan");

        assert_eq!(fixture.state_raw(), baseline);
        fixture.assert_no_scan_audit_row();
    }

    /// #2695 T9 - the identity guard alone: delete, recreate with equal
    /// fields, restore the S0 bytes. Only the generation differs.
    #[tokio::test]
    async fn scan_does_not_write_into_a_recreated_loop() {
        let (fixture, gated) = gated_command_fixture().await;
        fixture.delete().await;
        fixture.create().await;
        std::fs::write(
            fixture.dir.join(crate::config::loops::LOOP_STATE_FILE),
            &gated.s0_raw,
        )
        .expect("restore s0 bytes");
        let s0_raw = gated.s0_raw.clone();

        gated.finish().await.expect("scan");

        assert_eq!(fixture.state_raw(), s0_raw);
        fixture.assert_no_scan_audit_row();
    }

    /// #2695 T10 - a Loop deleted during delivery. No log capture exists in
    /// this crate, so the leg is: the scan is Ok and nothing was recreated.
    #[tokio::test]
    async fn scan_survives_a_delete_during_delivery() {
        let (fixture, gated) = gated_command_fixture().await;
        fixture.delete().await;

        gated.finish().await.expect("scan is Ok");

        assert!(
            !fixture.dir.exists(),
            "nothing recreated the Loop directory"
        );
    }

    /// #2695 T14 - every command bumps the Loop generation.
    #[tokio::test]
    async fn every_loop_command_bumps_the_generation() {
        let fixture = command_fixture();
        let generation = || fixture.scheduler.loop_generation(&fixture.dir);

        let before = generation();
        update_loop(
            fixture.app.handle().clone(),
            fixture.app.state::<Arc<LoopScheduler>>(),
            LoopUpdateRequest {
                project_path: fixture.project_path(),
                id: "daily-sync".to_string(),
                name: Some("Renamed".to_string()),
                expr: None,
                workgroup: None,
                prompt_body: None,
                busy_coordinator: None,
                session_start: None,
                enabled: None,
            },
        )
        .await
        .expect("update_loop");
        assert_ne!(generation(), before, "update_loop must bump");

        let before = generation();
        fixture.toggle(false).await;
        assert_ne!(generation(), before, "toggle_loop must bump");

        let before = generation();
        fixture.delete().await;
        assert_ne!(generation(), before, "delete_loop must bump");

        let before = generation();
        fixture.create().await;
        assert_ne!(generation(), before, "create_loop must bump");
    }
}
