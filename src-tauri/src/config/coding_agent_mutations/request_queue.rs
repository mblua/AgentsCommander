use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::settings::{AgentConfig, AppSettings};

use super::agent_op::{apply_coding_agent_op, outcome_from_written, CodingAgentOp};

/// Subdirectory of the config dir holding pending coding-agent mutation requests.
pub const CODING_AGENT_REQUESTS_DIR: &str =
    crate::config::instance_artifacts::CODING_AGENT_REQUESTS_DIR_NAME;
/// Results subdirectory (under the requests dir) the CLI polls for outcomes.
pub const RESULTS_SUBDIR: &str = "results";
/// A request older than this (poller `now` minus the CLI `createdAtMs`) is
/// rejected as expired, so a stale request cannot apply on a much later GUI boot.
pub const REQUEST_EXPIRY_MS: u64 = 300_000;

/// The request file the CLI drops for the poller (daemon path).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodingAgentRequest {
    pub request_id: String,
    pub created_at_ms: u64,
    pub op: CodingAgentOp,
}

/// The result file the poller writes and the CLI polls for.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodingAgentResult {
    pub request_id: String,
    pub ok: bool,
    /// #786: `"add"|"update"|"remove"` on success. Mirrored into the daemon-path
    /// stdout so the JSON shape matches the direct (GUI-closed) path exactly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed_id: Option<String>,
}

/// Disposition of a single processed request, returned by the pure handler so
/// the poller knows whether to swap in-memory state and emit an event.
#[derive(Debug, Clone, PartialEq)]
pub enum RequestDisposition {
    /// The request file was gone before we could claim it (a timed-out CLI
    /// cancelled it). Nothing applied, no result written; caller must NOT swap.
    Skipped,
    /// A result file was written but settings were NOT mutated/persisted
    /// (expired, malformed, validation error, or save failure). Do NOT swap.
    Rejected,
    /// Settings were mutated AND saved; a success result was written. The caller
    /// SHOULD swap in-memory state and emit `coding_agent_settings_updated`.
    Applied {
        op: &'static str,
        agent_id: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// Queue transport helpers (daemon path). All writes are atomic temp+rename.
// ---------------------------------------------------------------------------

/// Atomically write a request into `dir` as `<requestId>.json`. Returns the
/// final path (used by the CLI to cancel on timeout).
pub fn write_coding_agent_request(dir: &Path, req: &CodingAgentRequest) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("Failed to create coding-agent-requests dir: {e}"))?;
    let final_path = dir.join(format!("{}.json", req.request_id));
    let tmp_path = dir.join(format!("{}.json.tmp", req.request_id));
    let json = serde_json::to_string_pretty(req)
        .map_err(|e| format!("Failed to serialize request: {e}"))?;
    std::fs::write(&tmp_path, json)
        .map_err(|e| format!("Failed to write request temp file: {e}"))?;
    std::fs::rename(&tmp_path, &final_path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        format!("Failed to finalize request file: {e}")
    })?;
    Ok(final_path)
}

/// #786 R4: the result writer OWNS the results dir (`create_dir_all` first) so
/// the very first result on a fresh config dir cannot fail for a missing dir.
pub fn write_coding_agent_result(
    results_dir: &Path,
    res: &CodingAgentResult,
) -> Result<(), String> {
    std::fs::create_dir_all(results_dir)
        .map_err(|e| format!("Failed to create results dir: {e}"))?;
    let final_path = results_dir.join(format!("{}.json", res.request_id));
    let tmp_path = results_dir.join(format!("{}.json.tmp", res.request_id));
    let json = serde_json::to_string_pretty(res)
        .map_err(|e| format!("Failed to serialize result: {e}"))?;
    std::fs::write(&tmp_path, json)
        .map_err(|e| format!("Failed to write result temp file: {e}"))?;
    std::fs::rename(&tmp_path, &final_path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        format!("Failed to finalize result file: {e}")
    })?;
    Ok(())
}

