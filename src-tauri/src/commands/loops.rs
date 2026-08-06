use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::config::loops::{
    apply_loop_update_patch, baseline_loop_state, details_from_parts, loop_dir, next_due_after,
    read_loop_config, read_loop_state, sanitize_loop_id, validate_cron_expr, validate_loop_config,
    validate_loop_id, write_loop_config, write_loop_state_atomic, BusyCoordinatorPolicy,
    LoopConfigDetails, LoopConfigToml, LoopDef, LoopPolicy, LoopPrompt, LoopTarget, LoopTargetKind,
    LoopTrigger, LoopTriggerKind, LoopUpdatePatch, LOOP_TIMEZONE_LOCAL,
};
use crate::config::workspace::existing_workspace_dir;
// #1252: keep private. A `pub use` here would re-expose the emitter and kill the E0603 backstop.
use crate::loops::events::emit_loop_change;
use crate::loops::scheduler::LoopScheduler;

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
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopCronPreview {
    pub next_due_at: Option<chrono::DateTime<Utc>>,
    pub upcoming: Vec<chrono::DateTime<Utc>>,
}

#[tauri::command]
pub async fn create_loop(
    app: AppHandle,
    scheduler: State<'_, Arc<LoopScheduler>>,
    request: LoopCreateRequest,
) -> Result<LoopConfigDetails, String> {
    let project_dir = PathBuf::from(&request.project_path);
    let workspace_dir = workspace_for_project(&project_dir)?;
    crate::commands::ac_discovery::ensure_workspace_gitignore(&workspace_dir)?;
    let _guard = scheduler.mutation_guard().await;
    let id = match request.id.as_deref() {
        Some(id) => sanitize_loop_id(id)?,
        None => sanitize_loop_id(&request.name)?,
    };
    let dir = loop_dir(&workspace_dir, &id);
    if dir.exists() {
        return Err(format!("Loop '{}' already exists", id));
    }
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
        policy: LoopPolicy {
            busy_coordinator: request.busy_coordinator.unwrap_or_default(),
            ..LoopPolicy::default()
        },
    };
    validate_loop_config(&project_dir, &config)?;
    let dir = write_loop_config(&workspace_dir, &config)?;
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
    let workspace_dir = workspace_for_project(&project_dir)?;
    crate::commands::ac_discovery::ensure_workspace_gitignore(&workspace_dir)?;
    let _guard = scheduler.mutation_guard().await;
    let dir = loop_dir(&workspace_dir, &request.id);
    if !dir.is_dir() {
        return Err(format!("Loop '{}' not found", request.id));
    }
    let mut config = read_loop_config(&dir)?;
    let reset_schedule = apply_loop_update_patch(
        &mut config,
        LoopUpdatePatch {
            name: request.name,
            expr: request.expr,
            workgroup: request.workgroup,
            prompt_body: request.prompt_body,
            busy_coordinator: request.busy_coordinator,
            enabled: request.enabled,
        },
    )?;

    validate_loop_config(&project_dir, &config)?;
    let dir = write_loop_config(&workspace_dir, &config)?;
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
    let workspace_dir = workspace_for_project(&project_dir)?;
    let _guard = scheduler.mutation_guard().await;
    let dir = loop_dir(&workspace_dir, &id);
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
    let workspace_dir = workspace_for_project(&project_dir)?;
    let _guard = scheduler.mutation_guard().await;
    let dir = loop_dir(&workspace_dir, &id);
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
    let dir = write_loop_config(&workspace_dir, &config)?;
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
    let workspace_dir = workspace_for_project(&project_dir)?;
    let dir = loop_dir(&workspace_dir, &id);
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

fn workspace_for_project(project_dir: &Path) -> Result<PathBuf, String> {
    existing_workspace_dir(project_dir).ok_or_else(|| {
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
