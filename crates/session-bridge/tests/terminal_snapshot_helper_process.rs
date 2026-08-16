use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use terminal_snapshot_renderer::{
    decode_bounded, encode_api_json_success_from_model, encode_api_success_payload, to_ascii_json,
    validate_png_for_metadata, TerminalPngInfo, TerminalPngScreenMetadata,
    TerminalRendererMetadata, TerminalScreenModel, TerminalSnapshotApiError,
    TerminalSnapshotApiRequest, TerminalSnapshotFormat, TerminalSnapshotPayload,
    TerminalSnapshotPngMetadata, TerminalSnapshotReasonCode, MAX_JSON_BYTES, MAX_REQUEST_BYTES,
};

const TARGET: &str = "project:wg-1-team/member";
const REQUESTER: &str = "project:wg-1-team/coordinator";
const CELL_CANARY: &str = "K7N4X9";
const BEARER_CANARY: &str = "ACSNAP_HELPER_BEARER_1173_K7N4";
const MALFORMED_CANARY: &str = "ACSNAP_HELPER_MALFORMED_1173_K7N4";
const PATH_CANARY: &str = "ACSNAP_HELPER_CALLER_PATH_1173_K7N4";
const COLLISION_CANARY: &[u8] = b"ACSNAP_HELPER_COLLISION_1173_K7N4";
const PNG_BYTES: &[u8] =
    include_bytes!("../../terminal-snapshot-renderer/tests/fixtures/blank-cursor.png");

#[derive(Clone)]
struct CapturedRequest {
    method: String,
    path: String,
    headers: Vec<u8>,
    body: Vec<u8>,
}

type ReplyBuilder = Box<dyn Fn(&CapturedRequest) -> Reply + Send>;

enum Reply {
    Bytes(Vec<u8>),
    DropConnection,
    Trickle {
        head: Vec<u8>,
        first: Vec<u8>,
        delay: Duration,
        rest: Vec<u8>,
    },
}

