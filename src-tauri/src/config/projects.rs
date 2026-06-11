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
    register_new_project_impl(settings, raw_path, |workspace_dir| {
        crate::config::session_context::create_default_context_templates(workspace_dir)
    })
}

fn register_new_project_impl<F>(
    settings: &mut AppSettings,
    raw_path: &str,
    create_context_templates: F,
) -> Result<ProjectRegistration, ProjectError>
where
    F: FnOnce(&Path) -> Result<(), String>,
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

    let (workspace_dir, created) = match existing_workspace_dir(&abs) {
        Some(dir) => (dir, false),
        None => {
            let workspace_dir = workspace_dir_for_project(&abs);
            // Authoritative `created` flag (Round-1 G9): use non-recursive
            // `create_dir` so we can distinguish "we made the dir" from "another
            // process beat us to it" via `ErrorKind::AlreadyExists`. The previous
            // `is_dir()` check then `create_dir_all` pattern lied under that race.
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
            (workspace_dir, created)
        }
    };
    if created {
        if let Err(e) = crate::commands::ac_discovery::ensure_workspace_gitignore(&workspace_dir) {
            cleanup_new_workspace_after_setup_failure(&workspace_dir);
            return Err(ProjectError::WorkspaceGitignoreFailed(
                workspace_dir.clone(),
                e,
            ));
        }

        if let Err(e) = create_context_templates(&workspace_dir) {
            cleanup_new_workspace_after_setup_failure(&workspace_dir);
            return Err(ProjectError::ContextTemplatesCreateFailed(
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

    let abs_str = abs.to_string_lossy().into_owned();
    let registered = upsert_project_path(settings, &abs_str);
    Ok(ProjectRegistration {
        path: abs_str,
        registered,
        created,
    })
}

fn cleanup_new_workspace_after_setup_failure(workspace_dir: &Path) {
    if let Err(cleanup_err) = std::fs::remove_dir_all(workspace_dir) {
        log::warn!(
            "Failed to clean up newly created AC workspace at {:?} after setup failure: {}",
            workspace_dir,
            cleanup_err
        );
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

fn absolutise(raw: &str) -> Result<PathBuf, ProjectError> {
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
fn normalize_for_compare(s: &str) -> String {
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

fn enumerate_registered_project_candidates(project_paths: &[String]) -> Vec<ProjectResolution> {
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
    fn new_retries_after_failed_fresh_context_seed() {
        let fix = FixtureRoot::new("proj-new-template-retry");
        let mut s = AppSettings::default();

        let first_result =
            register_new_project_impl(&mut s, fix.path().to_str().unwrap(), |workspace_dir| {
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
            !fix.path().join(".ac").exists(),
            "fresh .ac directory should be cleaned up after seed failure"
        );
        assert!(s.project_paths.is_empty());

        let retry = register_new_project(&mut s, fix.path().to_str().unwrap())
            .expect("retry register new project");

        assert!(retry.created);
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
    fn new_does_not_backfill_templates_when_ac_already_exists() {
        let fix = FixtureRoot::new("proj-new-template-no-backfill");
        std::fs::create_dir_all(fix.path().join(".ac")).unwrap();
        let mut s = AppSettings::default();

        let r = register_new_project(&mut s, fix.path().to_str().unwrap()).unwrap();

        assert!(!r.created);
        assert!(!fix
            .path()
            .join(".ac")
            .join("Context.AgentsCommander.md")
            .exists());
        assert!(!fix
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

    // ── absolutise: relative + dot-dot collapse (Round-1 G4 + G13) ────────

    /// CWD is process-wide; restore on Drop. Any other test that mutates
    /// CWD in this same module would race — keep this confined.
    struct CwdGuard(PathBuf);
    impl Drop for CwdGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.0);
        }
    }

    #[test]
    fn absolutise_resolves_relative_path_against_cwd() {
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
    }
}
