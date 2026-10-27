use crate::config::HelperResult;
pub(crate) const PTY_RESPONSE_MAX_BYTES: usize = 16 * 1024;
use serde::{Deserialize, Serialize};
use uuid::{Uuid, Version};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PtyResult {
    version: u32,
    injection_id: String,
    #[serde(default)]
    op_id: Option<String>,
    #[serde(default)]
    sender: Option<String>,
    #[serde(default)]
    target: Option<String>,
    pub(crate) status: PtyStatus,
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
    pub(crate) reason: Option<PtyReason>,
}

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PtyStatus {
    Queued,
    Actuating,
    Injected,
    Rejected,
    Indeterminate,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PtyReason {
    pub(crate) code: String,
    detail: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ApiErrorResponse {
    api_version: String,
    error: String,
    detail: String,
}

pub(crate) fn parse_api_error_code(bytes: &[u8]) -> HelperResult<String> {
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

pub(crate) fn http_error_code(prefix: &str, status: reqwest::StatusCode, bytes: &[u8]) -> String {
    match parse_api_error_code(bytes) {
        Ok(code) => format!("{prefix}_http_{}_{code}", status.as_u16()),
        Err(_) => format!("{prefix}_http_{}_invalid_server_response", status.as_u16()),
    }
}

pub(crate) fn canonical_uuid_v4(value: &str) -> bool {
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
pub(crate) fn canonical_timestamp(value: &str) -> bool {
    canonical_timestamp_millis(value).is_some()
}

/// Canonical agent FQN (`<project>:room-<n>-<team>/<agent>`; the legacy `wg-` form is still
/// accepted until phase 2 (#1615) retires it). Delegates to the shared room-aware validator so
/// the helper cannot drift from the canonical identity grammar again (#2361).
pub(crate) fn canonical_agent_fqn(value: &str) -> bool {
    terminal_snapshot_renderer::validate_wg_fqn(value).is_ok()
}

pub(crate) fn sha256_hex(input: &[u8]) -> String {
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
    for chunk in padded.as_chunks::<64>().0 {
        let mut words = [0_u32; 64];
        for (index, bytes) in chunk.as_chunks::<4>().0.iter().enumerate() {
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

pub(crate) fn safe_reason_detail(code: &str) -> Option<&'static str> {
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
        "sender_not_coordinator" => "The sender is not the verified workgroup orchestrator.",
        "root_identity_invalid" => "The live Root Agent identity could not be verified.",
        "target_not_member" => "The target is not a verified member of the sender workgroup.",
        "target_is_coordinator" => "An orchestrator cannot target an orchestrator on this route.",
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

pub(crate) struct ExpectedPtyResult<'a> {
    pub(crate) target: &'a str,
    pub(crate) payload_bytes: u64,
    pub(crate) payload_sha256: &'a str,
}

pub(crate) async fn read_capped_response(mut response: reqwest::Response) -> HelperResult<Vec<u8>> {
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

pub(crate) fn parse_result(
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