struct TestServer {
    url: String,
    captured: Arc<Mutex<Vec<CapturedRequest>>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl TestServer {
    fn start(mut responder: impl FnMut(&CapturedRequest, usize) -> Reply + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let captured = Arc::new(Mutex::new(Vec::new()));
        let captured_for_thread = Arc::clone(&captured);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            while !stop_for_thread.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        let request = read_request(&mut stream);
                        let index = {
                            let mut captured = captured_for_thread.lock().unwrap();
                            captured.push(request.clone());
                            captured.len() - 1
                        };
                        match responder(&request, index) {
                            Reply::Bytes(bytes) => {
                                let _ = stream.write_all(&bytes);
                                let _ = stream.flush();
                            }
                            Reply::DropConnection => {}
                            Reply::Trickle {
                                head,
                                first,
                                delay,
                                rest,
                            } => {
                                let _ = stream.write_all(&head);
                                let _ = stream.write_all(&first);
                                let _ = stream.flush();
                                thread::sleep(delay);
                                let _ = stream.write_all(&rest);
                                let _ = stream.flush();
                            }
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => panic!("scripted server accept failed: {error}"),
                }
            }
        });
        Self {
            url: format!("http://{address}/"),
            captured,
            stop,
            thread: Some(thread),
        }
    }

    fn finish(mut self) -> Vec<CapturedRequest> {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
        self.captured.lock().unwrap().clone()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn read_request(stream: &mut TcpStream) -> CapturedRequest {
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut bytes = Vec::new();
    let header_end = loop {
        let mut chunk = [0_u8; 2048];
        let read = stream.read(&mut chunk).unwrap();
        assert!(read > 0, "helper closed before sending request headers");
        bytes.extend_from_slice(&chunk[..read]);
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
        assert!(
            bytes.len() <= 64 * 1024,
            "request headers exceeded test cap"
        );
    };
    let header_text = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
    let content_length = header_text
        .split("\r\n")
        .find_map(|line| {
            line.split_once(':').and_then(|(name, value)| {
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
        })
        .unwrap_or(0);
    while bytes.len() < header_end + content_length {
        let mut chunk = [0_u8; 2048];
        let read = stream.read(&mut chunk).unwrap();
        assert!(read > 0, "helper closed before sending request body");
        bytes.extend_from_slice(&chunk[..read]);
    }
    let request_line = header_text.lines().next().unwrap();
    let mut parts = request_line.split_whitespace();
    CapturedRequest {
        method: parts.next().unwrap().to_string(),
        path: parts.next().unwrap().to_string(),
        headers: bytes[..header_end].to_vec(),
        body: bytes[header_end..header_end + content_length].to_vec(),
    }
}

fn helper_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_agentscommander-api-helper"))
}

fn helper_command(url: &str, arguments: &[&str]) -> Command {
    let mut command = Command::new(helper_binary());
    command
        .args(arguments)
        .env("AGENTSCOMMANDER_API_URL", url)
        .env("AGENTSCOMMANDER_API_TOKEN", BEARER_CANARY)
        .env("RUST_BACKTRACE", "1");
    command
}

fn run_helper(url: &str, arguments: &[&str]) -> Output {
    helper_command(url, arguments).output().unwrap()
}

fn run_helper_with_closed_stdout(url: &str, arguments: &[&str]) -> Output {
    let mut child = helper_command(url, arguments)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    child.wait_with_output().unwrap()
}

fn fixture_model(cell_canary: bool) -> TerminalScreenModel {
    let mut model: TerminalScreenModel = decode_bounded(
        include_bytes!("../../terminal-snapshot-renderer/tests/fixtures/blank-cursor-model.json"),
        MAX_JSON_BYTES,
    )
    .unwrap();
    if cell_canary {
        model.screen.lines[0].cells[0].text = CELL_CANARY.to_string();
    }
    model
}

fn parse_request(request: &CapturedRequest) -> TerminalSnapshotApiRequest {
    let parsed: TerminalSnapshotApiRequest =
        decode_bounded(&request.body, MAX_REQUEST_BYTES).unwrap();
    parsed.validate().unwrap();
    parsed
}

fn success_body(request: &CapturedRequest) -> Vec<u8> {
    let request = parse_request(request);
    match request.format {
        TerminalSnapshotFormat::Json => encode_api_json_success_from_model(
            &request.request_id,
            REQUESTER,
            &request.to,
            &fixture_model(true),
        )
        .unwrap(),
        TerminalSnapshotFormat::Png => {
            let model = fixture_model(false);
            let image = model.screen.dimensions.checked_image_dimensions().unwrap();
            let metadata = TerminalSnapshotPngMetadata {
                schema_version: 1,
                request_id: request.request_id,
                captured_at: model.captured_at.clone(),
                requester: REQUESTER.to_string(),
                target: request.to,
                session: model.session.clone(),
                screen: TerminalPngScreenMetadata {
                    dimensions: model.screen.dimensions,
                    sequence: model.screen.sequence,
                    active_buffer: model.screen.active_buffer,
                    cursor: model.screen.cursor,
                    parser_errors: model.screen.parser_errors,
                },
                fidelity: model.fidelity.clone(),
                format: TerminalSnapshotFormat::Png,
                png: TerminalPngInfo {
                    bytes: PNG_BYTES.len() as u64,
                    pixel_width: image.pixel_width,
                    pixel_height: image.pixel_height,
                },
                renderer: TerminalRendererMetadata::version_one(0),
            };
            encode_api_success_payload(&TerminalSnapshotPayload::Png {
                metadata,
                png: PNG_BYTES.to_vec(),
            })
            .unwrap()
        }
    }
}

fn error_body(reason: TerminalSnapshotReasonCode) -> Vec<u8> {
    to_ascii_json(&TerminalSnapshotApiError::new(reason), 8 * 1024).unwrap()
}

fn strict_response(status: u16, body: &[u8], extra_headers: &str) -> Vec<u8> {
    let reason = if status == 200 { "OK" } else { "Response" };
    let mut response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json; charset=utf-8\r\nCache-Control: no-store\r\nPragma: no-cache\r\nContent-Length: {}\r\n{extra_headers}Connection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

fn success_response(request: &CapturedRequest) -> Vec<u8> {
    strict_response(200, &success_body(request), "")
}

fn assert_exact_failure(output: &Output, reason: TerminalSnapshotReasonCode) {
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        output.stderr,
        format!(
            "terminal_snapshot_error code={} detail={}\n",
            reason.as_str(),
            reason.detail()
        )
        .as_bytes()
    );
}

fn assert_one_ascii_line(bytes: &[u8]) {
    assert!(bytes.is_ascii());
    assert_eq!(bytes.last(), Some(&b'\n'));
    assert_eq!(bytes.iter().filter(|byte| **byte == b'\n').count(), 1);
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn assert_absent(bytes: &[u8], values: &[&[u8]]) {
    for value in values {
        assert!(
            !contains(bytes, value),
            "forbidden canary reached process surface"
        );
    }
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

fn assert_request_contract(request: &CapturedRequest) {
    let headers = String::from_utf8(request.headers.clone())
        .unwrap()
        .to_ascii_lowercase();
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/api/v1/terminal-snapshot");
    assert!(headers.contains("accept-encoding: identity\r\n"));
    assert!(headers.contains(&format!(
        "authorization: bearer {}\r\n",
        BEARER_CANARY.to_ascii_lowercase()
    )));
    assert!(!contains(&request.body, BEARER_CANARY.as_bytes()));
}

fn scan_regular_files(root: &Path, excluded: &[&Path], forbidden: &[&[u8]]) {
    if !root.exists() {
        return;
    }
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path).unwrap();
            if metadata.is_dir() {
                pending.push(path);
            } else if metadata.is_file() && !excluded.iter().any(|excluded| **excluded == path) {
                let bytes = std::fs::read(path).unwrap();
                assert_absent(&bytes, forbidden);
            }
        }
    }
}

#[test]
fn terminal_snapshot_helper_process_json_and_png_success_have_exact_confined_surfaces() {
    let json_server = TestServer::start(|request, _| Reply::Bytes(success_response(request)));
    let json_output = run_helper(
        &json_server.url,
        &["terminal-snapshot", "--to", TARGET, "--format", "json"],
    );
    let json_requests = json_server.finish();
    assert_eq!(json_output.status.code(), Some(0));
    assert!(json_output.stderr.is_empty());
    assert_one_ascii_line(&json_output.stdout);
    assert!(contains(&json_output.stdout, CELL_CANARY.as_bytes()));
    assert_absent(
        &json_output.stdout,
        &[BEARER_CANARY.as_bytes(), MALFORMED_CANARY.as_bytes()],
    );
    assert_eq!(json_requests.len(), 1);
    assert_request_contract(&json_requests[0]);
    let request_id = parse_request(&json_requests[0]).request_id;
    assert!(contains(&json_output.stdout, request_id.as_bytes()));

    let temporary = tempfile::tempdir().unwrap();
    let parent = temporary.path().join(PATH_CANARY);
    std::fs::create_dir(&parent).unwrap();
    let png_path = parent.join("snapshot.png");
    let png_text = png_path.to_string_lossy().into_owned();
    let png_server = TestServer::start(|request, _| Reply::Bytes(success_response(request)));
    let png_output = run_helper(
        &png_server.url,
        &[
            "terminal-snapshot",
            "--to",
            TARGET,
            "--format",
            "png",
            "--output",
            &png_text,
        ],
    );
    let png_requests = png_server.finish();
    assert_eq!(png_output.status.code(), Some(0));
    assert!(png_output.stderr.is_empty());
    assert_one_ascii_line(&png_output.stdout);
    assert_eq!(std::fs::read(&png_path).unwrap(), PNG_BYTES);
    assert!(!contains(&png_output.stdout, b"pngBase64"));
    assert_absent(
        &png_output.stdout,
        &[
            CELL_CANARY.as_bytes(),
            BEARER_CANARY.as_bytes(),
            PATH_CANARY.as_bytes(),
        ],
    );
    assert_eq!(png_requests.len(), 1);
    assert_request_contract(&png_requests[0]);
    let metadata: TerminalSnapshotPngMetadata = decode_bounded(
        &png_output.stdout[..png_output.stdout.len() - 1],
        MAX_REQUEST_BYTES,
    )
    .unwrap();
    validate_png_for_metadata(PNG_BYTES, &metadata).unwrap();
    scan_regular_files(
        temporary.path(),
        &[png_path.as_path()],
        &[BEARER_CANARY.as_bytes(), CELL_CANARY.as_bytes()],
    );
}

#[test]
fn terminal_snapshot_helper_process_surfaces_every_fixed_failure_without_reflection() {
    for reason in all_reason_codes()
        .into_iter()
        .filter(|reason| reason.http_status().is_some())
    {
        let server = TestServer::start(move |_, _| {
            Reply::Bytes(strict_response(
                reason.http_status().unwrap(),
                &error_body(reason),
                "",
            ))
        });
        let output = run_helper(&server.url, &["terminal-snapshot", "--to", TARGET]);
        let requests = server.finish();
        assert_exact_failure(&output, reason);
        assert_eq!(requests.len(), 1);
        assert_request_contract(&requests[0]);
        assert_absent(
            &output.stderr,
            &[BEARER_CANARY.as_bytes(), MALFORMED_CANARY.as_bytes()],
        );
    }

    let invalid = Command::new(helper_binary())
        .args([
            "terminal-snapshot",
            "--token",
            MALFORMED_CANARY,
            "--to",
            TARGET,
        ])
        .output()
        .unwrap();
    assert_exact_failure(&invalid, TerminalSnapshotReasonCode::InvalidRequest);
    assert_absent(&invalid.stderr, &[MALFORMED_CANARY.as_bytes()]);

    let unsafe_output = Command::new(helper_binary())
        .args([
            "terminal-snapshot",
            "--to",
            TARGET,
            "--format",
            "png",
            "--output",
            MALFORMED_CANARY,
        ])
        .output()
        .unwrap();
    assert_exact_failure(&unsafe_output, TerminalSnapshotReasonCode::UnsafePath);
    assert_absent(&unsafe_output.stderr, &[MALFORMED_CANARY.as_bytes()]);
}

#[test]
fn terminal_snapshot_helper_process_http_failures_are_fixed_single_post_and_secret_free() {
    let cases: Vec<ReplyBuilder> = vec![
        Box::new(|_| Reply::Bytes(strict_response(302, b"", "Location: /forwarded\r\n"))),
        Box::new(|_| Reply::Bytes(strict_response(421, b"", ""))),
        Box::new(|_| {
            Reply::Bytes(strict_response(
                200,
                b"{}",
                "Content-Type: application/json; charset=utf-8\r\n",
            ))
        }),
        Box::new(|_| Reply::Bytes(strict_response(200, b"{}", "Cache-Control: no-store\r\n"))),
        Box::new(|_| Reply::Bytes(strict_response(200, b"{}", "Pragma: no-cache\r\n"))),
        Box::new(|_| Reply::Bytes(strict_response(200, b"{}", "Content-Length: 2\r\n"))),
        Box::new(|_| Reply::Bytes(strict_response(200, b"{}", "Content-Encoding: gzip\r\n"))),
        Box::new(|_| {
            Reply::Bytes(strict_response(
                200,
                b"{}",
                "Content-Encoding: identity\r\nContent-Encoding: identity\r\n",
            ))
        }),
        Box::new(|_| {
            Reply::Bytes(strict_response(
                200,
                format!("{{\"malformed\":\"{MALFORMED_CANARY}\"}}").as_bytes(),
                "",
            ))
        }),
        Box::new(|_| {
            let body = format!("{{\"error\":\"{MALFORMED_CANARY}\"}}");
            let mut response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nCache-Control: no-store\r\nPragma: no-cache\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len() + 9
            )
            .into_bytes();
            response.extend_from_slice(body.as_bytes());
            Reply::Bytes(response)
        }),
        Box::new(|_| Reply::DropConnection),
    ];

    for case in cases {
        let server = TestServer::start(move |request, _| case(request));
        let output = run_helper(&server.url, &["terminal-snapshot", "--to", TARGET]);
        let requests = server.finish();
        assert_exact_failure(&output, TerminalSnapshotReasonCode::ResponseUnavailable);
        assert_eq!(requests.len(), 1, "helper retried or followed a redirect");
        assert_request_contract(&requests[0]);
        assert_absent(
            &output.stderr,
            &[BEARER_CANARY.as_bytes(), MALFORMED_CANARY.as_bytes()],
        );
    }
}

