use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use uuid::Uuid;

const SESSION_ID: &str = "11111111-1111-4111-8111-111111111111";
const TICKET: &str = "e2e-ticket";
const TOKEN: &str = "e2e-token";
const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

#[test]
fn docker_bridge_connects_streams_pty_output_and_exits() {
    if std::env::var("AGENTSCOMMANDER_RUN_DOCKER_E2E").as_deref() != Ok("1") {
        eprintln!("skipping Docker e2e; set AGENTSCOMMANDER_RUN_DOCKER_E2E=1");
        return;
    }

    assert_command_ok(Command::new("docker").arg("--version"), "docker --version");

    let repo_root = repo_root();
    let image = "agentscommander/session-bridge:e2e-test";
    assert_command_ok(
        Command::new("docker").current_dir(&repo_root).args([
            "build",
            "-f",
            "crates/session-bridge/Dockerfile",
            "-t",
            image,
            ".",
        ]),
        "docker build session bridge image",
    );

    let listener = TcpListener::bind("0.0.0.0:0").expect("bind e2e websocket listener");
    listener
        .set_nonblocking(true)
        .expect("set listener nonblocking");
    let port = listener.local_addr().expect("listener addr").port();
    let server = thread::spawn(move || run_ws_server(listener));

    let container_name = format!("ac-session-bridge-e2e-{}", Uuid::new_v4().simple());
    let output = Command::new("docker")
        .args([
            "run",
            "--rm",
            "--name",
            &container_name,
            "--add-host",
            "host.docker.internal:host-gateway",
            "--tmpfs",
            "/workspace",
            "-e",
            &format!("AGENTSCOMMANDER_API_URL=http://host.docker.internal:{port}"),
            "-e",
            &format!("AGENTSCOMMANDER_API_TOKEN={TOKEN}"),
            "-e",
            &format!("AGENTSCOMMANDER_SESSION_ID={SESSION_ID}"),
            "-e",
            &format!("AGENTSCOMMANDER_SESSION_REGISTRATION_TOKEN={TICKET}"),
            "-e",
            "AGENTSCOMMANDER_ROOT=/workspace",
            "-e",
            "AGENTSCOMMANDER_BRIDGE_WORKDIR=/workspace",
            "-e",
            "AGENTSCOMMANDER_BRIDGE_COMMAND=/bin/sh",
            "-e",
            "AGENTSCOMMANDER_BRIDGE_ARGS_JSON=[\"-lc\",\"printf bridge-e2e\"]",
            "-e",
            "AGENTSCOMMANDER_BRIDGE_ENV_JSON=[]",
            "-e",
            "AGENTSCOMMANDER_BRIDGE_COLS=80",
            "-e",
            "AGENTSCOMMANDER_BRIDGE_ROWS=24",
            image,
        ])
        .output()
        .expect("run bridge container");
    let server_result = server.join().expect("server thread");
    assert!(
        output.status.success(),
        "docker run failed\nstdout:\n{}\nstderr:\n{}\nserver:\n{server_result:?}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let observed = server_result.expect("server result");
    assert!(
        observed.headers.contains(&format!(
            "/api/v1/session-transport?sessionId={SESSION_ID}&ticket={TICKET}"
        )),
        "unexpected request path:\n{}",
        observed.headers
    );
    assert!(
        observed
            .headers
            .to_ascii_lowercase()
            .contains(&format!("authorization: bearer {TOKEN}")),
        "missing bearer token:\n{}",
        observed.headers
    );
    assert!(
        observed.text_frames.iter().any(|text| {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
                return false;
            };
            value["type"] == "hello"
                && value["sessionId"] == SESSION_ID
                && value["root"] == "/workspace"
        }),
        "missing hello frame: {:?}",
        observed.text_frames
    );
    assert!(
        observed
            .binary_output
            .windows(b"bridge-e2e".len())
            .any(|window| window == b"bridge-e2e"),
        "missing PTY output: {:?}",
        observed.binary_output
    );
    assert!(
        observed.text_frames.iter().any(|text| {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
                return false;
            };
            value["type"] == "exit" && value["code"] == 0
        }),
        "missing exit frame: {:?}",
        observed.text_frames
    );

    run_helper_post_status_e2e(image);
}

