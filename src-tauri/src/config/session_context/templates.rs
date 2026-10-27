use super::{
    classify_legacy_rendered_default_context, find_ac_root, get_default_agent_template,
    record_project_context_publication, CreateOnlyPublication, LegacyRenderedDefaultContext,
    CONTEXT_TEMPLATE_TEMP_COUNTER, COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
    GLOBAL_CONTEXT_TEMPLATE_FILENAME, LEGACY_AGENT_CONTEXT_TEMPLATE_FILENAME,
    LEGACY_AGENT_REPOS_PLACEHOLDER, ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME,
};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

pub fn create_default_context_templates(ac_root: &Path) -> Result<(), String> {
    let mut on_publication =
        |_: &'static str, _: crate::config::seeded_context_templates::ContextPublication| {};
    create_default_context_templates_with_publications(ac_root, &mut on_publication)
}

pub(crate) fn create_default_context_templates_with_publications(
    ac_root: &Path,
    on_publication: &mut dyn FnMut(
        &'static str,
        crate::config::seeded_context_templates::ContextPublication,
    ),
) -> Result<(), String> {
    crate::config::seeded_context_templates::ensure_project_context_templates_with_publications(
        ac_root,
        on_publication,
    )
}

pub(crate) fn write_template_if_missing(
    path: &Path,
    content: &str,
) -> Result<CreateOnlyPublication, String> {
    let mut clock = chrono::Utc::now;
    write_template_if_missing_with_clock(path, content, &mut clock)
}

pub(crate) fn write_template_if_missing_with_clock(
    path: &Path,
    content: &str,
    clock: &mut dyn FnMut() -> chrono::DateTime<chrono::Utc>,
) -> Result<CreateOnlyPublication, String> {
    write_template_if_missing_with(
        path,
        content,
        |path| crate::filesystem::context_templates::create_template_stage(path),
        clock,
    )
}

pub(crate) fn write_template_if_missing_with<W, F>(
    path: &Path,
    content: &str,
    open_new: F,
    clock: &mut dyn FnMut() -> chrono::DateTime<chrono::Utc>,
) -> Result<CreateOnlyPublication, String>
where
    W: ContextTemplateWriter,
    F: FnOnce(&Path) -> std::io::Result<W>,
{
    match crate::filesystem::context_templates::inspect_template_source(path) {
        Ok(_) => return Ok(CreateOnlyPublication::AlreadyPresent),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(format!(
                "failed to inspect context template {}: {}",
                path.display(),
                e
            ))
        }
    }

    let temp_path = unique_context_template_temp_path(path);
    let mut file = open_new(&temp_path).map_err(|e| {
        format!(
            "failed to create temporary context template {}: {}",
            temp_path.display(),
            e
        )
    })?;

    if let Err(e) = file.write_all(content.as_bytes()) {
        drop(file);
        cleanup_failed_context_template(&temp_path);
        return Err(format!(
            "failed to write context template {}: {}",
            path.display(),
            e
        ));
    }
    if let Err(e) = file.flush() {
        drop(file);
        cleanup_failed_context_template(&temp_path);
        return Err(format!(
            "failed to flush context template {}: {}",
            path.display(),
            e
        ));
    }
    if let Err(e) = file.sync_all() {
        drop(file);
        cleanup_failed_context_template(&temp_path);
        return Err(format!(
            "failed to sync context template {}: {}",
            path.display(),
            e
        ));
    }
    drop(file);

    match crate::filesystem::context_templates::publish_template_link(&temp_path, path) {
        Ok(()) => {
            let published_at = clock();
            cleanup_failed_context_template(&temp_path);
            Ok(CreateOnlyPublication::Published { published_at })
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            cleanup_failed_context_template(&temp_path);
            Ok(CreateOnlyPublication::AlreadyPresent)
        }
        Err(e) => {
            cleanup_failed_context_template(&temp_path);
            Err(format!(
                "failed to publish context template {}: {}",
                path.display(),
                e
            ))
        }
    }
}

pub(crate) trait ContextTemplateWriter: Write {
    fn sync_all(&mut self) -> std::io::Result<()>;
}

impl ContextTemplateWriter for crate::filesystem::context_templates::TemplateFile {
    fn sync_all(&mut self) -> std::io::Result<()> {
        crate::filesystem::context_templates::TemplateFile::sync_all(self)
    }
}

