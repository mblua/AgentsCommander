use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use clap::{Args, ValueEnum};
use terminal_snapshot_renderer::{
    canonical_timestamp, decode_host_response, to_ascii_json, TerminalSnapshotFormat,
    TerminalSnapshotHostResponse, TerminalSnapshotPayload, TerminalSnapshotReasonCode,
    MAX_REQUEST_BYTES, MAX_TRANSPORT_BYTES,
};
use uuid::Uuid;

use crate::phone::terminal_snapshot::{confirmation_tag, HostTerminalSnapshotRequest};

const POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Args, Clone)]
#[command(
    after_help = "Discover eligible names with: agentscommander list-peers-lean --token <live-session-token> --root <verified-requester-root> --snapshot-targets"
)]
pub struct TerminalSnapshotArgs {
    /// Live session token from AGENTSCOMMANDER_TOKEN
    #[arg(long)]
    pub token: String,

    /// Exact verified requester replica or Root Agent directory
    #[arg(long)]
    pub root: PathBuf,

    /// Canonical fully-qualified target identity
    #[arg(long)]
    pub to: String,

    /// Snapshot representation
    #[arg(long, value_enum, default_value_t = SnapshotFormatArg::Json)]
    pub format: SnapshotFormatArg,

    /// Absolute new .png output path, required only for PNG
    #[arg(long)]
    pub output: Option<PathBuf>,

    /// Wait timeout in seconds (5..60)
    #[arg(long, default_value_t = 15)]
    pub timeout: u64,
}

