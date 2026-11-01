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

use crate::config::instance_artifacts::SETTINGS_FILE_NAME;
use crate::phone::terminal_snapshot::{confirmation_tag, HostTerminalSnapshotRequest};

const POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Args, Clone)]
#[command(
    after_help = "Discover eligible names with: agentscommander list-peers-lean \
                  --token <live-session-token> --root <verified-requester-root> \
                  --snapshot-targets"
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalSnapshotField {
    Token,
    Root,
    To,
    /// Part of the closed `field=` set by contract. No current site attributes
    /// to it: every format mistake is a mistake about `--output` given the
    /// chosen format, so those sites name `output`. Kept so the vocabulary a
    /// consumer parses is the declared one.
    #[allow(dead_code)]
    Format,
    Output,
    Timeout,
}

impl TerminalSnapshotField {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Token => "token",
            Self::Root => "root",
            Self::To => "to",
            Self::Format => "format",
            Self::Output => "output",
            Self::Timeout => "timeout",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalSnapshotRejectionReason {
    TimeoutOutOfRange,
    TokenNotUuid,
    TokenIsPersistedStatic,
    TargetSyntaxInvalid,
    OutputRequiredForPng,
    OutputForbiddenForJson,
    OutputPathRejected,
    RequesterRootUnverified,
    RequesterNotCoordinator,
    RequesterRootObjectMismatch,
    LocalStateUnsafe,
    RequestIdCollision,
    RequestEncodingFailed,
    Unattributed,
}

impl TerminalSnapshotRejectionReason {
    const fn as_str(self) -> &'static str {
        match self {
            Self::TimeoutOutOfRange => "timeout_out_of_range",
            Self::TokenNotUuid => "token_not_uuid",
            Self::TokenIsPersistedStatic => "token_is_persisted_static",
            Self::TargetSyntaxInvalid => "target_syntax_invalid",
            Self::OutputRequiredForPng => "output_required_for_png",
            Self::OutputForbiddenForJson => "output_forbidden_for_json",
            Self::OutputPathRejected => "output_path_rejected",
            Self::RequesterRootUnverified => "requester_root_unverified",
            Self::RequesterNotCoordinator => "requester_not_coordinator",
            Self::RequesterRootObjectMismatch => "requester_root_object_mismatch",
            Self::LocalStateUnsafe => "local_state_unsafe",
            Self::RequestIdCollision => "request_id_collision",
            Self::RequestEncodingFailed => "request_encoding_failed",
            Self::Unattributed => "unattributed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TerminalSnapshotRejection {
    code: TerminalSnapshotReasonCode,
    field: Option<TerminalSnapshotField>,
    reason: TerminalSnapshotRejectionReason,
}

impl TerminalSnapshotRejection {
    const fn new(
        code: TerminalSnapshotReasonCode,
        field: Option<TerminalSnapshotField>,
        reason: TerminalSnapshotRejectionReason,
    ) -> Self {
        Self {
            code,
            field,
            reason,
        }
    }
}

/// Every error still propagating as a `String` - from the response path and
/// from every helper this phase does not re-type - converts here. The code is
/// preserved byte for byte; only the attribution is absent, which is exactly
/// what `unattributed` states.
impl From<String> for TerminalSnapshotRejection {
    fn from(value: String) -> Self {
        Self {
            code: reason_code(&value).unwrap_or(TerminalSnapshotReasonCode::Internal),
            field: None,
            reason: TerminalSnapshotRejectionReason::Unattributed,
        }
    }
}

pub fn execute(args: TerminalSnapshotArgs, history: &SnapshotStageHistory) -> i32 {
    match execute_inner(args, history) {
        Ok(()) => 0,
        Err(rejection) => fail(rejection, history),
    }
}

fn validate_snapshot_output(
    format: TerminalSnapshotFormat,
    output: Option<&Path>,
) -> Result<(), TerminalSnapshotRejection> {
    use TerminalSnapshotField as F;
    use TerminalSnapshotReasonCode as C;
    use TerminalSnapshotRejectionReason as R;

    match (format, output) {
        (TerminalSnapshotFormat::Json, None) => {}
        (TerminalSnapshotFormat::Png, Some(output)) => {
            crate::path_identity::validate_terminal_snapshot_output_path(output).map_err(
                |reason| {
                    TerminalSnapshotRejection::new(
                        reason_code(&reason).unwrap_or(C::Internal),
                        Some(F::Output),
                        R::OutputPathRejected,
                    )
                },
            )?;
        }
        (TerminalSnapshotFormat::Png, None) => {
            return Err(TerminalSnapshotRejection::new(
                C::InvalidRequest,
                Some(F::Output),
                R::OutputRequiredForPng,
            ))
        }
        (TerminalSnapshotFormat::Json, Some(_)) => {
            return Err(TerminalSnapshotRejection::new(
                C::InvalidRequest,
                Some(F::Output),
                R::OutputForbiddenForJson,
            ))
        }
    }
    Ok(())
}

fn validate_snapshot_args(
    args: &TerminalSnapshotArgs,
) -> Result<(Uuid, TerminalSnapshotFormat), TerminalSnapshotRejection> {
    use TerminalSnapshotField as F;
    use TerminalSnapshotReasonCode as C;
    use TerminalSnapshotRejectionReason as R;

    if !(5..=60).contains(&args.timeout) {
        return Err(TerminalSnapshotRejection::new(
            C::InvalidRequest,
            Some(F::Timeout),
            R::TimeoutOutOfRange,
        ));
    }
    let token = terminal_snapshot_renderer::validate_uuid(&args.token, Some(4)).map_err(|_| {
        TerminalSnapshotRejection::new(C::InvalidRequest, Some(F::Token), R::TokenNotUuid)
    })?;
    if is_persisted_static_token(&args.token) {
        return Err(TerminalSnapshotRejection::new(
            C::InvalidRequest,
            Some(F::Token),
            R::TokenIsPersistedStatic,
        ));
    }
    terminal_snapshot_renderer::validate_target_syntax(&args.to).map_err(|_| {
        TerminalSnapshotRejection::new(C::InvalidRequest, Some(F::To), R::TargetSyntaxInvalid)
    })?;
    let format = TerminalSnapshotFormat::from(args.format);
    validate_snapshot_output(format, args.output.as_deref())?;
    Ok((token, format))
}

fn execute_inner(
    args: TerminalSnapshotArgs,
    history: &SnapshotStageHistory,
) -> Result<(), TerminalSnapshotRejection> {
    use TerminalSnapshotField as F;
    use TerminalSnapshotReasonCode as C;
    use TerminalSnapshotRejectionReason as R;

    let (token, format) = validate_snapshot_args(&args)?;
    let (root_identity, from) = verify_requester_root(&args.root)?;
    let local = args.root.join(crate::config::agent_local_dir_name());
    let outbox = local.join("outbox");
    let request_directory = outbox.join("terminal-snapshot-requests");
    let response_directory = local.join("terminal-snapshot-responses");
    let local_state_unsafe =
        || TerminalSnapshotRejection::new(C::UnsafePath, Some(F::Root), R::LocalStateUnsafe);
    let local_identity =
        crate::path_identity::verify_directory(&local).map_err(|_| local_state_unsafe())?;
    if !crate::path_identity::is_verified_descendant(&local_identity, &root_identity) {
        return Err(local_state_unsafe());
    }
    let outbox_identity =
        crate::path_identity::verify_directory(&outbox).map_err(|_| local_state_unsafe())?;
    if !crate::path_identity::is_verified_descendant(&outbox_identity, &root_identity) {
        return Err(local_state_unsafe());
    }
    let propagated_local_state_unsafe = |reason: String| {
        TerminalSnapshotRejection::new(
            reason_code(&reason).unwrap_or(C::Internal),
            Some(F::Root),
            R::LocalStateUnsafe,
        )
    };
    ensure_private_child(&outbox, &request_directory).map_err(propagated_local_state_unsafe)?;
    ensure_private_child(&local, &response_directory).map_err(propagated_local_state_unsafe)?;

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
    let request_bytes = to_ascii_json(&request, MAX_REQUEST_BYTES).map_err(|_| {
        TerminalSnapshotRejection::new(C::InvalidRequest, None, R::RequestEncodingFailed)
    })?;
    let request_path = request_directory.join(format!("{request_id}.json"));
    let response_path = response_directory.join(format!("{request_id}.json"));
    if crate::filesystem::directories::cli_outbox_artifact_exists(&request_path)
        || crate::filesystem::directories::cli_outbox_artifact_exists(&response_path)
    {
        return Err(TerminalSnapshotRejection::new(
            C::UnsafePath,
            None,
            R::RequestIdCollision,
        ));
    }
    let published_request_identity = publish_request(
        &request_directory,
        &request_path,
        request_id,
        &request.nonce,
        &request_bytes,
        history,
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
            return Err(TerminalSnapshotRejection::from(
                "snapshot_timeout".to_string(),
            ));
        }
        if crate::filesystem::directories::cli_outbox_artifact_exists(&response_path) {
            let (response, response_identity) = read_response(
                &response_path,
                &request.request_id,
                &request.confirmation_tag,
                &request.to,
                request.format,
                deadline,
                history,
            )?;
            finish_response(
                &response_path,
                &response_identity,
                &response,
                format,
                args.output.as_deref(),
                &request.from,
                &request.to,
                deadline,
                history,
            )?;
            return Ok(());
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// #1245: fixed, path-free diagnostic for a local requester-identity failure.
/// The stable CLI error stays `requester_unavailable`; this text only reaches
/// `app.log`, never stdout or stderr. Every reason in this chain is a fixed
/// literal except the five `config::ac_root::ensure_authoritative_ac_root`
/// messages, which format absolute paths, so anything outside the known set is
/// reported as `unexpected`.
fn requester_identity_diagnostic(stage: &'static str, reason: &str) -> String {
    const KNOWN: [&str; 6] = [
        "sender_identity_invalid",
        "target_not_member",
        "invalid_target",
        "unsafe_path",
        "invalid_envelope",
        "capacity_exceeded",
    ];
    let safe = if KNOWN.contains(&reason) {
        reason
    } else {
        "unexpected"
    };
    format!("[terminal-snapshot] requester identity failed stage={stage} reason={safe}")
}

fn verify_requester_root(
    root: &Path,
) -> Result<(crate::path_identity::VerifiedPathIdentity, String), TerminalSnapshotRejection> {
    use TerminalSnapshotField as F;
    use TerminalSnapshotReasonCode as C;
    use TerminalSnapshotRejectionReason as R;

    if let Ok(identity) = crate::config::root_agent::verify_live_root_agent_path(root) {
        return Ok((
            identity,
            crate::config::root_agent::ROOT_AGENT_SENDER.to_string(),
        ));
    }
    let root_unverified = TerminalSnapshotRejection::new(
        C::RequesterUnavailable,
        Some(F::Root),
        R::RequesterRootUnverified,
    );
    let identity = crate::config::teams::verify_pty_input_replica_cwd(root).map_err(|reason| {
        log::warn!(
            "{}",
            requester_identity_diagnostic("replica_identity", &reason)
        );
        root_unverified
    })?;
    let supplied = crate::path_identity::verify_directory(root).map_err(|reason| {
        log::warn!(
            "{}",
            requester_identity_diagnostic("root_directory", &reason)
        );
        root_unverified
    })?;
    // One base `if` over two independent causes. `code=not_authorized` is
    // identical for both - nothing is reclassified - but they are different
    // mistakes with different fixes, so they get different reasons. Check
    // order is unchanged: `is_coordinator` first.
    if !identity.is_coordinator {
        return Err(TerminalSnapshotRejection::new(
            C::NotAuthorized,
            Some(F::Root),
            R::RequesterNotCoordinator,
        ));
    }
    if !crate::path_identity::same_object(&supplied, &identity.replica_identity) {
        return Err(TerminalSnapshotRejection::new(
            C::NotAuthorized,
            Some(F::Root),
            R::RequesterRootObjectMismatch,
        ));
    }
    Ok((supplied, identity.canonical_fqn))
}

fn is_persisted_static_token(token: &str) -> bool {
    let Some(directory) = crate::config::config_dir() else {
        return false;
    };
    let settings_token = crate::path_identity::read_bounded_regular(
        &directory.join(SETTINGS_FILE_NAME),
        1024 * 1024,
    )
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
    crate::filesystem::ensure_terminal_snapshot_child(parent, child)
}

fn publish_request(
    request_directory: &Path,
    destination: &Path,
    request_id: Uuid,
    nonce: &str,
    bytes: &[u8],
    history: &SnapshotStageHistory,
) -> Result<crate::path_identity::VerifiedPathIdentity, String> {
    let temporary = request_directory.join(format!(
        ".{request_id}.{nonce}.terminal-snapshot-request-tmp"
    ));
    crate::filesystem::publish_terminal_snapshot_request(temporary, destination, bytes, |stage| {
        enter_stage(stage, history)
    })
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
    crate::filesystem::remove_terminal_snapshot_cancellation(&cancellation)
}

fn read_response(
    path: &Path,
    request_id: &str,
    confirmation_tag: &str,
    target: &str,
    format: TerminalSnapshotFormat,
    deadline: Instant,
    history: &SnapshotStageHistory,
) -> Result<
    (
        TerminalSnapshotHostResponse,
        crate::path_identity::VerifiedPathIdentity,
    ),
    String,
> {
    enter_stage("read_response.entry_deadline", history);
    ensure_client_deadline(deadline)?;
    enter_stage("read_response.verify_initial", history);
    let initial = crate::path_identity::verify_regular_file(path)
        .map_err(|_| "response_unavailable".to_string())?;
    #[cfg(unix)]
    {
        enter_stage("read_response.private_mode", history);
        let private = crate::filesystem::terminal_snapshot_response_is_private(path);
        if !private {
            remove_response(path, &initial);
            return Err("response_unavailable".to_string());
        }
    }
    enter_stage("read_response.read_bounded", history);
    let (bytes, identity) =
        match crate::path_identity::read_bounded_regular(path, MAX_TRANSPORT_BYTES) {
            Ok(value) => value,
            Err(_) => {
                remove_response(path, &initial);
                return Err("response_unavailable".to_string());
            }
        };
    enter_stage("read_response.same_object_initial", history);
    if !crate::path_identity::same_object(&initial, &identity) {
        return Err("response_unavailable".to_string());
    }
    let outcome = (|| {
        enter_stage("read_response.pre_decode_deadline", history);
        ensure_client_deadline(deadline)?;
        enter_stage("read_response.decode", history);
        let response = decode_host_response(&bytes, request_id, confirmation_tag, target, format)
            .map_err(|_| "response_unavailable".to_string())?;
        enter_stage("read_response.post_decode_deadline", history);
        ensure_client_deadline(deadline)?;
        enter_stage("read_response.verify_current", history);
        let current = crate::path_identity::verify_regular_file(path)
            .map_err(|_| "response_unavailable".to_string())?;
        enter_stage("read_response.same_object_current", history);
        if !crate::path_identity::same_object(&identity, &current) {
            return Err("response_unavailable".to_string());
        }
        enter_stage("read_response.exit_deadline", history);
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
    history: &SnapshotStageHistory,
) -> Result<(), String> {
    let outcome = (|| {
        enter_stage("finish_response.validate_timestamp", history);
        let expires_at = terminal_snapshot_renderer::validate_timestamp(&response.expires_at)
            .map_err(|_| "response_unavailable".to_string())?;
        let now = chrono::Utc::now();
        if now >= expires_at {
            enter_stage("finish_response.expiry_elapsed", history);
            return Err("response_unavailable".to_string());
        }
        if expires_at > now + chrono::Duration::seconds(65) {
            enter_stage("finish_response.expiry_far_future", history);
            return Err("response_unavailable".to_string());
        }
        enter_stage("finish_response.deadline", history);
        ensure_client_deadline(deadline)?;
        handle_response(
            response,
            format,
            output,
            expected_requester,
            expected_target,
            deadline,
            history,
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
    history: &SnapshotStageHistory,
) -> Result<(), String> {
    enter_stage("handle_response.entry_deadline", history);
    ensure_client_deadline(deadline)?;
    if let Some(error) = response.error {
        enter_stage("handle_response.server_error", history);
        return Err(error.as_str().to_string());
    }
    enter_stage("handle_response.result_present", history);
    let result = response
        .result
        .as_ref()
        .ok_or_else(|| "response_unavailable".to_string())?;
    enter_stage("handle_response.identity_match", history);
    if result.requester() != expected_requester || result.target() != expected_target {
        return Err("response_unavailable".to_string());
    }
    match (format, result) {
        (TerminalSnapshotFormat::Json, TerminalSnapshotPayload::Json { snapshot }) => {
            enter_stage("handle_response.json.validate", history);
            snapshot
                .validate()
                .map_err(|_| "response_unavailable".to_string())?;
            enter_stage("handle_response.json.encode", history);
            let bytes = to_ascii_json(snapshot, MAX_TRANSPORT_BYTES)
                .map_err(|_| "response_unavailable".to_string())?;
            enter_stage("handle_response.json.stdout", history);
            write_stdout_line(&bytes, deadline)
        }
        (TerminalSnapshotFormat::Png, TerminalSnapshotPayload::Png { metadata, png }) => {
            enter_stage("handle_response.png.output_required", history);
            let output = output.ok_or_else(|| "invalid_request".to_string())?;
            enter_stage("handle_response.png.metadata_validate", history);
            metadata
                .validate()
                .map_err(|_| "response_unavailable".to_string())?;
            enter_stage("handle_response.png.metadata_encode", history);
            let bytes = to_ascii_json(metadata, MAX_REQUEST_BYTES)
                .map_err(|_| "output_failed".to_string())?;
            enter_stage("handle_response.png.pre_output_deadline", history);
            ensure_client_deadline(deadline)?;
            enter_stage("handle_response.png.create_output", history);
            let file = crate::path_identity::create_terminal_snapshot_output(output)?;
            enter_stage("handle_response.png.write_png", history);
            file.write_all_and_sync(png)?;
            enter_stage("handle_response.png.stdout", history);
            write_stdout_line(&bytes, deadline)
        }
        _ => {
            enter_stage("handle_response.format_payload_mismatch", history);
            Err("response_unavailable".to_string())
        }
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

// Records the last request/response stage entered, so an emitted failure code
// can be attributed to the step that produced it.
//
// The stage never reaches stdout and never reaches the emitted error line: plan
// §3.1 requires exactly one fixed stderr line and §6 fixes its shape, so putting
// the stage there would both break the contract and weaken the assertion that
// detects a wrong code. It goes to the file named by AC_TERMINAL_SNAPSHOT_STAGE_FILE
// and nowhere else, which means it is inert unless a test sets that variable.
//
// This is the only instrument that has located a wrong failure code on this
// path. Without it a `response_unavailable` that should have been
// `output_failed` is indistinguishable from a genuine server-response problem.
#[derive(Default)]
pub struct SnapshotStageHistory {
    current: std::sync::Mutex<Option<&'static str>>,
}

impl SnapshotStageHistory {
    fn current_stage(&self) -> &'static str {
        self.current
            .lock()
            .ok()
            .and_then(|current| *current)
            .unwrap_or("none")
    }
}

fn enter_stage(stage: &'static str, history: &SnapshotStageHistory) {
    if let Ok(mut current) = history.current.lock() {
        *current = Some(stage);
    }
}

fn record_stage_side_channel(code: &str, history: &SnapshotStageHistory) {
    let Ok(path) = crate::config::environment::terminal_snapshot_stage_file() else {
        return;
    };
    let stage = history.current_stage();
    crate::filesystem::append_terminal_snapshot_stage(&path, code, stage);
}

fn fail(rejection: TerminalSnapshotRejection, history: &SnapshotStageHistory) -> i32 {
    record_stage_side_channel(rejection.code.as_str(), history);
    eprintln!("{}", rejection_line(rejection));
    1
}

/// The whole emitted contract, in one place so tests assert on the same bytes
/// stderr receives. Every token after `code=` is a compile-time `&'static str`
/// from a `match` on an enum; no caller-supplied value can reach this line.
fn rejection_line(rejection: TerminalSnapshotRejection) -> String {
    match rejection.field {
        Some(field) => format!(
            "terminal_snapshot_error code={} field={} reason={} detail={}",
            rejection.code.as_str(),
            field.as_str(),
            rejection.reason.as_str(),
            rejection.code.detail()
        ),
        None => format!(
            "terminal_snapshot_error code={} reason={} detail={}",
            rejection.code.as_str(),
            rejection.reason.as_str(),
            rejection.code.detail()
        ),
    }
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
mod tests;