pub(crate) fn unique_context_template_temp_path(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Context.template.md");
    let counter = CONTEXT_TEMPLATE_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let process_id = std::process::id();
    parent.join(format!(".{file_name}.{process_id}.{counter}.tmp"))
}

pub(crate) fn cleanup_failed_context_template(path: &Path) {
    if let Err(e) = crate::filesystem::context_templates::remove_template_entry(path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            log::warn!(
                "failed to remove incomplete context template {} after write failure: {}",
                path.display(),
                e
            );
        }
    }
}

pub(crate) fn resolve_ac_root_context_dir(agent_root: &Path) -> Option<PathBuf> {
    // #979 F.2: the `ac-root-agent` -> parent fallback is deleted. This is NOT
    // sufficient on its own and is NOT what protects the project global.
    // `find_ac_root` is the first branch and matches ANY `.ac` ancestor,
    // and `config_dir()` sits inside a `.ac` tree in the dev and workgroup
    // layouts, so there the deletion is a no-op. It closes the installed layout
    // (no `.ac` ancestor) and removes a misleading affordance. The load-bearing
    // defenses are the `resolve_agent_context` guard and the cache-writer branch
    // in `ensure_session_context_with_config`.
    find_ac_root(agent_root)
}

/// #1737 (D22) - the single site in this file that renders an overlay
/// diagnostic. Twin of `settings.rs`'s `report_overlay_diagnostics`; the same
/// three-line match, so the "one site per source file" claim holds for both files.
pub(crate) fn report_overlay_diagnostic(
    diagnostic: &crate::config::local_overlay::OverlayDiagnostic,
) {
    use crate::config::local_overlay::OverlayDiagnosticLevel;
    match diagnostic.level() {
        OverlayDiagnosticLevel::Error => log::error!("{}", diagnostic.render()),
        OverlayDiagnosticLevel::Info => log::info!("{}", diagnostic.render()),
    }
}

/// #1737: render a rejected `.local.md` override through its typed diagnostic
/// (D9, D22) and fall back to the base content.
pub(crate) fn context_local_override(base: &Path) -> Option<String> {
    let override_value = crate::config::local_overlay::read_markdown_override(base);
    if let Some(diagnostic) = override_value.diagnostic() {
        report_overlay_diagnostic(&diagnostic);
    }
    match override_value {
        crate::config::local_overlay::MarkdownOverride::Present(content) => Some(content),
        _ => None,
    }
}

/// #1737: the operator's `.local.md` override for a managed context template read
/// from `agent_root`'s context directory, or `None`.
pub(crate) fn read_context_local_override(agent_root: &str, filename: &str) -> Option<String> {
    let context_dir = resolve_ac_root_context_dir(Path::new(agent_root))?;
    context_local_override(&context_dir.join(filename))
}

/// #1737: the `.local.md` override for a resolved context file, but only for the
/// three managed context templates. Every other context file is read verbatim.
pub(crate) fn managed_template_local_override(base: &Path) -> Option<String> {
    let name = base.file_name()?.to_str()?;
    if name != COORDINATOR_CONTEXT_TEMPLATE_FILENAME
        && name != GLOBAL_CONTEXT_TEMPLATE_FILENAME
        && name != ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME
    {
        return None;
    }
    context_local_override(base)
}

pub(crate) fn read_context_template(
    agent_root: &str,
    filename: &str,
) -> Result<Option<String>, String> {
    let Some(context_dir) = resolve_ac_root_context_dir(Path::new(agent_root)) else {
        return Ok(None);
    };
    let path = context_dir.join(filename);
    let metadata = match crate::filesystem::context_templates::inspect_template_source(&path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(format!(
                "Failed to inspect context template {}: {}",
                path.display(),
                e
            ))
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "Context template {} exists but is not a regular file",
            path.display()
        ));
    }
    let bytes = crate::filesystem::context_templates::read_template_bytes(&path)
        .map_err(|e| format!("Failed to read context template {}: {}", path.display(), e))?;
    String::from_utf8(bytes).map(Some).map_err(|e| {
        format!(
            "Context template {} is not valid UTF-8: {}",
            path.display(),
            e
        )
    })
}

