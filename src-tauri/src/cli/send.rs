use clap::Args;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::config::teams;
use crate::phone::types::OutboxMessage;

#[derive(Args)]
#[group(skip)]
#[command(group(
    clap::ArgGroup::new("send_payload")
        .required(true)
        .multiple(false)
        .args(["send", "command", "pty_input", "pty_input_stdin"])
))]
#[command(after_help = "\
DELIVERY MODES:\n  \
  wake            File messages inject into PTY and can spawn or respawn a persistent session. Logical PTY actions are capability- and idle-gated and can be terminally rejected before spawn.\n\n\
ROUTING: Before delivery, the CLI validates that the sender can reach the destination based on team \
membership and orchestrator rules (teams.json). If routing fails, the CLI exits immediately with code 1.\n\n\
DISCOVERY: Use `list-peers-lean` to get valid agent names for --to. The \"name\" field in the JSON output \
is the value to use.\n\n\
FILE-BASED MESSAGING: --send <filename> delivers a Markdown file. For Room \
replicas the file is resolved from <room-root>/messaging/<filename>. \
For the Root Agent the file is resolved from <root-agent-dir>/messaging/<filename>. \
`--send` is a filename only, never a path. Root Agent --to targets must be \
verified Room orchestrator replica names returned by list-peers-lean. \
Orchestrator --to targets may include the Root Agent canonical name \
`agentscommander://root-agent`; only identity-verified Room orchestrator \
replicas may use it.\n\n\
PRIVILEGED PTY INPUT: --pty-input and --pty-input-stdin submit validated exact UTF-8 text to one authorized coding-agent PTY. This never directly executes a host or container OS shell command. The caller's shell performs quoting and expansion before AC receives an argument, so prefer stdin for multiline, leading-hyphen, clipboard, process-list-sensitive, or otherwise sensitive text. `Queued` is not `Injected`; after a confirmation timeout keep the reported operation ID and do not resubmit under a new ID.\n\n\
DELIVERY CONFIRMATION: After queuing, send blocks up to --confirm-timeout seconds (default 90) \
waiting for the app's poller to confirm delivery. This bounds ONLY the synchronous confirmation \
handshake, not delivery itself: on confirmation timeout the CLI exits 1, but the message remains \
durably queued in the outbox and is typically still delivered afterwards (e.g. when wake must \
cold-spawn an idle peer). Exit 1 on confirmation timeout does NOT mean the message was lost; \
verify the outbox instead of re-sending. --confirm-timeout is distinct from --timeout, which \
bounds only the --get-output response wait. On enqueue, the CLI prints `Queued: <message-id>` to \
stdout; a missing `Queued:` line means the message was NOT enqueued.")]
pub struct SendArgs {
    /// Session token from AGENTSCOMMANDER_TOKEN. Shape-validated in the CLI;
    /// per-session authorization happens at the daemon mailbox. See `--help` TOKEN VALIDATION MODEL.
    #[arg(long)]
    pub token: Option<String>,

    /// Destination agent name (e.g., "repos/my-project"). Use `list-peers-lean` to discover valid names
    #[arg(long)]
    pub to: String,

    /// Filename (not path) of a message file that already exists in
    /// <room-root>/messaging/. Sender writes the file BEFORE calling send.
    /// Mutually exclusive with --command and the PTY input forms.
    #[arg(long)]
    pub send: Option<String>,

    /// Delivery mode (see DELIVERY MODES below)
    #[arg(long, default_value = "wake")]
    pub mode: String,

    /// Wait for and return the agent's response (blocks until reply or --timeout)
    #[arg(long, conflicts_with_all = ["pty_input", "pty_input_stdin"])]
    pub get_output: bool,

    /// Logical PTY action [possible values: clear, compact]. `clear` starts a
    /// fresh conversation: /new for an exact-stem direct Pi shell and /clear
    /// for direct Claude/Codex/Antigravity-family or Cursor agent shells. Pi compact
    /// is unsupported. The mapped session must be idle; unsupported mappings
    /// are terminally rejected before spawn. Not available from the Root Agent.
    /// Cannot be combined with --send
    #[arg(long)]
    pub command: Option<String>,

    /// Submit exact validated UTF-8 text to one authorized coding-agent PTY.
    /// The caller's shell performs quoting and expansion before AC receives the
    /// value. Prefer --pty-input-stdin for multiline, leading-hyphen, clipboard,
    /// process-list-sensitive, or otherwise sensitive text.
    #[arg(long, allow_hyphen_values = true)]
    pub pty_input: Option<OsString>,

    /// Read exact PTY input from stdin with a 65,536-byte UTF-8 ceiling.
    #[arg(long)]
    pub pty_input_stdin: bool,

    /// Configured agent id to use when `wake` spawns a new persistent session for
    /// the destination. `auto` picks the session's saved `lastCodingAgent`.
    #[arg(long, default_value = "auto")]
    pub agent: String,

    /// Profile slot letter A-Z applied to the coding agent the wake spawns or
    /// respawns. A disabled or missing letter walks down to the nearest enabled
    /// cell and the receipt reports `fallbackApplied`. Never writes the replica's
    /// tooling.profile/currentCodingAgent/lastCodingAgent. Not usable with PTY input.
    #[arg(long, conflicts_with_all = ["pty_input", "pty_input_stdin"])]
    pub profile: Option<String>,

    /// Timeout in seconds for the --get-output response wait (the
    /// delivery-confirmation wait is bounded separately by --confirm-timeout)
    #[arg(long, default_value = "300")]
    pub timeout: u64,

    /// Timeout in seconds for the delivery-confirmation wait, distinct from
    /// --timeout (which bounds the --get-output response wait). Bounds only the
    /// synchronous confirmation handshake: on timeout the CLI exits 1 but the
    /// message remains durably queued in the outbox and is typically still
    /// delivered. Exit 1 on confirmation timeout does NOT mean the message was
    /// lost; verify the outbox instead of re-sending
    #[arg(long, default_value = "90", value_parser = clap::value_parser!(u64).range(0..=3600))]
    pub confirm_timeout: u64,

    /// Agent root directory (required). Your working directory — used to derive your agent name
    #[arg(long)]
    pub root: Option<String>,

    /// Write message to a specific outbox directory instead of <root>/<local-dir>/outbox/
    #[arg(long, conflicts_with_all = ["pty_input", "pty_input_stdin"])]
    pub outbox: Option<String>,
}

/// Derive agent FQN from a path. Delegates to the canonical
/// `config::teams::agent_fqn_from_path` so WG replicas produce
/// `<project>:<wg>/<agent>` and origin agents produce `<project>/<agent>`.
///
/// Single source of truth — keep as a thin wrapper rather than a shadow copy.
pub(crate) fn agent_name_from_root(root: &str) -> String {
    crate::config::teams::agent_fqn_from_path(root)
}

pub(crate) fn sender_for_root(root: &str, root_is_root_agent: bool) -> String {
    if root_is_root_agent {
        crate::config::root_agent::ROOT_AGENT_SENDER.to_string()
    } else {
        agent_name_from_root(root)
    }
}

fn root_agent_target_allowed(target: &str, project_paths: &[String]) -> bool {
    crate::config::teams::verified_wg_coordinator_target(target, project_paths).is_some()
}

fn coordinator_to_root_target_allowed(sender: &str, project_paths: &[String]) -> bool {
    crate::config::teams::verified_wg_coordinator_target(sender, project_paths).is_some()
}

fn validate_root_agent_delivery_kind(
    root_is_root_agent: bool,
    command: Option<&str>,
) -> Result<(), &'static str> {
    if root_is_root_agent && command.is_some() {
        Err("Root Agent messaging is file-based; use --send with a root-to-orchestrator Markdown file, not --command")
    } else {
        Ok(())
    }
}

/// Delivery-confirmation ceiling for this invocation: `--confirm-timeout`
/// seconds, default 90 (#782). Single mapping point from the parsed flag to
/// the `Duration` that `execute` hands to `wait_for_delivery_confirmation`,
/// so tests can lock that an override actually reaches the helper.
fn confirm_timeout_from_args(args: &SendArgs) -> std::time::Duration {
    std::time::Duration::from_secs(args.confirm_timeout)
}

fn wait_for_delivery_confirmation(
    outbox_dir: &Path,
    msg_id: &str,
    mode_for_ack: &str,
    to_for_ack: &str,
    confirm_timeout: std::time::Duration,
    confirm_poll: std::time::Duration,
) -> Result<(), String> {
    let delivered_path = outbox_dir
        .join("delivered")
        .join(format!("{}.json", msg_id));
    let rejected_reason_path = outbox_dir
        .join("rejected")
        .join(format!("{}.reason.txt", msg_id));
    let start = std::time::Instant::now();

    loop {
        if delivered_path.exists() {
            crate::cli_println!(
                "Delivered: {} (mode={}, to={})",
                msg_id,
                mode_for_ack,
                to_for_ack
            );
            return Ok(());
        }
        if rejected_reason_path.exists() {
            let reason = std::fs::read_to_string(&rejected_reason_path)
                .unwrap_or_else(|_| "unknown reason".to_string());
            return Err(format!("message rejected: {}", reason.trim()));
        }
        if start.elapsed() >= confirm_timeout {
            return Err(format!(
                "delivery confirmation timeout after {}s (message {} may still be pending in {})",
                confirm_timeout.as_secs(),
                msg_id,
                outbox_dir.display()
            ));
        }
        std::thread::sleep(confirm_poll);
    }
}

