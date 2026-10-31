mod legacy_templates;
mod overwrite;
mod retire;
mod specs;
mod state;
mod sync;

use legacy_templates::{
    COORDINATOR_CONTEXT_TEMPLATE_BEFORE_CROSS_WORKGROUP_RULE,
    COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ORCHESTRATOR_RENAME,
    COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME,
    COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION,
    GLOBAL_CONTEXT_TEMPLATE_BEFORE_AGENT_REPOS, GLOBAL_CONTEXT_TEMPLATE_BEFORE_ANSWERING_SECTION,
    GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES, GLOBAL_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME,
    GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION,
    GLOBAL_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION,
    OLD_COORDINATOR_CONTEXT_TEMPLATE_BEFORE_RAISE_HAND,
    STANDALONE_GLOBAL_CONTEXT_BEFORE_CORE_CONCEPTS,
};
use overwrite::create_backup;
#[cfg(test)]
use overwrite::overwrite_context_template_with_default_with;
pub(crate) use overwrite::overwrite_context_template_with_default_with_publications;
pub use overwrite::{
    dedupe_context_template_updates, dismiss_context_template_update,
    overwrite_context_template_with_default,
};
pub(crate) use retire::retire_standalone_global_context;
#[cfg(test)]
use retire::retire_standalone_global_context_with;
use retire::validate_expected_hashes;
use specs::{
    actionable_project_spec_by_filename, is_known_generated_standalone_global_template,
    platform_specs, project_spec_by_filename, project_specs, root_spec,
};
#[cfg(test)]
use specs::{
    is_known_generated_coordinator_template, is_known_generated_platform_windows,
    HOST_PLATFORM_RULES_WINDOWS_BEFORE_GIT_BASH_DIRECT,
};
use state::{
    display_path, is_link_or_reparse, load_state, local_override_filename, persist_state,
    persist_state_best_effort, persist_state_strict, read_validated_snapshot, sha256_hex,
    validate_existing_dir, validate_existing_file, LoadedState, SeededContextTemplateSpec,
};
#[cfg(test)]
use state::{SeededContextTemplateEntry, SeededContextTemplateState};
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
#[cfg(test)]
use sync::{
    auto_update_generated_template, auto_update_generated_template_with,
    auto_update_generated_template_with_backup, create_missing_template,
};
use sync::{
    compute_pending_update, consume_template_execution, sync_one_template,
    validate_project_ac_root, validate_project_ac_root_for_scan,
};

pub const SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME: &str =
    crate::config::instance_artifacts::SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME;

const STATE_SCHEMA_VERSION: u32 = 1;
const CONTEXT_TEMPLATE_CHANGED: &str =
    "Context template changed on disk; reload the project before overwriting.";