pub(crate) fn read_or_create_context_template(
    agent_root: &str,
    filename: &str,
    default_content: &str,
) -> Result<Option<String>, String> {
    let mut on_publication =
        |_: &'static str, _: crate::config::seeded_context_templates::ContextPublication| {};
    read_or_create_context_template_with_publications(
        agent_root,
        filename,
        default_content,
        &mut on_publication,
    )
}

pub(crate) fn read_or_create_context_template_with_publications(
    agent_root: &str,
    filename: &str,
    default_content: &str,
    on_publication: &mut dyn FnMut(
        &'static str,
        crate::config::seeded_context_templates::ContextPublication,
    ),
) -> Result<Option<String>, String> {
    read_or_create_context_template_with_sync(
        agent_root,
        filename,
        default_content,
        on_publication,
        |context_dir, filename, on_publication| {
            crate::config::seeded_context_templates::
                sync_project_context_template_for_read_with_publications(
                    context_dir,
                    filename,
                    on_publication,
                )
        },
    )
}

pub(crate) fn read_or_create_context_template_with_sync<F>(
    agent_root: &str,
    filename: &str,
    default_content: &str,
    on_publication: &mut dyn FnMut(
        &'static str,
        crate::config::seeded_context_templates::ContextPublication,
    ),
    synchronize: F,
) -> Result<Option<String>, String>
where
    F: FnOnce(
        &Path,
        &str,
        &mut dyn FnMut(&'static str, crate::config::seeded_context_templates::ContextPublication),
    ) -> Result<(), String>,
{
    let Some(context_dir) = resolve_ac_root_context_dir(Path::new(agent_root)) else {
        return Ok(None);
    };
    if filename == GLOBAL_CONTEXT_TEMPLATE_FILENAME {
        migrate_legacy_agent_context_template(&context_dir)?;
    }
    let is_managed_project_template = filename == GLOBAL_CONTEXT_TEMPLATE_FILENAME
        || filename == COORDINATOR_CONTEXT_TEMPLATE_FILENAME;
    if is_managed_project_template {
        if let Err(e) = synchronize(&context_dir, filename, on_publication) {
            log::warn!(
                "[context-templates] failed to synchronize {} before reading: {}",
                context_dir.join(filename).display(),
                e
            );
        }
    }
    if let Some(content) = read_context_template(agent_root, filename)? {
        return Ok(Some(content));
    }
    if is_managed_project_template {
        return Ok(None);
    }
    let _ = write_template_if_missing(&context_dir.join(filename), default_content)?;
    read_context_template(agent_root, filename)
}

/// #1065 Stage F: read/sync a managed project context template under the project
/// seed-manifest gate, recording any create/recognized-update at its commit point
/// (plan sections 5.2/6.3). Ordering by gate state:
/// * held: synchronize (write) under the gate, then record the publication.
/// * pre-contention degradation: synchronize untracked (write, no manifest row).
/// * contention/unsafe: skip the synchronize write and never fall back to an
///   ungated write; use the existing file, or the in-memory default for a managed
///   template that is still absent (plan section 5.2).
///
/// `activation` is `None` for a test or unactivated caller, which runs the plain
/// ungated read/sync unchanged. A path with no resolvable project workspace runs
/// ungated too.
pub(crate) fn read_or_create_context_recorded(
    agent_root: &str,
    filename: &str,
    default_content: &str,
    activation: Option<&crate::config::seed_manifest::ManifestActivationToken>,
) -> Result<Option<String>, String> {
    use crate::config::seed_manifest::SoftProjectGate;
    use crate::config::seeded_context_templates::ContextPublication;
    let Some(token) = activation else {
        return read_or_create_context_template(agent_root, filename, default_content);
    };
    let Some(project_root) = resolve_ac_root_context_dir(Path::new(agent_root))
        .and_then(|context_dir| context_dir.parent().map(Path::to_path_buf))
    else {
        return read_or_create_context_template(agent_root, filename, default_content);
    };
    match crate::config::seed_manifest::acquire_project_gate_soft(&project_root) {
        SoftProjectGate::Held(mut guard) => {
            let result = {
                let mut on_publication = |fname: &'static str, publication: ContextPublication| {
                    record_project_context_publication(
                        &mut guard,
                        token,
                        fname,
                        publication.published_at,
                    );
                };
                read_or_create_context_template_with_publications(
                    agent_root,
                    filename,
                    default_content,
                    &mut on_publication,
                )
            };
            guard.release();
            result
        }
        SoftProjectGate::DegradedUntracked => {
            read_or_create_context_template(agent_root, filename, default_content)
        }
        SoftProjectGate::Unavailable(_) => {
            read_or_create_context_template_skip_sync(agent_root, filename, default_content)
        }
    }
}

