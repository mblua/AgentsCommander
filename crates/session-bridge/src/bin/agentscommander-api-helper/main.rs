use crate::config::{api_url, authorization_value, next_utf8, HelperResult};
use crate::pty_result::{
    canonical_uuid_v4, http_error_code, parse_api_error_code, parse_result, read_capped_response,
    sha256_hex, ExpectedPtyResult, PtyResult, PtyStatus,
};
use crate::snapshot::terminal_snapshot_command;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde::Serialize;
use serde_json::json;
use std::env;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::time::{Duration, Instant};
use uuid::Uuid;

mod config;
mod filesystem;
mod pty_result;
mod snapshot;

const INLINE_BODY_MAX_BYTES: usize = 256 * 1024;
const PTY_INPUT_MAX_BYTES: usize = 65_536;
const DEFAULT_CONFIRM_TIMEOUT_SECS: u64 = 90;

#[tokio::main]
async fn main() {
    match run().await {
        Ok(0) => {}
        Ok(code) => std::process::exit(code),
        Err(code) => {
            eprintln!("agentscommander-api-helper error: {code}");
            std::process::exit(1);
        }
    }
}

async fn run() -> HelperResult<i32> {
    let mut args = env::args_os().skip(1);
    let command = args.next().ok_or_else(|| "missing_command".to_string())?;
    let command = command
        .into_string()
        .map_err(|_| "invalid_command".to_string())?;
    let rest: Vec<OsString> = args.collect();
    match command.as_str() {
        "--help" | "-h" | "help" => {
            print_help();
            Ok(0)
        }
        "list-peers-lean" => {
            list_peers().await?;
            Ok(0)
        }
        "send" if is_help_request(&rest) => {
            print_send_help();
            Ok(0)
        }
        "send" => send(rest).await,
        "pty-input-status" => pty_input_status(rest).await,
        "terminal-snapshot" if is_help_request(&rest) => {
            print_terminal_snapshot_help();
            Ok(0)
        }
        "terminal-snapshot" => Ok(terminal_snapshot_command(rest).await),
        _ => Err("unknown_command".to_string()),
    }
}

fn is_help_request(args: &[OsString]) -> bool {
    args.len() == 1 && matches!(args[0].to_str(), Some("--help" | "-h"))
}

fn print_help() {
    println!(
        "agentscommander-api-helper \
         <list-peers-lean|send|pty-input-status|terminal-snapshot>\nRun \
         'agentscommander-api-helper send --help' for exact PTY-input \
         usage."
    );
}

fn print_terminal_snapshot_help() {
    println!(
        "Usage: agentscommander-api-helper terminal-snapshot --to \
         <exact-canonical-fqn> [--format json] [--timeout 15]\n       \
         agentscommander-api-helper terminal-snapshot --to \
         <exact-canonical-fqn> --format png --output \
         <absolute-new-file.png> [--timeout 15]"
    );
}

fn print_send_help() {
    println!(
        "Usage: agentscommander-api-helper send --to <fqn> (--pty-input \
         <text>|--pty-input=<text>|--pty-input-stdin) [--agent <id>] \
         [--confirm-timeout <seconds>]\nPTY input is exact UTF-8 text, not \
         an OS shell command. The caller's shell performs expansion before \
         this helper receives arguments. Prefer --pty-input-stdin for \
         multiline, leading-hyphen, clipboard, process-list-sensitive, or \
         otherwise sensitive text. Queued is not Injected; after a timeout \
         keep the operation ID and do not resubmit."
    );
}

async fn list_peers() -> HelperResult<()> {
    let client = reqwest::Client::new();
    let response = client
        .get(format!("{}/api/v1/peers", api_url()?))
        .header(AUTHORIZATION, authorization_value()?)
        .send()
        .await
        .map_err(|_| "request_failed".to_string())?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|_| "invalid_server_response".to_string())?;
    if !status.is_success() {
        return Err(format!("list_peers_failed_http_{}", status.as_u16()));
    }
    println!("{body}");
    Ok(())
}

#[derive(Default)]
struct SendOptions {
    to: Option<String>,
    file: Option<String>,
    message: Option<String>,
    pty_input: Option<OsString>,
    pty_input_stdin: bool,
    agent: Option<String>,
    mode: String,
    confirm_timeout: u64,
}