/// Read a result file if present (used by the CLI while polling).
pub fn read_coding_agent_result(results_dir: &Path, request_id: &str) -> Option<CodingAgentResult> {
    let path = results_dir.join(format!("{request_id}.json"));
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// #786 R5: expiry with a saturating subtraction. A `created_at_ms` in the
/// future (wall clock stepped back between the CLI write and the poller read)
/// yields age 0, so a fresh request is never wrongly rejected as expired.
pub fn is_expired(created_at_ms: u64, now_ms: u64) -> bool {
    now_ms.saturating_sub(created_at_ms) > REQUEST_EXPIRY_MS
}

pub(super) fn processing_path_for(request_path: &Path) -> PathBuf {
    let mut s = request_path.as_os_str().to_os_string();
    s.push(".processing");
    PathBuf::from(s)
}

/// #786 R8: process ONE request file end to end. PURE of Tauri:
/// `settings` is the candidate to mutate and `save` persists it. Order (R2):
/// claim (atomic rename to `.processing`) -> parse -> expiry -> apply -> write
/// result -> delete `.processing` LAST. Path-parameterized for tempdir tests.
pub fn process_coding_agent_request(
    request_path: &Path,
    results_dir: &Path,
    now_ms: u64,
    settings: &mut AppSettings,
    save: &mut dyn FnMut(&AppSettings) -> Result<AppSettings, String>,
) -> RequestDisposition {
    // Claim: an atomic rename on the same volume. ENOENT means a timed-out CLI
    // already removed the request; never apply it.
    let processing_path = processing_path_for(request_path);
    if std::fs::rename(request_path, &processing_path).is_err() {
        return RequestDisposition::Skipped;
    }
    // The CLI polls `results/<stem>.json`, where `<stem>` is the request-file
    // stem (also the requestId it generated).
    let request_id = request_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    let disposition = process_claimed(
        &processing_path,
        results_dir,
        &request_id,
        now_ms,
        settings,
        save,
    );
    // Delete the claim file LAST, after the result is durably written.
    let _ = std::fs::remove_file(&processing_path);
    disposition
}

fn process_claimed(
    processing_path: &Path,
    results_dir: &Path,
    request_id: &str,
    now_ms: u64,
    settings: &mut AppSettings,
    save: &mut dyn FnMut(&AppSettings) -> Result<AppSettings, String>,
) -> RequestDisposition {
    let content = match std::fs::read_to_string(processing_path) {
        Ok(c) => c,
        Err(e) => {
            write_reject(
                results_dir,
                request_id,
                format!("failed to read request: {e}"),
            );
            return RequestDisposition::Rejected;
        }
    };
    let request: CodingAgentRequest = match serde_json::from_str(&content) {
        Ok(r) => r,
        Err(e) => {
            write_reject(results_dir, request_id, format!("malformed request: {e}"));
            return RequestDisposition::Rejected;
        }
    };
    if is_expired(request.created_at_ms, now_ms) {
        write_reject(results_dir, request_id, "request expired".to_string());
        return RequestDisposition::Rejected;
    }
    match apply_coding_agent_op(settings, &request.op) {
        Ok(outcome) => {
            let written = match save(settings) {
                Ok(written) => written,
                Err(e) => {
                    write_reject(
                        results_dir,
                        request_id,
                        format!("failed to persist settings: {e}"),
                    );
                    return RequestDisposition::Rejected;
                }
            };
            let outcome = match outcome_from_written(outcome, &written) {
                Ok(outcome) => outcome,
                Err(error) => {
                    write_reject(results_dir, request_id, error);
                    return RequestDisposition::Rejected;
                }
            };
            *settings = written;
            let agent_id = outcome
                .agent
                .as_ref()
                .map(|a| a.id.clone())
                .or_else(|| outcome.removed_id.clone());
            let _ = write_coding_agent_result(
                results_dir,
                &CodingAgentResult {
                    request_id: request_id.to_string(),
                    ok: true,
                    op: Some(outcome.op.to_string()),
                    error: None,
                    agent: outcome.agent,
                    removed_id: outcome.removed_id,
                },
            );
            RequestDisposition::Applied {
                op: outcome.op,
                agent_id,
            }
        }
        Err(e) => {
            write_reject(results_dir, request_id, e);
            RequestDisposition::Rejected
        }
    }
}

fn write_reject(results_dir: &Path, request_id: &str, error: String) {
    let _ = write_coding_agent_result(
        results_dir,
        &CodingAgentResult {
            request_id: request_id.to_string(),
            ok: false,
            op: None,
            error: Some(error),
            agent: None,
            removed_id: None,
        },
    );
}