/// #1065 Stage F: read a managed project context template WITHOUT synchronizing it,
/// used when the project gate is contended so the write is skipped rather than
/// racing a cooperating writer. A managed template that is still absent yields
/// `Ok(None)` (the caller uses the in-memory default); it is never created ungated.
pub(crate) fn read_or_create_context_template_skip_sync(
    agent_root: &str,
    filename: &str,
    default_content: &str,
) -> Result<Option<String>, String> {
    let mut on_publication =
        |_: &'static str, _: crate::config::seeded_context_templates::ContextPublication| {};
    read_or_create_context_template_with_sync(
        agent_root,
        filename,
        default_content,
        &mut on_publication,
        |_context_dir, _filename, _on_publication| Ok(()),
    )
}

pub(crate) fn migrate_legacy_agent_context_template(context_dir: &Path) -> Result<(), String> {
    migrate_legacy_agent_context_template_with(context_dir, |legacy_path, new_path| {
        crate::filesystem::context_templates::publish_template_link(legacy_path, new_path)
    })
}

pub(crate) fn migrate_legacy_agent_context_template_with<F>(
    context_dir: &Path,
    publish_no_overwrite: F,
) -> Result<(), String>
where
    F: FnOnce(&Path, &Path) -> std::io::Result<()>,
{
    let new_path = context_dir.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    match crate::filesystem::context_templates::inspect_template_source(&new_path) {
        Ok(_) => return Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(format!(
                "Failed to inspect context template {}: {}",
                new_path.display(),
                e
            ))
        }
    }

    let legacy_path = context_dir.join(LEGACY_AGENT_CONTEXT_TEMPLATE_FILENAME);
    match crate::filesystem::context_templates::inspect_template_source(&legacy_path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(format!(
                    "Legacy context template {} exists but is not a regular file",
                    legacy_path.display()
                ));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(format!(
                "Failed to inspect legacy context template {}: {}",
                legacy_path.display(),
                e
            ))
        }
    }

    match publish_no_overwrite(&legacy_path, &new_path) {
        Ok(()) => {
            if let Err(e) =
                crate::filesystem::context_templates::remove_template_entry(&legacy_path)
            {
                log::warn!(
                    "Migrated legacy context template {} to {}, but failed to remove legacy file: \
                        {}",
                    legacy_path.display(),
                    new_path.display(),
                    e
                );
            }
            log::info!(
                "Migrated legacy context template {} to {}",
                legacy_path.display(),
                new_path.display()
            );
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(format!(
            "Failed to migrate legacy context template {} to {}: {}",
            legacy_path.display(),
            new_path.display(),
            e
        )),
    }
}

/// Return `content` with a leading `---\n…\n---` YAML frontmatter block removed.
/// If there is no frontmatter, returns `content` (post-BOM) unchanged.
///
/// Strips a leading UTF-8 BOM FIRST, then treats the input as frontmatter only
/// if it begins with exactly `---` immediately followed by a newline (`---\n`
/// or `---\r\n`) — this avoids mistaking a body that opens with a Markdown
/// `---` horizontal rule for frontmatter. If no closing `---` line is found,
/// returns `content` (post-BOM) unchanged.
pub(crate) fn strip_yaml_frontmatter(content: &str) -> &str {
    let content = content.strip_prefix('\u{FEFF}').unwrap_or(content);
    let after_open = match content
        .strip_prefix("---\n")
        .or_else(|| content.strip_prefix("---\r\n"))
    {
        Some(rest) => rest,
        None => return content,
    };
    // Scan for a closing `---` line; return the body after it. If there is no
    // closing line, the input was not real frontmatter — return it unchanged.
    let mut offset = 0;
    for line in after_open.split_inclusive('\n') {
        let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
        if trimmed == "---" {
            return &after_open[offset + line.len()..];
        }
        offset += line.len();
    }
    content
}

