use super::{
    create_backup, display_path, local_override_filename, read_validated_snapshot, sha256_hex,
    validate_existing_dir, validate_existing_file, ContextPublication, ContextTemplateExecution,
    ContextTemplateReplacement, ContextTemplateSkipReason, ContextTemplateUpdate, LoadedState,
    SeededContextTemplateSpec, TemplatePublication, TemplateSyncOutcome,
};
use crate::config::session_context::atomically_replace_context_template;
use std::path::{Path, PathBuf};

pub(crate) fn make_update(
    project_dir: &Path,
    ac_root: &Path,
    path: &Path,
    spec: SeededContextTemplateSpec,
    current_file_sha256: String,
    current_default_sha256: String,
) -> ContextTemplateUpdate {
    ContextTemplateUpdate {
        project_path: display_path(project_dir),
        workspace_path: display_path(ac_root),
        file_path: display_path(path),
        filename: spec.filename.to_string(),
        label: spec.label.to_string(),
        current_file_sha256,
        current_default_sha256,
        current_default_version: spec.current_version,
    }
}

pub(crate) fn create_missing_template(
    path: &Path,
    content: &str,
    clock: &mut dyn FnMut() -> chrono::DateTime<chrono::Utc>,
) -> ContextTemplateExecution<crate::config::session_context::CreateOnlyPublication> {
    let outcome = match crate::config::session_context::write_template_if_missing_with_clock(
        path, content, clock,
    ) {
        Ok(outcome) => outcome,
        Err(error) => return ContextTemplateExecution::failed(error),
    };
    let published = match outcome {
        crate::config::session_context::CreateOnlyPublication::Published { published_at } => {
            Some(ContextPublication { published_at })
        }
        crate::config::session_context::CreateOnlyPublication::AlreadyPresent => None,
    };
    ContextTemplateExecution::from_parts(
        validate_existing_file(path, "Context template").map(|()| outcome),
        published,
    )
}

pub(crate) fn auto_update_generated_template(
    path: &Path,
    spec: SeededContextTemplateSpec,
    expected_file_sha256: &str,
    clock: &mut dyn FnMut() -> chrono::DateTime<chrono::Utc>,
) -> ContextTemplateExecution<TemplatePublication> {
    auto_update_generated_template_with(
        path,
        spec,
        expected_file_sha256,
        clock,
        crate::config::session_context::atomically_replace_context_template,
    )
}

pub(crate) fn auto_update_generated_template_with<R>(
    path: &Path,
    spec: SeededContextTemplateSpec,
    expected_file_sha256: &str,
    clock: &mut dyn FnMut() -> chrono::DateTime<chrono::Utc>,
    replace: R,
) -> ContextTemplateExecution<TemplatePublication>
where
    R: FnOnce(
        &Path,
        &str,
        &mut dyn FnMut() -> chrono::DateTime<chrono::Utc>,
    ) -> Result<chrono::DateTime<chrono::Utc>, String>,
{
    auto_update_generated_template_with_backup(
        path,
        spec,
        expected_file_sha256,
        clock,
        create_backup,
        replace,
    )
}

