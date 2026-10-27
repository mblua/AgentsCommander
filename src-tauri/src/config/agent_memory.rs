//! #1172 - rotate an origin Agent Matrix's `memory/` to a timestamped archive under
//! `memory-archive/` at session spawn, so a fresh agent starts with a clean write
//! target and the
//! previous session's memory is preserved rather than deleted.

use std::path::{Path, PathBuf};

use crate::filesystem::agent_files;

/// Mirrors the `"memory"` entry of
/// `crate::commands::entity_creation::AGENT_MATRIX_DIRS` (`entity_creation.rs:128`).
const MEMORY_DIR_NAME: &str = "memory";

/// Deliberate independent twin of `phone::mailbox::ARCHIVE_TIMESTAMP_FORMAT`
/// (`mailbox.rs:848`), per D3. That constant is part of the `self-clear/`
/// naming contract the #749 resume prompt and the CLI help expose to agents.
/// The two must stay free to diverge, so the literal is duplicated instead of
/// exported from `phone` into `config`.
const MEMORY_ROTATION_TIMESTAMP_FORMAT: &str = "%Y%m%d_%H%M%S";

/// Two jobs, both load-bearing: it renders a path for logs so operators never
/// read `\\?\C:\...` in `app.log`, and it performs the `Path` to `String`
/// conversion that `is_canonical_agent_matrix_dir` requires. Same private
/// one-liner as `replica_identity.rs:23-25` and `root_agent.rs:1924`. The
/// conversion goes through `to_string_lossy`, so it is lossy in principle; that
/// is fail-safe here, because a degraded path fails `canonicalize` and the
/// caller rotates nothing.
fn display_path(path: &Path) -> String {
    crate::path_utils::path_to_string_without_windows_verbatim_prefix(path)
}

/// Create `path` as an empty directory, treating "someone else already made it"
/// as success. `AlreadyExists` means a concurrent session won the race to the
/// same desired end state, so warning about it would report a failure where
/// nothing failed (D9 sub-decision).
fn ensure_dir(path: &Path) -> std::io::Result<()> {
    match agent_files::create_memory_directory(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e),
    }
}

/// Launch-path entry point (D5). Returns nothing and never propagates an error:
/// every internal failure path warns and returns, so a rotation cannot abort a
/// session launch (acceptance criterion 3).
///
/// The FRESH-versus-resume decision is the caller's, not this function's. The
/// call site in `commands/session.rs` gates on `skip_auto_resume`; see D5 for
/// why that flag is the signal and what each entry point passes.
pub(crate) fn rotate_origin_memory_at_spawn(agent_root: &str) {
    let Some(matrix_root) = resolve_rotatable_matrix_root(agent_root) else {
        return;
    };
    let timestamp = chrono::Local::now()
        .format(MEMORY_ROTATION_TIMESTAMP_FORMAT)
        .to_string();
    match rotate_memory_dir(&matrix_root, &timestamp) {
        Ok(Some(dst)) => log::info!(
            "[memory] #1172: rotated {} -> {} at fresh session spawn",
            // 11.3.1: the source is a pure function of `matrix_root`.
            display_path(&matrix_root.join(MEMORY_DIR_NAME)),
            display_path(&dst)
        ),
        // Absent `memory/` (repaired in place, D9), or present and empty (D2).
        // Acceptance criterion 2 requires nothing at info level or above here.
        Ok(None) => {}
        Err(e) => log::warn!(
            "[memory] #1172: memory rotation failed for {} \
            (non-fatal; the session still launches): {}",
            display_path(&matrix_root),
            e
        ),
    }
}

/// Resolve the origin Agent Matrix whose `memory/` this session rotates, or
/// `None` when nothing may be rotated.
///
/// D7: the replica identity is read with the read-only reader.
/// `resolve_replica_matrix_root` is not used because it rewrites `config.json`
/// on every call through `local_config_io::update_config_json_object`, and the
/// context path has already read and repaired that file by the time this runs.
///
/// D6: `ac-root-agent` cannot satisfy `is_canonical_agent_matrix_dir`, so the
/// Root Agent is excluded by the filter itself.
fn resolve_rotatable_matrix_root(agent_root: &str) -> Option<PathBuf> {
    let candidate = if crate::config::session_context::is_replica_agent_dir(agent_root) {
        let (_config, identity) =
            crate::config::replica_identity::read_wg_replica_config_read_only(Path::new(
                agent_root,
            ))
            .ok()?;
        display_path(&identity.matrix_dir)
    } else {
        agent_root.to_string()
    };
    // Re-validate whatever the identity resolved to. A replica pointing at
    // something that is not a canonical matrix rotates nothing.
    if !crate::config::session_context::is_canonical_agent_matrix_dir(&candidate) {
        return None;
    }
    // Canonical (verbatim on Windows) is what the filesystem calls below use;
    // logging normalizes it (11.3.2).
    agent_files::canonical_memory_root(Path::new(&candidate)).ok()
}