/// #1065 Stage F: self-heal a stale generated global context template under the
/// project seed-manifest gate, recording the resulting publication (plan sections
/// 5.2/6.3). Ordering by gate state:
/// * held: heal (atomic replace) under the gate, then record the publication.
/// * pre-contention degradation: heal untracked (write, no manifest row).
/// * contention/unsafe: skip the heal write so it never races a cooperating writer;
///   the caller's in-memory render is already correct and the file heals on a later
///   spawn once the gate is free.
///
/// `activation` is `None` for a test or unactivated caller, which runs the plain
/// ungated heal; a path with no resolvable project workspace runs ungated too.
pub(crate) fn heal_stale_global_recorded(
    agent_root: &str,
    matrix_root: Option<&str>,
    skills_section: &str,
    activation: Option<&crate::config::seed_manifest::ManifestActivationToken>,
) -> crate::config::seeded_context_templates::ContextTemplateExecution<
    crate::config::seeded_context_templates::TemplatePublication,
> {
    use crate::config::seed_manifest::SoftProjectGate;
    let Some(token) = activation else {
        return heal_stale_global_context_template(agent_root, matrix_root, skills_section);
    };
    let Some(project_root) = resolve_ac_root_context_dir(Path::new(agent_root))
        .and_then(|context_dir| context_dir.parent().map(Path::to_path_buf))
    else {
        return heal_stale_global_context_template(agent_root, matrix_root, skills_section);
    };
    match crate::config::seed_manifest::acquire_project_gate_soft(&project_root) {
        SoftProjectGate::Held(mut guard) => {
            let execution =
                heal_stale_global_context_template(agent_root, matrix_root, skills_section);
            if let Some(publication) = &execution.published {
                record_project_context_publication(
                    &mut guard,
                    token,
                    GLOBAL_CONTEXT_TEMPLATE_FILENAME,
                    publication.published_at,
                );
            }
            guard.release();
            execution
        }
        SoftProjectGate::DegradedUntracked => {
            heal_stale_global_context_template(agent_root, matrix_root, skills_section)
        }
        SoftProjectGate::Unavailable(_) => {
            crate::config::seeded_context_templates::ContextTemplateExecution::failed(
                "self-heal skipped: project seed-manifest gate unavailable".to_string(),
            )
        }
    }
}

/// Best-effort on-disk self-heal (#664). The caller has already classified the
/// workspace global-context template as `StaleGenerated`, i.e. a provably
/// UNMODIFIED generated legacy default for these paths. Rewrite it to the
/// current tokenized default so future sessions read a clean template and
/// classify `NotLegacy` (normal token substitution, no recognition cost).
///
/// Safety:
/// - Re-validates the on-disk content under the SAME exact classifier
///   immediately before the swap. This NARROWS the read->write TOCTOU window to
///   a microsecond re-validate->publish gap; it does NOT close it (a user save
///   landing inside that residual gap is still clobbered, which is irreducible
///   without an OS file lock or a compare-and-swap publish, neither of which
///   `ReplaceFileW` provides). Residual risk accepted: identical-content write
///   to a single recognized-default target. If a user edited the file before
///   the re-check, it no longer returns `StaleGenerated` and we abort, never
///   clobbering the edit.
/// - Atomic replace (temp + fsync + drop-handle + rename/ReplaceFileW + dir
///   fsync).
/// - Any failure logs a warning and returns; the caller's in-memory render is
///   already correct, so a heal failure never breaks the session. The safe
///   failure mode is to do nothing. There is no backoff and no tried-once
///   marker, so a workspace that cannot be healed (read-only dir, AV lock on
///   the temp or dest) re-attempts on every resolve and re-pays the doubled
///   classify cost; this is an accepted best-effort tradeoff, never a failure
///   that reaches the agent.
pub(crate) fn heal_stale_global_context_template(
    agent_root: &str,
    matrix_root: Option<&str>,
    skills_section: &str,
) -> crate::config::seeded_context_templates::ContextTemplateExecution<
    crate::config::seeded_context_templates::TemplatePublication,
> {
    let mut clock = chrono::Utc::now;
    heal_stale_global_context_template_with_clock(
        agent_root,
        matrix_root,
        skills_section,
        &mut clock,
    )
}