pub(crate) fn auto_update_generated_template_with_backup<B, R>(
    path: &Path,
    spec: SeededContextTemplateSpec,
    expected_file_sha256: &str,
    clock: &mut dyn FnMut() -> chrono::DateTime<chrono::Utc>,
    backup: B,
    replace: R,
) -> ContextTemplateExecution<TemplatePublication>
where
    B: FnOnce(&Path, &[u8]) -> Result<PathBuf, String>,
    R: FnOnce(
        &Path,
        &str,
        &mut dyn FnMut() -> chrono::DateTime<chrono::Utc>,
    ) -> Result<chrono::DateTime<chrono::Utc>, String>,
{
    let snapshot = match read_validated_snapshot(path, "Context template") {
        Ok(Some(snapshot)) => snapshot,
        Ok(None) => {
            return ContextTemplateExecution::completed(TemplatePublication::ChangedUnderUs)
        }
        Err(error) => return ContextTemplateExecution::failed(error),
    };
    if snapshot.sha256 != expected_file_sha256 {
        log::warn!(
            "[context-templates] {} changed before generated update; preserving current content",
            path.display()
        );
        return ContextTemplateExecution::completed(TemplatePublication::ChangedUnderUs);
    }
    if !spec
        .is_known_generated
        .is_some_and(|matches| matches(&snapshot.content))
    {
        log::warn!(
            "[context-templates] {} no longer matches a known generated default; preserving \
                current content",
            path.display()
        );
        return ContextTemplateExecution::completed(TemplatePublication::ChangedUnderUs);
    }
    if spec.id == "rootAgent" {
        let backup_path = match backup(path, &snapshot.bytes) {
            Ok(path) => path,
            Err(error) => return ContextTemplateExecution::failed(error),
        };
        let after_backup = match read_validated_snapshot(path, "Context template") {
            Ok(snapshot) => snapshot,
            Err(error) => return ContextTemplateExecution::failed(error),
        };
        if !after_backup.as_ref().is_some_and(|current| {
            current.sha256 == snapshot.sha256 && current.bytes == snapshot.bytes
        }) {
            log::warn!(
                "[context-templates] {} changed after Root backup {}; preserving current content",
                path.display(),
                backup_path.display()
            );
            return ContextTemplateExecution::completed(TemplatePublication::ChangedUnderUs);
        }
    }
    let published_at = match replace(path, (spec.current_content)(), clock) {
        Ok(published_at) => published_at,
        Err(error) => return ContextTemplateExecution::failed(error),
    };
    let publication = ContextPublication { published_at };
    ContextTemplateExecution::with_publication(
        Ok(TemplatePublication::Published(publication)),
        publication,
    )
}

// All generated migrations share publication/state handling. Eligibility stays
// at each caller, and state advances only after successful publication.
pub(crate) fn sync_generated_template(
    path: &Path,
    spec: SeededContextTemplateSpec,
    expected_sha256: &str,
    loaded: &mut LoadedState,
    current_default_sha256: &str,
    clock: &mut dyn FnMut() -> chrono::DateTime<chrono::Utc>,
) -> ContextTemplateExecution<TemplateSyncOutcome> {
    let execution = auto_update_generated_template(path, spec, expected_sha256, clock);
    let published = execution.published;
    match execution.completion {
        Ok(target_outcome) => {
            if matches!(target_outcome, TemplatePublication::Published(_)) {
                loaded.mark_seeded(spec, current_default_sha256);
            }
            ContextTemplateExecution::from_parts(
                Ok(TemplateSyncOutcome {
                    pending_update: None,
                    replacement: None,
                    target_outcome,
                }),
                published,
            )
        }
        Err(error) => ContextTemplateExecution::from_parts(Err(error), published),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn sync_one_template(
    project_dir: Option<&Path>,
    ac_root: &Path,
    spec: SeededContextTemplateSpec,
    loaded: &mut LoadedState,
    allow_create_missing: bool,
    return_pending: bool,
    repair: bool,
    clock: &mut dyn FnMut() -> chrono::DateTime<chrono::Utc>,
) -> ContextTemplateExecution<TemplateSyncOutcome> {
    let path = ac_root.join(spec.filename);
    let current_default = (spec.current_content)();
    let current_default_sha256 = sha256_hex(current_default.as_bytes());
    let mut snapshot = match read_validated_snapshot(&path, "Context template") {
        Ok(snapshot) => snapshot,
        Err(error) => return ContextTemplateExecution::failed(error),
    };
    let mut carried_publication = None;

    if snapshot.is_none() {
        if !allow_create_missing {
            return ContextTemplateExecution::completed(TemplateSyncOutcome {
                pending_update: None,
                replacement: None,
                target_outcome: TemplatePublication::Skipped(
                    ContextTemplateSkipReason::CreationDisabled,
                ),
            });
        }
        let creation = create_missing_template(&path, current_default, clock);
        carried_publication = creation.published;
        if let Err(error) = creation.completion {
            return ContextTemplateExecution::from_parts(Err(error), carried_publication);
        }
        snapshot = match read_validated_snapshot(&path, "Context template") {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return ContextTemplateExecution::from_parts(Err(error), carried_publication)
            }
        };
        if let Some(snapshot) = &snapshot {
            if snapshot.sha256 == current_default_sha256
                && (spec.id != "rootAgent" || snapshot.bytes == current_default.as_bytes())
            {
                loaded.mark_seeded(spec, &current_default_sha256);
                let target_outcome = carried_publication
                    .map(TemplatePublication::Published)
                    .unwrap_or(TemplatePublication::AlreadyCurrent);
                return ContextTemplateExecution::from_parts(
                    Ok(TemplateSyncOutcome {
                        pending_update: None,
                        replacement: None,
                        target_outcome,
                    }),
                    carried_publication,
                );
            }
        }
    }

    let Some(snapshot) = snapshot else {
        let target_outcome = carried_publication
            .map(TemplatePublication::Published)
            .unwrap_or(TemplatePublication::Skipped(
                ContextTemplateSkipReason::MissingAfterCreate,
            ));
        return ContextTemplateExecution::from_parts(
            Ok(TemplateSyncOutcome {
                pending_update: None,
                replacement: None,
                target_outcome,
            }),
            carried_publication,
        );
    };

    if snapshot.sha256 == current_default_sha256
        && (spec.id != "rootAgent" || snapshot.bytes == current_default.as_bytes())
    {
        loaded.mark_seeded(spec, &current_default_sha256);
        let target_outcome = carried_publication
            .map(TemplatePublication::Published)
            .unwrap_or(TemplatePublication::AlreadyCurrent);
        return ContextTemplateExecution::from_parts(
            Ok(TemplateSyncOutcome {
                pending_update: None,
                replacement: None,
                target_outcome,
            }),
            carried_publication,
        );
    }

    let context = ExistingTemplateSync {
        project_dir,
        ac_root,
        path,
        spec,
        loaded,
        snapshot,
        current_default_sha256,
        carried_publication,
    };
    if spec.distribution_owned {
        return repair_distribution_template(context, current_default, repair, clock);
    }
    sync_customized_template(context, return_pending, clock)
}

struct ExistingTemplateSync<'a> {
    project_dir: Option<&'a Path>,
    ac_root: &'a Path,
    path: PathBuf,
    spec: SeededContextTemplateSpec,
    loaded: &'a mut LoadedState,
    snapshot: super::state::FileSnapshot,
    current_default_sha256: String,
    carried_publication: Option<ContextPublication>,
}