#[test]
fn terminal_snapshot_helper_process_bypasses_proxy_and_never_forwards_bearer() {
    let proxy = TestServer::start(|_, _| Reply::DropConnection);
    let direct = TestServer::start(|request, _| Reply::Bytes(success_response(request)));
    let output = helper_command(
        &direct.url,
        &["terminal-snapshot", "--to", TARGET, "--format", "json"],
    )
    .env("HTTP_PROXY", &proxy.url)
    .env("HTTPS_PROXY", &proxy.url)
    .env("ALL_PROXY", &proxy.url)
    .output()
    .unwrap();
    let direct_requests = direct.finish();
    let proxy_requests = proxy.finish();
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    assert_eq!(direct_requests.len(), 1);
    assert!(proxy_requests.is_empty());
    assert_request_contract(&direct_requests[0]);
}

#[test]
fn terminal_snapshot_helper_process_png_deadline_collision_corruption_and_output_failure_do_not_clobber(
) {
    let temporary = tempfile::tempdir().unwrap();
    let parent = temporary.path().join(PATH_CANARY);
    std::fs::create_dir(&parent).unwrap();

    let timeout_path = parent.join("deadline.png");
    let timeout_text = timeout_path.to_string_lossy().into_owned();
    let timeout_server = TestServer::start(|request, _| {
        let body = success_body(request);
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nCache-Control: no-store\r\nPragma: no-cache\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        Reply::Trickle {
            head,
            first: body[..1].to_vec(),
            delay: Duration::from_millis(5_300),
            rest: body[1..].to_vec(),
        }
    });
    let timeout = run_helper(
        &timeout_server.url,
        &[
            "terminal-snapshot",
            "--to",
            TARGET,
            "--format",
            "png",
            "--output",
            &timeout_text,
            "--timeout",
            "5",
        ],
    );
    let timeout_requests = timeout_server.finish();
    assert_exact_failure(&timeout, TerminalSnapshotReasonCode::SnapshotTimeout);
    assert_eq!(timeout_requests.len(), 1);
    assert!(!timeout_path.exists());

    let collision_path = parent.join("collision.png");
    let collision_for_server = collision_path.clone();
    let collision_text = collision_path.to_string_lossy().into_owned();
    let collision_server = TestServer::start(move |request, _| {
        std::fs::write(&collision_for_server, COLLISION_CANARY).unwrap();
        Reply::Bytes(success_response(request))
    });
    let collision = run_helper(
        &collision_server.url,
        &[
            "terminal-snapshot",
            "--to",
            TARGET,
            "--format",
            "png",
            "--output",
            &collision_text,
        ],
    );
    let collision_requests = collision_server.finish();
    assert_exact_failure(&collision, TerminalSnapshotReasonCode::UnsafePath);
    assert_eq!(collision_requests.len(), 1);
    assert_eq!(std::fs::read(&collision_path).unwrap(), COLLISION_CANARY);

    let corrupt_path = parent.join("corrupt.png");
    let corrupt_text = corrupt_path.to_string_lossy().into_owned();
    let corrupt_server = TestServer::start(|request, _| {
        let mut body = success_body(request);
        let base64 = body
            .windows(b"iVBOR".len())
            .position(|window| window == b"iVBOR")
            .unwrap();
        body[base64] = b'!';
        Reply::Bytes(strict_response(200, &body, ""))
    });
    let corrupt = run_helper(
        &corrupt_server.url,
        &[
            "terminal-snapshot",
            "--to",
            TARGET,
            "--format",
            "png",
            "--output",
            &corrupt_text,
        ],
    );
    let corrupt_requests = corrupt_server.finish();
    assert_exact_failure(&corrupt, TerminalSnapshotReasonCode::ResponseUnavailable);
    assert_eq!(corrupt_requests.len(), 1);
    assert!(!corrupt_path.exists());

    let completed_path = parent.join("completed-before-stdout-failure.png");
    let completed_text = completed_path.to_string_lossy().into_owned();
    let output_server = TestServer::start(|request, _| Reply::Bytes(success_response(request)));
    let output_failed = run_helper_with_closed_stdout(
        &output_server.url,
        &[
            "terminal-snapshot",
            "--to",
            TARGET,
            "--format",
            "png",
            "--output",
            &completed_text,
        ],
    );
    let output_requests = output_server.finish();
    assert_exact_failure(&output_failed, TerminalSnapshotReasonCode::OutputFailed);
    assert_eq!(output_requests.len(), 1);
    assert_eq!(std::fs::read(&completed_path).unwrap(), PNG_BYTES);

    scan_regular_files(
        temporary.path(),
        &[collision_path.as_path(), completed_path.as_path()],
        &[BEARER_CANARY.as_bytes(), MALFORMED_CANARY.as_bytes()],
    );
}
