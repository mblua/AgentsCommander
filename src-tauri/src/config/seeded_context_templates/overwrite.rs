use super::{
    actionable_project_spec_by_filename, display_path, load_state, persist_state_strict,
    read_validated_snapshot, sha256_hex, validate_expected_hashes, validate_project_ac_root,
    ContextPublication, ContextTemplateExecution, ContextTemplateOverwriteResult,
    ContextTemplateUpdate, LoadedState, SeededContextTemplateSpec, CONTEXT_TEMPLATE_CHANGED,
    CONTEXT_TEMPLATE_DEFAULT_CHANGED,
};
use crate::config::session_context::atomically_replace_context_template;
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

pub fn dismiss_context_template_update(
    ac_root: &Path,
    filename: &str,
    expected_file_sha256: &str,
    expected_default_sha256: &str,
) -> Result<(), String> {
    let spec = actionable_project_spec_by_filename(filename)?;
    let project_dir = validate_project_ac_root(ac_root)?;
    let mut loaded = load_state(ac_root, true)?;
    validate_expected_hashes(
        &project_dir,
        ac_root,
        spec,
        &loaded,
        expected_file_sha256,
        expected_default_sha256,
    )?;
    let path = ac_root.join(spec.filename);
    let Some(snapshot) = read_validated_snapshot(&path, "Context template")? else {
        return Err(CONTEXT_TEMPLATE_CHANGED.to_string());
    };
    if snapshot.sha256 != expected_file_sha256 {
        return Err(CONTEXT_TEMPLATE_CHANGED.to_string());
    }
    loaded.mark_ignored(spec, expected_file_sha256, expected_default_sha256);
    persist_state_strict(ac_root, &loaded)
}

pub fn overwrite_context_template_with_default(
    ac_root: &Path,
    filename: &str,
    expected_file_sha256: &str,
    expected_default_sha256: &str,
) -> Result<ContextTemplateOverwriteResult, String> {
    let mut on_publication = |_: &'static str, _: ContextPublication| {};
    let execution = overwrite_context_template_with_default_with_publications(
        ac_root,
        filename,
        expected_file_sha256,
        expected_default_sha256,
        &mut on_publication,
    );
    if let Some(publication) = execution.published {
        log::debug!(
            "[context-templates] {} overwrite published at {}",
            filename,
            publication.published_at
        );
    }
    execution.completion
}