fn repair_distribution_template(
    context: ExistingTemplateSync<'_>,
    current_default: &str,
    repair: bool,
    clock: &mut dyn FnMut() -> chrono::DateTime<chrono::Utc>,
) -> ContextTemplateExecution<TemplateSyncOutcome> {
    let ExistingTemplateSync {
        project_dir,
        ac_root,
        path,
        spec,
        loaded,
        snapshot,
        current_default_sha256,
        carried_publication,
    } = context;
    if !repair {
        // D2: no reporting channel on this path, so the repair is deferred to
        // the scan rather than replacing operator bytes with no notification.
        let target_outcome = carried_publication
            .map(TemplatePublication::Published)
            .unwrap_or(TemplatePublication::Skipped(
                ContextTemplateSkipReason::DistributionRepairDeferred,
            ));
        return ContextTemplateExecution::from_parts(
            Ok(TemplateSyncOutcome {
                pending_update: None,
                replacement: None,
                target_outcome,
            }),
            carried_publication,
        );
    }

    // Unreachable today: `repair` is true only on the scan, which always has a
    // project path. It must stay an error rather than a silent `None`.
    let project_dir = match project_dir {
        Some(project_dir) => project_dir,
        None => {
            return ContextTemplateExecution::from_parts(
                Err(format!(
                    "Cannot create context template update for {} without a project path",
                    path.display()
                )),
                carried_publication,
            )
        }
    };

    // D6: silent only when the bytes about to be replaced are exactly the ones
    // we last wrote. Computed before any write.
    let notify = loaded
        .trusted_entry(spec)
        .and_then(|entry| entry.last_seeded_sha256.as_deref())
        != Some(snapshot.sha256.as_str());

    let backup_path = match create_backup(&path, &snapshot.bytes) {
        Ok(backup_path) => backup_path,
        Err(error) => return ContextTemplateExecution::from_parts(Err(error), carried_publication),
    };
    let after_backup = match read_validated_snapshot(&path, "Context template") {
        Ok(after_backup) => after_backup,
        Err(error) => return ContextTemplateExecution::from_parts(Err(error), carried_publication),
    };
    if !after_backup
        .as_ref()
        .is_some_and(|after_backup| after_backup.sha256 == snapshot.sha256)
    {
        log::warn!(
            "[context-templates] {} changed before the distribution repair; the bytes read \
                first are kept in {}",
            path.display(),
            backup_path.display()
        );
        return ContextTemplateExecution::from_parts(
            Ok(TemplateSyncOutcome {
                pending_update: None,
                replacement: None,
                target_outcome: TemplatePublication::ChangedUnderUs,
            }),
            carried_publication,
        );
    }

    let published_at = match atomically_replace_context_template(&path, current_default, clock) {
        Ok(published_at) => published_at,
        Err(error) => {
            log::warn!(
                "[context-templates] replacement failed after backup {} was created: {}",
                backup_path.display(),
                error
            );
            return ContextTemplateExecution::from_parts(Err(error), carried_publication);
        }
    };
    loaded.mark_seeded(spec, &current_default_sha256);
    let replacement = notify.then(|| ContextTemplateReplacement {
        project_path: display_path(project_dir),
        workspace_path: display_path(ac_root),
        file_path: display_path(&path),
        filename: spec.filename.to_string(),
        label: spec.label.to_string(),
        backup_path: display_path(&backup_path),
        local_override_path: display_path(&ac_root.join(local_override_filename(spec.filename))),
    });
    let publication = ContextPublication { published_at };
    ContextTemplateExecution::with_publication(
        Ok(TemplateSyncOutcome {
            pending_update: None,
            replacement,
            target_outcome: TemplatePublication::Published(publication),
        }),
        publication,
    )
}

