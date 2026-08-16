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

use super::settings::AppSettings;
use super::ac_root::{existing_ac_root, has_ac_root, ac_root_for_project};

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
    fn refresh(&self, settings: &mut AppSettings) -> Result<(), ProjectError> {
        let result = match self {
            #[cfg(test)]
            Self::CallerOwned => Ok(()),
            #[cfg(test)]
            Self::Path(path) => {
                crate::config::settings::refresh_project_paths_from_path(settings, path)
            }
            #[cfg(not(test))]
            Self::Production => crate::config::settings::refresh_project_paths_from_disk(settings),
        };
        result.map_err(|error| ProjectError::ProjectSettingsFailed {
            operation: "refresh",
            error,
        })
    }

    fn save(&self, settings: &AppSettings) -> Result<(), ProjectError> {
        let result = match self {
            #[cfg(test)]
            Self::CallerOwned => Ok(()),
            #[cfg(test)]
            Self::Path(path) => {
                crate::config::settings::save_settings_with_project_paths_to_path(settings, path)
            }
            #[cfg(not(test))]
            Self::Production => crate::config::settings::save_settings_with_project_paths(settings),
        };
        result.map_err(|error| ProjectError::ProjectSettingsFailed {
            operation: "persist",
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
    let prepared =
        prepare_new_project_impl(raw_path, activation, |ac_root, on_publication| {
            crate::config::session_context::create_default_context_templates_with_publications(
                ac_root,
                on_publication,
            )
        })?;
    store.refresh(settings)?;
    let before_settings = settings.clone();
    let result = commit_prepared_new_project(settings, &prepared);
    if let Err(error) = prepared.revalidate() {
        *settings = before_settings;
        return Err(error);
    }
    // Save while the project gate is held when the historical CLI contract
    // would save. Its caller-owned repeat is the same snapshot after this
    // transaction and does not establish registration authority.
    let saved = result.created || result.registered;
    if saved {
        store.save(settings)?;
    }
    if let Err(error) = prepared.revalidate() {
        *settings = before_settings.clone();
        if saved {
            if let Err(rollback_error) = store.save(&before_settings) {
                return Err(ProjectError::ProjectSetupChanged(
                    prepared.abs.clone(),
                    format!(
                        "{}; failed to roll back project registration settings: {}",
                        error, rollback_error
                    ),
                ));
            }
        }
        return Err(error);
    }
    prepared.release();
    Ok(result)
}

#[cfg(test)]
fn register_new_project_with_settings_path(
    settings: &mut AppSettings,
    raw_path: &str,
    settings_path: &Path,
    activation: Option<&crate::config::seed_manifest::ManifestActivationToken>,
) -> Result<ProjectRegistration, ProjectError> {
    register_new_project_with_store(
        settings,
        raw_path,
        NewProjectSettingsStore::Path(settings_path.to_path_buf()),
        activation,
    )
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
    prepare_new_project_impl(
        raw_path,
        activation.as_ref(),
        |ac_root, on_publication| {
            crate::config::session_context::create_default_context_templates_with_publications(
                ac_root,
                on_publication,
            )
        },
    )
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
    std::fs::create_dir_all(&abs)
        .map_err(|e| ProjectError::AcRootCreateFailed(abs.clone(), e))?;

    let ac_root = ac_root_for_project(&abs);
    // The create syscall is the sole creation-intent authority. An earlier
    // existence observation cannot distinguish this caller from a contender.
    let created = match std::fs::create_dir(&ac_root) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(e) => {
            return Err(ProjectError::AcRootCreateFailed(
                ac_root.clone(),
                e,
            ))
        }
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
            return Err(ProjectError::AcRootGitignoreFailed(
                ac_root.clone(),
                e,
            ));
        }
    } else {
        // Gitignore sweep (Round-1 G15): best-effort when `.ac` pre-existed
        // because a transient FS error on someone else's gitignore should not
        // fail registration of a valid project.
        if let Err(e) = crate::commands::ac_discovery::ensure_ac_root_gitignore(&ac_root) {
            log::warn!(
                "[projects] gitignore sweep failed on pre-existing Project AC Root at {:?}: {} (best-effort, continuing)",
                ac_root, e
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
    IllegalWindowsChar,
    /// A reserved DOS device basename (CON, PRN, AUX, NUL, COM1-9, LPT1-9).
    ReservedDosName,
    /// A component ending in a space or dot (Win32 strips these).
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
mod tests {
    use super::*;
    use crate::config::settings::AppSettings;
    use std::path::Path;
    use std::sync::{Mutex, OnceLock};

    /// Auto-cleaned temp dir; mirrors `cli::task_ops::tests::FixtureRoot`.
    struct FixtureRoot(PathBuf);
    impl Drop for FixtureRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    impl FixtureRoot {
        fn new(prefix: &str) -> Self {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            std::process::id().hash(&mut h);
            std::thread::current().id().hash(&mut h);
            let path = std::env::temp_dir().join(format!(
                "{}-{}-{}",
                prefix,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0),
                h.finish()
            ));
            std::fs::create_dir_all(&path).expect("fixture root");
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }

    // ── register_existing_project ────────────────────────────────────────

    #[test]
    fn open_rejects_empty_path() {
        let mut s = AppSettings::default();
        assert!(matches!(
            register_existing_project(&mut s, ""),
            Err(ProjectError::EmptyPath(_))
        ));
    }

    #[test]
    fn open_rejects_missing_path() {
        let fix = FixtureRoot::new("proj-open-missing");
        let p = fix.path().join("does-not-exist");
        let mut s = AppSettings::default();
        let r = register_existing_project(&mut s, p.to_str().unwrap());
        assert!(matches!(r, Err(ProjectError::PathMissing(_))));
        assert!(s.project_paths.is_empty());
    }

    #[test]
    fn open_rejects_path_without_ac_root() {
        let fix = FixtureRoot::new("proj-open-no-workspace");
        let mut s = AppSettings::default();
        let r = register_existing_project(&mut s, fix.path().to_str().unwrap());
        assert!(matches!(r, Err(ProjectError::AcRootMissing(_))));
        assert!(s.project_paths.is_empty());
    }

    #[test]
    fn open_registers_path_with_ac() {
        let fix = FixtureRoot::new("proj-open-ok");
        std::fs::create_dir_all(fix.path().join(".ac")).unwrap();
        let mut s = AppSettings::default();
        let r = register_existing_project(&mut s, fix.path().to_str().unwrap()).unwrap();
        assert!(r.registered);
        assert!(!r.created);
        assert_eq!(s.project_paths.len(), 1);
        assert_eq!(s.project_path.as_deref(), Some(r.path.as_str()));
    }

    #[test]
    fn open_rejects_non_ac_root() {
        let fix = FixtureRoot::new("proj-open-invalid-workspace");
        std::fs::create_dir_all(fix.path().join(".workspace")).unwrap();
        let mut s = AppSettings::default();
        let r = register_existing_project(&mut s, fix.path().to_str().unwrap());
        assert!(matches!(r, Err(ProjectError::AcRootMissing(_))));
        assert!(s.project_paths.is_empty());
    }

    #[test]
    fn open_registers_path_when_ac_exists() {
        let fix = FixtureRoot::new("proj-open-both");
        std::fs::create_dir_all(fix.path().join(".ac")).unwrap();
        let mut s = AppSettings::default();
        let r = register_existing_project(&mut s, fix.path().to_str().unwrap()).unwrap();
        assert!(r.registered);
        assert_eq!(
            existing_ac_root(fix.path()),
            Some(fix.path().join(".ac"))
        );
    }

    #[test]
    fn open_is_idempotent_on_repeat_call() {
        let fix = FixtureRoot::new("proj-open-idem");
        std::fs::create_dir_all(fix.path().join(".ac")).unwrap();
        let mut s = AppSettings::default();
        let _ = register_existing_project(&mut s, fix.path().to_str().unwrap()).unwrap();
        let r2 = register_existing_project(&mut s, fix.path().to_str().unwrap()).unwrap();
        assert!(!r2.registered);
        assert_eq!(s.project_paths.len(), 1);
    }

    #[test]
    fn open_dedup_is_case_insensitive_and_slash_agnostic() {
        let fix = FixtureRoot::new("proj-open-norm");
        std::fs::create_dir_all(fix.path().join(".ac")).unwrap();
        let mut s = AppSettings::default();
        // Seed an entry with the exact path
        let original = fix.path().to_string_lossy().to_string();
        let _ = register_existing_project(&mut s, &original).unwrap();
        // Re-register with mixed slash + case
        let mangled = original.replace('\\', "/").to_uppercase();
        let r2 = register_existing_project(&mut s, &mangled).unwrap();
        assert!(!r2.registered, "case+slash variant should dedup");
        assert_eq!(s.project_paths.len(), 1);
        // Original retained, NOT replaced with the mangled form.
        assert_eq!(s.project_paths[0], original);
    }

    // ── register_new_project ─────────────────────────────────────────────

    #[test]
    fn new_creates_ac_when_missing() {
        let fix = FixtureRoot::new("proj-new-mkdir");
        let mut s = AppSettings::default();
        let r = register_new_project(&mut s, fix.path().to_str().unwrap()).unwrap();
        assert!(r.created);
        assert!(r.registered);
        assert!(fix.path().join(".ac").is_dir());
        assert!(fix.path().join(".ac").join(".gitignore").is_file());
        assert!(fix
            .path()
            .join(".ac")
            .join(crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME)
            .is_file());
        assert!(fix
            .path()
            .join(".ac")
            .join(crate::config::session_context::COORDINATOR_CONTEXT_TEMPLATE_FILENAME)
            .is_file());
        assert!(!fix.path().join(".ac").join("templates").exists());
    }

    #[test]
    fn new_creates_parent_directory_when_missing() {
        // Covers the `create_dir_all(&abs)` branch in register_new_project
        // for a path whose project folder does NOT yet exist on disk. The
        // existing `new_creates_ac_when_missing` test passes `fix.path()`
        // which `FixtureRoot::new` already created, so the parent-mkdir
        // branch was previously unexercised.
        let fix = FixtureRoot::new("proj-new-parent");
        let nested = fix.path().join("nested-not-yet-created");
        let mut s = AppSettings::default();
        let r = register_new_project(&mut s, nested.to_str().unwrap()).unwrap();
        assert!(r.created, "should report created=true for fresh path");
        assert!(r.registered);
        assert!(nested.is_dir(), "project root should have been created");
        assert!(nested.join(".ac").is_dir());
        assert!(nested.join(".ac").join(".gitignore").is_file());
    }

    #[test]
    fn new_skips_creation_when_ac_already_exists() {
        let fix = FixtureRoot::new("proj-new-existing");
        std::fs::create_dir_all(fix.path().join(".ac")).unwrap();
        let mut s = AppSettings::default();
        let r = register_new_project(&mut s, fix.path().to_str().unwrap()).unwrap();
        assert!(!r.created);
        assert!(r.registered);
        // gitignore swept opportunistically even though .ac pre-existed
        assert!(fix.path().join(".ac").join(".gitignore").is_file());
    }

    #[test]
    fn new_does_not_overwrite_existing_context_templates() {
        let fix = FixtureRoot::new("proj-new-template-existing");
        let ac_root = fix.path().join(".ac");
        std::fs::create_dir_all(&ac_root).unwrap();
        let agent_template = ac_root.join("Context.AgentsCommander.md");
        let coordinator_template = ac_root.join("Context.coordinator.md");
        std::fs::write(&agent_template, "CUSTOM_AGENT").unwrap();
        std::fs::write(&coordinator_template, "CUSTOM_COORDINATOR").unwrap();

        let mut s = AppSettings::default();
        let r = register_new_project(&mut s, fix.path().to_str().unwrap()).unwrap();

        assert!(!r.created);
        assert_eq!(
            std::fs::read_to_string(agent_template).unwrap(),
            "CUSTOM_AGENT"
        );
        assert_eq!(
            std::fs::read_to_string(coordinator_template).unwrap(),
            "CUSTOM_COORDINATOR"
        );
    }

    #[test]
    fn new_registration_repairs_a_failed_fresh_context_seed() {
        let fix = FixtureRoot::new("proj-new-template-retry");
        let mut s = AppSettings::default();

        let first_result = prepare_new_project_impl(
            fix.path().to_str().unwrap(),
            None,
            |ac_root, _on_publication| {
                std::fs::write(
                    ac_root
                        .join(crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME),
                    crate::config::session_context::get_default_agent_template(),
                )
                .expect("write partial agent context");
                Err("injected seed failure".to_string())
            },
        );

        assert!(matches!(
            first_result,
            Err(ProjectError::ContextTemplatesCreateFailed(_, _))
        ));
        assert!(
            fix.path().join(".ac").exists(),
            "fresh partial .ac directory must remain truthful after seed failure"
        );
        assert!(s.project_paths.is_empty());

        let retry = register_new_project(&mut s, fix.path().to_str().unwrap())
            .expect("retry register new project");

        assert!(!retry.created);
        assert!(retry.registered);
        assert!(fix
            .path()
            .join(".ac")
            .join(crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME)
            .is_file());
        assert!(fix
            .path()
            .join(".ac")
            .join(crate::config::session_context::COORDINATOR_CONTEXT_TEMPLATE_FILENAME)
            .is_file());
    }

    #[test]
    fn new_project_contender_can_complete_on_the_same_visible_root_before_creator_gate() {
        let fix = FixtureRoot::new("proj-new-contender");
        let path = fix.path().to_string_lossy().to_string();
        let creator_path = path.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let creator = std::thread::spawn(move || {
            let prepared = prepare_new_project_impl_with_hook(
                &creator_path,
                None,
                |ac_root, _on_publication| {
                    crate::config::session_context::create_default_context_templates(ac_root)
                },
                move |_, created| {
                    assert!(created, "creator must own the create_dir win");
                    ready_tx.send(()).expect("signal visible root");
                    release_rx
                        .recv_timeout(std::time::Duration::from_secs(5))
                        .expect("release creator gate acquisition");
                },
            )
            .expect("creator preparation");
            let created = prepared.created;
            prepared.release();
            created
        });

        ready_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("creator published .ac");
        let mut contender_settings = AppSettings::default();
        let contender = register_new_project(&mut contender_settings, &path)
            .expect("contender completes shared-root setup");
        assert!(!contender.created);
        assert!(contender.registered);
        assert!(fix
            .path()
            .join(".ac")
            .join(crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME)
            .is_file());
        assert!(fix
            .path()
            .join(".ac")
            .join(crate::config::session_context::COORDINATOR_CONTEXT_TEMPLATE_FILENAME)
            .is_file());

        release_tx.send(()).expect("release creator");
        assert!(creator.join().expect("join creator"));
        assert!(fix.path().join(".ac").is_dir());
    }

    #[test]
    fn new_project_creator_failure_after_gate_preserves_contender_output_and_root() {
        // Plan acceptance item 14 (lines 654/765): after the contender acquires/
        // releases the same in-root gate and publishes into the unchanged identity,
        // a creator that fails AFTER acquiring its own gate must not delete the root
        // or the contender's output and must register nothing; a later registration
        // then repairs from the truthful partial state. This directly exercises the
        // plan-line-1214 no-deletion fix in the same-identity contender race
        // (creation intent is revalidation data, never recursive-deletion authority).
        let fix = FixtureRoot::new("proj-new-contender-creator-fails");
        let path = fix.path().to_string_lossy().to_string();
        let creator_path = path.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let creator = std::thread::spawn(move || {
            // The template closure runs post-acquisition (projects.rs create path),
            // so returning Err here is a deterministic "creator fails after its own
            // gate acquisition". It writes nothing, so the contender's published
            // templates must be left untouched. Reduce to a Send-safe result.
            match prepare_new_project_impl_with_hook(
                &creator_path,
                None,
                |_ac_root, _on_publication| {
                    Err("injected creator failure after gate".to_string())
                },
                move |_, created| {
                    assert!(created, "creator must own the create_dir win");
                    ready_tx.send(()).expect("signal visible root");
                    release_rx
                        .recv_timeout(std::time::Duration::from_secs(5))
                        .expect("release creator gate acquisition");
                },
            ) {
                Ok(prepared) => {
                    prepared.release();
                    Ok(())
                }
                Err(error) => Err(error),
            }
        });

        // Contender uses the same unchanged root while the creator is paused before
        // its own acquisition; it acquires/releases the in-root gate and publishes
        // both templates.
        ready_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("creator published .ac");
        let mut contender_settings = AppSettings::default();
        let contender = register_new_project(&mut contender_settings, &path)
            .expect("contender completes shared-root setup");
        assert!(!contender.created);
        assert!(contender.registered);
        let agent_template = fix
            .path()
            .join(".ac")
            .join(crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME);
        let coordinator_template = fix
            .path()
            .join(".ac")
            .join(crate::config::session_context::COORDINATOR_CONTEXT_TEMPLATE_FILENAME);
        assert!(agent_template.is_file());
        assert!(coordinator_template.is_file());
        let agent_bytes = std::fs::read(&agent_template).expect("read contender agent template");
        let coordinator_bytes =
            std::fs::read(&coordinator_template).expect("read contender coordinator template");

        // Release the creator; it acquires the now-free gate and then fails.
        release_tx.send(()).expect("release creator");
        let creator_result = creator.join().expect("join creator");
        assert!(
            matches!(
                creator_result,
                Err(ProjectError::ContextTemplatesCreateFailed(_, _))
            ),
            "creator must fail after gate acquisition, got {creator_result:?}"
        );

        // Neither the root nor the contender's output was deleted by the failed creator,
        // and the creator registered no incomplete setup.
        assert!(
            fix.path().join(".ac").is_dir(),
            "root must survive creator failure"
        );
        assert!(
            agent_template.is_file(),
            "contender agent template must survive creator failure"
        );
        assert!(
            coordinator_template.is_file(),
            "contender coordinator template must survive creator failure"
        );
        assert_eq!(
            std::fs::read(&agent_template).expect("re-read agent template"),
            agent_bytes,
            "contender agent template bytes must be untouched"
        );
        assert_eq!(
            std::fs::read(&coordinator_template).expect("re-read coordinator template"),
            coordinator_bytes,
            "contender coordinator template bytes must be untouched"
        );

        // A later registration re-runs template ensure under the gate and completes
        // from the truthful partial state (the failed creator registered nothing).
        let mut repair_settings = AppSettings::default();
        let repair = register_new_project(&mut repair_settings, &path)
            .expect("later registration repairs from truthful partial state");
        assert!(!repair.created);
        assert!(repair.registered);
    }

    #[test]
    fn new_project_rejects_a_replaced_root_before_gate_without_touching_replacement() {
        let fix = FixtureRoot::new("proj-new-replaced-root");
        let replacement_marker = fix.path().join(".ac").join("foreign.txt");

        let result = prepare_new_project_impl_with_hook(
            fix.path().to_str().unwrap(),
            None,
            |ac_root, _on_publication| {
                crate::config::session_context::create_default_context_templates(ac_root)
            },
            |ac_root, created| {
                assert!(created);
                let detached = ac_root.with_extension("ac-detached");
                std::fs::rename(ac_root, &detached).expect("detach created root");
                std::fs::create_dir(ac_root).expect("install replacement root");
                std::fs::write(ac_root.join("foreign.txt"), b"FOREIGN")
                    .expect("write replacement marker");
            },
        );

        assert!(matches!(
            result,
            Err(ProjectError::ProjectSetupChanged(_, _))
        ));
        assert_eq!(std::fs::read(&replacement_marker).unwrap(), b"FOREIGN");
        assert!(!fix
            .path()
            .join(".ac")
            .join(crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME)
            .exists());
        assert!(!fix.path().join(".ac").join(".seed-manifest.lock").exists());
    }

    #[test]
    fn synchronous_new_project_refreshes_and_saves_disk_authoritative_settings() {
        let fix = FixtureRoot::new("proj-new-sync-settings");
        let settings_path = fix.path().join("settings.json");
        let disk_only = fix.path().join("disk-only").to_string_lossy().to_string();
        let mut disk_settings = AppSettings {
            project_paths: vec![disk_only.clone()],
            project_path: Some(disk_only.clone()),
            ..AppSettings::default()
        };
        crate::config::settings::save_settings_with_project_paths_to_path(
            &disk_settings,
            &settings_path,
        )
        .expect("seed disk settings");
        disk_settings.project_paths = vec!["stale-memory".to_string()];
        disk_settings.project_path = Some("stale-memory".to_string());
        let project = fix.path().join("new-project");

        let result = register_new_project_with_settings_path(
            &mut disk_settings,
            project.to_str().unwrap(),
            &settings_path,
            None,
        )
        .expect("gated synchronous registration");

        assert!(result.registered);
        assert_eq!(
            disk_settings.project_paths,
            vec![disk_only.clone(), result.path.clone()]
        );
        let saved: AppSettings =
            serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
        assert_eq!(saved.project_paths, vec![disk_only, result.path]);
    }

    #[test]
    fn new_backfills_templates_when_ac_already_exists() {
        let fix = FixtureRoot::new("proj-new-template-no-backfill");
        std::fs::create_dir_all(fix.path().join(".ac")).unwrap();
        let mut s = AppSettings::default();

        let r = register_new_project(&mut s, fix.path().to_str().unwrap()).unwrap();

        assert!(!r.created);
        assert!(fix
            .path()
            .join(".ac")
            .join("Context.AgentsCommander.md")
            .exists());
        assert!(fix
            .path()
            .join(".ac")
            .join("Context.coordinator.md")
            .exists());
        assert!(!fix.path().join(".ac").join("templates").exists());
    }

    #[test]
    fn new_skips_creation_when_ac_already_exists_via_ac_root_lookup() {
        let fix = FixtureRoot::new("proj-new-existing-lookup");
        std::fs::create_dir_all(fix.path().join(".ac")).unwrap();
        let mut s = AppSettings::default();
        let r = register_new_project(&mut s, fix.path().to_str().unwrap()).unwrap();
        assert!(!r.created);
        assert!(r.registered);
        assert!(fix.path().join(".ac").is_dir());
    }

    #[test]
    fn new_is_idempotent_for_registration() {
        let fix = FixtureRoot::new("proj-new-idem");
        let mut s = AppSettings::default();
        let _ = register_new_project(&mut s, fix.path().to_str().unwrap()).unwrap();
        let r2 = register_new_project(&mut s, fix.path().to_str().unwrap()).unwrap();
        assert!(!r2.created);
        assert!(!r2.registered);
        assert_eq!(s.project_paths.len(), 1);
    }

    // Stage E (#1064) clone/no-backfill registration sentinel (plan section 10.3
    // item 7, section 12 item 10, acceptance item 28/38): a cloned project
    // arrives with its `.ac` already present; registration never recreates it,
    // and re-discovery from a fresh settings snapshot neither backfills nor
    // duplicates the registration.
    #[test]
    fn stage_e_registering_a_cloned_project_preserves_ac_and_does_not_duplicate() {
        let fix = FixtureRoot::new("proj-stage-e-clone");
        std::fs::create_dir_all(fix.path().join(".ac")).unwrap();
        let mut settings = AppSettings::default();
        let first = register_new_project(&mut settings, fix.path().to_str().unwrap()).unwrap();
        assert!(!first.created, "an existing .ac is never recreated");
        assert!(first.registered);
        assert!(
            fix.path().join(".ac").is_dir(),
            "the cloned .ac is preserved"
        );

        let mut fresh = AppSettings::default();
        let rediscovered = register_new_project(&mut fresh, fix.path().to_str().unwrap()).unwrap();
        assert!(!rediscovered.created);
        assert_eq!(
            fresh.project_paths.len(),
            1,
            "re-discovering a clone must not duplicate the registration"
        );
        assert!(fix.path().join(".ac").is_dir());
    }

    #[test]
    fn new_rejects_when_path_is_a_regular_file() {
        let fix = FixtureRoot::new("proj-new-file");
        let f = fix.path().join("file.txt");
        std::fs::write(&f, b"x").unwrap();
        let mut s = AppSettings::default();
        let r = register_new_project(&mut s, f.to_str().unwrap());
        assert!(matches!(r, Err(ProjectError::NotADirectory(_))));
        assert!(s.project_paths.is_empty());
    }

    // ── upsert keeps legacy projectPath in lockstep ───────────────────────

    #[test]
    fn upsert_syncs_legacy_project_path_field() {
        let fix1 = FixtureRoot::new("proj-legacy-1");
        let fix2 = FixtureRoot::new("proj-legacy-2");
        std::fs::create_dir_all(fix1.path().join(".ac")).unwrap();
        std::fs::create_dir_all(fix2.path().join(".ac")).unwrap();
        let mut s = AppSettings::default();
        let r1 = register_existing_project(&mut s, fix1.path().to_str().unwrap()).unwrap();
        assert_eq!(s.project_path.as_deref(), Some(r1.path.as_str()));
        let r2 = register_existing_project(&mut s, fix2.path().to_str().unwrap()).unwrap();
        // project_path tracks the HEAD of the list (same as FE persistProjectPaths).
        assert_eq!(s.project_path.as_deref(), Some(r1.path.as_str()));
        assert_eq!(s.project_paths, vec![r1.path.clone(), r2.path.clone()]);
    }

    // ── #778 remove_project_path (inverse of upsert) ──────────────────────

    #[test]
    fn remove_project_path_removes_matching_entry_and_rederives_head() {
        let mut s = AppSettings {
            project_paths: vec!["C:/a".to_string(), "C:/b".to_string()],
            project_path: Some("C:/a".to_string()),
            ..AppSettings::default()
        };
        assert!(remove_project_path(&mut s, "C:/a"));
        assert_eq!(s.project_paths, vec!["C:/b".to_string()]);
        assert_eq!(s.project_path.as_deref(), Some("C:/b"));
    }

    #[test]
    fn remove_project_path_last_entry_clears_head_to_none() {
        let mut s = AppSettings {
            project_paths: vec!["C:/only".to_string()],
            project_path: Some("C:/only".to_string()),
            ..AppSettings::default()
        };
        assert!(remove_project_path(&mut s, "C:/only"));
        assert!(s.project_paths.is_empty());
        assert_eq!(s.project_path, None);
    }

    #[test]
    fn remove_project_path_is_case_slash_insensitive_and_preserves_byte_form() {
        // Stored byte-form uses backslashes + mixed case; remove via a normalized variant.
        let mut s = AppSettings {
            project_paths: vec![r"C:\Foo\".to_string(), "C:/keep".to_string()],
            project_path: Some(r"C:\Foo\".to_string()),
            ..AppSettings::default()
        };
        assert!(remove_project_path(&mut s, "c:/foo"));
        // Only C:\Foo\ removed; the surviving entry keeps its original byte-form.
        assert_eq!(s.project_paths, vec!["C:/keep".to_string()]);
        assert_eq!(s.project_path.as_deref(), Some("C:/keep"));
    }

    #[test]
    fn remove_project_path_no_match_returns_false_and_keeps_list() {
        let mut s = AppSettings {
            project_paths: vec!["C:/a".to_string()],
            project_path: Some("C:/a".to_string()),
            ..AppSettings::default()
        };
        assert!(!remove_project_path(&mut s, "C:/nope"));
        assert_eq!(s.project_paths, vec!["C:/a".to_string()]);
        assert_eq!(s.project_path.as_deref(), Some("C:/a"));
    }

    #[test]
    fn remove_project_path_also_drops_archived_entry() {
        let mut s = AppSettings {
            project_paths: vec!["C:/a".to_string()],
            project_path: Some("C:/a".to_string()),
            archived_project_paths: vec!["c:/a/".to_string(), "C:/b".to_string()],
            ..AppSettings::default()
        };

        assert!(remove_project_path(&mut s, "C:/a"));

        assert_eq!(s.archived_project_paths, vec!["C:/b".to_string()]);
    }

    // ── #881 archive_project_path ───────────────────────────────────────

    #[test]
    fn archive_project_path_moves_entry_and_rederives_head() {
        let mut s = AppSettings {
            project_paths: vec!["C:/a".to_string(), "C:/b".to_string()],
            project_path: Some("C:/a".to_string()),
            ..AppSettings::default()
        };

        assert!(archive_project_path(&mut s, "C:/a"));

        assert_eq!(s.project_paths, vec!["C:/b".to_string()]);
        assert_eq!(s.archived_project_paths, vec!["C:/a".to_string()]);
        assert_eq!(s.project_path.as_deref(), Some("C:/b"));
    }

    #[test]
    fn archive_project_path_last_entry_clears_head_to_none() {
        let mut s = AppSettings {
            project_paths: vec!["C:/only".to_string()],
            project_path: Some("C:/only".to_string()),
            ..AppSettings::default()
        };

        assert!(archive_project_path(&mut s, "C:/only"));

        assert!(s.project_paths.is_empty());
        assert_eq!(s.archived_project_paths, vec!["C:/only".to_string()]);
        assert_eq!(s.project_path, None);
    }

    #[test]
    fn archive_project_path_is_case_slash_insensitive_and_archives_stored_byte_form() {
        let mut s = AppSettings {
            project_paths: vec![r"C:\Foo\".to_string(), "C:/keep".to_string()],
            project_path: Some(r"C:\Foo\".to_string()),
            ..AppSettings::default()
        };

        assert!(archive_project_path(&mut s, "c:/foo"));

        assert_eq!(s.project_paths, vec!["C:/keep".to_string()]);
        assert_eq!(s.archived_project_paths, vec![r"C:\Foo\".to_string()]);
    }

    #[test]
    fn archive_project_path_is_idempotent() {
        let mut s = AppSettings {
            project_paths: vec!["C:/a".to_string()],
            project_path: Some("C:/a".to_string()),
            ..AppSettings::default()
        };

        assert!(archive_project_path(&mut s, "C:/a"));
        assert!(!archive_project_path(&mut s, "c:/a/"));

        assert_eq!(s.archived_project_paths, vec!["C:/a".to_string()]);
    }

    #[test]
    fn archive_project_path_records_unregistered_path() {
        let mut s = AppSettings::default();

        assert!(!archive_project_path(&mut s, "C:/missing"));

        assert_eq!(s.archived_project_paths, vec!["C:/missing".to_string()]);
        assert!(s.project_paths.is_empty());
    }

    #[test]
    fn unarchive_project_path_restores_archived_byte_form_without_io() {
        let mut s = AppSettings {
            archived_project_paths: vec!["C:/MissingProject/".to_string()],
            ..AppSettings::default()
        };

        assert!(unarchive_project_path(&mut s, "C:/MissingProject"));

        assert_eq!(s.project_paths, vec!["C:/MissingProject/".to_string()]);
        assert!(s.archived_project_paths.is_empty());
        assert_eq!(s.project_path.as_deref(), Some("C:/MissingProject/"));
    }

    #[test]
    fn upsert_project_path_unarchives_matching_entry() {
        let fix = FixtureRoot::new("proj-unarchive-upsert");
        std::fs::create_dir_all(fix.path().join(".ac")).expect("create .ac");
        let mut s = AppSettings {
            archived_project_paths: vec![fix.path().to_string_lossy().replace('\\', "/") + "/"],
            ..AppSettings::default()
        };

        let result = register_existing_project(&mut s, fix.path().to_str().unwrap())
            .expect("register existing");

        assert!(result.registered);
        assert!(s.archived_project_paths.is_empty());
        assert_eq!(s.project_paths, vec![result.path]);
    }

    #[cfg(windows)]
    #[test]
    fn upsert_project_path_unarchives_windows_verbatim_variant() {
        let mut s = AppSettings {
            archived_project_paths: vec![r"\\?\C:\Users\maria\MixedCaseProject".to_string()],
            ..AppSettings::default()
        };

        assert!(upsert_project_path(
            &mut s,
            r"C:\Users\maria\MixedCaseProject"
        ));

        assert_eq!(
            s.project_paths,
            vec![r"C:\Users\maria\MixedCaseProject".to_string()]
        );
        assert!(
            s.archived_project_paths.is_empty(),
            "verbatim and ordinary Windows paths must not remain in separate lists"
        );
    }

    #[test]
    fn archived_projects_reports_missing_folder_and_missing_ac_root() {
        let full = FixtureRoot::new("proj-archived-full");
        std::fs::create_dir_all(full.path().join(".ac")).expect("full .ac");
        let no_ac_root = FixtureRoot::new("proj-archived-no-workspace");
        let deleted = FixtureRoot::new("proj-archived-deleted");
        let deleted_path = deleted.path().to_string_lossy().to_string();
        std::fs::remove_dir_all(deleted.path()).expect("delete fixture");

        let rows = archived_projects_from_paths(&[
            full.path().to_string_lossy().to_string(),
            no_ac_root.path().to_string_lossy().to_string(),
            deleted_path,
        ]);

        assert!(rows[0].exists);
        assert!(rows[0].has_workspace);
        assert!(rows[1].exists);
        assert!(!rows[1].has_workspace);
        assert!(!rows[2].exists);
        assert!(!rows[2].has_workspace);
    }

    // ── absolutise: relative + dot-dot collapse (Round-1 G4 + G13) ────────

    /// CWD is process-wide; restore on Drop. Any other test that mutates
    /// CWD in this same module would race — keep this confined.
    struct CwdGuard(PathBuf);
    impl Drop for CwdGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.0);
        }
    }

    static CWD_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

    fn cwd_lock() -> std::sync::MutexGuard<'static, ()> {
        CWD_MUTEX
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn absolutise_resolves_relative_path_against_cwd() {
        let _cwd_lock = cwd_lock();
        let fix = FixtureRoot::new("proj-rel");
        std::fs::create_dir_all(fix.path().join(".ac")).unwrap();
        let prev = std::env::current_dir().unwrap();
        let _guard = CwdGuard(prev);
        std::env::set_current_dir(fix.path()).unwrap();
        let mut s = AppSettings::default();
        let r = register_existing_project(&mut s, ".").unwrap();
        // Persisted path must be absolute and equal to fix.path() lexically
        // (after `std::path::absolute(".")` collapses the trailing `.`).
        assert!(Path::new(&r.path).is_absolute(), "not absolute: {}", r.path);
        let normalized_persisted = r.path.replace('\\', "/").to_lowercase();
        let normalized_fixture = fix
            .path()
            .to_string_lossy()
            .replace('\\', "/")
            .to_lowercase();
        assert_eq!(normalized_persisted, normalized_fixture);
    }

    /// `..` collapse is Windows-only behaviour of `std::path::absolute`
    /// (POSIX preserves `..` for symlink-safety). On Windows the persisted
    /// path must contain no `..` component.
    #[cfg(windows)]
    #[test]
    fn absolutise_collapses_dotdot_segments_on_windows() {
        let _cwd_lock = cwd_lock();
        let fix = FixtureRoot::new("proj-dotdot");
        let project = fix.path().join("project");
        std::fs::create_dir_all(project.join(".ac")).unwrap();
        let sibling = fix.path().join("sibling");
        std::fs::create_dir_all(&sibling).unwrap();
        let prev = std::env::current_dir().unwrap();
        let _guard = CwdGuard(prev);
        let sibling = std::fs::canonicalize(&sibling).unwrap();
        std::env::set_current_dir(&sibling).unwrap();
        let mut s = AppSettings::default();
        let r = register_existing_project(&mut s, "..\\project").unwrap();
        assert!(
            !r.path.contains(".."),
            "persisted path should not contain `..` on Windows: {}",
            r.path
        );
    }

    // ── trailing-separator dedup (Round-1 IR.3.2) ────────────────────────

    #[test]
    fn upsert_dedup_strips_trailing_separator() {
        let fix = FixtureRoot::new("proj-trailing");
        std::fs::create_dir_all(fix.path().join(".ac")).unwrap();
        let mut s = AppSettings::default();
        let original = fix.path().to_string_lossy().to_string();
        let _ = register_existing_project(&mut s, &original).unwrap();
        // Add `\` (or `/` on POSIX) to simulate shell tab-completion.
        let with_trailing = if cfg!(windows) {
            format!("{}\\", original)
        } else {
            format!("{}/", original)
        };
        let r2 = register_existing_project(&mut s, &with_trailing).unwrap();
        assert!(
            !r2.registered,
            "trailing-separator variant should dedup: {} vs {}",
            original, with_trailing
        );
        assert_eq!(s.project_paths.len(), 1);
    }

    // ── resolve_project_reference ───────────────────────────────────────

    fn create_ac_project(path: &Path) {
        std::fs::create_dir_all(path.join(".ac")).unwrap();
    }

    #[test]
    fn resolve_project_reference_matches_registered_folder_name() {
        let fix = FixtureRoot::new("proj-resolve-name");
        let project = fix.path().join("ProjectAlpha");
        create_ac_project(&project);
        let project_paths = vec![project.to_string_lossy().to_string()];

        let resolved = resolve_project_reference(&project_paths, "ProjectAlpha").unwrap();

        assert_eq!(resolved.path, project);
        assert_eq!(resolved.folder_name, "ProjectAlpha");
        assert!(resolved.registered);
    }

    #[test]
    fn resolve_project_reference_matches_child_project_from_registered_parent() {
        let fix = FixtureRoot::new("proj-resolve-parent");
        let project = fix.path().join("ProjectAlpha");
        create_ac_project(&project);
        let project_paths = vec![fix.path().to_string_lossy().to_string()];

        let resolved = resolve_project_reference(&project_paths, "ProjectAlpha").unwrap();

        assert_eq!(resolved.path, project);
    }

    #[test]
    fn resolve_project_reference_rejects_non_ac_child_project() {
        let fix = FixtureRoot::new("proj-resolve-invalid-workspace");
        let project = fix.path().join("ProjectAlpha");
        std::fs::create_dir_all(project.join(".workspace")).unwrap();
        let project_paths = vec![fix.path().to_string_lossy().to_string()];

        let err = resolve_project_reference(&project_paths, "ProjectAlpha").unwrap_err();

        assert!(matches!(err, ProjectResolveError::NotFound(_)));
    }

    #[test]
    fn resolve_project_reference_rejects_ambiguous_folder_name() {
        let fix = FixtureRoot::new("proj-resolve-ambiguous");
        let parent_a = fix.path().join("a");
        let parent_b = fix.path().join("b");
        let project_a = parent_a.join("ProjectAlpha");
        let project_b = parent_b.join("ProjectAlpha");
        create_ac_project(&project_a);
        create_ac_project(&project_b);
        let project_paths = vec![
            parent_a.to_string_lossy().to_string(),
            parent_b.to_string_lossy().to_string(),
        ];

        let err = resolve_project_reference(&project_paths, "ProjectAlpha").unwrap_err();

        match err {
            ProjectResolveError::Ambiguous { candidates, .. } => {
                assert_eq!(candidates.len(), 2);
                assert!(candidates.iter().any(|p| p.contains("ProjectAlpha")));
            }
            other => panic!("expected ambiguous error, got {other:?}"),
        }
    }

    #[test]
    fn resolve_project_reference_rejects_unregistered_direct_path() {
        let fix = FixtureRoot::new("proj-resolve-unregistered");
        let project = fix.path().join("ProjectAlpha");
        create_ac_project(&project);

        let err = resolve_project_reference(&[], &project.to_string_lossy()).unwrap_err();

        assert!(matches!(err, ProjectResolveError::NotFound(_)));
    }

    #[test]
    fn resolve_project_reference_rejects_registered_direct_path_input() {
        let fix = FixtureRoot::new("proj-resolve-direct");
        let project = fix.path().join("ProjectAlpha");
        create_ac_project(&project);
        let project_paths = vec![project.to_string_lossy().to_string()];

        let err =
            resolve_project_reference(&project_paths, &project.to_string_lossy()).unwrap_err();

        assert!(matches!(err, ProjectResolveError::NotFound(_)));
    }

    #[test]
    fn resolve_project_reference_skips_dot_prefixed_child_projects() {
        let fix = FixtureRoot::new("proj-resolve-dot-child");
        let hidden = fix.path().join(".HiddenProject");
        create_ac_project(&hidden);
        let project_paths = vec![fix.path().to_string_lossy().to_string()];

        let err = resolve_project_reference(&project_paths, ".HiddenProject").unwrap_err();

        assert!(matches!(err, ProjectResolveError::NotFound(_)));
    }

    #[test]
    fn enumerate_registered_project_candidates_yields_a_nested_child_of_a_registered_parent() {
        let fix = FixtureRoot::new("proj-enumerate-nested");
        create_ac_project(fix.path());
        let child = fix.path().join("child");
        create_ac_project(&child);
        let project_paths = vec![fix.path().to_string_lossy().to_string()];

        let candidates = enumerate_registered_project_candidates(&project_paths);

        assert!(
            candidates.iter().any(|c| c.path == child),
            "nested child with .ac must remain a project candidate"
        );
    }

    // ── serde camelCase shape lock (Round-1 G14) ─────────────────────────

    #[test]
    fn project_registration_serializes_camel_case() {
        // Today's fields are already lowercase single-words, so no rename
        // happens. This test locks the invariant: a future field like
        // `ac_root` must serialize to `acRoot`. If the
        // `#[serde(rename_all = "camelCase")]` attribute is ever dropped,
        // this test catches it before the FE silently breaks.
        let r = ProjectRegistration {
            path: "X".to_string(),
            registered: true,
            created: false,
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"path\""), "missing path field: {}", json);
        assert!(
            json.contains("\"registered\""),
            "missing registered field: {}",
            json
        );
        assert!(
            json.contains("\"created\""),
            "missing created field: {}",
            json
        );
        // No snake_case relics from any current field name.
        assert!(
            !json.contains("ac_root"),
            "snake_case field name leaked: {}",
            json
        );

        let archived = ArchivedProject {
            path: "X".to_string(),
            folder_name: "X".to_string(),
            exists: true,
            has_workspace: false,
        };
        let json = serde_json::to_string(&archived).unwrap();
        assert!(
            json.contains("\"folderName\""),
            "missing folderName field: {}",
            json
        );
        assert!(
            json.contains("\"hasWorkspace\""),
            "missing hasWorkspace field: {}",
            json
        );
        assert!(
            !json.contains("folder_name") && !json.contains("has_workspace"),
            "snake_case archived field leaked: {}",
            json
        );
    }

    // ── #1077 instance-relative codec ────────────────────────────────────

    #[cfg(not(windows))]
    mod codec_posix {
        use super::super::{decode_instance_relative, encode_instance_relative, RelDecodeError};
        use std::path::{Path, PathBuf};

        #[test]
        fn encode_child_of_base() {
            assert_eq!(
                encode_instance_relative(
                    Path::new("/opt/bundle/projects/alpha"),
                    Path::new("/opt/bundle")
                ),
                Some("projects/alpha".to_string())
            );
        }

        #[test]
        fn encode_sibling_uses_dotdot() {
            assert_eq!(
                encode_instance_relative(
                    Path::new("/opt/projects/alpha"),
                    Path::new("/opt/bundle")
                ),
                Some("../projects/alpha".to_string())
            );
        }

        #[test]
        fn encode_base_itself_is_dot() {
            assert_eq!(
                encode_instance_relative(Path::new("/opt/bundle"), Path::new("/opt/bundle")),
                Some(".".to_string())
            );
        }

        #[test]
        fn encode_preserves_case_and_unicode_and_spaces() {
            assert_eq!(
                encode_instance_relative(
                    Path::new("/opt/bundle/Pro Jects/Ä lpha"),
                    Path::new("/opt/bundle")
                ),
                Some("Pro Jects/Ä lpha".to_string())
            );
        }

        #[test]
        fn encode_component_with_backslash_has_no_portable_form() {
            // A POSIX dir literally named `a\b` cannot be represented.
            assert_eq!(
                encode_instance_relative(Path::new("/opt/bundle/a\\b"), Path::new("/opt/bundle")),
                None
            );
        }

        #[test]
        fn decode_round_trips_encode() {
            let base = Path::new("/opt/bundle");
            let project = Path::new("/opt/projects/alpha");
            let wire = encode_instance_relative(project, base).unwrap();
            assert_eq!(
                decode_instance_relative(&wire, base).unwrap(),
                PathBuf::from("/opt/projects/alpha")
            );
        }

        #[test]
        fn decode_dot_is_base() {
            assert_eq!(
                decode_instance_relative(".", Path::new("/opt/bundle")).unwrap(),
                PathBuf::from("/opt/bundle")
            );
        }

        #[test]
        fn decode_rejects_noncanonical_and_injection() {
            let base = Path::new("/opt/bundle");
            assert_eq!(
                decode_instance_relative("", base),
                Err(RelDecodeError::Empty)
            );
            assert_eq!(
                decode_instance_relative("a\0b", base),
                Err(RelDecodeError::Nul)
            );
            assert_eq!(
                decode_instance_relative("a\\b", base),
                Err(RelDecodeError::Backslash)
            );
            assert_eq!(
                decode_instance_relative("/abs", base),
                Err(RelDecodeError::EmptySegment)
            );
            assert_eq!(
                decode_instance_relative("a/", base),
                Err(RelDecodeError::EmptySegment)
            );
            assert_eq!(
                decode_instance_relative("a//b", base),
                Err(RelDecodeError::EmptySegment)
            );
            assert_eq!(
                decode_instance_relative("a/./b", base),
                Err(RelDecodeError::DotSegment)
            );
            assert_eq!(
                decode_instance_relative("C:/x", base),
                Err(RelDecodeError::DriveRelative)
            );
            assert_eq!(
                decode_instance_relative("C:", base),
                Err(RelDecodeError::DriveRelative)
            );
        }

        #[test]
        fn decode_rejects_traversal_above_root() {
            // /opt/bundle has two normal components; three `..` walk above root.
            assert_eq!(
                decode_instance_relative("../../../etc", Path::new("/opt/bundle")),
                Err(RelDecodeError::EscapesRoot)
            );
        }

        #[test]
        fn decode_allows_sibling_traversal() {
            assert_eq!(
                decode_instance_relative("../projects/alpha", Path::new("/opt/bundle")).unwrap(),
                PathBuf::from("/opt/projects/alpha")
            );
        }

        #[test]
        fn decode_rejects_relative_base() {
            assert_eq!(
                decode_instance_relative("a", Path::new("relative/base")),
                Err(RelDecodeError::BaseNotAbsolute)
            );
        }

        #[test]
        fn posix_backslash_component_is_not_a_separator_but_is_rejected() {
            // Wire never contains `\`; a value with one is rejected outright.
            assert_eq!(
                decode_instance_relative("a\\b/c", Path::new("/opt/bundle")),
                Err(RelDecodeError::Backslash)
            );
        }

        #[test]
        fn posix_paths_are_case_distinct() {
            // Two POSIX projects differing only by case must produce distinct
            // wires (encode preserves case) and distinct decoded targets.
            let base = Path::new("/opt");
            let upper = encode_instance_relative(Path::new("/opt/Repo"), base).unwrap();
            let lower = encode_instance_relative(Path::new("/opt/repo"), base).unwrap();
            assert_eq!(upper, "Repo");
            assert_eq!(lower, "repo");
            assert_ne!(upper, lower);
            assert_ne!(
                decode_instance_relative(&upper, base).unwrap(),
                decode_instance_relative(&lower, base).unwrap()
            );
        }

        #[test]
        fn unicode_and_dot_round_trip() {
            let base = Path::new("/opt/bundle");
            // `.` names the base itself.
            assert_eq!(
                decode_instance_relative(".", base).unwrap(),
                PathBuf::from("/opt/bundle")
            );
            // Non-ASCII components survive an encode/decode round-trip.
            let project = Path::new("/opt/bundle/проект/Ω-α");
            let wire = encode_instance_relative(project, base).unwrap();
            assert_eq!(wire, "проект/Ω-α");
            assert_eq!(decode_instance_relative(&wire, base).unwrap(), project);
        }
    }

    #[cfg(windows)]
    mod codec_windows {
        use super::super::{decode_instance_relative, encode_instance_relative, RelDecodeError};
        use std::path::{Path, PathBuf};

        #[test]
        fn encode_child_and_sibling() {
            assert_eq!(
                encode_instance_relative(
                    Path::new(r"C:\bundle\projects\alpha"),
                    Path::new(r"C:\bundle")
                ),
                Some("projects/alpha".to_string())
            );
            assert_eq!(
                encode_instance_relative(Path::new(r"C:\projects\alpha"), Path::new(r"C:\bundle")),
                Some("../projects/alpha".to_string())
            );
        }

        #[test]
        fn encode_drive_letter_case_insensitive_root() {
            assert_eq!(
                encode_instance_relative(Path::new(r"c:\bundle\alpha"), Path::new(r"C:\bundle")),
                Some("alpha".to_string())
            );
        }

        #[test]
        fn encode_different_drive_has_no_relative_form() {
            assert_eq!(
                encode_instance_relative(Path::new(r"D:\projects\alpha"), Path::new(r"C:\bundle")),
                None
            );
        }

        #[test]
        fn encode_same_unc_share_ok_different_share_none() {
            assert_eq!(
                encode_instance_relative(
                    Path::new(r"\\server\share\bundle\alpha"),
                    Path::new(r"\\server\share\bundle")
                ),
                Some("alpha".to_string())
            );
            assert_eq!(
                encode_instance_relative(
                    Path::new(r"\\server\other\alpha"),
                    Path::new(r"\\server\share\bundle")
                ),
                None
            );
        }

        #[test]
        fn encode_verbatim_disk_normalizes_for_comparison() {
            assert_eq!(
                encode_instance_relative(
                    Path::new(r"\\?\C:\bundle\alpha"),
                    Path::new(r"C:\bundle")
                ),
                Some("alpha".to_string())
            );
        }

        #[test]
        fn decode_round_trips_and_rejects_windows_hazards() {
            let base = Path::new(r"C:\bundle");
            assert_eq!(
                decode_instance_relative("projects/alpha", base).unwrap(),
                PathBuf::from(r"C:\bundle\projects\alpha")
            );
            // ADS colon not in drive position → illegal char (drive-position
            // `X:` is caught earlier as DriveRelative, also a rejection).
            assert_eq!(
                decode_instance_relative("foo:bar", base),
                Err(RelDecodeError::IllegalWindowsChar)
            );
            assert_eq!(
                decode_instance_relative("a*b", base),
                Err(RelDecodeError::IllegalWindowsChar)
            );
            assert_eq!(
                decode_instance_relative("a:b", base),
                Err(RelDecodeError::DriveRelative)
            );
            // Trailing dot/space.
            assert_eq!(
                decode_instance_relative("alpha ", base),
                Err(RelDecodeError::TrailingDotOrSpace)
            );
            assert_eq!(
                decode_instance_relative("alpha.", base),
                Err(RelDecodeError::TrailingDotOrSpace)
            );
            // Reserved DOS device names, with and without extension.
            assert_eq!(
                decode_instance_relative("CON", base),
                Err(RelDecodeError::ReservedDosName)
            );
            assert_eq!(
                decode_instance_relative("con.txt", base),
                Err(RelDecodeError::ReservedDosName)
            );
            assert_eq!(
                decode_instance_relative("COM1", base),
                Err(RelDecodeError::ReservedDosName)
            );
            assert_eq!(
                decode_instance_relative("LPT9", base),
                Err(RelDecodeError::ReservedDosName)
            );
            // COM0 / COM10 are not reserved.
            assert!(decode_instance_relative("COM0", base).is_ok());
            assert!(decode_instance_relative("COM10", base).is_ok());
        }

        #[test]
        fn decode_rejects_traversal_above_drive_root() {
            assert_eq!(
                decode_instance_relative("../../x", Path::new(r"C:\bundle")),
                Err(RelDecodeError::EscapesRoot)
            );
        }

        #[test]
        fn decode_rejects_all_illegal_windows_chars_and_controls() {
            let base = Path::new(r"C:\bundle");
            for illegal in ["a<b", "a>b", "a\"b", "a|b", "a?b", "a*b"] {
                assert_eq!(
                    decode_instance_relative(illegal, base),
                    Err(RelDecodeError::IllegalWindowsChar),
                    "must reject {illegal:?}"
                );
            }
            // Control characters (below 0x20) are rejected.
            assert_eq!(
                decode_instance_relative("a\u{0001}b", base),
                Err(RelDecodeError::IllegalWindowsChar)
            );
            assert_eq!(
                decode_instance_relative("a\u{001f}b", base),
                Err(RelDecodeError::IllegalWindowsChar)
            );
        }

        #[test]
        fn encode_device_namespace_has_no_portable_form() {
            // A `\\.\` device-namespace path is unsupported and must NOT be
            // stripped into an ordinary path; it yields no relative companion.
            assert_eq!(
                encode_instance_relative(Path::new(r"\\.\COM1"), Path::new(r"C:\bundle")),
                None
            );
            // A verbatim non-disk device path is likewise unsupported.
            assert_eq!(
                encode_instance_relative(
                    Path::new(r"\\?\GLOBALROOT\Device\HarddiskVolume1\x"),
                    Path::new(r"C:\bundle")
                ),
                None
            );
        }
    }

    // ── #1077 resolution matrix / quarantine / dedupe (injected resolver) ──
    mod resolution {
        use super::super::*;
        use std::collections::HashMap;
        use std::path::Path;

        /// Injected resolver mapping syntactic path strings to canned outcomes.
        struct MapResolver {
            map: HashMap<String, SideOutcome>,
        }

        impl CandidateResolver for MapResolver {
            fn resolve(&self, syntactic: &Path) -> SideOutcome {
                self.map
                    .get(&syntactic.to_string_lossy().to_string())
                    .cloned()
                    .unwrap_or_else(|| {
                        SideOutcome::failed(
                            SideStatus::Missing,
                            syntactic.to_str().map(str::to_string),
                        )
                    })
            }
        }

        fn valid(canonical: &str, kind: SideStatus, vol: u64, file: u128) -> SideOutcome {
            SideOutcome {
                status: kind,
                syntactic_path: Some(canonical.to_string()),
                canonical_path: Some(canonical.to_string()),
                identity: Some(DirectoryIdentity { volume: vol, file }),
            }
        }

        fn active_pair(idx: usize, abs: &str, rel: Option<&str>) -> RawPair {
            RawPair {
                source: ProjectSource::ProjectPaths,
                index: Some(idx),
                absolute: RawStringField::string(abs),
                relative: match rel {
                    Some(r) => RawStringField::string(r),
                    None => RawStringField::absent(),
                },
            }
        }

        // Base has a single normal component so a plain relative wire resolves
        // directly under `/opt` (matching the `abs()` helper below).
        fn base() -> &'static Path {
            Path::new(if cfg!(windows) { r"C:\opt" } else { "/opt" })
        }

        fn abs(p: &str) -> String {
            if cfg!(windows) {
                format!(r"C:\opt\{}", p.replace('/', "\\"))
            } else {
                format!("/opt/{}", p)
            }
        }

        #[test]
        fn absolute_only_selects_and_wants_companion() {
            let a = abs("projects/alpha");
            let mut map = HashMap::new();
            map.insert(a.clone(), valid(&a, SideStatus::ValidDirectProject, 1, 10));
            let resolver = MapResolver { map };
            let report = resolve_registrations(
                &[active_pair(0, &a, None)],
                None,
                &[],
                Some(base()),
                false,
                false,
                &resolver,
            );
            assert_eq!(report.active_selected(), vec![display_canonical(&a)]);
            assert_eq!(report.pairs[0].repair, RepairKind::PopulateCompanion);
            assert_eq!(report.active_registration_count, 1);
            assert!(report.issues().next().is_none());
        }

        #[test]
        fn relative_wins_when_absolute_stale() {
            // Absolute is missing; relative decodes to a valid dir.
            let stale = abs("old/alpha");
            let rel_target = abs("projects/alpha");
            let mut map = HashMap::new();
            map.insert(
                rel_target.clone(),
                valid(&rel_target, SideStatus::ValidDirectProject, 1, 11),
            );
            // stale absolute not in map → Missing.
            let resolver = MapResolver { map };
            let report = resolve_registrations(
                &[active_pair(0, &stale, Some("projects/alpha"))],
                None,
                &[],
                Some(base()),
                true,
                false,
                &resolver,
            );
            assert_eq!(
                report.active_selected(),
                vec![display_canonical(&rel_target)]
            );
            assert_eq!(report.pairs[0].repair, RepairKind::ReplaceStaleAbsolute);
        }

        #[test]
        fn both_valid_different_is_conflict_selecting_neither() {
            let a = abs("projects/alpha");
            let rel_target = abs("projects/beta");
            let mut map = HashMap::new();
            map.insert(a.clone(), valid(&a, SideStatus::ValidDirectProject, 1, 20));
            map.insert(
                rel_target.clone(),
                valid(&rel_target, SideStatus::ValidDirectProject, 1, 21),
            );
            let resolver = MapResolver { map };
            let report = resolve_registrations(
                &[active_pair(0, &a, Some("projects/beta"))],
                None,
                &[],
                Some(base()),
                true,
                false,
                &resolver,
            );
            assert!(report.active_selected().is_empty());
            assert_eq!(report.pairs[0].issue, Some(IssueKind::Conflict));
        }

        #[test]
        fn both_valid_same_identity_selects_absolute() {
            let a = abs("projects/alpha");
            let rel_target = abs("aliased/alpha"); // different canonical, same identity
            let mut map = HashMap::new();
            map.insert(a.clone(), valid(&a, SideStatus::ValidDirectProject, 7, 30));
            map.insert(
                rel_target.clone(),
                valid(&rel_target, SideStatus::ValidDirectProject, 7, 30),
            );
            let resolver = MapResolver { map };
            let report = resolve_registrations(
                &[active_pair(0, &a, Some("aliased/alpha"))],
                None,
                &[],
                Some(base()),
                true,
                false,
                &resolver,
            );
            assert_eq!(report.active_selected(), vec![display_canonical(&a)]);
            assert!(report.pairs[0].issue.is_none());
        }

        #[test]
        fn quarantine_suppresses_duplicate_of_conflict_candidate() {
            // Registration 0 is a conflict between alpha and beta.
            // Registration 1 independently selects alpha → must be quarantined.
            let a = abs("projects/alpha");
            let b = abs("projects/beta");
            let mut map = HashMap::new();
            map.insert(a.clone(), valid(&a, SideStatus::ValidDirectProject, 1, 40));
            map.insert(b.clone(), valid(&b, SideStatus::ValidDirectProject, 1, 41));
            let resolver = MapResolver { map };
            let report = resolve_registrations(
                &[
                    active_pair(0, &a, Some("projects/beta")),
                    active_pair(1, &a, None),
                ],
                None,
                &[],
                Some(base()),
                true,
                false,
                &resolver,
            );
            assert!(
                report.active_selected().is_empty(),
                "alpha reintroduced despite conflict"
            );
            assert_eq!(report.pairs[0].issue, Some(IssueKind::Conflict));
            assert_eq!(report.pairs[1].issue, Some(IssueKind::Invalid));
        }

        #[test]
        fn quarantine_scope_overlap_suppresses_child_registration() {
            // Conflict on a collection root; a child registration overlaps its scope.
            let root = abs("collection");
            let other = abs("elsewhere");
            let child = abs("collection/child");
            let mut map = HashMap::new();
            map.insert(
                root.clone(),
                valid(&root, SideStatus::ValidCollectionRoot, 1, 50),
            );
            map.insert(
                other.clone(),
                valid(&other, SideStatus::ValidDirectProject, 1, 51),
            );
            map.insert(
                child.clone(),
                valid(&child, SideStatus::ValidDirectProject, 1, 52),
            );
            let resolver = MapResolver { map };
            let report = resolve_registrations(
                &[
                    active_pair(0, &root, Some("elsewhere")),
                    active_pair(1, &child, None),
                ],
                None,
                &[],
                Some(base()),
                true,
                false,
                &resolver,
            );
            assert!(report.active_selected().is_empty());
            assert_eq!(report.pairs[1].issue, Some(IssueKind::Invalid));
        }

        #[test]
        fn dedupe_keeps_first_active_and_active_beats_archived() {
            let a = abs("projects/alpha");
            let mut map = HashMap::new();
            map.insert(a.clone(), valid(&a, SideStatus::ValidDirectProject, 1, 60));
            let resolver = MapResolver { map };
            let archived = vec![RawPair {
                source: ProjectSource::ArchivedProjectPaths,
                index: Some(0),
                absolute: RawStringField::string(&a),
                relative: RawStringField::absent(),
            }];
            let report = resolve_registrations(
                &[active_pair(0, &a, None), active_pair(1, &a, None)],
                None,
                &archived,
                Some(base()),
                false,
                false,
                &resolver,
            );
            assert_eq!(report.active_selected(), vec![display_canonical(&a)]);
            assert!(
                report.archived_management_paths().is_empty(),
                "archived dup of active must drop"
            );
        }

        #[test]
        fn identity_unavailable_dual_candidates_fail_closed() {
            // Both sides valid, different canonical, NO identities → Unavailable.
            let a = abs("projects/alpha");
            let rel_target = abs("projects/beta");
            let no_id = |canon: &str| SideOutcome {
                status: SideStatus::ValidDirectProject,
                syntactic_path: Some(canon.to_string()),
                canonical_path: Some(canon.to_string()),
                identity: None,
            };
            let mut map = HashMap::new();
            map.insert(a.clone(), no_id(&a));
            map.insert(rel_target.clone(), no_id(&rel_target));
            let resolver = MapResolver { map };
            let report = resolve_registrations(
                &[active_pair(0, &a, Some("projects/beta"))],
                None,
                &[],
                Some(base()),
                true,
                false,
                &resolver,
            );
            assert!(report.active_selected().is_empty());
            assert_eq!(report.pairs[0].issue, Some(IssueKind::Invalid));
        }

        #[test]
        fn neither_valid_definite_notfound_is_missing() {
            let a = abs("gone");
            let resolver = MapResolver {
                map: HashMap::new(),
            }; // everything Missing
            let report = resolve_registrations(
                &[active_pair(0, &a, None)],
                None,
                &[],
                Some(base()),
                false,
                false,
                &resolver,
            );
            assert!(report.active_selected().is_empty());
            assert_eq!(report.pairs[0].issue, Some(IssueKind::Missing));
        }

        #[test]
        fn base_unavailable_uses_absolute_only_and_preserves_relative() {
            let a = abs("projects/alpha");
            let mut map = HashMap::new();
            map.insert(a.clone(), valid(&a, SideStatus::ValidDirectProject, 1, 70));
            let resolver = MapResolver { map };
            let report = resolve_registrations(
                &[active_pair(0, &a, Some("projects/alpha"))],
                None,
                &[],
                None, // no base
                true,
                false,
                &resolver,
            );
            assert_eq!(report.active_selected(), vec![display_canonical(&a)]);
            assert_eq!(
                report.pairs[0].relative_side.status,
                SideStatus::BaseUnavailable
            );
            // The raw relative value is retained in hidden state.
            assert_eq!(
                report.pairs[0].raw_relative,
                RawStringField::string("projects/alpha")
            );
        }

        #[test]
        fn genuine_singular_appends_and_blocks_active_when_unresolved() {
            let a = abs("projects/alpha");
            let sing = abs("gone/singular");
            let mut map = HashMap::new();
            map.insert(a.clone(), valid(&a, SideStatus::ValidDirectProject, 1, 80));
            // sing not in map → Missing → unresolved genuine singular.
            let resolver = MapResolver { map };
            let singular = RawPair {
                source: ProjectSource::ProjectPath,
                index: None,
                absolute: RawStringField::string(&sing),
                relative: RawStringField::absent(),
            };
            let report = resolve_registrations(
                &[active_pair(0, &a, None)],
                Some(singular),
                &[],
                Some(base()),
                true,
                false,
                &resolver,
            );
            assert_eq!(
                report.active_registration_count, 2,
                "genuine singular counts"
            );
            assert_eq!(report.active_selected(), vec![display_canonical(&a)]);
            // The unresolved genuine singular blocks active reconciliation.
            assert!(!report.active_reconcile_eligible);
        }

        #[test]
        fn redundant_singular_is_dropped() {
            let a = abs("projects/alpha");
            let mut map = HashMap::new();
            map.insert(a.clone(), valid(&a, SideStatus::ValidDirectProject, 1, 90));
            let resolver = MapResolver { map };
            let singular = RawPair {
                source: ProjectSource::ProjectPath,
                index: None,
                absolute: RawStringField::string(&a),
                relative: RawStringField::absent(),
            };
            let report = resolve_registrations(
                &[active_pair(0, &a, None)],
                Some(singular),
                &[],
                Some(base()),
                false,
                false,
                &resolver,
            );
            assert_eq!(
                report.active_registration_count, 1,
                "redundant singular not counted"
            );
        }
    }
}
