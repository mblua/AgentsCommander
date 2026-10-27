//! Shared open/new-project logic. Used by both the Tauri commands
//! (`commands::ac_discovery::open_project` / `new_project`) and the CLI verbs
//! (`cli::open_project` / `cli::new_project`). The same code path means UI and
//! CLI cannot diverge on dedup, validation, or registration order.
//!
//! This module is intentionally Tauri-free and CLI-free — it operates on a
//! mutable `&mut AppSettings` borrow plus a `&Path`. Callers own the
//! lock-acquire and the `save_settings` call.

pub(crate) mod ac_root_gitignore;

use crate::filesystem::projects as project_fs;
use std::collections::HashSet;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use super::ac_root::{ac_root_for_project, existing_ac_root, has_ac_root};
use super::settings::AppSettings;

#[cfg(test)]
pub(crate) use super::settings::project_path::{
    decode_instance_relative, encode_instance_relative, resolve_registrations, CandidateResolver,
    DirectoryIdentity, FsCandidateResolver, RawPair, RelDecodeError, RepairKind, SideOutcome,
};
pub(crate) use super::settings::project_path::{
    display_canonical, normalize_for_compare, IssueKind, ProjectPathPersistenceState,
    ProjectSource, RawJsonField, RawStringField, ResolvedPair, SideStatus, StructuralIssue,
};

pub(crate) fn configured_project_is_dir(path: &Path) -> bool {
    project_fs::project_path_is_dir(path)
}

pub(crate) fn canonical_source_project(path: &Path) -> std::io::Result<PathBuf> {
    project_fs::canonical_project_path(path)
}

pub(crate) fn canonical_repo_source(path: &Path) -> std::io::Result<PathBuf> {
    project_fs::canonical_project_path(path)
}

pub(crate) fn canonical_watch_project(path: &Path) -> Option<PathBuf> {
    project_fs::canonical_project_path(path).ok()
}

pub(crate) fn visit_discovery_project_children(base: &Path, visit: impl FnMut(PathBuf)) {
    project_fs::visit_discovery_project_children(base, visit);
}

