//! Shared open/new-project logic. Used by both the Tauri commands
//! (`commands::ac_discovery::open_project` / `new_project`) and the CLI verbs
//! (`cli::open_project` / `cli::new_project`). The same code path means UI and
//! CLI cannot diverge on dedup, validation, or registration order.
//!
//! This module is intentionally Tauri-free and CLI-free — it operates on a
//! mutable `&mut AppSettings` borrow plus a `&Path`. Callers own the
//! lock-acquire and the `save_settings` call.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::ac_root::{ac_root_for_project, existing_ac_root, has_ac_root};
use super::settings::AppSettings;

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
    if !abs.exists() {
        return Err(ProjectError::PathMissing(abs));
    }
    if !abs.is_dir() {
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
    if abs.exists() && !abs.is_dir() {
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

/// Mirrors the frontend `normalizePath` at
/// `src/sidebar/stores/project.ts:17-19`. Comparison only — the persisted
/// entry retains the original byte sequence.
pub(crate) fn normalize_for_compare(s: &str) -> String {
    let s = crate::path_utils::normalize_windows_verbatim_path(s);
    // Slashes normalised, lowercased, trailing `/` stripped. The trailing
    // strip closes Round-1 IR.3.2 (shell tab-completion appends `\` on dirs;
    // without trim, `C:\foo\` and `C:\foo` would become DIFFERENT entries).
    s.replace('\\', "/")
        .to_lowercase()
        .trim_end_matches('/')
        .to_string()
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

        if let Ok(entries) = std::fs::read_dir(base) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !is_real_directory(&path) {
                    continue;
                }
                let name = match path.file_name().and_then(|n| n.to_str()) {
                    Some(name) => name,
                    None => continue,
                };
                if name.starts_with('.') {
                    continue;
                }
                push_project_candidate(&path, true, &mut out, &mut seen);
            }
        }
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
    let md = match std::fs::symlink_metadata(path) {
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
    let canonical = std::fs::canonicalize(path).ok()?;
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

// ── #1077 portable instance-relative path codec ────────────────────────────
//
// Encodes/decodes the OS-neutral, `/`-separated companion form that pairs an
// absolute project path with the running executable's instance base. The wire
// grammar is deliberately strict and fail-closed: every decode rejection means
// the companion "did not load" and the absolute side is used instead. See
// plan #1077 §3.3. JSON extraction/serialization of these strings lives in
// `config/settings.rs`; this module owns only the lexical grammar.

/// Reasons a persisted instance-relative companion string fails to decode.
/// Each is a fail-closed rejection (the value is treated as not loading).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RelDecodeError {
    /// Empty string.
    Empty,
    /// Contains a NUL byte.
    Nul,
    /// Contains a `\` on any host (Unix filenames with `\` have no portable form).
    Backslash,
    /// A leading, trailing, or repeated `/` produced an empty segment.
    EmptySegment,
    /// An embedded `.` component (only a whole-string `.` names the base).
    DotSegment,
    /// A drive-relative component such as `C:` or `C:dir` (rejected on every host).
    DriveRelative,
    /// Parent traversal that would walk above the filesystem root.
    EscapesRoot,
    /// A Windows-illegal character (`< > : " | ? *` or a control char).
    #[cfg(windows)]
    IllegalWindowsChar,
    /// A reserved DOS device basename (CON, PRN, AUX, NUL, COM1-9, LPT1-9).
    #[cfg(windows)]
    ReservedDosName,
    /// A component ending in a space or dot (Win32 strips these).
    #[cfg(windows)]
    TrailingDotOrSpace,
    /// The instance base handed in was not absolute.
    BaseNotAbsolute,
}

/// Root/prefix identity used only for case-insensitive Windows root-compat
/// comparison. Ordinary and verbatim disk/UNC roots normalize to the same key;
/// device namespaces (`\\.\`, non-disk/UNC `\\?\`, GLOBALROOT) are unsupported
/// and never stripped.
#[cfg(windows)]
#[derive(Debug, Clone, PartialEq, Eq)]
enum RootKey {
    Disk(u8),
    Unc(String, String),
}

#[cfg(not(windows))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct RootKey;

/// Split an absolute canonical path into its root key and normal components.
/// Returns `None` for an unsupported prefix, a non-absolute path, or any
/// non-normal interior component (canonical inputs never contain `.`/`..`).
#[cfg(windows)]
fn split_root_and_normals(path: &Path) -> Option<(RootKey, Vec<std::ffi::OsString>)> {
    use std::path::{Component, Prefix};
    let mut comps = path.components();
    let prefix = match comps.next() {
        Some(Component::Prefix(p)) => p.kind(),
        _ => return None,
    };
    let root_key = match prefix {
        Prefix::Disk(l) | Prefix::VerbatimDisk(l) => RootKey::Disk(l.to_ascii_uppercase()),
        Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => RootKey::Unc(
            server.to_string_lossy().to_ascii_lowercase(),
            share.to_string_lossy().to_ascii_lowercase(),
        ),
        // Verbatim(non-disk), DeviceNS: unsupported. Never pass through the
        // broad prefix stripper; return None so the caller writes a null
        // companion and keeps absolute-only behavior.
        _ => return None,
    };
    // A drive-absolute path has RootDir after the prefix; `C:dir` (drive-relative)
    // does not and is rejected here.
    match comps.next() {
        Some(Component::RootDir) => {}
        _ => return None,
    }
    let mut norms = Vec::new();
    for c in comps {
        match c {
            Component::Normal(s) => norms.push(s.to_os_string()),
            _ => return None,
        }
    }
    Some((root_key, norms))
}

#[cfg(not(windows))]
fn split_root_and_normals(path: &Path) -> Option<(RootKey, Vec<std::ffi::OsString>)> {
    use std::path::Component;
    let mut comps = path.components();
    match comps.next() {
        Some(Component::RootDir) => {}
        _ => return None,
    }
    let mut norms = Vec::new();
    for c in comps {
        match c {
            Component::Normal(s) => norms.push(s.to_os_string()),
            _ => return None,
        }
    }
    Some((RootKey, norms))
}

/// Root compatibility. Windows drive letters and UNC server/share compare
/// case-insensitively (already normalized in `RootKey`); POSIX roots always
/// match. Incompatible roots (different drive letters or UNC shares) have no
/// lexical relative form.
#[cfg(windows)]
fn roots_compatible(a: &RootKey, b: &RootKey) -> bool {
    a == b
}

#[cfg(not(windows))]
fn roots_compatible(_a: &RootKey, _b: &RootKey) -> bool {
    true
}

/// Encode `project` relative to `base`. Both must be absolute, canonical paths.
/// Returns the OS-neutral `/`-separated wire form, or `None` when the roots are
/// incompatible, a prefix is unsupported, or any emitted component cannot be
/// represented losslessly by the wire grammar (non-UTF-8, contains `/` or `\`,
/// or is literally `.`/`..`). Interior components are compared EXACTLY (both
/// come from `canonicalize`, which fixes on-disk casing), so only the root is
/// case-insensitive on Windows.
pub(crate) fn encode_instance_relative(project: &Path, base: &Path) -> Option<String> {
    if !project.is_absolute() || !base.is_absolute() {
        return None;
    }
    let (proj_root, proj_norms) = split_root_and_normals(project)?;
    let (base_root, base_norms) = split_root_and_normals(base)?;
    if !roots_compatible(&proj_root, &base_root) {
        return None;
    }

    let mut common = 0usize;
    while common < proj_norms.len()
        && common < base_norms.len()
        && proj_norms[common] == base_norms[common]
    {
        common += 1;
    }

    let mut out: Vec<String> = Vec::new();
    for _ in common..base_norms.len() {
        out.push("..".to_string());
    }
    for c in &proj_norms[common..] {
        let s = c.to_str()?; // non-UTF-8 → no portable form
        if s.is_empty() || s.contains('/') || s.contains('\\') || s == "." || s == ".." {
            return None;
        }
        out.push(s.to_string());
    }

    if out.is_empty() {
        return Some(".".to_string());
    }
    Some(out.join("/"))
}

/// Decode a persisted `wire` companion against an absolute canonical `base`,
/// producing the syntactically resolved absolute path (before any filesystem
/// probe). Fail-closed: every malformed or unsafe spelling is an `Err`.
pub(crate) fn decode_instance_relative(wire: &str, base: &Path) -> Result<PathBuf, RelDecodeError> {
    if !base.is_absolute() {
        return Err(RelDecodeError::BaseNotAbsolute);
    }
    if wire.is_empty() {
        return Err(RelDecodeError::Empty);
    }
    if wire.contains('\0') {
        return Err(RelDecodeError::Nul);
    }
    // Reject `\` on every host so a Windows-authored value cannot smuggle a
    // separator and a Unix filename containing `\` has no portable companion.
    if wire.contains('\\') {
        return Err(RelDecodeError::Backslash);
    }
    // `.` is the only spelling for the base itself.
    if wire == "." {
        return Ok(base.to_path_buf());
    }

    let mut result = base.to_path_buf();
    for seg in wire.split('/') {
        if seg.is_empty() {
            // Leading/trailing/repeated separator.
            return Err(RelDecodeError::EmptySegment);
        }
        if seg == "." {
            return Err(RelDecodeError::DotSegment);
        }
        if seg == ".." {
            // Popping past the root is walking above the filesystem root.
            if !result.pop() {
                return Err(RelDecodeError::EscapesRoot);
            }
            continue;
        }
        validate_wire_component(seg)?;
        result.push(seg);
    }
    Ok(result)
}

/// Validate one non-`.`/`..` wire component before it is pushed onto the base.
fn validate_wire_component(seg: &str) -> Result<(), RelDecodeError> {
    // Drive-relative like `C:` or `C:dir` is rejected on every host so a
    // relative field can never smuggle a drive root.
    let bytes = seg.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Err(RelDecodeError::DriveRelative);
    }
    #[cfg(windows)]
    {
        for ch in seg.chars() {
            if (ch as u32) < 0x20 || matches!(ch, '<' | '>' | ':' | '"' | '|' | '?' | '*') {
                return Err(RelDecodeError::IllegalWindowsChar);
            }
        }
        if seg.ends_with(' ') || seg.ends_with('.') {
            return Err(RelDecodeError::TrailingDotOrSpace);
        }
        if is_reserved_dos_name(seg) {
            return Err(RelDecodeError::ReservedDosName);
        }
    }
    Ok(())
}