/// Poll for the `--get-output` response file, returning its contents once it
/// appears. Mirrors `wait_for_delivery_confirmation`: the file is checked BEFORE
/// the timeout, so a response that lands in the final poll window (or with
/// `timeout == 0`) is still read instead of being dropped. Returns `Err` on
/// timeout or a read failure.
fn wait_for_response(
    response_path: &Path,
    timeout: std::time::Duration,
    poll_interval: std::time::Duration,
) -> Result<String, String> {
    let resp_start = std::time::Instant::now();

    loop {
        if response_path.exists() {
            return std::fs::read_to_string(response_path)
                .map_err(|e| format!("failed to read response file: {}", e));
        }
        if resp_start.elapsed() >= timeout {
            return Err(format!(
                "timeout waiting for response after {}s",
                timeout.as_secs()
            ));
        }
        std::thread::sleep(poll_interval);
    }
}

/// If `root` lives inside `<project_dir>/<Project AC Root>/wg-<N>-*/__agent_*/`,
/// return `project_dir` as a UTF-8 `String` (`None` if the shape does not match
/// or `project_dir` is not valid UTF-8, matching `list_peers::detect_wg_replica`
/// which also uses `to_str()` rather than `to_string_lossy()`).
///
/// Thin wrapper over `config::ac_root::wg_replica_layout_from_agent_dir`, the
/// single source of the WG-replica walk-up shared with
/// `list_peers::detect_wg_replica` and
/// `phone::mailbox::derive_project_from_outbox_path`, so `send` resolves WG-peer
/// targets against the same source `list-peers` reports as `reachable: true`.
/// See #228 / #726.
fn derive_root_project_dir(root: &str) -> Result<Option<String>, String> {
    let Some(canon) = std::fs::canonicalize(root).ok() else {
        return Ok(None);
    };
    match crate::config::ac_root::wg_replica_layout_from_agent_dir(&canon)? {
        Some(layout) => Ok(layout.project_dir.to_str().map(|path| path.to_string())),
        None => Ok(None),
    }
}

fn ensure_workgroup_root_is_authoritative(wg_root: &Path) -> Result<(), String> {
    let ac_root = wg_root.parent().ok_or_else(|| {
        format!(
            "room root '{}' has no parent Project AC Root directory",
            wg_root.display()
        )
    })?;
    crate::config::ac_root::ensure_authoritative_ac_root(ac_root)
}

/// v4 UUID string length. Request ids are `Uuid::new_v4().to_string()`, so the
/// `--get-output` response markers embed two 36-char ids.
const REQUEST_ID_LEN: usize = 36;

/// Byte width the injected PTY notification adds around the message body: the
/// plain wrap plus the sender name. The `get_output` branch additionally sizes
/// the `--get-output` response-marker framing (`PTY_RESPONSE_MARKER_FIXED` plus
/// two request ids), but that framing only reaches the PTY on a non-interactive
/// session (`phone::mailbox` gates it on `!interactive`, unreachable since 0.7.0).
/// The live clamp therefore calls this with `get_output = false`; the `true`
/// branch is inert future-proofing for that not-yet-reachable marker path and is
/// exercised only by the contract tests.
fn notification_pty_overhead(sender_len: usize, get_output: bool) -> usize {
    let mut overhead = crate::phone::messaging::PTY_WRAP_FIXED + sender_len;
    if get_output {
        overhead += crate::phone::messaging::PTY_RESPONSE_MARKER_FIXED + 2 * REQUEST_ID_LEN;
    }
    overhead
}

fn read_bounded_pty_input<R: Read>(reader: R) -> Result<String, &'static str> {
    let mut bytes = Vec::with_capacity(crate::pty::backend::PTY_INPUT_MAX_BYTES + 1);
    reader
        .take(crate::pty::backend::PTY_INPUT_MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "invalid_text")?;
    if bytes.len() > crate::pty::backend::PTY_INPUT_MAX_BYTES {
        return Err("payload_too_large");
    }
    String::from_utf8(bytes).map_err(|_| "invalid_text")
}

fn pty_text_from_args(args: &SendArgs) -> Result<Option<String>, &'static str> {
    if let Some(value) = args.pty_input.clone() {
        return value.into_string().map(Some).map_err(|_| "invalid_text");
    }
    if args.pty_input_stdin {
        let stdin = std::io::stdin();
        let lock = stdin.lock();
        return read_bounded_pty_input(lock).map(Some);
    }
    Ok(None)
}

struct SensitivePtyEnvelope(Vec<u8>);

fn publish_pty_request(
    outbox: &Path,
    injection_id: &str,
    envelope: &OutboxMessage,
) -> Result<PathBuf, &'static str> {
    let outbox_identity =
        crate::path_identity::verify_directory(outbox).map_err(|_| "unsafe_path")?;
    let bytes = serde_json::to_vec(envelope).map_err(|_| "invalid_envelope")?;
    if bytes.len() > crate::phone::types::PTY_INPUT_HOST_ENVELOPE_MAX_BYTES {
        return Err("invalid_envelope");
    }
    let sensitive = SensitivePtyEnvelope(bytes);
    let temp = outbox.join(format!(
        ".{injection_id}.{}.pty-input-request-tmp",
        Uuid::new_v4()
    ));
    let final_path = outbox.join(format!("{injection_id}.json"));
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
    if file.write_all(&sensitive.0).is_err() || file.flush().is_err() || file.sync_all().is_err() {
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
    let (published, _) = crate::path_identity::read_bounded_regular(
        &final_path,
        crate::phone::types::PTY_INPUT_HOST_ENVELOPE_MAX_BYTES,
    )
    .map_err(|_| "publish_ambiguous")?;
    if published != sensitive.0 {
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

struct ExpectedPtyArtifact<'a> {
    injection_id: &'a str,
    op_id: &'a str,
    sender: &'a str,
    target: &'a str,
    payload_bytes: u64,
    payload_sha256: &'a str,
    issued_at: &'a str,
    expires_at: &'a str,
    confirmation_tag: &'a str,
}

fn validate_pty_artifact(
    path: &Path,
    expected: &ExpectedPtyArtifact<'_>,
) -> Result<crate::phone::types::PtyInputHostArtifact, &'static str> {
    let (bytes, _) = crate::path_identity::read_bounded_regular(
        path,
        crate::phone::types::PTY_INPUT_METADATA_MAX_BYTES,
    )
    .map_err(|_| "invalid_artifact")?;
    let value =
        crate::path_identity::parse_json_no_duplicates(&bytes).map_err(|_| "invalid_artifact")?;
    let artifact: crate::phone::types::PtyInputHostArtifact =
        serde_json::from_value(value).map_err(|_| "invalid_artifact")?;
    let result = &artifact.result;
    crate::phone::types::validate_enqueued_pty_input_result(result)
        .map_err(|_| "invalid_artifact")?;
    if artifact.confirmation_tag != expected.confirmation_tag
        || result.version != crate::phone::types::PTY_INPUT_VERSION
        || result.injection_id != expected.injection_id
        || result.op_id.as_deref() != Some(expected.op_id)
        || result.sender.as_deref() != Some(expected.sender)
        || result.target.as_deref() != Some(expected.target)
        || result.payload_bytes != Some(expected.payload_bytes)
        || result.payload_sha256.as_deref() != Some(expected.payload_sha256)
        || result.source_plane != Some(crate::phone::types::PtyInputSourcePlane::HostCli)
        || result.issued_at.as_deref() != Some(expected.issued_at)
        || result.expires_at.as_deref() != Some(expected.expires_at)
        || result.terminal != result.status.is_terminal()
    {
        return Err("invalid_artifact");
    }
    Ok(artifact)
}

fn find_pty_input_terminal_artifact(
    outbox: &Path,
    expected: &ExpectedPtyArtifact<'_>,
) -> Result<Option<crate::phone::types::PtyInputResult>, &'static str> {
    let candidates = [
        (
            "delivered",
            crate::phone::types::PtyInputPublicStatus::Injected,
        ),
        (
            "rejected",
            crate::phone::types::PtyInputPublicStatus::Rejected,
        ),
        (
            "indeterminate",
            crate::phone::types::PtyInputPublicStatus::Indeterminate,
        ),
    ];
    for (directory, status) in candidates {
        let path = outbox
            .join(directory)
            .join(format!("{}.json", expected.injection_id));
        if path.exists() {
            let artifact = validate_pty_artifact(&path, expected)?;
            if artifact.result.status != status || !artifact.result.terminal {
                return Err("invalid_artifact");
            }
            return Ok(Some(artifact.result));
        }
    }
    Ok(None)
}

