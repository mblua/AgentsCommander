//! Tauri commands wrapping `cli::task_ops::perform` for the TASK.md panel
//! action buttons (issue #162).
//!
//! Trust model: these commands run inside the GUI process under the
//! user's authority, same model as `rename_session`, `destroy_session`,
//! etc. No coordinator gate (the CLI verbs in `cli/task_*.rs` retain
//! their gate; this file does not call into them).

use std::path::Path;
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use crate::cli::task_ops::{self, TaskOp};
use crate::config::settings::SettingsState;
use crate::config::workspace::is_workspace_dir_name; // gate 5 (path-based clean, #545)
use crate::session::manager::SessionManager;
use crate::session::session::find_workgroup_task_path_for_cwd;

/// Payload returned to the frontend after a successful task mutation.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskUpdateResult {
    /// Absolute path of the workgroup root the TASK.md belongs to.
    /// Stripped of the Windows `\\?\` extended-length prefix when present.
    pub workgroup_root: String,
    /// Trimmed TASK.md content as displayed by the panel. `None` when the
    /// file is empty or missing post-edit (defensive — should not happen
    /// after a successful Wrote, but possible on race-deletion).
    pub task: Option<String>,
}

/// Resolve the workgroup root for a session id, returning a user-facing
/// error string suitable for direct propagation through the Tauri command
/// `Result<_, String>` boundary.
async fn resolve_wg_root(
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    session_id: &str,
) -> Result<std::path::PathBuf, String> {
    let uuid = Uuid::parse_str(session_id).map_err(|e| format!("invalid session id: {}", e))?;
    let mgr = session_mgr.read().await;
    let cwd = mgr
        .get_session(uuid)
        .await
        .map(|s| s.working_directory.clone())
        .ok_or_else(|| format!("session {} not found", session_id))?;
    drop(mgr);

    let task_path = find_workgroup_task_path_for_cwd(&cwd)
        .ok_or_else(|| format!("session {} is not under a wg-* ancestor", session_id))?;
    let wg_root = task_path
        .parent()
        .ok_or_else(|| "workgroup TASK.md path has no parent".to_string())?
        .to_path_buf();
    Ok(wg_root)
}

/// Validate a raw `wg-*` root path supplied by the frontend for the path-based
/// clean (#545). The session-based commands derive the root from a real session
/// cwd; this one trusts a caller-supplied string, so it must prove the path is a
/// real workgroup directory under a configured project root before any write, so
/// it cannot be coerced into writing a TASK.md anywhere on disk.
///
/// Returns the ORIGINAL (validated) path as a `PathBuf`, NOT the canonical form;
/// the canonical form is used ONLY for the five safety gates. The write/emit must
/// use the original arg because `std::fs::canonicalize` on Windows expands 8.3
/// short names (`PROGRA~1` -> `Program Files`) and resolves symlinks/junctions to
/// their real target, while discovery builds the stored `wg.path` from the raw
/// project path joined with on-disk names, with no canonicalization. So a
/// canonical emit key could diverge from the stored `wg.path` and the sidebar row
/// would silently fail to update. (This is NOT about the `\\?\` prefix, which is
/// already stripped both by backend `strip_unc` and by the frontend path
/// normalizer.) Do not "fix" this by switching the write/emit to canonical.
fn validate_wg_root(
    workgroup_root: &str,
    project_paths: &[String],
) -> Result<std::path::PathBuf, String> {
    // 1. Canonicalize: proves existence and resolves `..` + symlinks to a real
    //    absolute path. Mirrors open_in_explorer (window.rs). A traversal or a
    //    symlink that escapes is normalized here and then caught by gate 4.
    let canonical = std::fs::canonicalize(workgroup_root).map_err(|_| {
        format!(
            "workgroup root does not exist or is inaccessible: {}",
            workgroup_root
        )
    })?;

    // 2. Must be a directory.
    if !canonical.is_dir() {
        return Err(format!(
            "workgroup root is not a directory: {}",
            workgroup_root
        ));
    }

    // 3. Leaf name must start with `wg-`. Matches the discovery invariant and the
    //    session command's `wg-*` ancestor constraint. Blocks writing a TASK.md
    //    into a non-workgroup directory.
    let is_wg = canonical
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with("wg-"));
    if !is_wg {
        return Err(format!(
            "workgroup root is not a wg-* directory: {}",
            workgroup_root
        ));
    }

    // 4. Must be a component-wise descendant of a configured project root. Fail
    //    closed: if no project path canonicalizes to an ancestor, reject.
    //    `Path::starts_with` is COMPONENT-wise (not a string prefix), and both
    //    sides are canonicalized (OS-consistent casing on Windows), so `C:\proj`
    //    does not match `C:\project-x` and separator/prefix mismatches cannot
    //    sneak through. Un-canonicalizable project entries are skipped, not fatal.
    let under_known_root = project_paths.iter().any(|base| {
        std::fs::canonicalize(base)
            .map(|canon_base| canonical.starts_with(&canon_base))
            .unwrap_or(false)
    });
    if !under_known_root {
        return Err(format!(
            "workgroup root is not under a configured project path: {}",
            workgroup_root
        ));
    }

    // 5. The wg-* dir must be a DIRECT child of an `.ac` workspace dir, i.e. a
    //    genuine AC workgroup root, not merely a dir whose leaf starts with `wg-`.
    //    Discovery only ever surfaces workgroups as direct children of `<base>/.ac`,
    //    so this never rejects a real wg.path the frontend can send, and it blocks
    //    creating/clobbering a TASK.md (perform creates one when missing) in an
    //    arbitrary `wg-*`-named dir under a project root. `is_workspace_dir_name`
    //    is case-insensitive, matching `.ac`/`.AC` on Windows.
    let parent_is_workspace = canonical
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .is_some_and(is_workspace_dir_name);
    if !parent_is_workspace {
        return Err(format!(
            "workgroup root is not a direct child of an .ac workspace dir: {}",
            workgroup_root
        ));
    }

    Ok(std::path::PathBuf::from(workgroup_root))
}

