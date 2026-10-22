use super::{
    build_direct_matrix_context, build_replica_context_with_activation,
    build_root_agent_context_with_auto, ensure_session_context_with_config,
    get_default_coordinator_template, is_canonical_agent_matrix_dir, is_replica_agent_dir,
    read_context_local_override, read_or_create_context_recorded, root_context_candidate,
    strip_legacy_self_maintenance, ManagedContextTarget, COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
    MANAGED_CONTEXT_FILENAMES, PTY_INPUT_COORDINATOR_CONTEXT, SELF_MAINTENANCE_AUTO_SECTION,
};
use crate::config::instance_artifacts::CONTEXT_CACHE_DIR_NAME;
use std::path::Path;
use std::time::{Duration, SystemTime};

/// Resolve the final session context content for an agent directory.
/// Prefers replica config.json context[] and falls back to the per-agent default context.
#[cfg(test)]
pub(crate) fn resolve_session_context_content(
    cwd: &str,
    is_coordinator: bool,
    auto_self_clear: bool,
    repo_mounts: Option<&crate::pty::container_repos::RepoMountResolution>,
) -> Result<Option<String>, String> {
    resolve_session_context_content_with_activation(
        cwd,
        is_coordinator,
        auto_self_clear,
        repo_mounts,
        None,
    )
}

pub(crate) fn resolve_session_context_content_with_activation(
    cwd: &str,
    is_coordinator: bool,
    auto_self_clear: bool,
    repo_mounts: Option<&crate::pty::container_repos::RepoMountResolution>,
    activation: Option<&crate::config::seed_manifest::ManifestActivationToken>,
) -> Result<Option<String>, String> {
    let is_root = root_context_candidate(cwd);
    let context_path = if is_root {
        // Identity validation precedes config/template reads, including aliases
        // whose raw name would otherwise select the replica branch.
        build_root_agent_context_with_auto(cwd, repo_mounts, auto_self_clear)?
    } else if is_replica_agent_dir(cwd) {
        match build_replica_context_with_activation(cwd, repo_mounts, activation) {
            Ok(Some(combined_path)) => {
                log::info!(
                    "Using replica combined context for agent session: {}",
                    combined_path
                );
                combined_path
            }
            Ok(None) => ensure_session_context_with_config(cwd, None, repo_mounts, activation)?,
            Err(e) => return Err(e),
        }
    } else if is_canonical_agent_matrix_dir(cwd) {
        build_direct_matrix_context(cwd, repo_mounts, activation)?
    } else {
        return Ok(None);
    };

    let mut content = std::fs::read_to_string(&context_path).map_err(|e| {
        format!(
            "Failed to read resolved session context {}: {}",
            context_path, e
        )
    })?;

    // #979: never append coordinator prose to Root, and never let a Root cwd reach
    // `read_or_create_context_template`. That call resolves its directory through
    // `resolve_ac_root_context_dir`, so with a `.ac`-ancestor config dir (the dev
    // and workgroup layouts) a Root would CREATE and SYNC the *project's*
    // `Context.coordinator.md`. `is_coordinator` is false for Root today
    // (`is_coordinator_for_cwd` derives an FQN from the cwd and checks team
    // membership, teams.rs:802-805); this guard makes a wrong caller flag harmless.
    if is_coordinator && !is_root {
        let coordinator_body = read_or_create_context_recorded(
            cwd,
            COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
            get_default_coordinator_template(),
            activation,
        )?
        .unwrap_or_else(|| get_default_coordinator_template().to_string());
        // #1737 (D1): consumer-side substitution. The base pipeline above has already
        // synced, migrated and created the base file exactly as it does today; only
        // the bytes handed to the session change.
        let coordinator_body =
            read_context_local_override(cwd, COORDINATOR_CONTEXT_TEMPLATE_FILENAME)
                .unwrap_or(coordinator_body);
        if !coordinator_body.trim().is_empty() {
            content.push_str("\n\n---\n\n# Orchestrator Context\n\n");
            content.push_str(&coordinator_body);
        }
        if crate::config::teams::verify_pty_input_coordinator_root(Path::new(cwd)).is_ok() {
            content.push_str("\n\n");
            content.push_str(PTY_INPUT_COORDINATOR_CONTEXT);
        }
    }

    // #640 Single-source the self-maintenance directive. Strip any legacy block
    // (an existing workgroup may have one frozen in its persisted coordinator
    // template on disk), then append the canonical gated directive when ON.
    // Strip ALWAYS so an OFF setting truly removes the old always-on block.
    if !is_root {
        content = strip_legacy_self_maintenance(&content);
        if auto_self_clear {
            content.push_str(SELF_MAINTENANCE_AUTO_SECTION);
        }
    }

    Ok(Some(content))
}