fn wait_for_pty_input_confirmation(
    outbox: &Path,
    expected: &ExpectedPtyArtifact<'_>,
    timeout: std::time::Duration,
) -> Result<crate::phone::types::PtyInputResult, &'static str> {
    let start = std::time::Instant::now();
    loop {
        if let Some(result) = find_pty_input_terminal_artifact(outbox, expected)? {
            return Ok(result);
        }
        if start.elapsed() >= timeout {
            return Err("confirmation_timeout");
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}

fn pty_effective_project_paths(root: &str) -> Result<Vec<String>, String> {
    let mut paths =
        crate::config::settings::read_pty_input_project_paths_strict()?.unwrap_or_default();
    if let Some(project) = derive_root_project_dir(root)? {
        let canonical = std::fs::canonicalize(&project).ok();
        if !paths.iter().any(|candidate| {
            candidate == &project
                || canonical.as_ref().is_some_and(|value| {
                    std::fs::canonicalize(candidate).ok().as_ref() == Some(value)
                })
        }) {
            paths.push(project);
        }
    }
    Ok(paths)
}

fn execute_pty_input(args: SendArgs, root: String, text: String) -> i32 {
    use crate::phone::types::{
        canonical_pty_timestamp, pty_input_confirmation_tag, sha256_hex, PtyInputEnterMode,
        PtyInputPublicStatus, PtyInputWirePayload, PTY_INPUT_TTL_SECS, PTY_INPUT_VERSION,
    };

    if args.mode != "wake" || args.get_output || args.outbox.is_some() {
        eprintln!("Error: mixed_payload");
        return 1;
    }
    if let Err(error) = crate::pty::inject::validate_pty_input_text(&text) {
        eprintln!("Error: {error}");
        return 1;
    }
    let requested_agent = if args.agent == "auto" {
        None
    } else {
        if crate::config::coding_agent_mutations::validate_custom_agent_id(&args.agent).is_err() {
            eprintln!("Error: unsupported_profile");
            return 1;
        }
        Some(args.agent.clone())
    };
    let (token, master_or_root_credential) = match crate::cli::validate_cli_token(&args.token) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("{error}");
            return 1;
        }
    };
    if master_or_root_credential {
        eprintln!("Error: session_token_required");
        return 1;
    }
    let paths = match pty_effective_project_paths(&root) {
        Ok(paths) => paths,
        Err(_) => {
            eprintln!("Error: sender_identity_invalid");
            return 1;
        }
    };
    let sender_is_root = crate::config::root_agent::is_root_agent_path(&root);
    let route = match crate::config::teams::verify_pty_input_route(
        Path::new(&root),
        sender_is_root,
        &args.to,
        &paths,
    ) {
        Ok(route) => route,
        Err(code) => {
            eprintln!("Error: {code}");
            return 1;
        }
    };
    let supplied_root = match crate::path_identity::verify_directory(Path::new(&root)) {
        Ok(identity) => identity,
        Err(_) => {
            eprintln!("Error: unsafe_path");
            return 1;
        }
    };
    if !crate::path_identity::same_object(&supplied_root, &route.sender.replica_identity) {
        eprintln!("Error: sender_identity_invalid");
        return 1;
    }

    let injection_id = Uuid::new_v4().to_string();
    let op_id = injection_id.clone();
    let nonce = Uuid::new_v4().to_string();
    let issued = chrono::Utc::now();
    let issued_at = canonical_pty_timestamp(issued);
    let expires_at =
        canonical_pty_timestamp(issued + chrono::Duration::seconds(PTY_INPUT_TTL_SECS));
    let confirmation_tag = pty_input_confirmation_tag(&injection_id, &op_id, &nonce);
    let digest = sha256_hex(text.as_bytes());
    let payload_bytes = text.len() as u64;

    crate::cli_println!("Operation ID: {injection_id}");
    if std::io::stdout().flush().is_err() {
        eprintln!("Error: publish_failed");
        return 1;
    }

    let message = OutboxMessage {
        id: injection_id.clone(),
        token: Some(token),
        from: route.sender.canonical_fqn.clone(),
        to: route.target.canonical_fqn.clone(),
        body: String::new(),
        mode: "wake".to_string(),
        get_output: false,
        request_id: None,
        sender_agent: None,
        preferred_agent: String::new(),
        requested_profile: None,
        effective_agent_id: None,
        effective_profile: None,
        profile_fallback_applied: false,
        dispatch_not_applied: None,
        priority: "normal".to_string(),
        timestamp: issued_at.clone(),
        command: None,
        action: Some("pty-input".to_string()),
        target: None,
        force: None,
        timeout_secs: None,
        switch_coding_agent: None,
        switch_profile: None,
        dry_run: None,
        quiet_period_ms: None,
        pty_input: Some(PtyInputWirePayload {
            version: PTY_INPUT_VERSION,
            text,
            enter: PtyInputEnterMode::AgentSubmit,
            injection_id: injection_id.clone(),
            op_id: op_id.clone(),
            issued_at: issued_at.clone(),
            expires_at: expires_at.clone(),
            nonce,
            agent_id: requested_agent,
        }),
    };

    let expected = ExpectedPtyArtifact {
        injection_id: &injection_id,
        op_id: &op_id,
        sender: &route.sender.canonical_fqn,
        target: &route.target.canonical_fqn,
        payload_bytes,
        payload_sha256: &digest,
        issued_at: &issued_at,
        expires_at: &expires_at,
        confirmation_tag: &confirmation_tag,
    };

    let local_dir = PathBuf::from(&root).join(crate::config::agent_local_dir_name());
    if crate::path_identity::verify_directory(&local_dir).is_err() {
        eprintln!("Error: unsafe_path operation={injection_id}");
        return 1;
    }
    let outbox = local_dir.join("outbox");
    if !outbox.exists() && std::fs::create_dir(&outbox).is_err() {
        eprintln!("Error: publish_failed operation={injection_id}");
        return 1;
    }
    if let Err(code) = publish_pty_request(&outbox, &injection_id, &message) {
        let final_exists =
            crate::path_identity::verify_regular_file(&outbox.join(format!("{injection_id}.json")))
                .is_ok();
        let correlated_artifact = find_pty_input_terminal_artifact(&outbox, &expected)
            .ok()
            .flatten()
            .is_some();
        eprintln!(
            "Indeterminate: {injection_id} code={code} published={final_exists} correlatedArtifact={correlated_artifact}; do not resubmit under a new ID"
        );
        return 1;
    }
    crate::cli_println!(
        "Queued: {injection_id} (queued is not injected; waiting for terminal status)"
    );
    match wait_for_pty_input_confirmation(
        &outbox,
        &expected,
        std::time::Duration::from_secs(args.confirm_timeout),
    ) {
        Ok(result) => match result.status {
            PtyInputPublicStatus::Injected => {
                crate::cli_println!("Injected: {injection_id}");
                0
            }
            PtyInputPublicStatus::Rejected => {
                let code = result
                    .reason
                    .as_ref()
                    .map(|reason| reason_code_for_cli(reason.code))
                    .unwrap_or("invalid_envelope");
                eprintln!("Rejected: {injection_id} code={code}");
                1
            }
            PtyInputPublicStatus::Indeterminate => {
                let code = result
                    .reason
                    .as_ref()
                    .map(|reason| reason_code_for_cli(reason.code))
                    .unwrap_or("terminal_store_failed");
                eprintln!("Indeterminate: {injection_id} code={code}; do not resubmit");
                1
            }
            _ => {
                eprintln!("Indeterminate: {injection_id} code=invalid_artifact");
                1
            }
        },
        Err("confirmation_timeout") => {
            eprintln!(
                "Confirmation timeout: operation {injection_id} remains queued or actuating; do not resubmit under a new ID. Inspect its delivered, rejected, and indeterminate artifacts."
            );
            1
        }
        Err(code) => {
            eprintln!("Indeterminate: {injection_id} code={code}; do not resubmit");
            1
        }
    }
}