/// Case-insensitive DOS device basename check (CON, PRN, AUX, NUL, COM1-9,
/// LPT1-9). The device name matches on the portion before the first `.`, so
/// `CON.txt` and an ADS spelling both resolve to the reserved device.
#[cfg(windows)]
fn is_reserved_dos_name(seg: &str) -> bool {
    let stem = seg.split('.').next().unwrap_or(seg);
    let upper = stem.to_ascii_uppercase();
    if matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL") {
        return true;
    }
    let bytes = upper.as_bytes();
    upper.len() == 4
        && (upper.starts_with("COM") || upper.starts_with("LPT"))
        && bytes[3].is_ascii_digit()
        && bytes[3] != b'0'
}

// ── #1077 candidate validity and directory identity (§3.4) ─────────────────

/// Filesystem identity of a canonical directory. On Unix `(st_dev, st_ino)`; on
/// Windows `(dwVolumeSerialNumber, file index)` from an open handle. Used to
/// decide whether two valid candidate spellings name the same directory without
/// lowercasing path strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct DirectoryIdentity {
    pub volume: u64,
    pub file: u128,
}

/// Whether a validated directory is a direct project (`<target>/.ac` is a dir)
/// or a legacy collection root (has a non-dot child project).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectKind {
    DirectProject,
    CollectionRoot,
    None,
}

