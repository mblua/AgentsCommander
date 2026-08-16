use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;
use terminal_snapshot_renderer::{
    canonical_timestamp, decode_bounded, encode_host_failure_payload,
    encode_host_json_success_from_model, encode_host_success_payload, render_png, to_ascii_json,
    validate_png_for_metadata, TerminalScreenModel, TerminalSnapshotDocument,
    TerminalSnapshotFormat, TerminalSnapshotPayload, TerminalSnapshotPngMetadata,
    TerminalSnapshotReasonCode, MAX_JSON_BYTES, MAX_REQUEST_BYTES,
};

static HOST_PROCESS_LOCK: Mutex<()> = Mutex::new(());

fn host_process_guard() -> MutexGuard<'static, ()> {
    HOST_PROCESS_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ac-terminal-snapshot-host-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn copied_binary(directory: &Path) -> PathBuf {
    let source = Path::new(env!("CARGO_BIN_EXE_agentscommander-new"));
    let destination = directory.join(source.file_name().unwrap());
    std::fs::copy(source, &destination).unwrap();
    destination
}

fn run(binary: &Path, arguments: &[&str]) -> Output {
    Command::new(binary)
        .args(arguments)
        .env("RUST_LOG", "vt100=trace,agentscommander=trace")
        .output()
        .unwrap()
}

fn run_with_closed_stdout(binary: &Path, arguments: &[&str]) -> Output {
    let mut child = Command::new(binary)
        .args(arguments)
        .env("RUST_LOG", "vt100=trace,agentscommander=trace")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    child.wait_with_output().unwrap()
}

fn assert_fixed_failure(output: &Output, code: &str) {
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr.clone()).unwrap();
    assert_eq!(
        stderr,
        format!(
            "terminal_snapshot_error code={code} detail={}\n",
            all_reason_codes()
                .into_iter()
                .find(|reason| reason.as_str() == code)
                .unwrap()
                .detail()
        )
    );
}

#[test]
fn terminal_snapshot_help_is_machine_clean_and_names_target_discovery() {
    let _guard = host_process_guard();
    let temporary = TempDirectory::new();
    let binary = copied_binary(&temporary.0);
    let output = run(&binary, &["terminal-snapshot", "--help"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).unwrap();
    for expected in [
        "--token <TOKEN>",
        "--root <ROOT>",
        "--to <TO>",
        "--format <FORMAT>",
        "--output <OUTPUT>",
        "--timeout <TIMEOUT>",
        "list-peers-lean",
        "--snapshot-targets",
    ] {
        assert!(stdout.contains(expected), "missing {expected} in {stdout}");
    }
}

#[test]
fn terminal_snapshot_semantic_failures_are_fixed_and_secret_free() {
    let _guard = host_process_guard();
    let temporary = TempDirectory::new();
    let binary = copied_binary(&temporary.0);
    let secret = "sentinel-secret-token-value";
    let root = temporary.0.join("sentinel-secret-root");
    let root_text = root.to_string_lossy();
    let output = run(
        &binary,
        &[
            "terminal-snapshot",
            "--token",
            secret,
            "--root",
            &root_text,
            "--to",
            "project:wg-1-team/member",
            "--timeout",
            "4",
        ],
    );
    assert_fixed_failure(&output, "invalid_request");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!stderr.contains(secret));
    assert!(!stderr.contains(root_text.as_ref()));

    let valid_token = "00000000-0000-4000-8000-000000000117";
    let output = run(
        &binary,
        &[
            "terminal-snapshot",
            "--token",
            valid_token,
            "--root",
            &root_text,
            "--to",
            "project:wg-1-team/member",
            "--format",
            "png",
        ],
    );
    assert_fixed_failure(&output, "invalid_request");

    let output = run(
        &binary,
        &[
            "terminal-snapshot",
            "--token",
            valid_token,
            "--root",
            &root_text,
            "--to",
            "project:wg-1-team/member",
            "--format",
            "png",
            "--output",
            "relative.png",
        ],
    );
    assert_fixed_failure(&output, "unsafe_path");
}

