use crate::config::ac_root::existing_ac_root;
use std::path::Path;

#[tauri::command]
pub async fn keep_custom_context_template(
    path: String,
    filename: String,
    current_file_sha256: String,
    current_default_sha256: String,
) -> Result<(), String> {
    let ac_root = existing_ac_root(Path::new(&path))
        .ok_or_else(|| format!("Project AC Root not found for {}", path))?;
    crate::config::seeded_context_templates::dismiss_context_template_update(
        &ac_root,
        &filename,
        &current_file_sha256,
        &current_default_sha256,
    )
}

/// #1065 Stage F: explicit context-template overwrite recorded under the gate.
///
/// Overwrite is a user-initiated action, so a contention/unsafe gate failure
/// returns a retryable busy error (never a silent skip); a pre-contention gate
/// degradation overwrites untracked. The publication is recorded at the commit
/// point via the callback, before `persist_state_strict`, so a strict state-save
/// error still returns an IPC error without losing the manifest attempt (plan
/// section 5.2). `activation` is `None` for a `#[cfg(test)]` build or an
/// unactivated caller, which runs the plain ungated overwrite unchanged.
pub(crate) fn overwrite_context_template_recorded(
    project_root: &Path,
    ac_root: &Path,
    filename: &str,
    expected_file_sha256: &str,
    expected_default_sha256: &str,
    activation: Option<&crate::config::seed_manifest::ManifestActivationToken>,
) -> Result<crate::config::seeded_context_templates::ContextTemplateOverwriteResult, String> {
    use crate::config::seed_manifest::SoftProjectGate;
    use crate::config::seeded_context_templates::{
        overwrite_context_template_with_default,
        overwrite_context_template_with_default_with_publications, ContextPublication,
    };
    let Some(token) = activation else {
        return overwrite_context_template_with_default(
            ac_root,
            filename,
            expected_file_sha256,
            expected_default_sha256,
        );
    };
    match crate::config::seed_manifest::acquire_project_gate_soft(project_root) {
        SoftProjectGate::Held(mut guard) => {
            let execution = {
                let mut on_publication = |fname: &'static str, publication: ContextPublication| {
                    crate::config::session_context::record_project_context_publication(
                        &mut guard,
                        token,
                        fname,
                        publication.published_at,
                    );
                };
                overwrite_context_template_with_default_with_publications(
                    ac_root,
                    filename,
                    expected_file_sha256,
                    expected_default_sha256,
                    &mut on_publication,
                )
            };
            guard.release();
            execution.completion
        }
        SoftProjectGate::DegradedUntracked => overwrite_context_template_with_default(
            ac_root,
            filename,
            expected_file_sha256,
            expected_default_sha256,
        ),
        SoftProjectGate::Unavailable(error) => Err(format!(
            "Project is busy; retry the context template overwrite ({error})"
        )),
    }
}

#[tauri::command]
pub async fn overwrite_context_template_with_default(
    path: String,
    filename: String,
    current_file_sha256: String,
    current_default_sha256: String,
) -> Result<crate::config::seeded_context_templates::ContextTemplateOverwriteResult, String> {
    let ac_root = existing_ac_root(Path::new(&path))
        .ok_or_else(|| format!("Project AC Root not found for {}", path))?;
    crate::config::seed_manifest::run_blocking_owned(move || {
        // #1065 Stage F: production records the overwrite under the gate; a
        // `#[cfg(test)]` lib build stays non-emitting.
        #[cfg(not(test))]
        let activation = Some(crate::config::seed_manifest::ManifestActivationToken::production());
        #[cfg(test)]
        let activation: Option<crate::config::seed_manifest::ManifestActivationToken> = None;
        overwrite_context_template_recorded(
            Path::new(&path),
            &ac_root,
            &filename,
            &current_file_sha256,
            &current_default_sha256,
            activation.as_ref(),
        )
    })
    .await
    .map_err(|error| format!("context template overwrite blocking task failed: {error}"))?
}

/// Read the `context` array from a replica's config.json.
/// Returns an empty vec if the field is absent or the file doesn't exist.
#[tauri::command]
pub async fn get_replica_context_files(path: String) -> Result<Vec<String>, String> {
    let config_path = Path::new(&path).join("config.json");
    if !config_path.exists() {
        return Ok(vec![]);
    }

    let content = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("Failed to read config.json: {}", e))?;
    let parsed: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse config.json: {}", e))?;

    let files = parsed
        .get("context")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    Ok(files)
}

/// Write the `context` array to a replica's config.json.
/// Preserves all other fields in the config.
#[tauri::command]
pub async fn set_replica_context_files(path: String, files: Vec<String>) -> Result<(), String> {
    let config_path = Path::new(&path).join("config.json");

    crate::config::agent_config::update_agent_config(&config_path, |obj, _state| {
        if files.is_empty() {
            obj.remove("context");
        } else {
            obj.insert("context".to_string(), serde_json::json!(files));
        }
        Ok(())
    })?;

    log::info!("Updated context files for replica at {}: {:?}", path, files);
    Ok(())
}