fn parse_send_options(args: Vec<OsString>) -> HelperResult<SendOptions> {
    let mut options = SendOptions {
        mode: "wake".to_string(),
        confirm_timeout: DEFAULT_CONFIRM_TIMEOUT_SECS,
        ..SendOptions::default()
    };
    let mut iter = args.into_iter();
    while let Some(argument) = iter.next() {
        let argument = argument
            .into_string()
            .map_err(|_| "invalid_argument".to_string())?;
        if let Some(value) = argument.strip_prefix("--pty-input=") {
            if options.pty_input.is_some() || options.pty_input_stdin {
                return Err("exactly_one_payload_required".to_string());
            }
            options.pty_input = Some(OsString::from(value));
            continue;
        }
        match argument.as_str() {
            "--to" => options.to = Some(next_utf8(&mut iter, "invalid_target")?),
            "--send" => options.file = Some(next_utf8(&mut iter, "invalid_send_file")?),
            "--message" => options.message = Some(next_utf8(&mut iter, "invalid_message")?),
            "--pty-input" => {
                if options.pty_input.is_some() || options.pty_input_stdin {
                    return Err("exactly_one_payload_required".to_string());
                }
                options.pty_input = Some(iter.next().ok_or_else(|| "invalid_text".to_string())?)
            }
            "--pty-input-stdin" => {
                if options.pty_input.is_some() || options.pty_input_stdin {
                    return Err("exactly_one_payload_required".to_string());
                }
                options.pty_input_stdin = true;
            }
            "--agent" => options.agent = Some(next_utf8(&mut iter, "unsupported_profile")?),
            "--mode" => options.mode = next_utf8(&mut iter, "invalid_mode")?,
            "--confirm-timeout" => {
                let raw = next_utf8(&mut iter, "invalid_timeout")?;
                options.confirm_timeout = raw
                    .parse::<u64>()
                    .ok()
                    .filter(|value| *value <= 3_600)
                    .ok_or_else(|| "invalid_timeout".to_string())?;
            }
            "--get-output" | "--outbox" | "--token" | "--root" => {
                return Err("mixed_payload".to_string())
            }
            _ => return Err("unknown_send_argument".to_string()),
        }
    }
    if options.mode != "wake" {
        return Err("invalid_mode".to_string());
    }
    Ok(options)
}

async fn send(args: Vec<OsString>) -> HelperResult<i32> {
    let options = parse_send_options(args)?;
    let forms = usize::from(options.file.is_some())
        + usize::from(options.message.is_some())
        + usize::from(options.pty_input.is_some())
        + usize::from(options.pty_input_stdin);
    if forms != 1 {
        return Err("exactly_one_payload_required".to_string());
    }
    if options.pty_input.is_some() || options.pty_input_stdin {
        send_pty_input(options).await
    } else {
        send_standard(options).await?;
        Ok(0)
    }
}

async fn send_standard(options: SendOptions) -> HelperResult<()> {
    if options.agent.is_some() {
        return Err("unsupported_standard_argument".to_string());
    }
    let to = options.to.ok_or_else(|| "invalid_target".to_string())?;
    let body = match (options.file, options.message) {
        (Some(_), Some(_)) => return Err("exactly_one_payload_required".to_string()),
        (None, None) => return Err("exactly_one_payload_required".to_string()),
        (Some(path), None) => crate::filesystem::read_send_message(&path)
            .map_err(|_| "send_file_unreadable".to_string())?,
        (None, Some(text)) => text,
    };
    if body.len() > INLINE_BODY_MAX_BYTES {
        return Err("payload_too_large".to_string());
    }
    let request = json!({
        "apiVersion": "1",
        "opId": Uuid::new_v4().to_string(),
        "to": to,
        "message": {"inline": body, "contentType": "text/markdown"}
    });
    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/send", api_url()?))
        .header(AUTHORIZATION, authorization_value()?)
        .header(CONTENT_TYPE, "application/json")
        .json(&request)
        .send()
        .await
        .map_err(|_| "request_failed".to_string())?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|_| "invalid_server_response".to_string())?;
    if !status.is_success() {
        return Err(format!("send_failed_http_{}", status.as_u16()));
    }
    println!("{text}");
    Ok(())
}

fn read_bounded_stdin<R: Read>(reader: R) -> HelperResult<String> {
    let mut bytes = Vec::with_capacity(PTY_INPUT_MAX_BYTES + 1);
    reader
        .take(PTY_INPUT_MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "invalid_text".to_string())?;
    if bytes.len() > PTY_INPUT_MAX_BYTES {
        return Err("payload_too_large".to_string());
    }
    String::from_utf8(bytes).map_err(|_| "invalid_text".to_string())
}

