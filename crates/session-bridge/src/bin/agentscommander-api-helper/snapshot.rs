use crate::config::{api_url, authorization_value, next_utf8};
use crate::output::{create_snapshot_output, validate_snapshot_output_path};
use reqwest::header::{
    ACCEPT_ENCODING, AUTHORIZATION, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE,
    PRAGMA,
};
use std::ffi::OsString;
use std::io::Write;
use std::time::{Duration, Instant};
use uuid::Uuid;

struct TerminalSnapshotOptions {
    to: Option<String>,
    format: terminal_snapshot_renderer::TerminalSnapshotFormat,
    output: Option<std::path::PathBuf>,
    timeout: u64,
    saw_format: bool,
    saw_timeout: bool,
}

impl std::fmt::Debug for TerminalSnapshotOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TerminalSnapshotOptions")
            .field("has_target", &self.to.is_some())
            .field("format", &self.format)
            .field("has_output", &self.output.is_some())
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

fn parse_terminal_snapshot_options(
    args: Vec<OsString>,
) -> Result<TerminalSnapshotOptions, terminal_snapshot_renderer::TerminalSnapshotReasonCode> {
    use terminal_snapshot_renderer::{
        TerminalSnapshotFormat as F, TerminalSnapshotReasonCode as C,
    };

    let mut options = TerminalSnapshotOptions {
        to: None,
        format: F::Json,
        output: None,
        timeout: 15,
        saw_format: false,
        saw_timeout: false,
    };
    let mut iter = args.into_iter();
    while let Some(argument) = iter.next() {
        let argument = argument.into_string().map_err(|_| C::InvalidRequest)?;
        match argument.as_str() {
            "--to" if options.to.is_none() => {
                options.to =
                    Some(next_utf8(&mut iter, "invalid_request").map_err(|_| C::InvalidRequest)?);
            }
            "--format" if !options.saw_format => {
                let value =
                    next_utf8(&mut iter, "invalid_request").map_err(|_| C::InvalidRequest)?;
                options.format = value.parse().map_err(|_| C::InvalidRequest)?;
                options.saw_format = true;
            }
            "--output" if options.output.is_none() => {
                options.output = Some(std::path::PathBuf::from(
                    iter.next().ok_or(C::InvalidRequest)?,
                ));
            }
            "--timeout" if !options.saw_timeout => {
                let value =
                    next_utf8(&mut iter, "invalid_request").map_err(|_| C::InvalidRequest)?;
                options.timeout = value
                    .parse::<u64>()
                    .ok()
                    .filter(|value| (5..=60).contains(value))
                    .ok_or(C::InvalidRequest)?;
                options.saw_timeout = true;
            }
            "--token" | "--root" | "--session-id" | "--pretty" | "--force" | "--wake" => {
                return Err(C::InvalidRequest);
            }
            _ => return Err(C::InvalidRequest),
        }
    }
    let to = options.to.as_deref().ok_or(C::InvalidRequest)?;
    terminal_snapshot_renderer::validate_target_syntax(to).map_err(|_| C::InvalidRequest)?;
    match (options.format, options.output.as_deref()) {
        (F::Json, None) => {}
        (F::Png, Some(path)) => validate_snapshot_output_path(path)?,
        _ => return Err(C::InvalidRequest),
    }
    Ok(options)
}

pub(crate) async fn terminal_snapshot_command(args: Vec<OsString>) -> i32 {
    use terminal_snapshot_renderer::TerminalSnapshotReasonCode as C;

    let result = match parse_terminal_snapshot_options(args) {
        Ok(options) => execute_terminal_snapshot(options).await,
        Err(error) => Err(error),
    };
    match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!(
                "terminal_snapshot_error code={} detail={}",
                error.as_str(),
                error.detail()
            );
            if error == C::OutputFailed {
                let _ = std::io::stderr().flush();
            }
            1
        }
    }
}

