use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use crate::config::loops::{
    baseline_loop_state, details_from_parts, loop_dir, next_due_after, read_loop_config,
    read_loop_state, sanitize_loop_id, validate_cron_expr, validate_loop_config, validate_loop_id,
    write_loop_config, write_loop_state_atomic, AcLoopSummary, BusyCoordinatorPolicy,
    LoopConfigDetails, LoopConfigToml, LoopDef, LoopPolicy, LoopPrompt, LoopTarget, LoopTargetKind,
    LoopTrigger, LoopTriggerKind, LOOP_TIMEZONE_LOCAL,
};
use crate::config::workspace::existing_workspace_dir;
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopEventPayload {
    pub kind: String,
    pub project_path: String,
    pub loop_id: String,
    pub summary: Option<AcLoopSummary>,
    pub message: Option<String>,
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
    let dir = loop_dir(&workspace_dir, &request.id);
    if !dir.is_dir() {
        return Err(format!("Loop '{}' not found", request.id));
    }
    let mut config = read_loop_config(&dir)?;
    let mut reset_schedule = false;

    if let Some(name) = request.name {
        if name.trim().is_empty() {
            return Err("Loop name cannot be empty".to_string());
        }
        config.loop_def.name = name;
    }
    if let Some(expr) = request.expr {
        config.trigger.expr = expr;
        reset_schedule = true;
    }
    if let Some(workgroup) = request.workgroup {
        config.target.workgroup = workgroup;
        reset_schedule = true;
    }
    if let Some(prompt_body) = request.prompt_body {
        config.prompt.body = validated_prompt(prompt_body)?;
        reset_schedule = true;
    }
    if let Some(policy) = request.busy_coordinator {
        config.policy.busy_coordinator = policy;
        reset_schedule = true;
    }
    if let Some(enabled) = request.enabled {
        config.loop_def.enabled = enabled;
        reset_schedule = true;
    }

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
    let dir = loop_dir(&workspace_dir, &id);
    if !dir.is_dir() {
        return Err(format!("Loop '{}' not found", id));
    }
    let mut config = read_loop_config(&dir)?;
    config.loop_def.enabled = enabled;
    validate_loop_config(&project_dir, &config)?;
    let dir = write_loop_config(&workspace_dir, &config)?;
    let state = baseline_loop_state(&config, Utc::now())?;
    write_loop_state_atomic(&dir, &state)?;
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

pub fn emit_loop_change(
    app: &AppHandle,
    project_path: &Path,
    changed_path: &Path,
    loop_id: &str,
    kind: &str,
    summary: Option<AcLoopSummary>,
    message: Option<String>,
) {
    let project_path = std::fs::canonicalize(project_path).unwrap_or_else(|_| project_path.into());
    let changed_path = std::fs::canonicalize(changed_path).unwrap_or_else(|_| changed_path.into());
    let project_path_string = project_path.to_string_lossy().to_string();
    let changed_path_string = changed_path.to_string_lossy().to_string();
    let _ = app.emit(
        "loop_event",
        LoopEventPayload {
            kind: kind.to_string(),
            project_path: project_path_string.clone(),
            loop_id: loop_id.to_string(),
            summary,
            message,
        },
    );
    let _ = app.emit(
        "ac_project_refresh_requested",
        serde_json::json!({
            "id": uuid::Uuid::new_v4().to_string(),
            "projectPath": project_path_string,
            "changedPath": changed_path_string,
            "changedName": loop_id,
            "reason": format!("loop{}", capitalize_reason(kind)),
        }),
    );
}

fn capitalize_reason(kind: &str) -> String {
    let mut chars = kind.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => "Changed".to_string(),
    }
}