#[test]
fn terminal_snapshot_rejects_a_persisted_static_token_before_publication() {
    let _guard = host_process_guard();
    let temporary = TempDirectory::new();
    let binary = copied_binary(&temporary.0);
    let stem = binary.file_stem().unwrap().to_string_lossy();
    let config = temporary.0.join(format!(".{stem}"));
    std::fs::create_dir_all(&config).unwrap();
    let token = "00000000-0000-4000-8000-000000000118";
    std::fs::write(
        config.join("settings.json"),
        format!("{{\"rootToken\":\"{token}\"}}"),
    )
    .unwrap();
    let root_text = temporary.0.to_string_lossy();
    let output = run(
        &binary,
        &[
            "terminal-snapshot",
            "--token",
            token,
            "--root",
            &root_text,
            "--to",
            "project:wg-1-team/member",
        ],
    );
    assert_fixed_failure(&output, "invalid_request");
    assert!(!String::from_utf8(output.stderr).unwrap().contains(token));
}

const HOST_TOKEN_CANARY: &str = "11730000-0000-4000-8000-00000000c1a1";
const HOST_CELL_CANARY: &str = "H3K9Q2";
const HOST_MALFORMED_CANARY: &str = "ACSNAP_HOST_MALFORMED_1173_H3K9";
const HOST_PATH_CANARY: &str = "ACSNAP_HOST_CALLER_PATH_1173_H3K9";
const HOST_COLLISION_CANARY: &[u8] = b"ACSNAP_HOST_COLLISION_1173_H3K9";
const HOST_TARGET: &str = "project:wg-1-team/member";
const HOST_REQUESTER: &str = "agentscommander://root-agent";

struct HostLayout {
    bundle: PathBuf,
    binary: PathBuf,
    root: PathBuf,
    request_directory: PathBuf,
    response_directory: PathBuf,
}

impl HostLayout {
    fn new(temporary: &TempDirectory) -> Self {
        let bundle = temporary.0.join(HOST_PATH_CANARY);
        std::fs::create_dir(&bundle).unwrap();
        let binary = copied_binary(&bundle);
        let stem = binary.file_stem().unwrap().to_string_lossy();
        let config = bundle.join(format!(".{stem}"));
        let root = config.join("ac-root-agent");
        let local = root.join(format!(".{stem}"));
        let outbox = local.join("outbox");
        std::fs::create_dir_all(&outbox).unwrap();
        Self {
            bundle,
            binary,
            root,
            request_directory: outbox.join("terminal-snapshot-requests"),
            response_directory: local.join("terminal-snapshot-responses"),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WireHostRequest {
    kind: String,
    version: u32,
    request_id: String,
    token: String,
    from: String,
    to: String,
    format: TerminalSnapshotFormat,
    issued_at: String,
    expires_at: String,
    nonce: String,
    confirmation_tag: String,
}

struct CapturedHostRequest {
    wire: WireHostRequest,
    bytes: Vec<u8>,
}

fn spawn_responder(
    layout: &HostLayout,
    build_response: impl FnOnce(&WireHostRequest) -> Vec<u8> + Send + 'static,
) -> thread::JoinHandle<CapturedHostRequest> {
    let request_directory = layout.request_directory.clone();
    let response_directory = layout.response_directory.clone();
    thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let request_path = loop {
            assert!(Instant::now() < deadline, "host request was not published");
            if let Ok(entries) = std::fs::read_dir(&request_directory) {
                if let Some(path) = entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .find(|path| {
                        path.extension()
                            .is_some_and(|extension| extension == "json")
                            && !path.file_name().unwrap().to_string_lossy().starts_with('.')
                    })
                {
                    break path;
                }
            }
            thread::sleep(Duration::from_millis(5));
        };
        let bytes = std::fs::read(&request_path).unwrap();
        let wire: WireHostRequest = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(wire.kind, "terminal-snapshot");
        assert_eq!(wire.version, 1);
        assert_eq!(
            request_path.file_stem().unwrap().to_string_lossy(),
            wire.request_id
        );
        std::fs::remove_file(&request_path).unwrap();

        let response = build_response(&wire);
        let temporary = response_directory.join(format!(
            ".{}.test-terminal-snapshot-response-tmp",
            wire.request_id
        ));
        let destination = response_directory.join(format!("{}.json", wire.request_id));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary).unwrap();
        file.write_all(&response).unwrap();
        file.flush().unwrap();
        file.sync_all().unwrap();
        drop(file);
        publish_test_response(&temporary, &destination);
        CapturedHostRequest { wire, bytes }
    })
}

#[cfg(windows)]
fn publish_test_response(source: &Path, destination: &Path) {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }
    let canonical_leaf = |path: &Path| {
        std::fs::canonicalize(path.parent().unwrap())
            .unwrap()
            .join(path.file_name().unwrap())
    };
    let source: Vec<u16> = canonical_leaf(source)
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let destination: Vec<u16> = canonical_leaf(destination)
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    assert_ne!(
        unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), 0x0000_0008) },
        0
    );
}

