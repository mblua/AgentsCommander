use super::gitignore::ensure_ac_root_gitignore;
use crate::config::ac_root::{ac_root_for_project, canonical_ac_root_label, existing_ac_root};
use std::path::Path;

/// Check if a folder has a Project AC Root subdirectory.
pub(crate) fn check_project_path_inner(path: &str) -> bool {
    existing_ac_root(Path::new(path)).is_some()
}

/// Check if a folder has a Project AC Root subdirectory.
#[tauri::command]
pub async fn check_project_path(path: String) -> Result<bool, String> {
    Ok(check_project_path_inner(&path))
}

/// Create a canonical .ac/ directory inside the given path.
#[tauri::command]
pub async fn create_ac_project(path: String) -> Result<(), String> {
    crate::config::seed_manifest::run_blocking_owned(move || {
        // #1065 Stage F: the bare fresh-root create publishes its project context
        // templates under the sole production token; a `#[cfg(test)]` lib build
        // stays non-emitting so unit fixtures keep manifest-free `.ac` roots.
        #[cfg(not(test))]
        let activation = Some(crate::config::seed_manifest::ManifestActivationToken::production());
        #[cfg(test)]
        let activation: Option<crate::config::seed_manifest::ManifestActivationToken> = None;
        create_ac_project_impl(&path, activation.as_ref(), |ac_root, on_publication| {
            crate::config::session_context::create_default_context_templates_with_publications(
                ac_root,
                on_publication,
            )
        })?;
        // #1318 - a fresh `.ac` root seeds the catalog + masters immediately
        // (no restart needed). INSIDE this blocking closure, so the gate acquire
        // runs on the blocking thread. Fail-soft: any error is logged and the
        // boot loop covers the project at the next boot.
        crate::config::coding_agents_catalog::ensure_seeded_for_project(Path::new(&path));
        Ok(())
    })
    .await
    .map_err(|error| format!("AC project blocking preparation failed: {error}"))?
}

pub(crate) fn create_ac_project_impl<F>(
    path: &str,
    activation: Option<&crate::config::seed_manifest::ManifestActivationToken>,
    create_context_templates: F,
) -> Result<(), String>
where
    F: FnOnce(
        &Path,
        &mut dyn FnMut(&'static str, crate::config::seeded_context_templates::ContextPublication),
    ) -> Result<(), String>,
{
    create_ac_project_impl_with_hook(path, activation, create_context_templates, |_, _| {})
}

pub(crate) fn create_ac_project_impl_with_hook<F, H>(
    path: &str,
    activation: Option<&crate::config::seed_manifest::ManifestActivationToken>,
    create_context_templates: F,
    after_create_before_gate: H,
) -> Result<(), String>
where
    F: FnOnce(
        &Path,
        &mut dyn FnMut(&'static str, crate::config::seeded_context_templates::ContextPublication),
    ) -> Result<(), String>,
    H: FnOnce(&Path, bool),
{
    let project_dir =
        crate::config::projects::absolutise(path).map_err(|error| error.to_string())?;
    match std::fs::symlink_metadata(&project_dir) {
        Ok(metadata) if !metadata.is_dir() => {
            return Err(format!(
                "Project path is not a directory: {}",
                project_dir.display()
            ))
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(&project_dir).map_err(|error| {
                format!(
                    "Failed to create project directory {}: {}",
                    project_dir.display(),
                    error
                )
            })?;
        }
        Err(error) => {
            return Err(format!(
                "Failed to inspect project directory {}: {}",
                project_dir.display(),
                error
            ))
        }
    }

    let ac_root = ac_root_for_project(&project_dir);
    let created = match std::fs::create_dir(&ac_root) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(error) => {
            return Err(format!(
                "Failed to create {} directory: {}",
                canonical_ac_root_label(),
                error
            ))
        }
    };
    let pinned_project = crate::config::seed_manifest::PinnedDirectory::open(&project_dir)
        .map_err(|error| {
            format!(
                "Failed to pin project directory {}: {}",
                project_dir.display(),
                error
            )
        })?;
    let pinned_ac_root =
        crate::config::seed_manifest::PinnedDirectory::open(&ac_root).map_err(|error| {
            format!(
                "Failed to pin {} directory {}: {}",
                canonical_ac_root_label(),
                ac_root.display(),
                error
            )
        })?;
    after_create_before_gate(&ac_root, created);
    pinned_project.revalidate().map_err(|error| {
        format!(
            "AC project setup at {} changed before gate acquisition: {}; retry the operation",
            project_dir.display(),
            error
        )
    })?;
    pinned_ac_root.revalidate().map_err(|error| {
        format!(
            "AC project setup at {} changed before gate acquisition: {}; retry the operation",
            project_dir.display(),
            error
        )
    })?;

    let mut guard = crate::config::seed_manifest::ProjectSeedManifestGuard::acquire(&project_dir)
        .map_err(|error| {
        format!(
            "AC project setup at {} changed or is unavailable: {}; retry the operation",
            project_dir.display(),
            error
        )
    })?;
    pinned_project.revalidate().map_err(|error| {
        format!(
            "AC project setup at {} changed before gate acquisition: {}; retry the operation",
            project_dir.display(),
            error
        )
    })?;
    pinned_ac_root
        .revalidate_at(guard.ac_root())
        .map_err(|error| {
            format!(
                "AC project setup at {} changed before gate acquisition: {}; retry the operation",
                project_dir.display(),
                error
            )
        })?;
    guard.revalidate_owner().map_err(|error| {
        format!(
            "AC project setup at {} changed while preparing: {}; retry the operation",
            project_dir.display(),
            error
        )
    })?;
    if created {
        ensure_ac_root_gitignore(&ac_root)?;
        // #1065 Stage F: record each freshly created project context template under
        // the held gate at its commit-point time. A pre-existing `.ac` is not
        // backfilled (the `else` branch creates no templates), preserving the bare
        // create's no-backfill contract (acceptance item 38).
        let mut on_publication =
            |filename: &'static str,
             publication: crate::config::seeded_context_templates::ContextPublication| {
                if let Some(token) = activation {
                    crate::config::session_context::record_project_context_publication(
                        &mut guard,
                        token,
                        filename,
                        publication.published_at,
                    );
                }
            };
        create_context_templates(&ac_root, &mut on_publication)?;
    } else {
        ensure_ac_root_gitignore(&ac_root)?;
    }
    guard.revalidate_owner().map_err(|error| {
        format!(
            "AC project setup at {} changed while preparing: {}; retry the operation",
            project_dir.display(),
            error
        )
    })?;
    guard.release();
    Ok(())
}
