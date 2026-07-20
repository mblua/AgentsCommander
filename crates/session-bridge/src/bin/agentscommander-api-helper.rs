use std::env;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::time::{Duration, Instant};

use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::{Uuid, Version};

const INLINE_BODY_MAX_BYTES: usize = 256 * 1024;
const PTY_INPUT_MAX_BYTES: usize = 65_536;
const PTY_RESPONSE_MAX_BYTES: usize = 16 * 1024;
const DEFAULT_CONFIRM_TIMEOUT_SECS: u64 = 90;

type HelperResult<T> = Result<T, String>;

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
        _ => Err("unknown_command".to_string()),
    }
}

fn is_help_request(args: &[OsString]) -> bool {
    args.len() == 1 && matches!(args[0].to_str(), Some("--help" | "-h"))
}

fn print_help() {
    println!(
        "agentscommander-api-helper <list-peers-lean|send|pty-input-status>\nRun 'agentscommander-api-helper send --help' for exact PTY-input usage."
    );
}

fn print_send_help() {
    println!(
        "Usage: agentscommander-api-helper send --to <fqn> (--pty-input <text>|--pty-input=<text>|--pty-input-stdin) [--agent <id>] [--confirm-timeout <seconds>]\nPTY input is exact UTF-8 text, not an OS shell command. The caller's shell performs expansion before this helper receives arguments. Prefer --pty-input-stdin for multiline, leading-hyphen, clipboard, process-list-sensitive, or otherwise sensitive text. Queued is not Injected; after a timeout keep the operation ID and do not resubmit."
    );
}

fn api_url() -> HelperResult<String> {
    env::var("AGENTSCOMMANDER_API_URL")
        .map(|value| value.trim_end_matches('/').to_string())
        .map_err(|_| "api_url_unavailable".to_string())
}

fn api_token() -> HelperResult<String> {
    env::var("AGENTSCOMMANDER_API_TOKEN").map_err(|_| "api_token_unavailable".to_string())
}