#[cfg(not(windows))]
fn publish_test_response(source: &Path, destination: &Path) {
    std::fs::rename(source, destination).unwrap();
}

fn host_arguments(
    layout: &HostLayout,
    format: TerminalSnapshotFormat,
    output: Option<&Path>,
    timeout: u64,
) -> Vec<String> {
    let mut arguments = vec![
        "terminal-snapshot".to_string(),
        "--token".to_string(),
        HOST_TOKEN_CANARY.to_string(),
        "--root".to_string(),
        layout.root.to_string_lossy().into_owned(),
        "--to".to_string(),
        HOST_TARGET.to_string(),
        "--format".to_string(),
        format.to_string(),
        "--timeout".to_string(),
        timeout.to_string(),
    ];
    if let Some(output) = output {
        arguments.push("--output".to_string());
        arguments.push(output.to_string_lossy().into_owned());
    }
    arguments
}

fn run_host(
    layout: &HostLayout,
    format: TerminalSnapshotFormat,
    output: Option<&Path>,
    timeout: u64,
) -> Output {
    let arguments = host_arguments(layout, format, output, timeout);
    let borrowed: Vec<_> = arguments.iter().map(String::as_str).collect();
    run(&layout.binary, &borrowed)
}

fn run_host_with_closed_stdout(
    layout: &HostLayout,
    format: TerminalSnapshotFormat,
    output: Option<&Path>,
    timeout: u64,
) -> Output {
    let arguments = host_arguments(layout, format, output, timeout);
    let borrowed: Vec<_> = arguments.iter().map(String::as_str).collect();
    run_with_closed_stdout(&layout.binary, &borrowed)
}

fn host_model() -> TerminalScreenModel {
    let mut model: TerminalScreenModel = decode_bounded(
        include_bytes!(
            "../../crates/terminal-snapshot-renderer/tests/fixtures/blank-cursor-model.json"
        ),
        MAX_JSON_BYTES,
    )
    .unwrap();
    model.screen.lines[0].cells[0].text = HOST_CELL_CANARY.to_string();
    model
}

fn host_success_response(request: &WireHostRequest) -> Vec<u8> {
    let expires_at = canonical_timestamp(chrono::Utc::now() + chrono::Duration::seconds(60));
    match request.format {
        TerminalSnapshotFormat::Json => encode_host_json_success_from_model(
            &request.request_id,
            &request.confirmation_tag,
            &expires_at,
            &request.from,
            &request.to,
            &host_model(),
        )
        .unwrap(),
        TerminalSnapshotFormat::Png => {
            let model = host_model();
            let rendered = render_png(&model).unwrap();
            let payload = TerminalSnapshotPayload::Png {
                metadata: rendered.metadata(
                    request.request_id.clone(),
                    request.from.clone(),
                    request.to.clone(),
                    &model,
                ),
                png: rendered.bytes,
            };
            encode_host_success_payload(
                &request.request_id,
                &request.confirmation_tag,
                &expires_at,
                &payload,
            )
            .unwrap()
        }
    }
}

fn host_failure_response(request: &WireHostRequest, reason: TerminalSnapshotReasonCode) -> Vec<u8> {
    encode_host_failure_payload(
        &request.request_id,
        &request.confirmation_tag,
        &canonical_timestamp(chrono::Utc::now() + chrono::Duration::seconds(60)),
        reason,
    )
    .unwrap()
}

fn all_reason_codes() -> [TerminalSnapshotReasonCode; 16] {
    use TerminalSnapshotReasonCode as C;
    [
        C::InvalidRequest,
        C::RequesterUnavailable,
        C::TerminalSnapshotsDisabled,
        C::NotAuthorized,
        C::TargetUnavailable,
        C::SnapshotUnavailable,
        C::SnapshotTooLarge,
        C::AuthorityChanged,
        C::RateLimited,
        C::SnapshotTimeout,
        C::ServiceUnavailable,
        C::RenderFailed,
        C::UnsafePath,
        C::OutputFailed,
        C::ResponseUnavailable,
        C::Internal,
    ]
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn assert_absent(bytes: &[u8], forbidden: &[&[u8]]) {
    for value in forbidden {
        assert!(
            !contains(bytes, value),
            "forbidden canary reached process surface"
        );
    }
}

fn assert_one_ascii_line(bytes: &[u8]) {
    assert!(bytes.is_ascii());
    assert_eq!(bytes.last(), Some(&b'\n'));
    assert_eq!(bytes.iter().filter(|byte| **byte == b'\n').count(), 1);
}

fn assert_empty_protocol_directories(layout: &HostLayout) {
    for directory in [&layout.request_directory, &layout.response_directory] {
        if directory.exists() {
            assert_eq!(std::fs::read_dir(directory).unwrap().count(), 0);
        }
    }
}

fn scan_regular_files(root: &Path, excluded: &[&Path], forbidden: &[&[u8]]) {
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            let metadata = std::fs::symlink_metadata(&path).unwrap();
            if metadata.is_dir() {
                pending.push(path);
            } else if metadata.is_file() && !excluded.iter().any(|excluded| **excluded == path) {
                assert_absent(&std::fs::read(path).unwrap(), forbidden);
            }
        }
    }
}