fn strip_unc(p: &Path) -> String {
    let raw = p.to_string_lossy().into_owned();
    raw.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(raw)
}

fn emit_task_updated(
    app: &AppHandle,
    wg_root: &Path,
    task: &Option<String>,
    task_title: &Option<String>,
) {
    let _ = app.emit(
        "workgroup_task_updated",
        serde_json::json!({
            "workgroupRoot": strip_unc(wg_root),
            "source": "manual",
            "task": task.clone(),
            "taskTitle": task_title.clone(),
        }),
    );
}

/// Read the current YAML-frontmatter `title:` value of the workgroup
/// TASK.md for the given session. Returns `None` when there is no
/// frontmatter or no `title:` line.
#[tauri::command]
pub async fn task_get_title(
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    session_id: String,
) -> Result<Option<String>, String> {
    let wg_root = resolve_wg_root(&session_mgr, &session_id).await?;
    let task_path = wg_root.join("TASK.md");
    let content = match std::fs::read_to_string(&task_path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("read TASK.md: {}", e)),
    };
    let parsed = task_ops::parse_task(&content);
    Ok(task_ops::title_value_of(&parsed))
}

/// Set the YAML-frontmatter `title:` field of the workgroup TASK.md for
/// the given session. Returns the new (post-edit) trimmed TASK.md
/// content for direct local refresh, AND emits `workgroup_task_updated`
/// for sibling sessions/windows.
#[tauri::command]
pub async fn task_set_title(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    session_id: String,
    title: String,
) -> Result<TaskUpdateResult, String> {
    if title.trim().is_empty() {
        return Err("title cannot be empty".to_string());
    }
    if title.chars().any(|c| c.is_control() && c != '\t') {
        return Err("title must be a single line of printable characters \
             (control characters other than tab are not allowed)"
            .to_string());
    }
    // Round 2 (Grinch LOW-1): cap at 256 chars (typical YAML scalar
    // convention). Prevents a 1 MB pasted blob from becoming the title
    // and breaking panel layout / file ergonomics. Counts Unicode
    // scalars, not bytes — a 256-emoji title is allowed and renders
    // sensibly.
    if title.chars().count() > 256 {
        return Err("title is too long (max 256 characters)".to_string());
    }
    let wg_root = resolve_wg_root(&session_mgr, &session_id).await?;
    let outcome =
        task_ops::perform(&wg_root, TaskOp::SetUserTitle(title)).map_err(|e| e.to_string())?;
    log::info!(
        "[task] set_title for session {} -> {:?}",
        session_id,
        outcome
    );

    let (content, task_title) = match &outcome {
        task_ops::EditOutcome::Wrote { content, title, .. } => (content.clone(), title.clone()),
        task_ops::EditOutcome::NoOp { content, title } => (content.clone(), title.clone()),
        task_ops::EditOutcome::RejectedUserTitle { content, title } => {
            (content.clone(), title.clone())
        }
    };
    let trimmed = content.trim();
    let task = if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    };
    let result = TaskUpdateResult {
        workgroup_root: strip_unc(&wg_root),
        task: task.clone(),
    };
    emit_task_updated(&app, &wg_root, &task, &task_title);
    Ok(result)
}