pub(crate) fn heal_stale_global_context_template_with_clock(
    agent_root: &str,
    matrix_root: Option<&str>,
    skills_section: &str,
    clock: &mut dyn FnMut() -> chrono::DateTime<chrono::Utc>,
) -> crate::config::seeded_context_templates::ContextTemplateExecution<
    crate::config::seeded_context_templates::TemplatePublication,
> {
    use crate::config::seeded_context_templates::{
        ContextPublication, ContextTemplateExecution, ContextTemplateSkipReason,
        TemplatePublication,
    };

    let Some(context_dir) = resolve_ac_root_context_dir(Path::new(agent_root)) else {
        return ContextTemplateExecution::completed(TemplatePublication::Skipped(
            ContextTemplateSkipReason::AcRootUnavailable,
        ));
    };
    let path = context_dir.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);

    // TOCTOU re-validation: re-read with the RAW reader (no migrate, no
    // create-if-missing side effects) and re-classify under the exact contract.
    let current = match read_context_template(agent_root, GLOBAL_CONTEXT_TEMPLATE_FILENAME) {
        Ok(Some(content)) => content,
        Ok(None) => {
            return ContextTemplateExecution::completed(TemplatePublication::Skipped(
                ContextTemplateSkipReason::TargetMissing,
            ))
        }
        Err(e) => {
            return ContextTemplateExecution::failed(format!(
                "cannot re-read {}: {}",
                path.display(),
                e
            ));
        }
    };
    if !matches!(
        classify_legacy_rendered_default_context(&current, agent_root, matrix_root, skills_section),
        LegacyRenderedDefaultContext::StaleGenerated
    ) {
        // Changed under us, or no longer recognized as a generated legacy
        // default: preserve whatever is there, do nothing.
        return ContextTemplateExecution::completed(TemplatePublication::ChangedUnderUs);
    }

    let published_at =
        match atomically_replace_context_template(&path, get_default_agent_template(), clock) {
            Ok(published_at) => published_at,
            Err(error) => {
                return ContextTemplateExecution::failed(format!(
                    "failed to regenerate stale global context template {}: {}",
                    path.display(),
                    error
                ))
            }
        };
    log::info!(
        "#664 self-heal: regenerated stale global context template {} to the current default",
        path.display()
    );
    let publication = ContextPublication { published_at };
    ContextTemplateExecution::with_publication(
        Ok(TemplatePublication::Published(publication)),
        publication,
    )
}

/// Atomically replace `path` with `content`: write a unique temp file in the
/// SAME directory, fsync it, drop the handle, then publish via the shared
/// `root_agent::atomic_replace_existing` primitive (plain rename on Unix;
/// rename-if-absent else `ReplaceFileW(REPLACEFILE_WRITE_THROUGH)` on Windows).
/// The temp file is cleaned up on every failure path.
pub(crate) fn atomically_replace_context_template(
    path: &Path,
    content: &str,
    clock: &mut dyn FnMut() -> chrono::DateTime<chrono::Utc>,
) -> Result<chrono::DateTime<chrono::Utc>, String> {
    atomically_replace_context_template_with(path, content, clock, sync_context_template_parent)
}

pub(crate) fn atomically_replace_context_template_with<S>(
    path: &Path,
    content: &str,
    clock: &mut dyn FnMut() -> chrono::DateTime<chrono::Utc>,
    sync_parent: S,
) -> Result<chrono::DateTime<chrono::Utc>, String>
where
    S: FnOnce(&Path),
{
    let temp = unique_context_template_temp_path(path);
    let mut file =
        crate::filesystem::context_templates::create_template_stage(&temp).map_err(|e| {
            format!(
                "Failed to create temporary context template {}: {}",
                temp.display(),
                e
            )
        })?;

    if let Err(e) = file.write_all(content.as_bytes()) {
        drop(file);
        cleanup_failed_context_template(&temp);
        return Err(format!(
            "Failed to write temporary context template {}: {}",
            temp.display(),
            e
        ));
    }
    if let Err(e) = file.flush() {
        drop(file);
        cleanup_failed_context_template(&temp);
        return Err(format!(
            "Failed to flush temporary context template {}: {}",
            temp.display(),
            e
        ));
    }
    if let Err(e) = file.sync_all() {
        drop(file);
        cleanup_failed_context_template(&temp);
        return Err(format!(
            "Failed to sync temporary context template {}: {}",
            temp.display(),
            e
        ));
    }
    // CRITICAL (G1): drop the temp handle on the SUCCESS path BEFORE the publish.
    // On Windows, ReplaceFileW (and std::fs::rename) over a still-open source
    // fails with a sharing violation, so the heal would return Err on every
    // attempt and silently never converge on the primary platform. Mirrors
    // `root_agent::atomic_write_role`, which drops at root_agent.rs:696 before
    // `replace_role_file`.
    drop(file);

    if let Err(e) = super::super::root_agent::atomic_replace_existing(&temp, path) {
        cleanup_failed_context_template(&temp);
        return Err(e);
    }
    let published_at = clock();

    if let Some(parent) = path.parent() {
        sync_parent(parent);
    }

    Ok(published_at)
}

