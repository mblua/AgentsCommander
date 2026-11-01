//! #930 - resolve, copy in, and tear down a coding agent's host credential file
//! for container runtime. The per-agent *descriptor* lives on
//! `CodingAgentProfile` (session/profile.rs); this module turns a descriptor +
//! the session's host mount root into a concrete copy job and executes it.
//! Credential contents are NEVER logged (mirror pty/credentials.rs rule).

use std::path::{Path, PathBuf};

use crate::filesystem::{directories, documents};

use crate::session::profile::{ContainerCredentialSource, ContainerFirstRunState};

/// A resolved copy job: absolute host source file and absolute replica dest file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerCredentialPlan {
    pub source: PathBuf,
    pub dest: PathBuf,
    /// #930 - first-run state to stamp in the dest dir after the copy (None =
    /// none for this agent). Carried on the plan because the container backend
    /// sees only the plan, never the agent profile.
    pub first_run: Option<ContainerFirstRunState>,
}

/// F2 - true if `path` exists AND is a symlink or Windows junction/reparse point.
/// `symlink_metadata` does NOT traverse the leaf. NotFound => false (safe to
/// create/copy); any other error => true (undeterminable = skip-worthy). Mirrors
/// the private, entry-based `filesystem::config_seed::is_symlink_or_reparse`,
/// using the shared `path_identity` metadata predicate.
fn is_reparse_path(path: &Path) -> bool {
    documents::container_credential_is_reparse(path)
}

/// Resolve a copy job for `source` given the container bind-mount root
/// (`host_root`, canonical). Returns None (and logs at info) when the host
/// credential file does not exist, so a missing host login is a clean skip.
pub fn resolve_plan(
    source: &ContainerCredentialSource,
    host_root: &str,
) -> Option<ContainerCredentialPlan> {
    // Host config dir: an absolute env override wins, else ~/<host_dir>.
    let host_dir: PathBuf = source
        .host_dir_env
        .and_then(|k| crate::config::environment::container_credential_override(k).ok())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        // F6 - only honor an ABSOLUTE override; a relative/marker value cleanly
        // falls back to ~/<host_dir> instead of resolving against AC's CWD.
        .filter(|v| Path::new(v).is_absolute())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(source.host_dir)))?;

    let src = host_dir.join(source.file);
    if !crate::filesystem::directories::credential_source_is_file(&src) {
        log::info!(
            "[container-cred] no host credential at {}; container will start without host login",
            src.display()
        );
        return None;
    }
    let dest = Path::new(host_root)
        .join(source.container_dir)
        .join(source.file);
    Some(ContainerCredentialPlan {
        source: src,
        dest,
        first_run: source.first_run,
    })
}

/// Outcome of a `copy_in` that did not error. An F2 skip is NOT a failure (the
/// spawn continues), but it writes NO token, so the caller MUST distinguish it
/// from a real copy: stamping first-run state after a skip would suppress the
/// agent's login wizard on a container that has no credential at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyOutcome {
    /// The host credential was written to `plan.dest`.
    Copied,
    /// The dest dir or the dest leaf is a symlink/junction, so F2 refused to
    /// write through it and nothing was copied.
    SkippedReparse,
}