/// Clear the workgroup TASK.md for the given session to the canonical Clean state.
/// Returns the new (post-edit) trimmed TASK.md content for direct local refresh,
/// AND emits `workgroup_task_updated` for sibling sessions/windows.
#[tauri::command]
pub async fn task_clean(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    session_id: String,
) -> Result<TaskUpdateResult, String> {
    let wg_root = resolve_wg_root(&session_mgr, &session_id).await?;
    let outcome = task_ops::perform(&wg_root, TaskOp::Clean).map_err(|e| e.to_string())?;
    log::info!("[task] clean for session {} -> {:?}", session_id, outcome);

    let (content, task_title) = match &outcome {
        task_ops::EditOutcome::Wrote { content, title, .. } => (content.clone(), title.clone()),
        task_ops::EditOutcome::NoOp { content, title } => (content.clone(), title.clone()),
        task_ops::EditOutcome::RejectedUserTitle { content, title } => {
            (content.clone(), title.clone())
        }
    };
    let trimmed = content.trim();
    let task = if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    };
    let result = TaskUpdateResult {
        workgroup_root: strip_unc(&wg_root),
        task: task.clone(),
    };
    emit_task_updated(&app, &wg_root, &task, &task_title);
    Ok(result)
}

/// Clear the workgroup TASK.md addressed directly by its `wg-*` root path, for
/// cold workgroups with no live/exited session to resolve from (#545). Mirrors
/// `task_clean` after the root is resolved: same perform + emit + TaskUpdateResult.
/// The raw path is validated by `validate_wg_root` before any write (see the
/// helper docs for the threat model).
#[tauri::command]
pub async fn task_clean_at(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    workgroup_root: String,
) -> Result<TaskUpdateResult, String> {
    // Clone the small Vec and drop the read guard immediately; perform() does
    // blocking IO and must not be held across the settings lock.
    let project_paths = { settings.read().await.project_paths.clone() };
    let wg_root = validate_wg_root(&workgroup_root, &project_paths)?;

    let outcome = task_ops::perform(&wg_root, TaskOp::Clean).map_err(|e| e.to_string())?;
    log::info!("[task] clean_at for {} -> {:?}", workgroup_root, outcome);

    let (content, task_title) = match &outcome {
        task_ops::EditOutcome::Wrote { content, title, .. } => (content.clone(), title.clone()),
        task_ops::EditOutcome::NoOp { content, title } => (content.clone(), title.clone()),
        task_ops::EditOutcome::RejectedUserTitle { content, title } => {
            (content.clone(), title.clone())
        }
    };
    let trimmed = content.trim();
    let task = if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    };
    let result = TaskUpdateResult {
        workgroup_root: strip_unc(&wg_root),
        task: task.clone(),
    };
    emit_task_updated(&app, &wg_root, &task, &task_title);
    Ok(result)
}

#[cfg(test)]
mod tests {
    //! Covers `cli::task_ops::perform` and `validate_wg_root` through the
    //! local `read_task_fields_at` reader: a single read of TASK.md must
    //! return BOTH the trimmed body and the parsed YAML `title:`. The reader
    //! is test-only (#1177); #301 shipped without a production caller, and the
    //! assertions it backs still cover the live save and clean paths.
    use std::path::Path;

    use super::validate_wg_root;
    use crate::cli::task_ops::{perform, EditOutcome, TaskOp};

    /// Read TASK.md once and return both the trimmed full body and the parsed
    /// YAML `title:` value. Returning both from one read avoids torn results when
    /// an external writer races us between two reads. Caller emits both fields so
    /// the sidebar can update its title without waiting for the next 15s poll.
    fn read_task_fields_at(wg_root: &Path) -> (Option<String>, Option<String>) {
        let Ok(content) = std::fs::read_to_string(wg_root.join("TASK.md")) else {
            return (None, None);
        };
        let task_title = crate::commands::entity_creation::parse_task_title(&content);
        let trimmed = content.trim();
        let task = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
        (task, task_title)
    }