fn run_helper_post_status_e2e(image: &str) {
    let listener = TcpListener::bind("0.0.0.0:0").expect("bind helper HTTP listener");
    listener
        .set_nonblocking(true)
        .expect("set helper listener nonblocking");
    let port = listener.local_addr().expect("helper listener addr").port();
    let server = thread::spawn(move || run_helper_http_server(listener));
    let output = Command::new("docker")
        .args([
            "run",
            "--rm",
            "--add-host",
            "host.docker.internal:host-gateway",
            "--entrypoint",
            "agentscommander-api-helper",
            "-e",
            &format!("AGENTSCOMMANDER_API_URL=http://host.docker.internal:{port}"),
            "-e",
            &format!("AGENTSCOMMANDER_API_TOKEN={TOKEN}"),
            image,
            "send",
            "--to",
            "project:wg-1-team/member",
            "--pty-input=helper-e2e",
            "--confirm-timeout",
            "5",
        ])
        .output()
        .expect("run helper container");
    let observed = server
        .join()
        .expect("helper server thread")
        .expect("helper server result");
    assert!(
        output.status.success(),
        "helper docker run failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Operation ID:"),
        "missing operation id: {stdout}"
    );
    assert!(stdout.contains("Queued:"), "missing queued state: {stdout}");
    assert!(
        stdout.contains("Injected:"),
        "missing injected state: {stdout}"
    );
    assert!(!stdout.contains("helper-e2e") && !stdout.contains(TOKEN));
    assert_eq!(observed.post_text, "helper-e2e");
    assert_eq!(observed.post_target, "project:wg-1-team/member");
    assert_eq!(
        observed.get_path,
        format!("/api/v1/pty-input/{}", observed.op_id)
    );
    assert!(observed.authorization_ok);
}

#[derive(Debug)]
struct HelperObservation {
    op_id: String,
    post_target: String,
    post_text: String,
    get_path: String,
    authorization_ok: bool,
}

fn run_helper_http_server(listener: TcpListener) -> io::Result<HelperObservation> {
    let mut post = accept_with_timeout(&listener, Duration::from_secs(30))?;
    post.set_read_timeout(Some(Duration::from_secs(30)))?;
    post.set_write_timeout(Some(Duration::from_secs(30)))?;
    let (post_headers, post_body) = read_http_request(&mut post)?;
    let post_path = post_headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("");
    if post_path != "/api/v1/pty-input" {
        return Err(io::Error::other("unexpected helper POST path"));
    }
    let request: serde_json::Value =
        serde_json::from_slice(&post_body).map_err(io::Error::other)?;
    let op_id = request["opId"]
        .as_str()
        .ok_or_else(|| io::Error::other("missing helper op id"))?
        .to_string();
    let post_target = request["to"]
        .as_str()
        .ok_or_else(|| io::Error::other("missing helper target"))?
        .to_string();
    let post_text = request["ptyInput"]["text"]
        .as_str()
        .ok_or_else(|| io::Error::other("missing helper text"))?
        .to_string();
    let authorization_ok = post_headers
        .to_ascii_lowercase()
        .contains(&format!("authorization: bearer {TOKEN}"));
    let digest = "1ba7dd613c255580d75c7bf597a879108449a421c0e28e86f5442dffd2738c51";
    let injection_id = Uuid::new_v4().to_string();
    let queued = helper_result_json(
        &injection_id,
        &op_id,
        &post_target,
        post_text.len(),
        digest,
        false,
    );
    write_http_json(&mut post, 202, &queued)?;

    let mut get = accept_with_timeout(&listener, Duration::from_secs(30))?;
    get.set_read_timeout(Some(Duration::from_secs(30)))?;
    get.set_write_timeout(Some(Duration::from_secs(30)))?;
    let (get_headers, _) = read_http_request(&mut get)?;
    let get_path = get_headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or_else(|| io::Error::other("missing helper GET path"))?
        .to_string();
    let authorization_ok = authorization_ok
        && get_headers
            .to_ascii_lowercase()
            .contains(&format!("authorization: bearer {TOKEN}"));
    let injected = helper_result_json(
        &injection_id,
        &op_id,
        &post_target,
        post_text.len(),
        digest,
        true,
    );
    write_http_json(&mut get, 200, &injected)?;
    Ok(HelperObservation {
        op_id,
        post_target,
        post_text,
        get_path,
        authorization_ok,
    })
}

fn helper_result_json(
    injection_id: &str,
    op_id: &str,
    target: &str,
    payload_bytes: usize,
    payload_sha256: &str,
    injected: bool,
) -> Vec<u8> {
    let mut result = serde_json::json!({
        "version": 1,
        "injectionId": injection_id,
        "opId": op_id,
        "sender": "project:wg-1-team/coordinator",
        "target": target,
        "status": if injected { "injected" } else { "queued" },
        "terminal": injected,
        "payloadBytes": payload_bytes,
        "payloadSha256": payload_sha256,
        "sourcePlane": "container_api",
        "issuedAt": "2026-01-01T00:00:00.000Z",
        "expiresAt": "2026-01-01T00:10:00.000Z",
        "queuedAt": "2026-01-01T00:00:00.001Z"
    });
    if injected {
        result["selectedSessionId"] = serde_json::json!(Uuid::new_v4().to_string());
        result["selectedBackend"] = serde_json::json!("containerTransport");
        result["actuatingAt"] = serde_json::json!("2026-01-01T00:00:01.000Z");
        result["terminalAt"] = serde_json::json!("2026-01-01T00:00:02.000Z");
    }
    serde_json::to_vec(&result).expect("serialize helper result")
}

