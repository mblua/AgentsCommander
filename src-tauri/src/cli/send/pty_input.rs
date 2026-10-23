//! Privileged PTY input: publish, validate and confirm `--pty-input` requests.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use uuid::Uuid;

use super::{derive_root_project_dir, OutboxMessage, SendArgs};

pub(super) fn read_bounded_pty_input<R: Read>(reader: R) -> Result<String, &'static str> {
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

pub(super) fn pty_text_from_args(args: &SendArgs) -> Result<Option<String>, &'static str> {
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

pub(super) struct ExpectedPtyArtifact<'a> {
    pub(super) injection_id: &'a str,
    pub(super) op_id: &'a str,
    pub(super) sender: &'a str,
    pub(super) target: &'a str,
    pub(super) payload_bytes: u64,
    pub(super) payload_sha256: &'a str,
    pub(super) issued_at: &'a str,
    pub(super) expires_at: &'a str,
    pub(super) confirmation_tag: &'a str,
}

pub(super) fn validate_pty_artifact(
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

pub(super) fn find_pty_input_terminal_artifact(
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

pub(super) fn execute_pty_input(args: SendArgs, root: String, text: String) -> i32 {
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
            "Indeterminate: {injection_id} code={code} published={final_exists} \
                correlatedArtifact={correlated_artifact}; do not resubmit under a new ID"
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
                "Confirmation timeout: operation {injection_id} remains queued or actuating; do \
                    not resubmit under a new ID. Inspect its delivered, rejected, and \
                    indeterminate artifacts."
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
    crate::phone::types::pty_input_reason_code_name(code)
}
