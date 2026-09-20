use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::config::ac_root::existing_ac_root;
use crate::config::loops::{
    apply_loop_update_patch, baseline_loop_state, details_from_parts, loop_dir, next_due_after,
    read_loop_config, read_loop_state, sanitize_loop_id, validate_cron_expr, validate_loop_config,
    validate_loop_id, write_loop_config, write_loop_state_atomic, BusyCoordinatorPolicy,
    LoopConfigDetails, LoopConfigToml, LoopDef, LoopPolicy, LoopPrompt, LoopSessionStart,
    LoopTarget, LoopTargetKind, LoopTrigger, LoopTriggerKind, LoopUpdatePatch, LOOP_TIMEZONE_LOCAL,
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
    let _guard = scheduler.mutation_guard().await;
    let id = match request.id.as_deref() {
        Some(id) => sanitize_loop_id(id)?,
        None => sanitize_loop_id(&request.name)?,
    };
    let dir = loop_dir(&ac_root, &id);
    if dir.exists() {
        return Err(format!("Loop '{}' already exists", id));
    }
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
    validate_loop_config(&project_dir, &config)?;
    let dir = write_loop_config(&ac_root, &config)?;
    let state = baseline_loop_state(&config, Utc::now())?;
    write_loop_state_atomic(&dir, &state)?;
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
    let _guard = scheduler.mutation_guard().await;
    let dir = loop_dir(&ac_root, &request.id);
    if !dir.is_dir() {
        return Err(format!("Loop '{}' not found", request.id));
    }
    let mut config = read_loop_config(&dir)?;
    let reset_schedule = apply_loop_update_patch(&mut config, patch_from_update_request(request))?;

    validate_loop_config(&project_dir, &config)?;
    let dir = write_loop_config(&ac_root, &config)?;
    let state = if reset_schedule {
        baseline_loop_state(&config, Utc::now())?
    } else {
        read_loop_state(&dir).unwrap_or_default()
    };
    if reset_schedule {
        write_loop_state_atomic(&dir, &state)?;
    }
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
    let _guard = scheduler.mutation_guard().await;
    let dir = loop_dir(&ac_root, &id);
    if !dir.is_dir() {
        return Err(format!("Loop '{}' not found", id));
    }
    std::fs::remove_dir_all(&dir).map_err(|e| format!("Failed to remove Loop directory: {}", e))?;
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
    let _guard = scheduler.mutation_guard().await;
    let dir = loop_dir(&ac_root, &id);
    if !dir.is_dir() {
        return Err(format!("Loop '{}' not found", id));
    }
    let mut config = read_loop_config(&dir)?;
    let reset_schedule = apply_loop_update_patch(
        &mut config,
        LoopUpdatePatch {
            enabled: Some(enabled),
            ..LoopUpdatePatch::default()
        },
    )?;
    validate_loop_config(&project_dir, &config)?;
    let dir = write_loop_config(&ac_root, &config)?;
    let state = if reset_schedule {
        baseline_loop_state(&config, Utc::now())?
    } else {
        read_loop_state(&dir).unwrap_or_default()
    };
    if reset_schedule {
        write_loop_state_atomic(&dir, &state)?;
    }
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
    use crate::config::loops::{LoopSessionStart, MissedWhileClosedPolicy};

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
}