fn read_http_request(stream: &mut TcpStream) -> io::Result<(String, Vec<u8>)> {
    let headers = read_http_headers(stream)?;
    let content_length = header_value(&headers, "content-length")
        .map(|value| value.parse::<usize>())
        .transpose()
        .map_err(io::Error::other)?
        .unwrap_or(0);
    if content_length > 128 * 1024 {
        return Err(io::Error::other("helper request body too large"));
    }
    let mut body = vec![0_u8; content_length];
    stream.read_exact(&mut body)?;
    Ok((headers, body))
}

fn write_http_json(stream: &mut TcpStream, status: u16, body: &[u8]) -> io::Result<()> {
    let reason = if status == 202 { "Accepted" } else { "OK" };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("repo root")
        .to_path_buf()
}

fn assert_command_ok(command: &mut Command, label: &str) {
    let output = command
        .output()
        .unwrap_or_else(|err| panic!("{label}: {err}"));
    assert!(
        output.status.success(),
        "{label} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[derive(Debug)]
struct ServerObservation {
    headers: String,
    text_frames: Vec<String>,
    binary_output: Vec<u8>,
}

fn run_ws_server(listener: TcpListener) -> io::Result<ServerObservation> {
    let mut stream = accept_with_timeout(&listener, Duration::from_secs(30))?;
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;

    let headers = read_http_headers(&mut stream)?;
    let websocket_key = header_value(&headers, "sec-websocket-key")
        .ok_or_else(|| io::Error::other("missing Sec-WebSocket-Key"))?;
    write_upgrade_response(&mut stream, &websocket_key)?;

    let mut text_frames = Vec::new();
    let mut binary_output = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        let (opcode, payload) = read_ws_frame(&mut stream)?;
        match opcode {
            0x1 => {
                let text = String::from_utf8(payload).map_err(io::Error::other)?;
                let is_exit = serde_json::from_str::<serde_json::Value>(&text)
                    .ok()
                    .is_some_and(|value| value["type"] == "exit");
                text_frames.push(text);
                if is_exit {
                    stream.write_all(&[0x88, 0x00])?;
                    thread::sleep(Duration::from_millis(100));
                    break;
                }
            }
            0x2 => binary_output.extend(payload),
            0x8 => break,
            0x9 => stream.write_all(&[0x8a, 0x00])?,
            _ => {}
        }
    }

    Ok(ServerObservation {
        headers,
        text_frames,
        binary_output,
    })
}

fn accept_with_timeout(listener: &TcpListener, timeout: Duration) -> io::Result<TcpStream> {
    let deadline = Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((stream, _)) => return Ok(stream),
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "timed out waiting for bridge connection",
                    ));
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err(err) => return Err(err),
        }
    }
}

fn read_http_headers(stream: &mut TcpStream) -> io::Result<String> {
    let mut bytes = Vec::new();
    let mut buf = [0_u8; 1];
    while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
        let n = stream.read(&mut buf)?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "connection closed before HTTP headers",
            ));
        }
        bytes.push(buf[0]);
        if bytes.len() > 16 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "HTTP headers too large",
            ));
        }
    }
    String::from_utf8(bytes).map_err(io::Error::other)
}

fn header_value(headers: &str, key: &str) -> Option<String> {
    headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.trim().eq_ignore_ascii_case(key) {
            Some(value.trim().to_string())
        } else {
            None
        }
    })
}

fn write_upgrade_response(stream: &mut TcpStream, websocket_key: &str) -> io::Result<()> {
    let accept = websocket_accept(websocket_key);
    write!(
        stream,
        "HTTP/1.1 101 Switching Protocols\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Accept: {accept}\r\n\r\n"
    )
}

fn websocket_accept(websocket_key: &str) -> String {
    use base64::Engine;
    let digest = sha1_digest(format!("{websocket_key}{GUID}").as_bytes());
    base64::engine::general_purpose::STANDARD.encode(digest)
}

fn sha1_digest(input: &[u8]) -> [u8; 20] {
    use sha1::{Digest, Sha1};
    let mut hasher = Sha1::new();
    hasher.update(input);
    hasher.finalize().into()
}

fn read_ws_frame(stream: &mut TcpStream) -> io::Result<(u8, Vec<u8>)> {
    let mut header = [0_u8; 2];
    stream.read_exact(&mut header)?;
    let opcode = header[0] & 0x0f;
    let masked = header[1] & 0x80 != 0;
    let mut len = u64::from(header[1] & 0x7f);
    if len == 126 {
        let mut buf = [0_u8; 2];
        stream.read_exact(&mut buf)?;
        len = u64::from(u16::from_be_bytes(buf));
    } else if len == 127 {
        let mut buf = [0_u8; 8];
        stream.read_exact(&mut buf)?;
        len = u64::from_be_bytes(buf);
    }
    if len > 64 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "websocket frame too large",
        ));
    }

    let mut mask = [0_u8; 4];
    if masked {
        stream.read_exact(&mut mask)?;
    }
    let mut payload = vec![0_u8; len as usize];
    stream.read_exact(&mut payload)?;
    if masked {
        for (i, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[i % 4];
        }
    }
    Ok((opcode, payload))
}