pub(crate) fn sync_context_template_parent(parent: &Path) {
    if let Ok(dir) = crate::filesystem::context_templates::open_template_directory(parent) {
        if let Err(e) = dir.sync_all() {
            log::warn!(
                "#664 self-heal: failed to sync context template directory {}: {}",
                parent.display(),
                e
            );
        }
    }
}

pub(crate) const MANDATORY_GLOBAL_CONTEXT_PLACEHOLDERS: &[&str] = &[
    "{{WRITE_RESTRICTIONS}}",
    "{{INTER_AGENT_MESSAGING}}",
    "{{SESSION_CREDENTIALS}}",
    "{{CLI_CONTEXT}}",
    "{{HOST_PLATFORM_RULES}}",
    "{{SKILLS_SECTION}}",
    "{{AGENT_REPOS}}",
    "{{DELEGATED_TASK_REPORTING}}",
];

/// The unique top-level Markdown heading that each mandatory coarse placeholder's
/// rendered block emits. Used by the append-fallback to detect a legacy *inline*
/// template (section written without its coarse token) so the fallback does not
/// emit a second copy (#658). Returns `None` for anything not in the mandatory
/// set. The headings were verified against each block's source; none collides
/// with another token's section.
pub(crate) fn mandatory_section_heading(placeholder: &str) -> Option<&'static str> {
    Some(match placeholder {
        "{{WRITE_RESTRICTIONS}}" => "## GOLDEN RULE",
        "{{INTER_AGENT_MESSAGING}}" => "## Inter-Agent Messaging",
        "{{SESSION_CREDENTIALS}}" => "## Session credentials",
        "{{CLI_CONTEXT}}" => "## CLI executable",
        "{{HOST_PLATFORM_RULES}}" => "## Host Platform Rules",
        "{{SKILLS_SECTION}}" => "## Skills",
        "{{AGENT_REPOS}}" => "# Agent Repos",
        "{{DELEGATED_TASK_REPORTING}}" => "## Delegated Task Reporting",
        _ => return None,
    })
}

/// #1369 (C4): pre-rename spellings that still SATISFY a mandatory placeholder.
/// A template on disk carrying only the legacy token must not receive a second,
/// appended copy of the same block; the replace chain fills the legacy token in
/// place. Empty for every other placeholder.
pub(crate) fn mandatory_placeholder_aliases(placeholder: &str) -> &'static [&'static str] {
    match placeholder {
        "{{AGENT_REPOS}}" => &[LEGACY_AGENT_REPOS_PLACEHOLDER],
        _ => &[],
    }
}

/// True when `template` already contains the section that `placeholder` would
/// render, detected by a LINE-ANCHORED heading match (a trimmed line), never a
/// raw substring. The substring form is unsafe: e.g. a legacy hybrid template's
/// Self-discovery prose references "`## Inter-Agent Messaging`" inside backticks
/// mid-line, so a raw `contains` would false-positive and skip a genuinely-
/// missing messaging section. A truly-incomplete template (section heading
/// absent) returns false here, so the safety-net append still fires (#658).
///
/// `## GOLDEN RULE` is matched by prefix because its rendered heading carries a
/// trailing descriptor; every other heading is matched exactly.
pub(crate) fn mandatory_section_present_inline(template: &str, placeholder: &str) -> bool {
    let Some(heading) = mandatory_section_heading(placeholder) else {
        return false;
    };
    template.lines().map(str::trim).any(|line| {
        if heading == "## GOLDEN RULE" {
            line.starts_with(heading)
        } else {
            line == heading
        }
    })
}