/// Classified filesystem-probe failure. Kept distinct so a permission/I-O error
/// never collapses into "missing".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProbeError {
    NotFound,
    PermissionDenied,
    Io,
}

/// Per-side resolution status (internal diagnostics, §3.7). `is_valid()` marks
/// the two loadable outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SideStatus {
    /// The field was absent (or its value was JSON null).
    Absent,
    /// No authoritative instance base to resolve a relative companion.
    BaseUnavailable,
    /// The stored value could not be parsed into an absolute candidate.
    Malformed,
    /// Definite `NotFound` at canonicalize/probe.
    Missing,
    /// `PermissionDenied` at canonicalize/probe.
    Inaccessible,
    /// Any other probe I/O failure.
    ProbeIoError,
    /// The canonical target exists but is not a directory.
    NotADirectory,
    /// The directory exists but is neither a direct project nor a collection root.
    AcRootOrCollectionMissing,
    /// The canonical path is not losslessly representable as UTF-8.
    NonUtf8,
    ValidDirectProject,
    ValidCollectionRoot,
    // Note: an identity-unavailable outcome (two spellings that cannot be proven
    // the same directory) is recorded at the pair level as an `Invalid` issue,
    // not as a per-side status, so both sides retain their valid classification.
}

impl SideStatus {
    pub(crate) fn is_valid(self) -> bool {
        matches!(
            self,
            SideStatus::ValidDirectProject | SideStatus::ValidCollectionRoot
        )
    }
}

/// Resolution outcome of one side (absolute or instance-relative) of a logical
/// registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SideOutcome {
    pub status: SideStatus,
    /// The syntactically resolved absolute path (pre-probe), for diagnostics.
    pub syntactic_path: Option<String>,
    /// The raw canonical path (verbatim spelling on Windows) when validated.
    /// Used for comparison and re-encoding; the outward/selected form is derived
    /// via `display_canonical`.
    pub canonical_path: Option<String>,
    /// Directory identity when it could be obtained (best-effort).
    pub identity: Option<DirectoryIdentity>,
}

impl SideOutcome {
    fn failed(status: SideStatus, syntactic: Option<String>) -> Self {
        SideOutcome {
            status,
            syntactic_path: syntactic,
            canonical_path: None,
            identity: None,
        }
    }
}

/// Injected seam: resolve one syntactically-absolute candidate path to a
/// [`SideOutcome`]. The production implementation canonicalizes, proves a
/// directory, probes project/collection kind, and fetches the handle identity;
/// tests supply canned outcomes so permission/I-O/identity matrices need no
/// `chmod` or process-global hooks (§3.4).
pub(crate) trait CandidateResolver {
    fn resolve(&self, syntactic: &Path) -> SideOutcome;
}

/// Production resolver backed by the real filesystem.
pub(crate) struct FsCandidateResolver;

impl CandidateResolver for FsCandidateResolver {
    fn resolve(&self, syntactic: &Path) -> SideOutcome {
        validate_fs_candidate(syntactic)
    }
}

fn classify_io_error(e: &std::io::Error) -> SideStatus {
    match e.kind() {
        std::io::ErrorKind::NotFound => SideStatus::Missing,
        std::io::ErrorKind::PermissionDenied => SideStatus::Inaccessible,
        _ => SideStatus::ProbeIoError,
    }
}

fn classify_probe_error(e: &std::io::Error) -> ProbeError {
    match e.kind() {
        std::io::ErrorKind::NotFound => ProbeError::NotFound,
        std::io::ErrorKind::PermissionDenied => ProbeError::PermissionDenied,
        _ => ProbeError::Io,
    }
}