/// The deterministic core, modeled on `phone::mailbox::archive_root_md`
/// (`mailbox.rs:1595-1617`). The timestamp is injected by the caller, so this is
/// testable without mocking the clock.
///
/// `Ok(None)` means no rotation happened: `memory/` was absent (and has been
/// recreated empty, D9) or was present and empty (D2).
/// `Ok(Some(dst))` is the rotated directory.
fn rotate_memory_dir(matrix_root: &Path, timestamp: &str) -> std::io::Result<Option<PathBuf>> {
    let src = matrix_root.join(MEMORY_DIR_NAME);
    let Ok(metadata) = agent_files::memory_directory_metadata(&src) else {
        // D9: absent is repaired, not merely skipped. Without this, a failed
        // recreate below would leave the matrix permanently without a write
        // target, because this branch is what every later launch takes.
        // Best-effort: a failure here warns and still reports "no rotation".
        if let Err(e) = ensure_dir(&src) {
            log::warn!(
                "[memory] #1172: could not restore a missing memory/ at {} (non-fatal): {}",
                display_path(&src),
                e
            );
        }
        return Ok(None);
    };
    // 11.3.4: `is_symlink()` alone is FALSE for a Windows junction, which is
    // exactly the case this rejects. Renaming a junction would move the link
    // entry and the recreate below would replace the user's link with a real
    // directory. Same call shape as `mailbox.rs:89-91`. This reaches for the
    // `pub(crate)` helper in `entity_creation` rather than the private copy in
    // `session_context.rs:2322`, whose doc forbids refactoring the inlined
    // checks into a shared helper *there*; reusing an already-shared
    // `pub(crate)` one from another module is what `mailbox.rs:90` does.
    if !metadata.is_dir()
        || crate::commands::entity_creation::metadata_is_link_or_reparse(&metadata)
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "memory is not a real directory",
        ));
    }
    // D2: an empty `memory/` is not rotated.
    if agent_files::memory_directory_entries(&src)?
        .next()
        .is_none()
    {
        return Ok(None);
    }
    let parent = matrix_root.join("memory-archive");
    ensure_dir(&parent)?;
    // Revalidate even after AlreadyExists: the parent must never be a link.
    // This check then rename remains best effort on the trusted host; a
    // concurrent junction swap is not prevented atomically.
    let metadata = agent_files::memory_directory_metadata(&parent)?;
    if !metadata.is_dir()
        || crate::commands::entity_creation::metadata_is_link_or_reparse(&metadata)
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "memory archive parent is not a real directory",
        ));
    }
    let dst = parent.join(format!("{}_{}", MEMORY_DIR_NAME, timestamp));
    // Refuse to clobber (`mailbox.rs:1606-1613`). `symlink_metadata` rather
    // than `exists()`, so a dangling link at the destination also blocks. This
    // is a best-effort fast path, NOT an atomic guarantee; see section 7 for
    // what actually makes data loss impossible in the residual race.
    if agent_files::memory_directory_metadata(&dst).is_ok() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "memory rotation target already exists",
        ));
    }
    // No retry loop, per `mailbox.rs:1615` and its rationale at `:1590-1593`.
    agent_files::rename_memory_directory(&src, &dst)?;
    // Restore the write target so the layout contract of `entity_creation.rs:128`
    // stays complete. The rotation itself already succeeded, so this warns
    // rather than turning into `Err`; D9's self-repair above is what keeps a
    // failure here from being permanent.
    if let Err(e) = ensure_dir(&src) {
        log::warn!(
            "[memory] #1172: rotated {} but failed to recreate an empty memory/ at {} \
            (the next fresh launch restores it): {}",
            display_path(&dst),
            display_path(&src),
            e
        );
    }
    Ok(Some(dst))
}

#[cfg(test)]
mod tests;
