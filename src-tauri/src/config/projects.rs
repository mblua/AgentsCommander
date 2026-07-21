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
use super::workspace::{existing_workspace_dir, has_workspace_dir, workspace_dir_for_project};

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
    WorkspaceMissing(PathBuf),
    #[error("failed to resolve absolute path for '{0}': {1}")]
    CwdFailure(String, std::io::Error),
    #[error("failed to create .ac directory at {0}: {1}")]
    WorkspaceCreateFailed(PathBuf, std::io::Error),
    #[error("failed to write Project AC Root .gitignore at {0}: {1}")]
    WorkspaceGitignoreFailed(PathBuf, String),
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
    if existing_workspace_dir(&abs).is_none() {
        return Err(ProjectError::WorkspaceMissing(abs));
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
    register_new_project_with_store(settings, raw_path, store)
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
) -> Result<ProjectRegistration, ProjectError> {
    let prepared = prepare_new_project_impl(raw_path, |workspace_dir| {
        crate::config::session_context::create_default_context_templates(workspace_dir)
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
) -> Result<ProjectRegistration, ProjectError> {
    register_new_project_with_store(
        settings,
        raw_path,
        NewProjectSettingsStore::Path(settings_path.to_path_buf()),
    )
}

#[derive(Debug)]
pub(crate) struct PreparedNewProject {
    abs: PathBuf,
    created: bool,
    guard: crate::config::seed_manifest::ProjectSeedManifestGuard,
}

impl PreparedNewProject {
    pub(crate) fn workspace_dir(&self) -> &Path {
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
    prepare_new_project_impl(raw_path, |workspace_dir| {
        crate::config::session_context::create_default_context_templates(workspace_dir)
    })
}

fn prepare_new_project_impl<F>(
    raw_path: &str,
    create_context_templates: F,
) -> Result<PreparedNewProject, ProjectError>
where
    F: FnOnce(&Path) -> Result<(), String>,
{
    prepare_new_project_impl_with_hook(raw_path, create_context_templates, |_, _| {})
}

fn prepare_new_project_impl_with_hook<F, H>(
    raw_path: &str,
    create_context_templates: F,
    after_create_before_gate: H,
) -> Result<PreparedNewProject, ProjectError>
where
    F: FnOnce(&Path) -> Result<(), String>,
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
        .map_err(|e| ProjectError::WorkspaceCreateFailed(abs.clone(), e))?;

    let workspace_dir = workspace_dir_for_project(&abs);
    // The create syscall is the sole creation-intent authority. An earlier
    // existence observation cannot distinguish this caller from a contender.
    let created = match std::fs::create_dir(&workspace_dir) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(e) => {
            return Err(ProjectError::WorkspaceCreateFailed(
                workspace_dir.clone(),
                e,
            ))
        }
    };
    let pinned_project = crate::config::seed_manifest::PinnedDirectory::open(&abs)
        .map_err(|error| ProjectError::ProjectSetupChanged(abs.clone(), error.to_string()))?;
    let pinned_workspace = crate::config::seed_manifest::PinnedDirectory::open(&workspace_dir)
        .map_err(|error| ProjectError::ProjectSetupChanged(abs.clone(), error.to_string()))?;
    after_create_before_gate(&workspace_dir, created);
    pinned_project
        .revalidate()
        .map_err(|error| ProjectError::ProjectSetupChanged(abs.clone(), error.to_string()))?;
    pinned_workspace
        .revalidate()
        .map_err(|error| ProjectError::ProjectSetupChanged(abs.clone(), error.to_string()))?;

    let guard = crate::config::seed_manifest::ProjectSeedManifestGuard::acquire(&abs)
        .map_err(|error| ProjectError::ProjectSetupChanged(abs.clone(), error.to_string()))?;
    pinned_project
        .revalidate()
        .map_err(|error| ProjectError::ProjectSetupChanged(abs.clone(), error.to_string()))?;
    pinned_workspace
        .revalidate_at(guard.ac_root())
        .map_err(|error| ProjectError::ProjectSetupChanged(abs.clone(), error.to_string()))?;
    guard
        .revalidate_owner()
        .map_err(|error| ProjectError::ProjectSetupChanged(abs.clone(), error.to_string()))?;
    if created {
        if let Err(e) = crate::commands::ac_discovery::ensure_workspace_gitignore(&workspace_dir) {
            return Err(ProjectError::WorkspaceGitignoreFailed(
                workspace_dir.clone(),
                e,
            ));
        }
    } else {
        // Gitignore sweep (Round-1 G15): best-effort when `.ac` pre-existed
        // because a transient FS error on someone else's gitignore should not
        // fail registration of a valid project.
        if let Err(e) = crate::commands::ac_discovery::ensure_workspace_gitignore(&workspace_dir) {
            log::warn!(
                "[projects] gitignore sweep failed on pre-existing Project AC Root at {:?}: {} (best-effort, continuing)",
                workspace_dir, e
            );
        }
    }

    if let Err(e) = create_context_templates(&workspace_dir) {
        return Err(ProjectError::ContextTemplatesCreateFailed(
            workspace_dir.clone(),
            e,
        ));
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
                has_workspace: has_workspace_dir(path),
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
    if !is_real_directory(path) || !has_workspace_dir(path) {
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
    fn open_rejects_path_without_workspace() {
        let fix = FixtureRoot::new("proj-open-no-workspace");
        let mut s = AppSettings::default();
        let r = register_existing_project(&mut s, fix.path().to_str().unwrap());
        assert!(matches!(r, Err(ProjectError::WorkspaceMissing(_))));
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
    fn open_rejects_non_ac_workspace() {
        let fix = FixtureRoot::new("proj-open-invalid-workspace");
        std::fs::create_dir_all(fix.path().join(".workspace")).unwrap();
        let mut s = AppSettings::default();
        let r = register_existing_project(&mut s, fix.path().to_str().unwrap());
        assert!(matches!(r, Err(ProjectError::WorkspaceMissing(_))));
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
            existing_workspace_dir(fix.path()),
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
        let workspace_dir = fix.path().join(".ac");
        std::fs::create_dir_all(&workspace_dir).unwrap();
        let agent_template = workspace_dir.join("Context.AgentsCommander.md");
        let coordinator_template = workspace_dir.join("Context.coordinator.md");
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

        let first_result =
            prepare_new_project_impl(fix.path().to_str().unwrap(), |workspace_dir| {
                std::fs::write(
                    workspace_dir
                        .join(crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME),
                    crate::config::session_context::get_default_agent_template(),
                )
                .expect("write partial agent context");
                Err("injected seed failure".to_string())
            });

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
                |workspace_dir| {
                    crate::config::session_context::create_default_context_templates(workspace_dir)
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
                |_workspace_dir| Err("injected creator failure after gate".to_string()),
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
            |workspace_dir| {
                crate::config::session_context::create_default_context_templates(workspace_dir)
            },
            |workspace_dir, created| {
                assert!(created);
                let detached = workspace_dir.with_extension("ac-detached");
                std::fs::rename(workspace_dir, &detached).expect("detach created root");
                std::fs::create_dir(workspace_dir).expect("install replacement root");
                std::fs::write(workspace_dir.join("foreign.txt"), b"FOREIGN")
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
    fn new_skips_creation_when_ac_already_exists_via_workspace_lookup() {
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
    fn archived_projects_reports_missing_folder_and_missing_workspace() {
        let full = FixtureRoot::new("proj-archived-full");
        std::fs::create_dir_all(full.path().join(".ac")).expect("full .ac");
        let no_workspace = FixtureRoot::new("proj-archived-no-workspace");
        let deleted = FixtureRoot::new("proj-archived-deleted");
        let deleted_path = deleted.path().to_string_lossy().to_string();
        std::fs::remove_dir_all(deleted.path()).expect("delete fixture");

        let rows = archived_projects_from_paths(&[
            full.path().to_string_lossy().to_string(),
            no_workspace.path().to_string_lossy().to_string(),
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
        // `workspace_dir` must serialize to `workspaceDir`. If the
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
            !json.contains("workspace_dir"),
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
}
