use super::*;
use crate::pty_result::{
    canonical_agent_fqn, canonical_timestamp, parse_api_error_code, parse_result,
    safe_reason_detail, sha256_hex, ExpectedPtyResult, PtyStatus, PTY_RESPONSE_MAX_BYTES,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct Fixture {
    name: String,
    text: String,
    valid: bool,
}

pub(crate) enum ScriptedReply {
    DropConnection,
    Http(u16, Vec<u8>),
    Raw(Vec<u8>),
    Trickle {
        head: Vec<u8>,
        first: Vec<u8>,
        delay: Duration,
        rest: Vec<u8>,
    },
}

#[derive(Clone)]
pub(crate) struct CapturedRequest {
    method: String,
    path: String,
    pub(crate) headers: String,
    body: Vec<u8>,
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> CapturedRequest {
    use tokio::io::AsyncReadExt;

    let mut bytes = Vec::new();
    let header_end = loop {
        let mut chunk = [0_u8; 2048];
        let read = stream.read(&mut chunk).await.unwrap();
        assert!(read > 0, "client closed before request headers");
        bytes.extend_from_slice(&chunk[..read]);
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
        assert!(
            bytes.len() < 64 * 1024,
            "request headers are unexpectedly large"
        );
    };
    let headers = std::str::from_utf8(&bytes[..header_end]).unwrap();
    let captured_headers = headers.to_ascii_lowercase();
    let mut lines = headers.split("\r\n");
    let request_line = lines.next().unwrap();
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts.next().unwrap().to_string();
    let path = request_parts.next().unwrap().to_string();
    let content_length = lines
        .find_map(|line| {
            line.split_once(':').and_then(|(name, value)| {
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
        })
        .unwrap_or(0);
    while bytes.len() < header_end + content_length {
        let mut chunk = [0_u8; 2048];
        let read = stream.read(&mut chunk).await.unwrap();
        assert!(read > 0, "client closed before request body");
        bytes.extend_from_slice(&chunk[..read]);
    }
    CapturedRequest {
        method,
        path,
        headers: captured_headers,
        body: bytes[header_end..header_end + content_length].to_vec(),
    }
}

pub(crate) async fn scripted_server(
    replies: Vec<ScriptedReply>,
) -> (
    String,
    std::sync::Arc<std::sync::Mutex<Vec<CapturedRequest>>>,
    tokio::task::JoinHandle<()>,
) {
    use tokio::io::AsyncWriteExt;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_for_task = std::sync::Arc::clone(&captured);
    let task = tokio::spawn(async move {
        for reply in replies {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_request(&mut stream).await;
            captured_for_task.lock().unwrap().push(request);
            match reply {
                ScriptedReply::DropConnection => {}
                ScriptedReply::Http(status, body) => {
                    let reason = match status {
                        200 => "OK",
                        202 => "Accepted",
                        404 => "Not Found",
                        500 => "Internal Server Error",
                        _ => "Response",
                    };
                    let headers = format!(
                        "HTTP/1.1 {status} {reason}\r\nContent-Type: \
                         application/json\r\nContent-Length: {}\r\nConnection: \
                         close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(headers.as_bytes()).await;
                    let _ = stream.write_all(&body).await;
                    let _ = stream.shutdown().await;
                }
                ScriptedReply::Raw(response) => {
                    let _ = stream.write_all(&response).await;
                    let _ = stream.shutdown().await;
                }
                ScriptedReply::Trickle {
                    head,
                    first,
                    delay,
                    rest,
                } => {
                    let _ = stream.write_all(&head).await;
                    let _ = stream.write_all(&first).await;
                    tokio::time::sleep(delay).await;
                    let _ = stream.write_all(&rest).await;
                    let _ = stream.shutdown().await;
                }
            }
        }
    });
    (format!("http://{address}"), captured, task)
}

fn valid_result(op_id: &str, target: &str, digest: &str, status: &str) -> Vec<u8> {
    let injection_id = Uuid::new_v4().to_string();
    let mut result = json!({
        "version": 1,
        "injectionId": injection_id,
        "opId": op_id,
        "sender": "project:wg-1-team/coordinator",
        "target": target,
        "status": status,
        "terminal": false,
        "payloadBytes": 1,
        "payloadSha256": digest,
        "sourcePlane": "container_api",
        "issuedAt": "2026-01-01T00:00:00.000Z",
        "expiresAt": "2026-01-01T00:10:00.000Z",
        "queuedAt": "2026-01-01T00:00:00.001Z"
    });
    if matches!(status, "injected" | "indeterminate") {
        result["terminal"] = json!(true);
        result["selectedSessionId"] = json!(Uuid::new_v4().to_string());
        result["selectedBackend"] = json!("containerTransport");
        result["actuatingAt"] = json!("2026-01-01T00:00:01.000Z");
        result["terminalAt"] = json!("2026-01-01T00:00:03.000Z");
    }
    serde_json::to_vec(&result).unwrap()
}

#[test]
fn inline_cap_matches_daemon_contract() {
    assert_eq!(INLINE_BODY_MAX_BYTES, 256 * 1024);
    assert_eq!(PTY_INPUT_MAX_BYTES, 65_536);
}

#[test]
fn mirrored_validator_executes_shared_fixture() {
    let rows: Vec<Fixture> = serde_json::from_str(include_str!(
        "../../../tests/fixtures/pty_input_validation.json"
    ))
    .unwrap();
    for row in rows {
        assert_eq!(
            validate_pty_text(&row.text).is_ok(),
            row.valid,
            "fixture {}",
            row.name
        );
    }
}

#[test]
fn mirrored_validator_rejects_every_forbidden_control_and_bidi_scalar() {
    let mut forbidden: Vec<u32> = (0x00..=0x1f)
        .filter(|code| !matches!(code, 0x09 | 0x0a))
        .chain(0x7f..=0x9f)
        .collect();
    forbidden.extend([
        0x061c, 0x200e, 0x200f, 0x2028, 0x2029, 0x202a, 0x202b, 0x202c, 0x202d, 0x202e, 0x2066,
        0x2067, 0x2068, 0x2069,
    ]);
    for code in forbidden {
        let scalar = char::from_u32(code).expect("valid scalar");
        assert!(
            validate_pty_text(&format!("a{scalar}b")).is_err(),
            "U+{code:04X} must reject"
        );
    }
}

#[test]
fn parser_requires_one_form_and_rejects_host_flags() {
    let parsed = parse_send_options(vec![
        "--to".into(),
        "p:wg-1-t/a".into(),
        "--pty-input".into(),
        "-literal".into(),
    ])
    .unwrap();
    assert_eq!(parsed.pty_input.unwrap(), OsString::from("-literal"));
    let equals = parse_send_options(vec![
        "--to".into(),
        "p:wg-1-t/a".into(),
        "--pty-input=-literal $(shell)".into(),
    ])
    .unwrap();
    assert_eq!(
        equals.pty_input.unwrap(),
        OsString::from("-literal $(shell)")
    );
    assert!(parse_send_options(vec!["--pty-input=x".into(), "--pty-input-stdin".into(),]).is_err());
    assert!(parse_send_options(vec!["--root".into(), "x".into()]).is_err());
}

#[test]
fn bounded_stdin_and_agent_grammar_match_contract() {
    assert_eq!(
        read_bounded_stdin(&b"line one\nline two\n"[..]).unwrap(),
        "line one\nline two\n"
    );
    assert!(read_bounded_stdin(&b"x".repeat(PTY_INPUT_MAX_BYTES)[..]).is_ok());
    assert!(read_bounded_stdin(&b"x".repeat(PTY_INPUT_MAX_BYTES + 1)[..]).is_err());
    assert_eq!(
        validate_agent_id(Some("codex-main_1".into())).unwrap(),
        Some("codex-main_1".into())
    );
    assert!(validate_agent_id(Some("Bad".into())).is_err());
}

#[test]
fn response_fqn_validation_rejects_path_aliases_controls_and_bidi() {
    assert!(canonical_agent_fqn("project:room-25-ac-dev-team/member"));
    assert!(canonical_agent_fqn("project:wg-1-team/member"));
    for invalid in [
        ".:wg-1-team/member",
        "..:wg-1-team/member",
        "pro\nject:wg-1-team/member",
        "pro\u{202e}ject:wg-1-team/member",
        "project:room-25-ac-dev-team/",
        "project:room-25-ac-dev-team/bad_agent",
        "project:room-25-ac-dev-team/member/more",
        "project:room-x-ac-dev-team/member",
        "project:room-25-/member",
        "pro\u{2028}ject:room-25-ac-dev-team/member",
    ] {
        assert!(!canonical_agent_fqn(invalid), "accepted {invalid:?}");
    }
}

#[test]
fn sha256_and_strict_result_validation_are_pinned() {
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert!(canonical_timestamp("2024-02-29T23:59:59.999Z"));
    assert!(!canonical_timestamp("2023-02-29T23:59:59.999Z"));
    assert!(!canonical_timestamp("2026-02-31T00:00:00.000Z"));
    let op_id = Uuid::new_v4().to_string();
    let injection_id = Uuid::new_v4().to_string();
    let digest = sha256_hex(b"x");
    let body = serde_json::to_vec(&json!({
        "version": 1,
        "injectionId": injection_id,
        "opId": op_id,
        "sender": "project:wg-1-team/lead",
        "target": "project:wg-1-team/dev",
        "status": "queued",
        "terminal": false,
        "payloadBytes": 1,
        "payloadSha256": digest,
        "sourcePlane": "container_api",
        "issuedAt": "2026-01-01T00:00:00.000Z",
        "expiresAt": "2026-01-01T00:10:00.000Z",
        "queuedAt": "2026-01-01T00:00:00.000Z"
    }))
    .unwrap();
    let expected = ExpectedPtyResult {
        target: "project:wg-1-team/dev",
        payload_bytes: 1,
        payload_sha256: &digest,
    };
    assert!(parse_result(&body, &op_id, Some(&expected)).is_ok());

    let room_body = serde_json::to_vec(&json!({
        "version": 1,
        "injectionId": injection_id,
        "opId": op_id,
        "sender": "project:room-25-ac-dev-team/lead",
        "target": "project:room-25-ac-dev-team/dev",
        "status": "queued",
        "terminal": false,
        "payloadBytes": 1,
        "payloadSha256": digest,
        "sourcePlane": "container_api",
        "issuedAt": "2026-01-01T00:00:00.000Z",
        "expiresAt": "2026-01-01T00:10:00.000Z",
        "queuedAt": "2026-01-01T00:00:00.000Z"
    }))
    .unwrap();
    let room_expected = ExpectedPtyResult {
        target: "project:room-25-ac-dev-team/dev",
        payload_bytes: 1,
        payload_sha256: &digest,
    };
    assert!(parse_result(&room_body, &op_id, Some(&room_expected)).is_ok());

    let mut leaked: serde_json::Value = serde_json::from_slice(&body).unwrap();
    leaked["status"] = json!("rejected");
    leaked["terminal"] = json!(true);
    leaked["terminalAt"] = json!("2026-01-01T00:00:01.000Z");
    leaked["reason"] = json!({"code":"busy","detail":"payload sentinel"});
    assert!(parse_result(
        &serde_json::to_vec(&leaked).unwrap(),
        &op_id,
        Some(&expected)
    )
    .is_err());

    let valid_error = serde_json::to_vec(&json!({
        "apiVersion": "1",
        "error": "busy",
        "detail": safe_reason_detail("busy").unwrap()
    }))
    .unwrap();
    assert_eq!(parse_api_error_code(&valid_error).unwrap(), "busy");
    let adversarial_error = serde_json::to_vec(&json!({
        "apiVersion": "1",
        "error": "busy",
        "detail": "payload sentinel"
    }))
    .unwrap();
    assert!(parse_api_error_code(&adversarial_error).is_err());
}

#[tokio::test]
async fn connection_reset_get_404_then_retry_reuses_identical_post_bytes() {
    let op_id = Uuid::new_v4().to_string();
    let target = "project:wg-1-team/member";
    let digest = sha256_hex(b"x");
    let expected = ExpectedPtyResult {
        target,
        payload_bytes: 1,
        payload_sha256: &digest,
    };
    let request = serde_json::to_vec(&json!({
        "apiVersion": "1",
        "opId": op_id,
        "to": target,
        "ptyInput": {"version": 1, "text": "x", "enter": "agent-submit"}
    }))
    .unwrap();
    let (base_url, captured, task) = scripted_server(vec![
        ScriptedReply::DropConnection,
        ScriptedReply::Http(
            404,
            serde_json::to_vec(&json!({
                "apiVersion": "1",
                "error": "not_found",
                "detail": "operation_not_found"
            }))
            .unwrap(),
        ),
        ScriptedReply::Http(202, valid_result(&op_id, target, &digest, "queued")),
    ])
    .await;

    let result = submit_pty_request(
        &reqwest::Client::new(),
        &base_url,
        "Bearer test-token",
        &request,
        &op_id,
        &expected,
    )
    .await
    .unwrap();
    assert!(result.status == PtyStatus::Queued);
    task.await.unwrap();
    let captured = captured.lock().unwrap();
    assert_eq!(captured.len(), 3);
    assert_eq!(captured[0].method, "POST");
    assert_eq!(captured[1].method, "GET");
    assert_eq!(captured[2].method, "POST");
    assert_eq!(captured[0].body, request);
    assert_eq!(captured[2].body, request);
}

#[tokio::test]
async fn server_error_queries_same_operation_before_any_retry() {
    let op_id = Uuid::new_v4().to_string();
    let target = "project:wg-1-team/member";
    let digest = sha256_hex(b"x");
    let expected = ExpectedPtyResult {
        target,
        payload_bytes: 1,
        payload_sha256: &digest,
    };
    let request = b"immutable-request".to_vec();
    let (base_url, captured, task) = scripted_server(vec![
        ScriptedReply::Http(500, b"{}".to_vec()),
        ScriptedReply::Http(200, valid_result(&op_id, target, &digest, "injected")),
    ])
    .await;

    let result = submit_pty_request(
        &reqwest::Client::new(),
        &base_url,
        "Bearer test-token",
        &request,
        &op_id,
        &expected,
    )
    .await
    .unwrap();
    assert!(result.status == PtyStatus::Injected);
    task.await.unwrap();
    let captured = captured.lock().unwrap();
    assert_eq!(captured.len(), 2);
    assert_eq!(captured[0].method, "POST");
    assert_eq!(captured[1].method, "GET");
    assert!(captured[1].path.ends_with(&op_id));
}

#[tokio::test]
async fn malformed_successful_post_response_queries_same_operation_before_retry() {
    let op_id = Uuid::new_v4().to_string();
    let target = "project:wg-1-team/member";
    let digest = sha256_hex(b"x");
    let expected = ExpectedPtyResult {
        target,
        payload_bytes: 1,
        payload_sha256: &digest,
    };
    let request = b"immutable-request".to_vec();
    let (base_url, captured, task) = scripted_server(vec![
        ScriptedReply::Http(202, b"{malformed-success-sentinel".to_vec()),
        ScriptedReply::Http(200, valid_result(&op_id, target, &digest, "injected")),
    ])
    .await;

    let result = submit_pty_request(
        &reqwest::Client::new(),
        &base_url,
        "Bearer test-token",
        &request,
        &op_id,
        &expected,
    )
    .await
    .unwrap();
    assert!(result.status == PtyStatus::Injected);
    task.await.unwrap();
    let captured = captured.lock().unwrap();
    assert_eq!(captured.len(), 2);
    assert_eq!(captured[0].method, "POST");
    assert_eq!(captured[1].method, "GET");
    assert!(captured[1].path.ends_with(&op_id));
}

#[tokio::test]
async fn oversized_and_malformed_status_bodies_fail_without_body_disclosure() {
    let op_id = Uuid::new_v4().to_string();
    let oversized = vec![b'S'; PTY_RESPONSE_MAX_BYTES + 1];
    let (base_url, _, task) = scripted_server(vec![ScriptedReply::Http(200, oversized)]).await;
    let error = get_pty_status_at(
        &reqwest::Client::new(),
        &base_url,
        "Bearer test-token",
        &op_id,
        None,
    )
    .await
    .err()
    .expect("oversized response must fail");
    assert_eq!(error, "invalid_server_response");
    assert!(!error.contains('S'));
    task.await.unwrap();

    let (base_url, _, task) = scripted_server(vec![ScriptedReply::Http(
        200,
        b"{malformed-response-sentinel".to_vec(),
    )])
    .await;
    let error = get_pty_status_at(
        &reqwest::Client::new(),
        &base_url,
        "Bearer test-token",
        &op_id,
        None,
    )
    .await
    .err()
    .expect("malformed response must fail");
    assert_eq!(error, "invalid_server_response");
    assert!(!error.contains("malformed-response-sentinel"));
    task.await.unwrap();
}

#[tokio::test]
async fn status_lookup_uses_get_only() {
    let op_id = Uuid::new_v4().to_string();
    let target = "project:wg-1-team/member";
    let digest = sha256_hex(b"x");
    let (base_url, captured, task) = scripted_server(vec![ScriptedReply::Http(
        200,
        valid_result(&op_id, target, &digest, "queued"),
    )])
    .await;
    let result = get_pty_status_at(
        &reqwest::Client::new(),
        &base_url,
        "Bearer test-token",
        &op_id,
        None,
    )
    .await
    .unwrap()
    .unwrap();
    assert!(result.status == PtyStatus::Queued);
    task.await.unwrap();
    let captured = captured.lock().unwrap();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].method, "GET");
    assert!(captured[0].body.is_empty());
}

#[test]
fn send_parser_rejects_zero_and_every_payload_pair() {
    assert!(parse_send_options(vec!["--to".into(), "p:wg-1-t/a".into()])
        .and_then(|options| {
            let forms = usize::from(options.file.is_some())
                + usize::from(options.message.is_some())
                + usize::from(options.pty_input.is_some())
                + usize::from(options.pty_input_stdin);
            (forms == 1)
                .then_some(options)
                .ok_or_else(|| "exactly_one_payload_required".to_string())
        })
        .is_err());
    let forms = [
        vec!["--send", "m.md"],
        vec!["--message", "hello"],
        vec!["--pty-input", "hello"],
        vec!["--pty-input-stdin"],
    ];
    for left in 0..forms.len() {
        for right in (left + 1)..forms.len() {
            let mut args = vec!["--to".into(), "p:wg-1-t/a".into()];
            for part in forms[left].iter().chain(forms[right].iter()) {
                args.push(OsString::from(part));
            }
            let rejected = parse_send_options(args)
                .map(|parsed| {
                    usize::from(parsed.file.is_some())
                        + usize::from(parsed.message.is_some())
                        + usize::from(parsed.pty_input.is_some())
                        + usize::from(parsed.pty_input_stdin)
                        != 1
                })
                .unwrap_or(true);
            assert!(rejected, "pair {left}/{right} must be rejected by send");
        }
    }
}

/// Confinement rejects every symlinked ancestor, and macOS reaches its temporary
/// directory through one: `$TMPDIR` lives under `/var -> private/var`, and the
/// `/tmp` fallback is itself a link. Canonicalise the root once so these fixtures
/// exercise confinement instead of the platform's temporary-directory layout.
pub(crate) fn snapshot_temp_root() -> (tempfile::TempDir, std::path::PathBuf) {
    let temporary = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temporary.path()).unwrap();
    (temporary, root)
}