fn assert_request_canaries_are_confined(captured: &CapturedHostRequest, output: &Output) {
    assert_eq!(captured.wire.token, HOST_TOKEN_CANARY);
    assert_eq!(captured.wire.from, HOST_REQUESTER);
    assert_eq!(captured.wire.to, HOST_TARGET);
    assert_eq!(captured.wire.nonce.len(), 64);
    assert_eq!(captured.wire.confirmation_tag.len(), 64);
    let issued_at =
        terminal_snapshot_renderer::validate_timestamp(&captured.wire.issued_at).unwrap();
    let expires_at =
        terminal_snapshot_renderer::validate_timestamp(&captured.wire.expires_at).unwrap();
    assert!(issued_at < expires_at);
    assert!(contains(&captured.bytes, HOST_TOKEN_CANARY.as_bytes()));
    assert!(contains(&captured.bytes, captured.wire.nonce.as_bytes()));
    assert!(contains(
        &captured.bytes,
        captured.wire.confirmation_tag.as_bytes()
    ));
    let forbidden = [
        HOST_TOKEN_CANARY.as_bytes(),
        captured.wire.nonce.as_bytes(),
        captured.wire.confirmation_tag.as_bytes(),
        HOST_PATH_CANARY.as_bytes(),
        HOST_MALFORMED_CANARY.as_bytes(),
    ];
    assert_absent(&output.stdout, &forbidden);
    assert_absent(&output.stderr, &forbidden);
}

#[test]
fn terminal_snapshot_host_process_json_and_png_success_are_exact_and_confined() {
    let _guard = host_process_guard();
    let temporary = TempDirectory::new();
    let layout = HostLayout::new(&temporary);

    let json_responder = spawn_responder(&layout, host_success_response);
    let json_output = run_host(&layout, TerminalSnapshotFormat::Json, None, 5);
    let json_request = json_responder.join().unwrap();
    assert_eq!(
        json_output.status.code(),
        Some(0),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&json_output.stdout),
        String::from_utf8_lossy(&json_output.stderr)
    );
    assert!(json_output.stderr.is_empty());
    assert_one_ascii_line(&json_output.stdout);
    let document = TerminalSnapshotDocument::from_model(
        json_request.wire.request_id.clone(),
        HOST_REQUESTER.to_string(),
        HOST_TARGET.to_string(),
        &host_model(),
    );
    let mut expected = to_ascii_json(&document, MAX_JSON_BYTES).unwrap();
    expected.push(b'\n');
    assert_eq!(json_output.stdout, expected);
    assert!(contains(&json_output.stdout, HOST_CELL_CANARY.as_bytes()));
    assert_request_canaries_are_confined(&json_request, &json_output);
    assert_empty_protocol_directories(&layout);

    let output_parent = layout.bundle.join("requested-output");
    std::fs::create_dir(&output_parent).unwrap();
    let png_path = output_parent.join("snapshot.png");
    let png_responder = spawn_responder(&layout, host_success_response);
    let png_output = run_host(&layout, TerminalSnapshotFormat::Png, Some(&png_path), 5);
    let png_request = png_responder.join().unwrap();
    assert_eq!(
        png_output.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&png_output.stderr)
    );
    assert!(png_output.stderr.is_empty());
    assert_one_ascii_line(&png_output.stdout);
    assert!(!contains(&png_output.stdout, b"pngBase64"));
    assert_absent(&png_output.stdout, &[HOST_CELL_CANARY.as_bytes()]);
    let rendered = render_png(&host_model()).unwrap();
    assert_eq!(std::fs::read(&png_path).unwrap(), rendered.bytes);
    let metadata: TerminalSnapshotPngMetadata = decode_bounded(
        &png_output.stdout[..png_output.stdout.len() - 1],
        MAX_REQUEST_BYTES,
    )
    .unwrap();
    validate_png_for_metadata(&std::fs::read(&png_path).unwrap(), &metadata).unwrap();
    assert_request_canaries_are_confined(&png_request, &png_output);
    assert_empty_protocol_directories(&layout);

    scan_regular_files(
        &layout.bundle,
        &[layout.binary.as_path(), png_path.as_path()],
        &[
            HOST_TOKEN_CANARY.as_bytes(),
            HOST_CELL_CANARY.as_bytes(),
            json_request.wire.nonce.as_bytes(),
            json_request.wire.confirmation_tag.as_bytes(),
            png_request.wire.nonce.as_bytes(),
            png_request.wire.confirmation_tag.as_bytes(),
        ],
    );
}