fn sync_customized_template(
    context: ExistingTemplateSync<'_>,
    return_pending: bool,
    clock: &mut dyn FnMut() -> chrono::DateTime<chrono::Utc>,
) -> ContextTemplateExecution<TemplateSyncOutcome> {
    let ExistingTemplateSync {
        project_dir,
        ac_root,
        path,
        spec,
        loaded,
        snapshot,
        current_default_sha256,
        carried_publication,
    } = context;
    // Root migration authority comes only from exact shipped bytes, independently
    // of absent, stale, or forged persisted state.
    if spec.id == "rootAgent"
        && spec
            .is_known_generated
            .is_some_and(|matches| matches(&snapshot.content))
    {
        return sync_generated_template(
            &path,
            spec,
            &snapshot.sha256,
            loaded,
            &current_default_sha256,
            clock,
        );
    }
    if spec.id == "rootAgent" {
        log::warn!(
            "[context-templates] custom Root context {} is solely authoritative",
            path.display()
        );
    }
    let trusted_entry = loaded.trusted_entry(spec).cloned();
    if let Some(entry) = trusted_entry.as_ref() {
        if entry.last_seeded_sha256.as_deref() == Some(snapshot.sha256.as_str())
            && entry.last_seeded_sha256.as_deref() != Some(current_default_sha256.as_str())
            && spec
                .is_known_generated
                .is_some_and(|matches| matches(&snapshot.content))
        {
            return sync_generated_template(
                &path,
                spec,
                &snapshot.sha256,
                loaded,
                &current_default_sha256,
                clock,
            );
        }
    }

    let has_valid_entry = trusted_entry.is_some();
    if !has_valid_entry
        && spec
            .is_known_generated
            .is_some_and(|matches| matches(&snapshot.content))
    {
        return sync_generated_template(
            &path,
            spec,
            &snapshot.sha256,
            loaded,
            &current_default_sha256,
            clock,
        );
    }

    if spec.suppress_unknown_without_state && !has_valid_entry {
        log::debug!(
            "[context-templates] preserving ambiguous global context template {} without prompting",
            path.display()
        );
        let target_outcome = carried_publication
            .map(TemplatePublication::Published)
            .unwrap_or(TemplatePublication::Skipped(
                ContextTemplateSkipReason::AmbiguousWithoutState,
            ));
        return ContextTemplateExecution::from_parts(
            Ok(TemplateSyncOutcome {
                pending_update: None,
                replacement: None,
                target_outcome,
            }),
            carried_publication,
        );
    }

    if let Some(entry) = trusted_entry.as_ref() {
        if entry.ignored_observed_sha256.as_deref() == Some(snapshot.sha256.as_str())
            && entry.ignored_default_sha256.as_deref() == Some(current_default_sha256.as_str())
        {
            let target_outcome = carried_publication
                .map(TemplatePublication::Published)
                .unwrap_or(TemplatePublication::Skipped(
                    ContextTemplateSkipReason::IgnoredByUser,
                ));
            return ContextTemplateExecution::from_parts(
                Ok(TemplateSyncOutcome {
                    pending_update: None,
                    replacement: None,
                    target_outcome,
                }),
                carried_publication,
            );
        }
    }

    loaded.mark_observed(spec, &snapshot.sha256);
    let pending_update = if return_pending {
        let project_dir = match project_dir {
            Some(project_dir) => project_dir,
            None => {
                return ContextTemplateExecution::from_parts(
                    Err(format!(
                        "Cannot create context template update for {} without a project path",
                        path.display()
                    )),
                    carried_publication,
                )
            }
        };
        Some(make_update(
            project_dir,
            ac_root,
            &path,
            spec,
            snapshot.sha256,
            current_default_sha256,
        ))
    } else {
        log::warn!(
            "[context-templates] preserving customized context template {}; a newer default is \
                available",
            path.display()
        );
        None
    };
    let target_outcome = carried_publication
        .map(TemplatePublication::Published)
        .unwrap_or(TemplatePublication::Observed);
    ContextTemplateExecution::from_parts(
        Ok(TemplateSyncOutcome {
            pending_update,
            replacement: None,
            target_outcome,
        }),
        carried_publication,
    )
}