fn reason_code_for_cli(code: crate::phone::types::PtyInputReasonCode) -> &'static str {
    use crate::phone::types::PtyInputReasonCode as C;
    match code {
        C::InvalidEnvelope => "invalid_envelope",
        C::MixedPayload => "mixed_payload",
        C::UnsupportedVersion => "unsupported_version",
        C::InvalidEnterMode => "invalid_enter_mode",
        C::InvalidId => "invalid_id",
        C::InvalidNonce => "invalid_nonce",
        C::InvalidTimestamp => "invalid_timestamp",
        C::Expired => "expired",
        C::InvalidTarget => "invalid_target",
        C::InvalidText => "invalid_text",
        C::PayloadTooLarge => "payload_too_large",
        C::IdempotencyConflict => "idempotency_conflict",
        C::CapacityExceeded => "capacity_exceeded",
        C::SessionTokenRequired => "session_token_required",
        C::InvalidSessionToken => "invalid_session_token",
        C::AmbiguousSessionToken => "ambiguous_session_token",
        C::SenderSessionNotLive => "sender_session_not_live",
        C::SenderBackendNotLocal => "sender_backend_not_local",
        C::SenderIdentityInvalid => "sender_identity_invalid",
        C::SenderNotCoordinator => "sender_not_coordinator",
        C::RootIdentityInvalid => "root_identity_invalid",
        C::TargetNotMember => "target_not_member",
        C::TargetIsCoordinator => "target_is_coordinator",
        C::TargetOutOfScope => "target_out_of_scope",
        C::UnsafePath => "unsafe_path",
        C::ApiScopeRequired => "api_scope_required",
        C::ApiClientUnbound => "api_client_unbound",
        C::ApiClientStale => "api_client_stale",
        C::ApiBindingMismatch => "api_binding_mismatch",
        C::AuthorityChanged => "authority_changed",
        C::Busy => "busy",
        C::ResizeUnsettled => "resize_unsettled",
        C::UntrackedReadiness => "untracked_readiness",
        C::UnsupportedSession => "unsupported_session",
        C::NonpersistentLiveSession => "nonpersistent_live_session",
        C::InconsistentSession => "inconsistent_session",
        C::UnsupportedProfile => "unsupported_profile",
        C::ReadinessTimeout => "readiness_timeout",
        C::StoreCorrupt => "store_corrupt",
        C::RestoreInProgress => "restore_in_progress",
        C::PurgeInProgress => "purge_in_progress",
        C::SessionRace => "session_race",
        C::LeaseLost => "lease_lost",
        C::SpawnFailedSafe => "spawn_failed_safe",
        C::StoreTransient => "store_transient",
        C::FinalRevalidationFailed => "final_revalidation_failed",
        C::TextWriteFailed => "text_write_failed",
        C::RequiredEnterFailed => "required_enter_failed",
        C::DaemonRestartAfterActuation => "daemon_restart_after_actuation",
        C::RuntimeActuationOrphan => "runtime_actuation_orphan",
        C::TerminalStoreFailed => "terminal_store_failed",
        C::RedundantEnterFailed => "redundant_enter_failed",
        C::BoundaryMetadataFailed => "boundary_metadata_failed",
        C::ArtifactUnclaimed => "artifact_unclaimed",
    }
}

/// #1638: `Queued:` receipt line. The message id remains the first token in
/// both shapes (scripts that parse `Queued: <id>` keep working). Requested
/// (not effective) values are printed here — effective values are only
/// knowable at delivery time and are printed by the `Delivered:` line in
/// phase 1635-2.
fn queued_receipt_line(
    msg_id: &str,
    agent_for_ack: &str,
    requested_profile: &Option<String>,
) -> String {
    if agent_for_ack != "auto" || requested_profile.is_some() {
        let mut extra = format!("agent={}", agent_for_ack);
        if let Some(letter) = requested_profile.as_deref() {
            extra.push_str(&format!(", profile={}", letter));
        }
        format!("Queued: {} ({})", msg_id, extra)
    } else {
        format!("Queued: {}", msg_id)
    }
}

