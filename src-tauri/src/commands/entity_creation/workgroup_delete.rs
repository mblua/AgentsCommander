use super::delete_guard::{
    ensure_no_live_sessions_under_manager, is_rename_blocked_by_handle,
    validate_delete_root_not_link_or_reparse,
};
use super::matrix_layout::{selected_ac_root, validate_existing_name};
use super::team_update::emit_coordinator_refresh;
use super::workgroup::{acquire_lifecycle_project_gate, prune_workgroup_config_scope};
use crate::config::seed_manifest::ManifestActivationToken;
use crate::session::manager::SessionManager;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Manager, State};

/// Delete a single workgroup directory from {project_path}/.ac/{wg_name}/
/// Returns dirty repo list as an Err if any repos have uncommitted/unpushed work.
/// Project-only gated workgroup-delete primitive (#1063, plan sections 5.4/6.3).
///
/// Acquires the project seed-manifest gate, runs `delete` under it, prunes the
/// workgroup config scope when the logical path was removed (`Deleted`/`Partial`),
/// and releases the gate. It NEVER acquires the team-config guard. Blocking:
/// production runs it inside `run_blocking_owned` with `None` activation and a
/// no-op barrier; tests call it directly with `Some(for_test())` and/or an
/// `after_project_acquired` cross-process inversion barrier, and may inject the
/// `delete` outcome to cover `Partial`/`Blocked` pruning.
pub(crate) fn gated_workgroup_delete_with(
    project_root: &Path,
    workgroup_name: &str,
    activation: Option<&ManifestActivationToken>,
    after_project_acquired: impl FnOnce(),
    delete: impl FnOnce() -> WgDeleteOutcome,
) -> Result<WgDeleteOutcome, String> {
    let mut project_gate = acquire_lifecycle_project_gate(project_root)?;
    after_project_acquired();
    let outcome = delete();
    if outcome.logical_path_removed() {
        prune_workgroup_config_scope(project_gate.as_mut(), activation, workgroup_name);
    }
    drop(project_gate);
    Ok(outcome)
}

/// Pass `force = true` to skip the dirty-repo safety check (user already confirmed).
#[tauri::command]
pub async fn delete_workgroup(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    project_path: String,
    workgroup_name: String,
    force: Option<bool>,
) -> Result<(), String> {
    validate_existing_name(&workgroup_name, "Room")?;

    let base = selected_ac_root(Path::new(&project_path))?;

    let wg_dir = base.join(&workgroup_name);
    validate_delete_root_not_link_or_reparse(&wg_dir)?;

    ensure_no_live_sessions_under_manager(&wg_dir, session_mgr.inner()).await?;

    // Safety check: detect dirty repos before deleting (skip if force)
    if !force.unwrap_or(false) {
        let dirty_repos = check_workgroup_repos_dirty(std::slice::from_ref(&wg_dir));
        if !dirty_repos.is_empty() {
            let list = dirty_repos
                .iter()
                .map(|(repo, reason)| format!("  - {} ({})", repo, reason))
                .collect::<Vec<_>>()
                .join("\n");
            // DIRTY_REPOS: prefix is a sentinel the frontend uses to detect this error type
            return Err(format!(
                "DIRTY_REPOS:Cannot delete room: the following repos have pending \
                 work:\n{}\n\nCommit or push changes before deleting.",
                list
            ));
        }
    }

    // Preflight rename probe (#113 follow-up): try an atomic same-parent rename
    // BEFORE remove_dir_all. NTFS rename requires DELETE access on every open
    // handle to the dir or any descendant; if any blocker holds a handle without
    // FILE_SHARE_DELETE (terminal cwd, VSCode workspace open, file watcher,
    // memory-mapped TASK.md), the rename fails atomically — no files touched —
    // and we run the diagnostic on the still-intact tree. On success the dir is
    // re-parented to a sentinel name and removed; the user-visible WG is gone.
    //
    // #1063: acquire the project-only seed-manifest gate around the atomic rename
    // and (#1065 Stage F) activated prune inside blocking work, then release it
    // before the async blocker diagnostics that
    // `delete_workgroup_dir_backend_with_outcome` runs.
    let project_root = Path::new(&project_path).to_path_buf();
    let workgroup = workgroup_name.clone();
    let wg_for_delete = wg_dir.clone();
    let outcome = crate::config::seed_manifest::run_blocking_owned(move || {
        let activation = ManifestActivationToken::production();
        gated_workgroup_delete_with(
            &project_root,
            &workgroup,
            Some(&activation),
            || {},
            || try_atomic_delete_wg(&wg_for_delete),
        )
    })
    .await
    .map_err(|e| format!("Room delete blocking preparation failed: {e}"))??;
    delete_workgroup_dir_backend_with_outcome(
        &wg_dir,
        &workgroup_name,
        session_mgr.inner(),
        outcome,
    )
    .await?;
    // (#621) Drop the workgroup's coordinator_clocks keys from the in-memory store
    // and persist immediately (this command is not on the auto-close flush tick).
    if let Some(project_name) = Path::new(&project_path)
        .file_name()
        .and_then(|n| n.to_str())
    {
        if let Some(clocks) =
            app.try_state::<crate::config::coordinator_clocks::CoordinatorClocksState>()
        {
            crate::config::coordinator_clocks::remove_workgroup_in_state(
                clocks.inner(),
                project_name,
                &workgroup_name,
            );
        }
    }
    log::info!(
        "[entity_creation] Deleted workgroup: {} (force={})",
        workgroup_name,
        force.unwrap_or(false)
    );
    emit_coordinator_refresh(&app, session_mgr.inner()).await;
    Ok(())
}