/// Real-filesystem validation of a syntactically-absolute candidate. Follows a
/// user-supplied symlink/junction to its canonical target (distinct from
/// `is_real_directory`, which rejects links for child enumeration).
fn validate_fs_candidate(syntactic: &Path) -> SideOutcome {
    let syntactic_str = syntactic.to_str().map(str::to_string);
    let canon = match std::fs::canonicalize(syntactic) {
        Ok(c) => c,
        Err(e) => return SideOutcome::failed(classify_io_error(&e), syntactic_str),
    };
    let canonical_str = match canon.to_str() {
        Some(s) => s.to_string(),
        None => return SideOutcome::failed(SideStatus::NonUtf8, syntactic_str),
    };
    match std::fs::metadata(&canon) {
        Ok(md) if md.is_dir() => {}
        Ok(_) => {
            return SideOutcome {
                status: SideStatus::NotADirectory,
                syntactic_path: syntactic_str,
                canonical_path: Some(canonical_str),
                identity: None,
            };
        }
        Err(e) => {
            return SideOutcome {
                status: classify_io_error(&e),
                syntactic_path: syntactic_str,
                canonical_path: Some(canonical_str),
                identity: None,
            };
        }
    }
    let status = match probe_project_kind(&canon) {
        Ok(ProjectKind::DirectProject) => SideStatus::ValidDirectProject,
        Ok(ProjectKind::CollectionRoot) => SideStatus::ValidCollectionRoot,
        Ok(ProjectKind::None) => {
            return SideOutcome {
                status: SideStatus::AcRootOrCollectionMissing,
                syntactic_path: syntactic_str,
                canonical_path: Some(canonical_str),
                identity: None,
            };
        }
        Err(ProbeError::NotFound) => {
            return SideOutcome::failed(SideStatus::Missing, syntactic_str);
        }
        Err(ProbeError::PermissionDenied) => {
            return SideOutcome {
                status: SideStatus::Inaccessible,
                syntactic_path: syntactic_str,
                canonical_path: Some(canonical_str),
                identity: None,
            };
        }
        Err(ProbeError::Io) => {
            return SideOutcome {
                status: SideStatus::ProbeIoError,
                syntactic_path: syntactic_str,
                canonical_path: Some(canonical_str),
                identity: None,
            };
        }
    };
    let identity = directory_identity(&canon).ok();
    SideOutcome {
        status,
        syntactic_path: syntactic_str,
        canonical_path: Some(canonical_str),
        identity,
    }
}

/// Probe whether `target` (already canonical + proven a directory) is a direct
/// project or a legacy one-level collection root. A permission/I-O failure that
/// prevents proving a collection root is surfaced, never flattened to "missing".
fn probe_project_kind(target: &Path) -> Result<ProjectKind, ProbeError> {
    match std::fs::metadata(target.join(".ac")) {
        Ok(md) if md.is_dir() => return Ok(ProjectKind::DirectProject),
        Ok(_) => {}
        Err(e) => match e.kind() {
            std::io::ErrorKind::NotFound => {}
            std::io::ErrorKind::PermissionDenied => return Err(ProbeError::PermissionDenied),
            _ => return Err(ProbeError::Io),
        },
    }

    let entries = std::fs::read_dir(target).map_err(|e| classify_probe_error(&e))?;
    let mut probe_error: Option<ProbeError> = None;
    for entry in entries {
        let entry = match entry {
            Ok(en) => en,
            Err(e) => {
                probe_error.get_or_insert(classify_probe_error(&e));
                continue;
            }
        };
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let child = entry.path();
        match std::fs::metadata(&child) {
            Ok(md) if md.is_dir() => {}
            Ok(_) => continue,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                probe_error.get_or_insert(classify_probe_error(&e));
                continue;
            }
        }
        match std::fs::metadata(child.join(".ac")) {
            Ok(md) if md.is_dir() => return Ok(ProjectKind::CollectionRoot),
            Ok(_) => continue,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                probe_error.get_or_insert(classify_probe_error(&e));
                continue;
            }
        }
    }
    match probe_error {
        Some(err) => Err(err),
        None => Ok(ProjectKind::None),
    }
}

/// Directory identity of a canonical directory via an open handle. Follows the
/// (already-resolved) target; does not reuse `seed_manifest`'s reparse-detecting
/// open. Mirrors the reviewed handle pattern without coupling to its types.
#[cfg(windows)]
fn directory_identity(path: &Path) -> std::io::Result<DirectoryIdentity> {
    use std::mem::MaybeUninit;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let file = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?;
    let mut info = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
    let ok =
        unsafe { GetFileInformationByHandle(file.as_raw_handle() as HANDLE, info.as_mut_ptr()) };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let info = unsafe { info.assume_init() };
    let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    Ok(DirectoryIdentity {
        volume: u64::from(info.dwVolumeSerialNumber),
        file: u128::from(index),
    })
}

#[cfg(unix)]
fn directory_identity(path: &Path) -> std::io::Result<DirectoryIdentity> {
    use std::os::unix::fs::MetadataExt;
    let md = std::fs::metadata(path)?;
    Ok(DirectoryIdentity {
        volume: md.dev(),
        file: u128::from(md.ino()),
    })
}

#[cfg(not(any(unix, windows)))]
fn directory_identity(_path: &Path) -> std::io::Result<DirectoryIdentity> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "handle identity unavailable on this platform; canonical path equality is the fallback",
    ))
}

/// Whether two validated candidates name the same directory. Canonical string
/// equality short-circuits to `Same`; otherwise identity decides. When identity
/// cannot be established (unix/windows handle failure) the result is
/// `Unavailable` and the caller fails closed. On other platforms canonical
/// equality is the sole, documented signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SameDir {
    Same,
    Different,
    Unavailable,
}