/// Delete stale agent-specific context files from a replica cwd and rewrite the
/// current resolved context into the single configured filename required by the
/// coding agent being launched. `extra_managed_filenames` are additional names
/// to clean up (the union of every configured agent's resolved filename), so
/// switching a replica between agents removes the previous file before writing
/// the new one. Returns `Ok(None)` when `cwd` is not an AC-managed agent root.
///
/// G1 (symlink/junction-safe): cleanup uses `symlink_metadata` (not `exists`)
/// so a link/reparse point is detected and the link ENTRY removed, never its
/// target; and the writer refuses to write THROUGH a surviving link. #529
/// broadens the cleanup/write set from 3 fixed names to N configured + custom
/// names, which is why this writer is hardened against a stray link at one of
/// those names (an arbitrary-overwrite primitive, and a junction would brick
/// every launch via remove_file -> Err -> rollback).
pub fn materialize_agent_context_file_with_filename(
    cwd: &str,
    target_filename: &str,
    extra_managed_filenames: &[String],
    is_coordinator: bool,
    auto_self_clear: bool,
    repo_mounts: Option<&crate::pty::container_repos::RepoMountResolution>,
) -> Result<Option<String>, String> {
    materialize_agent_context_file_with_filename_activated(
        cwd,
        target_filename,
        extra_managed_filenames,
        is_coordinator,
        auto_self_clear,
        repo_mounts,
        None,
    )
}

/// #1065 Stage F: same as [`materialize_agent_context_file_with_filename`] but
/// threads a production activation token so the session-spawn context read/sync
/// and self-heal record their publications under the project gate. The session
/// create chokepoint passes `Some(..)` from inside its blocking worker; every
/// other caller passes `None` (unchanged, non-emitting).
pub(crate) fn materialize_agent_context_file_with_filename_activated(
    cwd: &str,
    target_filename: &str,
    extra_managed_filenames: &[String],
    is_coordinator: bool,
    auto_self_clear: bool,
    repo_mounts: Option<&crate::pty::container_repos::RepoMountResolution>,
    activation: Option<&crate::config::seed_manifest::ManifestActivationToken>,
) -> Result<Option<String>, String> {
    let content = match resolve_session_context_content_with_activation(
        cwd,
        is_coordinator,
        auto_self_clear,
        repo_mounts,
        activation,
    )? {
        Some(content) => content,
        None => return Ok(None),
    };

    materialize_resolved_context_file(cwd, target_filename, extra_managed_filenames, &content)
}

pub(crate) fn materialize_resolved_context_file(
    cwd: &str,
    target_filename: &str,
    extra_managed_filenames: &[String],
    content: &str,
) -> Result<Option<String>, String> {
    // String-level guard (path escape): never write outside the root, even if a
    // direct `pub` caller bypassed settings validation. The on-disk link checks
    // below guard against state no string validation can detect.
    if target_filename.contains('/') || target_filename.contains('\\') {
        return Err(format!(
            "Refusing to write context to a path with separators: {}",
            target_filename
        ));
    }

    let cwd_path = std::path::Path::new(cwd);

    // Cleanup set: built-ins ∪ all configured filenames ∪ target, deduped so the
    // target (usually already present via managed_instructions_filenames) is not
    // stat'd twice.
    let mut cleanup: Vec<&str> = MANAGED_CONTEXT_FILENAMES.to_vec();
    for f in extra_managed_filenames {
        if !cleanup.contains(&f.as_str()) {
            cleanup.push(f.as_str());
        }
    }
    if !cleanup.contains(&target_filename) {
        cleanup.push(target_filename);
    }

    for filename in cleanup {
        let path = cwd_path.join(filename);
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(format!(
                    "Failed to stat context file {}: {}",
                    path.display(),
                    e
                ))
            }
        };
        if is_link_or_reparse(&meta) {
            // Remove the link/junction ENTRY itself (never its target): try file
            // (file symlink) first, then dir (dir symlink / junction).
            std::fs::remove_file(&path)
                .or_else(|_| std::fs::remove_dir(&path))
                .map_err(|e| {
                    format!(
                        "Failed to remove link at context path {}: {}",
                        path.display(),
                        e
                    )
                })?;
        } else if meta.is_dir() {
            // A real directory named like a managed file is a genuine problem;
            // refuse rather than delete a directory tree.
            return Err(format!(
                "Refusing to launch: managed context path {} is a real directory, not a file",
                path.display()
            ));
        } else {
            std::fs::remove_file(&path).map_err(|e| {
                format!(
                    "Failed to remove stale context file {}: {}",
                    path.display(),
                    e
                )
            })?;
        }
    }

    let target_path = cwd_path.join(target_filename);
    // Defensive: the target is always in `cleanup`, so a surviving link here is
    // unexpected; never write THROUGH one (belt-and-suspenders for direct callers).
    if let Ok(meta) = std::fs::symlink_metadata(&target_path) {
        if is_link_or_reparse(&meta) {
            return Err(format!(
                "Refusing to write context through a link at {}",
                target_path.display()
            ));
        }
    }
    std::fs::write(&target_path, content)
        .map_err(|e| format!("Failed to write {}: {}", target_path.display(), e))?;

    log::info!(
        "Materialized managed agent context file in {}: {}",
        cwd,
        target_path.display()
    );

    Ok(Some(target_path.to_string_lossy().to_string()))
}