/// Outcome of a register call. Callers translate this into the verb-specific
/// stdout / IPC payload (CLI prints the lines from §2; Tauri command returns
/// the struct verbatim — `#[serde(rename_all = "camelCase")]`).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRegistration {
    /// Absolute path that was added (or matched) in `project_paths`.
    pub path: String,
    /// `true` when this call appended a new entry; `false` when the path was
    /// already present (case-insensitive, slash-normalised match).
    pub registered: bool,
    /// `true` when this call created `.ac/` on disk. Always `false` for
    /// `open_project`. `true` for `new_project` only when the directory did
    /// not already exist.
    pub created: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchivedProject {
    pub path: String,
    pub folder_name: String,
    pub exists: bool,
    pub has_workspace: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectResolution {
    pub path: PathBuf,
    pub folder_name: String,
    pub registered: bool,
}

/// Errors returned by the helper. `Display` strings are the exact stderr text
/// the CLI prints (prefixed with `Error: ` by the caller — see §4.4).
#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("path '{0}' is empty")]
    EmptyPath(String),
    #[error("path does not exist: {0}")]
    PathMissing(PathBuf),
    #[error("path is not a directory: {0}")]
    NotADirectory(PathBuf),
    #[error("no AC project at {0} (.ac/ not found)")]
    AcRootMissing(PathBuf),
    #[error("failed to resolve absolute path for '{0}': {1}")]
    CwdFailure(String, std::io::Error),
    #[error("failed to create .ac directory at {0}: {1}")]
    AcRootCreateFailed(PathBuf, std::io::Error),
    #[error("failed to write Project AC Root .gitignore at {0}: {1}")]
    AcRootGitignoreFailed(PathBuf, String),
    #[error("failed to create context templates in .ac directory at {0}: {1}")]
    ContextTemplatesCreateFailed(PathBuf, String),
    #[error("AC project setup at {0} is no longer stable: {1}; retry the operation")]
    ProjectSetupChanged(PathBuf, String),
    #[error("failed to {operation} project registration settings: {error}")]
    ProjectSettingsFailed {
        operation: &'static str,
        error: String,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ProjectResolveError {
    #[error("project reference is empty")]
    Empty,
    #[error("project reference must not contain NUL")]
    Invalid,
    #[error("project '{raw}' is ambiguous; candidates: {candidates:?}")]
    Ambiguous {
        raw: String,
        candidates: Vec<String>,
    },
    #[error("project '{0}' was not found in settings.projectPaths")]
    NotFound(String),
}

/// Validate an existing AC project and register it in `settings.project_paths`.
/// Errors when the path is missing, not a directory, or has no Project AC Root.
///
/// On success, mutates `settings.project_paths` (appends if new) and
/// `settings.project_path` (legacy single-project field — kept in sync with
/// `project_paths[0]` to match the frontend's `persistProjectPaths` contract
/// at `src/sidebar/stores/project.ts:163-171`).
///
/// Caller is responsible for `save_settings(settings)` AFTER this returns Ok.
pub fn register_existing_project(
    settings: &mut AppSettings,
    raw_path: &str,
) -> Result<ProjectRegistration, ProjectError> {
    let abs = absolutise(raw_path)?;
    if !project_fs::project_path_exists(&abs) {
        return Err(ProjectError::PathMissing(abs));
    }
    if !project_fs::project_path_is_dir(&abs) {
        return Err(ProjectError::NotADirectory(abs));
    }
    if existing_ac_root(&abs).is_none() {
        return Err(ProjectError::AcRootMissing(abs));
    }
    let abs_str = abs.to_string_lossy().into_owned();
    let registered = upsert_project_path(settings, &abs_str);
    Ok(ProjectRegistration {
        path: abs_str,
        registered,
        created: false,
    })
}

/// Ensure the AC project structure exists (creating `.ac/` and its
/// `.gitignore` when missing) and register it in `settings.project_paths`.
///
/// Errors only when the path is empty, the parent does not exist, or
/// `.ac/` cannot be created. A pre-existing `.ac/` is fine; the
/// gitignore sweep is opportunistic (matches `discover_project`'s behaviour
/// at `src-tauri/src/commands/ac_discovery.rs:1308-1309`).
pub fn register_new_project(
    settings: &mut AppSettings,
    raw_path: &str,
) -> Result<ProjectRegistration, ProjectError> {
    #[cfg(test)]
    let store = NewProjectSettingsStore::CallerOwned;
    #[cfg(not(test))]
    let store = NewProjectSettingsStore::Production;
    // #1065 Stage F: production activates seed-manifest emission for the CLI
    // `new-project` fresh-root registration path; a `#[cfg(test)]` lib build stays
    // non-emitting so existing unit tests keep manifest-free temp projects.
    #[cfg(not(test))]
    let activation = Some(crate::config::seed_manifest::ManifestActivationToken::production());
    #[cfg(test)]
    let activation: Option<crate::config::seed_manifest::ManifestActivationToken> = None;
    register_new_project_with_store(settings, raw_path, store, activation.as_ref())
}

enum NewProjectSettingsStore {
    #[cfg(test)]
    CallerOwned,
    #[cfg(test)]
    Path(PathBuf),
    #[cfg(not(test))]
    Production,
}

impl NewProjectSettingsStore {
    fn begin(
        &self,
        settings: &AppSettings,
    ) -> Result<Option<crate::config::settings::ProjectPathsWriteTransaction>, ProjectError> {
        let path = match self {
            #[cfg(test)]
            Self::CallerOwned => return Ok(None),
            #[cfg(test)]
            Self::Path(path) => path.clone(),
            #[cfg(not(test))]
            Self::Production => crate::config::settings::settings_path().ok_or_else(|| {
                ProjectError::ProjectSettingsFailed {
                    operation: "refresh",
                    error: "Could not determine settings directory".to_string(),
                }
            })?,
        };
        crate::config::settings::begin_project_paths_transaction_at_path(settings, &path)
            .map(Some)
            .map_err(|error| ProjectError::ProjectSettingsFailed {
                operation: "refresh",
                error,
            })
    }
}

fn register_new_project_with_store(
    settings: &mut AppSettings,
    raw_path: &str,
    store: NewProjectSettingsStore,
    activation: Option<&crate::config::seed_manifest::ManifestActivationToken>,
) -> Result<ProjectRegistration, ProjectError> {
    let prepared = prepare_new_project_impl(raw_path, activation, |ac_root, on_publication| {
        crate::config::session_context::create_default_context_templates_with_publications(
            ac_root,
            on_publication,
        )
    })?;
    let mut transaction = store.begin(settings)?;
    let before_raw = transaction
        .as_ref()
        .map(|token| token.raw_project_projection())
        .transpose()
        .map_err(|error| ProjectError::ProjectSettingsFailed {
            operation: "refresh",
            error,
        })?;
    let mut candidate = match &transaction {
        Some(token) => token.fresh_settings().clone(),
        None => settings.clone(),
    };
    if let Some(token) = &transaction {
        token
            .refresh_raw_projects(&mut candidate)
            .map_err(|error| ProjectError::ProjectSettingsFailed {
                operation: "refresh",
                error,
            })?;
    }
    if crate::config::settings::project_state_has_structural(&candidate) {
        return Err(ProjectError::ProjectSettingsFailed {
            operation: "refresh",
            error: "project metadata is malformed".to_string(),
        });
    }
    let result = commit_prepared_new_project(&mut candidate, &prepared);
    prepared.revalidate()?;
    let saved = result.created || result.registered;
    if saved {
        crate::config::settings::resync_project_state_from_runtime(&mut candidate);
        if let Some(token) = &mut transaction {
            candidate = token
                .commit_project_groups(&candidate, true, true)
                .map_err(|error| ProjectError::ProjectSettingsFailed {
                    operation: "persist",
                    error,
                })?;
        }
    }
    if let Err(error) = prepared.revalidate() {
        if saved {
            if let (Some(token), Some(raw)) = (&mut transaction, &before_raw) {
                match token.restore_raw_project_projection(raw) {
                    Ok(written) => *settings = written,
                    Err(rollback_error) => {
                        *settings = token.fresh_settings().clone();
                        settings.project_paths_persistence_error = Some(rollback_error.clone());
                        return Err(ProjectError::ProjectSetupChanged(
                            prepared.abs.clone(),
                            format!(
                                concat!(
                                    "{}; failed to roll back project ",
                                    "registration settings: {}"
                                ),
                                error, rollback_error
                            ),
                        ));
                    }
                }
            }
        }
        return Err(error);
    }
    *settings = candidate;
    drop(transaction);
    prepared.release();
    Ok(result)
}

#[derive(Debug)]
pub(crate) struct PreparedNewProject {
    abs: PathBuf,
    created: bool,
    guard: crate::config::seed_manifest::ProjectSeedManifestGuard,
}

impl PreparedNewProject {
    pub(crate) fn ac_root(&self) -> &Path {
        self.guard.ac_root()
    }

    pub(crate) fn revalidate(&self) -> Result<(), ProjectError> {
        self.guard
            .revalidate_owner()
            .map_err(|error| ProjectError::ProjectSetupChanged(self.abs.clone(), error.to_string()))
    }

    pub(crate) fn release(self) {
        self.guard.release();
    }
}

pub(crate) fn prepare_new_project(raw_path: &str) -> Result<PreparedNewProject, ProjectError> {
    // #1065 Stage F: the production build activates seed-manifest emission for the
    // GUI/web fresh-root registration path; a `#[cfg(test)]` lib build stays
    // non-emitting so existing unit tests keep manifest-free temp projects.
    #[cfg(not(test))]
    let activation = Some(crate::config::seed_manifest::ManifestActivationToken::production());
    #[cfg(test)]
    let activation: Option<crate::config::seed_manifest::ManifestActivationToken> = None;
    prepare_new_project_impl(raw_path, activation.as_ref(), |ac_root, on_publication| {
        crate::config::session_context::create_default_context_templates_with_publications(
            ac_root,
            on_publication,
        )
    })
}

fn prepare_new_project_impl<F>(
    raw_path: &str,
    activation: Option<&crate::config::seed_manifest::ManifestActivationToken>,
    create_context_templates: F,
) -> Result<PreparedNewProject, ProjectError>
where
    F: FnOnce(
        &Path,
        &mut dyn FnMut(&'static str, crate::config::seeded_context_templates::ContextPublication),
    ) -> Result<(), String>,
{
    prepare_new_project_impl_with_hook(raw_path, activation, create_context_templates, |_, _| {})
}

fn prepare_new_project_impl_with_hook<F, H>(
    raw_path: &str,
    activation: Option<&crate::config::seed_manifest::ManifestActivationToken>,
    create_context_templates: F,
    after_create_before_gate: H,
) -> Result<PreparedNewProject, ProjectError>
where
    F: FnOnce(
        &Path,
        &mut dyn FnMut(&'static str, crate::config::seeded_context_templates::ContextPublication),
    ) -> Result<(), String>,
    H: FnOnce(&Path, bool),
{
    let abs = absolutise(raw_path)?;
    // Allow PATH to not yet exist as a directory. Reject if PATH exists and
    // is a regular file (caller almost certainly fat-fingered).
    if project_fs::project_path_exists(&abs) && !project_fs::project_path_is_dir(&abs) {
        return Err(ProjectError::NotADirectory(abs));
    }
    // Ensure the parent (PATH itself) exists so the non-recursive
    // `create_dir` below can race-detect properly. `create_dir_all` is
    // idempotent on an already-existing dir, so this costs nothing extra
    // when PATH is already there.
    std::fs::create_dir_all(&abs).map_err(|e| ProjectError::AcRootCreateFailed(abs.clone(), e))?;

    let ac_root = ac_root_for_project(&abs);
    // The create syscall is the sole creation-intent authority. An earlier
    // existence observation cannot distinguish this caller from a contender.
    let created = match std::fs::create_dir(&ac_root) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(e) => return Err(ProjectError::AcRootCreateFailed(ac_root.clone(), e)),
    };
    let pinned_project = crate::config::seed_manifest::PinnedDirectory::open(&abs)
        .map_err(|error| ProjectError::ProjectSetupChanged(abs.clone(), error.to_string()))?;
    let pinned_ac_root = crate::config::seed_manifest::PinnedDirectory::open(&ac_root)
        .map_err(|error| ProjectError::ProjectSetupChanged(abs.clone(), error.to_string()))?;
    after_create_before_gate(&ac_root, created);
    pinned_project
        .revalidate()
        .map_err(|error| ProjectError::ProjectSetupChanged(abs.clone(), error.to_string()))?;
    pinned_ac_root
        .revalidate()
        .map_err(|error| ProjectError::ProjectSetupChanged(abs.clone(), error.to_string()))?;

    let mut guard = crate::config::seed_manifest::ProjectSeedManifestGuard::acquire(&abs)
        .map_err(|error| ProjectError::ProjectSetupChanged(abs.clone(), error.to_string()))?;
    pinned_project
        .revalidate()
        .map_err(|error| ProjectError::ProjectSetupChanged(abs.clone(), error.to_string()))?;
    pinned_ac_root
        .revalidate_at(guard.ac_root())
        .map_err(|error| ProjectError::ProjectSetupChanged(abs.clone(), error.to_string()))?;
    guard
        .revalidate_owner()
        .map_err(|error| ProjectError::ProjectSetupChanged(abs.clone(), error.to_string()))?;
    if created {
        if let Err(e) = crate::commands::ac_discovery::ensure_ac_root_gitignore(&ac_root) {
            return Err(ProjectError::AcRootGitignoreFailed(ac_root.clone(), e));
        }
    } else {
        // Gitignore sweep (Round-1 G15): best-effort when `.ac` pre-existed
        // because a transient FS error on someone else's gitignore should not
        // fail registration of a valid project.
        if let Err(e) = crate::commands::ac_discovery::ensure_ac_root_gitignore(&ac_root) {
            log::warn!(
                concat!(
                    "[projects] gitignore sweep failed on pre-existing ",
                    "Project AC Root at {:?}: {} (best-effort, continuing)",
                ),
                ac_root,
                e
            );
        }
    }

    {
        // #1065 Stage F: record each freshly created project context template into
        // the seed manifest at its commit-point time, under the gate held since
        // acquisition (plan sections 5.2/6.3). `activation` is `None` for a test or
        // unactivated caller, so the closure is a no-op and setup stays manifest-free.
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
        if let Err(e) = create_context_templates(&ac_root, &mut on_publication) {
            return Err(ProjectError::ContextTemplatesCreateFailed(
                ac_root.clone(),
                e,
            ));
        }
    }

    guard
        .revalidate_owner()
        .map_err(|error| ProjectError::ProjectSetupChanged(abs.clone(), error.to_string()))?;

    Ok(PreparedNewProject {
        abs,
        created,
        guard,
    })
}

pub(crate) fn commit_prepared_new_project(
    settings: &mut AppSettings,
    prepared: &PreparedNewProject,
) -> ProjectRegistration {
    let abs_str = prepared.abs.to_string_lossy().into_owned();
    let registered = upsert_project_path(settings, &abs_str);
    ProjectRegistration {
        path: abs_str,
        registered,
        created: prepared.created,
    }
}

pub fn resolve_project_reference(
    project_paths: &[String],
    raw_project: &str,
) -> Result<ProjectResolution, ProjectResolveError> {
    let project = raw_project.trim();
    if project.is_empty() {
        return Err(ProjectResolveError::Empty);
    }
    if project.contains('\0') {
        return Err(ProjectResolveError::Invalid);
    }

    let candidates = enumerate_registered_project_candidates(project_paths);

    let name_matches = candidates
        .iter()
        .filter(|candidate| candidate.folder_name.eq_ignore_ascii_case(project))
        .cloned()
        .collect::<Vec<_>>();
    match name_matches.len() {
        1 => return Ok(name_matches.into_iter().next().expect("one match")),
        n if n > 1 => {
            let candidates = name_matches
                .into_iter()
                .map(|candidate| candidate.path.to_string_lossy().to_string())
                .collect::<Vec<_>>();
            return Err(ProjectResolveError::Ambiguous {
                raw: project.to_string(),
                candidates,
            });
        }
        _ => {}
    }

    Err(ProjectResolveError::NotFound(project.to_string()))
}

// ── Private helpers ───────────────────────────────────────────────────────

pub(crate) fn absolutise(raw: &str) -> Result<PathBuf, ProjectError> {
    if raw.trim().is_empty() {
        return Err(ProjectError::EmptyPath(raw.to_string()));
    }
    // `std::path::absolute` (stable since Rust 1.79; toolchain is 1.93.1)
    // lexically resolves the path against the process CWD. On Windows it
    // also collapses `.`/`..` segments via `GetFullPathNameW` — closing
    // Round-1 G4 (silent double-registration of `..\projects` from
    // different CWDs). On POSIX the std API preserves `..` for
    // symlink-safety reasons; documented as §6.10. No filesystem IO,
    // no symlink resolution.
    std::path::absolute(raw).map_err(|e| ProjectError::CwdFailure(raw.to_string(), e))
}

/// Append `abs_path` to `settings.project_paths` iff no existing entry
/// normalises to the same key. Always re-syncs `settings.project_path` to
/// `project_paths[0]` so the legacy single-project field never drifts.
/// Returns `true` if a new entry was added.
fn upsert_project_path(settings: &mut AppSettings, abs_path: &str) -> bool {
    let key = normalize_for_compare(abs_path);
    settings
        .archived_project_paths
        .retain(|p| normalize_for_compare(p) != key);
    let exists = settings
        .project_paths
        .iter()
        .any(|p| normalize_for_compare(p) == key);
    let appended = if exists {
        false
    } else {
        settings.project_paths.push(abs_path.to_string());
        true
    };
    // Keep legacy `projectPath` field in lockstep with the head of the list,
    // matching the frontend's `persistProjectPaths` at
    // `src/sidebar/stores/project.ts:166-170`.
    settings.project_path = settings.project_paths.first().cloned();
    appended
}

/// #778: remove the entry whose `normalize_for_compare` key matches `abs_path`,
/// then re-derive the legacy `project_path` head. The inverse of
/// `upsert_project_path`; used by the `remove_project` command AFTER it has
/// reconciled `project_paths` from disk, so removing against the fresh list
/// cannot drop a CLI-appended entry the sidebar never showed. Returns `true` if
/// an entry was removed. Byte-form of surviving entries is untouched.
pub fn remove_project_path(settings: &mut AppSettings, abs_path: &str) -> bool {
    let key = normalize_for_compare(abs_path);
    let before = settings.project_paths.len();
    settings
        .project_paths
        .retain(|p| normalize_for_compare(p) != key);
    let removed = settings.project_paths.len() != before;
    settings
        .archived_project_paths
        .retain(|p| normalize_for_compare(p) != key);
    // Keep legacy `projectPath` in lockstep with the head (matches upsert).
    settings.project_path = settings.project_paths.first().cloned();
    removed
}

/// #881: move `abs_path` from the active list to the archived list.
pub fn archive_project_path(settings: &mut AppSettings, abs_path: &str) -> bool {
    let key = normalize_for_compare(abs_path);
    let matched = settings
        .project_paths
        .iter()
        .find(|p| normalize_for_compare(p) == key)
        .cloned();
    settings
        .project_paths
        .retain(|p| normalize_for_compare(p) != key);
    let removed = matched.is_some();
    let already_archived = settings
        .archived_project_paths
        .iter()
        .any(|p| normalize_for_compare(p) == key);
    if !already_archived {
        settings
            .archived_project_paths
            .push(matched.unwrap_or_else(|| abs_path.to_string()));
    }
    settings.project_path = settings.project_paths.first().cloned();
    removed
}

/// Move a stored archived project back to the active list without probing the
/// filesystem. Used only to roll back an archive operation that already passed
/// path normalization and then discovered late liveness.
pub fn unarchive_project_path(settings: &mut AppSettings, abs_path: &str) -> bool {
    let key = normalize_for_compare(abs_path);
    let matched = settings
        .archived_project_paths
        .iter()
        .find(|p| normalize_for_compare(p) == key)
        .cloned();
    let Some(restored) = matched else {
        return false;
    };
    settings
        .archived_project_paths
        .retain(|p| normalize_for_compare(p) != key);
    let already_active = settings
        .project_paths
        .iter()
        .any(|p| normalize_for_compare(p) == key);
    if !already_active {
        settings.project_paths.push(restored);
    }
    settings.project_path = settings.project_paths.first().cloned();
    true
}

pub fn archived_projects_from_paths(archived_project_paths: &[String]) -> Vec<ArchivedProject> {
    archived_project_paths
        .iter()
        .map(|raw| {
            let path = Path::new(raw);
            ArchivedProject {
                path: raw.clone(),
                folder_name: path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(raw)
                    .to_string(),
                exists: is_real_directory(path),
                has_workspace: has_ac_root(path),
            }
        })
        .collect()
}

pub fn enumerate_registered_project_candidates(project_paths: &[String]) -> Vec<ProjectResolution> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    for raw in project_paths {
        let base = Path::new(raw);
        if !is_real_directory(base) {
            continue;
        }

        push_project_candidate(base, true, &mut out, &mut seen);

        let _ = project_fs::visit_project_kind_children(base, |entry| {
            let Ok(path) = entry else {
                return ControlFlow::Continue(());
            };
            if !is_real_directory(&path) {
                return ControlFlow::Continue(());
            }
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(name) => name,
                None => return ControlFlow::Continue(()),
            };
            if name.starts_with('.') {
                return ControlFlow::Continue(());
            }
            push_project_candidate(&path, true, &mut out, &mut seen);
            ControlFlow::Continue(())
        });
    }

    out
}

fn push_project_candidate(
    path: &Path,
    registered: bool,
    out: &mut Vec<ProjectResolution>,
    seen: &mut HashSet<String>,
) {
    if !is_real_directory(path) || !has_ac_root(path) {
        return;
    }
    let Some(key) = canonical_key(path) else {
        return;
    };
    if !seen.insert(key) {
        return;
    }
    let Some(folder_name) = path
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string)
    else {
        return;
    };
    out.push(ProjectResolution {
        path: path.to_path_buf(),
        folder_name,
        registered,
    });
}

fn is_real_directory(path: &Path) -> bool {
    let md = match project_fs::project_path_symlink_metadata(path) {
        Ok(m) => m,
        Err(_) => return false,
    };
    if md.file_type().is_symlink() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        if md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return false;
        }
    }
    md.is_dir()
}

fn canonical_key(path: &Path) -> Option<String> {
    let canonical = project_fs::canonical_project_path(path).ok()?;
    Some(path_compare_key(&canonical.to_string_lossy()))
}

fn path_compare_key(s: &str) -> String {
    let without_extended = if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{}", rest)
    } else {
        s.strip_prefix(r"\\?\").unwrap_or(s).to_string()
    };
    normalize_for_compare(&without_extended)
}

#[cfg(test)]
mod tests;