pub(crate) fn compare_dirs(
    a_canonical: &str,
    a_identity: Option<&DirectoryIdentity>,
    b_canonical: &str,
    b_identity: Option<&DirectoryIdentity>,
) -> SameDir {
    if a_canonical == b_canonical {
        return SameDir::Same;
    }
    #[cfg(any(unix, windows))]
    {
        match (a_identity, b_identity) {
            (Some(x), Some(y)) => {
                if x == y {
                    SameDir::Same
                } else {
                    SameDir::Different
                }
            }
            _ => SameDir::Unavailable,
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (a_identity, b_identity);
        SameDir::Different
    }
}

/// Convert a canonical path to its outward/selected form: strip only the
/// ordinary Windows verbatim drive/UNC prefixes for display, preserving any
/// unsupported device-namespace spelling losslessly (§3.4).
#[cfg(windows)]
pub(crate) fn display_canonical(canonical: &str) -> String {
    if let Some(rest) = canonical.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{}", rest)
    } else if let Some(rest) = canonical.strip_prefix(r"\\?\") {
        let bytes = rest.as_bytes();
        if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
            rest.to_string()
        } else {
            canonical.to_string()
        }
    } else {
        canonical.to_string()
    }
}

#[cfg(not(windows))]
pub(crate) fn display_canonical(canonical: &str) -> String {
    canonical.to_string()
}

// ── #1077 resolution matrix, merge, quarantine, dedup (§3.4-§3.7) ───────────

/// Which persisted field group a logical registration belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectSource {
    /// The legacy singular `projectPath` pair.
    ProjectPath,
    /// The active plural `projectPaths` pair.
    ProjectPaths,
    /// The archived plural `archivedProjectPaths` pair.
    ArchivedProjectPaths,
}

/// Raw presence + value of one persisted string field. `present == false`
/// requires `value == None` (absent); `present == true, value == None` is an
/// explicit JSON null.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct RawStringField {
    pub present: bool,
    pub value: Option<String>,
}

impl RawStringField {
    pub(crate) fn absent() -> Self {
        RawStringField {
            present: false,
            value: None,
        }
    }
    pub(crate) fn string(s: impl Into<String>) -> Self {
        RawStringField {
            present: true,
            value: Some(s.into()),
        }
    }
    pub(crate) fn null() -> Self {
        RawStringField {
            present: true,
            value: None,
        }
    }
}

/// One logical registration's raw absolute + instance-relative slots, extracted
/// from the six-field schema before resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawPair {
    pub source: ProjectSource,
    pub index: Option<usize>,
    pub absolute: RawStringField,
    pub relative: RawStringField,
}

/// Blocking issue classification for a registration that selected no path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IssueKind {
    Conflict,
    Missing,
    Invalid,
}

/// Raw presence + value of a persisted field as arbitrary JSON (absent vs null
/// vs value). Used to preserve and report structurally-corrupt fields.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct RawJsonField {
    pub present: bool,
    pub value: Option<serde_json::Value>,
}

/// A structural-schema corruption in one project field group (wrong JSON type,
/// misaligned companion, orphan companion, etc.). Distinct from a per-pair
/// candidate that merely fails to load: it exposes no entry from the affected
/// list, preserves the raw bytes, and blocks all reconciliation/mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StructuralIssue {
    pub source: ProjectSource,
    pub reason: String,
    pub raw_absolute: RawJsonField,
    pub raw_relative: RawJsonField,
}

/// The write, if any, a resolved pair needs to reconcile its companion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RepairKind {
    /// Clean; no write.
    None,
    /// Valid absolute, companion absent/stale → generate a fresh companion.
    PopulateCompanion,
    /// Relative won → replace the stale absolute and refresh the companion.
    ReplaceStaleAbsolute,
    /// Both valid same directory → normalize a noncanonical pair.
    NormalizePair,
}

/// The fully resolved state of one logical registration (hidden persisted state).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedPair {
    pub source: ProjectSource,
    pub index: Option<usize>,
    pub raw_absolute: RawStringField,
    pub raw_relative: RawStringField,
    pub absolute_side: SideOutcome,
    pub relative_side: SideOutcome,
    /// Selected outward (display) canonical absolute path, if any.
    pub selected: Option<String>,
    /// Raw canonical (verbatim on Windows) of the selected side, for identity
    /// and component-scope comparison.
    pub selected_canonical_raw: Option<String>,
    /// Identity of the selected directory, if known.
    pub selected_identity: Option<DirectoryIdentity>,
    /// Non-`None` when this registration selected no path (or was quarantined).
    pub issue: Option<IssueKind>,
    /// The companion repair this pair needs, when it selected a path.
    pub repair: RepairKind,
}

impl ResolvedPair {
    fn is_selected(&self) -> bool {
        self.selected.is_some() && self.issue.is_none()
    }
}

/// Hidden persisted project-path state carried on `AppSettings` behind an
/// `Arc` (`#[serde(skip)]`). Retains every resolved pair's source/order/raw
/// values/selected path/identity/issue and the reconcile-eligibility bits, so
/// writers can rebuild companion arrays and the snapshot can report issues
/// without re-reading disk. Mutators use `Arc::make_mut` for copy-on-write.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ProjectPathPersistenceState {
    /// Every resolved pair (hidden state) in (active plural, genuine singular,
    /// archived) order. The first `active_registration_count` entries are the
    /// active group; the rest are archived.
    pub pairs: Vec<ResolvedPair>,
    /// Legacy singular head = first active selected path (or None).
    pub selected_head: Option<String>,
    /// Merged logical active registration count (before identity dedupe/quarantine).
    pub active_registration_count: usize,
    /// Merged logical archived registration count.
    pub archived_registration_count: usize,
    /// Whether the active plural companion array was present & aligned on disk.
    pub active_companion_present: bool,
    /// Whether the archived plural companion array was present & aligned on disk.
    pub archived_companion_present: bool,
    /// Whether a genuinely-different singular is appended to the active group.
    pub has_genuine_singular: bool,
    /// Whether the active group has a dirty, eligible repair to write.
    pub active_reconcile_eligible: bool,
    /// Whether the archived group has a dirty, eligible repair to write.
    pub archived_reconcile_eligible: bool,
    /// Structural-schema corruptions found in the raw fields. Any entry blocks
    /// all reconciliation and project-list mutation while unrelated preserve
    /// saves remain allowed.
    pub structural_issues: Vec<StructuralIssue>,
    /// Set for a state synthesized from a direct-constructed `AppSettings`'s
    /// runtime lists (no decoder-produced hidden state). Such a state is legacy
    /// runtime-authoritative: a Reconcile write emits its groups verbatim rather
    /// than copying disk, matching pre-#1077 verbatim-writer behavior until the
    /// explicit mutators maintain a real hidden state.
    pub runtime_authoritative: bool,
}

