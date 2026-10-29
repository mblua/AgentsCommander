//! Disk reads for user-owned documents and loop event path resolution.

use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub(crate) mod ac_discovery;
pub(crate) mod activity_log;
pub(crate) mod agency_templates;
pub(crate) mod agent_command;
pub(crate) mod agent_files;
pub(crate) mod agent_settings;
pub(crate) mod attachments;
pub(crate) mod capture_state;
pub(crate) mod catalog;
pub(crate) mod co_managed_config;
pub(crate) mod co_managed_queue;
pub(crate) mod coding_agent_profiles;
pub(crate) mod coding_agent_requests;
pub(crate) mod config_locations;
pub(crate) mod config_seed;
pub(crate) mod context_skills;
pub(crate) mod context_templates;
pub(crate) mod coordinator_clocks;
pub(crate) mod daemon_pid;
pub(crate) mod debug_logs;
pub(crate) mod directories;
pub(crate) mod documents;
pub(crate) mod injected_documents;
pub(crate) mod instance_ignore;
pub(crate) mod loop_files;
pub(crate) mod naming_journal;
pub(crate) mod phone_mailbox;
pub(crate) mod phone_messaging;
pub(crate) mod processes;
pub(crate) mod profile_targets;
pub(crate) mod projects;
pub(crate) mod remote_activity;
pub(crate) mod repositories;
pub(crate) mod room_diagnostics;
pub(crate) mod seed_manifest;
pub(crate) mod self_clear_files;
pub(crate) mod session_context;
pub(crate) mod spec_board;
pub(crate) mod tasks;
pub(crate) mod telegram_logs;
pub(crate) mod terminal_snapshots;
pub(crate) mod transcripts;

/// Read the capture catalog without changing the platform's file semantics.
pub(crate) fn read_capture_catalog(path: &Path) -> std::io::Result<String> {
    std::fs::read_to_string(path)
}

/// Preserve canonical loop event paths, falling back to the supplied spelling.
pub(crate) fn canonical_loop_event_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.into())
}

/// Read persisted session text with unchanged platform errors.
pub(crate) fn read_session_container_text(path: &Path) -> std::io::Result<String> {
    std::fs::read_to_string(path)
}

/// Ensure the snapshot directory with unchanged platform errors.
pub(crate) fn ensure_session_container_directory(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
}

/// Write the operation's unique stage using the original filesystem semantics.
pub(crate) fn write_session_container_stage(path: &Path, bytes: String) -> std::io::Result<()> {
    use std::io::Write;

    let mut file = std::fs::File::create(path)?;
    file.write_all(bytes.as_bytes())?;
    #[cfg(all(test, windows))]
    let file = tests::issue_3278_sync_handle(path, file)?;
    file.sync_all()
}

/// Replace the snapshot using the existing native rename operation.
pub(crate) fn replace_session_container_file(tmp: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::rename(tmp, dst)?;
    #[cfg(unix)]
    if let Err(error) = sync_session_container_parent(dst) {
        log::warn!(
            "[sessions] published {} but parent directory sync failed: {}",
            dst.display(),
            error
        );
    }
    // Windows has no portable directory sync through File::open.
    Ok(())
}

#[cfg(unix)]
fn sync_session_container_parent(dst: &Path) -> std::io::Result<()> {
    let parent = dst.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "snapshot has no parent")
    })?;
    std::fs::File::open(parent)?.sync_all()
}

/// Remove only the supplied operation stage.
pub(crate) fn remove_session_container_stage(path: &Path) -> std::io::Result<()> {
    std::fs::remove_file(path)
}

/// Read snapshot bytes for typed publication readback.
pub(crate) fn read_session_container_bytes(path: &Path) -> std::io::Result<Vec<u8>> {
    std::fs::read(path)
}

#[cfg(test)]
pub(crate) fn corrupt_session_container_for_test(path: &Path) -> std::io::Result<()> {
    std::fs::write(path, b"{invalid fixture publication")
}
#[cfg(test)]
mod tests;