fn authorization_value() -> HelperResult<String> {
    Ok(format!("Bearer {}", api_token()?))
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

fn next_utf8(
    iter: &mut impl Iterator<Item = OsString>,
    code: &'static str,
) -> HelperResult<String> {
    iter.next()
        .ok_or_else(|| code.to_string())?
        .into_string()
        .map_err(|_| code.to_string())
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
        (Some(path), None) => {
            std::fs::read_to_string(path).map_err(|_| "send_file_unreadable".to_string())?
        }
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

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PtyResult {
    version: u32,
    injection_id: String,
    #[serde(default)]
    op_id: Option<String>,
    #[serde(default)]
    sender: Option<String>,
    #[serde(default)]
    target: Option<String>,
    status: PtyStatus,
    terminal: bool,
    #[serde(default)]
    payload_bytes: Option<u64>,
    #[serde(default)]
    payload_sha256: Option<String>,
    #[serde(default)]
    source_plane: Option<String>,
    #[serde(default)]
    selected_session_id: Option<String>,
    #[serde(default)]
    selected_backend: Option<String>,
    #[serde(default)]
    issued_at: Option<String>,
    #[serde(default)]
    expires_at: Option<String>,
    #[serde(default)]
    queued_at: Option<String>,
    #[serde(default)]
    actuating_at: Option<String>,
    #[serde(default)]
    terminal_at: Option<String>,
    #[serde(default)]
    reason: Option<PtyReason>,
}

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum PtyStatus {
    Queued,
    Actuating,
    Injected,
    Rejected,
    Indeterminate,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PtyReason {
    code: String,
    detail: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ApiErrorResponse {
    api_version: String,
    error: String,
    detail: String,
}

fn parse_api_error_code(bytes: &[u8]) -> HelperResult<String> {
    let response: ApiErrorResponse =
        serde_json::from_slice(bytes).map_err(|_| "invalid_server_response".to_string())?;
    if response.api_version != "1"
        || response.error.is_empty()
        || response.error.len() > 64
        || response
            .error
            .bytes()
            .any(|byte| !(byte.is_ascii_lowercase() || byte == b'_'))
        || response.detail.is_empty()
        || response.detail.len() > 1_024
        || response.detail.chars().any(char::is_control)
    {
        return Err("invalid_server_response".to_string());
    }
    if let Some(detail) = safe_reason_detail(&response.error) {
        if response.detail != detail {
            return Err("invalid_server_response".to_string());
        }
    } else if !matches!(
        response.error.as_str(),
        "bad_request"
            | "unauthorized"
            | "forbidden"
            | "not_found"
            | "rejected"
            | "rate_limited"
            | "service_unavailable"
            | "internal"
    ) {
        return Err("invalid_server_response".to_string());
    }
    Ok(response.error)
}

fn http_error_code(prefix: &str, status: reqwest::StatusCode, bytes: &[u8]) -> String {
    match parse_api_error_code(bytes) {
        Ok(code) => format!("{prefix}_http_{}_{code}", status.as_u16()),
        Err(_) => format!("{prefix}_http_{}_invalid_server_response", status.as_u16()),
    }
}

fn canonical_uuid_v4(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|id| {
        !id.is_nil()
            && id.get_version() == Some(Version::Random)
            && id.hyphenated().to_string() == value
    })
}

fn canonical_timestamp_millis(value: &str) -> Option<i64> {
    let bytes = value.as_bytes();
    if bytes.len() != 24
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || bytes[19] != b'.'
        || bytes[23] != b'Z'
    {
        return None;
    }
    let digit_positions = [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18, 20, 21, 22];
    if !digit_positions
        .iter()
        .all(|index| bytes[*index].is_ascii_digit())
    {
        return None;
    }
    let number = |start: usize, end: usize| {
        std::str::from_utf8(&bytes[start..end])
            .ok()
            .and_then(|raw| raw.parse::<u32>().ok())
    };
    let year = number(0, 4)?;
    let month = number(5, 7)?;
    let day = number(8, 10)?;
    let hour = number(11, 13)?;
    let minute = number(14, 16)?;
    let second = number(17, 19)?;
    let millis = number(20, 23)?;
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let month_days = [
        31_u32,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if !(1..=12).contains(&month)
        || day == 0
        || day > month_days[(month - 1) as usize]
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    let days_before_year = i64::from(year) * 365 + i64::from(year.div_ceil(4))
        - i64::from(year.div_ceil(100))
        + i64::from(year.div_ceil(400));
    let days_before_month: i64 = month_days[..(month - 1) as usize]
        .iter()
        .map(|days| i64::from(*days))
        .sum();
    let days = days_before_year + days_before_month + i64::from(day - 1);
    Some(
        (((days * 24 + i64::from(hour)) * 60 + i64::from(minute)) * 60 + i64::from(second)) * 1_000
            + i64::from(millis),
    )
}

#[cfg(test)]
fn canonical_timestamp(value: &str) -> bool {
    canonical_timestamp_millis(value).is_some()
}

fn forbidden_identity_scalar(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{061c}'
                | '\u{200e}'
                | '\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}'
        )
}

fn canonical_agent_fqn(value: &str) -> bool {
    if value.len() > 1_024 || value.matches(':').count() != 1 {
        return false;
    }
    let Some((project, local)) = value.split_once(':') else {
        return false;
    };
    let Some((workgroup, agent)) = local.split_once('/') else {
        return false;
    };
    if project.is_empty()
        || matches!(project, "." | "..")
        || local.matches('/').count() != 1
        || project.chars().any(|character| {
            forbidden_identity_scalar(character)
                || matches!(character, '/' | '\\' | '*' | '?' | '"' | '<' | '>' | '|')
        })
        || agent.is_empty()
        || !agent
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return false;
    }
    let Some(rest) = workgroup.strip_prefix("wg-") else {
        return false;
    };
    let Some((digits, team)) = rest.split_once('-') else {
        return false;
    };
    !digits.is_empty()
        && digits.bytes().all(|byte| byte.is_ascii_digit())
        && !team.is_empty()
        && team
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn sha256_hex(input: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut state = [
        0x6a09e667_u32,
        0xbb67ae85,
        0x3c6ef372,
        0xa54ff53a,
        0x510e527f,
        0x9b05688c,
        0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_len = (input.len() as u64).wrapping_mul(8);
    let mut padded = Vec::with_capacity(input.len().saturating_add(72));
    padded.extend_from_slice(input);
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in padded.chunks_exact(64) {
        let mut words = [0_u32; 64];
        for (index, bytes) in chunk.chunks_exact(4).enumerate() {
            words[index] = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        }
        for index in 16..64 {
            let s0 = words[index - 15].rotate_right(7)
                ^ words[index - 15].rotate_right(18)
                ^ (words[index - 15] >> 3);
            let s1 = words[index - 2].rotate_right(17)
                ^ words[index - 2].rotate_right(19)
                ^ (words[index - 2] >> 10);
            words[index] = words[index - 16]
                .wrapping_add(s0)
                .wrapping_add(words[index - 7])
                .wrapping_add(s1);
        }
        let mut working = state;
        for index in 0..64 {
            let choice = (working[4] & working[5]) ^ ((!working[4]) & working[6]);
            let majority =
                (working[0] & working[1]) ^ (working[0] & working[2]) ^ (working[1] & working[2]);
            let sum1 = working[4].rotate_right(6)
                ^ working[4].rotate_right(11)
                ^ working[4].rotate_right(25);
            let sum0 = working[0].rotate_right(2)
                ^ working[0].rotate_right(13)
                ^ working[0].rotate_right(22);
            let temp1 = working[7]
                .wrapping_add(sum1)
                .wrapping_add(choice)
                .wrapping_add(K[index])
                .wrapping_add(words[index]);
            let temp2 = sum0.wrapping_add(majority);
            working = [
                temp1.wrapping_add(temp2),
                working[0],
                working[1],
                working[2],
                working[3].wrapping_add(temp1),
                working[4],
                working[5],
                working[6],
            ];
        }
        for (value, delta) in state.iter_mut().zip(working) {
            *value = value.wrapping_add(delta);
        }
    }
    state.iter().map(|word| format!("{word:08x}")).collect()
}

fn safe_reason_detail(code: &str) -> Option<&'static str> {
    Some(match code {
        "invalid_envelope" => "The PTY input envelope is invalid.",
        "mixed_payload" => "PTY input cannot be mixed with another payload type.",
        "unsupported_version" => "The PTY input contract version is unsupported.",
        "invalid_enter_mode" => "The PTY input Enter mode is invalid.",
        "invalid_id" => "A canonical UUID version 4 operation ID is required.",
        "invalid_nonce" => "A distinct canonical UUID version 4 nonce is required.",
        "invalid_timestamp" => "The PTY input timestamp is invalid.",
        "expired" => "The PTY input operation expired before actuation.",
        "invalid_target" => "An exact canonical workgroup target is required.",
        "invalid_text" => "The PTY input text contains a forbidden scalar or is empty.",
        "payload_too_large" => "The PTY input text exceeds 65,536 UTF-8 bytes.",
        "idempotency_conflict" => "The operation ID was already used with different semantics.",
        "capacity_exceeded" => "The privileged PTY input queue is at capacity.",
        "session_token_required" => "A live session token is required for PTY input.",
        "invalid_session_token" => "The PTY input session token is invalid or stale.",
        "ambiguous_session_token" => "The PTY input session token is not unique.",
        "sender_session_not_live" => "The sender session is not live.",
        "sender_backend_not_local" => "The filesystem PTY input plane requires a local sender.",
        "sender_identity_invalid" => "The sender replica identity could not be verified.",
        "sender_not_coordinator" => "The sender is not the verified workgroup coordinator.",
        "root_identity_invalid" => "The live Root Agent identity could not be verified.",
        "target_not_member" => "The target is not a verified member of the sender workgroup.",
        "target_is_coordinator" => "A coordinator cannot target a coordinator on this route.",
        "target_out_of_scope" => "The target is outside the sender's PTY input authority.",
        "unsafe_path" => "A privileged path failed confinement or link safety checks.",
        "api_scope_required" => "The API token is not scoped for PTY input.",
        "api_client_unbound" => "A live automatically bound container credential is required.",
        "api_client_stale" => "The bound API credential is revoked, expired, or stale.",
        "api_binding_mismatch" => "The API credential does not match the live container route.",
        "authority_changed" => "Sender or target authority changed before actuation.",
        "busy" => "The target coding agent is busy.",
        "resize_unsettled" => "The target terminal resize state is not settled.",
        "untracked_readiness" => "The target readiness state is incomplete.",
        "unsupported_session" => "The target session is not a trusted coding-agent session.",
        "nonpersistent_live_session" => "A live nonpersistent target session prevents actuation.",
        "inconsistent_session" => "The target session lifecycle is inconsistent.",
        "unsupported_profile" => "No trusted supported coding-agent profile is available.",
        "readiness_timeout" => "The spawned coding agent did not become ready in time.",
        "store_corrupt" => "The privileged operation store failed an integrity check.",
        "restore_in_progress" => "Session restore temporarily blocks target preparation.",
        "purge_in_progress" => "Workgroup purge temporarily blocks target preparation.",
        "session_race" => "The target session changed during preparation.",
        "lease_lost" => "The preparation lease was lost before actuation.",
        "spawn_failed_safe" => "Target spawn failed without leaving an ambiguous session.",
        "store_transient" => "A transient operation-store failure prevented actuation.",
        "final_revalidation_failed" => {
            "Final authority or readiness validation failed after the no-replay boundary."
        }
        "text_write_failed" => "The exact text write could not be durably proven.",
        "required_enter_failed" => "The required Enter write could not be durably proven.",
        "daemon_restart_after_actuation" => "The daemon restarted after the no-replay boundary.",
        "runtime_actuation_orphan" => {
            "The actuation owner disappeared after the no-replay boundary."
        }
        "terminal_store_failed" => "The terminal result could not be durably recorded.",
        "redundant_enter_failed" => {
            "The redundant second Enter failed after successful submission."
        }
        "boundary_metadata_failed" => {
            "Submission succeeded but boundary metadata could not be completed."
        }
        "artifact_unclaimed" => "A host terminal artifact was not confirmed before compaction.",
        _ => return None,
    })
}

struct ExpectedPtyResult<'a> {
    target: &'a str,
    payload_bytes: u64,
    payload_sha256: &'a str,
}

async fn read_capped_response(mut response: reqwest::Response) -> HelperResult<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > PTY_RESPONSE_MAX_BYTES as u64)
    {
        return Err("invalid_server_response".to_string());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "invalid_server_response".to_string())?
    {
        if body.len().saturating_add(chunk.len()) > PTY_RESPONSE_MAX_BYTES {
            return Err("invalid_server_response".to_string());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn parse_result(
    bytes: &[u8],
    expected_op_id: &str,
    expected: Option<&ExpectedPtyResult<'_>>,
) -> HelperResult<PtyResult> {
    let result: PtyResult =
        serde_json::from_slice(bytes).map_err(|_| "invalid_server_response".to_string())?;
    let terminal = matches!(
        result.status,
        PtyStatus::Injected | PtyStatus::Rejected | PtyStatus::Indeterminate
    );
    let identifiers_valid = result.version == 1
        && result.op_id.as_deref() == Some(expected_op_id)
        && canonical_uuid_v4(expected_op_id)
        && canonical_uuid_v4(&result.injection_id)
        && result.sender.as_deref().is_some_and(canonical_agent_fqn)
        && result.target.as_deref().is_some_and(canonical_agent_fqn);
    let payload_valid = matches!(result.payload_bytes, Some(1..=65_536))
        && result.payload_sha256.as_deref().is_some_and(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        && result.source_plane.as_deref() == Some("container_api");
    let issued_millis = result
        .issued_at
        .as_deref()
        .and_then(canonical_timestamp_millis);
    let expires_millis = result
        .expires_at
        .as_deref()
        .and_then(canonical_timestamp_millis);
    let queued_millis = result
        .queued_at
        .as_deref()
        .and_then(canonical_timestamp_millis);
    let actuating_millis = match result.actuating_at.as_deref() {
        None => Some(None),
        Some(value) => canonical_timestamp_millis(value).map(Some),
    };
    let terminal_millis = match result.terminal_at.as_deref() {
        None => Some(None),
        Some(value) => canonical_timestamp_millis(value).map(Some),
    };
    let timestamps_valid = matches!(
        (
            issued_millis,
            expires_millis,
            queued_millis,
            actuating_millis,
            terminal_millis,
        ),
        (
            Some(issued),
            Some(expires),
            Some(queued),
            Some(actuating),
            Some(terminal),
        ) if expires - issued == 10 * 60 * 1_000
            && queued >= issued
            && queued < expires
            && actuating.is_none_or(|value| value >= queued && value < expires)
            && terminal.is_none_or(|value| {
                value >= queued && actuating.is_none_or(|at| value >= at)
            })
    );
    let selected_valid = result.selected_session_id.is_some() == result.selected_backend.is_some()
        && result
            .selected_session_id
            .as_deref()
            .is_none_or(canonical_uuid_v4)
        && result
            .selected_backend
            .as_deref()
            .is_none_or(|backend| matches!(backend, "localProcess" | "containerTransport"));
    let reason_valid = result
        .reason
        .as_ref()
        .is_none_or(|reason| safe_reason_detail(&reason.code) == Some(reason.detail.as_str()));
    let state_valid = result.terminal == terminal
        && result.terminal_at.is_some() == terminal
        && match result.status {
            PtyStatus::Queued => {
                result.actuating_at.is_none()
                    && result.selected_session_id.is_none()
                    && result.reason.as_ref().is_none_or(|reason| {
                        matches!(
                            reason.code.as_str(),
                            "restore_in_progress"
                                | "purge_in_progress"
                                | "session_race"
                                | "lease_lost"
                                | "spawn_failed_safe"
                                | "store_transient"
                        )
                    })
            }
            PtyStatus::Actuating => {
                result.actuating_at.is_some()
                    && result.selected_session_id.is_some()
                    && result.reason.is_none()
            }
            PtyStatus::Injected => {
                result.actuating_at.is_some()
                    && result.selected_session_id.is_some()
                    && result.reason.as_ref().is_none_or(|reason| {
                        matches!(
                            reason.code.as_str(),
                            "redundant_enter_failed" | "boundary_metadata_failed"
                        )
                    })
            }
            PtyStatus::Rejected => {
                result.actuating_at.is_none()
                    && result.selected_session_id.is_none()
                    && result.reason.as_ref().is_some_and(|reason| {
                        !matches!(
                            reason.code.as_str(),
                            "final_revalidation_failed"
                                | "text_write_failed"
                                | "required_enter_failed"
                                | "daemon_restart_after_actuation"
                                | "runtime_actuation_orphan"
                                | "terminal_store_failed"
                                | "redundant_enter_failed"
                                | "boundary_metadata_failed"
                                | "artifact_unclaimed"
                        )
                    })
            }
            PtyStatus::Indeterminate => {
                result.actuating_at.is_some()
                    && result.selected_session_id.is_some()
                    && result.reason.as_ref().is_some_and(|reason| {
                        matches!(
                            reason.code.as_str(),
                            "final_revalidation_failed"
                                | "text_write_failed"
                                | "required_enter_failed"
                                | "daemon_restart_after_actuation"
                                | "runtime_actuation_orphan"
                                | "terminal_store_failed"
                        )
                    })
            }
        };
    let expected_valid = expected.is_none_or(|expected| {
        result.target.as_deref() == Some(expected.target)
            && result.payload_bytes == Some(expected.payload_bytes)
            && result.payload_sha256.as_deref() == Some(expected.payload_sha256)
    });
    if !identifiers_valid
        || !payload_valid
        || !timestamps_valid
        || !selected_valid
        || !reason_valid
        || !state_valid
        || !expected_valid
    {
        return Err("invalid_server_response".to_string());
    }
    Ok(result)
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
                "Confirmation timeout: keep operation {op_id}; do not resubmit. Run: agentscommander-api-helper pty-input-status --op-id {op_id}"
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
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct Fixture {
        name: String,
        text: String,
        valid: bool,
    }

    enum ScriptedReply {
        DropConnection,
        Http(u16, Vec<u8>),
    }

    #[derive(Clone)]
    struct CapturedRequest {
        method: String,
        path: String,
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
            body: bytes[header_end..header_end + content_length].to_vec(),
        }
    }

    async fn scripted_server(
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
                            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = stream.write_all(headers.as_bytes()).await;
                        let _ = stream.write_all(&body).await;
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
            "../../tests/fixtures/pty_input_validation.json"
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
        assert!(
            parse_send_options(vec!["--pty-input=x".into(), "--pty-input-stdin".into(),]).is_err()
        );
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
        assert!(canonical_agent_fqn("project:wg-1-team/member"));
        for invalid in [
            ".:wg-1-team/member",
            "..:wg-1-team/member",
            "pro\nject:wg-1-team/member",
            "pro\u{202e}ject:wg-1-team/member",
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
}