const CONTEXT_TEMPLATE_DEFAULT_CHANGED: &str =
    "Context template default changed; reload the project before overwriting.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ContextPublication {
    pub(crate) published_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextTemplateSkipReason {
    CreationDisabled,
    MissingAfterCreate,
    AmbiguousWithoutState,
    IgnoredByUser,
    AcRootUnavailable,
    TargetMissing,
    /// #1748: a distribution-owned template differs from the current default on a
    /// path that cannot report the replacement; the repair is left to the scan.
    DistributionRepairDeferred,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TemplatePublication {
    Published(ContextPublication),
    AlreadyCurrent,
    ChangedUnderUs,
    Observed,
    Skipped(ContextTemplateSkipReason),
}

#[derive(Debug, Clone)]
pub(crate) struct ContextTemplateExecution<T> {
    pub(crate) completion: Result<T, String>,
    pub(crate) published: Option<ContextPublication>,
}

impl<T> ContextTemplateExecution<T> {
    pub(crate) fn from_parts(
        completion: Result<T, String>,
        published: Option<ContextPublication>,
    ) -> Self {
        Self {
            completion,
            published,
        }
    }

    pub(crate) fn completed(value: T) -> Self {
        Self::from_parts(Ok(value), None)
    }

    pub(crate) fn failed(error: String) -> Self {
        Self::from_parts(Err(error), None)
    }

    pub(crate) fn with_publication(
        completion: Result<T, String>,
        publication: ContextPublication,
    ) -> Self {
        Self::from_parts(completion, Some(publication))
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TemplateSyncOutcome {
    pub(crate) pending_update: Option<ContextTemplateUpdate>,
    pub(crate) replacement: Option<ContextTemplateReplacement>,
    pub(crate) target_outcome: TemplatePublication,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextTemplateUpdate {
    pub project_path: String,
    pub workspace_path: String,
    pub file_path: String,
    pub filename: String,
    pub label: String,
    pub current_file_sha256: String,
    pub current_default_sha256: String,
    pub current_default_version: u32,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextTemplateOverwriteResult {
    pub file_path: String,
    pub backup_path: String,
    pub current_default_sha256: String,
}

/// #1748: everything a caller needs to tell the user that AgentsCommander replaced
/// the bytes they had on disk. Produced ONLY when the replaced bytes were not ours.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextTemplateReplacement {
    pub project_path: String,
    pub workspace_path: String,
    pub file_path: String,
    pub filename: String,
    pub label: String,
    pub backup_path: String,
    pub local_override_path: String,
}

pub fn ensure_project_context_templates(ac_root: &Path) -> Result<(), String> {
    let mut on_publication = |_: &'static str, _: ContextPublication| {};
    ensure_project_context_templates_with_publications(ac_root, &mut on_publication)
}

pub(crate) fn ensure_project_context_templates_with_publications(
    ac_root: &Path,
    on_publication: &mut dyn FnMut(&'static str, ContextPublication),
) -> Result<(), String> {
    let mut clock = chrono::Utc::now;
    ensure_project_context_templates_with_clock(ac_root, &mut clock, on_publication)
}

fn ensure_project_context_templates_with_clock(
    ac_root: &Path,
    clock: &mut dyn FnMut() -> chrono::DateTime<chrono::Utc>,
    on_publication: &mut dyn FnMut(&'static str, ContextPublication),
) -> Result<(), String> {
    crate::filesystem::context_templates::ensure_template_directory(ac_root).map_err(|e| {
        format!(
            "failed to create context templates directory {}: {}",
            ac_root.display(),
            e
        )
    })?;
    validate_existing_dir(ac_root, "Context template directory")?;
    // #1795: the four project-level shared directories are created when a
    // project's `.ac` root is bootstrapped. Hard error, like the
    // `create_dir_all(ac_root)` two lines above and on the same root: an `.ac`
    // root that cannot take a subdirectory is already a broken bootstrap.
    crate::config::shared_locations::create_project_shared_dirs(ac_root).map_err(|e| {
        format!(
            "failed to create project shared directories under {}: {}",
            ac_root.display(),
            e
        )
    })?;
    let skills_result = crate::config::shared_locations::ensure_project_skills_dir(ac_root)
        .map_err(|error| error.to_string())
        .and_then(|()| crate::config::root_agent::ensure_project_self_handoff_skill_at(ac_root));
    if let Err(error) = skills_result {
        log::warn!(
            "[project-skills] unavailable at {}: {}; continuing without default project skills",
            ac_root
                .join(crate::config::shared_locations::PROJECT_SKILLS_DIR)
                .display(),
            error
        );
    }
    let mut loaded = load_state(ac_root, false)?;
    for spec in project_specs() {
        let execution =
            sync_one_template(None, ac_root, spec, &mut loaded, true, false, false, clock);
        let _ = consume_template_execution(spec, execution, on_publication)?;
    }
    persist_state_best_effort(ac_root, &loaded);
    Ok(())
}

/// #1625: absent-only seed of the three per-execution-platform rule files at
/// render time for an already-known project. Mirrors
/// `ensure_project_context_templates_with_clock` but ONLY for `platform_specs()`
/// (global/coordinator are never touched here; their read-sync stays the
/// managed-filename path in session_context).
pub fn ensure_platform_context_templates(context_dir: &Path) -> Result<(), String> {
    let mut on_publication = |_: &'static str, _: ContextPublication| {};
    ensure_platform_context_templates_with_publications(context_dir, &mut on_publication)
}

pub(crate) fn ensure_platform_context_templates_with_publications(
    context_dir: &Path,
    on_publication: &mut dyn FnMut(&'static str, ContextPublication),
) -> Result<(), String> {
    let mut clock = chrono::Utc::now;
    ensure_platform_context_templates_with_clock(context_dir, &mut clock, on_publication)
}

fn ensure_platform_context_templates_with_clock(
    context_dir: &Path,
    clock: &mut dyn FnMut() -> chrono::DateTime<chrono::Utc>,
    on_publication: &mut dyn FnMut(&'static str, ContextPublication),
) -> Result<(), String> {
    validate_existing_dir(context_dir, "Context template directory")?;
    let mut loaded = load_state(context_dir, false)?;
    for spec in platform_specs() {
        let execution = sync_one_template(
            None,
            context_dir,
            spec,
            &mut loaded,
            true,
            false,
            false,
            clock,
        );
        let _ = consume_template_execution(spec, execution, on_publication)?;
    }
    persist_state_best_effort(context_dir, &loaded);
    Ok(())
}

pub fn scan_project_context_template_updates(
    project_dir: &Path,
    ac_root: &Path,
) -> Result<Vec<ContextTemplateUpdate>, String> {
    let mut on_publication = |_: &'static str, _: ContextPublication| {};
    scan_project_context_template_updates_with_publications(
        project_dir,
        ac_root,
        &mut on_publication,
    )
}

pub(crate) fn scan_project_context_template_updates_with_publications(
    project_dir: &Path,
    ac_root: &Path,
    on_publication: &mut dyn FnMut(&'static str, ContextPublication),
) -> Result<Vec<ContextTemplateUpdate>, String> {
    let mut clock = chrono::Utc::now;
    scan_project_context_template_updates_with_clock(
        project_dir,
        ac_root,
        &mut clock,
        on_publication,
    )
}

fn scan_project_context_template_updates_with_clock(
    project_dir: &Path,
    ac_root: &Path,
    clock: &mut dyn FnMut() -> chrono::DateTime<chrono::Utc>,
    on_publication: &mut dyn FnMut(&'static str, ContextPublication),
) -> Result<Vec<ContextTemplateUpdate>, String> {
    scan_project_context_templates_with_clock(project_dir, ac_root, clock, on_publication)
        .map(|(updates, _replacements)| updates)
}

/// #1748: the scan is the only sync path whose return value reaches the UI, so it
/// is the only one that repairs a distribution-owned template (D2). Phase 02 turns
/// the returned replacements into a surfaced value; here they are logged.
fn scan_project_context_templates_with_clock(
    project_dir: &Path,
    ac_root: &Path,
    clock: &mut dyn FnMut() -> chrono::DateTime<chrono::Utc>,
    on_publication: &mut dyn FnMut(&'static str, ContextPublication),
) -> Result<(Vec<ContextTemplateUpdate>, Vec<ContextTemplateReplacement>), String> {
    validate_project_ac_root_for_scan(project_dir, ac_root)?;
    let mut loaded = load_state(ac_root, false)?;
    let mut updates = Vec::new();
    let mut replacements = Vec::new();
    for spec in project_specs() {
        let execution = sync_one_template(
            Some(project_dir),
            ac_root,
            spec,
            &mut loaded,
            false,
            true,
            true,
            clock,
        );
        let outcome = consume_template_execution(spec, execution, on_publication)?;
        if let Some(replacement) = outcome.replacement {
            log::info!(
                "[context-templates] {} was replaced with the current default; the previous bytes \
                    are in {}",
                replacement.file_path,
                replacement.backup_path
            );
            replacements.push(replacement);
        }
        if let Some(update) = outcome.pending_update {
            updates.push(update);
        }
    }
    persist_state_best_effort(ac_root, &loaded);
    dedupe_context_template_updates(&mut updates);
    Ok((updates, replacements))
}

pub fn sync_project_context_template_for_read(
    context_dir: &Path,
    filename: &str,
) -> Result<(), String> {
    let mut on_publication = |_: &'static str, _: ContextPublication| {};
    sync_project_context_template_for_read_with_publications(
        context_dir,
        filename,
        &mut on_publication,
    )
}

pub(crate) fn sync_project_context_template_for_read_with_publications(
    context_dir: &Path,
    filename: &str,
    on_publication: &mut dyn FnMut(&'static str, ContextPublication),
) -> Result<(), String> {
    let mut clock = chrono::Utc::now;
    sync_project_context_template_for_read_with_clock(
        context_dir,
        filename,
        &mut clock,
        on_publication,
    )
}

fn sync_project_context_template_for_read_with_clock(
    context_dir: &Path,
    filename: &str,
    clock: &mut dyn FnMut() -> chrono::DateTime<chrono::Utc>,
    on_publication: &mut dyn FnMut(&'static str, ContextPublication),
) -> Result<(), String> {
    let Some(spec) = project_spec_by_filename(filename) else {
        return Ok(());
    };
    validate_existing_dir(context_dir, "Context template directory")?;
    let mut loaded = load_state(context_dir, false)?;
    let execution = sync_one_template(
        None,
        context_dir,
        spec,
        &mut loaded,
        true,
        false,
        false,
        clock,
    );
    let result = consume_template_execution(spec, execution, on_publication).map(|_| ());
    persist_state_best_effort(context_dir, &loaded);
    result
}

pub fn ensure_root_context_template(config_dir: &Path) -> Result<(), String> {
    crate::filesystem::context_templates::ensure_template_directory(config_dir).map_err(|e| {
        format!(
            "Failed to create root agent config directory {}: {}",
            config_dir.display(),
            e
        )
    })?;
    validate_existing_dir(config_dir, "Root agent config directory")?;
    let mut loaded = load_state(config_dir, false)?;
    let mut clock = chrono::Utc::now;
    let execution = sync_one_template(
        None,
        config_dir,
        root_spec(),
        &mut loaded,
        true,
        false,
        false,
        &mut clock,
    );
    execution.completion?;
    persist_state_best_effort(config_dir, &loaded);
    Ok(())
}

#[cfg(test)]
mod tests;