// #1063: production `delete_workgroup` now runs the delete under the project gate
// via `gated_workgroup_delete_with` and interprets the outcome with
// `delete_workgroup_dir_backend_with_outcome`, so this ungated convenience wrapper
// is retained for tests only.
#[cfg(all(test, windows))]
pub(crate) async fn delete_workgroup_dir_backend(
    wg_dir: &Path,
    workgroup_name: &str,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
) -> Result<(), String> {
    delete_workgroup_dir_backend_with_outcome(
        wg_dir,
        workgroup_name,
        session_mgr,
        try_atomic_delete_wg(wg_dir),
    )
    .await
}

pub(crate) async fn delete_workgroup_dir_backend_with_outcome(
    wg_dir: &Path,
    workgroup_name: &str,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    outcome: WgDeleteOutcome,
) -> Result<(), String> {
    match outcome {
        WgDeleteOutcome::Deleted => {
            // fall through to success path
        }
        WgDeleteOutcome::Blocked(e) => {
            let raw = e.to_string();
            log::info!(
                "[entity_creation] delete_workgroup: file-in-use detected for '{}' on rename \
                 probe, running blocker diagnostic on intact tree",
                workgroup_name
            );
            let report = crate::commands::wg_delete_diagnostic::diagnose_blockers(
                wg_dir,
                workgroup_name,
                &raw, // raw OS error verbatim — see plan §C.1
                session_mgr,
            )
            .await;
            let json = serde_json::to_string(&report).map_err(|se| {
                format!(
                    "Failed to serialize blocker report: {}; original error: {}",
                    se, raw
                )
            })?;
            return Err(format!("BLOCKERS:{}", json));
        }
        WgDeleteOutcome::Partial { orphan_path, error } => {
            return Err(format!(
                "Partial room delete: renamed '{}' to orphan '{}', but failed to remove orphan: {}",
                wg_dir.display(),
                orphan_path.display(),
                error
            ));
        }
        WgDeleteOutcome::Other(e) => {
            return Err(format!("Failed to delete room directory: {}", e));
        }
    }
    Ok(())
}

/// Check all repo-* dirs inside the given workgroup dirs for dirty git state.
/// Returns a list of (repo_display_name, reason) for repos with pending work.
pub(crate) fn check_workgroup_repos_dirty(wg_dirs: &[PathBuf]) -> Vec<(String, String)> {
    #[cfg(windows)]
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let mut dirty: Vec<(String, String)> = Vec::new();

    for wg_dir in wg_dirs {
        let wg_name = wg_dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let entries = match std::fs::read_dir(wg_dir) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let dir_name = entry.file_name();
            let dir_name_str = dir_name.to_string_lossy();
            if !dir_name_str.starts_with("repo-") {
                continue;
            }
            if !path.join(".git").exists() {
                continue;
            }

            let display = format!("{}/{}", wg_name, dir_name_str);
            let mut reasons: Vec<&str> = Vec::new();

            // Check for uncommitted changes (staged + unstaged + untracked)
            let mut cmd = std::process::Command::new("git");
            crate::pty::credentials::scrub_credentials_from_std_command(&mut cmd);
            crate::config::agent_path::apply_search_path_to_std_command(&mut cmd);
            cmd.args(["status", "--porcelain"])
                .current_dir(&path)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null());
            #[cfg(windows)]
            {
                #[allow(unused_imports)]
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(CREATE_NO_WINDOW);
            }
            if let Ok(output) = cmd.output() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if !stdout.trim().is_empty() {
                    reasons.push("uncommitted changes");
                }
            }

            // Check for unpushed commits
            let mut cmd2 = std::process::Command::new("git");
            crate::pty::credentials::scrub_credentials_from_std_command(&mut cmd2);
            crate::config::agent_path::apply_search_path_to_std_command(&mut cmd2);
            cmd2.args(["log", "@{upstream}..HEAD", "--oneline"])
                .current_dir(&path)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null());
            #[cfg(windows)]
            {
                #[allow(unused_imports)]
                use std::os::windows::process::CommandExt;
                cmd2.creation_flags(CREATE_NO_WINDOW);
            }
            match cmd2.output() {
                Ok(output) if output.status.success() => {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    if !stdout.trim().is_empty() {
                        reasons.push("unpushed commits");
                    }
                }
                _ => {
                    // No upstream configured — local-only branch = unpushed work
                    reasons.push("no remote upstream");
                }
            }

            if !reasons.is_empty() {
                dirty.push((display, reasons.join(", ")));
            }
        }
    }

    dirty
}