pub(crate) fn compute_pending_update(
    project_dir: &Path,
    ac_root: &Path,
    spec: SeededContextTemplateSpec,
    loaded: &LoadedState,
) -> Result<Option<ContextTemplateUpdate>, String> {
    // D4: a distribution-owned template is never offered as a pending update.
    if spec.distribution_owned {
        return Ok(None);
    }
    let path = ac_root.join(spec.filename);
    let current_default = (spec.current_content)();
    let current_default_sha256 = sha256_hex(current_default.as_bytes());
    let Some(snapshot) = read_validated_snapshot(&path, "Context template")? else {
        return Ok(None);
    };
    if snapshot.sha256 == current_default_sha256 {
        return Ok(None);
    }

    let trusted_entry = loaded.trusted_entry(spec);
    let has_valid_entry = trusted_entry.is_some();
    if !has_valid_entry
        && spec
            .is_known_generated
            .is_some_and(|matches| matches(&snapshot.content))
    {
        return Ok(None);
    }
    if spec.suppress_unknown_without_state && !has_valid_entry {
        return Ok(None);
    }
    if let Some(entry) = trusted_entry {
        if entry.ignored_observed_sha256.as_deref() == Some(snapshot.sha256.as_str())
            && entry.ignored_default_sha256.as_deref() == Some(current_default_sha256.as_str())
        {
            return Ok(None);
        }
    }

    Ok(Some(make_update(
        project_dir,
        ac_root,
        &path,
        spec,
        snapshot.sha256,
        current_default_sha256,
    )))
}

pub(crate) fn validate_project_ac_root(ac_root: &Path) -> Result<PathBuf, String> {
    validate_existing_dir(ac_root, "Project AC Root")?;
    let name = ac_root.file_name().and_then(|name| name.to_str());
    if name != Some(crate::config::ac_root::canonical_ac_root_label()) {
        return Err(format!(
            "{} is not a Project AC Root directory",
            ac_root.display()
        ));
    }
    ac_root
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("Project AC Root {} has no parent", ac_root.display()))
}

pub(crate) fn validate_project_ac_root_for_scan(
    project_dir: &Path,
    ac_root: &Path,
) -> Result<(), String> {
    validate_existing_dir(ac_root, "Project AC Root")?;
    let expected = crate::config::ac_root::ac_root_for_project(project_dir);
    if ac_root != expected {
        return Err(format!(
            "Project AC Root {} is not the canonical child of {}",
            ac_root.display(),
            project_dir.display()
        ));
    }
    Ok(())
}

pub(crate) fn consume_template_execution(
    spec: SeededContextTemplateSpec,
    execution: ContextTemplateExecution<TemplateSyncOutcome>,
    on_publication: &mut dyn FnMut(&'static str, ContextPublication),
) -> Result<TemplateSyncOutcome, String> {
    if let Some(publication) = execution.published {
        on_publication(spec.filename, publication);
    }
    if let Ok(outcome) = &execution.completion {
        log::debug!(
            "[context-templates] {} target outcome: {:?}",
            spec.filename,
            outcome.target_outcome
        );
    }
    execution.completion
}