pub fn execute(args: SendArgs) -> i32 {
    let root = match args.root {
        Some(ref r) => r.clone(),
        None => {
            eprintln!("Error: --root is required. Specify your agent's root directory.");
            return 1;
        }
    };
    let pty_text = match pty_text_from_args(&args) {
        Ok(value) => value,
        Err(code) => {
            eprintln!("Error: {code}");
            return 1;
        }
    };
    if let Some(text) = pty_text {
        return execute_pty_input(args, root, text);
    }

    let root_is_root_agent = crate::config::root_agent::is_root_agent_path(&root);
    if let Err(reason) =
        validate_root_agent_delivery_kind(root_is_root_agent, args.command.as_deref())
    {
        eprintln!("Error: {}", reason);
        return 1;
    }

    // Validate token before proceeding
    let is_root = match crate::cli::validate_cli_token(&args.token) {
        Ok((_token, root)) => root,
        Err(msg) => {
            eprintln!("{}", msg);
            return 1;
        }
    };

    let sender = sender_for_root(&root, root_is_root_agent);
    let ac_dir = PathBuf::from(&root).join(crate::config::agent_local_dir_name());

    // Validate mode — "queue" is no longer supported
    let valid_modes = ["wake"];
    if !valid_modes.contains(&args.mode.as_str()) {
        eprintln!(
            "Error: invalid mode '{}'. Valid: {}",
            args.mode,
            valid_modes.join(", ")
        );
        return 1;
    }

    // ── Resolve --to against known projects (Decision 2 / §AR2-shared) ─────
    //
    // Qualified FQN → validated shape + existence. Unqualified WG-local →
    // two-level scan, unambiguous → canonical FQN, ambiguous → reject with
    // candidate list, unknown → reject. Origin/bare → pass through.
    //
    // CLI-side resolution is belt-and-braces (§DR1); the mailbox also
    // canonicalizes on receive (§AR2-norm) so direct outbox writes cannot
    // bypass the reject-on-ambiguity rule.
    let settings = crate::config::settings::load_settings();
    // Build an in-memory project-path slice that includes the project
    // derived from --root (if any). Mirrors list-peers's WG-replica walk-up
    // discovery so qualified WG-peer targets that list-peers reports as
    // reachable do not fail send with `not found in any known project`.
    // settings.json is NOT modified. See #228 / plan _plans/228-cli-daemon-laterals.md.
    let mut effective_project_paths = settings.project_paths.clone();
    let root_project = match derive_root_project_dir(&root) {
        Ok(root_project) => root_project,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };
    if let Some(root_project) = root_project {
        // Canonicalize the target once. Compare by string first and only fall
        // back to a per-path canonicalize syscall when the strings differ, so the
        // common "project already registered" case does no extra filesystem work.
        // If canonicalizing `root_project` itself fails (rare: it resolved
        // milliseconds ago inside `derive_root_project_dir`), the string check
        // still catches an exact duplicate.
        let canon_root_project = std::fs::canonicalize(&root_project).ok();
        let already_present = effective_project_paths.iter().any(|p| {
            if p == &root_project {
                return true;
            }
            match &canon_root_project {
                Some(canon_target) => std::fs::canonicalize(p).ok().as_ref() == Some(canon_target),
                None => false,
            }
        });
        if !already_present {
            effective_project_paths.push(root_project);
        }
    }
    let resolved_to =
        match crate::config::teams::resolve_agent_target(&args.to, &effective_project_paths) {
            Ok(fqn) => fqn,
            Err(e) => {
                eprintln!("Error: {}", e);
                return 1;
            }
        };

    // ── Pre-validate routing ──────────────────────────────────────────────

    if root_is_root_agent {
        if !root_agent_target_allowed(&resolved_to, &effective_project_paths) {
            eprintln!(
                "Error: root-agent routing rejected — '{}' is not a verified Room orchestrator replica. Use list-peers-lean from the Root Agent and pass one of its name values.",
                resolved_to
            );
            return 1;
        }
    } else if crate::config::root_agent::is_root_agent_target(&resolved_to) {
        // #293 — coordinator → root. Only identity-verified WG coordinators
        // are allowed to address the Root Agent. Master/root token still goes
        // through this gate intentionally: the URI is meaningful only when
        // paired with a real coordinator identity, and the verified check is
        // cheap.
        if !coordinator_to_root_target_allowed(&sender, &effective_project_paths) {
            eprintln!(
                "Error: routing rejected — '{}' is not a verified Room orchestrator replica and cannot message '{}'. Replies to the Root Agent are reserved for verified Room orchestrators.",
                sender,
                crate::config::root_agent::ROOT_AGENT_SENDER
            );
            return 1;
        }
    } else if !is_root {
        // Load discovered teams and check if sender can reach destination BEFORE
        // writing to outbox. Fail immediately with a clear error if not.
        let discovered = teams::discover_teams();
        if !teams::can_communicate(&sender, &resolved_to, &discovered) {
            eprintln!(
                "Error: routing rejected — '{}' cannot reach '{}'. \
                 Check team membership and orchestrator rules.",
                sender, resolved_to
            );
            return 1;
        }
    }

    // --send + --command mutually exclusive (P0-3)
    if args.send.is_some() && args.command.is_some() {
        eprintln!("Error: --send and --command are mutually exclusive");
        return 1;
    }

    // Resolve message body from --send (file-based messaging per plan §4.1 [r2])
    let message_body = if let Some(ref filename) = args.send {
        if let Err(e) = crate::phone::messaging::validate_filename_only(filename) {
            eprintln!("Error: {}", e);
            return 1;
        }
        let agent_root_path = std::path::Path::new(&root);
        let msg_dir = if root_is_root_agent {
            match crate::phone::messaging::root_messaging_dir(agent_root_path) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("Error: failed to resolve root messaging dir: {}", e);
                    return 1;
                }
            }
        } else {
            let wg_root = match crate::phone::messaging::workgroup_root(agent_root_path) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!(
                        "Error: --send requires --root under a `room-*` or legacy `wg-*` Room directory unless --root is the canonical Root Agent directory; {}",
                        e
                    );
                    return 1;
                }
            };
            if let Err(e) = ensure_workgroup_root_is_authoritative(&wg_root) {
                eprintln!("Error: {}", e);
                return 1;
            }
            match crate::phone::messaging::messaging_dir(&wg_root) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("Error: failed to resolve messaging dir: {}", e);
                    return 1;
                }
            }
        };
        let abs = match crate::phone::messaging::resolve_existing_message(&msg_dir, filename) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("Error: {}", e);
                return 1;
            }
        };

        // UNC-strip only at the single emission site (plan §13.4).
        let abs_str = abs.to_string_lossy();
        let abs_display = abs_str.trim_start_matches(r"\\?\");
        let body = crate::phone::messaging::format_file_notification(abs_display);

        // Pre-wrap long-body warn (plan §13.4 §7.9).
        if body.len() > 200 {
            log::warn!(
                "[send] notification body length {} is unusually long",
                body.len()
            );
        }

        // PTY_SAFE_MAX clamp. The wrap no longer embeds wg_root or bin_path, so
        // the overhead is the fixed framing plus `from`. The live wake path never
        // injects the `--get-output` response markers (`phone::mailbox` gates them
        // on a non-interactive session, unreachable today), so budget the plain
        // wrap here by passing `false`. Keying this on `args.get_output` would flip
        // a near-limit long path from delivered to rejected over marker bytes that
        // never reach the PTY.
        let overhead = notification_pty_overhead(sender.len(), false);
        if body.len() + overhead > crate::phone::messaging::PTY_SAFE_MAX {
            eprintln!(
                "Error: notification exceeds PTY-safe length (body {} + overhead {} > {}). \
                 Shorten slug or move room to a shallower path.",
                body.len(),
                overhead,
                crate::phone::messaging::PTY_SAFE_MAX
            );
            return 1;
        }

        body
    } else {
        String::new()
    };

    // Require at least --send or --command
    if message_body.is_empty() && args.command.is_none() {
        eprintln!("Error: --send or --command is required");
        return 1;
    }

    // Validate --command if present
    const ALLOWED_COMMANDS: &[&str] = &["clear", "compact"];
    if let Some(ref cmd) = args.command {
        if !ALLOWED_COMMANDS.contains(&cmd.as_str()) {
            eprintln!(
                "Error: unsupported command '{}'. Allowed: {}",
                cmd,
                ALLOWED_COMMANDS.join(", ")
            );
            return 1;
        }
    }

    let msg_id = Uuid::new_v4().to_string();
    let request_id = if args.get_output {
        Some(Uuid::new_v4().to_string())
    } else {
        None
    };

    let mode_for_ack = args.mode.clone();
    let to_for_ack = resolved_to.clone();
    // #1638: clone the agent id for the enqueue receipt before the message
    // literal moves `args.agent` (precedent: `mode_for_ack` above).
    let agent_for_ack = args.agent.clone();
    // Resolve before `args` fields move into OutboxMessage below; the whole-struct
    // borrow would be rejected after the partial moves.
    let confirm_timeout = confirm_timeout_from_args(&args);

    let requested_profile = match args.profile.as_deref() {
        None => None,
        Some(raw) => match crate::config::settings::normalize_profile_letter(raw) {
            Some(letter) => Some(letter),
            None => {
                eprintln!("Error: --profile must be a single letter A through Z");
                return 1;
            }
        },
    };
    // #1638: clone for the receipt line — the message literal below moves
    // `requested_profile` into the outbox message.
    let requested_profile_for_ack = requested_profile.clone();

    let message = OutboxMessage {
        id: msg_id.clone(),
        token: args.token,
        from: sender.clone(),
        to: resolved_to,
        body: message_body,
        mode: args.mode,
        get_output: args.get_output,
        request_id: request_id.clone(),
        sender_agent: None,
        preferred_agent: args.agent,
        requested_profile,
        effective_agent_id: None,
        effective_profile: None,
        profile_fallback_applied: false,
        dispatch_not_applied: None,
        priority: "normal".to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        command: args.command,
        action: None,
        target: None,
        force: None,
        timeout_secs: None,
        switch_coding_agent: None,
        switch_profile: None,
        dry_run: None,
        quiet_period_ms: None,
        pty_input: None,
    };

    // Write to --outbox if specified, app outbox if root/master token, otherwise <root>/<local_dir>/outbox/
    let outbox_dir = if let Some(ref outbox_path) = args.outbox {
        PathBuf::from(outbox_path)
    } else if is_root {
        // Root/master token: use the app outbox so the MailboxPoller always finds it
        let app_outbox = crate::config::config_dir()
            .map(|d| d.join("app-outbox-path.txt"))
            .and_then(|p| std::fs::read_to_string(&p).ok())
            .map(|s| PathBuf::from(s.trim()));
        match app_outbox {
            Some(p) if p.is_dir() => p,
            _ => ac_dir.join("outbox"),
        }
    } else {
        ac_dir.join("outbox")
    };
    if let Err(e) = std::fs::create_dir_all(&outbox_dir) {
        eprintln!("Error: failed to create outbox directory: {}", e);
        return 1;
    }

    let outbox_path = outbox_dir.join(format!("{}.json", msg_id));
    let json = match serde_json::to_string_pretty(&message) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("Error: failed to serialize message: {}", e);
            return 1;
        }
    };

    if let Err(e) = std::fs::write(&outbox_path, json) {
        eprintln!("Error: failed to write outbox file: {}", e);
        return 1;
    }
    // #1596: enqueue receipt on stdout — the only observable signal when the
    // caller cannot see the GUI-subsystem binary's exit code (PowerShell
    // direct capture). A missing `Queued:` line means NOT enqueued.
    crate::cli_println!(
        "{}",
        queued_receipt_line(&msg_id, &agent_for_ack, &requested_profile_for_ack)
    );
    log::info!(
        "[send] queued message {} to '{}' in {}",
        msg_id,
        to_for_ack,
        outbox_dir.display()
    );

    // ── Poll for delivery confirmation ────────────────────────────────────
    // The MailboxPoller will pick up the file and move it to delivered/ or
    // rejected/. Wait until we know the outcome.
    if let Err(e) = wait_for_delivery_confirmation(
        &outbox_dir,
        &msg_id,
        &mode_for_ack,
        &to_for_ack,
        confirm_timeout,
        std::time::Duration::from_millis(250),
    ) {
        log::warn!("[send] {}", e);
        eprintln!("Error: {}", e);
        return 1;
    }

    // ── If --get-output, wait for response after confirmed delivery ───────
    if let Some(rid) = request_id {
        let responses_dir = ac_dir.join("responses");
        let response_path = responses_dir.join(format!("{}.json", rid));
        let timeout = std::time::Duration::from_secs(args.timeout);
        let poll_interval = std::time::Duration::from_secs(2);

        crate::cli_println!("Waiting for response (timeout: {}s)...", args.timeout);

        match wait_for_response(&response_path, timeout, poll_interval) {
            Ok(content) => {
                crate::cli_println!("{}", content);
                return 0;
            }
            Err(e) => {
                eprintln!("Error: {}", e);
                return 1;
            }
        }
    }

    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_after_help_documents_enqueue_receipt() {
        // #1596: the after_help must document the `Queued:` enqueue receipt so
        // callers know a missing receipt line means NOT enqueued.
        use clap::CommandFactory;
        let cmd = crate::cli::Cli::command();
        let send = cmd
            .get_subcommands()
            .find(|c| c.get_name() == "send")
            .expect("send subcommand");
        let after = send
            .get_after_help()
            .expect("after_help present")
            .to_string();
        assert!(
            after.contains("On enqueue, the CLI prints `Queued: <message-id>` to stdout"),
            "after_help missing enqueue-receipt sentence"
        );
        assert!(
            after.contains("a missing `Queued:` line means the message was NOT enqueued"),
            "after_help missing NOT-enqueued sentence"
        );
    }

    fn make_verified_coordinator_fixture() -> (tempfile::TempDir, Vec<String>) {
        let temp = tempfile::TempDir::new().unwrap();
        let project = temp.path().join("proj-a");
        let ac_root = project.join(".ac");
        let team_dir = ac_root.join("_team_dev-team");
        let origin_tech_lead = ac_root.join("_agent_tech-lead");
        let origin_dev_rust = ac_root.join("_agent_dev-rust");
        let wg_dir = ac_root.join("wg-1-dev-team");
        let tech_lead_replica = wg_dir.join("__agent_tech-lead");
        let dev_rust_replica = wg_dir.join("__agent_dev-rust");

        for dir in [
            &team_dir,
            &origin_tech_lead,
            &origin_dev_rust,
            &tech_lead_replica,
            &dev_rust_replica,
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }

        std::fs::write(
            team_dir.join("config.json"),
            r#"{"agents":["../_agent_dev-rust"],"coordinator":"../_agent_tech-lead"}"#,
        )
        .unwrap();
        std::fs::write(
            tech_lead_replica.join("config.json"),
            r#"{"identity":"../../_agent_tech-lead"}"#,
        )
        .unwrap();
        std::fs::write(
            dev_rust_replica.join("config.json"),
            r#"{"identity":"../../_agent_dev-rust"}"#,
        )
        .unwrap();

        let paths = vec![temp.path().to_string_lossy().to_string()];
        (temp, paths)
    }

    #[test]
    fn root_agent_sender_uses_reserved_constant() {
        assert_eq!(
            sender_for_root("C:/tmp/agentscommander/ac-root-agent", true),
            crate::config::root_agent::ROOT_AGENT_SENDER
        );
        assert_ne!(
            sender_for_root("C:/tmp/agentscommander/_agent_root-agent", false),
            crate::config::root_agent::ROOT_AGENT_SENDER
        );
    }

    #[test]
    fn root_agent_routing_rejects_origin_coordinator() {
        let (_temp, paths) = make_verified_coordinator_fixture();

        assert!(!root_agent_target_allowed("proj-a/tech-lead", &paths));
    }

    #[test]
    fn send_rejects_send_path_for_wg() {
        let filename = r"messaging\20260524-040000-wg1-a-to-wg1-b-x.md";

        assert!(crate::phone::messaging::validate_filename_only(filename).is_err());
    }

    #[test]
    fn send_rejects_send_path_for_root() {
        let filename = r"messaging\20260524-040000-root-to-wg1-tech-lead-x.md";

        assert!(crate::phone::messaging::validate_filename_only(filename).is_err());
    }

    #[test]
    fn root_agent_send_accepts_verified_wg_coordinator() {
        let (_temp, paths) = make_verified_coordinator_fixture();

        assert!(root_agent_target_allowed(
            "proj-a:wg-1-dev-team/tech-lead",
            &paths
        ));
    }

    #[test]
    fn coordinator_to_root_target_allowed_accepts_verified_coordinator() {
        let (_temp, paths) = make_verified_coordinator_fixture();
        assert!(coordinator_to_root_target_allowed(
            "proj-a:wg-1-dev-team/tech-lead",
            &paths
        ));
    }

    #[test]
    fn coordinator_to_root_target_allowed_rejects_non_coordinator() {
        let (_temp, paths) = make_verified_coordinator_fixture();
        assert!(!coordinator_to_root_target_allowed(
            "proj-a:wg-1-dev-team/dev-rust",
            &paths
        ));
    }

    #[test]
    fn coordinator_to_root_target_allowed_rejects_origin_agent() {
        let (_temp, paths) = make_verified_coordinator_fixture();
        assert!(!coordinator_to_root_target_allowed(
            "proj-a/tech-lead",
            &paths
        ));
    }

    #[test]
    fn root_agent_command_clear_and_compact_are_rejected_before_outbox_write() {
        for command in ["clear", "compact"] {
            assert_eq!(
                validate_root_agent_delivery_kind(true, Some(command)),
                Err("Root Agent messaging is file-based; use --send with a root-to-orchestrator Markdown file, not --command")
            );
        }
    }

    #[test]
    fn non_root_command_behavior_is_unchanged() {
        assert_eq!(
            validate_root_agent_delivery_kind(false, Some("compact")),
            Ok(())
        );
    }

    #[test]
    fn get_output_does_not_tighten_live_pty_clamp() {
        // The live wake path never injects the `--get-output` response markers
        // (`phone::mailbox` gates them on a non-interactive session, unreachable
        // today), so the CLI clamp must budget a `--get-output` send exactly like
        // a plain send. The live clamp calls `notification_pty_overhead(_, false)`;
        // this locks that a near-limit path is not rejected over marker bytes that
        // never reach the PTY, guarding against re-keying it on `args.get_output`.
        let sender_len = "project:wg-1-team/agent".len();
        let live_overhead = notification_pty_overhead(sender_len, false);
        let plain_overhead = crate::phone::messaging::PTY_WRAP_FIXED + sender_len;

        // Live budget is the plain wrap: get-output adds nothing on the live path.
        assert_eq!(live_overhead, plain_overhead);

        // A body sized to exactly fill the plain-wrap budget must be ACCEPTED by
        // the clamp (`body.len() + overhead > PTY_SAFE_MAX` is false).
        let body_len = crate::phone::messaging::PTY_SAFE_MAX - plain_overhead;
        assert!(body_len + live_overhead <= crate::phone::messaging::PTY_SAFE_MAX);

        // Under the reverted tightening, counting the never-injected marker
        // overhead would have rejected this same body. Confirm the corrected live
        // path does not, so the tightening cannot silently return.
        let tightened_overhead = plain_overhead
            + crate::phone::messaging::PTY_RESPONSE_MARKER_FIXED
            + 2 * REQUEST_ID_LEN;
        assert!(body_len + tightened_overhead > crate::phone::messaging::PTY_SAFE_MAX);
    }

    #[test]
    fn wait_for_delivery_confirmation_accepts_delivered_file() {
        let temp = tempfile::TempDir::new().unwrap();
        let delivered_dir = temp.path().join("delivered");
        std::fs::create_dir_all(&delivered_dir).unwrap();
        std::fs::write(delivered_dir.join("msg-1.json"), "{}").unwrap();

        let result = wait_for_delivery_confirmation(
            temp.path(),
            "msg-1",
            "wake",
            "project:wg-1-team/agent",
            std::time::Duration::from_millis(10),
            std::time::Duration::from_millis(1),
        );

        assert!(result.is_ok());
    }

    #[test]
    fn wait_for_delivery_confirmation_reports_rejection_reason() {
        let temp = tempfile::TempDir::new().unwrap();
        let rejected_dir = temp.path().join("rejected");
        std::fs::create_dir_all(&rejected_dir).unwrap();
        std::fs::write(rejected_dir.join("msg-2.reason.txt"), "bad route").unwrap();

        let err = wait_for_delivery_confirmation(
            temp.path(),
            "msg-2",
            "wake",
            "project:wg-1-team/agent",
            std::time::Duration::from_millis(10),
            std::time::Duration::from_millis(1),
        )
        .unwrap_err();

        assert!(err.contains("message rejected: bad route"));
    }

    #[test]
    fn wait_for_delivery_confirmation_reports_pending_outbox_on_timeout() {
        let temp = tempfile::TempDir::new().unwrap();

        let err = wait_for_delivery_confirmation(
            temp.path(),
            "msg-3",
            "wake",
            "project:wg-1-team/agent",
            std::time::Duration::from_millis(1),
            std::time::Duration::from_millis(1),
        )
        .unwrap_err();

        assert!(err.contains("delivery confirmation timeout"));
        assert!(err.contains("msg-3"));
        assert!(err.contains(&temp.path().display().to_string()));
    }

    #[test]
    fn wait_for_response_returns_present_file_at_zero_timeout() {
        // Regression lock for the ordering bug: a response already on disk must
        // be read even when the timeout has fully elapsed (timeout == 0). The old
        // timeout-first ordering dropped it and returned a timeout error instead.
        let temp = tempfile::TempDir::new().unwrap();
        let response_path = temp.path().join("resp.json");
        std::fs::write(&response_path, "{\"ok\":true}").unwrap();

        let result = wait_for_response(
            &response_path,
            std::time::Duration::from_secs(0),
            std::time::Duration::from_millis(1),
        );

        assert_eq!(result.unwrap(), "{\"ok\":true}");
    }

    #[test]
    fn wait_for_response_returns_present_file_within_window() {
        let temp = tempfile::TempDir::new().unwrap();
        let response_path = temp.path().join("resp.json");
        std::fs::write(&response_path, "hello").unwrap();

        let result = wait_for_response(
            &response_path,
            std::time::Duration::from_millis(50),
            std::time::Duration::from_millis(1),
        );

        assert_eq!(result.unwrap(), "hello");
    }

    #[test]
    fn wait_for_response_times_out_when_absent() {
        // The timeout must still fire (no infinite loop) when no file appears.
        let temp = tempfile::TempDir::new().unwrap();
        let missing = temp.path().join("never.json");

        let err = wait_for_response(
            &missing,
            std::time::Duration::from_millis(5),
            std::time::Duration::from_millis(1),
        )
        .unwrap_err();

        assert!(err.contains("timeout waiting for response"));
    }

    #[test]
    fn derive_root_project_dir_walks_up_wg_replica_path() {
        let temp = tempfile::TempDir::new().unwrap();
        let project = temp.path().join("proj-x");
        let agent_root = project.join(".ac").join("wg-1-devs").join("__agent_alice");
        std::fs::create_dir_all(&agent_root).unwrap();

        // On Windows, std::fs::canonicalize returns `\\?\C:\...` extended-length
        // paths; compute the expected via canonicalize so the assertion holds on
        // both Windows and Unix. Mirror pattern from list_peers.rs:972-1010
        // (`extended_length_prefix_normalizes`).
        let expected = std::fs::canonicalize(&project)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let got = derive_root_project_dir(agent_root.to_str().unwrap())
            .unwrap()
            .expect("should derive project from WG replica path");
        assert_eq!(got, expected);
    }

    #[test]
    fn derive_root_project_dir_accepts_ac_root() {
        let temp = tempfile::TempDir::new().unwrap();
        let project = temp.path().join("proj-x");
        let root = project.join(".ac").join("wg-1-devs").join("__agent_alice");
        std::fs::create_dir_all(&root).unwrap();

        let got = derive_root_project_dir(root.to_str().unwrap())
            .unwrap()
            .expect(".ac workspace should be accepted");
        let expected = std::fs::canonicalize(&project)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert_eq!(got, expected);
    }

    #[test]
    fn derive_root_project_dir_returns_none_for_non_wg_path() {
        let temp = tempfile::TempDir::new().unwrap();
        // A random subdir without the WG-replica shape.
        let random = temp.path().join("random").join("dir");
        std::fs::create_dir_all(&random).unwrap();
        assert!(derive_root_project_dir(random.to_str().unwrap())
            .unwrap()
            .is_none());
    }

    #[test]
    fn derive_root_project_dir_returns_none_when_one_level_too_high() {
        let temp = tempfile::TempDir::new().unwrap();
        let wg_dir = temp.path().join("proj-x").join(".ac").join("wg-1-devs");
        std::fs::create_dir_all(&wg_dir).unwrap();
        // Pointing at the WG dir, not the __agent_* dir → must return None.
        assert!(derive_root_project_dir(wg_dir.to_str().unwrap())
            .unwrap()
            .is_none());
    }

    // ── clap parse smoke: #782 --confirm-timeout ──

    fn parse_send_args(extra: &[&str]) -> SendArgs {
        use clap::Parser;
        let mut argv = vec![
            "agentscommander",
            "send",
            "--token",
            "11111111-1111-1111-1111-111111111111",
            "--to",
            "proj-a/agent",
            "--send",
            "20260704-000000-wg1-a-to-wg1-b-x.md",
            "--root",
            "anything",
        ];
        argv.extend_from_slice(extra);
        let parsed = crate::cli::Cli::try_parse_from(argv).expect("clap should accept send args");
        match parsed.command.expect("subcommand present") {
            crate::cli::Commands::Send(args) => args,
            _ => panic!("expected Send subcommand"),
        }
    }

    fn try_parse_payload_args(payload: &[&str]) -> Result<SendArgs, clap::Error> {
        use clap::Parser;
        let mut argv = vec![
            "agentscommander",
            "send",
            "--token",
            "11111111-1111-4111-8111-111111111111",
            "--to",
            "proj-a:wg-1-dev-team/dev-rust",
            "--root",
            "anything",
        ];
        argv.extend_from_slice(payload);
        let parsed = crate::cli::Cli::try_parse_from(argv)?;
        match parsed.command.expect("subcommand present") {
            crate::cli::Commands::Send(args) => Ok(args),
            _ => panic!("expected Send subcommand"),
        }
    }

    #[test]
    fn send_payload_group_accepts_four_single_forms_and_rejects_every_pair() {
        let forms: [&[&str]; 4] = [
            &["--send", "20260704-000000-wg1-a-to-wg1-b-x.md"],
            &["--command", "clear"],
            &["--pty-input", "-literal shell $(text)"],
            &["--pty-input-stdin"],
        ];
        for form in forms {
            assert!(try_parse_payload_args(form).is_ok(), "single form {form:?}");
        }
        assert!(try_parse_payload_args(&[]).is_err());
        for left in 0..forms.len() {
            for right in (left + 1)..forms.len() {
                let args: Vec<&str> = forms[left]
                    .iter()
                    .chain(forms[right].iter())
                    .copied()
                    .collect();
                assert!(
                    try_parse_payload_args(&args).is_err(),
                    "payload pair {left}/{right}"
                );
            }
        }
    }

    #[test]
    fn pty_input_conflicts_and_bounded_reader_are_exact() {
        assert!(try_parse_payload_args(&["--pty-input", "x", "--get-output"]).is_err());
        assert!(try_parse_payload_args(&["--pty-input", "x", "--outbox", "custom",]).is_err());
        let exact = vec![b'x'; crate::pty::backend::PTY_INPUT_MAX_BYTES];
        assert_eq!(
            read_bounded_pty_input(exact.as_slice()).unwrap().len(),
            exact.len()
        );
        let oversized = vec![b'x'; crate::pty::backend::PTY_INPUT_MAX_BYTES + 1];
        assert_eq!(
            read_bounded_pty_input(oversized.as_slice()),
            Err("payload_too_large")
        );
        assert_eq!(read_bounded_pty_input(&[0xff][..]), Err("invalid_text"));
    }

    #[test]
    fn host_artifact_validation_requires_every_correlation_field_and_directory() {
        use crate::phone::types::{
            canonical_pty_timestamp, PtyInputHostArtifact, PtyInputPublicStatus, PtyInputReason,
            PtyInputReasonCode, PtyInputResult, PtyInputSourcePlane,
        };

        let temp = tempfile::tempdir().unwrap();
        let injection_id = Uuid::new_v4().to_string();
        let issued = chrono::Utc::now();
        let issued_at = canonical_pty_timestamp(issued);
        let expires_at = canonical_pty_timestamp(
            issued + chrono::Duration::seconds(crate::phone::types::PTY_INPUT_TTL_SECS),
        );
        let payload_sha256 = crate::phone::types::sha256_hex(b"exact");
        let expected = ExpectedPtyArtifact {
            injection_id: &injection_id,
            op_id: &injection_id,
            sender: "project:wg-1-team/lead",
            target: "project:wg-1-team/member",
            payload_bytes: 5,
            payload_sha256: &payload_sha256,
            issued_at: &issued_at,
            expires_at: &expires_at,
            confirmation_tag: "confirmation",
        };
        let mut result = PtyInputResult::new(injection_id.clone(), PtyInputPublicStatus::Rejected);
        result.op_id = Some(injection_id.clone());
        result.sender = Some(expected.sender.to_string());
        result.target = Some(expected.target.to_string());
        result.payload_bytes = Some(expected.payload_bytes);
        result.payload_sha256 = Some(payload_sha256.clone());
        result.source_plane = Some(PtyInputSourcePlane::HostCli);
        result.issued_at = Some(issued_at.clone());
        result.expires_at = Some(expires_at.clone());
        result.queued_at = Some(canonical_pty_timestamp(issued));
        result.terminal_at = Some(canonical_pty_timestamp(issued));
        result.reason = Some(PtyInputReason::from_code(PtyInputReasonCode::Busy));
        let artifact = PtyInputHostArtifact {
            result,
            confirmation_tag: expected.confirmation_tag.to_string(),
        };
        let path = temp.path().join("artifact.json");
        let artifact_bytes = serde_json::to_vec(&artifact).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&artifact_bytes)
                .unwrap()
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>(),
            ["confirmationTag".to_string(), "result".to_string()]
                .into_iter()
                .collect()
        );
        std::fs::write(&path, artifact_bytes).unwrap();
        assert!(validate_pty_artifact(&path, &expected).is_ok());

        let mut tampered = artifact.clone();
        tampered.confirmation_tag = "copied".to_string();
        std::fs::write(&path, serde_json::to_vec(&tampered).unwrap()).unwrap();
        assert_eq!(
            validate_pty_artifact(&path, &expected),
            Err("invalid_artifact")
        );

        let delivered = temp.path().join("delivered");
        std::fs::create_dir(&delivered).unwrap();
        std::fs::write(
            delivered.join(format!("{injection_id}.json")),
            serde_json::to_vec(&artifact).unwrap(),
        )
        .unwrap();
        assert_eq!(
            find_pty_input_terminal_artifact(temp.path(), &expected),
            Err("invalid_artifact")
        );
    }

    #[test]
    fn send_args_confirm_timeout_defaults_to_90() {
        let args = parse_send_args(&[]);
        assert_eq!(
            args.confirm_timeout, 90,
            "--confirm-timeout must default to 90"
        );
        assert_eq!(
            confirm_timeout_from_args(&args),
            std::time::Duration::from_secs(90),
            "default must map to the 90s ceiling handed to wait_for_delivery_confirmation"
        );
        // The --get-output response wait keeps its own independent default.
        assert_eq!(args.timeout, 300);
    }

    #[test]
    fn send_args_confirm_timeout_override_reaches_confirmation_wait() {
        let args = parse_send_args(&["--confirm-timeout", "7"]);
        assert_eq!(args.confirm_timeout, 7);
        assert_eq!(
            confirm_timeout_from_args(&args),
            std::time::Duration::from_secs(7),
            "override must be the exact Duration execute() hands to wait_for_delivery_confirmation"
        );
        // Overriding the confirmation wait must not touch the --get-output wait.
        assert_eq!(args.timeout, 300);
    }

    #[test]
    fn send_args_timeout_flag_does_not_affect_confirm_timeout() {
        let args = parse_send_args(&["--timeout", "45"]);
        assert_eq!(args.timeout, 45);
        assert_eq!(
            args.confirm_timeout, 90,
            "--timeout bounds the --get-output wait only; it must not override --confirm-timeout"
        );
    }

    #[test]
    fn send_help_documents_logical_actions_and_pi_mapping() {
        use clap::CommandFactory;
        let cmd = crate::cli::Cli::command();
        let mut subcommand = cmd
            .get_subcommands()
            .find(|cmd| cmd.get_name() == "send")
            .expect("send subcommand")
            .clone();
        let help = subcommand.render_long_help().to_string();

        assert!(help.contains("Logical PTY action"), "{help}");
        assert!(help.contains("clear"), "{help}");
        assert!(help.contains("/new"), "{help}");
        assert!(help.contains("/clear"), "{help}");
        assert!(help.contains("exact-stem direct Pi shell"), "{help}");
        assert!(help.contains("Pi compact"), "{help}");
        assert!(help.contains("must be idle"), "{help}");
        assert!(help.contains("terminally rejected before spawn"), "{help}");
        assert!(
            !help.contains("if Exited, respawn. Always delivers"),
            "logical actions must not inherit an unconditional delivery promise: {help}"
        );
    }

    #[test]
    fn send_help_documents_confirm_timeout_semantics() {
        use clap::CommandFactory;
        let cmd = crate::cli::Cli::command();
        let mut subcommand = cmd
            .get_subcommands()
            .find(|cmd| cmd.get_name() == "send")
            .expect("send subcommand")
            .clone();
        let help = subcommand.render_long_help().to_string();

        assert!(help.contains("--confirm-timeout"), "{help}");
        // Queued-vs-confirmed semantics: timeout does not mean the message was lost.
        assert!(help.contains("durably queued"), "{help}");
        assert!(
            help.contains("does NOT mean the message was lost"),
            "{help}"
        );
        // The two timeout flags must be distinguished from each other.
        assert!(help.contains("--get-output response wait"), "{help}");
    }

    // ── #1638 --profile: parse, validation, outbox JSON, receipt ──

    /// Watch the outbox dir for the queued message file and fabricate the
    /// `delivered/` artifact so `execute`'s confirmation wait resolves fast.
    fn confirm_delivery_watcher(outbox: std::path::PathBuf) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
            while std::time::Instant::now() < deadline {
                if let Ok(entries) = std::fs::read_dir(&outbox) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.extension().and_then(|e| e.to_str()) == Some("json") {
                            let id = path
                                .file_stem()
                                .and_then(|s| s.to_str())
                                .unwrap_or_default()
                                .to_string();
                            let delivered = outbox.join("delivered");
                            let _ = std::fs::create_dir_all(&delivered);
                            let _ = std::fs::write(delivered.join(format!("{}.json", id)), "{}");
                            return;
                        }
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        })
    }

    /// Run `execute` end-to-end against the verified-coordinator fixture with
    /// the message file in place and a dedicated outbox dir. Returns
    /// `(exit code, fixture tempdir, outbox tempdir)`.
    fn enqueue_send_fixture(extra: &[&str]) -> (i32, tempfile::TempDir, tempfile::TempDir) {
        use clap::Parser;
        let (temp, _paths) = make_verified_coordinator_fixture();
        let wg_root = temp.path().join("proj-a").join(".ac").join("wg-1-dev-team");
        let messaging = wg_root.join("messaging");
        std::fs::create_dir_all(&messaging).unwrap();
        std::fs::write(
            messaging.join("20260704-000000-wg1-a-to-wg1-b-x.md"),
            "# Hello from fixture\n\nBody.",
        )
        .unwrap();
        let outbox = tempfile::TempDir::new().unwrap();
        let agent_root = wg_root.join("__agent_dev-rust");
        let mut argv = vec![
            "agentscommander",
            "send",
            "--token",
            "11111111-1111-1111-1111-111111111111",
            "--to",
            "proj-a:wg-1-dev-team/dev-rust",
            "--send",
            "20260704-000000-wg1-a-to-wg1-b-x.md",
            "--root",
            agent_root.to_str().unwrap(),
            "--outbox",
            outbox.path().to_str().unwrap(),
        ];
        argv.extend_from_slice(extra);
        let parsed = crate::cli::Cli::try_parse_from(argv).expect("clap should accept send args");
        let args = match parsed.command.expect("subcommand present") {
            crate::cli::Commands::Send(args) => args,
            _ => panic!("expected Send subcommand"),
        };
        let watcher = confirm_delivery_watcher(outbox.path().to_path_buf());
        let code = execute(args);
        let _ = watcher.join();
        (code, temp, outbox)
    }

    fn outbox_json(outbox: &tempfile::TempDir) -> serde_json::Value {
        let entries: Vec<_> = std::fs::read_dir(outbox.path())
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
            .collect();
        assert_eq!(entries.len(), 1, "expected exactly one outbox message file");
        serde_json::from_str(&std::fs::read_to_string(entries[0].path()).unwrap()).unwrap()
    }

    #[test]
    fn send_args_profile_flag_accepts_a_single_letter_and_normalizes_case() {
        let args = parse_send_args(&["--profile", "c"]);
        assert_eq!(args.profile.as_deref(), Some("c"));

        // execute-time validation normalizes to uppercase: the outbox JSON
        // carries `requestedProfile` = "C".
        let (code, _temp, outbox) = enqueue_send_fixture(&["--profile", "c"]);
        assert_eq!(code, 0, "enqueue path must succeed");
        let json = outbox_json(&outbox);
        assert_eq!(json["requestedProfile"], "C");
    }

    #[test]
    fn send_rejects_invalid_profile_letter_before_enqueue() {
        use clap::Parser;
        for bad in ["3", "AB", ""] {
            // Build the fixture argv for the bad value. clap may reject the
            // empty string at parse time (still exit 1 pre-enqueue); otherwise
            // execute must reject it before writing anything.
            let (temp, _paths) = make_verified_coordinator_fixture();
            let wg_root = temp.path().join("proj-a").join(".ac").join("wg-1-dev-team");
            let messaging = wg_root.join("messaging");
            std::fs::create_dir_all(&messaging).unwrap();
            std::fs::write(
                messaging.join("20260704-000000-wg1-a-to-wg1-b-x.md"),
                "body",
            )
            .unwrap();
            let outbox = tempfile::TempDir::new().unwrap();
            let agent_root = wg_root.join("__agent_dev-rust");
            let argv = vec![
                "agentscommander",
                "send",
                "--token",
                "11111111-1111-1111-1111-111111111111",
                "--to",
                "proj-a:wg-1-dev-team/dev-rust",
                "--send",
                "20260704-000000-wg1-a-to-wg1-b-x.md",
                "--root",
                agent_root.to_str().unwrap(),
                "--outbox",
                outbox.path().to_str().unwrap(),
                "--profile",
                bad,
            ];
            let code = match crate::cli::Cli::try_parse_from(argv) {
                Ok(parsed) => match parsed.command.expect("subcommand present") {
                    crate::cli::Commands::Send(args) => execute(args),
                    _ => panic!("expected Send subcommand"),
                },
                Err(_) => 1, // clap-level rejection: exit non-zero, nothing written
            };
            assert_eq!(code, 1, "--profile {bad:?} must exit 1");
            assert_eq!(
                std::fs::read_dir(outbox.path()).unwrap().count(),
                0,
                "--profile {bad:?} must not write any outbox file"
            );
        }
    }

    #[test]
    fn send_profile_conflicts_with_pty_input() {
        assert!(try_parse_payload_args(&["--profile", "C", "--pty-input", "x"]).is_err());
        assert!(try_parse_payload_args(&["--profile", "C", "--pty-input-stdin"]).is_err());
        // clap rejects before execute: no outbox file can be written.
    }

    #[test]
    fn send_without_flags_writes_no_requested_profile_key() {
        let (code, _temp, outbox) = enqueue_send_fixture(&[]);
        assert_eq!(code, 0, "enqueue path must succeed");
        let json = outbox_json(&outbox);
        let object = json.as_object().unwrap();
        assert!(
            !object.contains_key("requestedProfile"),
            "no-flag send must not write requestedProfile"
        );
        assert_eq!(json["preferredAgent"], "auto");
        // Round-2 Block C: none of the four annotation keys may appear either —
        // the no-flag outbox JSON is byte-identical to today.
        for key in [
            "effectiveAgentId",
            "effectiveProfile",
            "profileFallbackApplied",
            "dispatchNotApplied",
        ] {
            assert!(
                !object.contains_key(key),
                "{key} must be absent without flags"
            );
        }
    }

    #[test]
    fn send_with_agent_and_profile_writes_requested_profile_field() {
        let (code, _temp, outbox) = enqueue_send_fixture(&["--agent", "codex", "--profile", "C"]);
        assert_eq!(code, 0, "enqueue path must succeed");
        let json = outbox_json(&outbox);
        assert_eq!(json["requestedProfile"], "C");
        assert_eq!(json["preferredAgent"], "codex");
        for key in [
            "effectiveAgentId",
            "effectiveProfile",
            "profileFallbackApplied",
            "dispatchNotApplied",
        ] {
            assert!(
                !json.as_object().unwrap().contains_key(key),
                "{key} must stay absent at enqueue time"
            );
        }
    }

    #[test]
    fn queued_receipt_prints_requested_agent_and_profile_when_flags_present() {
        // Flag case: agent + profile.
        assert_eq!(
            queued_receipt_line("m-1", "codex", &Some("C".to_string())),
            "Queued: m-1 (agent=codex, profile=C)"
        );
        // Flag case: profile only — agent stays `auto` on the wire but is
        // still echoed for symmetry with the Delivered: line.
        assert_eq!(
            queued_receipt_line("m-1", "auto", &Some("C".to_string())),
            "Queued: m-1 (agent=auto, profile=C)"
        );
        // No-flag case: exact legacy shape, message id first token.
        assert_eq!(queued_receipt_line("m-1", "auto", &None), "Queued: m-1");
        // Agent-only case (pre-existing --agent semantics) keeps its shape.
        assert_eq!(
            queued_receipt_line("m-1", "codex", &None),
            "Queued: m-1 (agent=codex)"
        );
    }
}