/// Result of a preflight-rename `delete_workgroup` attempt.
///
/// `pub(crate)` so the unit tests can pattern-match on the variants.
pub(crate) enum WgDeleteOutcome {
    /// Rename succeeded and the renamed dir was removed.
    Deleted,
    /// Rename succeeded, but deleting the renamed orphan failed. The
    /// user-visible workgroup name is gone, but callers must report this as a
    /// partial failure and skip refresh.
    Partial {
        orphan_path: PathBuf,
        error: std::io::Error,
    },
    /// Rename failed with a Windows file-in-use error. Tree is intact; caller
    /// should run the blocker diagnostic and return `BLOCKERS:` to the frontend.
    Blocked(std::io::Error),
    /// Rename failed with any other error (NotFound, permission, invalid path,
    /// …). Caller passes the raw error through unchanged.
    Other(std::io::Error),
}

impl WgDeleteOutcome {
    /// Whether the original user-visible workgroup path is gone (#1063, plan
    /// section 5.4). `true` for `Deleted` and `Partial` (the atomic rename moved
    /// the original path away, so its replica config rows are no longer active
    /// even when hidden-orphan cleanup failed); `false` for `Blocked`/`Other`,
    /// which leave the path intact and must preserve rows.
    pub(crate) fn logical_path_removed(&self) -> bool {
        matches!(
            self,
            WgDeleteOutcome::Deleted | WgDeleteOutcome::Partial { .. }
        )
    }
}

/// Atomically detect blockers before deleting a workgroup directory.
///
/// Strategy: rename the WG dir to a unique sentinel name in the same parent
/// (NTFS metadata-only operation, fails atomically if any handle blocks it),
/// then `remove_dir_all` the renamed dir. If rename fails with a file-in-use
/// error the WG is still intact, so the caller can run the diagnostic over the
/// original tree and surface a `BLOCKERS:` report.
///
/// Suffix scheme: `.deleting-<wg_name>-<uuid>` — leading `.` keeps any orphan
/// (rare race: rename succeeds but remove_dir_all fails) invisible to the
/// `entity_prefix::has_entity_prefix` filters in `ac_discovery` and
/// `cli::list_peers`, so an orphan won't surface as a ghost room. UUID is
/// used (already in `Cargo.toml`) to guarantee uniqueness across rapid retries.
///
/// `pub(crate)` so unit tests can drive it directly.
pub(crate) fn try_atomic_delete_wg(wg_dir: &Path) -> WgDeleteOutcome {
    try_atomic_delete_wg_with_remove(wg_dir, |path| std::fs::remove_dir_all(path))
}

pub(crate) fn try_atomic_delete_wg_with_remove(
    wg_dir: &Path,
    remove_dir_all: impl FnOnce(&Path) -> std::io::Result<()>,
) -> WgDeleteOutcome {
    let parent = match wg_dir.parent() {
        Some(p) => p,
        None => {
            return WgDeleteOutcome::Other(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "room directory has no parent",
            ));
        }
    };
    let original_name = match wg_dir.file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => {
            return WgDeleteOutcome::Other(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "room directory has no filename",
            ));
        }
    };
    let temp_name = format!(".deleting-{}-{}", original_name, uuid::Uuid::new_v4());
    let temp_path = parent.join(&temp_name);

    match std::fs::rename(wg_dir, &temp_path) {
        Ok(()) => {
            if let Err(e) = remove_dir_all(&temp_path) {
                log::warn!(
                    "[entity_creation] Renamed workgroup '{}' to '{}' but remove_dir_all \
                     failed: {}. \
                     User-visible WG is gone; orphan remains on disk.",
                    wg_dir.display(),
                    temp_path.display(),
                    e
                );
                return WgDeleteOutcome::Partial {
                    orphan_path: temp_path,
                    error: e,
                };
            }
            WgDeleteOutcome::Deleted
        }
        Err(e) => {
            if is_rename_blocked_by_handle(&e) {
                WgDeleteOutcome::Blocked(e)
            } else {
                WgDeleteOutcome::Other(e)
            }
        }
    }
}