/// Tokenization signals captured ONCE from the ORIGINAL template BEFORE the
/// append loop runs (the loop's own `push_str(placeholder)` re-introduces `{{`
/// coarse tokens mid-iteration, which would corrupt `has_any_placeholder`).
/// Used by `coarse_section_dedup_safe` to decide whether an inline copy is
/// current for THIS agent (#658 round-3).
pub(crate) struct TemplateTokenSignals {
    /// The original template carried at least one `{{placeholder}}` => it is a
    /// tokenized template (current default or the #658 inline hybrid), NOT a
    /// fully-baked legacy-rendered template whose inline copies hold another
    /// agent's literal values.
    pub(crate) has_any_placeholder: bool,
    /// `{{AGENT_ROOT}}` present => the inline Golden Rule is tokenized (its write
    /// paths are refilled by the replace chain), not baked. Unique to the
    /// write-restrictions section; absent from the coarse default and any baked
    /// template.
    pub(crate) has_agent_root: bool,
    /// `{{PEER_NAME_FORMAT}}` / `{{SEND_MESSAGE_INSTRUCTIONS}}` present => the
    /// inline Inter-Agent Messaging section is tokenized, not baked. Unique to
    /// that section.
    pub(crate) has_messaging_tokens: bool,
}

impl TemplateTokenSignals {
    pub(crate) fn capture(template: &str) -> Self {
        Self {
            has_any_placeholder: template.contains("{{"),
            has_agent_root: template.contains("{{AGENT_ROOT}}"),
            has_messaging_tokens: template.contains("{{PEER_NAME_FORMAT}}")
                || template.contains("{{SEND_MESSAGE_INSTRUCTIONS}}"),
        }
    }
}

/// Whether the append-fallback may DEDUP (skip appending) `placeholder` when its
/// section heading is already present inline. Encodes the dedup-safety invariant
/// (#658, section 3): dedup only when the inline copy is guaranteed current for
/// THIS agent. A genuinely-baked or Root-sensitive section returns false here so
/// the current block is re-appended instead of trusting the stale inline copy.
pub(crate) fn coarse_section_dedup_safe(
    placeholder: &str,
    is_root_agent: bool,
    signals: &TemplateTokenSignals,
) -> bool {
    match placeholder {
        // Static blocks: no AGENT-specific content, so an inline copy is never
        // stale FOR THIS AGENT (it may differ from the current default wording;
        // preserving the inline copy is the accepted tradeoff, matching the
        // edited-legacy preservation behavior). Gated on `has_any_placeholder` so
        // a fully-baked legacy template renders exactly as today (no dedup;
        // everything appends).
        "{{CLI_CONTEXT}}" | "{{SESSION_CREDENTIALS}}" | "{{DELEGATED_TASK_REPORTING}}" => {
            signals.has_any_placeholder
        }
        // Golden Rule: never on the Root path (root-only baked sub-sections,
        // HIGH-1); otherwise only when the inline Golden Rule is tokenized so its
        // write paths are refilled.
        "{{WRITE_RESTRICTIONS}}" => !is_root_agent && signals.has_agent_root,
        // Messaging: only when the inline section is tokenized (peer/path bits
        // refilled).
        "{{INTER_AGENT_MESSAGING}}" => signals.has_messaging_tokens,
        // Coarse dynamic lists: reaching the append branch means the coarse token
        // is absent, so any inline copy is a baked/stale list -> never dedup,
        // always append the current block.
        "{{SKILLS_SECTION}}" | "{{AGENT_REPOS}}" => false,
        // #1605: never dedup an inline `## Host Platform Rules` copy: the block is
        // platform- and file-dependent, so an inline baked copy can be stale or
        // platform-wrong; appending the current block is the safe side (mirrors
        // SKILLS_SECTION/AGENT_REPOS). No existing template generation carries the
        // heading, so no real-world duplication can occur.
        "{{HOST_PLATFORM_RULES}}" => false,
        _ => false,
    }
}

impl ContextTemplateWriter for std::fs::File {
    fn sync_all(&mut self) -> std::io::Result<()> {
        crate::filesystem::context_templates::sync_template_test_writer(self)
    }
}