impl ProjectPathPersistenceState {
    /// The active-group resolved pairs (plural then genuine singular).
    pub(crate) fn active_pairs(&self) -> &[ResolvedPair] {
        &self.pairs[..self.active_registration_count]
    }
    /// The archived-group resolved pairs.
    pub(crate) fn archived_pairs(&self) -> &[ResolvedPair] {
        &self.pairs[self.active_registration_count..]
    }
    /// Selected canonical (display) active paths, in order, filtered of
    /// unresolved/quarantined/deduped entries.
    pub(crate) fn active_selected(&self) -> Vec<String> {
        self.active_pairs()
            .iter()
            .filter_map(|p| p.selected.clone())
            .collect()
    }
    /// Archived management projection (§3.6): every archived pair that is either
    /// selected or a non-conflicting removable stored row (missing/invalid),
    /// keeping "Remove from list" working for a project whose directory vanished
    /// while never exposing one side of an archived conflict. This is the runtime
    /// `archivedProjectPaths` projection; the active list stays selected-only so
    /// session restoration never restores an unvalidated project.
    pub(crate) fn archived_management_paths(&self) -> Vec<String> {
        self.archived_pairs()
            .iter()
            .filter_map(|p| match (&p.selected, p.issue) {
                (Some(sel), _) => Some(sel.clone()),
                (None, Some(IssueKind::Conflict)) => None,
                (None, Some(_)) => p.raw_absolute.value.clone(),
                (None, None) => None,
            })
            .collect()
    }
    /// Blocking issues (registrations that selected no path), in `pairs` order.
    pub(crate) fn issues(&self) -> impl Iterator<Item = &ResolvedPair> {
        self.pairs.iter().filter(|p| p.issue.is_some())
    }
    /// Whether any structural corruption is present (blocks all reconcile/mutation).
    pub(crate) fn has_structural(&self) -> bool {
        !self.structural_issues.is_empty()
    }
}

/// Resolve one side (absolute or relative) of a registration against the base.
fn resolve_side(
    field: &RawStringField,
    is_relative: bool,
    base: Option<&Path>,
    resolver: &dyn CandidateResolver,
) -> SideOutcome {
    // Absent or explicit null → nothing to resolve.
    let value = match (field.present, field.value.as_deref()) {
        (false, _) | (true, None) => return SideOutcome::failed(SideStatus::Absent, None),
        (true, Some(v)) => v,
    };

    if is_relative {
        let Some(base) = base else {
            return SideOutcome::failed(SideStatus::BaseUnavailable, None);
        };
        match decode_instance_relative(value, base) {
            Ok(syntactic) => resolver.resolve(&syntactic),
            Err(_) => SideOutcome::failed(SideStatus::Malformed, None),
        }
    } else {
        let abs = PathBuf::from(value);
        if !abs.is_absolute() {
            return SideOutcome::failed(SideStatus::Malformed, Some(value.to_string()));
        }
        resolver.resolve(&abs)
    }
}

/// Classify a no-selection outcome. Returns `None` for a genuinely empty pair
/// (both sides absent), `Some(Missing)` when the only non-absent statuses are
/// definite `NotFound`, and `Some(Invalid)` otherwise.
fn classify_no_selection(abs: SideStatus, rel: SideStatus) -> Option<IssueKind> {
    let mut any = false;
    let mut all_missing = true;
    for status in [abs, rel] {
        if status == SideStatus::Absent {
            continue;
        }
        any = true;
        if status != SideStatus::Missing {
            all_missing = false;
        }
    }
    if !any {
        None
    } else if all_missing {
        Some(IssueKind::Missing)
    } else {
        Some(IssueKind::Invalid)
    }
}