async fn execute_terminal_snapshot(
    options: TerminalSnapshotOptions,
) -> Result<(), terminal_snapshot_renderer::TerminalSnapshotReasonCode> {
    use terminal_snapshot_renderer::{
        decode_api_error, decode_api_success, to_ascii_json, TerminalSnapshotApiRequest,
        TerminalSnapshotFormat as F, TerminalSnapshotPayload, TerminalSnapshotReasonCode as C,
        API_VERSION, MAX_ERROR_BYTES, MAX_REQUEST_BYTES, MAX_TRANSPORT_BYTES,
    };

    let base = validated_snapshot_api_url()?;
    let endpoint = base
        .join("api/v1/terminal-snapshot")
        .map_err(|_| C::ResponseUnavailable)?;
    let request_id = Uuid::new_v4().to_string();
    let request = TerminalSnapshotApiRequest {
        api_version: API_VERSION.to_string(),
        request_id: request_id.clone(),
        to: options.to.clone().ok_or(C::InvalidRequest)?,
        format: options.format,
    };
    request.validate().map_err(|_| C::InvalidRequest)?;
    let body = to_ascii_json(&request, MAX_REQUEST_BYTES).map_err(|_| C::InvalidRequest)?;
    let deadline = Instant::now() + Duration::from_secs(options.timeout);
    let client = snapshot_http_client()?;
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or(C::SnapshotTimeout)?;
    let response = tokio::time::timeout(
        remaining,
        client
            .post(endpoint)
            .header(
                AUTHORIZATION,
                authorization_value().map_err(|_| C::ResponseUnavailable)?,
            )
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT_ENCODING, "identity")
            .body(body)
            .send(),
    )
    .await
    .map_err(|_| C::SnapshotTimeout)?
    .map_err(|_| C::ResponseUnavailable)?;
    validate_snapshot_response_headers(&response)?;
    let status = response.status().as_u16();
    let cap = if status == 200 {
        MAX_TRANSPORT_BYTES
    } else {
        MAX_ERROR_BYTES
    };
    let bytes = read_snapshot_response_body(response, cap, deadline).await?;
    if status != 200 {
        return Err(decode_api_error(&bytes, status)
            .map_err(|_| C::ResponseUnavailable)?
            .error);
    }
    let success = decode_api_success(&bytes, &request_id, &request.to, request.format)
        .map_err(|_| C::ResponseUnavailable)?;
    drop(bytes);
    if Instant::now() >= deadline {
        return Err(C::SnapshotTimeout);
    }
    match (request.format, success.result) {
        (F::Json, TerminalSnapshotPayload::Json { snapshot }) => {
            let line = to_ascii_json(&snapshot, MAX_TRANSPORT_BYTES)
                .map_err(|_| C::ResponseUnavailable)?;
            if Instant::now() >= deadline {
                return Err(C::SnapshotTimeout);
            }
            write_snapshot_stdout(&line)
        }
        (F::Png, TerminalSnapshotPayload::Png { metadata, png }) => {
            if Instant::now() >= deadline {
                return Err(C::SnapshotTimeout);
            }
            let output = options.output.as_deref().ok_or(C::InvalidRequest)?;
            let file = create_snapshot_output(output)?;
            file.write_and_verify(&png)?;
            let line = to_ascii_json(&metadata, MAX_REQUEST_BYTES).map_err(|_| C::OutputFailed)?;
            if Instant::now() >= deadline {
                return Err(C::SnapshotTimeout);
            }
            write_snapshot_stdout(&line)
        }
        _ => Err(C::ResponseUnavailable),
    }
}

fn snapshot_http_client(
) -> Result<reqwest::Client, terminal_snapshot_renderer::TerminalSnapshotReasonCode> {
    use terminal_snapshot_renderer::TerminalSnapshotReasonCode as C;

    reqwest::Client::builder()
        .no_proxy()
        .retry(reqwest::retry::never())
        .redirect(reqwest::redirect::Policy::none())
        .referer(false)
        .build()
        .map_err(|_| C::ResponseUnavailable)
}

fn validated_snapshot_api_url(
) -> Result<reqwest::Url, terminal_snapshot_renderer::TerminalSnapshotReasonCode> {
    let raw = api_url()
        .map_err(|_| terminal_snapshot_renderer::TerminalSnapshotReasonCode::ResponseUnavailable)?;
    validate_snapshot_api_url(&raw)
}