impl std::fmt::Debug for TerminalSnapshotArgs {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TerminalSnapshotArgs")
            .field("format", &self.format)
            .field("has_output", &self.output.is_some())
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum SnapshotFormatArg {
    Json,
    Png,
}

impl From<SnapshotFormatArg> for TerminalSnapshotFormat {
    fn from(value: SnapshotFormatArg) -> Self {
        match value {
            SnapshotFormatArg::Json => Self::Json,
            SnapshotFormatArg::Png => Self::Png,
        }
    }
}

pub fn execute(args: TerminalSnapshotArgs) -> i32 {
    match execute_inner(args) {
        Ok(()) => 0,
        Err(reason) => fail(&reason),
    }
}

fn execute_inner(args: TerminalSnapshotArgs) -> Result<(), String> {
    if !(5..=60).contains(&args.timeout) {
        return Err("invalid_request".to_string());
    }
    let token = terminal_snapshot_renderer::validate_uuid(&args.token, Some(4))
        .map_err(|_| "invalid_request".to_string())?;
    if is_persisted_static_token(&args.token) {
        return Err("invalid_request".to_string());
    }
    terminal_snapshot_renderer::validate_target_syntax(&args.to)
        .map_err(|_| "invalid_request".to_string())?;
    let format = TerminalSnapshotFormat::from(args.format);
    match (format, args.output.as_deref()) {
        (TerminalSnapshotFormat::Json, None) => {}
        (TerminalSnapshotFormat::Png, Some(output)) => {
            crate::path_identity::validate_terminal_snapshot_output_path(output)?;
        }
        _ => return Err("invalid_request".to_string()),
    }

    let (root_identity, from) = verify_requester_root(&args.root)?;
    let local = args.root.join(crate::config::agent_local_dir_name());
    let outbox = local.join("outbox");
    let request_directory = outbox.join("terminal-snapshot-requests");
    let response_directory = local.join("terminal-snapshot-responses");
    let local_identity =
        crate::path_identity::verify_directory(&local).map_err(|_| "unsafe_path".to_string())?;
    if !crate::path_identity::is_verified_descendant(&local_identity, &root_identity) {
        return Err("unsafe_path".to_string());
    }
    let outbox_identity =
        crate::path_identity::verify_directory(&outbox).map_err(|_| "unsafe_path".to_string())?;
    if !crate::path_identity::is_verified_descendant(&outbox_identity, &root_identity) {
        return Err("unsafe_path".to_string());
    }
    ensure_private_child(&outbox, &request_directory)?;
    ensure_private_child(&local, &response_directory)?;

    let request_id = Uuid::new_v4();
    let issued = chrono::Utc::now();
    let expires =
        issued + chrono::Duration::seconds(i64::try_from(args.timeout.min(30)).unwrap_or(30));
    let mut nonce = [0_u8; 32];
    getrandom::fill(&mut nonce).map_err(|_| "service_unavailable".to_string())?;
    let nonce = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
    let mut request = HostTerminalSnapshotRequest {
        kind: "terminal-snapshot".to_string(),
        version: 1,
        request_id: request_id.to_string(),
        token: token.to_string(),
        from,
        to: args.to,
        format,
        issued_at: canonical_timestamp(issued),
        expires_at: canonical_timestamp(expires),
        nonce,
        confirmation_tag: String::new(),
    };
    request.confirmation_tag = confirmation_tag(&request);
    let request_bytes =
        to_ascii_json(&request, MAX_REQUEST_BYTES).map_err(|_| "invalid_request".to_string())?;
    let request_path = request_directory.join(format!("{request_id}.json"));
    let response_path = response_directory.join(format!("{request_id}.json"));
    if request_path.exists() || response_path.exists() {
        return Err("unsafe_path".to_string());
    }
    let published_request_identity = publish_request(
        &request_directory,
        &request_path,
        request_id,
        &request.nonce,
        &request_bytes,
    )?;

    let deadline = Instant::now() + Duration::from_secs(args.timeout);
    loop {
        if Instant::now() >= deadline {
            cancel_request(
                &request_path,
                &request_directory,
                request_id,
                &request.nonce,
                &published_request_identity,
            );
            return Err("snapshot_timeout".to_string());
        }
        if response_path.exists() {
            let (response, response_identity) = read_response(
                &response_path,
                &request.request_id,
                &request.confirmation_tag,
                &request.to,
                request.format,
                deadline,
            )?;
            return finish_response(
                &response_path,
                &response_identity,
                &response,
                format,
                args.output.as_deref(),
                &request.from,
                &request.to,
                deadline,
            );
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn verify_requester_root(
    root: &Path,
) -> Result<(crate::path_identity::VerifiedPathIdentity, String), String> {
    if let Ok(identity) = crate::config::root_agent::verify_live_root_agent_path(root) {
        return Ok((
            identity,
            crate::config::root_agent::ROOT_AGENT_SENDER.to_string(),
        ));
    }
    let identity = crate::config::teams::verify_pty_input_replica_cwd(root)
        .map_err(|_| "requester_unavailable".to_string())?;
    let supplied = crate::path_identity::verify_directory(root)
        .map_err(|_| "requester_unavailable".to_string())?;
    if !identity.is_coordinator
        || !crate::path_identity::same_object(&supplied, &identity.replica_identity)
    {
        return Err("not_authorized".to_string());
    }
    Ok((supplied, identity.canonical_fqn))
}

fn is_persisted_static_token(token: &str) -> bool {
    let Some(directory) = crate::config::config_dir() else {
        return false;
    };
    let settings_token =
        crate::path_identity::read_bounded_regular(&directory.join("settings.json"), 1024 * 1024)
            .ok()
            .and_then(|(bytes, _)| crate::path_identity::parse_json_no_duplicates(&bytes).ok())
            .and_then(|value| {
                value
                    .get("rootToken")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            });
    if settings_token.as_deref() == Some(token) {
        return true;
    }
    crate::path_identity::read_bounded_regular(&directory.join("master-token.txt"), 8 * 1024)
        .ok()
        .and_then(|(bytes, _)| String::from_utf8(bytes).ok())
        .is_some_and(|persisted| persisted.trim() == token)
}

fn ensure_private_child(parent: &Path, child: &Path) -> Result<(), String> {
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

fn publish_request(
    request_directory: &Path,
    destination: &Path,
    request_id: Uuid,
    nonce: &str,
    bytes: &[u8],
) -> Result<crate::path_identity::VerifiedPathIdentity, String> {
    let temporary = request_directory.join(format!(
        ".{request_id}.{nonce}.terminal-snapshot-request-tmp"
    ));
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
    let mut file = options
        .open(&temporary)
        .map_err(|_| "response_unavailable".to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
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
    file.write_all(bytes)
        .and_then(|_| file.flush())
        .and_then(|_| file.sync_all())
        .map_err(|_| "response_unavailable".to_string())?;
    let temporary_identity =
        crate::path_identity::verify_opened_regular_file(&temporary, &file, false)
            .map_err(|_| "response_unavailable".to_string())?;
    if crate::path_identity::publish_new_file_atomic(&temporary, destination).is_err() {
        if let Ok(current) = crate::path_identity::verify_regular_file(&temporary) {
            if crate::path_identity::same_object(&temporary_identity, &current) {
                let _ = std::fs::remove_file(&temporary);
            }
        }
        return Err("response_unavailable".to_string());
    }
    let published =
        match crate::path_identity::verify_opened_regular_file(destination, &file, false) {
            Ok(identity) => identity,
            Err(_)
                if std::fs::symlink_metadata(destination)
                    .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
            {
                // A compatible daemon may claim the published final before this
                // post-publication path check. The retained handle/object proof
                // remains the cancellation identity; an absent final is already
                // non-cancellable and must not turn accepted work into a local
                // publication failure.
                temporary_identity
            }
            Err(_) => return Err("response_unavailable".to_string()),
        };
    drop(file);
    crate::path_identity::sync_parent_best_effort(destination);
    Ok(published)
}

fn cancel_request(
    request_path: &Path,
    request_directory: &Path,
    request_id: Uuid,
    nonce: &str,
    expected: &crate::path_identity::VerifiedPathIdentity,
) {
    let _ = cancel_request_observed(
        request_path,
        request_directory,
        request_id,
        nonce,
        expected,
        |_| {},
    );
}

#[cfg(test)]
pub(crate) fn cancel_request_for_test(
    request_path: &Path,
    request_directory: &Path,
    request_id: Uuid,
    nonce: &str,
    expected: &crate::path_identity::VerifiedPathIdentity,
) -> bool {
    cancel_request_observed(
        request_path,
        request_directory,
        request_id,
        nonce,
        expected,
        |_| {},
    )
}

fn cancel_request_observed<F>(
    request_path: &Path,
    request_directory: &Path,
    request_id: Uuid,
    nonce: &str,
    expected: &crate::path_identity::VerifiedPathIdentity,
    after_claim: F,
) -> bool
where
    F: FnOnce(&Path),
{
    let Ok(current_final) = crate::path_identity::verify_regular_file(request_path) else {
        return false;
    };
    if !crate::path_identity::same_object(expected, &current_final) {
        return false;
    }
    let cancellation =
        request_directory.join(format!(".{request_id}.{nonce}.terminal-snapshot-cancelled"));
    if crate::path_identity::publish_new_file_atomic(request_path, &cancellation).is_err() {
        return false;
    }
    after_claim(&cancellation);
    let Ok(current) = crate::path_identity::verify_regular_file(&cancellation) else {
        return false;
    };
    if !crate::path_identity::same_object(expected, &current) {
        return false;
    }
    std::fs::remove_file(cancellation).is_ok()
}

fn read_response(
    path: &Path,
    request_id: &str,
    confirmation_tag: &str,
    target: &str,
    format: TerminalSnapshotFormat,
    deadline: Instant,
) -> Result<
    (
        TerminalSnapshotHostResponse,
        crate::path_identity::VerifiedPathIdentity,
    ),
    String,
> {
    ensure_client_deadline(deadline)?;
    let initial = crate::path_identity::verify_regular_file(path)
        .map_err(|_| "response_unavailable".to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let private = std::fs::symlink_metadata(path)
            .map(|metadata| metadata.permissions().mode() & 0o777 == 0o600)
            .unwrap_or(false);
        if !private {
            remove_response(path, &initial);
            return Err("response_unavailable".to_string());
        }
    }
    let (bytes, identity) =
        match crate::path_identity::read_bounded_regular(path, MAX_TRANSPORT_BYTES) {
            Ok(value) => value,
            Err(_) => {
                remove_response(path, &initial);
                return Err("response_unavailable".to_string());
            }
        };
    if !crate::path_identity::same_object(&initial, &identity) {
        return Err("response_unavailable".to_string());
    }
    let outcome = (|| {
        ensure_client_deadline(deadline)?;
        let response = decode_host_response(&bytes, request_id, confirmation_tag, target, format)
            .map_err(|_| "response_unavailable".to_string())?;
        ensure_client_deadline(deadline)?;
        let current = crate::path_identity::verify_regular_file(path)
            .map_err(|_| "response_unavailable".to_string())?;
        if !crate::path_identity::same_object(&identity, &current) {
            return Err("response_unavailable".to_string());
        }
        ensure_client_deadline(deadline)?;
        Ok(response)
    })();
    match outcome {
        Ok(response) => Ok((response, identity)),
        Err(reason) => {
            remove_response(path, &identity);
            Err(reason)
        }
    }
}

fn remove_response(path: &Path, expected: &crate::path_identity::VerifiedPathIdentity) -> bool {
    crate::path_identity::remove_regular_file_if_same(path, expected)
}

#[allow(clippy::too_many_arguments)]
fn finish_response(
    path: &Path,
    identity: &crate::path_identity::VerifiedPathIdentity,
    response: &TerminalSnapshotHostResponse,
    format: TerminalSnapshotFormat,
    output: Option<&Path>,
    expected_requester: &str,
    expected_target: &str,
    deadline: Instant,
) -> Result<(), String> {
    let outcome = (|| {
        let expires_at = terminal_snapshot_renderer::validate_timestamp(&response.expires_at)
            .map_err(|_| "response_unavailable".to_string())?;
        let now = chrono::Utc::now();
        if now >= expires_at || expires_at > now + chrono::Duration::seconds(65) {
            return Err("response_unavailable".to_string());
        }
        ensure_client_deadline(deadline)?;
        handle_response(
            response,
            format,
            output,
            expected_requester,
            expected_target,
            deadline,
        )
    })();
    remove_response(path, identity);
    outcome
}

fn handle_response(
    response: &TerminalSnapshotHostResponse,
    format: TerminalSnapshotFormat,
    output: Option<&Path>,
    expected_requester: &str,
    expected_target: &str,
    deadline: Instant,
) -> Result<(), String> {
    ensure_client_deadline(deadline)?;
    if let Some(error) = response.error {
        return Err(error.as_str().to_string());
    }
    let result = response
        .result
        .as_ref()
        .ok_or_else(|| "response_unavailable".to_string())?;
    if result.requester() != expected_requester || result.target() != expected_target {
        return Err("response_unavailable".to_string());
    }
    match (format, result) {
        (TerminalSnapshotFormat::Json, TerminalSnapshotPayload::Json { snapshot }) => {
            snapshot
                .validate()
                .map_err(|_| "response_unavailable".to_string())?;
            let bytes = to_ascii_json(snapshot, MAX_TRANSPORT_BYTES)
                .map_err(|_| "response_unavailable".to_string())?;
            write_stdout_line(&bytes, deadline)
        }
        (TerminalSnapshotFormat::Png, TerminalSnapshotPayload::Png { metadata, png }) => {
            let output = output.ok_or_else(|| "invalid_request".to_string())?;
            metadata
                .validate()
                .map_err(|_| "response_unavailable".to_string())?;
            let bytes = to_ascii_json(metadata, MAX_REQUEST_BYTES)
                .map_err(|_| "output_failed".to_string())?;
            ensure_client_deadline(deadline)?;
            let file = crate::path_identity::create_terminal_snapshot_output(output)?;
            file.write_all_and_sync(png)?;
            write_stdout_line(&bytes, deadline)
        }
        _ => Err("response_unavailable".to_string()),
    }
}

fn write_stdout_line(bytes: &[u8], deadline: Instant) -> Result<(), String> {
    ensure_client_deadline(deadline)?;
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(bytes)
        .and_then(|_| stdout.write_all(b"\n"))
        .and_then(|_| stdout.flush())
        .map_err(|_| "output_failed".to_string())
}

fn ensure_client_deadline(deadline: Instant) -> Result<(), String> {
    if Instant::now() >= deadline {
        Err("snapshot_timeout".to_string())
    } else {
        Ok(())
    }
}

fn fail(reason: &str) -> i32 {
    let reason = reason_code(reason).unwrap_or(TerminalSnapshotReasonCode::Internal);
    eprintln!(
        "terminal_snapshot_error code={} detail={}",
        reason.as_str(),
        reason.detail()
    );
    1
}

fn reason_code(value: &str) -> Option<TerminalSnapshotReasonCode> {
    use TerminalSnapshotReasonCode as C;
    Some(match value {
        "invalid_request" => C::InvalidRequest,
        "requester_unavailable" => C::RequesterUnavailable,
        "terminal_snapshots_disabled" => C::TerminalSnapshotsDisabled,
        "not_authorized" => C::NotAuthorized,
        "target_unavailable" => C::TargetUnavailable,
        "snapshot_unavailable" => C::SnapshotUnavailable,
        "snapshot_too_large" => C::SnapshotTooLarge,
        "authority_changed" => C::AuthorityChanged,
        "rate_limited" => C::RateLimited,
        "snapshot_timeout" => C::SnapshotTimeout,
        "service_unavailable" => C::ServiceUnavailable,
        "render_failed" => C::RenderFailed,
        "unsafe_path" => C::UnsafePath,
        "output_failed" => C::OutputFailed,
        "response_unavailable" => C::ResponseUnavailable,
        "internal" => C::Internal,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_private_response(path: &Path, bytes: &[u8]) {
        std::fs::write(path, bytes).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
    }

    #[test]
    fn timeout_cancellation_claims_and_removes_only_the_original_request() {
        let directory = tempfile::TempDir::new().unwrap();
        let directory_identity = crate::path_identity::verify_directory(directory.path()).unwrap();
        let caller_output = directory.path().join("caller-output.png");
        let nonce = "a".repeat(64);

        let request_id = Uuid::new_v4();
        let request = directory.path().join(format!("{request_id}.json"));
        let marker = directory
            .path()
            .join(format!(".{request_id}.{nonce}.terminal-snapshot-cancelled"));
        std::fs::write(&request, b"unclaimed-request").unwrap();
        let expected = crate::path_identity::verify_regular_file(&request).unwrap();
        assert!(cancel_request_observed(
            &request,
            directory.path(),
            request_id,
            &nonce,
            &expected,
            |claimed| {
                assert_eq!(claimed, marker);
                assert!(!request.exists());
                let claimed_identity = crate::path_identity::verify_regular_file(claimed).unwrap();
                assert!(crate::path_identity::same_object(
                    &expected,
                    &claimed_identity
                ));
                assert!(crate::path_identity::is_verified_descendant(
                    &claimed_identity,
                    &directory_identity
                ));
            },
        ));
        assert!(!request.exists());
        assert!(!marker.exists());
        assert!(!caller_output.exists());

        let request_id = Uuid::new_v4();
        let request = directory.path().join(format!("{request_id}.json"));
        let processing = directory.path().join(format!(
            ".{request_id}.{}.terminal-snapshot-processing",
            Uuid::new_v4()
        ));
        std::fs::write(&request, b"daemon-claimed-request").unwrap();
        let expected = crate::path_identity::verify_regular_file(&request).unwrap();
        crate::path_identity::publish_new_file_atomic(&request, &processing).unwrap();
        assert!(!cancel_request_for_test(
            &request,
            directory.path(),
            request_id,
            &nonce,
            &expected,
        ));
        assert_eq!(
            std::fs::read(&processing).unwrap(),
            b"daemon-claimed-request"
        );
        assert!(!directory
            .path()
            .join(format!(".{request_id}.{nonce}.terminal-snapshot-cancelled"))
            .exists());

        let request_id = Uuid::new_v4();
        let request = directory.path().join(format!("{request_id}.json"));
        let marker = directory
            .path()
            .join(format!(".{request_id}.{nonce}.terminal-snapshot-cancelled"));
        std::fs::write(&request, b"collision-request").unwrap();
        let expected = crate::path_identity::verify_regular_file(&request).unwrap();
        std::fs::write(&marker, b"marker-collision").unwrap();
        assert!(!cancel_request_for_test(
            &request,
            directory.path(),
            request_id,
            &nonce,
            &expected,
        ));
        assert_eq!(std::fs::read(&request).unwrap(), b"collision-request");
        assert_eq!(std::fs::read(&marker).unwrap(), b"marker-collision");

        let request_id = Uuid::new_v4();
        let request = directory.path().join(format!("{request_id}.json"));
        let accepted = directory.path().join(format!(
            ".{request_id}.{}.terminal-snapshot-processing",
            Uuid::new_v4()
        ));
        std::fs::write(&request, b"original-request").unwrap();
        let expected = crate::path_identity::verify_regular_file(&request).unwrap();
        crate::path_identity::publish_new_file_atomic(&request, &accepted).unwrap();
        std::fs::write(&request, b"replacement-request").unwrap();
        assert!(!cancel_request_for_test(
            &request,
            directory.path(),
            request_id,
            &nonce,
            &expected,
        ));
        assert_eq!(std::fs::read(&request).unwrap(), b"replacement-request");
        assert_eq!(std::fs::read(&accepted).unwrap(), b"original-request");

        let request_id = Uuid::new_v4();
        let request = directory.path().join(format!("{request_id}.json"));
        let marker = directory
            .path()
            .join(format!(".{request_id}.{nonce}.terminal-snapshot-cancelled"));
        let displaced = directory.path().join("displaced-original-request");
        std::fs::write(&request, b"original-at-marker").unwrap();
        let expected = crate::path_identity::verify_regular_file(&request).unwrap();
        assert!(!cancel_request_observed(
            &request,
            directory.path(),
            request_id,
            &nonce,
            &expected,
            |claimed| {
                std::fs::rename(claimed, &displaced).unwrap();
                std::fs::write(claimed, b"replacement-at-marker").unwrap();
            },
        ));
        assert_eq!(std::fs::read(&marker).unwrap(), b"replacement-at-marker");
        assert_eq!(std::fs::read(&displaced).unwrap(), b"original-at-marker");
        let displaced_identity = crate::path_identity::verify_regular_file(&displaced).unwrap();
        assert!(crate::path_identity::same_object(
            &expected,
            &displaced_identity
        ));
        assert!(!caller_output.exists());
    }

    #[test]
    fn malformed_corrupt_and_late_responses_are_removed_after_rejection() {
        let directory = tempfile::TempDir::new().unwrap();
        let target = "project:wg-1-team/member";
        let tag = "a".repeat(64);
        let request_id = Uuid::new_v4().to_string();
        let deadline = Instant::now() + Duration::from_secs(30);

        let malformed = directory.path().join("malformed.json");
        write_private_response(&malformed, b"{");
        assert_eq!(
            read_response(
                &malformed,
                &request_id,
                &tag,
                target,
                TerminalSnapshotFormat::Json,
                deadline,
            )
            .unwrap_err(),
            "response_unavailable"
        );
        assert!(!malformed.exists());

        let corrupt = directory.path().join("corrupt.json");
        let corrupt_bytes = terminal_snapshot_renderer::encode_host_failure_payload(
            &Uuid::new_v4().to_string(),
            &tag,
            &canonical_timestamp(chrono::Utc::now() + chrono::Duration::seconds(60)),
            TerminalSnapshotReasonCode::InvalidRequest,
        )
        .unwrap();
        write_private_response(&corrupt, &corrupt_bytes);
        assert_eq!(
            read_response(
                &corrupt,
                &request_id,
                &tag,
                target,
                TerminalSnapshotFormat::Json,
                deadline,
            )
            .unwrap_err(),
            "response_unavailable"
        );
        assert!(!corrupt.exists());

        let late = directory.path().join("late.json");
        let late_bytes = terminal_snapshot_renderer::encode_host_failure_payload(
            &request_id,
            &tag,
            &canonical_timestamp(chrono::Utc::now() - chrono::Duration::seconds(1)),
            TerminalSnapshotReasonCode::InvalidRequest,
        )
        .unwrap();
        write_private_response(&late, &late_bytes);
        let (response, identity) = read_response(
            &late,
            &request_id,
            &tag,
            target,
            TerminalSnapshotFormat::Json,
            deadline,
        )
        .unwrap();
        assert_eq!(
            finish_response(
                &late,
                &identity,
                &response,
                TerminalSnapshotFormat::Json,
                None,
                "project:wg-1-team/coordinator",
                target,
                deadline,
            )
            .unwrap_err(),
            "response_unavailable"
        );
        assert!(!late.exists());
    }

    #[test]
    fn expired_deadline_wins_before_response_read_or_output_handoff() {
        let missing =
            std::env::temp_dir().join(format!("ac-snapshot-missing-{}.json", Uuid::new_v4()));
        let error = read_response(
            &missing,
            "00000000-0000-4000-8000-000000000119",
            &"a".repeat(64),
            "project:wg-1-team/member",
            TerminalSnapshotFormat::Json,
            Instant::now(),
        )
        .unwrap_err();
        assert_eq!(error, "snapshot_timeout");
        assert_eq!(
            write_stdout_line(b"{}", Instant::now()).unwrap_err(),
            "snapshot_timeout"
        );
    }

    #[test]
    fn argument_debug_omits_token_target_and_path_canaries() {
        const AUTH_CANARY: &str = "AUTH_1173_C3P7";
        const PATH_CANARY: &str = r"C:\PATH_1173_C3P7\snapshot.png";
        let args = TerminalSnapshotArgs {
            token: AUTH_CANARY.to_string(),
            root: PathBuf::from(PATH_CANARY),
            to: AUTH_CANARY.to_string(),
            format: SnapshotFormatArg::Png,
            output: Some(PathBuf::from(PATH_CANARY)),
            timeout: 15,
        };
        let diagnostic = format!("{args:?}");
        assert!(!diagnostic.contains(AUTH_CANARY));
        assert!(!diagnostic.contains(PATH_CANARY));
        assert!(diagnostic.contains("format: Png"));
        assert!(diagnostic.contains("has_output: true"));
        assert!(diagnostic.contains("timeout: 15"));
    }

    #[test]
    fn format_conversion_is_exact() {
        assert_eq!(
            TerminalSnapshotFormat::from(SnapshotFormatArg::Json),
            TerminalSnapshotFormat::Json
        );
        assert_eq!(
            TerminalSnapshotFormat::from(SnapshotFormatArg::Png),
            TerminalSnapshotFormat::Png
        );
    }
}
