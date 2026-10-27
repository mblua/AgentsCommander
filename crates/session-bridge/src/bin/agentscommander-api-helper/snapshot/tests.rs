use super::*;
use crate::tests::snapshot_temp_root;
use crate::tests::{scripted_server, ScriptedReply};
fn strict_snapshot_response(status: u16, body: &[u8], extra_headers: &str) -> Vec<u8> {
    let reason = if status == 200 { "OK" } else { "Response" };
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json; \
         charset=utf-8\r\nCache-Control: no-store\r\nPragma: \
         no-cache\r\nContent-Length: {}\r\n{extra_headers}Connection: \
         close\r\n\r\n{}",
        body.len(),
        String::from_utf8_lossy(body),
    )
    .into_bytes()
}

#[tokio::test]
async fn terminal_snapshot_client_never_follows_redirects_or_retries_protocol_status() {
    for (status, extra) in [
        (302, "Location: http://127.0.0.1:9/forwarded\r\n"),
        (421, ""),
    ] {
        let scripted = strict_snapshot_response(status, b"", extra);
        let (base, captured, task) = scripted_server(vec![ScriptedReply::Raw(scripted)]).await;
        let client = snapshot_http_client().unwrap();
        let response = client
            .post(format!("{base}/api/v1/terminal-snapshot"))
            .header(ACCEPT_ENCODING, "identity")
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), status);
        task.await.unwrap();
        let captured = captured.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert!(captured[0]
            .headers
            .contains("accept-encoding: identity\r\n"));
    }
}

#[tokio::test]
async fn terminal_snapshot_response_rejects_compression_and_duplicate_security_headers() {
    for extra in ["Content-Encoding: gzip\r\n", "Cache-Control: no-store\r\n"] {
        let response = strict_snapshot_response(200, b"{}", extra);
        let (base, _, task) = scripted_server(vec![ScriptedReply::Raw(response)]).await;
        let client = snapshot_http_client().unwrap();
        let response = client.get(base).send().await.unwrap();
        assert_eq!(
            validate_snapshot_response_headers(&response).unwrap_err(),
            terminal_snapshot_renderer::TerminalSnapshotReasonCode::ResponseUnavailable
        );
        task.await.unwrap();
    }
}

#[tokio::test]
async fn terminal_snapshot_streamed_body_uses_one_absolute_deadline() {
    let head = include_bytes!("tests/fixtures/response-head.http").to_vec();
    let (base, _, task) = scripted_server(vec![ScriptedReply::Trickle {
        head,
        first: b"{".to_vec(),
        delay: Duration::from_millis(100),
        rest: b"}\n\n".to_vec(),
    }])
    .await;
    let client = snapshot_http_client().unwrap();
    let response = client.get(base).send().await.unwrap();
    validate_snapshot_response_headers(&response).unwrap();
    let error = read_snapshot_response_body(
        response,
        terminal_snapshot_renderer::MAX_TRANSPORT_BYTES,
        Instant::now() + Duration::from_millis(20),
    )
    .await
    .unwrap_err();
    assert_eq!(
        error,
        terminal_snapshot_renderer::TerminalSnapshotReasonCode::SnapshotTimeout
    );
    task.await.unwrap();
}

#[test]
fn terminal_snapshot_base_url_rejects_credentials_query_fragment_and_suffix() {
    for invalid in [
        "ftp://example.test/",
        "http://user@example.test/",
        "http://example.test/?query=1",
        "http://example.test/#fragment",
        "http://example.test/api/v1/",
    ] {
        assert!(validate_snapshot_api_url(invalid).is_err(), "{invalid}");
    }
    assert!(validate_snapshot_api_url("http://example.test/").is_ok());
    assert!(validate_snapshot_api_url("https://example.test/").is_ok());
}

#[test]
fn terminal_snapshot_parser_enforces_format_output_and_duplicates() {
    use terminal_snapshot_renderer::{
        TerminalSnapshotFormat as F, TerminalSnapshotReasonCode as C,
    };

    let json =
        parse_terminal_snapshot_options(vec!["--to".into(), "project:wg-1-team/member".into()])
            .unwrap();
    assert_eq!(json.format, F::Json);
    assert_eq!(json.timeout, 15);
    assert!(json.output.is_none());

    let (_temporary, root) = snapshot_temp_root();
    let output = root.join("snapshot.png");
    let png = parse_terminal_snapshot_options(vec![
        "--to".into(),
        "project:wg-1-team/member".into(),
        "--format".into(),
        "png".into(),
        "--output".into(),
        output.clone().into_os_string(),
        "--timeout".into(),
        "60".into(),
    ])
    .unwrap();
    assert_eq!(png.format, F::Png);
    assert_eq!(png.output.as_deref(), Some(output.as_path()));
    assert_eq!(png.timeout, 60);

    for invalid in [
        vec![
            "--to".into(),
            "project:wg-1-team/member".into(),
            "--output".into(),
            output.clone().into_os_string(),
        ],
        vec![
            "--to".into(),
            "project:wg-1-team/member".into(),
            "--timeout".into(),
            "4".into(),
        ],
        vec![
            "--to".into(),
            "project:wg-1-team/member".into(),
            "--to".into(),
            "project:wg-1-team/other".into(),
        ],
        vec![
            "--token".into(),
            "secret".into(),
            "--to".into(),
            "project:wg-1-team/member".into(),
        ],
    ] {
        assert!(matches!(
            parse_terminal_snapshot_options(invalid),
            Err(C::InvalidRequest)
        ));
    }
}

/// Plan line 157 in the code: `--output` travels from the raw `OsString` into a
/// `PathBuf` with no conversion, so a non-UTF-8 Unix leaf must survive the parser
/// byte for byte. Parsing and validation create nothing, which is why this holds on
/// every Unix, macOS included.
#[cfg(unix)]
#[test]
fn terminal_snapshot_parser_preserves_non_utf8_output_bytes() {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    let (_temporary, root) = snapshot_temp_root();
    let leaf = std::ffi::OsString::from_vec(b"snapshot-\xff.png".to_vec());
    let output = root.join(leaf);
    let options = parse_terminal_snapshot_options(vec![
        "--to".into(),
        "project:wg-1-team/member".into(),
        "--format".into(),
        "png".into(),
        "--output".into(),
        output.clone().into_os_string(),
    ])
    .unwrap();

    assert_eq!(
        options
            .output
            .as_deref()
            .map(|path| path.as_os_str().as_bytes()),
        Some(output.as_os_str().as_bytes())
    );
    assert!(!output.exists(), "parsing created the output leaf");
}

#[test]
fn terminal_snapshot_option_debug_omits_target_and_path_canaries() {
    const AUTH_CANARY: &str = "AUTH_1173_HELPER_B7W4";
    const PATH_CANARY: &str = r"C:\PATH_1173_HELPER_B7W4\snapshot.png";
    let options = TerminalSnapshotOptions {
        to: Some(AUTH_CANARY.to_string()),
        format: terminal_snapshot_renderer::TerminalSnapshotFormat::Png,
        output: Some(std::path::PathBuf::from(PATH_CANARY)),
        timeout: 15,
        saw_format: true,
        saw_timeout: true,
    };
    let diagnostic = format!("{options:?}");
    assert!(!diagnostic.contains(AUTH_CANARY));
    assert!(!diagnostic.contains(PATH_CANARY));
    assert!(diagnostic.contains("has_target: true"));
    assert!(diagnostic.contains("format: Png"));
    assert!(diagnostic.contains("has_output: true"));
}