/// Copy the host credential into the replica dir. Best-effort: refuses to write
/// through a container-planted symlink/junction (F2), creates the dest parent
/// dir, overwrites any existing dest with the current host token, and (on Unix)
/// tightens perms to 0o600. Errors are returned for the caller to log; the caller
/// does NOT abort the spawn on failure. `Ok(CopyOutcome::SkippedReparse)` means
/// no token was written, so it must never be treated as a successful copy.
pub fn copy_in(plan: &ContainerCredentialPlan) -> std::io::Result<CopyOutcome> {
    // F2 - never write through a container-planted symlink/junction, on the
    // container config dir or on the credential leaf.
    if let Some(parent) = plan.dest.parent() {
        if is_reparse_path(parent) {
            log::warn!(
                "[container-cred] dest dir {} is a symlink/reparse point; skipping copy-in",
                parent.display()
            );
            return Ok(CopyOutcome::SkippedReparse);
        }
    }
    if is_reparse_path(&plan.dest) {
        log::warn!(
            "[container-cred] dest {} is a symlink/reparse point; skipping copy-in",
            plan.dest.display()
        );
        return Ok(CopyOutcome::SkippedReparse);
    }
    if let Some(parent) = plan.dest.parent() {
        directories::ensure_container_credential_directory(parent)?;
    }
    documents::copy_container_credential(&plan.source, &plan.dest)?;
    #[cfg(unix)]
    {
        // F4 - do not silently swallow a perms failure; a token left broadly
        // readable must at least be observable in the log.
        if let Err(e) = documents::set_container_credential_private(&plan.dest) {
            log::warn!(
                "[container-cred] failed to set 0o600 on {}: {}",
                plan.dest.display(),
                e
            );
        }
    }
    log::info!(
        "[container-cred] copied host credential into {}",
        plan.dest.display()
    );
    Ok(CopyOutcome::Copied)
}