/// Combine both resolved sides into a selected path + repair need + issue,
/// per the §3.5 resolution matrix.
fn combine_sides(pair: RawPair, abs: SideOutcome, rel: SideOutcome) -> ResolvedPair {
    let av = abs.status.is_valid();
    let rv = rel.status.is_valid();

    let mut selected = None;
    let mut selected_canonical_raw = None;
    let mut selected_identity = None;
    let mut issue = None;
    let mut repair = RepairKind::None;

    match (av, rv) {
        (true, true) => {
            let same = compare_dirs(
                abs.canonical_path.as_deref().unwrap_or_default(),
                abs.identity.as_ref(),
                rel.canonical_path.as_deref().unwrap_or_default(),
                rel.identity.as_ref(),
            );
            match same {
                SameDir::Same => {
                    // Prefer the absolute candidate's canonical form. A valid side
                    // always carries a canonical path; default defensively rather
                    // than panicking if that invariant is ever weakened.
                    let canon = abs.canonical_path.clone().unwrap_or_default();
                    selected = Some(display_canonical(&canon));
                    selected_canonical_raw = Some(canon);
                    selected_identity = abs.identity;
                    // Normalize if the persisted spellings are not already the
                    // selected canonical forms.
                    if !pair_is_canonical(&pair, selected.as_deref(), &rel) {
                        repair = RepairKind::NormalizePair;
                    }
                }
                SameDir::Different => {
                    issue = Some(IssueKind::Conflict);
                }
                SameDir::Unavailable => {
                    issue = Some(IssueKind::Invalid);
                }
            }
        }
        (true, false) => {
            let canon = abs.canonical_path.clone().unwrap_or_default();
            selected = Some(display_canonical(&canon));
            selected_canonical_raw = Some(canon);
            selected_identity = abs.identity;
            // Populate/repair the companion when the relative side did not load.
            repair = RepairKind::PopulateCompanion;
        }
        (false, true) => {
            let canon = rel.canonical_path.clone().unwrap_or_default();
            selected = Some(display_canonical(&canon));
            selected_canonical_raw = Some(canon);
            selected_identity = rel.identity;
            repair = RepairKind::ReplaceStaleAbsolute;
        }
        (false, false) => {
            issue = classify_no_selection(abs.status, rel.status);
        }
    }

    ResolvedPair {
        source: pair.source,
        index: pair.index,
        raw_absolute: pair.absolute,
        raw_relative: pair.relative,
        absolute_side: abs,
        relative_side: rel,
        selected,
        selected_canonical_raw,
        selected_identity,
        issue,
        repair,
    }
}

/// Whether the persisted pair already stores the selected absolute path (display
/// form) plus a present, non-drifting companion, so no normalization is needed.
fn pair_is_canonical(pair: &RawPair, selected: Option<&str>, rel: &SideOutcome) -> bool {
    let Some(selected) = selected else {
        return false;
    };
    let abs_matches = pair
        .absolute
        .value
        .as_deref()
        .map(|v| v == selected)
        .unwrap_or(false);
    // The companion is fine if it is present and it resolved to the same dir.
    let rel_ok = pair.relative.present && rel.status.is_valid();
    abs_matches && rel_ok
}

/// Resolve the full six-field record set: run the §3.5 matrix per registration,
/// merge the legacy singular mirror, quarantine conflict scopes, deduplicate by
/// identity, and build the runtime selected lists plus hidden state.
///
/// `base` is the already-canonicalized instance base (or `None`). The
/// `*_companion_present` flags say whether each plural relative array was
/// present and length-aligned on disk (drives reconcile eligibility, §4.2).
pub(crate) fn resolve_registrations(
    active_plural: &[RawPair],
    singular: Option<RawPair>,
    archived: &[RawPair],
    base: Option<&Path>,
    active_companion_present: bool,
    archived_companion_present: bool,
    resolver: &dyn CandidateResolver,
) -> ProjectPathPersistenceState {
    let resolve_pair = |pair: RawPair| -> ResolvedPair {
        let abs = resolve_side(&pair.absolute, false, base, resolver);
        let rel = resolve_side(&pair.relative, true, base, resolver);
        combine_sides(pair, abs, rel)
    };

    let mut active: Vec<ResolvedPair> = active_plural.iter().cloned().map(resolve_pair).collect();

    // Merge the legacy singular mirror (§3.6).
    let mut genuine_singular = false;
    if let Some(sing) = singular {
        let resolved_singular = resolve_pair(sing);
        match active.first() {
            Some(first) if singular_mirrors_first(&resolved_singular, first) => {
                // Redundant mirror of the first plural entry; drop it.
            }
            _ => {
                genuine_singular = true;
                active.push(resolved_singular);
            }
        }
    }

    let mut archived: Vec<ResolvedPair> = archived.iter().cloned().map(resolve_pair).collect();

    // Counts BEFORE dedupe/quarantine (redundant singular already excluded; a
    // genuinely different singular counts as one extra active record).
    let active_registration_count = active.len();
    let archived_registration_count = archived.len();

    // Conflict-quarantine pass (§3.4): suppress any otherwise-selected record
    // whose identity or component scope overlaps either candidate of a conflict.
    let quarantine = build_quarantine_set(&active, &archived);
    apply_quarantine(&mut active, &quarantine);
    apply_quarantine(&mut archived, &quarantine);

    // Identity dedupe (§3.6): active in order first, then archived; active wins.
    dedupe_selected(&mut active, &mut archived);

    // Reconcile eligibility per group (§4.2).
    let active_reconcile_eligible =
        group_reconcile_eligible(&active, active_companion_present, genuine_singular);
    let archived_reconcile_eligible =
        group_reconcile_eligible(&archived, archived_companion_present, false);

    let selected_head = active.iter().find_map(|p| p.selected.clone());

    let mut pairs = active;
    pairs.extend(archived);

    ProjectPathPersistenceState {
        pairs,
        selected_head,
        active_registration_count,
        archived_registration_count,
        active_companion_present,
        archived_companion_present,
        has_genuine_singular: genuine_singular,
        active_reconcile_eligible,
        archived_reconcile_eligible,
        structural_issues: Vec::new(),
        runtime_authoritative: false,
    }
}