    #[test]
    fn read_task_fields_at_missing_file_returns_none_pair() {
        let dir = tempfile::tempdir().unwrap();
        let (task, title) = read_task_fields_at(dir.path());
        assert_eq!(task, None);
        assert_eq!(title, None);
    }

    #[test]
    fn read_task_fields_at_empty_file_returns_none_pair() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("TASK.md"), "").unwrap();
        let (task, title) = read_task_fields_at(dir.path());
        assert_eq!(task, None);
        assert_eq!(title, None);
    }

    #[test]
    fn read_task_fields_at_parses_frontmatter_title_and_body() {
        let dir = tempfile::tempdir().unwrap();
        let content = "---\ntitle: 'My Brief'\n---\nbody line\n";
        std::fs::write(dir.path().join("TASK.md"), content).unwrap();
        let (task, title) = read_task_fields_at(dir.path());
        assert_eq!(title.as_deref(), Some("My Brief"));
        assert_eq!(task.as_deref(), Some(content.trim()));
    }

    #[test]
    fn read_task_fields_at_body_only_no_frontmatter_returns_body_and_no_title() {
        // grinch LOW (PR #304 review): coverage gap — a file with body content
        // but no YAML frontmatter must return (Some(body), None), not (None, _).
        let dir = tempfile::tempdir().unwrap();
        let content = "Just a body line\nmore body\n";
        std::fs::write(dir.path().join("TASK.md"), content).unwrap();
        let (task, title) = read_task_fields_at(dir.path());
        assert_eq!(title, None);
        assert_eq!(task.as_deref(), Some(content.trim()));
    }

    #[test]
    fn set_title_via_task_ops_round_trip_returns_title() {
        // End-to-end mirror of the task_set_title body: perform() then
        // read_task_fields_at(). Validates that the path the Tauri command
        // takes ends up with a non-empty taskTitle in the payload.
        let dir = tempfile::tempdir().unwrap();
        perform(dir.path(), TaskOp::SetTitle("Hello World".to_string())).expect("set title");
        let (task, title) = read_task_fields_at(dir.path());
        assert_eq!(title.as_deref(), Some("Hello World"));
        assert!(
            task.is_some(),
            "task body should not be empty after set-title"
        );
    }

    #[test]
    fn clean_via_task_ops_round_trip_returns_clean_title() {
        // After Clean, the canonical title is "Clean" (see TaskOp::Clean
        // docs). The important thing for issue #301 is the payload carries
        // the title at all (no None → no undefined → no spread-clobber).
        let dir = tempfile::tempdir().unwrap();
        perform(dir.path(), TaskOp::SetTitle("Old Title".to_string())).expect("set initial title");
        perform(dir.path(), TaskOp::Clean).expect("clean");
        let (_task, title) = read_task_fields_at(dir.path());
        assert_eq!(title.as_deref(), Some("Clean"));
    }

    // ---- #545: validate_wg_root + task_clean_at path-based clean ----
    //
    // `validate_wg_root` takes `&[String]`, so it is unit-testable directly with
    // no Tauri harness. Gate 1 canonicalizes, so every fixture dir MUST exist on
    // disk. `%TEMP%` is often itself a junction; the helper canonicalizes both the
    // candidate and each project base, so the component-wise `starts_with` still
    // holds. The `#[tauri::command]` wrapper (AppHandle/State) is not unit-testable
    // without a Tauri runtime, same as `task_clean`; the pure helper + perform
    // tests cover the logic.

    /// Create `<base>/.ac/wg-1` on disk and return its path as the string the
    /// frontend would send. Centralizes the gate-1-needs-existence requirement.
    fn make_real_wg(base: &std::path::Path) -> String {
        let wg = base.join(".ac").join("wg-1");
        std::fs::create_dir_all(&wg).unwrap();
        wg.to_string_lossy().into_owned()
    }

    fn paths_of(p: &std::path::Path) -> Vec<String> {
        vec![p.to_string_lossy().into_owned()]
    }

    // 1.
    #[test]
    fn validate_wg_root_accepts_real_wg_under_project_root() {
        let proj = tempfile::tempdir().unwrap();
        let wg = make_real_wg(proj.path());
        assert!(validate_wg_root(&wg, &paths_of(proj.path())).is_ok());
    }

    // 2. gate 1 (canonicalize fails on a missing path)
    #[test]
    fn validate_wg_root_rejects_nonexistent_path() {
        let proj = tempfile::tempdir().unwrap();
        let bogus = proj
            .path()
            .join(".ac")
            .join("wg-does-not-exist")
            .to_string_lossy()
            .into_owned();
        assert!(validate_wg_root(&bogus, &paths_of(proj.path())).is_err());
    }

    // 3. gate 3 (leaf does not start with `wg-`)
    #[test]
    fn validate_wg_root_rejects_non_wg_dir() {
        let proj = tempfile::tempdir().unwrap();
        let notwg = proj.path().join(".ac").join("notwg");
        std::fs::create_dir_all(&notwg).unwrap();
        let arg = notwg.to_string_lossy().into_owned();
        assert!(validate_wg_root(&arg, &paths_of(proj.path())).is_err());
    }

    // 4. gate 2 (a file, not a directory)
    #[test]
    fn validate_wg_root_rejects_file_not_dir() {
        let proj = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(proj.path().join(".ac")).unwrap();
        let file = proj.path().join(".ac").join("wg-x");
        std::fs::write(&file, "x").unwrap();
        let arg = file.to_string_lossy().into_owned();
        assert!(validate_wg_root(&arg, &paths_of(proj.path())).is_err());
    }

    // 5. gate 4 (a sibling root, not under the configured project path). The
    //    fixture parent is `.ac` so it passes gate 5, isolating gate 4.
    #[test]
    fn validate_wg_root_rejects_path_outside_project_roots() {
        let proj = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let wg = make_real_wg(other.path());
        // project_paths points at `proj`, but the candidate lives under `other`.
        assert!(validate_wg_root(&wg, &paths_of(proj.path())).is_err());
    }

    // 6. gate 4 via traversal: `..` is normalized by canonicalize, then confined,
    //    not naively string-rejected.
    #[test]
    fn validate_wg_root_traversal_cannot_escape() {
        let root = tempfile::tempdir().unwrap();
        let proj = root.path().join("proj");
        let other = root.path().join("other");
        std::fs::create_dir_all(proj.join(".ac").join("wg-1")).unwrap();
        std::fs::create_dir_all(other.join(".ac").join("wg-1")).unwrap();
        // proj/.ac/wg-1/../../../other/.ac/wg-1 canonicalizes to other/.ac/wg-1.
        let arg = proj
            .join(".ac")
            .join("wg-1")
            .join("..")
            .join("..")
            .join("..")
            .join("other")
            .join(".ac")
            .join("wg-1")
            .to_string_lossy()
            .into_owned();
        assert!(validate_wg_root(&arg, &paths_of(&proj)).is_err());
    }

    // 7. end-to-end: validate then perform(Clean) yields the canonical Clean title.
    #[test]
    fn task_clean_at_round_trip_returns_clean_title() {
        let proj = tempfile::tempdir().unwrap();
        let wg = make_real_wg(proj.path());
        perform(
            std::path::Path::new(&wg),
            TaskOp::SetTitle("Real Brief".to_string()),
        )
        .expect("seed title");
        let validated = validate_wg_root(&wg, &paths_of(proj.path())).expect("validate");
        perform(&validated, TaskOp::Clean).expect("clean");
        let (_task, title) = read_task_fields_at(&validated);
        assert_eq!(title.as_deref(), Some("Clean"));
    }

    // 8. gate 4 fail-closed: an empty project_paths slice rejects everything.
    #[test]
    fn validate_wg_root_rejects_empty_project_paths() {
        let proj = tempfile::tempdir().unwrap();
        let wg = make_real_wg(proj.path());
        let empty: Vec<String> = vec![];
        assert!(validate_wg_root(&wg, &empty).is_err());
    }

    // 9. an un-canonicalizable project entry is skipped, never a wildcard match;
    //    one bad entry does not poison a valid one.
    #[test]
    fn validate_wg_root_skips_uncanonicalizable_project_entry() {
        let proj = tempfile::tempdir().unwrap();
        let wg = make_real_wg(proj.path());
        let only_bogus = vec!["Z:\\does\\not\\exist".to_string()];
        assert!(validate_wg_root(&wg, &only_bogus).is_err());
        let bogus_plus_real = vec![
            "Z:\\does\\not\\exist".to_string(),
            proj.path().to_string_lossy().into_owned(),
        ];
        assert!(validate_wg_root(&wg, &bogus_plus_real).is_ok());
    }

    // 10. a single descendant-of-base check (gate 4) covers the
    //     `<base>/<child>/.ac/wg-*` discovery layout; gate 5 still passes.
    #[test]
    fn validate_wg_root_accepts_wg_under_child_of_base() {
        let proj = tempfile::tempdir().unwrap();
        let wg = proj.path().join("child").join(".ac").join("wg-1");
        std::fs::create_dir_all(&wg).unwrap();
        let arg = wg.to_string_lossy().into_owned();
        assert!(validate_wg_root(&arg, &paths_of(proj.path())).is_ok());
    }

    // 11. core contract: the validated path returned is the ORIGINAL arg, never
    //     the canonical form (a canonical return would break event-key matching
    //     at runtime while passing every other test).
    #[test]
    fn validate_wg_root_returns_original_not_canonical() {
        let proj = tempfile::tempdir().unwrap();
        let wg = make_real_wg(proj.path());
        let result = validate_wg_root(&wg, &paths_of(proj.path())).expect("validate");
        assert_eq!(result, std::path::PathBuf::from(&wg));
        // On a junction-backed temp dir (common for %TEMP% on Windows) the
        // canonical form differs from the arg; when it does, the result must be
        // the arg, not the canonical path.
        if let Ok(canon) = std::fs::canonicalize(&wg) {
            if canon.as_path() != std::path::Path::new(&wg) {
                assert_ne!(result, canon, "must return original arg, not canonical");
            }
        }
    }

    // 12. canonicalize normalizes case, so gate 4's case-sensitive `starts_with`
    //     still matches a case-variant input. Windows-only: case-insensitive path
    //     semantics are platform-specific.
    #[cfg(windows)]
    #[test]
    fn validate_wg_root_accepts_case_variant_input() {
        let proj = tempfile::tempdir().unwrap();
        let wg = make_real_wg(proj.path());
        // Flip the drive-letter case (e.g. C:\ -> c:\); canonicalize resolves it
        // back to the real on-disk casing.
        let mut chars: Vec<char> = wg.chars().collect();
        if let Some(first) = chars.first_mut() {
            *first = if first.is_ascii_uppercase() {
                first.to_ascii_lowercase()
            } else {
                first.to_ascii_uppercase()
            };
        }
        let variant: String = chars.into_iter().collect();
        let r = validate_wg_root(&variant, &paths_of(proj.path()));
        assert!(
            r.is_ok(),
            "case-variant drive letter should validate: {:?}",
            r
        );
    }

    // B1. create-on-missing: a wg dir with no TASK.md gets a canonical Clean file
    //     written (backup None because nothing pre-existed). Pins this capability
    //     as a conscious, gate-5-guarded behavior.
    #[test]
    fn task_clean_at_creates_task_when_missing() {
        let proj = tempfile::tempdir().unwrap();
        let wg = make_real_wg(proj.path());
        let validated = validate_wg_root(&wg, &paths_of(proj.path())).expect("validate");
        assert!(
            !validated.join("TASK.md").exists(),
            "precondition: no TASK.md"
        );
        let outcome = perform(&validated, TaskOp::Clean).expect("clean creates file");
        assert!(matches!(outcome, EditOutcome::Wrote { backup: None, .. }));
        assert!(validated.join("TASK.md").exists());
        let (_task, title) = read_task_fields_at(&validated);
        assert_eq!(title.as_deref(), Some("Clean"));
    }

    // B2. gate 5: a `wg-*` dir whose parent is NOT an `.ac` workspace dir is
    //     rejected even though it passes gates 1-4. This is the footgun closer.
    #[test]
    fn validate_wg_root_rejects_wg_dir_not_under_dot_ac() {
        let proj = tempfile::tempdir().unwrap();
        let wg = proj.path().join("sub").join("wg-1");
        std::fs::create_dir_all(&wg).unwrap();
        let arg = wg.to_string_lossy().into_owned();
        assert!(validate_wg_root(&arg, &paths_of(proj.path())).is_err());
    }
}