#[test]
fn terminal_snapshot_host_process_surfaces_every_fixed_server_failure() {
    let _guard = host_process_guard();
    let temporary = TempDirectory::new();
    let layout = HostLayout::new(&temporary);
    let mut dynamic_canaries = Vec::new();
    for reason in all_reason_codes()
        .into_iter()
        .filter(|reason| *reason != TerminalSnapshotReasonCode::OutputFailed)
    {
        let responder = spawn_responder(&layout, move |request| {
            host_failure_response(request, reason)
        });
        let output = run_host(&layout, TerminalSnapshotFormat::Json, None, 5);
        let captured = responder.join().unwrap();
        assert_fixed_failure(&output, reason.as_str());
        assert_request_canaries_are_confined(&captured, &output);
        dynamic_canaries.push(captured.wire.nonce);
        dynamic_canaries.push(captured.wire.confirmation_tag);
        assert_empty_protocol_directories(&layout);
    }
    let dynamic_refs: Vec<_> = dynamic_canaries
        .iter()
        .map(|value| value.as_bytes())
        .chain(std::iter::once(HOST_TOKEN_CANARY.as_bytes()))
        .collect();
    scan_regular_files(&layout.bundle, &[layout.binary.as_path()], &dynamic_refs);
}

// Targeted harness for the closed-stdout PNG sub-case, the one that
// intermittently reported `response_unavailable` where the contract says
// `output_failed`. The event is rare enough (measured 0.107%) that the full gate
// is a useless instrument for it: seeing zero failures in a few dozen gate runs
// says nothing. This drives thousands of iterations of only that sub-case
// instead, and pairs with AC_TERMINAL_SNAPSHOT_STAGE_FILE to record which stage
// produced whatever code was emitted.
//
// Inert unless AC_D2_ITERATIONS is set, so it costs a normal run nothing.
// `assert_fixed_failure` stays byte-for-byte strict; it is only wrapped so one
// recurrence does not truncate the sample before it is complete.
#[test]
fn d2_targeted_closed_stdout_harness() {
    let Ok(iterations) = std::env::var("AC_D2_ITERATIONS") else {
        return;
    };
    let iterations: usize = iterations
        .parse()
        .expect("AC_D2_ITERATIONS must be a count");
    let _guard = host_process_guard();
    let temporary = TempDirectory::new();
    let layout = HostLayout::new(&temporary);
    let output_parent = layout.bundle.join("d2-harness-outputs");
    std::fs::create_dir(&output_parent).unwrap();
    let expected_png = render_png(&host_model()).unwrap().bytes;

    let mut mismatches: Vec<String> = Vec::new();
    for iteration in 0..iterations {
        let path = output_parent.join(format!("d2-{iteration}.png"));
        let responder = spawn_responder(&layout, host_success_response);
        let output =
            run_host_with_closed_stdout(&layout, TerminalSnapshotFormat::Png, Some(&path), 5);
        // The responder panics if the client never published a request. Record
        // that instead of ending the run, so one recurrence does not truncate
        // the sample.
        if responder.join().is_err() {
            mismatches.push(format!("iteration={iteration} responder-saw-no-request"));
        }
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            assert_fixed_failure(&output, "output_failed");
        }))
        .is_err()
        {
            mismatches.push(format!(
                "iteration={iteration} stderr={:?}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        match std::fs::read(&path) {
            Ok(bytes) if bytes == expected_png => {}
            other => mismatches.push(format!(
                "iteration={iteration} png-not-complete ok={}",
                other.is_ok()
            )),
        }
        let _ = std::fs::remove_file(&path);
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            assert_empty_protocol_directories(&layout);
        }))
        .is_err()
        {
            mismatches.push(format!("iteration={iteration} protocol-directories-dirty"));
            for directory in [&layout.request_directory, &layout.response_directory] {
                if let Ok(entries) = std::fs::read_dir(directory) {
                    for entry in entries.filter_map(Result::ok) {
                        let _ = std::fs::remove_file(entry.path());
                    }
                }
            }
        }
    }
    eprintln!(
        "D2HARNESS iterations={iterations} mismatches={}",
        mismatches.len()
    );
    assert!(
        mismatches.is_empty(),
        "D2 recurred in {} of {iterations} iterations: {mismatches:?}",
        mismatches.len()
    );
}