/// Whether a resolved singular is the redundant mirror of the first plural
/// entry: matching raw absolute spelling (platform-correct) or the same
/// validated directory identity.
fn singular_mirrors_first(singular: &ResolvedPair, first: &ResolvedPair) -> bool {
    if let (Some(a), Some(b)) = (
        singular.raw_absolute.value.as_deref(),
        first.raw_absolute.value.as_deref(),
    ) {
        if normalize_for_compare(a) == normalize_for_compare(b) {
            return true;
        }
    }
    if let (Some(a), Some(b)) = (
        &singular.selected_canonical_raw,
        &first.selected_canonical_raw,
    ) {
        return matches!(
            compare_dirs(
                a,
                singular.selected_identity.as_ref(),
                b,
                first.selected_identity.as_ref()
            ),
            SameDir::Same
        );
    }
    false
}

/// Both canonical identities and canonical component paths of every
/// both-valid/different (conflict) pair.
struct QuarantineSet {
    identities: Vec<DirectoryIdentity>,
    canonical_paths: Vec<String>,
}

fn build_quarantine_set(active: &[ResolvedPair], archived: &[ResolvedPair]) -> QuarantineSet {
    let mut identities = Vec::new();
    let mut canonical_paths = Vec::new();
    for pair in active.iter().chain(archived.iter()) {
        if pair.issue == Some(IssueKind::Conflict) {
            for side in [&pair.absolute_side, &pair.relative_side] {
                if let Some(id) = side.identity {
                    identities.push(id);
                }
                if let Some(canon) = &side.canonical_path {
                    canonical_paths.push(canon.clone());
                }
            }
        }
    }
    QuarantineSet {
        identities,
        canonical_paths,
    }
}

/// Suppress any selected pair whose identity equals a quarantined candidate or
/// whose canonical scope overlaps one component-wise (ancestor/descendant/equal).
fn apply_quarantine(pairs: &mut [ResolvedPair], quarantine: &QuarantineSet) {
    for pair in pairs.iter_mut() {
        if !pair.is_selected() {
            continue;
        }
        let id_hit = pair
            .selected_identity
            .as_ref()
            .map(|id| quarantine.identities.contains(id))
            .unwrap_or(false);
        let scope_hit = pair
            .selected_canonical_raw
            .as_deref()
            .map(|canon| {
                quarantine
                    .canonical_paths
                    .iter()
                    .any(|q| paths_overlap(canon, q))
            })
            .unwrap_or(false);
        if id_hit || scope_hit {
            pair.selected = None;
            pair.selected_canonical_raw = None;
            pair.selected_identity = None;
            pair.repair = RepairKind::None;
            pair.issue = Some(IssueKind::Invalid);
        }
    }
}

/// Component-wise ancestor/descendant/equal overlap of two canonical paths.
fn paths_overlap(a: &str, b: &str) -> bool {
    let (pa, pb) = (Path::new(a), Path::new(b));
    pa.starts_with(pb) || pb.starts_with(pa)
}

/// Identity dedupe across selected records: active in order then archived, active
/// winning over a duplicate archived entry. Identity-unavailable comparison of
/// two distinct-canonical selected records fails the later one closed.
fn dedupe_selected(active: &mut [ResolvedPair], archived: &mut [ResolvedPair]) {
    let mut kept: Vec<(String, Option<DirectoryIdentity>)> = Vec::new();
    // Two passes over one logical ordering: active first (wins), then archived.
    for pair in active.iter_mut().chain(archived.iter_mut()) {
        if !pair.is_selected() {
            continue;
        }
        let canon = pair.selected_canonical_raw.clone().unwrap_or_default();
        let id = pair.selected_identity;
        let mut duplicate = false;
        let mut unavailable = false;
        for (kcanon, kid) in &kept {
            match compare_dirs(&canon, id.as_ref(), kcanon, kid.as_ref()) {
                SameDir::Same => {
                    duplicate = true;
                    break;
                }
                SameDir::Unavailable => {
                    unavailable = true;
                    break;
                }
                SameDir::Different => {}
            }
        }
        if duplicate {
            // Silent dedupe: drop the later duplicate from runtime.
            pair.selected = None;
            pair.selected_canonical_raw = None;
            pair.selected_identity = None;
            pair.repair = RepairKind::None;
        } else if unavailable {
            // Cannot rule out a duplicate → fail closed.
            pair.selected = None;
            pair.selected_canonical_raw = None;
            pair.selected_identity = None;
            pair.repair = RepairKind::None;
            pair.issue = Some(IssueKind::Invalid);
        } else {
            kept.push((canon, id));
        }
    }
}

/// Whether a field group has a dirty, write-eligible repair (§4.2). A present,
/// aligned companion array can always copy unresolved slots, so any dirty pair
/// makes the group eligible. An absent companion array requires every retained
/// entry to have selected (one unresolved legacy entry blocks the group). A
/// genuinely different unresolved active singular also blocks the active group.
fn group_reconcile_eligible(
    pairs: &[ResolvedPair],
    companion_present: bool,
    genuine_singular: bool,
) -> bool {
    let has_dirty = pairs
        .iter()
        .any(|p| p.is_selected() && p.repair != RepairKind::None);
    if !has_dirty {
        return false;
    }
    let has_unresolved = pairs.iter().any(|p| p.issue.is_some());
    if genuine_singular && has_unresolved {
        return false;
    }
    if companion_present {
        true
    } else {
        // Absent companion: cannot invent a null slot for an unresolved entry.
        !has_unresolved
    }
}

#[cfg(test)]
mod tests;