/// True iff `meta` is a symlink or (Windows) any reparse point (junction).
/// Local copy of the gate at `cli/agency_templates.rs:867-880`; kept private to
/// this module to avoid widening another module's API (the codebase already
/// inlines this same check in `coding_agent_profiles.rs`;
/// do NOT refactor those into one helper here).
pub(crate) fn is_link_or_reparse(meta: &std::fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

/// Enum-based entry point preserved as a thin wrapper so the existing test call
/// sites (`materialize_agent_context_file(cwd, ManagedContextTarget::X, bool)`)
/// keep compiling unchanged.
pub fn materialize_agent_context_file(
    cwd: &str,
    target: ManagedContextTarget,
    is_coordinator: bool,
) -> Result<Option<String>, String> {
    // #640 test-only wrapper: the sole production caller is
    // materialize_agent_context_file_with_filename in session.rs, which resolves
    // and passes the real auto_self_clear flag. Pass false here so no production
    // path loses the gated directive.
    materialize_agent_context_file_with_filename(
        cwd,
        target.filename(),
        &[],
        is_coordinator,
        false,
        None,
    )
}

// ── Context-cache GC (#621) ───────────────────────────────────────────────

/// (#621) Retention window for generated context-cache files. A live agent
/// re-writes its cache on every launch (fresh mtime), so anything untouched this
/// long is an orphan (removed workgroup / renamed replica dir). 30 days is
/// generous enough never to drop the cache of an agent the user simply has not
/// launched recently, while still capping the directory. Internal GC knob,
/// intentionally a const (not a user setting); promote to settings later if needed.
pub(crate) const CONTEXT_CACHE_RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// (#621) Startup entry point: GC the context-cache dir with the default
/// retention. No-op (logs) when no config dir resolves. Never returns an error;
/// housekeeping must not break startup. This pass is BOTH the orphan cleanup for
/// removed workgroups (their cache ages out) AND the cap for the unbounded-growth
/// secondary finding.
pub fn sweep_context_cache_at_startup() {
    let Some(context_dir) = super::super::config_dir().map(|d| d.join(CONTEXT_CACHE_DIR_NAME))
    else {
        log::warn!("[context-cache] no config dir; skipping startup sweep");
        return;
    };
    let removed = sweep_context_cache_dir(&context_dir, SystemTime::now(), CONTEXT_CACHE_RETENTION);
    if removed > 0 {
        log::info!(
            "[context-cache] startup sweep removed {} stale cache file(s)",
            removed
        );
    }
}

/// (#621) Testable core: unlink every generated context file in `context_dir`
/// whose mtime is older than `retention` relative to `now`. Returns the count
/// removed. Only touches the three known generated prefixes ending in `.md`, so an
/// unrelated file dropped in the dir is never deleted. A file whose mtime cannot be
/// read (or is in the future) is KEPT (conservative). A missing dir is a no-op.
pub fn sweep_context_cache_dir(context_dir: &Path, now: SystemTime, retention: Duration) -> usize {
    let entries = match std::fs::read_dir(context_dir) {
        Ok(e) => e,
        Err(_) => return 0, // first run / missing dir
    };
    let mut removed = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !is_generated_context_filename(name) {
            continue;
        }
        // Keep on any uncertainty (no metadata / no mtime / clock skew).
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(mtime) = meta.modified() else { continue };
        let Ok(age) = now.duration_since(mtime) else {
            continue;
        }; // mtime in the future -> keep
        if age > retention {
            match std::fs::remove_file(&path) {
                Ok(()) => removed += 1,
                Err(e) => {
                    log::warn!(
                        "[context-cache] failed to remove stale {}: {}",
                        path.display(),
                        e
                    )
                }
            }
        }
    }
    removed
}

/// (#621) True for the three generated context-cache filename shapes
/// (`ac-context-*.md`, `replica-context-*.md`, `matrix-context-*.md`).
pub(crate) fn is_generated_context_filename(name: &str) -> bool {
    name.ends_with(".md")
        && (name.starts_with("ac-context-")
            || name.starts_with("replica-context-")
            || name.starts_with("matrix-context-"))
}

/// Simple deterministic hash for a string (for temp file naming).
pub(crate) fn simple_hash(s: &str) -> u64 {
    let mut hash: u64 = 5381;
    for byte in s.bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(byte as u64);
    }
    hash
}