fn validate_pty_text(text: &str) -> HelperResult<()> {
    if text.is_empty() {
        return Err("invalid_text".to_string());
    }
    if text.len() > PTY_INPUT_MAX_BYTES {
        return Err("payload_too_large".to_string());
    }
    if text.chars().any(|ch| {
        matches!(
            ch,
            '\u{0000}'..='\u{0008}'
                | '\u{000b}'
                | '\u{000c}'
                | '\u{000d}'
                | '\u{000e}'..='\u{001f}'
                | '\u{007f}'..='\u{009f}'
                | '\u{061c}'
                | '\u{200e}'
                | '\u{200f}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}'
        )
    }) {
        return Err("invalid_text".to_string());
    }
    Ok(())
}

fn validate_agent_id(agent: Option<String>) -> HelperResult<Option<String>> {
    match agent.as_deref() {
        None | Some("auto") => Ok(None),
        Some(value)
            if value.len() <= 64
                && value.bytes().enumerate().all(|(index, byte)| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || (index > 0 && matches!(byte, b'_' | b'-'))
                })
                && value
                    .as_bytes()
                    .first()
                    .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit()) =>
        {
            Ok(Some(value.to_string()))
        }
        Some(_) => Err("unsupported_profile".to_string()),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PtyPostRequest {
    api_version: &'static str,
    op_id: String,
    to: String,
    pty_input: PtyPostPayload,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_id: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PtyPostPayload {
    version: u32,
    text: String,
    enter: &'static str,
}

async fn get_pty_status_at(
    client: &reqwest::Client,
    base_url: &str,
    authorization: &str,
    op_id: &str,
    expected: Option<&ExpectedPtyResult<'_>>,
) -> HelperResult<Option<PtyResult>> {
    let response = client
        .get(format!("{base_url}/api/v1/pty-input/{op_id}"))
        .header(AUTHORIZATION, authorization)
        .send()
        .await
        .map_err(|_| "request_failed".to_string())?;
    let status = response.status();
    let bytes = match read_capped_response(response).await {
        Ok(bytes) => bytes,
        Err(_) if status.is_success() => return Err("invalid_server_response".to_string()),
        Err(_) => {
            return Err(format!(
                "status_failed_http_{}_invalid_server_response",
                status.as_u16()
            ));
        }
    };
    if status == reqwest::StatusCode::NOT_FOUND {
        let code = parse_api_error_code(&bytes).map_err(|_| {
            format!(
                "status_failed_http_{}_invalid_server_response",
                status.as_u16()
            )
        })?;
        if code == "not_found" {
            return Ok(None);
        }
        return Err(format!(
            "status_failed_http_{}_invalid_server_response",
            status.as_u16()
        ));
    }
    if !status.is_success() {
        return Err(http_error_code("status_failed", status, &bytes));
    }
    parse_result(&bytes, op_id, expected).map(Some)
}

async fn submit_pty_request(
    client: &reqwest::Client,
    base_url: &str,
    authorization: &str,
    request_bytes: &[u8],
    op_id: &str,
    expected: &ExpectedPtyResult<'_>,
) -> HelperResult<PtyResult> {
    for delay in [0_u64, 250, 500] {
        if delay > 0 {
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }
        let response = client
            .post(format!("{base_url}/api/v1/pty-input"))
            .header(AUTHORIZATION, authorization)
            .header(CONTENT_TYPE, "application/json")
            .body(request_bytes.to_vec())
            .send()
            .await;
        match response {
            Ok(response) => {
                let status = response.status();
                let bytes = read_capped_response(response).await;
                if status.is_success() {
                    if let Ok(result) = bytes
                        .as_deref()
                        .map_err(|_| "invalid_server_response".to_string())
                        .and_then(|bytes| parse_result(bytes, op_id, Some(expected)))
                    {
                        return Ok(result);
                    }
                    match get_pty_status_at(client, base_url, authorization, op_id, Some(expected))
                        .await
                    {
                        Ok(Some(result)) => return Ok(result),
                        Ok(None) | Err(_) => continue,
                    }
                }
                if status.is_client_error() {
                    let bytes = bytes.map_err(|_| {
                        format!(
                            "pty_input_failed_http_{}_invalid_server_response",
                            status.as_u16()
                        )
                    })?;
                    return Err(http_error_code("pty_input_failed", status, &bytes));
                }
                match get_pty_status_at(client, base_url, authorization, op_id, Some(expected))
                    .await
                {
                    Ok(Some(result)) => return Ok(result),
                    Ok(None) | Err(_) => continue,
                }
            }
            Err(_) => {
                match get_pty_status_at(client, base_url, authorization, op_id, Some(expected))
                    .await
                {
                    Ok(Some(result)) => return Ok(result),
                    Ok(None) | Err(_) => continue,
                }
            }
        }
    }
    Err("ambiguous_post".to_string())
}

async fn send_pty_input(options: SendOptions) -> HelperResult<i32> {
    let to = options.to.ok_or_else(|| "invalid_target".to_string())?;
    let text = match (options.pty_input, options.pty_input_stdin) {
        (Some(_), true) => return Err("exactly_one_payload_required".to_string()),
        (None, false) => return Err("exactly_one_payload_required".to_string()),
        (Some(value), false) => value
            .into_string()
            .map_err(|_| "invalid_text".to_string())?,
        (None, true) => {
            let stdin = std::io::stdin();
            read_bounded_stdin(stdin.lock())?
        }
    };
    validate_pty_text(&text)?;
    let agent_id = validate_agent_id(options.agent)?;
    let payload_bytes = text.len() as u64;
    let payload_sha256 = sha256_hex(text.as_bytes());
    let expected = ExpectedPtyResult {
        target: &to,
        payload_bytes,
        payload_sha256: &payload_sha256,
    };
    let op_id = Uuid::new_v4().to_string();
    println!("Operation ID: {op_id}");
    std::io::stdout()
        .flush()
        .map_err(|_| "output_failed".to_string())?;
    let request = PtyPostRequest {
        api_version: "1",
        op_id: op_id.clone(),
        to: to.clone(),
        pty_input: PtyPostPayload {
            version: 1,
            text,
            enter: "agent-submit",
        },
        agent_id,
    };
    let request_bytes = serde_json::to_vec(&request).map_err(|_| "invalid_request".to_string())?;
    let client = reqwest::Client::new();
    let base_url = api_url()?;
    let authorization = authorization_value()?;
    let mut result = submit_pty_request(
        &client,
        &base_url,
        &authorization,
        &request_bytes,
        &op_id,
        &expected,
    )
    .await?;
    println!("Queued: {op_id} (queued is not injected)");
    let deadline = Instant::now() + Duration::from_secs(options.confirm_timeout);
    loop {
        match result.status {
            PtyStatus::Injected => {
                println!("Injected: {op_id}");
                return Ok(0);
            }
            PtyStatus::Rejected => {
                let code = result
                    .reason
                    .as_ref()
                    .map(|reason| reason.code.as_str())
                    .unwrap_or("invalid_server_response");
                eprintln!("Rejected: {op_id} code={code}");
                return Ok(1);
            }
            PtyStatus::Indeterminate => {
                let code = result
                    .reason
                    .as_ref()
                    .map(|reason| reason.code.as_str())
                    .unwrap_or("invalid_server_response");
                eprintln!("Indeterminate: {op_id} code={code}; do not resubmit");
                return Ok(1);
            }
            PtyStatus::Queued | PtyStatus::Actuating => {}
        }
        if Instant::now() >= deadline {
            eprintln!(
                "Confirmation timeout: keep operation {op_id}; do not resubmit. \
                 Run: agentscommander-api-helper pty-input-status --op-id {op_id}"
            );
            return Ok(1);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
        result = get_pty_status_at(&client, &base_url, &authorization, &op_id, Some(&expected))
            .await?
            .ok_or_else(|| "operation_not_found_after_enqueue".to_string())?;
    }
}

async fn pty_input_status(args: Vec<OsString>) -> HelperResult<i32> {
    let mut iter = args.into_iter();
    let mut op_id = None;
    while let Some(argument) = iter.next() {
        let argument = argument
            .into_string()
            .map_err(|_| "invalid_argument".to_string())?;
        match argument.as_str() {
            "--op-id" => op_id = Some(next_utf8(&mut iter, "invalid_id")?),
            _ => return Err("invalid_status_argument".to_string()),
        }
    }
    let op_id = op_id.ok_or_else(|| "invalid_id".to_string())?;
    if !canonical_uuid_v4(&op_id) {
        return Err("invalid_id".to_string());
    }
    let result = get_pty_status_at(
        &reqwest::Client::new(),
        &api_url()?,
        &authorization_value()?,
        &op_id,
        None,
    )
    .await?
    .ok_or_else(|| "operation_not_found".to_string())?;
    let output =
        serde_json::to_string(&result).map_err(|_| "invalid_server_response".to_string())?;
    println!("{output}");
    Ok(0)
}

#[cfg(test)]
mod tests;