#[test]
fn terminal_snapshot_host_process_transport_deadline_collision_and_output_failure_are_safe() {
    let _guard = host_process_guard();
    let temporary = TempDirectory::new();
    let layout = HostLayout::new(&temporary);
    let output_parent = layout.bundle.join("failure-outputs");
    std::fs::create_dir(&output_parent).unwrap();

    let malformed_responder = spawn_responder(&layout, |_| {
        format!("{{\"malformed\":\"{HOST_MALFORMED_CANARY}\"}}").into_bytes()
    });
    let malformed = run_host(&layout, TerminalSnapshotFormat::Json, None, 5);
    let malformed_request = malformed_responder.join().unwrap();
    assert_fixed_failure(&malformed, "response_unavailable");
    assert_request_canaries_are_confined(&malformed_request, &malformed);
    assert_empty_protocol_directories(&layout);

    let corrupt_path = output_parent.join("corrupt.png");
    let corrupt_responder = spawn_responder(&layout, |request| {
        let mut response = host_success_response(request);
        let base64 = response
            .windows(b"iVBOR".len())
            .position(|window| window == b"iVBOR")
            .unwrap();
        response[base64] = b'!';
        response
    });
    let corrupt = run_host(&layout, TerminalSnapshotFormat::Png, Some(&corrupt_path), 5);
    let corrupt_request = corrupt_responder.join().unwrap();
    assert_fixed_failure(&corrupt, "response_unavailable");
    assert!(!corrupt_path.exists());
    assert_request_canaries_are_confined(&corrupt_request, &corrupt);
    assert_empty_protocol_directories(&layout);

    let collision_path = output_parent.join("collision.png");
    std::fs::write(&collision_path, HOST_COLLISION_CANARY).unwrap();
    let collision = run_host(
        &layout,
        TerminalSnapshotFormat::Png,
        Some(&collision_path),
        5,
    );
    assert_fixed_failure(&collision, "unsafe_path");
    assert_eq!(
        std::fs::read(&collision_path).unwrap(),
        HOST_COLLISION_CANARY
    );
    assert_empty_protocol_directories(&layout);

    let timeout_path = output_parent.join("deadline.png");
    let timeout = run_host(&layout, TerminalSnapshotFormat::Png, Some(&timeout_path), 5);
    assert_fixed_failure(&timeout, "snapshot_timeout");
    assert!(!timeout_path.exists());
    assert_empty_protocol_directories(&layout);

    let completed_path = output_parent.join("completed-before-stdout-failure.png");
    let completed_responder = spawn_responder(&layout, host_success_response);
    let output_failed = run_host_with_closed_stdout(
        &layout,
        TerminalSnapshotFormat::Png,
        Some(&completed_path),
        5,
    );
    let completed_request = completed_responder.join().unwrap();
    assert_fixed_failure(&output_failed, "output_failed");
    assert_eq!(
        std::fs::read(&completed_path).unwrap(),
        render_png(&host_model()).unwrap().bytes
    );
    assert_request_canaries_are_confined(&completed_request, &output_failed);
    assert_empty_protocol_directories(&layout);

    scan_regular_files(
        &layout.bundle,
        &[
            layout.binary.as_path(),
            collision_path.as_path(),
            completed_path.as_path(),
        ],
        &[
            HOST_TOKEN_CANARY.as_bytes(),
            HOST_CELL_CANARY.as_bytes(),
            HOST_MALFORMED_CANARY.as_bytes(),
            malformed_request.wire.nonce.as_bytes(),
            malformed_request.wire.confirmation_tag.as_bytes(),
            corrupt_request.wire.nonce.as_bytes(),
            corrupt_request.wire.confirmation_tag.as_bytes(),
            completed_request.wire.nonce.as_bytes(),
            completed_request.wire.confirmation_tag.as_bytes(),
        ],
    );
}