pub(crate) fn publish_cli_pty_request(
    outbox: &Path,
    temp: PathBuf,
    final_path: PathBuf,
    bytes: &[u8],
    outbox_identity: crate::path_identity::VerifiedPathIdentity,
    max_bytes: usize,
) -> Result<PathBuf, &'static str> {
    match std::fs::symlink_metadata(&final_path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        _ => return Err("publish_ambiguous"),
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(0x0002_0000);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        options
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let mut file = options.open(&temp).map_err(|_| "publish_failed")?;
    if file.write_all(bytes).is_err() || file.flush().is_err() || file.sync_all().is_err() {
        drop(file);
        if let Err(error) = std::fs::remove_file(&temp) {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err("publish_ambiguous");
            }
        }
        return Err("publish_failed");
    }
    let temp_identity = match crate::path_identity::verify_opened_regular_file(&temp, &file, false)
    {
        Ok(identity) => identity,
        Err(_) => {
            drop(file);
            if let Err(error) = std::fs::remove_file(&temp) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    return Err("publish_ambiguous");
                }
            }
            return Err("publish_failed");
        }
    };
    drop(file);

    let prepublication_safe = (|| {
        let current_outbox = crate::path_identity::verify_directory(outbox)?;
        let current_temp = crate::path_identity::verify_regular_file(&temp)?;
        if !crate::path_identity::same_object(&outbox_identity, &current_outbox)
            || !crate::path_identity::same_object(&temp_identity, &current_temp)
        {
            return Err("unsafe_path".to_string());
        }
        match std::fs::symlink_metadata(&final_path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            _ => Err("unsafe_path".to_string()),
        }
    })();
    if prepublication_safe.is_err() {
        if let Err(error) = std::fs::remove_file(&temp) {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err("publish_ambiguous");
            }
        }
        return Err("publish_ambiguous");
    }
    if crate::path_identity::publish_new_file_atomic(&temp, &final_path).is_err() {
        if let Err(error) = std::fs::remove_file(&temp) {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err("publish_ambiguous");
            }
        }
        return Err("publish_ambiguous");
    }
    let (published, _) = crate::path_identity::read_bounded_regular(&final_path, max_bytes)
        .map_err(|_| "publish_ambiguous")?;
    if published != bytes {
        return Err("publish_ambiguous");
    }
    let published_outbox =
        crate::path_identity::verify_directory(outbox).map_err(|_| "publish_ambiguous")?;
    if !crate::path_identity::same_object(&outbox_identity, &published_outbox) {
        return Err("publish_ambiguous");
    }
    if let Ok(parent) = std::fs::File::open(outbox) {
        parent.sync_all().map_err(|_| "publish_ambiguous")?;
    }
    Ok(final_path)
}

pub(crate) fn ensure_terminal_snapshot_child(parent: &Path, child: &Path) -> Result<(), String> {
    let before = crate::path_identity::verify_directory(parent)?;
    match std::fs::create_dir(child) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err("unsafe_path".to_string()),
    }
    let child_before = crate::path_identity::verify_directory(child)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(child, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| "unsafe_path".to_string())?;
        if std::fs::symlink_metadata(child)
            .map_err(|_| "unsafe_path".to_string())?
            .permissions()
            .mode()
            & 0o777
            != 0o700
        {
            return Err("unsafe_path".to_string());
        }
    }
    let after = crate::path_identity::verify_directory(parent)?;
    let child_identity = crate::path_identity::verify_directory(child)?;
    if !crate::path_identity::same_object(&before, &after)
        || !crate::path_identity::same_object(&child_before, &child_identity)
        || !crate::path_identity::is_verified_descendant(&child_identity, &before)
    {
        return Err("unsafe_path".to_string());
    }
    Ok(())
}

pub(crate) fn publish_terminal_snapshot_request(
    temporary: PathBuf,
    destination: &Path,
    bytes: &[u8],
    mut stage: impl FnMut(&'static str),
) -> Result<crate::path_identity::VerifiedPathIdentity, String> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    stage("publish_request.open_temporary");
    let mut file = options
        .open(&temporary)
        .map_err(|_| "response_unavailable".to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        stage("publish_request.unix_mode");
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|_| "response_unavailable".to_string())?;
        if file
            .metadata()
            .map_err(|_| "response_unavailable".to_string())?
            .permissions()
            .mode()
            & 0o777
            != 0o600
        {
            return Err("response_unavailable".to_string());
        }
    }
    stage("publish_request.write_temporary");
    file.write_all(bytes)
        .and_then(|_| file.flush())
        .and_then(|_| file.sync_all())
        .map_err(|_| "response_unavailable".to_string())?;
    stage("publish_request.verify_temporary");
    let temporary_identity =
        crate::path_identity::verify_opened_regular_file(&temporary, &file, false)
            .map_err(|_| "response_unavailable".to_string())?;
    stage("publish_request.publish_atomic");
    if crate::path_identity::publish_new_file_atomic(&temporary, destination).is_err() {
        if let Ok(current) = crate::path_identity::verify_regular_file(&temporary) {
            if crate::path_identity::same_object(&temporary_identity, &current) {
                let _ = std::fs::remove_file(&temporary);
            }
        }
        return Err("response_unavailable".to_string());
    }
    stage("publish_request.verify_published");
    // The request is already published, so this is a best-effort observation of a
    // name a compatible daemon may legitimately claim at any moment, not a
    // confinement gate. Confinement was established before publication by the
    // create_new no-follow open and the verify on the retained handle, and that
    // retained handle/object proof is what cancellation matches on. Failing here
    // would abandon a token-bearing request the daemon may still serve, leaving
    // its response unconsumed until the TTL sweep. Fall back to the retained proof
    // so the poll loop either consumes the response or cancels the request at the
    // deadline; cancellation is identity-gated, so a final that is no longer our
    // object is left untouched.
    let published = crate::path_identity::verify_opened_regular_file(destination, &file, false)
        .unwrap_or(temporary_identity);
    drop(file);
    crate::path_identity::sync_parent_best_effort(destination);
    Ok(published)
}

pub(crate) fn remove_terminal_snapshot_cancellation(cancellation: &Path) -> bool {
    std::fs::remove_file(cancellation).is_ok()
}

#[cfg(unix)]
pub(crate) fn terminal_snapshot_response_is_private(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|metadata| metadata.permissions().mode() & 0o777 == 0o600)
        .unwrap_or(false)
}

pub(crate) fn append_terminal_snapshot_stage(path: &str, code: &str, stage: &str) {
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "code={code} stage={stage}");
    }
}
