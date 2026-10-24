//! IPC wrappers for the role-template picker.

pub(crate) use crate::config::role_templates::copy_dir_recursive;
pub use crate::config::role_templates::{
    agency_manifest_path, agency_template_metas_from_cache, agency_templates_dir,
    agency_templates_status, collect_role_templates, ensure_default_templates_dir,
    ensure_default_templates_dir_at_config, read_agency_manifest, resolve_local_templates_dir,
    resolve_role_template, validate_role_template_body, AgencyTemplatesManifest,
    AgencyTemplatesStatus, LocalTemplatesDir, ResolvedRoleTemplate, RoleTemplateMeta,
    AGENCY_MANIFEST_FILE, AGENCY_TEMPLATES_DIR,
};

// ---------------------------------------------------------------------------
// Command
// ---------------------------------------------------------------------------

/// List all role templates available to the New Agent picker: cached Agency
/// templates plus local templates from the resolved path.
///
/// Returns `Ok` even when the explicit `agentTemplatesPath` is invalid — that
/// failure is logged via `log::error!` (→ ErrorModal) and cached Agency
/// templates are still returned. A missing config dir returns an empty list.
#[tauri::command]
pub async fn list_role_templates(
    settings: tauri::State<'_, crate::config::settings::SettingsState>,
) -> Result<Vec<RoleTemplateMeta>, String> {
    let snapshot = settings
        .current_snapshot()
        .await
        .map_err(|error| error.to_string())?;
    match crate::config::config_dir() {
        Some(config_dir) => Ok(collect_role_templates(&snapshot, &config_dir)),
        None => {
            log::warn!(
                "[role-templates] could not resolve config dir; returning no role templates"
            );
            Ok(Vec::new())
        }
    }
}

#[tauri::command]
pub async fn get_agency_templates_status() -> Result<AgencyTemplatesStatus, String> {
    tauri::async_runtime::spawn_blocking(crate::config::agency_cache::status_cache)
        .await
        .map_err(|e| format!("Agency template status task failed: {}", e))?
}

#[tauri::command]
pub async fn update_agency_templates() -> Result<crate::config::agency_cache::UpdateResult, String>
{
    tauri::async_runtime::spawn_blocking(|| {
        crate::config::agency_cache::update_cache(
            crate::config::agency_cache::AgencyTemplatesUpdateArgs::default_ui_update(),
        )
    })
    .await
    .map_err(|e| format!("Agency template update task failed: {}", e))?
}