fn validate_snapshot_api_url(
    raw: &str,
) -> Result<reqwest::Url, terminal_snapshot_renderer::TerminalSnapshotReasonCode> {
    use terminal_snapshot_renderer::TerminalSnapshotReasonCode as C;

    let url = reqwest::Url::parse(raw).map_err(|_| C::ResponseUnavailable)?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(C::ResponseUnavailable);
    }
    Ok(url)
}

fn validate_snapshot_response_headers(
    response: &reqwest::Response,
) -> Result<(), terminal_snapshot_renderer::TerminalSnapshotReasonCode> {
    use terminal_snapshot_renderer::{
        TerminalSnapshotReasonCode as C, MAX_ERROR_BYTES, MAX_TRANSPORT_BYTES,
    };

    let headers = response.headers();
    if !one_header_equals(headers, CONTENT_TYPE, "application/json; charset=utf-8")
        || !one_header_equals(headers, CACHE_CONTROL, "no-store")
        || !one_header_equals(headers, PRAGMA, "no-cache")
    {
        return Err(C::ResponseUnavailable);
    }
    let encodings: Vec<_> = headers.get_all(CONTENT_ENCODING).iter().collect();
    if encodings.len() > 1
        || encodings
            .first()
            .is_some_and(|value| !value.as_bytes().eq_ignore_ascii_case(b"identity"))
    {
        return Err(C::ResponseUnavailable);
    }
    let lengths: Vec<_> = headers.get_all(CONTENT_LENGTH).iter().collect();
    if lengths.len() > 1 {
        return Err(C::ResponseUnavailable);
    }
    if let Some(length) = lengths.first() {
        let length = length
            .to_str()
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .ok_or(C::ResponseUnavailable)?;
        let cap = if response.status().as_u16() == 200 {
            MAX_TRANSPORT_BYTES
        } else {
            MAX_ERROR_BYTES
        };
        if length > cap {
            return Err(C::ResponseUnavailable);
        }
    }
    Ok(())
}

fn one_header_equals(
    headers: &reqwest::header::HeaderMap,
    name: reqwest::header::HeaderName,
    expected: &str,
) -> bool {
    let values: Vec<_> = headers.get_all(name).iter().collect();
    values.len() == 1
        && values[0]
            .to_str()
            .is_ok_and(|actual| actual.eq_ignore_ascii_case(expected))
}

async fn read_snapshot_response_body(
    mut response: reqwest::Response,
    cap: usize,
    deadline: Instant,
) -> Result<Vec<u8>, terminal_snapshot_renderer::TerminalSnapshotReasonCode> {
    use terminal_snapshot_renderer::TerminalSnapshotReasonCode as C;

    let claimed = response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok());
    let mut bytes = Vec::with_capacity(claimed.unwrap_or(0).min(cap));
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or(C::SnapshotTimeout)?;
        let chunk = tokio::time::timeout(remaining, response.chunk())
            .await
            .map_err(|_| C::SnapshotTimeout)?
            .map_err(|_| C::ResponseUnavailable)?;
        let Some(chunk) = chunk else {
            break;
        };
        if bytes
            .len()
            .checked_add(chunk.len())
            .is_none_or(|size| size > cap)
        {
            return Err(C::ResponseUnavailable);
        }
        bytes.extend_from_slice(&chunk);
    }
    if claimed.is_some_and(|claimed| claimed != bytes.len()) {
        return Err(C::ResponseUnavailable);
    }
    Ok(bytes)
}

fn write_snapshot_stdout(
    bytes: &[u8],
) -> Result<(), terminal_snapshot_renderer::TerminalSnapshotReasonCode> {
    use terminal_snapshot_renderer::TerminalSnapshotReasonCode as C;

    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(bytes)
        .and_then(|_| stdout.write_all(b"\n"))
        .and_then(|_| stdout.flush())
        .map_err(|_| C::OutputFailed)
}

#[cfg(test)]
mod tests;