pub(crate) fn overwrite_context_template_with_default_with_publications(
    ac_root: &Path,
    filename: &str,
    expected_file_sha256: &str,
    expected_default_sha256: &str,
    on_publication: &mut dyn FnMut(&'static str, ContextPublication),
) -> ContextTemplateExecution<ContextTemplateOverwriteResult> {
    let mut clock = chrono::Utc::now;
    overwrite_context_template_with_default_with(
        ac_root,
        filename,
        expected_file_sha256,
        expected_default_sha256,
        &mut clock,
        on_publication,
        persist_state_strict,
    )
}

pub(crate) fn prepare_context_template_overwrite(
    ac_root: &Path,
    filename: &str,
    expected_file_sha256: &str,
    expected_default_sha256: &str,
) -> Result<(SeededContextTemplateSpec, LoadedState, PathBuf, PathBuf), String> {
    let spec = actionable_project_spec_by_filename(filename)?;
    let project_dir = validate_project_ac_root(ac_root)?;
    let loaded = load_state(ac_root, true)?;
    validate_expected_hashes(
        &project_dir,
        ac_root,
        spec,
        &loaded,
        expected_file_sha256,
        expected_default_sha256,
    )?;

    let path = ac_root.join(spec.filename);
    let Some(snapshot) = read_validated_snapshot(&path, "Context template")? else {
        return Err(CONTEXT_TEMPLATE_CHANGED.to_string());
    };
    if snapshot.sha256 != expected_file_sha256 {
        return Err(CONTEXT_TEMPLATE_CHANGED.to_string());
    }
    if sha256_hex((spec.current_content)().as_bytes()) != expected_default_sha256 {
        return Err(CONTEXT_TEMPLATE_DEFAULT_CHANGED.to_string());
    }

    let backup_path = create_backup(&path, &snapshot.bytes)?;
    let Some(snapshot_after_backup) = read_validated_snapshot(&path, "Context template")? else {
        return Err(CONTEXT_TEMPLATE_CHANGED.to_string());
    };
    if snapshot_after_backup.sha256 != expected_file_sha256 {
        return Err(CONTEXT_TEMPLATE_CHANGED.to_string());
    }

    Ok((spec, loaded, path, backup_path))
}

pub(crate) fn overwrite_context_template_with_default_with<P>(
    ac_root: &Path,
    filename: &str,
    expected_file_sha256: &str,
    expected_default_sha256: &str,
    clock: &mut dyn FnMut() -> chrono::DateTime<chrono::Utc>,
    on_publication: &mut dyn FnMut(&'static str, ContextPublication),
    persist: P,
) -> ContextTemplateExecution<ContextTemplateOverwriteResult>
where
    P: FnOnce(&Path, &LoadedState) -> Result<(), String>,
{
    let (spec, mut loaded, path, backup_path) = match prepare_context_template_overwrite(
        ac_root,
        filename,
        expected_file_sha256,
        expected_default_sha256,
    ) {
        Ok(prepared) => prepared,
        Err(error) => return ContextTemplateExecution::failed(error),
    };

    let published_at =
        match atomically_replace_context_template(&path, (spec.current_content)(), clock) {
            Ok(published_at) => published_at,
            Err(error) => {
                log::warn!(
                    "[context-templates] replacement failed after backup {} was created: {}",
                    backup_path.display(),
                    error
                );
                return ContextTemplateExecution::failed(error);
            }
        };
    let publication = ContextPublication { published_at };
    on_publication(spec.filename, publication);

    loaded.mark_seeded(spec, expected_default_sha256);
    let result = ContextTemplateOverwriteResult {
        file_path: display_path(&path),
        backup_path: display_path(&backup_path),
        current_default_sha256: expected_default_sha256.to_string(),
    };
    ContextTemplateExecution::with_publication(
        persist(ac_root, &loaded).map(|()| result),
        publication,
    )
}

pub(crate) fn create_backup(path: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
    let parent = path.parent().ok_or_else(|| {
        format!(
            "Could not resolve parent directory for context template {}",
            path.display()
        )
    })?;
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("Invalid context template filename {}", path.display()))?;
    let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%SZ").to_string();

    for index in 0..1000_u32 {
        let backup_name = match index {
            0 => format!("{filename}.bak"),
            1 => format!("{filename}.{timestamp}.bak"),
            n => format!("{filename}.{timestamp}.{n}.bak"),
        };
        let backup_path = parent.join(backup_name);
        let mut file =
            match crate::filesystem::context_templates::create_template_stage(&backup_path) {
                Ok(file) => file,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    return Err(format!(
                        "Failed to create backup context template {}: {}",
                        backup_path.display(),
                        e
                    ))
                }
            };
        if let Err(e) = file.write_all(bytes) {
            return Err(format!(
                "Failed to write backup context template {}: {}",
                backup_path.display(),
                e
            ));
        }
        if let Err(e) = file.flush() {
            return Err(format!(
                "Failed to flush backup context template {}: {}",
                backup_path.display(),
                e
            ));
        }
        if let Err(e) = file.sync_all() {
            return Err(format!(
                "Failed to sync backup context template {}: {}",
                backup_path.display(),
                e
            ));
        }
        drop(file);
        if let Ok(dir) = crate::filesystem::context_templates::open_template_directory(parent) {
            if let Err(e) = dir.sync_all() {
                log::warn!(
                    "[context-templates] failed to sync backup directory {}: {}",
                    parent.display(),
                    e
                );
            }
        }
        return Ok(backup_path);
    }

    Err(format!(
        "Failed to create a unique backup path for {}",
        path.display()
    ))
}

pub fn dedupe_context_template_updates(updates: &mut Vec<ContextTemplateUpdate>) {
    let mut seen = HashSet::new();
    updates.retain(|update| {
        seen.insert((
            update.workspace_path.clone(),
            update.filename.clone(),
            update.current_file_sha256.clone(),
            update.current_default_sha256.clone(),
        ))
    });
}