/// Delete a previously copied credential file on teardown. Best-effort and
/// idempotent: a missing file is not an error. Refuses to delete THROUGH a
/// symlink/junction (F2): AC never creates a link, so a reparse-point leaf is
/// attacker-planted and is left untouched rather than followed.
pub fn remove_copied(dest: &Path) {
    if is_reparse_path(dest) {
        log::warn!(
            "[container-cred] {} is a symlink/reparse point; refusing to delete through it",
            dest.display()
        );
        return;
    }
    match documents::remove_container_credential_artifact(dest) {
        Ok(()) => log::info!(
            "[container-cred] removed copied credential {}",
            dest.display()
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => log::warn!(
            "[container-cred] failed to remove copied credential {}: {}",
            dest.display(),
            e
        ),
    }
}

/// Set `key` to `true` in `map`. Returns true when that actually changed
/// something (so the caller only rewrites the file when needed).
fn set_true(map: &mut serde_json::Map<String, serde_json::Value>, key: &str) -> bool {
    if map.get(key) == Some(&serde_json::Value::Bool(true)) {
        return false;
    }
    map.insert(key.to_string(), serde_json::Value::Bool(true));
    true
}

/// #930 - stamp the container's first-run state next to the credential we just
/// copied, so the agent's interactive TUI actually uses it. Claude Code gates
/// its onboarding wizard on `hasCompletedOnboarding` in
/// `$CLAUDE_CONFIG_DIR/.claude.json` and the folder-trust dialog on
/// `projects[<cwd>].hasTrustDialogAccepted`, and checks NEITHER against the
/// credential: without these flags a valid copied token still lands on "Select
/// login method" (reproduced in a real container).
///
/// Best-effort, non-destructive, and never aborts the spawn:
/// - merges into an existing JSON object, preserving every other key;
/// - creates the file when it is absent or empty;
/// - SKIPS with a warn when the file is unparseable or is not a JSON object,
///   so user data is never clobbered;
/// - refuses to write through a symlink/junction (F2), like `copy_in`;
/// - writes via a temp file + rename, so a crash cannot truncate the config.
///
/// The config file can hold user identity fields, so contents are never logged.
pub fn ensure_first_run_state(plan: &ContainerCredentialPlan, container_workdir: &str) {
    let Some(state) = plan.first_run else {
        return;
    };
    let Some(dir) = plan.dest.parent() else {
        return;
    };
    let path = dir.join(state.file);

    // F2 - never write through a container-planted symlink/junction, on the
    // config dir or on the config file itself.
    if is_reparse_path(dir) {
        log::warn!(
            "[container-cred] config dir {} is a symlink/reparse point; skipping first-run state",
            dir.display()
        );
        return;
    }
    if is_reparse_path(&path) {
        log::warn!(
            "[container-cred] {} is a symlink/reparse point; skipping first-run state",
            path.display()
        );
        return;
    }
    if let Err(e) = directories::ensure_container_credential_directory(dir) {
        log::warn!(
            "[container-cred] failed to create {}: {}; skipping first-run state",
            dir.display(),
            e
        );
        return;
    }

    let Some(mut root) = read_first_run_state(&path) else {
        return;
    };

    let mut changed = set_true(&mut root, state.onboarding_flag);

    changed |= apply_project_flags(&mut root, &state, container_workdir, &path);

    if !changed {
        log::debug!(
            "[container-cred] first-run state already present in {}",
            path.display()
        );
        return;
    }

    let text = match serde_json::to_string_pretty(&serde_json::Value::Object(root)) {
        Ok(text) => text,
        Err(e) => {
            log::warn!(
                "[container-cred] failed to serialize first-run state for {}: {}",
                path.display(),
                e
            );
            return;
        }
    };
    // Temp + rename: a crash mid-write must never truncate an existing config.
    let tmp = dir.join(format!("{}.ac-{}.tmp", state.file, uuid::Uuid::new_v4()));
    if let Err(e) = documents::write_container_first_run_stage(&tmp, text.as_bytes()) {
        log::warn!(
            "[container-cred] failed to write first-run state to {}: {}",
            tmp.display(),
            e
        );
        let _ = documents::remove_container_credential_artifact(&tmp);
        return;
    }
    if let Err(e) = documents::rename_container_first_run_stage(&tmp, &path) {
        log::warn!(
            "[container-cred] failed to install first-run state at {}: {}",
            path.display(),
            e
        );
        let _ = documents::remove_container_credential_artifact(&tmp);
        return;
    }
    log::info!(
        "[container-cred] ensured container first-run state in {} ({} + trust for {})",
        path.display(),
        state.onboarding_flag,
        container_workdir
    );
}

fn apply_project_flags(
    root: &mut serde_json::Map<String, serde_json::Value>,
    state: &ContainerFirstRunState,
    container_workdir: &str,
    path: &Path,
) -> bool {
    let mut changed = false;
    // `projects[<container_workdir>]` = { "hasTrustDialogAccepted": true, ... }.
    // The mount root IS the container workdir, and AC already trusts it by
    // bind-mounting it, so pre-accepting the folder-trust dialog adds no new
    // exposure. A non-object at either level is left untouched (warn + skip).
    if !state.project_flags.is_empty() {
        let projects = root
            .entry(state.projects_key)
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        match projects.as_object_mut() {
            Some(projects) => {
                let entry = projects
                    .entry(container_workdir)
                    .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
                match entry.as_object_mut() {
                    Some(entry) => {
                        for flag in state.project_flags {
                            changed |= set_true(entry, flag);
                        }
                    }
                    None => log::warn!(
                        concat!(
                            "[container-cred] {} entry for '{}' is not a JSON object; skipping ",
                            "the trust entry",
                        ),
                        path.display(),
                        container_workdir
                    ),
                }
            }
            None => log::warn!(
                "[container-cred] {} field '{}' is not a JSON object; skipping the trust entry",
                path.display(),
                state.projects_key
            ),
        }
    }
    changed
}

fn read_first_run_state(path: &Path) -> Option<serde_json::Map<String, serde_json::Value>> {
    let root = match documents::read_container_first_run_state(path) {
        // An empty file carries no user data, so treat it like a missing one.
        Ok(text) if text.trim().is_empty() => serde_json::Map::new(),
        Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(serde_json::Value::Object(map)) => map,
            Ok(_) => {
                log::warn!(
                    "[container-cred] {} is not a JSON object; skipping first-run state",
                    path.display()
                );
                return None;
            }
            Err(e) => {
                log::warn!(
                    "[container-cred] {} is not valid JSON ({}); skipping first-run state",
                    path.display(),
                    e
                );
                return None;
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::Map::new(),
        Err(e) => {
            log::warn!(
                "[container-cred] failed to read {}: {}; skipping first-run state",
                path.display(),
                e
            );
            return None;
        }
    };
    Some(root)
}

#[cfg(test)]
mod tests;
